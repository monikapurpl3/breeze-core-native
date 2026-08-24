# site/ — the aspic project host

`aspic.salataputarica.hr.eu.org` is where **this** project publishes: the index
page in this directory today, and the signed package repositories that follow at
4.0.0. It is a plain static host — nginx serving files, nothing else.

It exists because the Python project's host, `bolero`, now holds a **sunset**
line. Breeze Core 3.2.0 is where the Python server stops; everything published
for it stays exactly where it is and keeps installing, and new work goes here
instead. Aspic is also meant to carry more than one project, which is why the
layout below is per-project rather than one shared repository tree.

## What is where

| | |
|---|---|
| `index.html`, `aspic.css`, `favicon.svg` | the site. Published; see `WEB_FILES` in `publish.sh` |
| `aspic.conf` | the nginx vhost. **This copy is the source of truth**; the live file is a copy |
| `install-host.sh` | one-time (idempotent) host setup: web root, SELinux label, certificate, vhost, renewal, scanner jail |
| `publish.sh` | push the site as a timestamped release and swap `current` |

## Publishing

```bash
./site/publish.sh          # ~10 KB, a couple of seconds
```

It stages an **allow-list** of files (not everything in this directory — the
vhost and these scripts live here too), refuses to publish unrendered
placeholders, a page linking to a path that is not being published, or anything
that looks like a private key, then uploads to `releases/<utc-timestamp>/` and
atomically swaps the `current` symlink. Three releases are kept.

Rolling back is one command, printed at the top of `publish.sh`.

## Host setup

```bash
scp -r site/ mrrp:~/aspic-site/
ssh mrrp 'sudo bash ~/aspic-site/install-host.sh'
```

Re-run it after editing `aspic.conf` — it reinstalls the vhost, tests it and
reloads. Everything else it does is skipped when already done.

Four things in it are less obvious than they look:

- **The SELinux label.** `/var/www` on this host is `var_t`, so a directory
  created under it inherits `var_t` and nginx is denied every read beneath it —
  a 403 with nothing in the error log to explain it. The script pins
  `httpd_sys_content_t` the way `/var/www/bac-calculator` already is.
- **Bootstrap, then real vhost.** The real vhost names a certificate, so nginx
  will not validate its own config until that certificate exists — and the
  certificate cannot be issued without a server block to answer the challenge
  in. So a temporary HTTP-only vhost goes in first and is replaced.
- **`certonly --nginx`, plus a deploy hook.** certbot authenticates through
  nginx but must not *edit* the vhost, which is deployed from this repo. The
  reload after a renewal therefore comes from
  `/etc/letsencrypt/renewal-hooks/deploy/reload-nginx.sh`, which runs for every
  certificate on the host — including the older ones that have no installer
  recorded to do it for them.
- **The renewal cron.** This host renews from root's crontab, and it pointed at
  `/usr/bin/certbot` after certbot had moved to a pip virtualenv at
  `/usr/local/bin/certbot`. Nothing complains about that: renewals just stop
  until a certificate expires. The script repairs a stale path and then proves
  the whole thing works with `certbot renew --dry-run`.

## The vhost, briefly

Static files, `GET`/`HEAD` only, every other method 405. TLS 1.2/1.3 with an
ECDSA certificate, HSTS, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`,
COOP/CORP, a `Permissions-Policy` that turns off every feature, dotfiles denied,
`releases/` unreachable except through `current`, per-IP request and connection
limits, and its own access log — which is wired into the `nginx-scan` fail2ban
jail, because that jail reads a fixed list of logs and every vhost here with a
dedicated log had fallen out of its view.

Two of its details are worth knowing before editing:

- **The CSP is a `map`, not a header per location.** In nginx an `add_header`
  inside a `location` discards *every* header inherited from the server block,
  so relaxing one directive in one place would silently drop HSTS and the rest
  with it. The map lets the shared error pages — each of which carries an inline
  `<style>` holding its status colour — have `'unsafe-inline'` for style while
  the site itself keeps `default-src 'none'; style-src 'self'`. That is also why
  the stylesheet here is an external file rather than inline: it buys the
  strict policy.
- **The `Cache-Control` map already has rules for repository paths.** They match
  nothing yet. They are written now because a cached index is how a published
  release becomes invisible to `apt update`, and that failure reads as a broken
  publish rather than as a caching bug.

## Adding a project

One section in `index.html`, one pill in the nav, one colour in `aspic.css`, and
a subtree under the web root named after the project (`/<project>/deb/…`).

Per-project subtrees rather than one shared `/deb/`: a single tree would be
kinder to somebody installing two of these projects — one repository entry
instead of two — but it forces every project onto one signing key, one metadata
rebuild and one release cadence, and it means a broken publish of one project
breaks `apt update` for all of them. Independent trees cost the second-project
user one extra `sources.list` line.
