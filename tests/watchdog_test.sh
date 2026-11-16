#!/bin/bash
# FreeForGem - see LICENSE
# Integration test of the accepted-shares watchdog with local mock pools (tests/mockpool.py), one coin and one fault per run:
#   watchdog_test.sh pearl|noid|prox|innov reject|silent [BIN_DIR] [OUT_DIR]
# The primary pool takes the connection and the login and sends jobs, but REJECTS every share (reject) or never answers one (silent);
# the backup is a normal pool. The miner (--noshare-minutes 0.4 --reject-minutes 0.3) must reconnect, then move to the backup and get
# its shares accepted there. Needs a GPU (real mining against the mocks), localhost only, ~2-3 minutes.
set -u
COIN=${1:?usage: watchdog_test.sh pearl|noid|prox|innov reject|silent [BIN_DIR] [OUT_DIR]}
MODE=${2:?reject|silent}
BIN=${3:-$PWD}
OUT=${4:-/tmp/watchdog-test-$COIN-$MODE}
HERE=$(cd "$(dirname "$0")" && pwd)
PA=${PA:-41201}; PB=${PB:-41202}
mkdir -p "$OUT"; cd "$OUT" || exit 2
rm -f a.ev b.ev miner.log
MOCKS=()
cleanup() { for p in "${MOCKS[@]}"; do kill "$p" 2>/dev/null; done; }
trap cleanup EXIT
fail=0
ok()  { echo "  ok:   $*"; }
bad() { echo "  FAIL: $*"; fail=1; }
TLS=""
case $COIN in
  pearl) KIND=pearl; POOLS="127.0.0.1:$PA,127.0.0.1:$PB"; W=prl1pqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq
         ARGS="--wallet $W --worker wdtest --lock-cclock 1500 --lock-mclock 810";;
  noid)  KIND=noid; TLS="--tls $HERE/fixtures/mock-cert.pem $HERE/fixtures/mock-key.pem"
         POOLS="stratum+ssl://127.0.0.1:$PA?insecure,stratum+ssl://127.0.0.1:$PB?insecure"; W=o1qtestwallet0000000000000000000000000000000000000000000000
         ARGS="--algo noid --wallet $W --worker wdtest --lock-cclock 1500 --lock-mclock 810";;
  prox)  KIND=prox; POOLS="stratum+tcp://127.0.0.1:$PA,stratum+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         ARGS="--algo prox --wallet $W --worker wdtest --password x --lock-cclock 1500";;
  innov) KIND=innov; POOLS="innov+tcp://127.0.0.1:$PA,innov+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111
         ARGS="--algo prox --wallet $W --worker wdtest --lock-cclock 1500";;
  *) echo "unknown coin $COIN"; exit 2;;
esac
if [ "${FB_NOCLOCK:-0}" = 1 ]; then ARGS=$(echo "$ARGS" | sed 's/--lock-[cm]clock [0-9]*//g'); fi
BIN_EXE="$BIN/freeforgem"; [ -x "$BIN_EXE" ] || BIN_EXE="$BIN/freeforgem.exe"
UW=${W:0:12}
PY=${PYTHON:-python3}
$PY "$HERE/mockpool.py" --mode "$KIND" --port "$PA" --events a.ev --submit-mode "$MODE" --difficulty 10000 $TLS & MOCKS+=($!)
$PY "$HERE/mockpool.py" --mode "$KIND" --port "$PB" --events b.ev --difficulty 10000 $TLS & MOCKS+=($!)
sleep 1
echo "== $COIN: the primary :$PA answers shares with '$MODE', the backup :$PB is normal"
FFGP_DEVFEE_TEST=2 FFGP_FAILBACK_CONFIRM_S=8 FFGP_FAILBACK_TICK_MS=500 FFG_COLOR=0 \
  "$BIN_EXE" --pool "$POOLS" $ARGS --noshare-minutes 0.4 --reject-minutes 0.3 --failback-minutes 1 --minutes 3.5 --no-update-check --uv-dir "$OUT/uv" > miner.log 2>&1 &
MINER=$!
t=0; while [ $t -lt 170 ]; do [ "$(grep -c " share $UW" b.ev 2>/dev/null)" -ge 3 ] && break; sleep 1; t=$((t+1)); done
check() { if eval "$1"; then ok "$2"; else bad "$2 [$1]"; fi; }
check '[ "$(grep -c " share $UW" a.ev)" -ge 3 ]' "shares were sent to the primary first ($(grep -c " share $UW" a.ev))"
check 'grep -aq "watchdog: .*reconnecting" miner.log' "the watchdog reconnected once"
check 'grep -aq "watchdog: .*switching to pool 2 of 2" miner.log' "the watchdog moved to the next pool"
check '[ "$(grep -c " share $UW" b.ev)" -ge 3 ]' "shares are accepted on the backup ($(grep -c " share $UW" b.ev), $t s)"
# the pool logs a share before the miner prints its answer: give the acknowledgements a few seconds
t=0; while [ $t -lt 15 ] && [ "$(grep -a -c ":user\] share ACCEPTED" miner.log)" -lt 3 ]; do sleep 1; t=$((t+1)); done
check '[ "$(grep -a -c ":user\] share ACCEPTED" miner.log)" -ge 3 ]' "the miner counts them as accepted"
check '! grep -a -i "panick" miner.log' "no panics"
kill -INT $MINER 2>/dev/null; wait $MINER 2>/dev/null
echo
[ $fail -eq 0 ] && echo "RESULT watchdog $COIN $MODE: PASSED" || echo "RESULT watchdog $COIN $MODE: FAILED"
exit $fail
