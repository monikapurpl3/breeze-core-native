# Exposing it safely

Breeze Core is **LAN-first**. It binds `127.0.0.1` out of the box, and the
safest deployment is the one where that never changes.

This page is about what to do when you want it reachable from outside the
house anyway — and about being honest that there is a cheaper answer than
most of it.

## Read this first

**A VPN is better than everything on this page.** WireGuard or Tailscale on
your phone, the server left bound to the LAN, nothing published. No TLS
certificate to renew, no reverse proxy to misconfigure, no attack surface to
reason about, and the LAN-only admin checks keep meaning what they say. If that
is available to you, take it and stop reading.

What follows is for when it genuinely is not.

## The four settings that matter

| | |
|---|---|
| `BREEZE_HOST` | **never `0.0.0.0`.** Bind loopback with a proxy in front, or one specific LAN address |
| `AC_BEHIND_PROXY` | `1` **only** when a proxy you control is in front. See below — getting this wrong is the one failure that quietly disables a control |
| `AC_SECURITY_HEADERS` | leave on unless the proxy sets the same headers itself |
| `AC_MIN_AUTH_VERSION` | `2` once every client signs its requests. See [Signed auth (v2) migration](Signed-auth-v2-migration) |

## What is already true without configuring anything

Worth knowing, because it changes what you actually need to add.

- **Control needs two credentials.** The API key gets a client as far as
  *starting* a pairing and no further; a per-device credential is what
  authorises control. Leaking the key does not let anyone touch an air
  conditioner.
- **Admin actions are LAN-only, unconditionally.** Approving a pairing, listing
  credentials and revoking one all require a private source address, and there
  is no setting to turn that off. The failure is a `403`, not a `401`.
- **There is no CORS middleware, deliberately.** The panel is same-origin.
  Permissive CORS would let any other page in your browser drive the API.
- **`/docs` does not exist**, so there is no generated schema to enumerate.
- **Every secret comparison is constant-time**, and only hashes of credentials
  are stored.
- **The hardening headers are on by default**:

  ```
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
  Referrer-Policy: no-referrer
  Content-Security-Policy: default-src 'self'; base-uri 'none';
      frame-ancestors 'none'; object-src 'none'; form-action 'self'
  Strict-Transport-Security: max-age=63072000; includeSubDomains
  ```

  The CSP is strict enough that inline styles and inline scripts are refused,
  which is why the panel has neither. `Strict-Transport-Security` is ignored
  over plain HTTP and takes effect once something terminates TLS in front.

  **Do not set these in both places.** Two `Content-Security-Policy` headers
  are *intersected*, not deduplicated, and the result is usually a policy
  neither side intended. Either leave `AC_SECURITY_HEADERS` on and have the
  proxy add nothing, or set `AC_SECURITY_HEADERS=0` and own them in the proxy.

## The one that bites: `--behind-proxy`

Admin actions require a **private** source address. A reverse proxy is itself
on a private address. So without `AC_BEHIND_PROXY=1`, every proxied request
passes the LAN-only check — the control is still there, still returning `403`
to nobody, and no longer protecting anything.

Set it, **and** make the proxy send the real client address, **and** check the
result:

```sh
curl -s -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  https://your.host/api/system \
  | grep -oE '"(client_ip|client_is_private|behind_proxy_enabled)":[^,}]*'
```

`client_ip` must be your actual address. If it reads `127.0.0.1` from a phone
on mobile data, this is broken right now.

> `--behind-proxy` is one-way: it can turn proxy trust **on**, never off. A
> stray flag cannot quietly undo `AC_BEHIND_PROXY=1` and convert the LAN-only
> check into a check that every proxied request passes.

**And the proxy must overwrite the header, not append to it.** If it passes
through a client-supplied `X-Forwarded-For`, anyone can claim to be
`192.168.1.5` and approve their own pairing. In nginx that means
`proxy_set_header X-Forwarded-For $remote_addr;` — `$proxy_add_x_forwarded_for`
appends, and appending is the vulnerable form here.

## Defence in depth, in the order I would add it

### 1. Restrict the admin paths at the proxy

The strongest single measure, and it does not depend on the app getting
anything right:

The three LAN-only routes are `POST /api/auth/enroll/approve`,
`GET /api/auth/devices` and `DELETE /api/auth/devices/{token_id}`. Note the
`enroll/` in the middle of the first one — a block written against
`/api/auth/approve` matches nothing and protects nothing:

```nginx
location ~ ^/api/auth/(enroll/approve|devices) {
    allow 192.168.1.0/24;
    allow 127.0.0.1;
    deny all;
    proxy_pass http://127.0.0.1:8420;
}
```

`enroll/start` and `enroll/poll` are deliberately *not* in there: a client
being paired has to reach them from wherever it is, and they need the API key.
It is the **approval** that must come from your network.

Now approval and credential management are unreachable from outside regardless
of what the app thinks the client address is. This is what makes a missing
`--behind-proxy` a lapse in depth rather than an open door.

### 2. Rate-limit the enrolment endpoints

**The server does not rate-limit anything.** A pairing code is 40 bits,
single-use, dead in 60 seconds, hashed, and compared in constant time — so
guessing one is improbable — but it is no longer also *slow*, and neither is
guessing the API key.

Put a limiter in front:

```nginx
limit_req_zone $binary_remote_addr zone=breeze_enroll:10m rate=10r/m;

location /api/auth/ {
    limit_req zone=breeze_enroll burst=5 nodelay;
    proxy_pass http://127.0.0.1:8420;
}
```

Ten a minute is generous for a human typing a code and hostile to a script.

### 3. Turn off what you are not using

If nothing subscribes to the event stream, nothing needs `/api/units/stream`
exposed. If you never use the panel from outside, expose only `/api/`. A path
that is not reachable cannot be got wrong.

### 4. `AC_MIN_AUTH_VERSION=2`

Once every client signs its requests, v1 bearer tokens stop being accepted and
no reusable secret crosses the wire at all. Do this *after* migrating clients,
not before — see [Signed auth (v2) migration](Signed-auth-v2-migration).

## Hardening without systemd

The packaged systemd unit ships a sandbox: `ProtectSystem=strict`,
`ProtectHome`, `PrivateTmp`, an empty `CapabilityBoundingSet`,
`SystemCallFilter=@system-service`, and `ReadWritePaths=/etc/breeze-core` as
the only writable path. There is also a commented-out egress lockdown to
uncomment and adjust:

```ini
IPAddressAllow=127.0.0.0/8 ::1/128 192.168.0.0/16
IPAddressDeny=any
```

That is worth doing: the server has no business reaching anything but your LAN
once pairing is done.

**On any other init system you have none of that**, and the substitutes are:

| Platform | Instead |
|---|---|
| OpenRC / runit / s6 | a strict firewall, and the unprivileged `breeze` account the packages already create |
| FreeBSD | a thin **jail** — the strongest isolation available there |
| OpenBSD | `pf` for egress, and the `_breeze` account |
| Windows | the service runs as a dedicated account; use Windows Firewall for egress |
| containers | `--read-only` with a volume for `/etc/breeze-core`, `--cap-drop ALL`, and the distroless image, which has no shell to get |

## The go-live checklist

- [ ] `BREEZE_HOST` is loopback or one specific address — **not `0.0.0.0`**
- [ ] TLS terminates somewhere, and HTTP redirects to it
- [ ] `AC_BEHIND_PROXY=1` **and** `/api/system` shows your real `client_ip`
- [ ] the proxy sets `X-Forwarded-For` from `$remote_addr`, not appending
- [ ] `/api/auth/enroll/approve` and `/api/auth/devices` restricted to the LAN
      at the proxy (mind the `enroll/`)
- [ ] a rate limit on `/api/auth/`
- [ ] security headers set in exactly **one** place
- [ ] `/etc/breeze-core` owned by `breeze`, mode `750`
- [ ] **`config.json` backed up somewhere off the machine** — a paired unit's V3
      credentials cannot be re-issued
- [ ] `breeze-core diag` passes from outside
- [ ] you have decided about `AC_MIN_AUTH_VERSION=2`

## What is genuinely not protected

Stated plainly, because a checklist implies completeness and this one is not
complete:

- **No rate limiting anywhere in the server.** See above.
- **No account lockout, and no notion of accounts.** Credentials are per
  device, revocable individually, and that is the whole model.
- **The API key is in `config.json` in plaintext.** It has to be — clients send
  it. `0640`, owned by `breeze`.
- **No audit log.** The access log records requests, not who did what.
- **A device credential is a bearer secret under auth v1.** Anything that can
  read it can use it until you revoke it. That is what v2 exists to fix.

Next: [Reverse proxy and TLS](Reverse-proxy-and-TLS) for the actual
configurations.
