<#
.SYNOPSIS
    Install / uninstall Breeze Core as a hardened Windows background service.

.DESCRIPTION
    Registers breeze-core.exe as a Windows service using NSSM. Runs as the
    low-privilege LOCAL SERVICE account, keeps state in
    %ProgramData%\breeze-core with locked-down ACLs, and (in LAN mode) opens
    only a LocalSubnet firewall rule for the port. This is the Windows analogue
    of the systemd unit the Linux packages install.

    NSSM is not packed into the installer. Antivirus products flag installers
    that carry it - malware uses it to persist, so its bytes inside a setup
    program read as a threat - and the installer fetches it instead, with
    -Action FetchNssm: from an existing install, from beside the installer, from
    nssm.cc, or from aspic's mirror, in that order, and whichever it gets must
    match the hashes pinned below. Upgrades and offline installs (put
    nssm-2.24.zip or nssm.exe next to the installer) need no internet.

    Called by the NSIS installer, but fully usable on its own.

.EXAMPLE
    # LAN-first (default): bind the detected LAN IP, open a LocalSubnet rule
    powershell -ExecutionPolicy Bypass -File install-service.ps1 -Action Install -InstallDir "C:\Program Files\Breeze Core"

.EXAMPLE
    # Behind Caddy: bind loopback, trust the local proxy, no inbound rule
    powershell -File install-service.ps1 -Action Install -BehindProxy

.EXAMPLE
    # Change one thing and keep the rest (port, environment, firewall choices)
    powershell -File install-service.ps1 -Action Reconfigure -Port 8431

.EXAMPLE
    powershell -File install-service.ps1 -Action Uninstall
#>
[CmdletBinding()]
param(
    [ValidateSet('Install', 'Uninstall', 'Reconfigure', 'Stop', 'Upgrade', 'Describe',
                 'CheckPort', 'CheckEnv', 'FetchNssm', 'FetchGup',
                 'RegisterUpdateCheck', 'UnregisterUpdateCheck')]
    [string]$Action = 'Install',

    [string]$InstallDir = "$env:ProgramFiles\Breeze Core",
    [string]$DataDir    = "$env:ProgramData\breeze-core",
    [string]$ServiceName = 'BreezeCore',

    [string]$BindHost = '',      # empty = auto-detect LAN IP (or 127.0.0.1 with -BehindProxy)
    [ValidateRange(1, 65535)]
    [int]$Port = 8420,

    [switch]$BehindProxy,        # bind loopback + --behind-proxy + AC_BEHIND_PROXY=1
    [switch]$LockEgress,         # add an outbound "block Internet" rule (best-effort)
    [switch]$NoFirewall,
    [string]$EnvFile = '',       # extra NAME=value lines for the service's environment
    [switch]$NoStart,            # Install/Reconfigure: leave the service stopped
    [switch]$Purge,              # on uninstall, also delete the data dir (config, tokens, programs)

    [string]$Nssm = '',          # path to nssm.exe; auto-resolved if empty

    [switch]$Start,              # Upgrade only: start the service afterwards

    [string]$OutDir = '',        # Describe: where service.ini and service.env go
    [string]$Dest = '',          # FetchNssm/FetchGup: where the verified file is written
    [string]$LocalDir = '',      # FetchNssm/FetchGup: a directory to look in first
    [string]$GupUrl = '',        # FetchGup: where the updater is published
    [string]$GupSha256 = ''      # FetchGup: what it must hash to
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar makes downloads on Windows PowerShell 5.1
# an order of magnitude slower, and nobody sees it inside the installer.
$ProgressPreference = 'SilentlyContinue'
$Bound = $PSBoundParameters
function Info($m)  { Write-Host "[breeze] $m" }
function Warn($m)  { Write-Host "[breeze] WARNING: $m" -ForegroundColor Yellow }
function Die($m)   { Write-Host "[breeze] ERROR: $m" -ForegroundColor Red; exit 1 }

# NSSM 2.24, the last release. nssm.cc serves the zip; aspic mirrors the same
# bytes (packaging/repo/build-repo.sh checks them against the same hash before
# publishing). The zip is pinned so a download can be checked before it is
# opened, and the executable is pinned so a copy found on disk - an existing
# install, or one put beside the installer - is held to the same standard.
$NssmZipSha256 = '727d1e42275c605e0f04aba98095c38a8e1e46def453cdffce42869428aa6743'
$NssmExeSha256 = 'f689ee9af94b00e9e3f0bb072b34caaf207f32dcb4f5782fc9ca351df9a06c97'  # win64/nssm.exe
$NssmUrls = @(
    'https://nssm.cc/release/nssm-2.24.zip',
    'https://aspic.salataputarica.hr.eu.org/windows/vendor/nssm-2.24.zip'
)

# The updater's scheduled task, and the only place it may download from.
$TaskPath = '\Breeze Core\'
$TaskName = 'Check for updates'
$UpdatePrefix = 'https://aspic.salataputarica.hr.eu.org/windows/'

# Environment names the installer owns. They are set from the port, network
# and data-directory choices, so a second copy in the free-form list would at
# best be ignored and at worst quietly undo one of those choices.
$ManagedEnv = @('AC_CONFIG', 'AC_CONFIG_DIR', 'AC_DEVICES', 'AC_PROGRAMS', 'AC_TIMERS',
                'AC_BEHIND_PROXY', 'AC_ENROLL_LAN_ONLY', 'BREEZE_HOST', 'BREEZE_PORT')

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
    Die "nssm.exe not found. Pass -Nssm <path>, or fetch it: install-service.ps1 -Action FetchNssm -Dest `"$InstallDir\nssm.exe`""
}

function Get-Sha256([string]$File) {
    (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()
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

# ------------------------------------------------------ Environment variables
#
# One NAME=value per line; blank lines and lines starting with # are ignored,
# so the installer's page can offer commented-out examples. Returns the problem
# as a string, or the parsed lines.
function Read-EnvLines([string]$File) {
    $out = @()
    $n = 0
    foreach ($raw in (Get-Content -LiteralPath $File)) {
        $n++
        $line = $raw.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        if ($line -notmatch '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
            return "line ${n}: '$line' is not NAME=value (a name is letters, digits and _, not starting with a digit)"
        }
        $name = $Matches[1]
        if ($ManagedEnv -contains $name.ToUpperInvariant()) {
            return "line ${n}: $name is set by the installer from the port, network and data folder choices - leave it out"
        }
        $out += "$name=$($Matches[2])"
    }
    return ,$out
}

# ------------------------------------------------------ What is set up now
#
# Read from where the settings really live - NSSM's registry key and the
# firewall - rather than from anything this script wrote down about itself,
# which `nssm edit` would make stale.
function Get-CurrentSettings {
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName\Parameters"
    $p = Get-ItemProperty $key -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    $argsText = [string]$p.AppParameters
    $s = @{
        Application = [string]$p.Application
        Port        = 8420
        Host        = ''
        BehindProxy = $false
        Firewall    = $true
        Egress      = $false
        Env         = @()
    }
    if ($argsText -match '--port\s+(\d+)') { $s.Port = [int]$Matches[1] }
    if ($argsText -match '--host\s+(\S+)') { $s.Host = $Matches[1] }
    if ($argsText -match '--behind-proxy') { $s.BehindProxy = $true }
    foreach ($e in @($p.AppEnvironmentExtra)) {
        if ($e -and $e -match '^([^=]+)=') {
            if ($ManagedEnv -notcontains $Matches[1].ToUpperInvariant()) { $s.Env += $e }
        }
    }
    $lanRule = Get-NetFirewallRule -DisplayName "Breeze Core (LAN)" -ErrorAction SilentlyContinue
    # Only a LAN bind ever has an inbound rule; on loopback there is nothing
    # to tell "firewall off" from "not needed", so keep the default.
    if ($s.Host -and $s.Host -ne '127.0.0.1' -and -not $s.BehindProxy) { $s.Firewall = [bool]$lanRule }
    $s.Egress = [bool](Get-NetFirewallRule -DisplayName "Breeze Core egress lockdown" -ErrorAction SilentlyContinue)
    return $s
}

function Do-Describe {
    if (-not $OutDir) { Die "-OutDir is required" }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $s = Get-CurrentSettings
    $task = Get-ScheduledTask -TaskPath $TaskPath -TaskName $TaskName -ErrorAction SilentlyContinue
    $b = { param($v) if ($v) { '1' } else { '0' } }
    $ini = @('[service]')
    if ($s) {
        $local = ($s.Host -eq '127.0.0.1') -or $s.BehindProxy
        $ini += "installed=1"
        $ini += "native=$(& $b ([IO.Path]::GetFileName($s.Application) -eq 'breeze-core.exe'))"
        $ini += "port=$($s.Port)"
        $ini += "host=$($s.Host)"
        $ini += "local=$(& $b $local)"
        $ini += "behind_proxy=$(& $b $s.BehindProxy)"
        $ini += "firewall=$(& $b $s.Firewall)"
        $ini += "egress=$(& $b $s.Egress)"
        $ini += "env_lines=$(@($s.Env).Count)"
    } else {
        $ini += "installed=0"
    }
    $ini += "update_check=$(& $b $task)"
    # INI for NSIS's ReadINIStr, which wants the system code page or UTF-16 -
    # UTF-16 LE with a BOM is the one both read without guessing.
    [IO.File]::WriteAllLines((Join-Path $OutDir 'service.ini'), [string[]]$ini, [Text.Encoding]::Unicode)
    $envText = if ($s -and $s.Env) { ($s.Env -join "`r`n") + "`r`n" } else { '' }
    [IO.File]::WriteAllText((Join-Path $OutDir 'service.env'), $envText, [Text.Encoding]::Unicode)
    exit 0
}

# ------------------------------------------------------------- Checks for the UI
function Do-CheckPort {
    # Listening already? The installer asks before using a busy port; a
    # service that cannot bind just restarts every five seconds, and the
    # only sign of it is a log nobody has opened yet.
    $l = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $l) { exit 0 }
    $name = try { (Get-Process -Id $l.OwningProcess -ErrorAction Stop).ProcessName } catch { "process $($l.OwningProcess)" }
    # Our own server holds its port until the upgrade stops it.
    if ($name -eq 'breeze-core') { exit 0 }
    Write-Output $name
    exit 1
}

function Do-CheckEnv {
    if (-not $EnvFile -or -not (Test-Path $EnvFile)) { exit 0 }
    $r = Read-EnvLines $EnvFile
    if ($r -is [string]) { Write-Output $r; exit 1 }
    exit 0
}

# --------------------------------------------------------------- Downloads
#
# Both write to a temporary name beside -Dest and move it into place only once
# it is verified, so a failed or tampered download never sits where the
# installer would copy it from.
function Enable-Tls12 {
    # Windows PowerShell 5.1 still offers TLS 1.0 first on older .NET setups;
    # nssm.cc and aspic both want 1.2.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

function Save-Verified([string]$From, [string]$Sha, [string]$To) {
    if ((Get-Sha256 $From) -ne $Sha) { return $false }
    if ((Resolve-Path -LiteralPath $From).Path -ne [IO.Path]::GetFullPath($To)) {
        Copy-Item -LiteralPath $From -Destination $To -Force
    }
    return $true
}

function Expand-NssmZip([string]$Zip, [string]$To) {
    if ((Get-Sha256 $Zip) -ne $NssmZipSha256) { return $false }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $z = [System.IO.Compression.ZipFile]::OpenRead($Zip)
    try {
        # win64 only: breeze-core.exe is an x86-64 program, and ARM64 Windows
        # runs both under emulation.
        $entry = $z.Entries | Where-Object { $_.FullName -match '(^|/)win64/nssm\.exe$' } | Select-Object -First 1
        if (-not $entry) { return $false }
        $tmp = "$To.part"
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $tmp, $true)
    } finally { $z.Dispose() }
    if ((Get-Sha256 $tmp) -ne $NssmExeSha256) { Remove-Item $tmp -Force; return $false }
    Move-Item -LiteralPath $tmp -Destination $To -Force
    return $true
}

function Do-FetchNssm {
    if (-not $Dest) { $Dest = Join-Path $InstallDir 'nssm.exe' }
    New-Item -ItemType Directory -Force -Path (Split-Path $Dest) | Out-Null

    $have = @((Join-Path $InstallDir 'nssm.exe'))
    if ($LocalDir) { $have += (Join-Path $LocalDir 'nssm.exe') }
    foreach ($c in $have) {
        if ((Test-Path -LiteralPath $c) -and (Save-Verified $c $NssmExeSha256 $Dest)) {
            Info "NSSM: using $c (checksum matches)"
            exit 0
        }
        if (Test-Path -LiteralPath $c) { Warn "NSSM: $c does not match the pinned checksum - not using it" }
    }
    if ($LocalDir) {
        $zip = Join-Path $LocalDir 'nssm-2.24.zip'
        if ((Test-Path -LiteralPath $zip) -and (Expand-NssmZip $zip $Dest)) {
            Info "NSSM: unpacked $zip (checksums match)"
            exit 0
        }
    }

    Enable-Tls12
    $zip = Join-Path $env:TEMP ("nssm-{0}.zip" -f [guid]::NewGuid().ToString('N'))
    try {
        foreach ($u in $NssmUrls) {
            Info "NSSM: downloading $u"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $u -OutFile $zip -TimeoutSec 60
            } catch {
                Warn "NSSM: $u failed: $($_.Exception.Message)"
                continue
            }
            if (Expand-NssmZip $zip $Dest) {
                Info "NSSM: $u verified (sha256 $NssmZipSha256)"
                exit 0
            }
            Warn "NSSM: what $u served does not match the pinned checksum - not using it"
        }
    } finally { Remove-Item $zip -Force -ErrorAction SilentlyContinue }
    Die "Could not get NSSM. Setup needs it to run the server as a service. Connect to the internet, or put nssm-2.24.zip (from https://nssm.cc/download) next to the installer, and run it again."
}

function Do-FetchGup {
    if (-not $GupUrl -or -not $GupSha256) { Die "-GupUrl and -GupSha256 are required" }
    if (-not $Dest) { $Dest = Join-Path $InstallDir 'updater\GUP.exe' }
    New-Item -ItemType Directory -Force -Path (Split-Path $Dest) | Out-Null
    $sha = $GupSha256.ToLowerInvariant()
    $name = [IO.Path]::GetFileName(([Uri]$GupUrl).AbsolutePath)

    $have = @((Join-Path $InstallDir 'updater\GUP.exe'))
    if ($LocalDir) { $have += (Join-Path $LocalDir $name) }
    foreach ($c in $have) {
        if ((Test-Path -LiteralPath $c) -and (Save-Verified $c $sha $Dest)) {
            Info "Updater: using $c (checksum matches)"
            exit 0
        }
    }

    Enable-Tls12
    $tmp = "$Dest.part"
    try {
        Info "Updater: downloading $GupUrl"
        Invoke-WebRequest -UseBasicParsing -Uri $GupUrl -OutFile $tmp -TimeoutSec 60
        if ((Get-Sha256 $tmp) -ne $sha) { Die "Updater: what $GupUrl served does not match the pinned checksum - not using it" }
        Move-Item -LiteralPath $tmp -Destination $Dest -Force
        Info "Updater: verified (sha256 $sha)"
        exit 0
    } catch {
        Die "Updater: $GupUrl failed: $($_.Exception.Message)"
    } finally { Remove-Item $tmp -Force -ErrorAction SilentlyContinue }
}

# ------------------------------------------------------- The sign-in check
function Do-RegisterUpdateCheck {
    Assert-Admin
    $gup = Join-Path $InstallDir 'updater\GUP.exe'
    if (-not (Test-Path $gup)) { Die "no updater at $gup" }
    # -forceDomain: GUP refuses any download that does not start with this,
    # whatever the feed says. Narrowed to the Windows directory, not the host.
    $act = New-ScheduledTaskAction -Execute $gup -Argument "-forceDomain=$UpdatePrefix" -WorkingDirectory (Split-Path $gup)
    $trg = New-ScheduledTaskTrigger -AtLogOn
    # A few minutes in, not at the moment of signing in, when everything else
    # on the machine is starting too.
    $trg.Delay = 'PT5M'
    # Whoever signs in and is an administrator - the people who can install
    # an update - in their own session, since GUP asks before it does
    # anything. Limited: GUP itself needs no rights, and the installer it
    # starts asks for elevation on its own. By SID, because the group's name
    # is translated on a non-English Windows.
    $prn = New-ScheduledTaskPrincipal -GroupId 'S-1-5-32-544' -RunLevel Limited
    $set = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -StartWhenAvailable -RunOnlyIfNetworkAvailable -MultipleInstances IgnoreNew `
        -ExecutionTimeLimit (New-TimeSpan -Hours 4)
    Register-ScheduledTask -TaskPath $TaskPath -TaskName $TaskName -Action $act -Trigger $trg `
        -Principal $prn -Settings $set -Force `
        -Description 'Asks aspic whether a newer Breeze Core is out, a few minutes after an administrator signs in. Silent unless there is one, and asks before installing it.' | Out-Null
    Info "Update check at sign-in: on"
}

function Do-UnregisterUpdateCheck {
    Assert-Admin
    if (Get-ScheduledTask -TaskPath $TaskPath -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $TaskName -Confirm:$false
    }
    # And the folder, which Task Scheduler keeps once created. Only if empty.
    try {
        $svc = New-Object -ComObject Schedule.Service
        $svc.Connect()
        $f = $svc.GetFolder($TaskPath.TrimEnd('\'))
        if ($f.GetTasks(1).Count -eq 0 -and $f.GetFolders(0).Count -eq 0) {
            $svc.GetFolder('\').DeleteFolder($TaskPath.Trim('\'), 0)
        }
    } catch { }
    Info "Update check at sign-in: off"
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
    Do-UnregisterUpdateCheck

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

    $extraEnv = @()
    if ($EnvFile) {
        if (-not (Test-Path $EnvFile)) { Die "-EnvFile $EnvFile does not exist" }
        $r = Read-EnvLines $EnvFile
        if ($r -is [string]) { Die "-EnvFile ${EnvFile}: $r" }
        $extraEnv = @($r)
    } elseif ($script:CarriedEnv) {
        $extraEnv = @($script:CarriedEnv)
    }

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
    $envExtra += $extraEnv
    # Straight into NSSM's REG_MULTI_SZ rather than through `nssm set`: a value
    # with a space or a quote in it would otherwise go through Windows
    # PowerShell's argument quoting on the way, which is not always faithful.
    Set-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName\Parameters" `
        -Name AppEnvironmentExtra -Type MultiString -Value ([string[]]$envExtra)
    if ($extraEnv) { Info "Extra environment: $(@($extraEnv | ForEach-Object { ($_ -split '=', 2)[0] }) -join ', ')" }
    # Logs + rotation; auto-restart on crash.
    & $nssm set $ServiceName AppStdout (Join-Path $logs 'service.log') | Out-Null
    & $nssm set $ServiceName AppStderr (Join-Path $logs 'service.log') | Out-Null
    & $nssm set $ServiceName AppRotateFiles 1 | Out-Null
    & $nssm set $ServiceName AppRotateBytes 1048576 | Out-Null
    & $nssm set $ServiceName AppExit Default Restart | Out-Null
    & $nssm set $ServiceName AppRestartDelay 5000 | Out-Null

    # --- Firewall ----------------------------------------------------------
    Get-NetFirewallRule -DisplayName "Breeze Core (LAN)" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    Get-NetFirewallRule -DisplayName "Breeze Core egress lockdown" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    if (-not $NoFirewall -and -not $BehindProxy -and $BindHost -ne '127.0.0.1') {
        Info "Opening inbound TCP $Port from the local subnet only"
        New-NetFirewallRule -DisplayName "Breeze Core (LAN)" -Direction Inbound -Action Allow `
            -Protocol TCP -LocalPort $Port -RemoteAddress LocalSubnet -Profile Any | Out-Null
    } elseif ($NoFirewall) {
        Info "No inbound firewall rule, as asked - other devices reach it only if a rule of your own allows it"
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
        New-NetFirewallRule -DisplayName "Breeze Core egress lockdown" -Direction Outbound -Action Block `
            -Program $exe -RemoteAddress Internet -Profile Any | Out-Null
        Warn "Egress lockdown added (blocks '$exe' to Internet). Verify your units are still reachable."
    }

    # --- Start (only once paired) -----------------------------------------
    # The server refuses to start without an api_key and says so; starting it
    # here would just put that message in a log nobody is reading yet.
    if ($NoStart) {
        Info "Left stopped, as it was before - start it with: nssm start $ServiceName"
    } elseif (Test-Path (Join-Path $DataDir 'config.json')) {
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

# Reconfigure changes what it is told to and keeps everything else. It used to
# be Install under another name, which rebuilt the service from defaults: the
# Caddy wizard's "-Reconfigure -BehindProxy" put the port back to 8420 (with
# Caddy still pointing at the old one), dropped every extra environment
# variable and removed the egress lockdown.
function Do-Reconfigure {
    $cur = Get-CurrentSettings
    if ($cur -and [IO.Path]::GetFileName($cur.Application) -eq 'breeze-core.exe') {
        if (-not $Bound.ContainsKey('Port'))        { $script:Port = $cur.Port }
        if (-not $Bound.ContainsKey('BehindProxy')) { $script:BehindProxy = [switch]$cur.BehindProxy }
        if (-not $Bound.ContainsKey('LockEgress'))  { $script:LockEgress = [switch]$cur.Egress }
        if (-not $Bound.ContainsKey('NoFirewall'))  { $script:NoFirewall = [switch](-not $cur.Firewall) }
        # A host is kept only while the proxy choice is: moving behind a proxy
        # means loopback, and leaving one means finding the LAN address again.
        if (-not $Bound.ContainsKey('BindHost') -and -not $Bound.ContainsKey('BehindProxy')) {
            $script:BindHost = $cur.Host
        }
        if (-not $Bound.ContainsKey('EnvFile')) { $script:CarriedEnv = $cur.Env }
        Info "Reconfiguring: keeping everything not given on the command line"
    }
    Do-Install
}

# --- Upgrading in place ------------------------------------------------------
#
# Re-running the installer used to run Do-Install, which rewrites the service
# from defaults: it put the bind address back to the LAN IP, dropped
# --behind-proxy, reset the port, replaced AppEnvironmentExtra (losing anything
# added with `nssm edit`, BREEZE_WORKERS included) and reopened the inbound LAN
# firewall rule on a machine that had deliberately been set up proxy-only. So
# every upgrade quietly undid the hardening. An upgrade now leaves all of that
# alone and changes only what a new version actually needs: which executable
# to run.

# Exit codes the installer reads. Anything else is a failure.
$ExitStopped    = 0    # installed, and now not running
$ExitWasRunning = 10   # installed and running: stopped, start it again after
$ExitNotThere   = 20   # no service yet: this is a first install

function Do-Stop {
    Assert-Admin
    $svc = Get-Service $ServiceName -ErrorAction SilentlyContinue
    if (-not $svc) { exit $ExitNotThere }
    if ($svc.Status -eq 'Stopped') { exit $ExitStopped }
    # Before any file is replaced: NSSM and breeze-core.exe are both locked
    # while the service runs, and an installer that cannot overwrite them
    # either stops with a retry dialog or, silent, leaves half an upgrade.
    Info "Stopping service '$ServiceName' for the upgrade"
    Stop-Service $ServiceName -Force -ErrorAction Stop
    $svc.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    exit $ExitWasRunning
}

function Do-Upgrade {
    Assert-Admin
    $nssm = Resolve-Nssm $Nssm
    $exe = Join-Path $InstallDir 'breeze-core.exe'
    if (-not (Test-Path $exe)) {
        Die "breeze-core.exe not found in $InstallDir. Point -InstallDir at the installed tree."
    }

    # Read what the service runs straight from NSSM's registry key: `nssm get`
    # writes UTF-16, which PowerShell 5.1 does not reliably decode.
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName\Parameters"
    $params = Get-ItemProperty $key -ErrorAction SilentlyContinue
    $current = if ($params) { [string]$params.Application } else { '' }
    if ([IO.Path]::GetFileName($current) -ne 'breeze-core.exe') {
        # Not this server: most likely a Python-era install, whose arguments
        # and environment mean nothing to breeze-core.exe. That one really
        # does need configuring from scratch.
        Warn "The existing service runs '$current', not breeze-core.exe - configuring it from scratch"
        Do-Install
        return
    }

    Info "Upgrading in place: bind address, port, proxy mode, environment and firewall rules are kept"
    # Only where the program lives, in case the install moved.
    & $nssm set $ServiceName Application $exe | Out-Null
    & $nssm set $ServiceName AppDirectory $InstallDir | Out-Null

    if ($Start) {
        & $nssm start $ServiceName | Out-Null
        Info "Service '$ServiceName' started again"
    } else {
        Info "Service '$ServiceName' was not running before the upgrade, so it is left stopped"
    }
}

$script:CarriedEnv = @()
switch ($Action) {
    'Install'               { Do-Install }
    'Reconfigure'           { Do-Reconfigure }
    'Uninstall'             { Do-Uninstall }
    'Stop'                  { Do-Stop }
    'Upgrade'               { Do-Upgrade }
    'Describe'              { Do-Describe }
    'CheckPort'             { Do-CheckPort }
    'CheckEnv'              { Do-CheckEnv }
    'FetchNssm'             { Do-FetchNssm }
    'FetchGup'              { Do-FetchGup }
    'RegisterUpdateCheck'   { Do-RegisterUpdateCheck }
    'UnregisterUpdateCheck' { Do-UnregisterUpdateCheck }
}
