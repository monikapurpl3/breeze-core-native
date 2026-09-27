# Installing on Windows

A guided installer that registers a hardened Windows service. About a megabyte.

**[Breeze-Core-Setup-4.2.0.exe](https://aspic.salataputarica.hr.eu.org/windows/Breeze-Core-Setup-4.2.0.exe)**
· [sha256](https://aspic.salataputarica.hr.eu.org/windows/Breeze-Core-Setup-4.2.0.exe.sha256)

x86-64 only.

> **The installer downloads two small helpers while it runs** — NSSM, which
> runs the server as a service, and the updater — each checked against a fixed
> checksum. So a first install needs the internet, or see
> [Installing offline](#installing-offline). Installers up to 4.1.1 carried NSSM
> inside instead.

## Simple or Advanced

The installer asks first.

- **Simple** installs to Program Files, serves your local network on port 8420
  with the firewall opened to the local network only, and checks for updates
  when you sign in. The one choice is whether to set up Caddy for public HTTPS
  at the end.
- **Advanced** lets you choose the folder, the port (it warns if something
  already uses it), whether the local network or only this PC can reach the
  server, the firewall rule, blocking the server from the internet, the update
  check, Caddy, and extra environment variables for the server — see
  [Configuration](Configuration) for what those do.

Run it again over an existing install and it asks instead whether to **keep the
current settings** — the default, and what a silent or automatic update does —
or **review and change them**, starting from what the service is set to now.
Your paired units and enrolled devices are kept either way.

## What the installer does

Two components. The server is required; the reverse proxy is optional and off
by default.

**The server**, always:

- installs the executable and a few scripts into `%ProgramFiles%\Breeze Core`;
- registers a service called **BreezeCore** running as **`LOCAL SERVICE`** —
  not as SYSTEM, and not as you — bound to this machine's LAN address (the one
  with a default gateway, so not a VMware or Hyper-V adapter);
- creates `%ProgramData%\breeze-core` for the four store files, with its ACL
  locked to that account;
- adds a **LAN-only inbound firewall rule** for its port, 8420 unless chosen
  otherwise;
- adds Start-menu shortcuts: **Pair AC units**, **Diagnose**, **Edit service
  (nssm)**, **Set up Caddy reverse proxy**, **Uninstall** and **Check for
  updates**.

**Caddy**, only if you tick the box — for public HTTPS. See below.

It is unsigned, so SmartScreen will object. The published SHA-256 is what you
have to go on; check it before running:

```powershell
Get-FileHash .\Breeze-Core-Setup-4.2.0.exe -Algorithm SHA256
```

## Why it downloads NSSM

Antivirus products flag installers that carry NSSM, because malware uses it to
keep itself running, so its bytes inside a setup program look like a threat.
Setup therefore fetches it, trying in turn: the copy an existing install
already has (so **an upgrade needs no internet**), a copy next to the
installer, [nssm.cc](https://nssm.cc/download), and a mirror on aspic.
Whichever it gets must match a checksum pinned in the installer, or it is not
used. If it cannot get NSSM at all, it stops **before changing anything** and
says so.

The updater is fetched the same way, from aspic, and is optional: if it cannot
be had, setup carries on without it.

### Installing offline

Download [`nssm-2.24.zip`](https://nssm.cc/release/nssm-2.24.zip) (or take it
from [aspic's mirror](https://aspic.salataputarica.hr.eu.org/windows/vendor/)),
put it in the same folder as the installer, and run the installer. It is
checked against the same checksum.

## Updates

The installer adds a **Check for updates** shortcut and — on by default — a
quiet check a few minutes after an administrator signs in. Both ask aspic
whether a newer Breeze Core is out; the sign-in check says nothing when there
is not. When there is one, it asks before downloading anything:

- **Yes** opens the new installer as usual;
- **Yes, quietly** installs it silently, keeping every setting;
- **Not now** does nothing.

Installing still needs an administrator, so Windows asks for elevation.

What it trusts: it downloads only from
`https://aspic.salataputarica.hr.eu.org/windows/`, over HTTPS, and refuses
anything else. The installer is not code-signed, so that is the same trust as
downloading it by hand from this page.

To turn the sign-in check off, run the installer again and choose to review
the settings, or disable **Breeze Core → Check for updates** in Task
Scheduler.

The updater is **WinGUp**, the updater Notepad++ uses, built from source with
its Notepad++ branding and behaviour patched out. It is a separate program
under the **LGPL-3.0**, not Breeze Core's AGPL; its licence texts, notices and
a pointer to its exact source are in `%ProgramFiles%\Breeze Core\updater\`, and
the source is published beside the binary on
[aspic](https://aspic.salataputarica.hr.eu.org/windows/updater/).

## After installing

Pair the units. Use the Start-menu shortcut, or from a terminal:

```powershell
& "$env:ProgramFiles\Breeze Core\breeze-core.exe" pair
```

That writes `%ProgramData%\breeze-core\config.json`. Start the service:

```powershell
Start-Service BreezeCore
```

Then open `http://<this machine's LAN address>:8420` and pair a browser —
[First run and pairing](First-run-and-pairing).

### Changing the address or the port

- Run the installer again and choose **Review and change the
  settings**; or, from an elevated PowerShell, change just what you name and
  keep the rest:

  ```powershell
  & "$env:ProgramFiles\Breeze Core\install-service.ps1" -Action Reconfigure -Port 8431
  & "$env:ProgramFiles\Breeze Core\install-service.ps1" -Action Reconfigure -BindHost 192.168.1.10
  ```

- **Any version**: the **Edit service (nssm)** shortcut opens the service's
  settings. The address and port are the arguments on the *Application* tab,
  `serve --host <address> --port <port>`; restart the service afterwards. If
  you change the port this way, change the firewall rule too.

## Managing the service

```powershell
Get-Service BreezeCore
Restart-Service BreezeCore
Stop-Service BreezeCore

# what it is actually doing
Get-Content "$env:ProgramData\breeze-core\logs\service.log" -Tail 40 -Wait
```

The service is supervised by **NSSM**, installed next to the executable, which
setup downloads as described above.

## Public HTTPS, with Caddy

The optional component. The wizard downloads Caddy, renders a hardened
Caddyfile with your domain, rebinds the service to loopback (on whichever port
it uses), and registers Caddy as a service of its own.

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
the service, the firewall rules, the program files, the updater and its
sign-in check.

**`%ProgramData%\breeze-core` is kept.** A paired V3 unit's token and key were
issued once by a cloud that no longer hands them out, so nothing in the
packaging deletes them. Remove that folder by hand for a full wipe.

## Differences worth knowing about

| | On Windows |
|---|---|
| config directory | `%ProgramData%\breeze-core`, not `/etc/breeze-core` |
| file modes | Windows has no mode bits; the directory's **ACL** is locked instead |
| service account | `LOCAL SERVICE` |
| service settings | NSSM's, in the registry (`nssm edit BreezeCore`), not an env file |
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

The usual causes are the ordinary ones — something else on its port, an
address this machine does not have (after a DHCP change, say), or a config
directory the service account cannot write. [Troubleshooting](Troubleshooting)
covers all three; only the paths differ.

If setup stops saying it **could not get NSSM**, nothing was installed or
changed: connect to the internet, or [install offline](#installing-offline).

## Notes on the build, if you are curious

The installer refuses to build if the version in `Cargo.toml` and the version
the staged binary *reports* disagree. That check exists because an artifact
stamped with one version and built from another is a whole class of bug — the
Python line once shipped an installer claiming 2.3.0 while the package was on
3.x — and it is cheap to make impossible rather than merely unlikely.
