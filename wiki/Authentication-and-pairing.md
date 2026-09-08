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

**It lives for 60 seconds.** Not configurable. A pairing code is something
somebody is actively typing; a longer window is only useful to somebody who is
not.

### The credential

- **256-bit**, stored **SHA-256-hashed** in `devices.json` — the plaintext is
  handed out exactly once and never written down.
- **90-day lifetime**, not configurable.
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

Both base64 alphabets are accepted — standard and url-safe, padded or not. The
web panel and the Android app both emit base64url; accepting standard too is
what the reference does, and refusing it broke non-browser clients about
thirteen times in fourteen before 4.0.1.

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

The server includes its own time so a client can compute its offset and retry.
That is why the skew window is a constant rather than a setting — a wider window
is a wider replay window, and the self-healing response fixes the actual problem
instead.

## Admin actions are LAN-only, unconditionally

Approving a pairing, listing devices and revoking one all require a **private
source address**: loopback, RFC1918, or link-local. There is no setting to turn
this off. 3.x had `AC_ENROLL_LAN_ONLY`, and it existed only to make a bad
configuration possible.

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

## Rate limiting — a known gap

**4.x does not rate-limit the enrolment endpoints. 3.x does.** Stated plainly
because it is a regression, not a design decision:

| Endpoint | 3.2.0 | 4.x |
|---|---|---|
| `POST /api/auth/enroll/start` | 10 per minute per address | unlimited |
| `POST /api/auth/enroll/poll` | 120 per minute per address | unlimited |
| `POST /api/auth/enroll/approve` | 10 per minute per address | unlimited |

What still protects a pairing code: it is 8 characters of base32 — 40 bits —
**single-use**, dead after **60 seconds**, compared in constant time,
and stored only as a hash — and approving one requires a private source address
regardless. Guessing it inside the window is improbable. It is simply not also
*slow*, which it was in 3.x.

Also note the API key itself has never been rate-limited in either
implementation. If the server is reachable from anywhere you do not control,
that is what a reverse proxy's own limiter is for — see
[Exposing it safely](Exposing-it-safely).

A wrong, expired or already-used code all return the **same** `404`. Telling
them apart would tell a guesser which of the three they had achieved.

## If you lose things

| Lost | Consequence |
|---|---|
| the API key | read it from `config.json`; it is not derived from anything |
| `devices.json` | every client re-pairs. Annoying, not serious. |
| **`config.json`** | **the serious one** — a paired V3 unit's token and key were issued once by a cloud that no longer hands them out. Back this file up. |

That last row is why every page here says to back up the config directory. It is
not the API key that is irreplaceable; it is the units.
