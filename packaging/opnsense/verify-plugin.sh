#!/usr/bin/env bash
# Check the os-breeze-core plugin as far as it can honestly be checked without a
# real OPNsense firewall to install it on.
#
#   ./packaging/opnsense/verify-plugin.sh [freebsd-builder-host]
#
# WHAT THIS DOES NOT DO: exercise the GUI. Nothing here loads the Volt template,
# renders the form, saves a setting through the model, or watches configd call
# the rc script. Those need OPNsense itself. The PHP is syntax-checked, the XML
# is checked for well-formedness, the package is checked for the right ABI and
# contents, and the binary is checked to run on a FreeBSD 14 userland - which is
# the set of things that can fail silently and be caught from here.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
HOST="${1:-192.168.122.131}"
USER_AT="${BSD_USER:-monika}@$HOST"
ROOT="${FB14_ROOT:-/jail/fb14}"
VER="$(grep -m1 '^version' crates/breeze-core/Cargo.toml | cut -d'"' -f2)"
FILES=packaging/opnsense/files
PKG="packaging/out/opnsense/os-breeze-core-$VER.pkg"

MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1

pass=0; fail=0
ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; fail=$((fail+1)); }
head_() { printf '\n== %s\n' "$1"; }

head_ "PHP syntax"
# In a container rather than on the builder: neither the FreeBSD host nor the
# FreeBSD 14 root has php, and installing one on a VM to lint four files is a
# poor trade when the linter is one docker run away.
if php_out=$(docker run --rm -v "$MOUNT/$FILES:/f:ro" php:8.3-cli \
      sh -c 'set -e; for f in $(find /f -name "*.php"); do php -l "$f"; done' 2>&1); then
  echo "$php_out" | sed 's/^/    /'
  ok "$(printf '%s' "$php_out" | grep -c 'No syntax errors') PHP file(s) parse"
else
  echo "$php_out" | sed 's/^/    /'
  bad "PHP syntax errors"
fi

head_ "XML well-formedness"
if xml_out=$(docker run --rm -v "$MOUNT/$FILES:/f:ro" php:8.3-cli \
      php -r 'foreach (new RecursiveIteratorIterator(new RecursiveDirectoryIterator("/f")) as $p) {
                if (substr($p, -4) !== ".xml") continue;
                $d = new DOMDocument();
                if (!@$d->load((string)$p)) { fwrite(STDERR, "BAD: $p\n"); exit(1); }
                echo "well-formed: ", basename((string)$p), "\n";
              }' 2>&1); then
  echo "$xml_out" | sed 's/^/    /'
  ok "$(printf '%s' "$xml_out" | grep -c well-formed) XML file(s) parse"
else
  echo "$xml_out" | sed 's/^/    /'
  bad "malformed XML"
fi

head_ "shell scripts"
for s in "$FILES/usr/local/etc/rc.d/breeze_core" \
         "$FILES/usr/local/lib/breeze-core/serve.sh" \
         "$FILES/usr/local/opnsense/scripts/OPNsense/BreezeCore/setup.sh"; do
  if sh -n "$s" 2>/dev/null; then ok "$(basename "$s") parses"; else bad "$(basename "$s")"; fi
done

head_ "configd wiring"
# reconfigureAction() calls stop, template reload, then start or reload, so all
# of those actions have to exist or saving a setting fails at the last step.
acts="$FILES/usr/local/opnsense/service/conf/actions.d/actions_breezecore.conf"
for a in start stop restart reload status configure; do
  if grep -q "^\[$a\]" "$acts"; then ok "action [$a]"; else bad "missing action [$a]"; fi
done
# The template's target path is the one load_rc_config() will source, and
# nothing else works if it is 'tidier'.
if grep -q '^breeze-core.conf:/usr/local/etc/rc.conf.d/breeze_core$' \
     "$FILES/usr/local/opnsense/service/templates/OPNsense/BreezeCore/+TARGETS"; then
  ok "the template renders to /usr/local/etc/rc.conf.d/breeze_core"
else
  bad "+TARGETS does not render where load_rc_config() reads"
fi

head_ "the package"
if [ -f "$PKG" ]; then
  ok "$(basename "$PKG") exists ($(( $(wc -c < "$PKG") / 1024 )) KB)"
else
  bad "no package - run packaging/opnsense/build-plugin.sh"
fi

# ABI and contents, asked of the builder because pkg lives there.
remote=$(ssh -o BatchMode=yes "$USER_AT" "
  set -eu
  P=\$(ls -1 $ROOT/tmp/pkgout/os-breeze-core-*.pkg 2>/dev/null | tail -1)
  [ -n \"\$P\" ] || { echo 'NOPKG'; exit 0; }
  echo \"ABI=\$(pkg query -F \"\$P\" %q)\"
  echo \"DEPS=\$(pkg query -F \"\$P\" '%dn' | tr '\n' ' ')\"
  echo \"FILES=\$(pkg query -F \"\$P\" '%Fp' | wc -l | tr -d ' ')\"
  pkg query -F \"\$P\" '%Fp' | grep -c 'mvc/app' | sed 's/^/MVC=/'
  # The point of the whole exercise: this binary has to run on FreeBSD 14.
  doas chroot $ROOT /usr/bin/env sh -c 'ls /tmp/stage/usr/local/bin/breeze-core >/dev/null' 2>/dev/null \
    && doas chroot $ROOT /tmp/stage/usr/local/bin/breeze-core --version | head -1 | sed 's/^/RUNS=/' \
    || echo 'RUNS=no'
" 2>/dev/null || echo "")

abi=$(printf '%s' "$remote" | sed -n 's/^ABI=//p')
deps=$(printf '%s' "$remote" | sed -n 's/^DEPS=//p' | tr -d ' ')
nfiles=$(printf '%s' "$remote" | sed -n 's/^FILES=//p')
nmvc=$(printf '%s' "$remote" | sed -n 's/^MVC=//p')
runs=$(printf '%s' "$remote" | sed -n 's/^RUNS=//p')

[ "$abi" = "FreeBSD:14:amd64" ] && ok "built for $abi, not the builder's 15" || bad "ABI is '$abi', expected FreeBSD:14:amd64"
[ -z "$deps" ] && ok "no dependencies (the Python plugin needed python311)" || bad "it depends on: $deps"
[ "${nfiles:-0}" -ge 15 ] && ok "$nfiles files in the package" || bad "only ${nfiles:-0} files - plist problem"
[ "${nmvc:-0}" -ge 6 ] && ok "$nmvc MVC files (controllers, model, views)" || bad "only ${nmvc:-0} MVC files"
case "$runs" in
  breeze-core*) ok "the binary runs on a FreeBSD 14 userland: $runs" ;;
  *) bad "the binary did not run in the FreeBSD 14 root" ;;
esac

head_ "$pass passed, $fail failed"
cat <<'CAVEAT'

  Still unverified, and it cannot be verified from here: the GUI itself.
  Nothing above renders the Volt page, saves a setting through the model, or
  watches configd drive the rc script. That needs a real OPNsense box.

CAVEAT
[ "$fail" = 0 ] || exit 1
