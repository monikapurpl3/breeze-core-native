# Building and releasing

How a release is actually made. Useful if you are contributing, packaging for
somewhere new, or just want to know what the published artifacts went through.

For building it to *use*, see
[Installing from source](Installing-from-source).

## The shape of it

Everything runs from one workstation, in containers, with no CI:

```
cargo test --workspace                       480 tests, 18 suites
packaging/build-binaries.sh                  one static executable per target
packaging/nfpm/build-packages.sh             deb, rpm, pacman, apk, ipk
packaging/xbps/build-xbps.sh                 Void
packaging/portage/build-overlay.sh           Gentoo
packaging/container/build-images.sh          the two images
packaging/bsd/build-{free,net,open}bsd.sh    on real BSD machines
packaging/opnsense/build-plugin.sh           in a FreeBSD 14 root
packaging/windows/build-installer.ps1        NSIS
packaging/repo/build-repo.sh                 the signed repository tree
packaging/repo/verify-repo.sh                install it all, in containers
site/publish.sh --tree packaging/out/aspic   publish
```

The BSD scripts drive real machines over ssh, because Zig bundles no BSD libc
and there is no honest way to cross-build for them. Everything else is
containers on the one host.

## Order matters, and the scripts enforce it

Every packager **refuses to wrap a binary older than the source.** That guard
exists because it happened: a `.deb` went into a verification container
carrying a CLI three commits old, and the only reason it was caught is that a
test asserted on the help text. Cross-built binaries are not rebuilt by `cargo
build`, so editing the source and running only the packaging step silently
packages the previous build under the new version number.

```
!! the amd64 binary is older than the source (crates/breeze-http/src/state.rs)
   run: ./packaging/build-binaries.sh amd64
```

The repository builder has a second guard: it **refuses to sign a tree
assembled from two versions at once.** `build-packages.sh` deliberately does
not clear its output directory — it takes an architecture list, so clearing
would delete the architectures a partial run is not building — and every copy
in the repository builder globs by extension rather than by version. Those two
combine to publish both versions silently, with a package count that disagrees
with the release page and nothing looking wrong until somebody counts.

## Versions and releases

Two numbers, and the second one matters more than it looks.

The **version** lives in exactly one place, `crates/breeze-core/Cargo.toml`,
and every script reads it from there. Nothing else needs editing.

The **package release** — the trailing `-N` in `breeze-core-4.0.2-2` — is for
when the packaged bytes change and the version does not. Without it, a rebuilt
4.0.2 is *identical* to a package manager, so no existing install ever upgrades
to the fix:

```sh
BC_RELEASE=2          ./packaging/nfpm/build-packages.sh
BC_XBPS_REVISION=2    ./packaging/xbps/build-xbps.sh
BC_PORTAGE_REVISION=1 ./packaging/portage/build-overlay.sh   # Gentoo -r1
```

The Windows installer, the BSD packages and the OPNsense plugin have no release
field in this setup: re-downloading gets the new one.

## The four signing keys

They live in `packaging/repo/keys/`, are git-ignored, are generated on first
run, and **must be backed up.** There are four because no two package managers
agree:

| Key | For | Why its own |
|---|---|---|
| GPG RSA-4096 | apt, dnf, pacman | **RSA, not ed25519** — rpm 4.14 (RHEL/Alma/Rocky 8) cannot *import* an ed25519 public key at all, and every signature then reads as NOKEY. rpm 4.16 is fine, which is what makes it a trap: the machine you test on works |
| RSA-4096 | apk | apk-tools has no ed25519 option |
| usign ed25519 | opkg | OpenWrt's own signer; GPG is useless to it |
| RSA-4096 | xbps | all `xbps-rindex` signs with |

FreeBSD and OpenBSD are signed **on their own machines** — `pkg repo` and
`signify` respectively — and those keys are shredded (`rm -P`) after use rather
than left on a VM.

`packaging/repo/portage-git/` needs backing up for the same reason as the keys,
for a different one: it holds the Gentoo overlay's git history. Regenerating it
each release would give every commit a new hash, and `emerge --sync` would then
fail with unrelated histories for everyone who had already added the overlay.

## Verification is the point

`verify-repo.sh` installs from the built tree in clean containers of each
distribution — Debian 12, AlmaLinux 9, AlmaLinux 8, Arch, Alpine, Void and
Gentoo — served over HTTP by an nginx container, because that is the only way
to exercise the URL layout and the index fetch as a real client does.

**Both directions, every time.** Every one of those clients will happily
install from an unsigned source if asked the wrong way, so each case first
proves the client *refuses* the repository without the key, then proves it
accepts it with the key. Only the pair means anything.

`--live` runs the same checks against the published URL, which is the only
thing that exercises DNS, TLS, the vhost's headers and the tree together.

Alongside it:

- `packaging/nfpm/verify-packages.sh` — every package installed and started in
  a container of its distribution;
- `packaging/xbps/verify-xbps.sh` — 38 checks in a Void container, ending in a
  live HTTP request to a server running as the service account;
- `packaging/portage/verify-portage.sh` — a real `emerge`, plus the ebuild's
  CHOST guard exercised against seven CHOST values;
- `packaging/bsd/verify-*.sh` — on the real machines;
- `packaging/opnsense/verify-plugin.sh` — 18 checks, and it states plainly
  which parts it cannot check;
- `packaging/test/differential.py` — the interesting one, below.

### The differential harness

Runs 4.x and a live 3.2.0 against the same configuration and diffs every
endpoint: status, JSON type, shape and value. An `EXPECTED` dict documents
every deliberate difference, so anything not in it is a finding. It drove the
contract deviations from 142 to zero unexplained.

This is the test that catches a *misunderstanding* of the contract, which no
amount of unit testing can — a unit test agrees with whatever the code does.

It enrols a credential on each server to do its work, and **revokes it in a
`finally`**. It did not always: an earlier version left one behind per run, and
thirty-two real, working, non-expiring credentials accumulated in a production
`devices.json` before anyone noticed `breeze-core devices` had become pages
long.

## Cutting a release

```sh
# 1. version
$EDITOR crates/breeze-core/Cargo.toml && cargo update -p breeze-core --offline

# 2. everything, in order
cargo test --workspace
./packaging/build-binaries.sh
./packaging/nfpm/build-packages.sh
./packaging/xbps/build-xbps.sh
./packaging/portage/build-overlay.sh
./packaging/container/build-images.sh
# and, on the real machines
./packaging/bsd/build-freebsd.sh && ./packaging/bsd/build-netbsd.sh \
  && ./packaging/bsd/build-openbsd.sh
./packaging/opnsense/build-plugin.sh
powershell -File packaging/windows/build-installer.ps1 -OutDir packaging/out/windows

# 3. the repository, and prove it
./packaging/repo/build-repo.sh
./packaging/repo/verify-repo.sh

# 4. publish
gh release create v4.0.2 --notes-file NOTES.md packaging/out/release/*
./packaging/container/build-images.sh --push
./site/publish.sh --tree packaging/out/aspic
./packaging/repo/verify-repo.sh --live
```

Build everything from a **committed** tree. `--version` embeds the commit and
appends `-dirty` otherwise, and a release artifact reporting a commit that does
not exist is a small lie that costs someone an afternoon later.

`build-installer.ps1` needs `-OutDir packaging/out/windows` or it leaves the
`.exe` next to its own script and the repository builder publishes nothing.

## Publishing

`site/publish.sh` uploads to a timestamped release directory and atomically
swaps the `current` symlink nginx serves, keeping the last three for instant
rollback. It refuses to publish if a page links to a path that is not in the
upload, checks no key material is about to be uploaded, and smoke-tests one
entry point per repository family afterwards.

Rolling back is deliberately not a flag — it is one command on the host:

```sh
ssh host 'cd /var/www/aspic && ln -sfn releases/<ts> current.new && mv -Tf current.new current'
```

## Container images

Two, amd64 and arm64, built from `packaging/out` so the image ships **the same
binary the packages ship** rather than one compiled separately. A container
that is a different build from the `.deb` is a container that can fail
differently.

**No `latest` tag**, deliberately: on a daemon that controls heating, `latest`
is an upgrade nobody asked for at a moment nobody chose.

`build-images.sh --push` ends with an **anonymous** pull check against
`ghcr.io/token` — not `docker manifest inspect`, which Docker Desktop's
credential helper silently authenticates. That unsound test is how thirteen
image versions once went public without anyone noticing.
