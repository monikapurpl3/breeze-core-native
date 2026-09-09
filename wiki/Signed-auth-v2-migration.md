# Signed auth (v2) migration

**This is optional.** Both auth versions are accepted by default and there is
no deprecation date. If your server is on the LAN or behind a VPN, v1 is fine
and you can close this page.

It is worth doing if the server is reachable from anywhere you do not control.

## The difference in one paragraph

**v1** sends a bearer token with every request. It is simple, and it means a
reusable secret crosses the wire every time — so anything that can read one
request can replay it, and can keep doing so until you revoke the credential.

**v2** has the client generate an Ed25519 keypair, register only the **public**
half, and sign every request. Nothing secret crosses the wire in either
direction, ever. A captured request cannot be replayed, because the signature
covers a timestamp and a single-use nonce.

## What a v2 request looks like

```
X-API-Key:              <the key>
X-Breeze-Auth-Version:  2
X-Breeze-Key-Id:        <token_id>
X-Breeze-Timestamp:     <unix seconds>
X-Breeze-Nonce:         <base64url, unpadded>
X-Breeze-Signature:     <base64url Ed25519 signature>
```

The signature covers a canonical string of exactly six newline-separated
fields:

```
breeze-auth-v2\n{METHOD}\n{path?query}\n{timestamp}\n{nonce}\n{sha3_512(body) hex}
```

Four things about that string are load-bearing, and each is a way to get it
wrong:

- **The path includes the query.** Signing the path alone would let a query
  string be swapped after the fact.
- **The body digest is SHA3-512**, not SHA-2. Reach for the wrong family and
  the client and server disagree about every request, with no clue as to why.
  This is the single most common mistake — see the note below.
- **The timestamp bounds replay** to a window either side of the server's
  clock (`AC_AUTH_SKEW_SECONDS`, 60 s by default, each way).
- **The nonce makes each request single-use** inside that window. The server
  keeps a cache sized from the skew setting.

Both base64 alphabets are accepted — standard and url-safe, padded or not.

> **WebCrypto implements no SHA-3 at all.** Not SHA3-512, not any of it. That
> is why the web panel ships a hand-written Keccak implementation. If you are
> writing a browser client, budget for that; if you are writing anything else,
> check your library really gives you SHA3-512 and not SHA-512, which is a
> different function with a confusingly similar name.

## Migrating a client

An already-enrolled v1 client upgrades itself in place, without re-pairing and
without an admin having to approve anything again:

1. generate an Ed25519 keypair locally;
2. `POST /api/auth/upgrade` **authenticated with the existing v1 credential**,
   sending the public key;
3. from then on, sign requests.

```sh
curl -X POST http://server:8420/api/auth/upgrade \
  -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"public_key": "<base64 Ed25519 public key, 32 bytes>"}'
```

```json
{ "token_id": "7f3c9a21", "auth_version": 2 }
```

The credential keeps its `token_id`, its label and its expiry. Only how it
proves itself changes.

**The private key never leaves the client**, and the server has no way to ask
for it. In a browser, WebCrypto creates it non-extractable — it can sign and
will not hand the bytes back to the page that made it — and it lives in
IndexedDB. On Android it is a 32-byte seed in Keystore-wrapped storage.

### Errors from `upgrade`

| Code | Means |
|---|---|
| `401` | no authenticated device — you called it without a working v1 credential |
| `400` | `public_key` is not a valid Ed25519 public key |
| `404` | the credential authenticated but is no longer in the store |
| `422` | the body is not the expected shape |

## Enrolling as v2 from the start

A new client can skip v1 entirely: pass `auth_version: 2` and its public key
when it *starts* enrolment, and it never holds a bearer token at all.

```sh
curl -X POST http://server:8420/api/auth/enroll/start \
  -H "X-API-Key: $KEY" -H 'Content-Type: application/json' \
  -d '{"label": "Kitchen tablet", "auth_version": 2,
       "public_key": "<base64 Ed25519 public key>"}'
```

Approval and polling are unchanged, and the poll response carries no
`device_token` — there is nothing to hand out, which is the point. A malformed
public key here is a `422`, treated as the client's mistake rather than a
server failure.

That is the better path for anything new. `upgrade` exists for clients that
already exist.

## Which of your clients are on which

```sh
breeze-core devices
```

Or `GET /api/auth/devices` — each record carries its `auth_version`. Work
through the list before doing the next part.

## Turning v1 off

Once **every** client signs its requests:

```sh
# /etc/breeze-core/breeze-core.env
AC_MIN_AUTH_VERSION=2
```

Restart, and a v1 credential is refused with **`426 Upgrade Required`** — a
distinct status precisely so a client can tell "you need to upgrade" from
"your credentials are wrong" and act on it rather than deleting them.

> **Check the list first.** Setting this with a v1 client still out there locks
> that client out until you either upgrade it or set the value back. That is
> recoverable — nothing is destroyed — but it is an avoidable hour of
> confusion, and it will be the phone belonging to whoever is least interested
> in your configuration file.

Clients in the wild:

| Client | v2 |
|---|---|
| the web panel | yes, and it prefers v2 |
| the Android app | yes |
| `breeze-core` CLI | yes, via `breeze-core login` |
| anything you wrote | you tell me |

## Debugging a signature that will not verify

The server deliberately tells you nothing about *why* a signature failed —
except for clock skew. So the way in is to make the server show its work:

```sh
sudo systemctl stop breeze-core
sudo -u breeze BREEZE_DEBUG_AUTH=1 /usr/bin/breeze-core serve \
  --host 127.0.0.1 --port 8420
```

That logs the canonical string the server built, byte for byte. Compare it with
what your client signed. In order of likelihood, the difference is:

1. **the body digest** — SHA-512 instead of SHA3-512;
2. **the query string** — dropped from the signed path, or re-ordered;
3. **an empty body** — it is still `sha3_512("")`, not an empty digest field;
4. **the timestamp** — seconds, not milliseconds;
5. **base64 padding or alphabet** — this one is *not* it, both are accepted.

### The one error that tells you something

Clock skew is reported in full, because it is the only auth failure a client
can genuinely fix by itself:

```json
{ "detail": { "error": "clock_skew", "retryable": true,
              "server_time": 1788889209.7, "max_skew_seconds": 60 } }
```

The server hands over its own time so the client can compute its offset and
retry. **Fix the clock rather than widening the window** — a wider
`AC_AUTH_SKEW_SECONDS` is a wider replay window by exactly as much. Widen it
only for a device that genuinely cannot know the time, like something with no
RTC and no NTP.

## What v2 does not do

- It does not encrypt anything. Use TLS as well —
  [Reverse proxy and TLS](Reverse-proxy-and-TLS).
- It does not replace the API key. Both are still required.
- It does not change the LAN-only rule on admin actions.
- It does not rate-limit anything. Nothing in the server does — see
  [Exposing it safely](Exposing-it-safely).

Full model: [Authentication and pairing](Authentication-and-pairing).
