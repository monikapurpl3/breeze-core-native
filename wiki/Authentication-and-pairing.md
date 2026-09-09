# Authentication and pairing

Two credentials, and control needs both. Neither one alone is enough, and that
is the whole design.

| | What it is | What it gets you |
|---|---|---|
| **API key** | one shared secret, in `config.json` | permission to *begin* pairing, and nothing else |
| **Device credential** | one per client, revocable, expiring | permission to control units |

The API key is an **enrolment secret**. Leaking it does not let anyone touch an
air conditioner: they can start a pairing, which then needs an admin on your LAN
to approve it. That is the point of splitting the two.

## Pairing, end to end

RFC 8628-style — the same shape as putting a code into a TV app.

```
client                              server                        admin on the LAN
  │                                   │                                  │
  ├─ POST /api/auth/enroll/start ────►│  (needs the API key)             │
  │◄──── session_id, user_code ───────┤                                  │
  │      "BONG-W3GN", 60 seconds      │                                  │
  │                                   │                                  │
  │      shows the code to a human ───┼─────────────────────────────────►│
  │                                   │◄─ POST /api/auth/enroll/approve ─┤
  │                                   │   (must come from a private address)
  ├─ POST /api/auth/enroll/poll ─────►│                                  │
  │◄──── the device credential ───────┤                                  │
```

```sh
# on the server, or anywhere on the LAN
sudo breeze-core approve BONG-W3GN
```

### The code

Eight characters, hyphenated in the middle: `BONG-W3GN`. Standard base32 —
`A`–`Z` and `2`–`7` — so `0`, `1`, `8` and `9` never appear, because a human
reading a code aloud confuses them with `O`, `I`, `B` and `g`. Five random bytes,
40 bits, exactly one base32 group so there is no padding. Stored **hashed**,
compared in constant time, single-use.

**It lives for 60 seconds**, or whatever `AC_CODE_TTL` says. Sixty is the right
answer for a code somebody is actively typing, and a longer window is mostly
useful to somebody who is not — but a slow phone on a slow network is a real
thing, so the number is yours to set.

### The credential

- **256-bit**, stored **SHA-256-hashed** in `devices.json` — the plaintext is
  handed out exactly once and never written down.
- **Ten-year lifetime** by default (`AC_TOKEN_TTL_DAYS`, in days; set it to `0`
  for credentials that never expire). Long on purpose: the failure mode of a
  short lifetime is a phone that stops working while you are away from the LAN
  that could re-approve it.
- **Revocable individually**, by `token_id`, without disturbing any other
  client.
- Carries a **label** so the list is readable: "Monique's Phone", not a hash
  prefix.

```sh
breeze-core devices                # what is enrolled
breeze-core revoke 7f3c9a21        # one client, immediately
```

## Two auth versions

Both are accepted by default. `AC_MIN_AUTH_VERSION=2` refuses v1 with `426
Upgrade Required` — see [Signed auth (v2) migration](Signed-auth-v2-migration).

### v1 — bearer token

```
X-API-Key: <key>
Authorization: Bearer <token>
```

Simple, and it puts a reusable secret on the wire every request. Fine over a
VPN or on the LAN; less fine through anything you do not control.

### v2 — Ed25519 request signing

The client generates a keypair, registers only the **public** half, and signs
every request. Nothing secret crosses the wire in either direction, ever.

```
X-API-Key:              <key>
X-Breeze-Auth-Version:  2
X-Breeze-Key-Id:        <token_id>
X-Breeze-Timestamp:     <unix seconds>
X-Breeze-Nonce:         <base64url, unpadded>
X-Breeze-Signature:     <base64url Ed25519 signature>
```

The signature covers a canonical string of exactly six newline-separated fields:

```
breeze-auth-v2\n{METHOD}\n{path?query}\n{timestamp}\n{nonce}\n{sha3_512(body) hex}
```

Four things about that string are load-bearing:

- **The path includes the query.** Signing the path alone would let a query be
  swapped after the fact.
- **The body digest is SHA3-512**, not SHA-2. WebCrypto implements no SHA-3 at
  all, which is why the web panel ships a hand-written Keccak. Using the wrong
  family means the client and server disagree about every request.
- **The timestamp bounds replay** to a window either side of the server's clock.
- **The nonce makes each request single-use** inside that window; the server
  keeps a cache sized from it.

Both base64 alphabets are accepted — standard and url-safe, padded or not.

**In the browser, the private key is non-extractable.** WebCrypto creates it so
that it can sign and will not hand the bytes back to the page that made it. It
lives in IndexedDB. On Android it is a 32-byte seed in Keystore-wrapped storage.

### A bad clock is recoverable

A rejected signature normally tells you nothing, deliberately. The one exception
is clock skew:

```json
{ "detail": { "error": "clock_skew", "retryable": true,
              "server_time": 1788889209.7, "max_skew_seconds": 60 } }
```

The server includes its own time so a client can compute its offset and retry,
which is why the default window is only 60 seconds each way: a wider window is
a wider replay window, and this response fixes the actual problem instead.
`AC_AUTH_SKEW_SECONDS` widens it for a client that cannot correct its own clock
— an embedded thing with no RTC and no NTP — at that cost.

## Admin actions are LAN-only, unconditionally

Approving a pairing, listing devices and revoking one all require a **private
source address**: loopback, RFC1918, or link-local. There is no setting to turn
this off.

The failure is a **`403`, not a `401`** — the credential was fine, the *location*
was not, and a client retrying with better credentials would never succeed.

> **Behind a reverse proxy this needs care.** Without `AC_BEHIND_PROXY=1` the
> server sees the proxy's address, which is itself private — so every proxied
> request passes the LAN check and it stops meaning anything at all. With it set,
> the proxy must actually send `X-Forwarded-For` from the real client. Check
> `connection.client_ip` on `GET /api/system`: if it reads `127.0.0.1` when you
> connected from a phone, this is broken right now.

## What the server stores, and what it never does

| | |
|---|---|
| `config.json` | the API key in plaintext — it has to be, clients send it |
| `devices.json` | credential **hashes** only, plus label, dates, `auth_version`, and the public key for v2 |
| never | a device credential in plaintext, anywhere, after handing it out once |
| never | any of it in `/api/config` or `/api/system` |

Every secret comparison — API key, code hash, token hash — is constant-time.
`breeze-core diag` greps the diagnostic responses for secret-looking values, so
the omissions are checked rather than trusted.

## What stops somebody guessing a pairing code

A code is 40 bits of base32, **single-use**, dead after **60 seconds**, compared
in constant time, and stored only as a hash. Approving one additionally requires
a private source address. A wrong, expired and already-used code all return the
same `404` — telling them apart would tell a guesser which of the three they had
achieved.

**The enrolment endpoints are not rate-limited.** If the server is reachable
from anywhere you do not control, put a limiter in the reverse proxy in front of
it — see [Exposing it safely](Exposing-it-safely). The same applies to the API
key, which has never been rate-limited.

## If you lose things

| Lost | Consequence |
|---|---|
| the API key | read it from `config.json`; it is not derived from anything |
| `devices.json` | every client re-pairs. Annoying, not serious. |
| **`config.json`** | **the serious one** — a paired V3 unit's token and key were issued once by a cloud that no longer hands them out. Back this file up. |

That last row is why every page here says to back up the config directory. It is
not the API key that is irreplaceable; it is the units.
