# packaging/macos/ — cross-building for macOS

`build-binaries.sh macos-arm64 macos-x86_64` builds both Mac binaries on the
workstation (or in CI on Linux), with `cargo zigbuild`, like every Linux target.
No Mac and no Apple SDK are involved.

## The one thing zig does not have: CoreFoundation

zig carries macOS's C library (`libSystem`, `libiconv`) but no frameworks. Only
one dependency needs one: `chrono`'s local time zone goes through
`iana-time-zone`, which asks CoreFoundation for the system time zone. That is
seven functions, so linking stops at `unable to find framework 'CoreFoundation'`
and nowhere else.

`CoreFoundation.framework/CoreFoundation.tbd` is a **link stub written here**,
not a copy of anything from Apple's SDK. It only says that the framework at its
real install name, `/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation`,
exports those seven symbols, so the linker can record the dependency. At run
time macOS loads its own CoreFoundation, which is on every Mac.

If `iana-time-zone` ever uses another function, the link fails, naming the
missing symbol. Add it to the list. The symbols it uses today:

```sh
grep -ohE '\bCF[A-Za-z]+\b' ~/.cargo/registry/src/*/iana-time-zone-*/src/tz_darwin.rs | sort -u
```

## What comes out

- **`macos-arm64`** (Apple silicon) and **`macos-x86_64`** (Intel), each about
  2–2.5 MB, needing **macOS 13 or newer**, which is zig's floor.
- **Apple silicon will not run an unsigned binary at all.** zig's linker gives
  the arm64 one an *ad-hoc* signature, which is enough to run it. That is not a
  Developer ID: a copy downloaded with a browser is quarantined, and Gatekeeper
  refuses it until the quarantine is cleared
  (`xattr -d com.apple.quarantine breeze-core`). A copy fetched with `curl` is
  not quarantined.
- They load only what every Mac has: `libSystem`, `libiconv`, `libcharset` and
  CoreFoundation. Check with
  `llvm-objdump --macho --dylibs-used packaging/out/bin/macos-arm64/breeze-core`.

`.github/workflows/macos.yml` cross-builds them the same way on Linux, then runs
them on real Apple-silicon and Intel Macs.
