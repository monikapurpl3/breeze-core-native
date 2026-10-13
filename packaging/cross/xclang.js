// clang + lld against a downloaded sysroot, for targets zig ships no libc for
// in a usable form.
//
//   node xclang.js <clang-target> <sysroot> [extra flags...] -- <arguments...>
//
// Used for OpenBSD: zig can link an OpenBSD binary, but from generic stubs,
// so the result asks for an unversioned `libc.so` and carries no IBT marking.
// Linking against the release's real libraries (see openbsd-sysroot.sh) gives
// exactly what OpenBSD's own toolchain gives. cc-rs's `--target=<rust triple>`
// is dropped; the clang target given first is the one that counts.
'use strict';
const { spawnSync } = require('child_process');

const argv = process.argv.slice(2);
const sep = argv.indexOf('--');
if (sep < 2) {
  console.error('usage: node xclang.js <clang-target> <sysroot> [extra...] -- <args...>');
  process.exit(2);
}
const [target, sysroot, ...extra] = argv.slice(0, sep);
const args = argv.slice(sep + 1).filter((a) => !a.startsWith('--target='));
const r = spawnSync(
  'clang',
  [`--target=${target}`, `--sysroot=${sysroot}`, '-fuse-ld=lld', ...extra, ...args],
  { stdio: 'inherit' },
);
if (r.error) console.error(`xclang.js: ${r.error.message}`);
process.exit(r.status ?? 1);
