# Security Policy

Breeze Core controls physical devices and, when exposed, faces the public
internet — security reports are very welcome.

## Reporting a vulnerability

**Please report privately — do not open a public issue.**

Use GitHub's private vulnerability reporting: the repository's **Security** tab
→ **Report a vulnerability**, or
[open a draft advisory](https://github.com/monikapurpl3/breeze-core-native/security/advisories/new).
Include:

- what the issue is and its impact;
- steps or a proof-of-concept to reproduce;
- the affected version and commit (`breeze-core --version` prints both) and how
  you are running it — distribution, container, behind nginx or Caddy, and so
  on.

Please give a reasonable window to fix before any public disclosure. There is
no bounty — this is a hobby project — but credit is gladly given.

## Scope

**In scope:** the API, auth and enrolment logic; the Ed25519 request signing
and its canonical string; the scheduler and timer runner; the web panel; the
Midea protocol implementation; privilege and isolation issues in the shipped
service units and packaging; and anything that lets an unauthorised party read
or control units.

**Out of scope:** issues that require an already-compromised host or LAN;
self-inflicted misconfiguration contrary to
[Exposing it safely](https://github.com/monikapurpl3/breeze-core-native/wiki/Exposing-it-safely);
and vulnerabilities in third-party dependencies — report those upstream, but do
tell us if we are using them unsafely.

## Known gaps, stated up front

These are documented rather than reported, so nobody needs to spend time
writing them up:

- **There is no rate limiting anywhere in the server.** A pairing code is 40
  bits, single-use, dead in 60 seconds, hashed and compared in constant time,
  so guessing one is improbable — but it is not also *slow*, and neither is
  guessing the API key. Put a limiter in your reverse proxy.
- **No audit log.** The access log records requests, not who did what.
- **The API key is stored in plaintext** in `config.json`, mode `0640`. It has
  to be: clients send it.
- **A v1 device credential is a bearer secret.** Anything that can read one can
  use it until it is revoked. Ed25519 signing (auth v2) is what fixes that, and
  `AC_MIN_AUTH_VERSION=2` makes it mandatory — see
  [Signed auth (v2) migration](https://github.com/monikapurpl3/breeze-core-native/wiki/Signed-auth-v2-migration).
- **The OPNsense GUI plugin's PHP and XML have never run on a real firewall.**

## Hardening

If you are exposing Breeze Core beyond your LAN,
**[Exposing it safely](https://github.com/monikapurpl3/breeze-core-native/wiki/Exposing-it-safely)**
is the review and go-live checklist, and
**[Reverse proxy and TLS](https://github.com/monikapurpl3/breeze-core-native/wiki/Reverse-proxy-and-TLS)**
has the configurations. Following them closes the common exposure risks.

The single most common real-world mistake is **a reverse proxy that does not
forward the client address**, or that appends to a client-supplied
`X-Forwarded-For` rather than overwriting it. Either one turns the LAN-only
admin check into a check that every proxied request passes. `GET /api/system`
reports what the server actually saw, which is the way to settle it.

## Handling of secrets

- Only **hashes** of device credentials and pairing codes are stored; a
  credential's plaintext is handed out once and never written down.
- Every secret comparison — API key, code hash, token hash — is
  **constant time**.
- `config.json` is `0640`, the other three stores are `0600`, and the modes are
  **enforced on every write** rather than set once at install time.
- **No telemetry, and no cloud callback after pairing.** The only outbound
  connection the server can make is fetching a V3 unit's credentials during
  pairing, and that is behind a build feature which can be compiled out
  entirely.
- `/api/system` and `/api/config` never contain a secret — not the API key, not
  a credential, not a unit's V3 token or key. `breeze-core diag` greps those
  responses for secret-looking values, so the omission is checked rather than
  trusted.
