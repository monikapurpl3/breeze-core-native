# packaging/windows — the installer, the service, the updater and the proxy

Windows gets the same treatment as the Linux packages: a guided installer, a
hardened service, an update check, and an optional reverse proxy for public
HTTPS.

| File | What it is |
|---|---|
| `breeze-core-setup.nsi` | NSIS guided installer (compile → `Breeze-Core-Setup.exe`). **Simple** (recommended settings, optional Caddy) or **Advanced** (folder, port, LAN or this PC only, firewall rule, blocking the internet, the update check, Caddy, extra environment variables); on an upgrade, **keep** the settings or **review and change** them |
| `install-service.ps1` | Everything the installer does to the machine: registers the hardened `BreezeCore` service (`LOCAL SERVICE`, LAN-only firewall rule, locked-down `%ProgramData%\breeze-core`), upgrades it in place, **fetches NSSM and the updater** against pinned SHA-256s, describes the current settings for the upgrade pages, checks a port and an environment list, and registers the sign-in update check |
| `wingup/` | The updater: WinGUp built from source with the Notepad++ branding patched out. **LGPL-3.0, not AGPL** — see `wingup/README.md` before changing anything there |
| `caddy-wizard.ps1` | Guided Caddy reverse proxy: downloads Caddy, renders a hardened Caddyfile (automatic HTTPS, security headers, real client IP, LAN-only admin, unbuffered SSE), rebinds the service to loopback on its own port, registers Caddy as a service. Supports `-DryRun` |
| `breeze-tripwire.ps1` | fail2ban-style watcher: tails Caddy's access log and bans abusive addresses with Windows Firewall. The LAN is never banned; bans expire |
| `Caddyfile.example` | Reference copy of what the wizard renders |
| `pair.cmd` | `breeze-core pair` with `AC_CONFIG` preset, for the Start-menu shortcut |
| `build-installer.ps1` | Builds the installer, taking the version from `Cargo.toml` and the updater's pin from `wingup/SHA256SUMS` |

## Building it

```powershell
./packaging/build-binaries.sh windows                          # from git-bash
.\packaging\windows\build-installer.ps1                        # -OutDir <dir> for a versioned copy
```

`build-installer.ps1` reads the version from `crates/breeze-core/Cargo.toml`,
**asks the staged binary what version it reports**, and refuses to build if the
two disagree. The Python line had a whole class of bug here — an artifact
stamped with one version and built from another, including an installer that
shipped claiming 2.3.0 while the package was on 3.x — and the two checks are
what make that impossible rather than unlikely. It also renders the updater's
`gup.xml` for this version, and takes the updater's name and checksum from
`wingup/SHA256SUMS`.

The updater itself is built separately and rarely, with
`wingup\build-gup.ps1`; see `wingup/README.md`. Nothing third-party is
committed or packed into the installer: `build-repo.sh` publishes GUP (with its
source and notices) under `/windows/updater/` and a mirror of NSSM under
`/windows/vendor/` on aspic, each checked against the same pins.

**This is the one release artifact that cannot be cross-built:** `makensis`
needs Windows. Build it here at release time and attach it to the tag.

## What the installer needs from the network

The Python installer had to find a Python 3.11+, build a virtualenv, and
download dependencies from PyPI, and most of its support burden was one of
those going wrong on somebody else's machine. This one installs a single
executable with the panel compiled into it.

Up to 4.1.1 it also carried NSSM and needed no network at all. **From 4.1.2 it
downloads NSSM during setup instead**, because antivirus products flag
installers that embed it (malware uses NSSM to persist, so its bytes inside a
setup program read as a threat). `install-service.ps1 -Action FetchNssm` tries,
in order: the copy in an existing install, `nssm.exe` or `nssm-2.24.zip` next
to the installer, nssm.cc, and aspic's mirror — and whichever it gets must
match the pinned zip and exe hashes. So an **upgrade needs no internet**, and
neither does an offline first install with `nssm-2.24.zip` put beside the
installer. It runs before anything is stopped or copied: a machine that cannot
get NSSM is left exactly as it was.

The updater (`GUP.exe`) is fetched the same way, from aspic, and is optional:
setup carries on without it. Caddy is downloaded by its wizard, only when
somebody asks for public HTTPS.

## Updates

WinGUp, in `%ProgramFiles%\Breeze Core\updater\`, asks
`https://aspic.salataputarica.hr.eu.org/windows/update/<version>.xml` whether a
newer release is out; `build-repo.sh` writes those feeds on every release. A
Start-menu **Check for updates** runs it and also says "up to date"; if chosen
(on by default), a scheduled task runs it quietly a few minutes after an
administrator signs in. It asks before downloading or installing, and it
refuses any download not under `https://aspic.salataputarica.hr.eu.org/windows/`
(`-forceDomain`). The installer is not Authenticode-signed, so GUP's signature
checks cannot be used: an update trusts HTTPS to aspic, exactly as downloading
the installer by hand does. "Yes, quietly" runs the new installer with `/S`,
which keeps every setting.

## Decisions worth knowing before editing

- **`LOCAL SERVICE`, not `LocalSystem`.** The server needs the LAN and its own
  four state files, nothing else. The tripwire is the exception and runs as
  `LocalSystem`, because managing firewall rules needs that.
- **The data directory is `%ProgramData%\breeze-core`** with inherited ACEs
  dropped (`icacls /inheritance:r`) — SYSTEM and Administrators full, LOCAL
  SERVICE **modify**. Modify rather than read because the server writes
  `devices.json` on every enrolment and `timers.json` as timers fire. It is
  never deleted on uninstall unless `-Purge` is passed: a paired V3 unit's token
  and key cannot be obtained from Midea again.
- **The service is not started until `config.json` exists.** Starting it first
  just writes "run breeze-core pair" into a log nobody is reading yet.
- **An upgrade keeps the service as it is.** `-Action Upgrade` changes only
  which executable runs; re-registering from defaults used to reset the bind
  address, the port, `--behind-proxy`, the environment and the firewall rules.
  `-Action Reconfigure` changes what it is given and keeps the rest — the Caddy
  wizard's `-Reconfigure -BehindProxy` used to put the port back to 8420.
- **The settings are read back from where they live** (NSSM's registry key and
  the firewall), by `-Action Describe`, not from anything the installer wrote
  down about itself — `nssm edit` would make that stale.
- **Extra environment lines go straight into NSSM's `REG_MULTI_SZ`**, not
  through `nssm set`, so a value with a space or a quote survives; the names the
  installer sets itself are refused.
- **`--behind-proxy` is passed as a flag *and* set as `AC_BEHIND_PROXY=1`.**
  Either would do; both means editing one in `nssm edit BreezeCore` cannot
  silently half-disable it. Without it the server reads the proxy's address as
  the client's, every request looks like it came from the LAN, and the LAN-only
  admin checks stop meaning anything.
- **The Caddyfile sets no `trusted_proxies`.** That is what makes Caddy
  overwrite `X-Forwarded-For` with the real peer, so an outsider cannot forge a
  private "LAN" client. Adding a public range there quietly turns the admin gate
  off.
- **`flush_interval -1` on `/api/units/stream`.** Server-sent events are one
  response that never ends; a buffering proxy gives you a panel with no live
  state and no error to explain it. The Python line's wizard did not have this
  route, and it is the one thing here that is not a straight port.
- **NSIS strings stop at 1024 characters**, so the environment box goes to and
  from a file through the System plugin rather than `${NSD_GetText}`.
- **All the scripts are ASCII and BOM-free**, so Windows PowerShell 5.1 and
  PowerShell 7+ both parse them. Keep it that way — an em dash in a `.ps1` is a
  parse error on 5.1 with a byte-order mark.

## Uninstalling

Programs and Features, or:

```powershell
powershell -File "C:\Program Files\Breeze Core\install-service.ps1" -Action Uninstall
# add -Purge to delete %ProgramData%\breeze-core as well
```

The uninstaller also removes the update check's scheduled task and the updater,
the `BreezeCaddy` and `BreezeTripwire` services, and every `Breeze *` /
`BreezeBan *` firewall rule, so a Caddy setup does not leave half a proxy and a
pile of bans behind.
