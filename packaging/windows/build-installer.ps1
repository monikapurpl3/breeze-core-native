# Build Breeze-Core-Setup.exe.
#
# The version is read from crates/breeze-core/Cargo.toml rather than typed in,
# because in the Python line it was a hardcoded default in the .nsi ("2.3.0") -
# so an installer built without an explicit /DVERSION silently claimed to be a
# version it wasn't, and one shipped that way.
#
#   .\packaging\windows\build-installer.ps1                  # version from source
#   .\packaging\windows\build-installer.ps1 -OutDir C:\out   # put the .exe elsewhere
#
# NOTE: this is the one release artifact that cannot be cross-built - makensis
# needs Windows. Build it here at release time and attach it to the tag.
[CmdletBinding()]
param(
    [string]$OutDir,
    [string]$Makensis = "C:\Program Files (x86)\NSIS\makensis.exe"
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = Resolve-Path (Join-Path $here "..\..")

# --- version, straight from the crate -----------------------------------
$cargo = Join-Path $repo "crates\breeze-core\Cargo.toml"
$m = Select-String -Path $cargo -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $m) { throw "could not read version from $cargo" }
$version = $m.Matches[0].Groups[1].Value
Write-Host "version: $version (from crates/breeze-core/Cargo.toml)"

if (-not (Test-Path $Makensis)) {
    throw "makensis not found at $Makensis (winget install NSIS.NSIS, or pass -Makensis)"
}

# --- the binary the installer wraps -------------------------------------
$binDir = Join-Path $repo "packaging\out\bin\windows"
$exe = Join-Path $binDir "breeze-core.exe"
if (-not (Test-Path $exe)) {
    throw "$exe missing - run: ./packaging/build-binaries.sh windows"
}
# Refuse to wrap a binary that is not the version being claimed. The Python line
# had a whole class of bug here: an artifact stamped with one version and built
# from another. Asking the binary is cheap and it cannot be argued with.
$reported = (& $exe --version | Select-Object -First 1)
if ($reported -ne "breeze-core $version") {
    throw "the staged binary reports '$reported' but this build claims $version - rebuild it"
}
Write-Host "binary : $exe ($([math]::Round((Get-Item $exe).Length / 1KB)) KB, reports '$reported')"

# --- The updater: pinned, not packed ------------------------------------
# NSSM and GUP are both downloaded by the installer, not carried in it (see
# the .nsi's header). NSSM's pins live in install-service.ps1; GUP's is the
# committed wingup\SHA256SUMS that build-gup.ps1 wrote, and that build-repo.sh
# holds the published copy to.
$sums = Get-Content (Join-Path $here "wingup\SHA256SUMS")
$gupLine = $sums | Where-Object { $_ -match '^([0-9a-f]{64})  (GUP-(.+)\.exe)$' } | Select-Object -First 1
if (-not $gupLine) { throw "no GUP-*.exe line in wingup\SHA256SUMS - run wingup\build-gup.ps1" }
$gupSha = $Matches[1]; $gupName = $Matches[2]; $gupVer = $Matches[3]
$built = Join-Path $repo "packaging\out\gup\$gupName"
if (Test-Path $built) {
    if ((Get-FileHash $built -Algorithm SHA256).Hash.ToLower() -ne $gupSha) {
        throw "$built does not match wingup\SHA256SUMS - the pin and the build have drifted apart"
    }
}
Write-Host "updater: $gupName (sha256 $gupSha)"

# The two files that carry this version into the updater's folder.
$stage = Join-Path $repo "packaging\out\windows-stage"
New-Item -ItemType Directory -Force -Path $stage | Out-Null
foreach ($t in @(@('gup.xml.in', 'gup.xml'), @('README.txt.in', 'README.txt'))) {
    $text = [IO.File]::ReadAllText((Join-Path $here "wingup\$($t[0])"))
    $text = $text.Replace('@VERSION@', $version).Replace('@GUPVER@', $gupVer)
    # CRLF: these are read by people in Notepad and by GUP, both on Windows.
    $text = ($text -replace "`r?`n", "`r`n")
    [IO.File]::WriteAllText((Join-Path $stage $t[1]), $text)
}

# Decimal megabytes with a decimal point whatever the machine's locale: a
# Croatian Windows would otherwise write 2,3.
$exeMb = ([math]::Round((Get-Item $exe).Length / 1e6, 1)).ToString([Globalization.CultureInfo]::InvariantCulture)
& $Makensis "/DVERSION=$version" "/DEXE_MB=$exeMb" "/DGUP_NAME=$gupName" "/DGUP_SHA256=$gupSha" `
    "/DSTAGE=$stage" (Join-Path $here "breeze-core-setup.nsi")
if ($LASTEXITCODE -ne 0) { throw "makensis failed ($LASTEXITCODE)" }

$out = Join-Path $here "Breeze-Core-Setup.exe"
if (-not (Test-Path $out)) { throw "makensis reported success but produced no .exe" }

if ($OutDir) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    # A versioned name for hosting alongside other releases, and the plain name
    # the release asset has always had.
    Copy-Item $out (Join-Path $OutDir "Breeze-Core-Setup-$version.exe") -Force
    Copy-Item $out (Join-Path $OutDir "Breeze-Core-Setup.exe") -Force
    Write-Host "copied to $OutDir"
}

$sha = (Get-FileHash $out -Algorithm SHA256).Hash
"{0}  {1} bytes  sha256={2}" -f $out, (Get-Item $out).Length, $sha
