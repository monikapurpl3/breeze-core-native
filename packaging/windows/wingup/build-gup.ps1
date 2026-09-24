<#
.SYNOPSIS
    Build GUP.exe - WinGup, modified for Breeze Core - from pinned source.

.DESCRIPTION
    Clones notepad-plus-plus/wingup at a pinned tag, refuses if the tag no
    longer points at the pinned commit, applies breeze.patch, builds curl
    (static, Schannel) and then GUP with the local Visual Studio, and writes
    to packaging\out\gup\:

        GUP-<ver>.exe              the updater
        GUP-<ver>.exe.sha256       what the installer checks the download against
        wingup-<ver>-source.tar.gz the exact corresponding source (LGPL)
        notices\                   LGPL-3.0, GPL-3.0 and the third-party notices

    and pins the result in packaging\windows\wingup\SHA256SUMS, which is
    committed: the installer downloads GUP-<ver>.exe from aspic and refuses
    anything that does not match it, and build-repo.sh refuses to publish a
    GUP-<ver>.exe that does not match it either.

    A build is not reproducible byte for byte (the linker stamps it), so a
    rebuild of the same version would publish different bytes under a name
    older installers have already pinned. This refuses to overwrite an
    existing build: bump $Suffix instead.

    See README.md in this directory for why each of those exists.

.EXAMPLE
    .\packaging\windows\wingup\build-gup.ps1
#>
[CmdletBinding()]
param(
    [string]$VsDir = "C:\Program Files\Microsoft Visual Studio\18\Enterprise",
    [string]$Toolset = "v145"
)
$ErrorActionPreference = 'Stop'

# The upstream release this is built from, and the commit that tag must be. A
# tag can be moved; a build that silently followed it would ship source nobody
# reviewed under a version everyone had.
$Tag    = 'v5.4.3'
$Commit = '80b6e0ff669c9a173a112035d8a08ef06a972b22'
# Bump when breeze.patch changes, so a changed binary never reuses a name.
$Suffix = 'breeze.1'
$Ver    = "5.4.3-$Suffix"

$here   = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo   = (Resolve-Path (Join-Path $here '..\..\..')).Path
$work   = Join-Path $repo 'packaging\out\gup-build'
$out    = Join-Path $repo 'packaging\out\gup'
$pin    = Join-Path $here 'SHA256SUMS'
$vcvars = Join-Path $VsDir 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) { throw "no vcvars64.bat under $VsDir - pass -VsDir" }
if ((Test-Path $pin) -and (Select-String -Path $pin -SimpleMatch "GUP-$Ver.exe" -Quiet)) {
    throw "GUP-$Ver.exe is already pinned in $pin - bump `$Suffix to build a new one"
}

function Invoke-InVs([string]$what, [string]$cmd) {
    Write-Host "=== $what"
    cmd /c "`"$vcvars`" >nul && cd /d `"$work`" && $cmd"
    if ($LASTEXITCODE -ne 0) { throw "$what failed ($LASTEXITCODE)" }
}

Write-Host "=== wingup $Tag"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
git clone --quiet --depth 1 --branch $Tag https://github.com/notepad-plus-plus/wingup.git $work
if ($LASTEXITCODE -ne 0) { throw "clone failed" }
$head = (git -C $work rev-parse HEAD).Trim()
if ($head -ne $Commit) { throw "tag $Tag is now $head, not $Commit - refusing to build from a moved tag" }

git -C $work apply (Join-Path $here 'breeze.patch')
if ($LASTEXITCODE -ne 0) { throw "breeze.patch does not apply to $Tag" }

# Upstream's tree dropped these two, and the zlib licence forbids removing its
# notice from a source distribution - which the archive below is.
Copy-Item (Join-Path $here 'notices\ZipLib-Licence.txt') (Join-Path $work 'src\ZipLib\Licence.txt')
Copy-Item (Join-Path $here 'notices\bzip2-LICENSE.txt') (Join-Path $work 'src\ZipLib\extlibs\bzip2\LICENSE')
Copy-Item (Join-Path $here 'breeze.patch') (Join-Path $work 'breeze.patch')
Copy-Item (Join-Path $here 'README.md') (Join-Path $work 'README.breeze-core.md')
Copy-Item (Join-Path $here 'nativeLang.xml') (Join-Path $work 'nativeLang.breeze-core.xml')

# curl as upstream's buildCurlAll.bat builds it, x64 Release only: static,
# Schannel (the OS trust store, no OpenSSL). CMAKE_GENERATOR_INSTANCE because
# this Visual Studio is not one vswhere reports.
$vsFwd = $VsDir -replace '\\', '/'
$curl = '-DBUILD_SHARED_LIBS=OFF -DCURL_STATIC_CRT=ON -DBUILD_CURL_EXE=OFF -DCURL_USE_SCHANNEL=ON ' +
        '-DCURL_USE_OPENSSL=OFF -DBUILD_TESTING=OFF -DUSE_LIBPSL=OFF -DCURL_USE_LIBPSL=OFF ' +
        '-DUSE_NGHTTP2=OFF -DUSE_LIBIDN2=OFF'
Invoke-InVs 'curl: configure' "cmake curl -B curl\build\x64 -G `"Visual Studio 18 2026`" -A x64 -DCMAKE_GENERATOR_INSTANCE=`"$vsFwd`" $curl"
Invoke-InVs 'curl: build' 'cmake --build curl\build\x64 --config Release'

# The solution names toolset v143; this Visual Studio carries v145.
Invoke-InVs 'GUP: build' "msbuild vcproj\GUP.sln /m /nologo /v:minimal /p:Configuration=Release /p:Platform=x64 /p:PlatformToolset=$Toolset"

$exe = Join-Path $work 'bin64\GUP.exe'
if (-not (Test-Path $exe)) { throw "the build reported success but produced no $exe" }
$desc = (Get-Item $exe).VersionInfo.FileDescription
if ($desc -notmatch 'modified for Breeze Core') {
    throw "GUP.exe describes itself as '$desc' - the patch did not reach the binary"
}

Write-Host "=== output"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force (Join-Path $out 'notices') | Out-Null
$gup = Join-Path $out "GUP-$Ver.exe"
Copy-Item $exe $gup
$sha = (Get-FileHash $gup -Algorithm SHA256).Hash.ToLower()
# sha256sum's own format, so `sha256sum -c` works in the download directory.
[IO.File]::WriteAllText("$gup.sha256", "$sha  GUP-$Ver.exe`n")
foreach ($f in 'LGPL-3.0.txt', 'GPL-3.0.txt', 'THIRD-PARTY-NOTICES.txt') {
    Copy-Item (Join-Path $here "notices\$f") (Join-Path $out "notices\$f")
}

# The corresponding source: the pinned tree with the patch applied and the
# notices restored, committed so `git archive` takes exactly that and none of
# the build output lying beside it.
git -C $work add -u
git -C $work add -f src/ZipLib/Licence.txt src/ZipLib/extlibs/bzip2/LICENSE breeze.patch README.breeze-core.md nativeLang.breeze-core.xml
git -C $work -c user.name='Breeze Core build' -c user.email='build@invalid' commit --quiet -m "Breeze Core ${Suffix}: breeze.patch applied, dropped licence notices restored"
if ($LASTEXITCODE -ne 0) { throw "could not commit the patched tree" }
git -C $work archive --format=tar.gz --prefix="wingup-$Ver/" -o (Join-Path $out "wingup-$Ver-source.tar.gz") HEAD
if ($LASTEXITCODE -ne 0) { throw "could not archive the source" }

$src = Join-Path $out "wingup-$Ver-source.tar.gz"
$srcSha = (Get-FileHash $src -Algorithm SHA256).Hash.ToLower()
# LF and sha256sum's format: build-repo.sh checks these with `sha256sum -c`.
$sums = "$sha  GUP-$Ver.exe`n$srcSha  wingup-$Ver-source.tar.gz`n"
[IO.File]::WriteAllText((Join-Path $out 'SHA256SUMS'), $sums)
[IO.File]::WriteAllText($pin, $sums)

Get-ChildItem -Recurse $out -File | ForEach-Object { "  {0,-40} {1,9} bytes" -f $_.FullName.Substring($out.Length + 1), $_.Length }
"GUP $Ver  sha256=$sha  ($desc)"
"pinned in $pin - commit it with the change that ships this build"
