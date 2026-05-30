#!/usr/bin/env bash
#
# Multi-node sync integration test. Builds the harness image, brings up three
# isolated nodes, and asserts (1) two-node sync and (2) roster-driven mesh
# device introduction over real device-to-device connections.
#
# Requires Docker. Run: tests/docker/run.sh   (or: make test-sync-docker)

set -euo pipefail
cd "$(dirname "$0")"

A=http://localhost:18090
B=http://localhost:18091
C=http://localhost:18092

cleanup() { docker compose down -v >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "==> Building image (first build is slow)…"
docker compose build
echo "==> Starting nodes…"
docker compose up -d

wait_up() {
  local url=$1 name=$2 i
  for i in $(seq 1 90); do
    if curl -fsS "$url/id" >/dev/null 2>&1; then echo "    $name is up"; return 0; fi
    sleep 2
  done
  echo "TIMEOUT waiting for $name control API"; docker compose logs "$name"; return 1
}

get_id()    { curl -fsS "$1/id" | sed -E 's/.*"deviceId":"([^"]+)".*/\1/'; }
pair()      { curl -fsS -X POST "$1/pair" -d "{\"deviceId\":\"$2\",\"name\":\"$3\"}" >/dev/null; }
make_book() { curl -fsS -X POST "$1/book" -d "{\"title\":\"$2\",\"text\":\"lorem ipsum dolor sit amet\"}" >/dev/null; }

# Poll until GET $url$path contains $needle, or fail.
wait_contains() {
  local url=$1 path=$2 needle=$3 label=$4 i
  for i in $(seq 1 90); do
    if curl -fsS "$url$path" | grep -q "$needle"; then echo "    PASS: $label"; return 0; fi
    sleep 2
  done
  echo "    FAIL: $label"
  echo "      (never saw '$needle' at $url$path; last = $(curl -fsS "$url$path" 2>/dev/null))"
  return 1
}

echo "==> Waiting for control APIs…"
wait_up "$A" node-a
wait_up "$B" node-b
wait_up "$C" node-c

ID_A=$(get_id "$A"); ID_B=$(get_id "$B"); ID_C=$(get_id "$C")
echo "    a=$ID_A"
echo "    b=$ID_B"
echo "    c=$ID_C"

echo "==> Scenario 1: two-node sync (pair a <-> b)"
pair "$A" "$ID_B" node-b
pair "$B" "$ID_A" node-a
make_book "$A" "Book-From-A"
wait_contains "$B" /books "Book-From-A" "book created on A reached B"

echo "==> Scenario 2: device introduction / mesh (pair c with a only)"
pair "$A" "$ID_C" node-c
pair "$C" "$ID_A" node-a
# b only ever paired with a; it must learn c from the roster propagated via a.
wait_contains "$B" /devices "$ID_C" "node-c appears in node-b's devices (roster mesh closure)"
# …and data created on c must reach b through the mesh it just formed.
make_book "$C" "Book-From-C"
wait_contains "$B" /books "Book-From-C" "book created on C reached B via the mesh"

echo "==> ALL SCENARIOS PASSED"
