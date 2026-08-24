# site/ — the aspic project host

`aspic.salataputarica.hr.eu.org` is where this project publishes: five signed
package repositories, an index page explaining how to add them, and a page per
project. It is a plain static host — nginx serving files, nothing else, and no
key ever lives on it.

It exists because the Python project's host, `bolero`, now holds a line that has
stopped: Breeze Core 3.2.0 is the last of it. Everything published there stays
exactly where it is and keeps installing, and new work goes here instead. Aspic
is meant to carry more than one project, and the layout below is what that
implies — see "Adding a project".

## What is where

| | |
|---|---|
| `index.html`, `aspic.css`, `favicon.svg` | the index: how to add the repository. See `WEB_FILES` in `publish.sh` |
| `breeze-core/index.html` | one page per project, at `/breeze-core/` |
| `aspic.conf` | the nginx vhost. **This copy is the source of truth**; the live file is a copy |
| `install-host.sh` | one-time (idempotent) host setup: web root, SELinux label, certificate, vhost, renewal, scanner jail |
| `publish.sh` | push the site as a timestamped release and swap `current` |

## Publishing

```bash
./site/publish.sh          # the pages only, a couple of seconds
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
- **The `Cache-Control` map has rules for repository paths** — no-cache for every
  index, immutable for the packages themselves. A cached index is how a published
  release becomes invisible to `apt update`, and that failure reads as a broken
  publish rather than as a caching bug.

## Adding a project

**One page, one card, and a package.** The repositories are shared, so a new
project needs no new repository: build its package with the same name it will be
installed by, put it through `packaging/repo/build-repo.sh`, and it appears in
the existing `/deb`, `/rpm/<arch>`, `/arch/<arch>`, `/alpine/<arch>` and
`/openwrt/<arch>` indexes.

Then give it a page: `site/<project>/index.html`, a card in the roster on
`site/index.html`, a colour in `aspic.css`, and its file in `WEB_FILES` in
`publish.sh`.

One repository per package-manager family rather than one per project, because
somebody who has added aspic should get everything on it — a second project
costing a second `sources.list` entry is a worse deal than sharing one signing
key and one metadata rebuild. It also means the index page stays the same size
as the project list grows: it explains how to add the repository, and each
project explains itself.

## Publishing the repositories

`./site/publish.sh` on its own publishes only the pages, and **refuses** if that
would make anything live disappear — a page-only push would otherwise delete
every repository under it, and `apt update` would stop working for everyone. The
whole tree goes up with:

```bash
./packaging/repo/build-repo.sh                  # assemble + sign, pages included
./packaging/repo/verify-repo.sh                 # install from it, in containers
./site/publish.sh --tree packaging/out/aspic
./packaging/repo/verify-repo.sh --live           # ...and again, from the real URL
```
