# Version history

The 4.x line. For 3.x and earlier, see the Python project's own
[version history](https://github.com/monikapurpl3/breeze-core/wiki/Version-history) —
everything published for it stays where it is and keeps installing.

## 4.0.1

A bug-fix release, and most of what it fixes was found by a tool rather than by
reading code.

`packaging/test/differential.py` runs 4.x and a live 3.2.0 against the *same*
`config.json` and diffs every endpoint — status code, JSON type, shape, value.
It started at **142 differences** and ends at none that are not documented with
a reason. That check exists because a unit test agrees with whatever the code
does, so it cannot catch a misunderstanding of the contract; the reference is
still installable, so the honest test is to ask both and compare.

**Fixed**

- **A bad enum answered `422`; the reference answers `400`.** The split is
  inherited rather than designed — pydantic rejected numeric bounds while
  parsing, so FastAPI produced its own `422`, while an unknown mode name
  survived parsing and was refused with an explicit `400`. Answering one status
  for everything is invisible until a client branches on it.
- **The bad-enum message wording**, back to the reference's `Unknown mode: X`.
- **`/api/system` reported a null init system on a hardened deployment.** It read
  `/proc/1/comm` and nothing else, which `ProtectProc=invisible` makes
  unreadable — and that directive is in the project's own hardening drop-in. It
  also meant a null init on macOS, Windows and all three BSDs, which have no
  `/proc` at all. Now checks `/run/systemd/system` first, as the reference does,
  plus OpenRC, procd, runit and s6 markers, with pid 1 kept as the last resort
  for the container case where the server itself *is* pid 1.
- **Six `settings.*` fields were missing from `/api/system`** — the ones 4.0.0
  turned into constants. "No longer configurable" is not "no longer a fact": the
  panel displays them by name and showed blanks. Reported with their fixed
  values.
- **`os.*` used our own field names** where the reference has established ones,
  so `distro_id`, `distro_version`, `pretty_name`, `system`, `hostname`,
  `kernel_version` and `libc` all read blank in the panel.
- **Also added to `/api/system`:** `network.bind_host` / `bind_port`,
  `cpu.cores` / `arch` / `endianness`, `paths.package`, `server.installed_at`,
  and per-unit `online` and cached `capabilities`.
- **`capabilities` sent `30.0` where the reference sends `30`.** JSON has one
  number type and consumers do not: invisible to JavaScript, fatal to a client
  that asks for an integer.
- **`b64_decode` accepted url-safe base64 only; the reference accepts both.**
  Python's `urlsafe_b64decode` leaves an already-present `+` or `/` valid, so
  standard base64 has always worked against 3.x. Refusing it broke such clients
  *intermittently* — a signature encodes without a `+` or `/` roughly 7% of the
  time, so about one request in fourteen succeeded, which is far worse than
  failing outright. Neither the web panel nor the Android app was affected; both
  already emit base64url.

**Added**

- **An access log, on by default.** One line per request: peer, method, path,
  status, duration. 4.0.0 logged a single line per process lifetime, so a
  misbehaving deployment left nothing to look at. `BREEZE_LOG=0` silences it.
- **`BREEZE_DEBUG=1`** traces the control path — the JSON a client sent, what the
  unit reported before the merge, the setpoint sent to it, what the unit echoed
  back — and dumps the exact canonical string behind any rejected signature.
- **A reference byte vector for the control frame.** `get_state` was pinned to
  msmart-ng's real bytes from the start; `set_state`, the frame that actually
  changes a unit, had none.
- **Container images** — see below.

**Not in the 4.0.1 release**

No FreeBSD, NetBSD, OpenBSD or OPNsense packages: those are built on real
machines and the build hosts were unreachable. They will be added to the 4.0.1
release rather than held for the next one.

### Containers, new in 4.0.1

Two images replacing the Python line's five:

| Tag | Base | Size |
|---|---|---|
| `4.0.1` | `scratch` | **5.9 MB** |
| `4.0.1-debug` | `busybox:musl` | 8.4 MB |

Against 137 MB for the Alpine image and 291 MB for UBI. All five of the old ones
existed because an interpreter needs a distribution around it, and that reason is
gone. amd64 and arm64. Details: [Installing with containers](Installing-with-containers).

## 4.0.0

The Rust reimplementation. Same REST API, same store files, same `AC_*`
variables, same service name, same panel, same Android app — meant to be
installed *over* a 3.x deployment rather than migrated to.

| | 3.2.0 | 4.0.0 |
|---|---|---|
| what installs | interpreter + ~40 wheels, or a ~25 MB frozen bundle | one ~2.5 MB executable |
| package dependencies | `python312` / `python311` and friends | **none** |
| resident memory | ~62 MB | **~2.4 MB** |
| disk installed | ~59 MB | **~3 MB** |
| architectures with packages | amd64, arm64, armhf | those **plus riscv64, ppc64le, s390x** |
| OpenBSD | source install into a virtualenv | a signed `pkg_add` package |
| build | emulate the target, compile on it | cross-compile, no emulation |

Those memory and disk figures are measured on one x86-64 server with the same
three units paired, before and after the in-place upgrade.

**What changed underneath**

- **The Midea LAN protocol is implemented directly.** No
  [msmart-ng](https://github.com/mill1000/midea-msmart), no Python: V2/V3
  framing, the handshake, the encryption and discovery all live in
  `breeze-proto`, pinned against the reference implementation's own byte vectors.
- **The panel is compiled in.** A build script generates an `include_bytes!`
  table from `static/`, so there is no directory to deploy and no
  `WorkingDirectory` to get wrong.
- **`breeze-store` reads and writes 3.x's files byte-for-byte**, including
  fields it does not use and `null` for an absent optional rather than omitting
  the key. That is what makes the upgrade an upgrade.
- **No async runtime, no web framework, no TLS server.** Fourteen external
  crates. See [Architecture](Architecture).

**New**

- `breeze-core control` — set a unit from the shell, positionally, without
  `curl` and a JSON body.
- `breeze-core login` — enrol a machine's CLI as its own device.
- `/metrics` in Prometheus format.
- A signed FreeBSD `pkg` repository and the project's first OpenBSD **package**;
  the Python line could only manage a source install there.
- riscv64, ppc64le and s390x as first-class packages rather than
  proof-of-concept builds. See [Ports and architectures](Ports-and-architectures).

**What went away**

- **`/docs`.** No framework, so no generated OpenAPI schema. [REST API](REST-API)
  is the reference and it is complete.
- **The zsh tools**, replaced by subcommands. The flags they took are still
  accepted.
- **Brotli.** gzip only, deliberately.
- **The ability to loosen some settings** — LAN-only admin approval, the code
  TTL, the token TTL and the clock-skew window are constants now.

## Version numbers

4.0.0 follows 3.2.0 because the implementation changed, not because the contract
did. Nothing about the wire format, the store files or the service name moved, so
a client cannot tell which is answering except by `GET /api/version`.

The major bump is a warning about what you are installing, not about what you
will have to change.
