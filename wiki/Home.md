# Breeze Core

Self-hosted control for **Midea air conditioners**: a LAN-first REST API, a web
control panel, a diagnostic CLI, and an optional native Android app. After a
one-time local pairing there is **no cloud dependency** — commands go from your
browser or phone to your own server to the unit, over your own network.

This wiki is the full documentation. The
[repository README](https://github.com/monikapurpl3/breeze-core-native) is the
short version.

**4.0.0 is the same project, reimplemented in Rust.** Same REST API, same store
files, same `AC_*` variables, same service name, same panel, same Android app —
it is meant to be installed *over* a 3.x deployment rather than migrated to. One
static executable, around 2.5 MB, with the panel compiled into it and no
interpreter and no runtime dependencies at all. If you are arriving from the
Python line, read [Coming from the Python line](Coming-from-the-Python-line)
first; everything else on this wiki describes 4.x.

---

## Pick your starting point

| If you… | Go to |
|---|---|
| already run **Breeze Core 3.x** | [Coming from the Python line](Coming-from-the-Python-line) |
| run **Linux** and want updates through your package manager | [Installing from packages](Installing-from-packages) |
| already run **containers** | [Installing with containers](Installing-with-containers) |
| run **Windows** | [Installing on Windows](Installing-on-Windows) |
| run **FreeBSD, NetBSD or OpenBSD** | [Installing on the BSDs](Installing-on-the-BSDs) |
| run **OPNsense** | [Installing on OPNsense](Installing-on-OPNsense) |
| run **macOS, NixOS, or anything unusual** | [Installing from source](Installing-from-source) |
| have it installed and want to pair your units | [First run and pairing](First-run-and-pairing) |
| are writing a client, or automating it | [REST API](REST-API) · [Control schema](Control-schema) |
| want to reach it from outside the house | [Exposing it safely](Exposing-it-safely) — read this **first** |
| have something broken | [Troubleshooting](Troubleshooting) |

---

## What it is

Three decoupled components sharing exactly one contract, the `/api/*` endpoints.
Delete any client and the rest keeps working.

| Component | What it is |
|---|---|
| **Server** | A Rust binary — the only stateful part, and a standalone REST API that neither knows nor cares that a UI exists. |
| **Web panel** | The same vanilla-JS ES modules as before, now compiled into the executable. Still just an HTTP client. |
| **CLI** | `breeze-core` itself: pairing, control, diagnostics, the admin side of enrolment. See [Command-line tools](Command-line-tools). |
| **Breeze for Android** | Optional native app: one unit per screen, home-screen widgets, Android Auto, programs, diagnostics. [Its own repo ↗](https://github.com/monikapurpl3/breeze) |

The Midea LAN protocol — discovery, the V3 handshake, the framing, the
encryption — is implemented directly in `breeze-proto`, so there is no
[msmart-ng](https://github.com/mill1000/midea-msmart) and no Python underneath
any more. HTTP, TLS, JSON and the scheduler are the standard library plus a small
set of crates. Layer by layer: [Architecture](Architecture).

## What you get out of the box

- **Control from anywhere on your LAN** — browser, Android app, home-screen
  widgets, Android Auto, REST, `curl`, cron.
- **Automation that runs on the server** — favourites, schedules and
  temperature curves fire whether or not your phone is home, charged, or awake.
  See [Programs, schedules, curves](Programs-schedules-and-curves).
- **Live state without polling** — Server-Sent Events push every change, whether
  it came from you, a schedule, or another client. The poller idles until
  somebody subscribes, so a server nobody is watching makes no LAN traffic.
- **Two-credential access** with Ed25519 request signing, LAN-gated admin
  approval, per-device revocation. See
  [Authentication and pairing](Authentication-and-pairing).
- **Real diagnostics** — `breeze-core diag` checks auth posture, per-unit
  latency, capability probing and input validation, and the same battery is
  mirrored in the app.
- **One file to install** — no interpreter, no virtualenv, no wheels, no
  `python3-*` dependencies. Every Linux package declares **zero dependencies**,
  and so do all three BSD packages.
- **Packages, not tarballs** — deb, rpm, pacman, apk, OpenWrt ipk, FreeBSD,
  NetBSD, OpenBSD, an OPNsense plugin, a Windows installer, and a signed
  repository so updates arrive the normal way.
- **Quiet by default** — the beep is off unless a client asks for it, so a 3 a.m.
  setpoint change wakes nobody.

No account, no telemetry, no cloud callbacks after pairing, no "pro" tier. It is
[AGPL-3.0](https://github.com/monikapurpl3/breeze-core-native/blob/main/LICENSE).

## What the rewrite actually changed

Nothing about the contract, and quite a lot about what it costs to run. The
detail is in [Version history](Version-history) and
[Ports and architectures](Ports-and-architectures); the summary:

| | 3.2.0 (Python) | 4.0.0 (Rust) |
|---|---|---|
| what installs | interpreter + ~40 wheels, or a ~25 MB frozen bundle | one ~2.5 MB executable |
| package dependencies | `python312` / `python311` and friends | **none** |
| resident memory | ~62 MB | **~2.4 MB** |
| disk installed | ~59 MB | **~3 MB** |
| architectures with real packages | amd64, arm64, armhf | those **plus riscv64, ppc64le, s390x** |
| ppc64le / s390x / riscv64 | proof-of-concept, built under QEMU, frozen | tier-1, cross-built, published every release |
| OpenBSD | source install into a virtualenv | a signed `pkg_add` package |
| build | emulate the target, compile everything on it | cross-compile with `cargo-zigbuild`, no emulation |

<sub>Both figures are from the same x86-64 server, running the same three
paired units, before and after the in-place upgrade. What `dnf` said while doing
it: <i>Total size of inbound packages is 1 MiB… After this operation, 56 MiB
will be freed (install 3 MiB, remove 59 MiB).</i></sub>

## What it is not

Worth knowing before you invest an evening — the longer version, with the full
trade-off table, is in [Compared to NetHome Plus](Compared-to-NetHome-Plus).

- It needs **a machine that is always on**, and you are now its sysadmin.
- **Pairing is not fully local.** Units join Wi-Fi through the vendor app, and V3
  units need one internet-connected discovery run to fetch their token and key.
  Everything after that is offline — back those credentials up.
- **Nothing is exposed to the internet by default.** Control from away means a
  VPN (recommended) or a proxy you secure yourself.
- **The feature ceiling is your firmware's.** No filter reset, no energy
  dashboard, and some units silently ignore horizontal swing.
- **The native app is Android-only.** iOS and desktop get the web panel.
- **It is not a home-automation platform.** One brand, one job. If you already
  run Home Assistant, its Midea integration may suit you better.
