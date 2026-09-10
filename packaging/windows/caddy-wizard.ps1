<#
.SYNOPSIS
    Guided, hardened Caddy reverse-proxy setup for Breeze Core on Windows.

.DESCRIPTION
    Downloads the official Caddy binary, renders a hardened Caddyfile (automatic
    HTTPS via Let's Encrypt, transport-security headers, LAN-only admin gate, and
    the X-Forwarded-For OVERWRITE the exposure runbook requires), rebinds the
    Breeze Core service to loopback behind the proxy, registers Caddy as a
    Windows service, and opens 80/443. Optionally installs the fail2ban-style
    tripwire.

    Use -DryRun to preview everything - the settings, the rendered Caddyfile and
    every command - without downloading, writing services or touching the
    firewall.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File caddy-wizard.ps1 -Domain breeze.example.com -Email you@example.com

.EXAMPLE
    powershell -File caddy-wizard.ps1 -Domain breeze.example.com -Email you@example.com -SetupTripwire -DryRun
#>
[CmdletBinding()]
param(
    [string]$Domain = '',
    [string]$Email = '',
    [string]$Upstream = '127.0.0.1:8420',
    [string[]]$LanCidr = @('192.168.0.0/16', '10.0.0.0/8', '172.16.0.0/12', '127.0.0.1/8'),

    [string]$InstallDir = "$env:ProgramFiles\Breeze Core",
    [string]$DataDir    = "$env:ProgramData\breeze-core",
    [string]$CaddyDir   = "$env:ProgramFiles\Breeze Core\caddy",
    [string]$CaddyService = 'BreezeCaddy',

    [switch]$KeepLanBind,     # don't rebind Breeze Core to loopback (leave it LAN-exposed too)
    [switch]$SetupTripwire,   # also install the log-watching IP banner
    [switch]$DryRun,
    [string]$Nssm = ''
)

$ErrorActionPreference = 'Stop'
function Info($m) { Write-Host "[caddy] $m" }
function Warn($m) { Write-Host "[caddy] WARNING: $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "[caddy] ERROR: $m" -ForegroundColor Red; exit 1 }
function Plan($m) { Write-Host "[caddy] would: $m" -ForegroundColor Cyan }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = New-Object Security.Principal.WindowsPrincipal($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Die "Run this from an elevated (Administrator) PowerShell."
    }
}
function Resolve-Nssm {
    $cands = @($Nssm, (Join-Path $InstallDir 'nssm.exe'), (Join-Path $PSScriptRoot 'vendor\nssm.exe'), (Join-Path $PSScriptRoot 'nssm.exe')) |
        Where-Object { $_ -and (Test-Path $_) }
    if ($cands) { return $cands[0] }
    $cmd = Get-Command nssm.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    Die "nssm.exe not found. Pass -Nssm <path> or run fetch-vendor.ps1."
}

if (-not $DryRun) { Assert-Admin }

# --- Collect settings ------------------------------------------------------
if (-not $Domain) { $Domain = Read-Host "Public domain for Breeze Core (e.g. breeze.example.com)" }
if (-not $Domain) { Die "A domain is required." }
if (-not $Email)  { $Email  = Read-Host "Email for Let's Encrypt (expiry notices; ACME account)" }
if (-not $Email)  { Die "An ACME email is required." }

$caddyExe   = Join-Path $CaddyDir 'caddy.exe'
$caddyData  = Join-Path $DataDir 'caddy'          # cert/key storage (in ProgramData)
$caddyfile  = Join-Path $DataDir 'Caddyfile'
$accessLog  = Join-Path $DataDir 'logs\caddy-access.json'
$lanTokens  = ($LanCidr -join ' ')

Info "Domain     : $Domain"
Info "ACME email : $Email"
Info "Upstream   : $Upstream (the Breeze Core service, loopback)"
Info "LAN ranges : $lanTokens"
Info "Caddyfile  : $caddyfile"
Info "Cert store : $caddyData"
Info "Access log : $accessLog"

# --- Render the hardened Caddyfile -----------------------------------------
# Notes:
#  * The APP already sends a strict CSP; only transport-security headers are set
#    here, to avoid a second, conflicting policy.
#  * With NO trusted_proxies set, Caddy overwrites X-Forwarded-For with the real
#    peer, so an outsider cannot forge a private "LAN" client. Keep it that way:
#    the LAN-only admin check is only as good as that overwrite.
#  * The live-state stream needs `flush_interval -1`. Server-sent events are one
#    response that never ends, and a proxy that buffers turns a working panel
#    into one that shows nothing and never errors - the failure is invisible
#    from the server's side, which is why it gets its own matcher here.
$caddyfileText = @"
{
	email $Email
	# Keep Caddy's certificates and keys under ProgramData, not in the service
	# account's profile.
	storage file_system {
		root "$caddyData"
	}
}

$Domain {
	encode zstd gzip

	log {
		output file "$accessLog" {
			roll_size 10MiB
			roll_keep 10
		}
		format json
	}

	header {
		Strict-Transport-Security "max-age=31536000; includeSubDomains; preload"
		X-Content-Type-Options "nosniff"
		X-Frame-Options "DENY"
		Referrer-Policy "no-referrer"
		Cross-Origin-Opener-Policy "same-origin"
		-Server
	}

	# Admin endpoints: LAN-only, the same rule the app enforces. The proxy's 403
	# is also what the tripwire watches for.
	@admin path /api/auth/enroll/approve* /api/auth/devices*
	handle @admin {
		route {
			@notlan not remote_ip $lanTokens
			respond @notlan 403
			reverse_proxy $Upstream {
				header_up X-Forwarded-For {remote_host}
				header_up X-Real-IP {remote_host}
			}
		}
	}

	# Server-sent events: no buffering, no timeout. Without flush_interval -1
	# the panel's live state silently never arrives.
	@stream path /api/units/stream
	handle @stream {
		reverse_proxy $Upstream {
			header_up X-Forwarded-For {remote_host}
			header_up X-Real-IP {remote_host}
			flush_interval -1
		}
	}

	# Everything else -> the app. X-Forwarded-For is the ONLY header the server
	# reads for the client address -- X-Real-IP is ignored by it entirely, and is
	# set here only because other tooling reads it. Caddy would add XFF anyway,
	# but naming it keeps the load-bearing header visible in the file:
	# with NO trusted_proxies configured every client is untrusted, so a
	# client-sent (forged) XFF is dropped and replaced with the real address.
	# Do NOT add public ranges to trusted_proxies.
	reverse_proxy $Upstream {
		header_up X-Forwarded-For {remote_host}
		header_up X-Real-IP {remote_host}
	}
}
"@

if ($DryRun) {
    Write-Host ""
    Write-Host "----- Caddyfile ($caddyfile) -----" -ForegroundColor Cyan
    Write-Host $caddyfileText
    Write-Host "----- end Caddyfile -----" -ForegroundColor Cyan
    Write-Host ""
    $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
    Plan "download Caddy -> $caddyExe  (https://caddyserver.com/api/download?os=windows&arch=$arch)"
    Plan "write the Caddyfile above to $caddyfile"
    if (-not $KeepLanBind) { Plan "rebind Breeze Core to 127.0.0.1 + --behind-proxy (install-service.ps1 -Reconfigure -BehindProxy)" }
    Plan "register service '$CaddyService' -> caddy run --config `"$caddyfile`""
    Plan "open inbound TCP 80,443 (Any profile)"
    if ($SetupTripwire) { Plan "install the tripwire watcher (breeze-tripwire.ps1) as service 'BreezeTripwire'" }
    Write-Host ""
    Info "Dry run only - nothing was changed."
    return
}

$nssm = Resolve-Nssm

# --- Download Caddy --------------------------------------------------------
New-Item -ItemType Directory -Force -Path $CaddyDir, $caddyData, (Split-Path $accessLog) | Out-Null
if (-not (Test-Path $caddyExe)) {
    $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
    $url = "https://caddyserver.com/api/download?os=windows&arch=$arch"
    Info "Downloading Caddy ($arch) from $url"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $caddyExe -TimeoutSec 120
} else { Info "Caddy already present at $caddyExe" }

# --- Write the Caddyfile & validate ----------------------------------------
Set-Content -Path $caddyfile -Value $caddyfileText -Encoding UTF8
Info "Validating Caddyfile"
& $caddyExe validate --config $caddyfile --adapter caddyfile
if ($LASTEXITCODE -ne 0) { Die "Caddy rejected the config - see the message above." }

# --- Rebind Breeze Core behind the proxy -----------------------------------
if (-not $KeepLanBind) {
    $svcScript = Join-Path $PSScriptRoot 'install-service.ps1'
    if (Test-Path $svcScript) {
        Info "Rebinding Breeze Core to loopback behind the proxy"
        & powershell -NoProfile -ExecutionPolicy Bypass -File $svcScript -Action Reconfigure -BehindProxy -InstallDir $InstallDir -DataDir $DataDir -Nssm $nssm
    } else {
        Warn "install-service.ps1 not found next to this wizard - rebind Breeze Core to 127.0.0.1 manually."
    }
}

# --- Register Caddy as a service -------------------------------------------
if (Get-Service $CaddyService -ErrorAction SilentlyContinue) {
    Info "Reconfiguring existing '$CaddyService'"
    & $nssm stop $CaddyService confirm | Out-Null
} else {
    Info "Registering service '$CaddyService'"
    & $nssm install $CaddyService $caddyExe run --config $caddyfile --adapter caddyfile | Out-Null
}
& $nssm set $CaddyService Application $caddyExe | Out-Null
& $nssm set $CaddyService AppParameters "run --config `"$caddyfile`" --adapter caddyfile" | Out-Null
& $nssm set $CaddyService AppDirectory $CaddyDir | Out-Null
& $nssm set $CaddyService DisplayName "Breeze Caddy (reverse proxy)" | Out-Null
& $nssm set $CaddyService Start SERVICE_AUTO_START | Out-Null
& $nssm set $CaddyService AppStdout (Join-Path $DataDir 'logs\caddy.log') | Out-Null
& $nssm set $CaddyService AppStderr (Join-Path $DataDir 'logs\caddy.log') | Out-Null
& $nssm set $CaddyService AppExit Default Restart | Out-Null
& $nssm start $CaddyService | Out-Null
Info "Caddy started."

# --- Firewall (public 80/443) ----------------------------------------------
Get-NetFirewallRule -DisplayName "Breeze Caddy (HTTP/HTTPS)" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName "Breeze Caddy (HTTP/HTTPS)" -Direction Inbound -Action Allow `
    -Protocol TCP -LocalPort 80,443 -Profile Any | Out-Null
Info "Opened inbound TCP 80,443."

# --- Optional tripwire ------------------------------------------------------
if ($SetupTripwire) {
    $tw = Join-Path $PSScriptRoot 'breeze-tripwire.ps1'
    if (Test-Path $tw) {
        Info "Installing the tripwire watcher as service 'BreezeTripwire'"
        & $nssm install BreezeTripwire powershell.exe "-NoProfile -ExecutionPolicy Bypass -File `"$tw`" -AccessLog `"$accessLog`" -LanCidr $lanTokens" | Out-Null
        & $nssm set BreezeTripwire DisplayName "Breeze Tripwire (fail2ban-style IP banner)" | Out-Null
        & $nssm set BreezeTripwire Start SERVICE_AUTO_START | Out-Null
        & $nssm set BreezeTripwire AppStdout (Join-Path $DataDir 'logs\tripwire.log') | Out-Null
        & $nssm set BreezeTripwire AppStderr (Join-Path $DataDir 'logs\tripwire.log') | Out-Null
        & $nssm set BreezeTripwire AppExit Default Restart | Out-Null
        & $nssm start BreezeTripwire | Out-Null
        Info "Tripwire running (bans on repeated 4xx / any admin 403)."
    } else {
        Warn "breeze-tripwire.ps1 not found next to this wizard - skipping."
    }
}

Write-Host ""
Info "Done. Point $Domain's DNS at this host (A/AAAA), open ports 80/443 at your router,"
Info "then browse to https://$Domain - Caddy fetches the certificate automatically."
