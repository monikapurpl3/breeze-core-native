// `zig cc` for a target cargo-zigbuild will not drive.
//
//   node zcc.js <zig-triple> <compiler or linker arguments...>
//
// cargo calls this as the linker, and cc-rs calls it as the C compiler, for
// the targets build-binaries.sh routes here (NetBSD, loongarch64). Two things
// it does that a bare `zig cc -target ...` cannot:
//
//   * drops `--target=<rust triple>`, which cc-rs adds for any clang-like
//     compiler and which zig cannot parse ("UnknownOperatingSystem" for
//     x86_64-unknown-netbsd): the zig triple given first is the one that counts;
//   * drops linker flags zig's linker rejects outright. Rust's loongarch64 musl
//     target passes -Wl,--no-rosegment, which is a no-op for a static binary.
//
// Runs the zig named by $ZIG (build-binaries.sh pins it), else zig on PATH.
// A response file (@file) is passed through untouched; rustc writes one only
// for a command line too long for Windows, which no build here produces.
'use strict';
const { spawnSync } = require('child_process');

const DROP = new Set(['-Wl,--no-rosegment']);
const [triple, ...args] = process.argv.slice(2);
if (!triple) {
  console.error('usage: node zcc.js <zig-triple> <args...>');
  process.exit(2);
}
const kept = args.filter((a) => !a.startsWith('--target=') && !DROP.has(a));
const r = spawnSync(process.env.ZIG || 'zig', ['cc', '-target', triple, ...kept], {
  stdio: 'inherit',
});
if (r.error) console.error(`zcc.js: ${r.error.message}`);
process.exit(r.status ?? 1);
