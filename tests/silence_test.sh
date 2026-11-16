#!/bin/bash
# FreeForGem - see LICENSE
# Integration test of the dead-link detection (host/src/silence.rs) with local mock pools (tests/mockpool.py), one coin per run:
#   silence_test.sh pearl|noid|prox|innov mute|quiet [BIN_DIR] [OUT_DIR]
# mute:  the primary goes half-open 50 s after it started (--mute-after: the open connection stays open but nothing arrives any
#        more, no new connection is accepted).  The miner must notice within 75 s of that and fail over to the backup, whose shares
#        are accepted.  (Failover before this check took 180..300 s.)
# quiet: ONE pool that sends a job only every 45 s (like the real Pearl pools) for 3.5 minutes: the miner must not reconnect
#        (no false dead-link verdicts: exactly one login, no "no data from the pool" line).
# Needs a GPU (real mining against the mocks), localhost only.
set -u
COIN=${1:?usage: silence_test.sh pearl|noid|prox|innov mute|quiet [BIN_DIR] [OUT_DIR]}
MODE=${2:?mute|quiet}
BIN=${3:-$PWD}
OUT=${4:-/tmp/silence-test-$COIN-$MODE}
HERE=$(cd "$(dirname "$0")" && pwd)
PA=${PA:-41301}; PB=${PB:-41302}
if (echo > /dev/tcp/127.0.0.1/$PA) 2>/dev/null || (echo > /dev/tcp/127.0.0.1/$PB) 2>/dev/null; then echo "ports $PA/$PB are in use (a mock or a miner of an earlier run?): refusing to run"; exit 2; fi
mkdir -p "$OUT"; cd "$OUT" || exit 2
rm -f a.ev b.ev miner.log
MOCKS=()
cleanup() { for p in "${MOCKS[@]}"; do kill "$p" 2>/dev/null; done; }
trap cleanup EXIT
fail=0
ok()  { echo "  ok:   $*"; }
bad() { echo "  FAIL: $*"; fail=1; }
check() { if eval "$1"; then ok "$2"; else bad "$2 [$1]"; fi; }
TLS=""
case $COIN in
  pearl) KIND=pearl; POOLS="127.0.0.1:$PA,127.0.0.1:$PB"; W=prl1pqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq
         ARGS="--wallet $W --worker sltest --lock-cclock 1500 --lock-mclock 810";;
  noid)  KIND=noid; TLS="--tls $HERE/fixtures/mock-cert.pem $HERE/fixtures/mock-key.pem"
         POOLS="stratum+ssl://127.0.0.1:$PA?insecure,stratum+ssl://127.0.0.1:$PB?insecure"; W=o1qtestwallet0000000000000000000000000000000000000000000000
         ARGS="--algo noid --wallet $W --worker sltest --lock-cclock 1500 --lock-mclock 810";;
  prox)  KIND=prox; POOLS="stratum+tcp://127.0.0.1:$PA,stratum+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         ARGS="--algo prox --wallet $W --worker sltest --password x --lock-cclock 1500";;
  innov) KIND=innov; POOLS="innov+tcp://127.0.0.1:$PA,innov+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         ARGS="--algo prox --wallet $W --worker sltest --lock-cclock 1500";;
  *) echo "unknown coin $COIN"; exit 2;;
esac
if [ "${FB_NOCLOCK:-0}" = 1 ]; then ARGS=$(echo "$ARGS" | sed 's/--lock-[cm]clock [0-9]*//g'); fi
BIN_EXE="$BIN/freeforgem"; [ -x "$BIN_EXE" ] || BIN_EXE="$BIN/freeforgem.exe"
UW=${W:0:12}
PY=${PYTHON:-python3}
if [ "$MODE" = mute ]; then
  $PY "$HERE/mockpool.py" --mode "$KIND" --port "$PA" --events a.ev --difficulty 10000 --mute-after 50 $TLS & MOCKS+=($!)
  $PY "$HERE/mockpool.py" --mode "$KIND" --port "$PB" --events b.ev --difficulty 10000 $TLS & MOCKS+=($!)
  sleep 1
  echo "== $COIN: the primary :$PA goes half-open 50 s after its start, the backup :$PB is normal"
  FFG_COLOR=0 "$BIN_EXE" --pool "$POOLS" $ARGS --minutes 3 --no-update-check --uv-dir "$OUT/uv" > miner.log 2>&1 &
  MINER=$!
  t=0; while [ $t -lt 150 ]; do [ "$(grep -c " share $UW" b.ev 2>/dev/null)" -ge 2 ] && break; sleep 1; t=$((t+1)); done
  TM=$(grep -m1 "^[0-9.]* mute" a.ev | cut -d' ' -f1)
  TB=$(grep -m1 " login $UW" b.ev | cut -d' ' -f1)
  check '[ -n "$TM" ] && [ -n "$TB" ]' "the primary went silent and the miner logged in on the backup"
  if [ -n "$TM" ] && [ -n "$TB" ]; then
    D=$(python3 -c "print(round($TB-$TM,1))" 2>/dev/null || $PY -c "print(round($TB-$TM,1))")
    echo "  failover took $D s after the primary went silent"
    check '[ "$(echo "$D <= 75" | bc 2>/dev/null || $PY -c "print(int($D<=75))")" = 1 ]' "failover within 75 s ($D s)"
  fi
  check 'grep -aq "no data from the pool for" miner.log' "the silence was reported by the session"
  check 'grep -aq "failing over to pool 2" miner.log' "failover logged"
  check '[ "$(grep -c " share $UW" b.ev)" -ge 2 ]' "shares reach the backup ($(grep -c " share $UW" b.ev))"
  check '[ "$(grep -a -c ":user\] share ACCEPTED" miner.log)" -ge 2 ]' "the miner counts them as accepted"
else
  $PY "$HERE/mockpool.py" --mode "$KIND" --port "$PA" --events a.ev --difficulty 10000 --interval 45 $TLS & MOCKS+=($!)
  sleep 1
  echo "== $COIN: one pool, a job every 45 s, 3.5 minutes: no reconnect allowed"
  case $COIN in noid) POOL1="stratum+ssl://127.0.0.1:$PA?insecure";; prox) POOL1="stratum+tcp://127.0.0.1:$PA";; innov) POOL1="innov+tcp://127.0.0.1:$PA";; *) POOL1="127.0.0.1:$PA";; esac
  FFG_COLOR=0 "$BIN_EXE" --pool "$POOL1" $ARGS --minutes 3.5 --no-update-check --uv-dir "$OUT/uv" > miner.log 2>&1 &
  MINER=$!
  sleep 215
  check '[ "$(grep -c " login $UW" a.ev)" -eq 1 ]' "exactly one login on the pool ($(grep -c " login $UW" a.ev)): no reconnect"
  check '! grep -aq "no data from the pool for" miner.log' "no dead-link verdict in the log"
  check '[ "$(grep -c " share $UW" a.ev)" -ge 1 ]' "shares were sent ($(grep -c " share $UW" a.ev))"
fi
kill -INT $MINER 2>/dev/null
for i in $(seq 1 90); do kill -0 $MINER 2>/dev/null || break; sleep 1; done
kill -KILL $MINER 2>/dev/null; wait $MINER 2>/dev/null
check '! grep -a -i "panick" miner.log' "no panics"
echo
[ $fail -eq 0 ] && echo "RESULT silence $COIN $MODE: PASSED" || echo "RESULT silence $COIN $MODE: FAILED"
exit $fail
