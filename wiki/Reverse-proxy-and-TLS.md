# Reverse proxy and TLS

Breeze Core speaks **plain HTTP and has no TLS server**. That is deliberate:
terminating TLS well means certificate issuance, renewal, OCSP, cipher policy
and a decade of protocol history, all of which nginx, Apache and Caddy already
do better than a 2 MB appliance daemon ever would.

So if you want HTTPS, something goes in front. This page is the configuration.

> Before you put this on the internet, read
> [Exposing it safely](Exposing-it-safely). **A VPN is a better answer than
> everything here**, and this page assumes you have already decided against it.

## The four things a proxy in front of this must get right

Any proxy, in any configuration:

1. **`X-Forwarded-For`, overwritten — not appended.** This is the one the
   server reads, and the only one. `X-Real-IP` is ignored entirely.
2. **`AC_BEHIND_PROXY=1` on the server**, or that header is not trusted and the
   LAN-only admin check decides on the proxy's own address.
3. **No buffering on `/api/units/stream`.** Server-Sent Events are one response
   that never ends.
4. **Security headers in exactly one place.** Two `Content-Security-Policy`
   headers are *intersected*, not deduplicated.

Everything below is those four rules written out per proxy.

### Why overwritten and not appended

If the proxy passes a client-supplied `X-Forwarded-For` through, anyone can
claim to be `192.168.1.5` and approve their own pairing. The LAN-only check
would read the forged value and agree.

In nginx that is the difference between two variables that look
interchangeable and are not:

```nginx
proxy_set_header X-Forwarded-For $remote_addr;               # correct
proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for; # appends — do not
```

`$proxy_add_x_forwarded_for` is the usual idiom and the wrong one here.

## nginx

```nginx
server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name breeze.example.org;

    ssl_certificate     /etc/letsencrypt/live/breeze.example.org/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/breeze.example.org/privkey.pem;

    # The server already sends CSP, X-Frame-Options, Referrer-Policy and
    # nosniff. Do not repeat them here. HSTS is the one worth adding at the
    # edge, because only the edge knows TLS is in play.
    add_header Strict-Transport-Security "max-age=63072000; includeSubDomains" always;

    # --- admin: LAN only ---------------------------------------------------
    # Mind the enroll/ in the middle. A block written against
    # /api/auth/approve matches nothing and protects nothing.
    location ~ ^/api/auth/(enroll/approve|devices) {
        allow 192.168.1.0/24;
        allow 127.0.0.1;
        deny all;

        proxy_pass http://127.0.0.1:8420;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # --- the event stream: no buffering ------------------------------------
    location = /api/units/stream {
        proxy_pass http://127.0.0.1:8420;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 1h;   # the response never ends; do not time it out
        gzip off;
    }

    # --- everything else ---------------------------------------------------
    location / {
        proxy_pass http://127.0.0.1:8420;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}

server {
    listen 80;
    listen [::]:80;
    server_name breeze.example.org;
    return 301 https://$host$request_uri;
}
```

And on the server side:

```sh
# /etc/breeze-core/breeze-core.env
BREEZE_HOST=127.0.0.1
BREEZE_OPTS=--behind-proxy
```

Certificates with certbot: `sudo certbot --nginx -d breeze.example.org`. It
edits the `ssl_*` lines and sets up renewal.

## Caddy

Shorter, and it gets the certificate itself — which is the whole reason to
prefer it here.

```caddyfile
breeze.example.org {
	encode gzip

	header {
		Strict-Transport-Security "max-age=63072000; includeSubDomains"
		-Server
	}

	# Admin: LAN only. Note the enroll/ path.
	@admin path /api/auth/enroll/approve* /api/auth/devices*
	handle @admin {
		@notlan not remote_ip 192.168.0.0/16 10.0.0.0/8 172.16.0.0/12 127.0.0.1/8
		respond @notlan 403
		reverse_proxy 127.0.0.1:8420 {
			header_up X-Forwarded-For {remote_host}
			header_up X-Forwarded-Proto {scheme}
		}
	}

	# The event stream: never buffer it.
	@stream path /api/units/stream
	handle @stream {
		reverse_proxy 127.0.0.1:8420 {
			header_up X-Forwarded-For {remote_host}
			header_up X-Forwarded-Proto {scheme}
			flush_interval -1
		}
	}

	reverse_proxy 127.0.0.1:8420 {
		header_up X-Forwarded-For {remote_host}
		header_up X-Forwarded-Proto {scheme}
	}
}
```

**Set no `trusted_proxies` anywhere.** That is what makes Caddy *overwrite*
`X-Forwarded-For` with the real peer address. Adding a public range to
`trusted_proxies` quietly turns the LAN-only check off, because Caddy then
believes whatever the client claimed.

Caddy sets `X-Forwarded-For` by default even without the `header_up` line. It
is written out anyway so that removing it is a deliberate act rather than an
accident, and so the header that matters is visible in the file.

The container compose file `docker-compose.https.yml` ships this shape already
— see [Installing with containers](Installing-with-containers).

## Apache

```apache
<VirtualHost *:443>
    ServerName breeze.example.org

    SSLEngine on
    SSLCertificateFile    /etc/letsencrypt/live/breeze.example.org/fullchain.pem
    SSLCertificateKeyFile /etc/letsencrypt/live/breeze.example.org/privkey.pem

    Header always set Strict-Transport-Security "max-age=63072000; includeSubDomains"

    ProxyPreserveHost On
    RequestHeader set X-Forwarded-Proto "https"

    # mod_remoteip, so the forwarded address is the one that gets logged and
    # sent on. Without RemoteIPHeader, Apache appends and the value can be
    # forged.
    RemoteIPHeader X-Forwarded-For

    <Location ~ "^/api/auth/(enroll/approve|devices)">
        Require ip 192.168.1.0/24 127.0.0.1
    </Location>

    # The event stream must not be buffered or compressed.
    <Location "/api/units/stream">
        SetEnv no-gzip 1
        SetEnv proxy-sendchunked 1
    </Location>

    ProxyPass        / http://127.0.0.1:8420/
    ProxyPassReverse / http://127.0.0.1:8420/
</VirtualHost>
```

Apache needs `mod_proxy`, `mod_proxy_http`, `mod_headers`, `mod_remoteip` and
`mod_ssl` enabled.

## Check it, do not assume it

`GET /api/system` reports how *the request you just made* reached the server,
which is the only test that actually settles this:

```sh
curl -s -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  https://breeze.example.org/api/system \
  | grep -oE '"(client_ip|client_is_private|forwarded_for|behind_proxy_enabled|scheme)":[^,}]*'
```

What you want to see, from a phone on mobile data:

| Field | Should be |
|---|---|
| `client_ip` | your **real** public address |
| `client_is_private` | `false` from outside, `true` from the LAN |
| `behind_proxy_enabled` | `true` |
| `scheme` | `https` — meaning the proxy said so |

**If `client_ip` reads `127.0.0.1`, this is broken right now**, and the
LAN-only admin check is currently passing every proxied request.

Then check the stream really streams:

```sh
curl -N -H "X-API-Key: $KEY" -H "Authorization: Bearer $TOKEN" \
  https://breeze.example.org/api/units/stream
```

Events should trickle in. If nothing appears for a long time and then several
arrive at once, something is buffering.

## Things that go wrong

| Symptom | Cause |
|---|---|
| pairing approval `403`s from the LAN | proxy not sending `X-Forwarded-For`, or `--behind-proxy` not set |
| **anyone** can approve a pairing | `X-Forwarded-For` appended rather than overwritten, or a `trusted_proxies` that includes the internet |
| live updates arrive in bursts, or never | buffering on the stream route |
| the stream dies after a minute | `proxy_read_timeout` too low — the response is meant never to end |
| the panel loads but is unstyled, console full of CSP errors | security headers set in **both** places and intersected |
| a redirect loop | `X-Forwarded-Proto` missing, so the server thinks it is on plain HTTP |
| `426 Upgrade Required` | unrelated to the proxy — see [Signed auth (v2) migration](Signed-auth-v2-migration) |

## A note on compression

The server compresses responses with **gzip** — no brotli. Every client in play
sends `gzip`, and brotli was several hundred kilobytes of binary for a few
percent on documents this size.

It **excludes the event stream** from compression itself. If your proxy
compresses instead, exclude that path there too: a compressor with a buffer is
a buffer.
