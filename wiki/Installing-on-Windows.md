# Installing on Windows

A guided installer that registers a hardened Windows service. About 1.2 MB, and
it needs no internet connection.

**[Breeze-Core-Setup-4.0.2.exe](https://aspic.salataputarica.hr.eu.org/windows/Breeze-Core-Setup-4.0.2.exe)**
· [sha256](https://aspic.salataputarica.hr.eu.org/windows/Breeze-Core-Setup-4.0.2.exe.sha256)

x86-64 only.

## What the installer does

Two components. The server is required; the reverse proxy is optional and off
by default.

**The server**, always:

- installs the executable and a few scripts into `%ProgramFiles%\Breeze Core`;
- registers a service called **BreezeCore** running as **`LOCAL SERVICE`** —
  not as SYSTEM, and not as you;
- creates `%ProgramData%\breeze-core` for the four store files, with its ACL
  locked to that account;
- adds a **LAN-only inbound firewall rule** for port 8420;
- puts a "Pair your air conditioners" shortcut in the Start menu.

**Caddy**, only if you tick the box on the last page — for public HTTPS. See
below.

It is unsigned, so SmartScreen will object. The published SHA-256 is what you
have to go on; check it before running:

```powershell
Get-FileHash .\Breeze-Core-Setup-4.0.2.exe -Algorithm SHA256
```

## After installing

Pair the units. Use the Start-menu shortcut, or from a terminal:

```powershell
& "$env:ProgramFiles\Breeze Core\breeze-core.exe" pair
```

That writes `%ProgramData%\breeze-core\config.json`. Then set the bind address
and restart the service:

```powershell
notepad "$env:ProgramData\breeze-core\breeze-core.env"
Restart-Service BreezeCore
```

Then open `http://<that address>:8420` and pair a browser —
[First run and pairing](First-run-and-pairing).

## Managing the service

```powershell
Get-Service BreezeCore
Restart-Service BreezeCore
Stop-Service BreezeCore

# what it is actually doing
Get-Content "$env:ProgramData\breeze-core\logs\service.log" -Tail 40 -Wait
```

The service is supervised by **NSSM**, bundled in the installer rather than
downloaded, which is why nothing needs the internet at install time.

## Public HTTPS, with Caddy

The optional component. The wizard downloads Caddy, renders a hardened
Caddyfile with your domain, rebinds the service to loopback, and registers
Caddy as a service of its own.

```powershell
& "$env:ProgramFiles\Breeze Core\caddy-wizard.ps1"          # guided
& "$env:ProgramFiles\Breeze Core\caddy-wizard.ps1" -DryRun  # show, change nothing
```

`-DryRun` first is worth the thirty seconds.

What it renders, and why each part is there:

- **automatic HTTPS**, with Caddy getting and renewing the certificate;
- **no `trusted_proxies`**, which is what makes Caddy *overwrite*
  `X-Forwarded-For` with the real peer address. Adding a public range there
  would quietly turn the server's LAN-only admin check off, because Caddy would
  then believe whatever a client claimed;
- **the admin endpoints returning 403** to anything off the LAN — the server
  enforces this itself as well, and the proxy's 403 is what the tripwire
  watches for;
- **`flush_interval -1`** on the event stream, because Server-Sent Events are
  one response that never ends, and a proxy that buffers gives you a panel with
  no live state and no error anywhere to explain it;
- **transport-security headers only.** The app already sends a strict CSP, and
  a second policy from the proxy would fight it.

`Caddyfile.example` in the install directory is a reference copy of what the
wizard writes. If you edit one, edit the other.

### The tripwire

`breeze-tripwire.ps1` is a fail2ban-shaped watcher: it tails Caddy's access log
and bans abusive addresses with Windows Firewall. **The LAN is never banned**,
and bans expire.

That "never ban the LAN" rule is not decoration. On the Python line, a
fail2ban rule once banned the shared address that a phone's carrier NAT put
everybody behind, and the result was a lockout that looked like an application
bug for a while. Read
[Exposing it safely](Exposing-it-safely) before you rely on any of this.

## Uninstalling

Add/Remove Programs, or the uninstaller in the install directory. It removes
the service, the firewall rule and the program files.

**`%ProgramData%\breeze-core` is kept.** A paired V3 unit's token and key were
issued once by a cloud that no longer hands them out, so nothing in the
packaging deletes them. Remove that folder by hand for a full wipe.

## Differences worth knowing about

| | On Windows |
|---|---|
| config directory | `%ProgramData%\breeze-core`, not `/etc/breeze-core` |
| file modes | Windows has no mode bits; the directory's **ACL** is locked instead |
| service account | `LOCAL SERVICE` |
| sandboxing | no systemd sandbox to lean on; the firewall rule and the service account are the isolation |
| the binary | `breeze-core.exe`, and it is the same code as everywhere else |

Everything else — the REST API, the panel, the CLI verbs, the store formats —
is identical. A `config.json` written on Windows is readable on Linux and the
other way round.

## If it will not start

```powershell
Get-Service BreezeCore
Get-Content "$env:ProgramData\breeze-core\logs\service.log" -Tail 40
& "$env:ProgramFiles\Breeze Core\breeze-core.exe" --version
& "$env:ProgramFiles\Breeze Core\breeze-core.exe" diag
```

The usual causes are the ordinary ones — something else on port 8420, a
`BREEZE_HOST` naming an address this machine does not have, or a config
directory the service account cannot write. [Troubleshooting](Troubleshooting)
covers all three; only the paths differ.

## Notes on the build, if you are curious

The installer refuses to build if the version in `Cargo.toml` and the version
the staged binary *reports* disagree. That check exists because an artifact
stamped with one version and built from another is a whole class of bug — the
Python line once shipped an installer claiming 2.3.0 while the package was on
3.x — and it is cheap to make impossible rather than merely unlikely.
