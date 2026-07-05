#!/usr/bin/env bash
# Run a monad-sonar debug crawl and emit per-validator APP-LEVEL RTT via the auth-UDP ping/pong
# roundtrip (reaches validators that answer discovery but block ICMP). RTT is from the vantage
# this runs on. If a geo-latency dir is passed, self-calibrates against ICMP -> comparable ms.
# Usage: sonar-rtt.sh [network] [config] [run_secs] [geo_dir_for_calibration]
set -uo pipefail
cd "$(dirname "$0")/.."
BIN=${SONAR_BIN:-./target/debug/monad-sonar}
NET=${1:-testnet}; CFG=${2:-configs/$NET.toml}; SECS=${3:-40}; GEO=${4:-}
LOG=$(mktemp)
RUST_LOG='monad_peer_discovery=debug' NO_COLOR=1 timeout $((SECS+45)) "$BIN" peers \
  --network "$NET" --config "$CFG" --run-secs "$SECS" --out "/tmp/sonar-peers-$NET.json" >"$LOG" 2>&1
sed -i 's/\x1b\[[0-9;]*m//g' "$LOG"
if [ -n "$GEO" ]; then
  python3 tools/sonar_rtt.py "$LOG" "sonar-rtt-$NET.json" "$GEO/rtt-averaged.json" "$GEO/validators-with-ips.json"
else
  python3 tools/sonar_rtt.py "$LOG" "sonar-rtt-$NET.json"
fi
rm -f "$LOG"
