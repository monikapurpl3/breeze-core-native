# packaging/windows — the installer, the service and the proxy

Windows gets the same treatment as the Linux packages: a guided installer, a
hardened service, and an optional reverse proxy for public HTTPS.

| File | What it is |
|---|---|
| `breeze-core-setup.nsi` | NSIS guided installer (compile → `Breeze-Core-Setup.exe`). The server is required; Caddy is a separate, optional component |
| `install-service.ps1` | Register/unregister the hardened `BreezeCore` service — bundled NSSM, `LOCAL SERVICE`, LAN-only firewall rule, locked-down `%ProgramData%\breeze-core` |
| `caddy-wizard.ps1` | Guided Caddy reverse proxy: downloads Caddy, renders a hardened Caddyfile (automatic HTTPS, security headers, real client IP, LAN-only admin, unbuffered SSE), rebinds the service to loopback, registers Caddy as a service. Supports `-DryRun` |
| `breeze-tripwire.ps1` | fail2ban-style watcher: tails Caddy's access log and bans abusive addresses with Windows Firewall. The LAN is never banned; bans expire |
| `Caddyfile.example` | Reference copy of what the wizard renders |
| `pair.cmd` | `breeze-core pair` with `AC_CONFIG` preset, for the Start-menu shortcut |
| `fetch-vendor.ps1` | Downloads NSSM into `vendor\` for bundling (git-ignored, not committed) |
| `build-installer.ps1` | Builds the installer, taking the version from `Cargo.toml` |

## Building it

```powershell
./packaging/build-binaries.sh windows                          # from git-bash
powershell -ExecutionPolicy Bypass -File .\packaging\windows\fetch-vendor.ps1
.\packaging\windows\build-installer.ps1                        # -OutDir <dir> for a versioned copy
```

`build-installer.ps1` reads the version from `crates/breeze-core/Cargo.toml`,
**asks the staged binary what version it reports**, and refuses to build if the
two disagree. The Python line had a whole class of bug here — an artifact
stamped with one version and built from another, including an installer that
shipped claiming 2.3.0 while the package was on 3.x — and the two checks are
what make that impossible rather than unlikely.

**This is the one release artifact that cannot be cross-built:** `makensis`
needs Windows. Build it here at release time and attach it to the tag.

## What the rewrite changed here

The Python installer had to find a Python 3.11+, build a virtualenv, and
download dependencies from PyPI. Most of its support burden was one of those
three going wrong on somebody else's machine, and the installer needed the
internet to produce a working service.

**This one installs a single 2.2 MB executable with the panel compiled into it
and needs no network at all.** NSSM is bundled for the same reason: an installer
that has to reach the internet to register a service fails on exactly the
machine that most needs to work offline. Caddy is the opposite case — it is
downloaded by the wizard, at the moment somebody asks for public HTTPS, because
most installs never want it.

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
- **All the scripts are ASCII and BOM-free**, so Windows PowerShell 5.1 and
  PowerShell 7+ both parse them. Keep it that way — an em dash in a `.ps1` is a
  parse error on 5.1 with a byte-order mark.

## Uninstalling

Programs and Features, or:

```powershell
powershell -File "C:\Program Files\Breeze Core\install-service.ps1" -Action Uninstall
# add -Purge to delete %ProgramData%\breeze-core as well
```

The uninstaller also removes the `BreezeCaddy` and `BreezeTripwire` services and
every `Breeze *` / `BreezeBan *` firewall rule, so a Caddy setup does not leave
half a proxy and a pile of bans behind.
