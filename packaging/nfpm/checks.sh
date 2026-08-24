# The checks every distro runs once the package is installed. Sourced into each
# verification container by verify-packages.sh, which substitutes @VERSION@.
#
# A FILE, not a shell variable in the harness. It started life as a
# single-quoted string in verify-packages.sh, which meant the single quotes in
# `printf '{"api_key":...}'` closed that string and bash then stripped the JSON's
# double quotes — the container wrote {api_key:x} and the server rejected it, so
# five packages "failed verification" for a bug that was entirely in the test.
# Nested shell quoting is not worth being clever about.
set -eu

echo "-- the binary runs"
breeze-core --version | grep -q "breeze-core @VERSION@"

echo "-- and it is static: no interpreter, nothing to satisfy"
# Every libc words this differently: glibc says "not a dynamic executable",
# musl says "Not a valid dynamic program".
if ldd /usr/bin/breeze-core 2>&1 | grep -qiE "not a dynamic|not a valid dynamic|statically linked"; then
  :
else
  echo "this binary appears to be dynamically linked:"; ldd /usr/bin/breeze-core; exit 1
fi

echo "-- the service account exists"
getent passwd breeze >/dev/null

echo "-- the state directory is 0750 breeze:breeze"
test "$(stat -c %a /etc/breeze-core)" = 750
test "$(stat -c %U:%G /etc/breeze-core)" = "breeze:breeze"

echo "-- the env file is there, root-owned and not world-readable"
test "$(stat -c %a /etc/breeze-core/breeze-core.env)" = 640
# root:breeze, not breeze:breeze — the daemon reads this file and an admin
# writes it. Checked because the archlinux packager drops declared ownership.
test "$(stat -c %U:%G /etc/breeze-core/breeze-core.env)" = "root:breeze"

echo "-- the path the Python package used still resolves"
test -x /usr/lib/breeze-core/breeze-core

echo "-- pair is device pairing, not an alias for login"
breeze-core --help | grep -q "pair \[--ip"

echo "-- and every verb the reference documents is present"
for v in serve pair diag units approve devices revoke login; do
  breeze-core --help | grep -q "breeze-core $v" || { echo "missing verb: $v"; exit 1; }
done

# Only meaningful on a fresh install. In the upgrade cases a config.json is
# already there, so the server starts and keeps running — and the `grep -q`
# waiting for a refusal never returns. That hung two containers for half an
# hour, which is also why there is now a timeout around every case.
if [ -f /etc/breeze-core/config.json ]; then
  echo "-- (a config is already here, so the fresh-install check does not apply)"
else
  echo "-- a fresh install refuses to serve, and says what to run"
  timeout 20 breeze-core serve --host 127.0.0.1 --port 18421 2>&1 | grep -q "breeze-core pair"
fi

echo "-- with a config it starts, answers, and serves the panel"
# Written by hand rather than by `pair`: pairing needs air conditioners and a
# container has none. This is the minimum a paired deployment has.
#
# Only if there is not one already: the upgrade cases put a marker in this file
# before upgrading and then check it survived, and an earlier version of this
# overwrote it — so the upgrade assertion failed on a config that had in fact
# been preserved perfectly.
if [ ! -f /etc/breeze-core/config.json ]; then
  printf %s "{\"api_key\":\"verification-only\",\"units\":[]}" > /etc/breeze-core/config.json
fi
breeze-core serve --host 127.0.0.1 --port 18420 >/tmp/serve.log 2>&1 &
pid=$!
ok=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 0.4
  if wget -qO- http://127.0.0.1:18420/api/health 2>/dev/null | grep -q "status" ; then
    ok=1; break
  fi
done
panel=0
if wget -qO- http://127.0.0.1:18420/ 2>/dev/null | grep -qi "<html"; then
  panel=1
fi
kill $pid 2>/dev/null || true
test "$ok" = 1 || { echo "the server never answered:"; cat /tmp/serve.log; exit 1; }
# The panel is compiled into the binary, so a package that installs a single
# file should still serve a web interface. Nothing else here would notice if the
# embedding broke.
test "$panel" = 1 || { echo "the panel did not come back as HTML"; exit 1; }

echo "ALL CHECKS PASSED"
