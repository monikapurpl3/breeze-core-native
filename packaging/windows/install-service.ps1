<#
.SYNOPSIS
    Install / uninstall Breeze Core as a hardened Windows background service.

.DESCRIPTION
    Registers breeze-core.exe as a Windows service using the bundled NSSM. Runs
    as the low-privilege LOCAL SERVICE account, keeps state in
    %ProgramData%\breeze-core with locked-down ACLs, and (in LAN mode) opens
    only a LocalSubnet firewall rule for the port. This is the Windows analogue
    of the systemd unit the Linux packages install.

    Simpler than the Python line's version of this script, and the difference is
    the whole point of the rewrite: there is no interpreter to find, no
    virtualenv to build and no dependencies to download, so this script needs no
    internet at all. It registers one executable.

    Called by the NSIS installer, but fully usable on its own.

.EXAMPLE
    # LAN-first (default): bind the detected LAN IP, open a LocalSubnet rule
    powershell -ExecutionPolicy Bypass -File install-service.ps1 -Action Install -InstallDir "C:\Program Files\Breeze Core"

.EXAMPLE
    # Behind Caddy: bind loopback, trust the local proxy, no inbound rule
    powershell -File install-service.ps1 -Action Install -BehindProxy

.EXAMPLE
    powershell -File install-service.ps1 -Action Uninstall
#>
[CmdletBinding()]
param(
    [ValidateSet('Install', 'Uninstall', 'Reconfigure')]
    [string]$Action = 'Install',

    [string]$InstallDir = "$env:ProgramFiles\Breeze Core",
    [string]$DataDir    = "$env:ProgramData\breeze-core",
    [string]$ServiceName = 'BreezeCore',

    [string]$BindHost = '',      # empty = auto-detect LAN IP (or 127.0.0.1 with -BehindProxy)
    [int]$Port = 8420,

    [switch]$BehindProxy,        # bind loopback + --behind-proxy + AC_BEHIND_PROXY=1
    [switch]$LockEgress,         # add an outbound "block Internet" rule (best-effort)
    [switch]$NoFirewall,
    [switch]$Purge,              # on uninstall, also delete the data dir (config, tokens, programs)

    [string]$Nssm = ''           # path to nssm.exe; auto-resolved if empty
)

$ErrorActionPreference = 'Stop'
function Info($m)  { Write-Host "[breeze] $m" }
function Warn($m)  { Write-Host "[breeze] WARNING: $m" -ForegroundColor Yellow }
function Die($m)   { Write-Host "[breeze] ERROR: $m" -ForegroundColor Red; exit 1 }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = New-Object Security.Principal.WindowsPrincipal($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Die "Run this from an elevated (Administrator) PowerShell."
    }
}

function Resolve-Nssm {
    param([string]$Hint)
    $cands = @(
        $Hint,
        (Join-Path $InstallDir 'nssm.exe'),
        (Join-Path $PSScriptRoot 'vendor\nssm.exe'),
        (Join-Path $PSScriptRoot 'nssm.exe')
    ) | Where-Object { $_ -and (Test-Path $_) }
    if ($cands) { return $cands[0] }
    $cmd = Get-Command nssm.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    Die "nssm.exe not found. Pass -Nssm <path>, or run fetch-vendor.ps1 to download it."
}

# The address that actually routes off this machine.
#
# NOT "the first non-loopback IPv4", which is what this used to be. On any
# workstation with VMware, Hyper-V or WSL installed that picks a host-only
# address: the machine this was first tested on offered 192.168.11.1,
# 192.168.122.1 and 172.31.160.1 ahead of its real 192.168.1.67, and the
# installer duly bound the service to a virtual adapter nothing on the LAN can
# reach. Nothing errors; the panel is simply unreachable from every device you
# own.
#
# A default gateway is what tells a real network from a virtual one.
function Get-LanIPv4 {
    try {
        $cfg = Get-NetIPConfiguration -ErrorAction Stop |
            Where-Object { $_.IPv4Address -and $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up' } |
            Select-Object -First 1
        if ($cfg) { return @($cfg.IPv4Address)[0].IPAddress }
    } catch { }
    # No gateway anywhere is legitimate (an isolated LAN), so fall back to a
    # configured private address on an adapter that is up, skipping the obvious
    # virtuals by name.
    try {
        $ip = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction Stop |
            Where-Object {
                $_.IPAddress -notlike '127.*' -and $_.IPAddress -notlike '169.254.*' -and
                $_.PrefixOrigin -in 'Dhcp','Manual' -and
                $_.InterfaceAlias -notmatch 'VMware|vEthernet|VirtualBox|Hyper-V|WSL|Loopback'
            } |
            Sort-Object -Property SkipAsSource |
            Select-Object -First 1 -ExpandProperty IPAddress
        if ($ip) { return $ip }
    } catch { }
    return '127.0.0.1'
}

# ---------------------------------------------------------------- Uninstall
function Do-Uninstall {
    Assert-Admin
    $nssm = ''
    try { $nssm = Resolve-Nssm $Nssm } catch { }

    if (Get-Service $ServiceName -ErrorAction SilentlyContinue) {
        Info "Stopping and removing service '$ServiceName'"
        if ($nssm) {
            & $nssm stop $ServiceName confirm | Out-Null
            & $nssm remove $ServiceName confirm | Out-Null
        } else {
            Stop-Service $ServiceName -Force -ErrorAction SilentlyContinue
            sc.exe delete $ServiceName | Out-Null
        }
    } else { Info "Service '$ServiceName' not present" }

    foreach ($rn in @("Breeze Core (LAN)", "Breeze Core egress lockdown")) {
        Get-NetFirewallRule -DisplayName $rn -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    }

    if ($Purge -and (Test-Path $DataDir)) {
        Warn "Purging data dir $DataDir (config, tokens, programs, timers)"
        Remove-Item -Recurse -Force $DataDir
    } else {
        # Kept by default on purpose: a paired V3 unit's token and key cannot be
        # obtained from Midea again, so deleting config.json is not a
        # recoverable action.
        Info "Left data dir intact: $DataDir (use -Purge to remove)"
    }
    Info "Uninstall complete."
}

# ------------------------------------------------------------------ Install
function Do-Install {
    Assert-Admin

    if (-not $BindHost) { $BindHost = if ($BehindProxy) { '127.0.0.1' } else { Get-LanIPv4 } }
    $nssm = Resolve-Nssm $Nssm

    $exe = Join-Path $InstallDir 'breeze-core.exe'
    if (-not (Test-Path $exe)) {
        Die "breeze-core.exe not found in $InstallDir. Point -InstallDir at the installed tree."
    }

    # --- Data dir + hardened ACLs -----------------------------------------
    $logs = Join-Path $DataDir 'logs'
    New-Item -ItemType Directory -Force -Path $DataDir, $logs | Out-Null
    Info "Locking down $DataDir (SYSTEM + Administrators full, LOCAL SERVICE modify)"
    # /inheritance:r drops inherited ACEs - the Windows analogue of chmod 750.
    # LOCAL SERVICE needs Modify rather than Read: the server writes devices.json
    # every time a client enrols, and timers.json as timers fire.
    & icacls "$DataDir" /inheritance:r /grant:r `
        "*S-1-5-18:(OI)(CI)F" `
        "*S-1-5-32-544:(OI)(CI)F" `
        "*S-1-5-19:(OI)(CI)M" | Out-Null

    # --- Service via NSSM --------------------------------------------------
    $appArgs = "serve --host $BindHost --port $Port"
    # --behind-proxy is honoured by the server rather than merely tolerated: it
    # is what makes it read X-Forwarded-For instead of the proxy's own address,
    # and without it every proxied request looks like it came from the LAN and
    # the LAN-only admin checks stop meaning anything.
    if ($BehindProxy) { $appArgs += " --behind-proxy" }

    if (Get-Service $ServiceName -ErrorAction SilentlyContinue) {
        Info "Reconfiguring existing service '$ServiceName'"
        & $nssm stop $ServiceName confirm | Out-Null
    } else {
        Info "Registering service '$ServiceName'"
        & $nssm install $ServiceName $exe $appArgs | Out-Null
    }
    & $nssm set $ServiceName Application $exe | Out-Null
    & $nssm set $ServiceName AppParameters $appArgs | Out-Null
    & $nssm set $ServiceName AppDirectory $InstallDir | Out-Null
    & $nssm set $ServiceName DisplayName "Breeze Core" | Out-Null
    & $nssm set $ServiceName Description "Self-hosted, LAN-first control for Midea air conditioners." | Out-Null
    # Low-privilege built-in account (no password, minimal rights).
    & $nssm set $ServiceName ObjectName "NT AUTHORITY\LocalService" "" | Out-Null
    & $nssm set $ServiceName Start SERVICE_AUTO_START | Out-Null
    # Four stores, not three: the native server also keeps timers.json. Naming
    # each one explicitly rather than relying on AC_CONFIG_DIR keeps this
    # readable in `nssm edit BreezeCore`.
    $envExtra = @(
        "AC_CONFIG=$DataDir\config.json",
        "AC_DEVICES=$DataDir\devices.json",
        "AC_PROGRAMS=$DataDir\programs.json",
        "AC_TIMERS=$DataDir\timers.json"
    )
    if ($BehindProxy) {
        $envExtra += "AC_BEHIND_PROXY=1"
        $envExtra += "AC_ENROLL_LAN_ONLY=1"
    }
    & $nssm set $ServiceName AppEnvironmentExtra @envExtra | Out-Null
    # Logs + rotation; auto-restart on crash.
    & $nssm set $ServiceName AppStdout (Join-Path $logs 'service.log') | Out-Null
    & $nssm set $ServiceName AppStderr (Join-Path $logs 'service.log') | Out-Null
    & $nssm set $ServiceName AppRotateFiles 1 | Out-Null
    & $nssm set $ServiceName AppRotateBytes 1048576 | Out-Null
    & $nssm set $ServiceName AppExit Default Restart | Out-Null
    & $nssm set $ServiceName AppRestartDelay 5000 | Out-Null

    # --- Firewall ----------------------------------------------------------
    if (-not $NoFirewall) {
        Get-NetFirewallRule -DisplayName "Breeze Core (LAN)" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
        if (-not $BehindProxy -and $BindHost -ne '127.0.0.1') {
            Info "Opening inbound TCP $Port from the local subnet only"
            New-NetFirewallRule -DisplayName "Breeze Core (LAN)" -Direction Inbound -Action Allow `
                -Protocol TCP -LocalPort $Port -RemoteAddress LocalSubnet -Profile Any | Out-Null
        } else {
            Info "Behind proxy / loopback bind - no inbound rule (Caddy talks to it on 127.0.0.1)"
        }
        if ($LockEgress) {
            # Best-effort: block the server from Internet-classified addresses,
            # leaving the LAN reachable. Windows decides what counts as
            # "Internet", so verify your units still answer afterwards.
            #
            # Note this blocks cloud pairing too - that is the one feature that
            # deliberately talks to Midea, so run `breeze-core pair` before
            # turning this on, or expect it to fail.
            Get-NetFirewallRule -DisplayName "Breeze Core egress lockdown" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
            New-NetFirewallRule -DisplayName "Breeze Core egress lockdown" -Direction Outbound -Action Block `
                -Program $exe -RemoteAddress Internet -Profile Any | Out-Null
            Warn "Egress lockdown added (blocks '$exe' to Internet). Verify your units are still reachable."
        }
    }

    # --- Start (only once paired) -----------------------------------------
    # The server refuses to start without an api_key and says so; starting it
    # here would just put that message in a log nobody is reading yet.
    if (Test-Path (Join-Path $DataDir 'config.json')) {
        Info "Starting service"
        & $nssm start $ServiceName | Out-Null
        Info "Service '$ServiceName' started, bound to ${BindHost}:$Port"
    } else {
        Warn "No config.json yet - pair your units first, then start the service:"
        Warn "    `"$exe`" pair --out `"$DataDir\config.json`""
        Warn "    nssm start $ServiceName"
    }

    Info "Done. Manage with: nssm start/stop/restart/edit $ServiceName  |  logs: $logs\service.log"
}

switch ($Action) {
    'Install'     { Do-Install }
    'Reconfigure' { Do-Install }
    'Uninstall'   { Do-Uninstall }
}
