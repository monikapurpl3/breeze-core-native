#!/usr/bin/env bash
# Run `breeze-core proxy` for real, against nginx, Caddy and Apache, in clean
# Debian containers -- answering its questions on stdin as a person would.
#
#   ./packaging/test/verify-proxy.sh                 # nginx caddy apache
#   ./packaging/test/verify-proxy.sh caddy
#
# Needs packaging/out/bin/amd64/breeze-core (./packaging/build-binaries.sh amd64).
#
# For each server it checks that the wizard's configuration validates and
# serves the name; that Breeze Core ends up on 127.0.0.1 with --behind-proxy;
# that a request from ANOTHER container carrying a forged
# `X-Forwarded-For: 192.168.1.5` is logged by the server with its real
# address -- the property everything else rests on -- and that `--undo` puts
# every file back as it was.
#
# Not covered, because a container cannot: a real certificate (certbot is
# declined; Caddy is given `local_certs`, which the wizard must keep), DNS
# (the .test name never resolves, and the wizard is told to continue), and a
# real init system (Breeze Core is restarted by the harness).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
BIN="packaging/out/bin/amd64/breeze-core"
[ -f "$BIN" ] || { echo "no $BIN -- run ./packaging/build-binaries.sh amd64"; exit 1; }
MOUNT="$REPO"
case "$MOUNT" in /[a-z]/*) MOUNT="$(echo "$MOUNT" | sed -E 's#^/([a-z])/#\U\1:/#')" ;; esac
export MSYS_NO_PATHCONV=1
NET=bc-proxy-verify
DOMAIN=breeze.example.test

want=("$@")
[ ${#want[@]} -gt 0 ] || want=(nginx caddy apache)

cleanup() { docker rm -f bc-proxy bc-client >/dev/null 2>&1 || true; docker network rm "$NET" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create "$NET" >/dev/null

pass=0; fail=0
for server in "${want[@]}"; do
  case "$server" in
    nginx)  packages="nginx curl procps";   answers='n c y n y y' ;;
    apache) packages="apache2 curl procps"; answers='n c y n y y' ;;
    caddy)  packages="caddy curl procps";   answers='n c y y y' ;;
    *) echo "unknown server $server"; exit 1 ;;
  esac
  echo
  echo "=== $server"
  docker rm -f bc-proxy >/dev/null 2>&1 || true
  docker run -d --name bc-proxy --network "$NET" -v "$MOUNT:/repo:ro" debian:bookworm-slim sleep infinity >/dev/null

  # --- set the scene: the server installed, Breeze Core running as a service would
  docker exec -e PKGS="$packages" -e SERVER="$server" bc-proxy sh -euc '
    export DEBIAN_FRONTEND=noninteractive
    apt-get -qq update >/dev/null && apt-get -qq install -y $PKGS >/dev/null 2>&1
    install -m 0755 /repo/packaging/out/bin/amd64/breeze-core /usr/bin/breeze-core
    mkdir -p /etc/breeze-core
    sed "s/^BREEZE_HOST=.*/BREEZE_HOST=0.0.0.0/" /repo/packaging/nfpm/breeze-core.env > /etc/breeze-core/breeze-core.env
    cp /etc/breeze-core/breeze-core.env /tmp/env.original
    printf "{\"api_key\": \"%s\", \"units\": []}\n" "$(head -c 16 /dev/urandom | od -An -tx1 | tr -d " \n")" > /etc/breeze-core/config.json
    # A stand-in for the init system: restarts the server, re-reading the env
    # file each time, as systemd would.
    cat > /usr/local/bin/supervise <<'"'"'SUP'"'"'
#!/bin/sh
while true; do
  set -a; . /etc/breeze-core/breeze-core.env; set +a
  breeze-core serve --host "$BREEZE_HOST" --port "$BREEZE_PORT" $BREEZE_OPTS >> /tmp/breeze.log 2>&1
  sleep 1
done
SUP
    chmod +x /usr/local/bin/supervise
    nohup supervise >/dev/null 2>&1 &
    # Debian starts nothing in a container; start what is installed, as it
    # would be on a server.
    case "$SERVER" in
      nginx) nginx ;;
      apache) apache2ctl -k start ;;
      caddy)
        # A Caddyfile that already has content -- a global block -- which the
        # wizard must keep. local_certs stands in for Lets Encrypt.
        printf "{\n\tlocal_certs\n}\n\n" > /etc/caddy/Caddyfile
        caddy start --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 ;;
    esac
    sleep 2
  '

  # --- the wizard, answered on stdin
  printf '%s\n' $answers | docker exec -i bc-proxy breeze-core proxy --server "$server" --domain "$DOMAIN" > "packaging/out/proxy-$server.log" 2>&1 \
    && wizard=0 || wizard=$?

  # --- what it left behind
  result="$(docker exec -e SERVER="$server" -e DOMAIN="$DOMAIN" bc-proxy sh -c '
    ok() { echo "  ok    $1"; }; bad() { echo "  FAIL  $1"; }
    grep -q "^BREEZE_HOST=127.0.0.1$" /etc/breeze-core/breeze-core.env && ok "BREEZE_HOST is 127.0.0.1" || bad "BREEZE_HOST"
    grep -q "^BREEZE_OPTS=--behind-proxy$" /etc/breeze-core/breeze-core.env && ok "--behind-proxy is set" || bad "--behind-proxy"
    [ -s /etc/breeze-core/proxy-undo.json ] && ok "the undo journal was written" || bad "no journal"
    # Restart Breeze Core onto the new settings, as the init system would.
    pkill -x breeze-core; sleep 3
    if [ "$SERVER" = caddy ]; then
      code=$(curl -sk -o /dev/null -w "%{http_code}" --resolve "$DOMAIN:443:127.0.0.1" "https://$DOMAIN/api/health")
    else
      code=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: $DOMAIN" http://127.0.0.1/api/health)
    fi
    [ "$code" = 200 ] && ok "$DOMAIN/api/health through the proxy: 200" || bad "health through the proxy: $code"
  ' 2>&1)"
  echo "$result"

  # A client in another container, forging the header the LAN check reads.
  docker rm -f bc-client >/dev/null 2>&1 || true
  if [ "$server" = caddy ]; then
    url="https://$DOMAIN/api/health"
    docker run --rm --name bc-client --network "$NET" curlimages/curl:latest -sk \
      --resolve "$DOMAIN:443:$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' bc-proxy)" \
      -H "X-Forwarded-For: 192.168.1.5" "$url" >/dev/null 2>&1 || true
  else
    docker run --rm --name bc-client --network "$NET" curlimages/curl:latest -s \
      -H "Host: $DOMAIN" -H "X-Forwarded-For: 192.168.1.5" http://bc-proxy/api/health >/dev/null 2>&1 || true
  fi
  # The client is somewhere on the test network's /24 (not the proxy itself);
  # Breeze Core logs "<client> (via 127.0.0.1) GET ...".
  subnet="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' bc-proxy | sed 's/\.[0-9]*$//' | sed 's/\./\\./g')"
  forged="$(docker exec bc-proxy sh -c 'tail -3 /tmp/breeze.log')"
  if echo "$forged" | grep -q "192\.168\.1\.5"; then
    echo "  FAIL  the forged X-Forwarded-For reached the server as the client"; result="$result FAIL"
  elif echo "$forged" | grep -q "^$subnet\.[0-9]* (via 127\.0\.0\.1) GET /api/health"; then
    echo "  ok    a forged X-Forwarded-For lost: logged as $(echo "$forged" | grep -o "^$subnet\.[0-9]*" | tail -1)"
  else
    echo "  FAIL  could not see the client's request in the log:"; echo "$forged" | sed 's/^/        /'; result="$result FAIL"
  fi

  # --- and back out
  undo="$(echo y | docker exec -i bc-proxy breeze-core proxy --undo 2>&1)" || true
  echo "$undo" > "packaging/out/proxy-$server-undo.log"
  back="$(docker exec -e SERVER="$server" bc-proxy sh -c '
    ok() { echo "  ok    $1"; }; bad() { echo "  FAIL  $1"; }
    cmp -s /etc/breeze-core/breeze-core.env /tmp/env.original && ok "undo: the env file is as it was" || bad "undo: env differs"
    [ ! -e /etc/breeze-core/proxy-undo.json ] && ok "undo: the journal is gone" || bad "undo: journal left"
    case "$SERVER" in
      nginx)  [ ! -e /etc/nginx/conf.d/breeze-core.conf ] && ok "undo: the nginx file is gone" || bad "undo: nginx file left"; nginx -t 2>/dev/null && ok "undo: nginx still validates" || bad "undo: nginx broken" ;;
      apache) [ ! -e /etc/apache2/sites-available/breeze-core.conf ] && [ ! -e /etc/apache2/sites-enabled/breeze-core.conf ] && ok "undo: the site is gone and disabled" || bad "undo: apache site left"; apache2ctl configtest 2>/dev/null && ok "undo: apache still validates" || bad "undo: apache broken" ;;
      caddy)  printf "{\n\tlocal_certs\n}\n\n" | cmp -s - /etc/caddy/Caddyfile && ok "undo: the Caddyfile is as it was" || bad "undo: Caddyfile differs" ;;
    esac
  ' 2>&1)"
  echo "$back"

  if [ "$wizard" = 0 ] && ! echo "$result $back" | grep -q FAIL; then
    printf '  \033[32mPASS\033[0m  %s (transcripts: packaging/out/proxy-%s*.log)\n' "$server" "$server"; pass=$((pass+1))
  else
    printf '  \033[31mFAIL\033[0m  %s (wizard exit %s; see packaging/out/proxy-%s.log)\n' "$server" "$wizard" "$server"; fail=$((fail+1))
  fi
  docker rm -f bc-proxy >/dev/null 2>&1 || true
done

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" -eq 0 ]
