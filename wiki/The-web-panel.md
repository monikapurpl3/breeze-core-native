# The web panel

Open `http://<BREEZE_HOST>:8420` in any modern browser. There is nothing to
install and nothing to build.

The panel is **compiled into the executable** — it is not a directory of files
on disk — so there is no static path to deploy, no working directory to get
wrong, and no way for the panel and the server to be different versions of
themselves.

It is also **just an HTTP client of the API**. Everything it does, you can do
with `curl`; delete it and the API and the Android app carry on unaffected.

## First visit

It asks for the API key, then walks you through pairing — the same handshake
every client uses. See [First run and pairing](First-run-and-pairing).

After that the browser holds its **own credential** and does not ask again.
Each browser is a separate client, revocable on its own.

## A card per unit

Each unit gets a card carrying its name, whether it is online, and the
controls:

| Control | Values |
|---|---|
| power | on / off |
| mode | `AUTO` `COOL` `DRY` `HEAT` `FAN_ONLY` |
| target temperature | 16–30 °C in 0.5° steps |
| fan speed | 20 / 40 / 60 / 80 / 100, plus auto |
| swing | `OFF` `VERTICAL` `HORIZONTAL` `BOTH` |
| eco, turbo | on / off where the unit has them |

**The card reconciles against what the unit says, not what you asked for.**
Every control returns the unit's own state after the change, so a setting the
firmware quietly ignored shows as ignored rather than as applied. That is why a
slider can spring back: the unit disagreed, and the panel is telling you.

**Controls the firmware does not support are hidden**, not shown greyed out.
The server reports each unit's capabilities and the panel asks before drawing.

### The climate bar

The indoor / outdoor / target bar is a port of the Android app's, rule for
rule, so both clients describe the same room the same way. The **warmest** of
the three temperatures sets the top of the scale.

`outdoor_temperature` is frequently `null` — plenty of units have no outdoor
probe — and absence is drawn as absence, not as zero.

## Live updates, without polling

The panel subscribes to the server's event stream, so a change made anywhere —
another browser, a phone, a schedule, a timer — appears on every open panel
within a tick.

Two consequences worth knowing:

- **With nobody watching, the server makes no LAN traffic at all.** The poller
  idles until something subscribes, so the first update after you open the
  panel can take up to one tick (`AC_STREAM_TICK`, 5 s by default).
- **A buffering proxy breaks this**, and it looks exactly like a dead
  connection. See [Reverse proxy and TLS](Reverse-proxy-and-TLS).

## Programs and timers

The panel is a full editor for both.

**Timers** are one-shot: "off in 45 minutes". You ask in *minutes*, never a
time of day, and the countdown comes from the server so a phone in another
timezone with a wrong clock still counts down correctly. See [Timers](Timers).

**Programs** come in three kinds — favourites, schedules and curves — and the
scheduler that runs them lives in the server, so they fire whether or not any
client is open. See
[Programs, schedules and curves](Programs-schedules-and-curves).

## Managing units from the panel

Add, rename and remove units without touching a file or the CLI. "Scan" runs
the same LAN discovery `breeze-core pair` uses and offers whatever answers.

Renaming changes the label only; the unit's id is what everything references,
so a rename breaks no schedule, timer or program.

## The Nerd panel

Everything the server knows about itself, on one screen: OS, init system, CPU,
the four store paths and their modes, every unit and its cached capabilities,
enrolled credentials, scheduler and stream state, and **how the request you
just made reached the server**.

That last part earns its place. If `client_ip` reads `127.0.0.1` when you are
connecting from a phone, your proxy is not forwarding the real address — which
is exactly what silently breaks LAN-only approval, and is invisible everywhere
else.

It is rendered **generically**, walking whatever JSON the server sends rather
than naming fields, so a fact the server learns later shows up here without the
panel being taught about it. The Android app has the same screen behind seven
taps on the version number.

**It never contains a secret** — not the API key, not a credential, not a
unit's V3 token or key. `breeze-core diag` greps the response for
secret-looking values, so the omission is checked rather than trusted. That
makes the Nerd panel safe to screenshot when asking for help.

## Themes

Six palettes — **Teal**, **Indigo**, **Violet**, **Rose**, **Green** and
**Amber** — plus a custom colour picker.

A palette sets one seed colour and the stylesheet derives the accent and the
hue-tinted surfaces from it with `color-mix`, so the whole panel stays
coherent. Picking a preset does not destroy a custom colour you spent a minute
choosing: going back to "custom" restores it.

The choice is **per browser**, in `localStorage`. It is not server state and it
does not follow you to another device.

## Notes on how it is built

Only interesting if you are modifying it:

- **Native ES modules, no bundler and no build step.** `index.html` loads
  `js/app.js` as a module and the modules import each other by relative path.
- **Every request goes through `apiFetch()` in `api.js`**, which injects
  credentials and handles a `401` by clearing them and re-prompting. Nothing
  calls `fetch()` directly — a past rewrite that did shipped a panel which
  `401`'d on every call.
- **Strict CSP: no inline styles or scripts.** `default-src 'self'`, so CSS
  lives in the stylesheet and dynamic styling is applied via `element.style`
  in JS, which the policy allows. A `style="…"` attribute or a `<style>` block
  is blocked, and adding either would mean loosening the policy for everyone.
- **A hand-written Keccak** (`sha3.js`) is in there because auth v2 digests
  bodies with SHA3-512 and WebCrypto implements no SHA-3 at all.
- **The private signing key is non-extractable.** WebCrypto creates it so that
  it can sign and will not hand the bytes back to the page that made it. It
  lives in IndexedDB.
- The file list is **generated** at build time by walking `static/`, so a file
  added to the panel cannot be silently left out of the binary.
