#!/usr/bin/env bash
# End-to-end test: raddy auto-provisions a certificate for a named site via
# ACME (against a local Pebble server) and serves HTTPS with it.
#
# Requires: sudo (to bind :443 and :80), a working openssl, network to fetch
# pebble on first run.
#
# Usage: tests/pebble/e2e.sh
set -euo pipefail

cd "$(dirname "$0")/../.."          # repo root
ROOT="$(pwd)"
PEBBLE_DIR="tests/pebble"
BIN="$ROOT/target/debug/raddy"

# --- ensure the pebble binary is present ---
PEBBLE_BIN="$ROOT/$PEBBLE_DIR/pebble-linux-amd64/linux/amd64/pebble"
if [ ! -x "$PEBBLE_BIN" ]; then
  echo "setting up pebble (first run)..."
  bash "$ROOT/$PEBBLE_DIR/setup.sh"
fi

UPSTREAM_PORT=19090
CERT_DIR=$(mktemp -d)
CONFIG=$(mktemp --suffix=.Raddyfile)
PEBBLE_CA="$PEBBLE_DIR/certs/pebble-ca.pem"
cleanup() {
  [ -n "${RADDY_PID:-}" ] && sudo kill "$RADDY_PID" 2>/dev/null || true
  [ -n "${PEBBLE_PID:-}" ] && kill "$PEBBLE_PID" 2>/dev/null || true
  pkill -x pebble 2>/dev/null || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" 2>/dev/null || true
  rm -rf "$CERT_DIR" "$CONFIG"
}
trap cleanup EXIT

# --- build ---
cargo build -q

# --- start an upstream ---
python3 -m http.server "$UPSTREAM_PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
UP_PID=$!
sleep 0.5

# --- pebble ---
(
  cd "$ROOT/$PEBBLE_DIR"
  PEBBLE_VA_ALWAYS_VALID=1 "$PEBBLE_BIN" -config config/pebble-config.json >pebble.log 2>&1
) &
PEBBLE_PID=$!
sleep 2
if ! timeout 5 curl -s --cacert "$PEBBLE_CA" https://localhost:14000/dir >/dev/null 2>&1; then
  echo "pebble did not become reachable" >&2
  exit 1
fi

# --- raddy config: one named site (default 443 -> TLS) proxying to the upstream ---
cat > "$CONFIG" <<EOF
raddy.test {
    reverse_proxy 127.0.0.1:$UPSTREAM_PORT
}
EOF

# --- run raddy under sudo so :443 can bind ---
sudo "$BIN" run -c "$CONFIG" \
  --cert-dir "$CERT_DIR" \
  --acme-directory "https://localhost:14000/dir" \
  --acme-root-pem "$PEBBLE_CA" \
  >/tmp/raddy_e2e.log 2>&1 &
RADDY_PID=$!

# --- wait for the certificate to be issued ---
echo "waiting for ACME certificate..."
for _ in $(seq 1 60); do
  if [ -f "$CERT_DIR/raddy.test.pem" ]; then
    echo "certificate issued: $CERT_DIR/raddy.test.pem"
    break
  fi
  sleep 1
done
if [ ! -f "$CERT_DIR/raddy.test.pem" ]; then
  echo "timed out waiting for certificate; raddy log:" >&2
  tail -30 /tmp/raddy_e2e.log >&2 || true
  exit 1
fi

# --- verify HTTPS through raddy with the issued certificate ---
# `curl --cacert` validates the served leaf against the issued chain, so a 200
# proves the TLS listener served the correct ACME certificate for raddy.test.
STATUS=$(curl -s -o /dev/null -w "%{http_code}" --cacert "$CERT_DIR/raddy.test.pem" \
  --resolve raddy.test:443:127.0.0.1 \
  https://raddy.test/ 2>/dev/null)
if [ "$STATUS" = "200" ]; then
  echo "OK: HTTPS forwarding works with the ACME-issued certificate (status $STATUS)"
else
  echo "FAILED: unexpected HTTPS status: $STATUS" >&2
  exit 1
fi

echo "e2e PASSED"
