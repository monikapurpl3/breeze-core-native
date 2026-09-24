# The Windows updater (WinGUp, modified)

Breeze Core's Windows updater is **WinGUp**, the generic updater Notepad++
uses, built from source with a small patch that removes the Notepad++
branding and behaviour. It is a separate program: the installer downloads it
next to `breeze-core.exe`, a Start-menu shortcut and an optional sign-in task
run it, and it never links to or is linked by anything else here.

## Licence — read this before changing anything in this directory

**The files in this directory that are part of, or modify, WinGUp are licensed
LGPL-3.0-or-later, not AGPL-3.0-or-later like the rest of this repository.**
That is `breeze.patch` and `nativeLang.xml`. The build script and `gup.xml.in`
are Breeze Core's own and follow the repository's licence.

WinGUp is LGPL-3.0; shipping it beside an AGPL program is aggregation (GPLv3
and AGPLv3 section 5), so neither licence reaches the other program. What the
LGPL does require of us, and what `build-gup.ps1` produces:

- the LGPL-3.0 **and** GPL-3.0 texts beside the binary, because the LGPL is
  written as additional permissions on top of the GPL;
- the notices of everything compiled into `GUP.exe` (`notices/`): curl, ZipLib,
  zlib, bzip2, TinyXml, the LZMA SDK and sha-2;
- the **exact corresponding source**, published beside the binary: the
  upstream tree at a pinned commit, with `breeze.patch` applied and the
  ZipLib and bzip2 licence files restored — upstream's tree dropped both,
  and the zlib licence forbids removing its notice from a source distribution;
- the modified version marked as such, which the patch does in the version
  resource ("WinGup, modified for Breeze Core").

## What the patch changes

Against notepad-plus-plus/wingup **v5.4.3** (`80b6e0ff669c9a173a112035d8a08ef06a972b22`):

| | why |
|---|---|
| the no-update dialog's two links go to aspic and the wiki | they opened notepad-plus-plus.org |
| the installer is started with `/S`, not `/closeRunningNpp /S …` | those switches are Notepad++'s |
| the wait for Notepad++'s instance mutex never matches | it stalled every check by 3 s whenever Notepad++ was open |
| the fallback download folder is `%APPDATA%\Breeze Core` | it was Notepad++'s (downloads go to `%TEMP%`; this is only used when neither `%TEMP%` nor `%TMP%` is set) |
| the current version is shown as it is | upstream reformats it on Notepad++'s single-digit scheme |
| "Never" is hidden | with no app window to tell, it was a dead button |
| `gup.xml` and friends are read from beside `GUP.exe` | upstream uses the working directory, and a scheduled task starts in System32 |
| the version resource says "modified for Breeze Core" | the LGPL requires a modified version to be marked |

## Building

```powershell
.\packaging\windows\wingup\build-gup.ps1
```

Needs Visual Studio's C++ tools and CMake. Clones the pinned tag, checks the
commit, applies the patch, builds curl (static, Schannel — no OpenSSL, the OS
trust store) and then GUP, and writes to `packaging/out/gup/`: `GUP.exe`, the
source archive, and the notices. `packaging/repo/build-repo.sh` publishes all
three under `/windows/updater/` on aspic, and the installer downloads
`GUP.exe` from there, checking it against the SHA-256 it was built with.
