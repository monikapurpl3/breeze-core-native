# Reverse proxy and TLS

Breeze Core speaks **plain HTTP and has no TLS server**. That is deliberate:
terminating TLS well means certificate issuance, renewal, OCSP, cipher policy
and a decade of protocol history, all of which nginx, Apache and Caddy already
do better than a 2 MB appliance daemon ever would.

So if you want HTTPS, something goes in front. This page is the configuration.

> Before you put this on the internet, read
> [Exposing it safely](Exposing-it-safely). **A VPN is a better answer than
> everything here**, and this page assumes you have already decided against it.

## Or let `breeze-core proxy` do it

New in 4.3.0, on Linux. It sets up any of the three servers below, asking before
each step and saying what it is about to do:

```sh
sudo breeze-core proxy                                # asks everything
sudo breeze-core proxy --server nginx --domain breeze.example.org
breeze-core proxy --dry-run                           # the whole plan, changing nothing
sudo breeze-core proxy --undo                         # take it all back out
```

1. **The web server.** It shows which of nginx, Apache and Caddy are installed
   and what is already on ports 80 and 443. If none is installed, it offers to
   install one with the system's package manager.
2. **The name.** It explains the DNS record you need at your DNS host: a CNAME
   to a name that already points home (a dynamic-DNS name, say), or an A record
   with your public address. It also explains the router: ports 80 and 443
   forwarded to this machine. Then it checks what the name resolves to. To
   compare that with your public address it asks `api.ipify.org`, and only if
   you say yes. If the record is not there yet, you can check again, carry on,
   or stop.
3. **The configuration.** It writes the configuration on this page, sets for
   your name and port, and shows it to you first. Then it validates it
   (`nginx -t`, `apachectl configtest`, `caddy validate`) and reloads the
   server. A configuration that does not validate is removed again rather than
   left behind.
4. **The certificate.** For nginx and Apache it runs `certbot` interactively, so
   Let's Encrypt asks for your email and the terms itself; certbot is offered
   if it is missing. Caddy gets its certificate on its own.
5. **Breeze Core behind it.** It sets `BREEZE_HOST=127.0.0.1` and
   `--behind-proxy` in `breeze-core.env`, shows the change, and restarts the
   service. Then it checks the name through the proxy. If you choose to keep
   the direct LAN address as well, it warns you never to forward port 8420
   on the router. With `--behind-proxy` on, anyone who reaches that port
   directly can claim to be on your network.

**Every file it creates or replaces, and every command it runs, is recorded in
`/etc/breeze-core/proxy-undo.json`.** A file it replaces is backed up first,
beside it, as `<file>.before-breeze-proxy`. `--undo` reads that record and puts
everything back as it was:

- the original files restored, and the new ones removed, including certbot's
  separate HTTPS site on Apache;
- the sites disabled and the web server reloaded;
- Breeze Core back on its old address.

Two things stay. Packages it installed stay installed. A certificate stays in
`/etc/letsencrypt`, and the undo prints the `certbot delete` line for it.

It is tested for real against all three servers on Debian
(`packaging/test/verify-proxy.sh`). The test includes a client forging
`X-Forwarded-For` from another machine, and an undo after certbot's Apache
file. A container cannot get a real certificate, so certbot is a stand-in
there. The Apache part supports the Debian
(`/etc/apache2`) and Fedora (`/etc/httpd/conf.d`) layouts, and nginx's
`conf.d` and Alpine's `http.d`.

On **Windows** the same command opens the installer's Caddy wizard (also in
the Start menu: *Breeze Core > Set up Caddy reverse proxy*). On the BSDs and
OPNsense it points to this page. The configurations below are what it writes,
so you can also do it by hand.

## The four things a proxy in front of this must get right

Any proxy, in any configuration:

1. **`X-Forwarded-For`, overwritten — not appended.** This is the one the
   server reads, and the only one. `X-Real-IP` is ignored entirely.
2. **`AC_BEHIND_PROXY=1` on the server**, or that header is not trusted and the
   LAN-only admin check decides on the proxy's own address. With it on, only
   the proxy may be able to reach the server's port. Bind it to `127.0.0.1`,
   or anyone who reaches the port directly can write the header themselves.
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

    # X-Forwarded-For OVERWRITTEN with the real peer. mod_proxy's own
    # X-Forwarded-For appends to whatever the client sent, so it is switched
    # off and the header set outright: one entry, the real one.
    ProxyAddHeaders Off
    RequestHeader set X-Forwarded-For   "expr=%{REMOTE_ADDR}"
    RequestHeader set X-Forwarded-Proto "expr=%{REQUEST_SCHEME}"

    <Location ~ "^/api/auth/(enroll/approve|devices)">
        Require ip 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 127.0.0.1 ::1
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

Apache needs `mod_proxy`, `mod_proxy_http`, `mod_headers` and `mod_ssl`
enabled, and is 2.4.10 or later for the `expr=` form.

> **If you set Apache up from this page before October 2026, change it.** The
> earlier version used `RemoteIPHeader X-Forwarded-For` and left mod_proxy's
> header on, which appends. A request sent with `X-Forwarded-For: 192.168.1.5`
> then reached the server as `192.168.1.5, <real address>`. Servers before 4.3.0
> read the left-most entry, so that request passed the LAN-only check from
> anywhere, and could approve its own pairing. **4.3.0 reads the right-most
> entry**, the one the proxy itself added, which no client can write. So 4.3.0
> is safe even behind the old configuration, but change it anyway. Tested: the
> configuration above forwards only the real address. nginx with
> `$remote_addr` and Caddy without `trusted_proxies` were never affected. To
> check yours, send a request with a made-up `X-Forwarded-For` and look at
> `client_ip` in `/api/system`, as below.

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

From 4.3.0 the access log shows the same thing on every line. Behind a proxy it
names the client the server believed, then the proxy:

```
203.0.113.9 (via 127.0.0.1) GET /api/health -> 200 (0.4ms)
```

Send one request with a made-up `X-Forwarded-For: 192.168.1.5` from outside.
The line should still show your real address. If it shows `192.168.1.5`, the
proxy is passing the client's header through.

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
| **anyone** can approve a pairing | the client's `X-Forwarded-For` passed through untouched; or `--behind-proxy` on while port 8420 is reachable directly, not only through the proxy; or a `trusted_proxies` that includes the internet. Before 4.3.0, also an *appended* header |
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
