<#
.SYNOPSIS
    fail2ban-style IP banner for Breeze Core behind Caddy on Windows.

.DESCRIPTION
    Carried over from the Python line unchanged, deliberately: it watches Caddy's
    log and drives Windows Firewall, so it knows nothing about the server behind
    it and had nothing to gain from the rewrite. This is also the one component
    here that can lock somebody out of their own machine, so a gratuitous rewrite
    could only have made it worse.

    Tails Caddy's JSON access log and bans abusive source IPs with Windows
    Firewall block rules - the Windows analogue of the fail2ban jails the Linux
    exposure runbook sets up. Two triggers:

      * general : too many 4xx/5xx from one IP inside a window  -> ban
      * tripwire: ANY 403 on an admin endpoint (/api/auth/enroll/approve,
                  /api/auth/devices) is hostile by definition       -> instant ban

    LAN ranges are never banned (so you can't lock yourself out). Bans expire
    automatically. Designed to run as a service (see caddy-wizard.ps1
    -SetupTripwire); it needs rights to manage the firewall, so run it as
    LocalSystem (NSSM's default) rather than LOCAL SERVICE.

.EXAMPLE
    powershell -File breeze-tripwire.ps1 -AccessLog "C:\ProgramData\breeze-core\logs\caddy-access.json"
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AccessLog,
    [string[]]$LanCidr = @('192.168.0.0/16', '10.0.0.0/8', '172.16.0.0/12', '127.0.0.1/8'),

    [int]$MaxRetry = 5,          # 4xx/5xx hits before a general ban
    [int]$FindWindowSec = 600,   # ...within this rolling window
    [int]$BanSec = 86400,        # general ban duration (24h)
    [int]$TripwireBanSec = 604800, # admin-tripwire ban duration (7d)
    [int]$PollSec = 5,
    [string]$AdminPathRegex = '^/api/auth/(enroll/approve|devices)',
    [string]$StatePath = ''
)

$ErrorActionPreference = 'Continue'
$countedStatuses = @(400, 401, 403, 404, 405, 422, 429)
if (-not $StatePath) { $StatePath = Join-Path (Split-Path $AccessLog) 'tripwire-state.json' }

function Log($m) { Write-Host ("[tripwire] {0}  {1}" -f (Get-Date).ToString('s'), $m) }

# --- CIDR membership (IPv4) -----------------------------------------------
function Test-InCidr {
    param([string]$Ip, [string]$Cidr)
    $parts = $Cidr.Split('/')
    if ($parts.Count -ne 2) { return $false }
    try {
        $addr = ([System.Net.IPAddress]::Parse($Ip)).GetAddressBytes()
        $net  = ([System.Net.IPAddress]::Parse($parts[0])).GetAddressBytes()
    } catch { return $false }
    if ($addr.Length -ne $net.Length) { return $false }   # only compare same family
    $bits = [int]$parts[1]
    for ($i = 0; $i -lt $addr.Length; $i++) {
        if ($bits -le 0) { break }
        $take = [Math]::Min(8, $bits)
        $mask = [byte](0xFF -shl (8 - $take))
        if (($addr[$i] -band $mask) -ne ($net[$i] -band $mask)) { return $false }
        $bits -= 8
    }
    return $true
}
function Test-IsLan { param([string]$Ip) foreach ($c in $LanCidr) { if (Test-InCidr $Ip $c) { return $true } } return $false }

# --- State ----------------------------------------------------------------
$script:hits = @{}   # ip -> [datetime[]] of recent counted hits
$script:bans = @{}   # ip -> expiry (datetime)

function Load-State {
    if (Test-Path $StatePath) {
        try {
            $s = Get-Content $StatePath -Raw | ConvertFrom-Json
            foreach ($p in $s.bans.PSObject.Properties) { $script:bans[$p.Name] = [datetime]$p.Value }
        } catch { }
    }
    # Reconcile with any BreezeBan-* rules that already exist.
    Get-NetFirewallRule -DisplayName 'BreezeBan *' -ErrorAction SilentlyContinue | ForEach-Object {
        $ip = $_.DisplayName -replace '^BreezeBan ', ''
        if (-not $script:bans.ContainsKey($ip)) { $script:bans[$ip] = (Get-Date).AddSeconds($BanSec) }
    }
}
function Save-State {
    try {
        $obj = @{ bans = @{} }
        foreach ($k in $script:bans.Keys) { $obj.bans[$k] = $script:bans[$k].ToString('o') }
        ($obj | ConvertTo-Json -Depth 4) | Set-Content -Path $StatePath -Encoding UTF8
    } catch { }
}

function Ban-Ip {
    param([string]$Ip, [int]$Seconds, [string]$Why)
    if (Test-IsLan $Ip) { return }                     # never ban the LAN
    if ($script:bans.ContainsKey($Ip)) { return }      # already banned
    $name = "BreezeBan $Ip"
    if (-not (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue)) {
        try {
            New-NetFirewallRule -DisplayName $name -Direction Inbound -Action Block `
                -RemoteAddress $Ip -Profile Any -ErrorAction Stop | Out-Null
        } catch { Log "failed to add firewall rule for ${Ip}: $($_.Exception.Message)"; return }
    }
    $script:bans[$Ip] = (Get-Date).AddSeconds($Seconds)
    $script:hits.Remove($Ip)
    Log "BANNED $Ip for ${Seconds}s - $Why"
    Save-State
}

function Expire-Bans {
    $now = Get-Date
    foreach ($ip in @($script:bans.Keys)) {
        if ($script:bans[$ip] -le $now) {
            Get-NetFirewallRule -DisplayName "BreezeBan $ip" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
            $script:bans.Remove($ip)
            Log "unbanned $ip (expired)"
            Save-State
        }
    }
}

function Get-ClientIp {
    param($req)
    foreach ($f in @('client_ip', 'remote_ip', 'remote_addr')) {
        $v = $req.$f
        if ($v) { return ($v -split ':')[0] }   # strip :port if present
    }
    return $null
}

function Process-Line {
    param([string]$Line)
    if (-not $Line.Trim()) { return }
    try { $o = $Line | ConvertFrom-Json } catch { return }
    if ($null -eq $o.status -or $null -eq $o.request) { return }
    $status = [int]$o.status
    $ip = Get-ClientIp $o.request
    if (-not $ip -or (Test-IsLan $ip)) { return }
    $uri = [string]$o.request.uri

    if ($status -eq 403 -and $uri -match $AdminPathRegex) {
        Ban-Ip $ip $TripwireBanSec "admin tripwire ($uri)"
        return
    }
    if ($countedStatuses -contains $status) {
        $now = Get-Date
        if (-not $script:hits.ContainsKey($ip)) { $script:hits[$ip] = @() }
        $window = $script:hits[$ip] + $now | Where-Object { $_ -gt $now.AddSeconds(-$FindWindowSec) }
        $script:hits[$ip] = @($window)
        if ($script:hits[$ip].Count -ge $MaxRetry) {
            Ban-Ip $ip $BanSec "$($script:hits[$ip].Count) 4xx/5xx in ${FindWindowSec}s"
        }
    }
}

# --- Main loop -------------------------------------------------------------
Log "watching $AccessLog (maxretry=$MaxRetry/${FindWindowSec}s, ban=${BanSec}s, tripwire=${TripwireBanSec}s)"
Log "LAN (never banned): $($LanCidr -join ', ')"
Load-State
[long]$pos = if (Test-Path $AccessLog) { (Get-Item $AccessLog).Length } else { 0 }  # start at tail

while ($true) {
    try {
        if (Test-Path $AccessLog) {
            [long]$len = (Get-Item $AccessLog).Length
            if ($len -lt $pos) { $pos = 0 }             # rotated/truncated
            if ($len -gt $pos) {
                $fs = [System.IO.File]::Open($AccessLog, 'Open', 'Read', 'ReadWrite')
                try {
                    [void]$fs.Seek($pos, 'Begin')
                    $sr = New-Object System.IO.StreamReader($fs)
                    while ($null -ne ($line = $sr.ReadLine())) { Process-Line $line }
                    $pos = $fs.Position
                } finally { $fs.Close() }
            }
        }
        Expire-Bans
    } catch { Log "loop error: $($_.Exception.Message)" }
    Start-Sleep -Seconds $PollSec
}
