#!/bin/bash
# FreeForGem - see LICENSE
# Integration test of the accepted-shares watchdog across pool switches with local mock pools (tests/mockpool.py), one coin per run:
#   noshare_switch_test.sh pearl|noid|prox|innov [BIN_DIR] [OUT_DIR]
# Two pools of very different share difficulty: the primary is easy (shares every few seconds), the backup is hard (about 100x
# rarer).  The session goes primary -> backup -> primary -> backup -> primary (the primary mock is killed and started again, so
# every move is a real failover / failback).  With --noshare-minutes 0.4 the watchdog must NEVER say "no share found ... although
# ~N were expected" or act at all: the number of shares expected on the backup comes from ITS difficulty, never from the easy
# pool's rate (1.7.8: CMFD kept the rate of the previous pool: "no share found in 5.8 min although ~10 were expected").
# Needs a GPU (real mining against the mocks), localhost only, ~5 minutes.
set -u
COIN=${1:?usage: noshare_switch_test.sh pearl|noid|prox|innov [BIN_DIR] [OUT_DIR]}
BIN=${2:-$PWD}
OUT=${3:-/tmp/noshare-switch-test-$COIN}
HERE=$(cd "$(dirname "$0")" && pwd)
PA=${PA:-41301}; PB=${PB:-41302}
mkdir -p "$OUT"; cd "$OUT" || exit 2
rm -f a.ev b.ev miner.log
MOCKS=(); APID=""
cleanup() { for p in "${MOCKS[@]}" $APID; do kill "$p" 2>/dev/null; done; }
trap cleanup EXIT
fail=0
ok()   { echo "  ok:   $*"; }
bad()  { echo "  FAIL: $*"; fail=1; }
check() { if eval "$1"; then ok "$2"; else bad "$2 [$1]"; fi; }
TLS=""
case $COIN in
  pearl) MODE=pearl; POOLS="127.0.0.1:$PA,127.0.0.1:$PB"; W=prl1pqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq
         EASY="--difficulty 10000"; HARD="--difficulty 1000000"
         ARGS="--wallet $W --worker nstest --lock-cclock 1500 --lock-mclock 810";;
  noid)  MODE=noid; TLS="--tls $HERE/fixtures/mock-cert.pem $HERE/fixtures/mock-key.pem"
         POOLS="stratum+ssl://127.0.0.1:$PA?insecure,stratum+ssl://127.0.0.1:$PB?insecure"; W=o1qtestwallet0000000000000000000000000000000000000000000000
         EASY="--noid-hi 0000000040000000"; HARD="--noid-hi 0000000000400000"
         ARGS="--algo noid --wallet $W --worker nstest --lock-cclock 1500 --lock-mclock 810";;
  prox)  MODE=prox; POOLS="stratum+tcp://127.0.0.1:$PA,stratum+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         EASY="--k 28"; HARD="--k 35"
         ARGS="--algo prox --wallet $W --worker nstest --password x --lock-cclock 1500";;
  innov) MODE=innov; POOLS="innov+tcp://127.0.0.1:$PA,innov+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         EASY="--k 28"; HARD="--k 35"
         ARGS="--algo prox --wallet $W --worker nstest --lock-cclock 1500";;
  *) echo "unknown coin $COIN"; exit 2;;
esac
if [ "${FB_NOCLOCK:-0}" = 1 ]; then ARGS=$(echo "$ARGS" | sed 's/--lock-[cm]clock [0-9]*//g'); fi
BIN_EXE="$BIN/freeforgem"; [ -x "$BIN_EXE" ] || BIN_EXE="$BIN/freeforgem.exe"
UW=${W:0:12}
PY=${PYTHON:-python3}
starta() { $PY "$HERE/mockpool.py" --mode "$MODE" --port "$PA" --events a.ev $EASY $TLS & APID=$!; sleep 1; }
stopa()  { kill "$APID" 2>/dev/null; wait "$APID" 2>/dev/null; APID=""; }
$PY "$HERE/mockpool.py" --mode "$MODE" --port "$PB" --events b.ev $HARD $TLS & MOCKS+=($!)
starta
echo "== $COIN: primary :$PA easy, backup :$PB hard; five moves, no watchdog line allowed"
FFGP_DEVFEE_TEST=2 FFGP_FAILBACK_CONFIRM_S=8 FFGP_FAILBACK_TICK_MS=500 FFG_COLOR=0 \
  "$BIN_EXE" --pool "$POOLS" $ARGS --noshare-minutes 0.4 --reject-minutes 0.3 --failback-minutes 0.15 --minutes 6 --no-update-check --uv-dir "$OUT/uv" > miner.log 2>&1 &
MINER=$!
waitfor() { local t=0; while [ $t -lt "$2" ]; do eval "$1" && return 0; sleep 1; t=$((t+1)); done; return 1; }
for round in 1 2; do
  waitfor '[ "$(grep -c " share $UW" a.ev)" -ge $((3*round)) ]' 90 && ok "round $round: shares reach the easy primary" || bad "round $round: no shares on the primary"
  stopa
  waitfor 'grep -a -c "failing over to pool 2" miner.log | grep -q "^'$round'$"' 60 && ok "round $round: failover" || bad "round $round: no failover"
  sleep 50   # on the hard backup longer than the watchdog window (0.4 min)
  starta
  touch a.ev
  waitfor 'grep -a -c "answered 2 checks in a row" miner.log | grep -q "^'$round'$"' 90 && ok "round $round: failback" || bad "round $round: no failback"
done
sleep 20
echo "== accounting"
check '! grep -a "watchdog:" miner.log' "the watchdog said nothing in the whole run"
check '! grep -a "no share found" miner.log' "no 'no share found' warning"
check '[ "$(grep -c " share $UW" b.ev)" -ge 1 ]' "shares reached the hard backup too ($(grep -c " share $UW" b.ev))"
check '! grep -a -i "panick" miner.log' "no panics"
kill -INT $MINER 2>/dev/null; wait $MINER 2>/dev/null
echo
[ $fail -eq 0 ] && echo "RESULT noshare_switch $COIN: PASSED" || echo "RESULT noshare_switch $COIN: FAILED"
exit $fail
