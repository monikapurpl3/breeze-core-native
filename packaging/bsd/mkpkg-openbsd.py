#!/usr/bin/env python3
"""Build the OpenBSD package without OpenBSD.

    mkpkg-openbsd.py --repo DIR --bin FILE --version VER --out PKG.tgz [--arch amd64]

Writes an UNSIGNED package; signify -zS signs it afterwards (build-bsd.sh does
both, in the bc-bsdpkg container). Until 4.3.2 this was pkg_create(1) on an
OpenBSD VM, and pkg_create is Perl that only runs on OpenBSD. An OpenBSD
package is simple enough to write directly, and this one is checked against
what pkg_create made for 4.3.2:

  * a ustar archive, gzipped, whose first two members are +CONTENTS (the
    packing list) and +DESC, then the files in packing-list order;
  * every file listed with @sha (base64 SHA-256), @size and @ts, the last being
    its timestamp: the archive member itself carries mtime 0, and pkg_add sets
    the installed file's time from @ts;
  * +CONTENTS and +DESC are 0444 root:wheel, the files root:bin;
  * the signature is signify's gzip mode, in the gzip header's comment field,
    and names its key -- which is why the key must be called aspic-pkg.sec:
    pkg_add opens /etc/signify/aspic-pkg.pub because the signature says so.

The packing list does the work an install script would do badly: @newgroup and
@newuser declare the service account, so pkg_delete removes it again, and the
configuration is @sample, so pkg_add seeds it once and never touches an edited
copy -- what you want for a file holding an API key and a unit's V3
credentials. 799 is an unused id in the range OpenBSD leaves for local daemons.

FULLPKGPATH (the @comment pkgpath=) is not decoration: without it pkg_add
prints "Use of uninitialized value in hash element at PkgAdd.pm line 304" on
every install, because its security-quirks check keys a hash on it.
"""
import argparse
import base64
import gzip
import hashlib
import io
import os
import tarfile
import time

COMMENT = "LAN-first REST API and web panel for Midea air conditioners"
DESC = """\
Self-hosted, LAN-first control for Midea air conditioners: a REST API, a web
control panel, device-pairing authentication, server-side schedules and
temperature curves, and a diagnostic CLI.

One static executable with the panel compiled into it - no interpreter, no
runtime dependencies. Configuration lives in /etc/breeze-core; enable the
service with `rcctl enable breeze_core`.
"""
MAINTAINER = "monikapurpl3 <monikapurpl3@users.noreply.github.com>"


def sha(data: bytes) -> str:
    return base64.b64encode(hashlib.sha256(data).digest()).decode()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, help="source tree (rc script, env, LICENSE)")
    ap.add_argument("--bin", required=True, help="the openbsd breeze-core binary")
    ap.add_argument("--version", required=True)
    ap.add_argument("--arch", default="amd64")
    ap.add_argument("--out", required=True, help="unsigned package to write")
    ap.add_argument("--ts", type=int,
                    default=int(os.environ.get("SOURCE_DATE_EPOCH", time.time())))
    a = ap.parse_args()

    def read(rel: str) -> bytes:
        with open(os.path.join(a.repo, rel), "rb") as f:
            return f.read()

    with open(a.bin, "rb") as f:
        binary = f.read()
    # (name in the archive, contents, mode). The rc script is absolute: it is
    # outside the /usr/local prefix the other entries are relative to.
    rc = ("/etc/rc.d/breeze_core", read("packaging/bsd/rc.openbsd"), 0o755)
    files = [
        ("bin/breeze-core", binary, 0o755),
        ("share/doc/breeze-core/LICENSE", read("LICENSE"), 0o644),
        ("share/examples/breeze-core/breeze-core.env",
         read("packaging/nfpm/breeze-core.env"), 0o644),
    ]
    for name, data, _ in [rc, *files]:
        if b"\r\n" in data and not name.startswith("bin/"):
            raise SystemExit(f"{name} has CRLF line endings; see packaging/check-eol.sh")

    desc = f"{COMMENT}\n{DESC}\nMaintainer: {MAINTAINER}\n".encode()

    def entry(data: bytes) -> list[str]:
        return [f"@sha {sha(data)}", f"@size {len(data)}", f"@ts {a.ts}"]

    plist = [
        f"@name breeze-core-{a.version}",
        "@comment pkgpath=comms/breeze-core ftp=no",
        f"@arch {a.arch}",
        "+DESC",
        f"@sha {sha(desc)}",
        f"@size {len(desc)}",
        "@newgroup breeze:799",
        "@newuser breeze:799:799:daemon:Breeze Core:/nonexistent:/sbin/nologin",
        "@cwd /usr/local",
        f"@rcscript {rc[0]}",
        *entry(rc[1]),
    ]
    for name, data, _ in files:
        plist += [name, *entry(data)]
    plist += [
        "@owner breeze", "@group breeze", "@mode 750",
        "@sample /etc/breeze-core/",
        "@mode 640",
        "@sample /etc/breeze-core/breeze-core.env",
        "@owner", "@group", "@mode",
    ]
    contents = ("\n".join(plist) + "\n").encode()

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        def add(name: str, data: bytes, mode: int, meta: bool) -> None:
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = mode
            ti.uid, ti.uname = 0, "root"
            ti.gid, ti.gname = (0, "wheel") if meta else (7, "bin")
            ti.mtime = a.ts if meta else 0
            tar.addfile(ti, io.BytesIO(data))

        add("+CONTENTS", contents, 0o444, True)
        add("+DESC", desc, 0o444, True)
        for name, data, mode in [rc, *files]:
            add(name, data, mode, False)

    with open(a.out, "wb") as f:
        f.write(gzip.compress(buf.getvalue(), compresslevel=9, mtime=0))
    print(f"built (unsigned): {a.out}")


if __name__ == "__main__":
    main()
