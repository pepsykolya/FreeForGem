#!/bin/bash
# FreeForGem - see LICENSE
# Integration test of pool failover and failback with local mock pools (tests/mockpool.py), one coin per run:
#   failback_test.sh pearl|noid|prox|innov [BIN_DIR] [OUT_DIR]
# The miner is started with --pool PRIMARY,BACKUP while the primary is DOWN; the backup takes over (failover), then the primary
# is brought up and the miner must return to it (failback) within the check interval; shares keep being accepted on both, every
# share a pool received was answered (nothing lost in the switch), and the dev fee connection goes to the pool the user is on
# (FFGP_DEVFEE_TEST=2: 70 s cycle, 15 s slot).  Needs a GPU (real mining against the mocks): run it on a card you may use
# (CUDA_VISIBLE_DEVICES), e.g. through the lab's slot tool.  Localhost only; never contacts a real pool.  ~4 minutes.
set -u
COIN=${1:?usage: failback_test.sh pearl|noid|prox|innov [BIN_DIR] [OUT_DIR]}
BIN=${2:-$PWD}
OUT=${3:-/tmp/failback-test-$COIN}
HERE=$(cd "$(dirname "$0")" && pwd)
PA=${PA:-41101}; PB=${PB:-41102}
mkdir -p "$OUT"; cd "$OUT" || exit 2
rm -f a.ev b.ev miner.log ./*.out
MOCKS=()
cleanup() { for p in "${MOCKS[@]}"; do kill "$p" 2>/dev/null; done; }
trap cleanup EXIT
fail=0
ok()   { echo "  ok:   $*"; }
bad()  { echo "  FAIL: $*"; fail=1; }
check() { if eval "$1"; then ok "$2"; else bad "$2 [$1]"; fi; }

if [ "$COIN" = noid ]; then   # TLS (stratum+ssl, ?insecure: a self-signed certificate) to exercise the TLS probe too
  # a self-signed test certificate for 127.0.0.1 (tests/fixtures, localhost mock only)
  TLS="--tls $HERE/fixtures/mock-cert.pem $HERE/fixtures/mock-key.pem"
else TLS=""; fi
PY=${PYTHON:-python3}
mock() { $PY "$HERE/mockpool.py" --mode "$1" --port "$2" --events "$3" $TLS & MOCKS+=($!); }
case $COIN in
  pearl) MODE=pearl; POOLS="127.0.0.1:$PA,127.0.0.1:$PB"; W=prl1pqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq; FEEW=prl1pjcsj
         ARGS="--wallet $W --worker fbtest --lock-cclock 1500 --lock-mclock 810";;
  noid)  MODE=noid; POOLS="stratum+ssl://127.0.0.1:$PA?insecure,stratum+ssl://127.0.0.1:$PB?insecure"; W=o1qtestwallet0000000000000000000000000000000000000000000000; FEEW=o1qxh67m5
         ARGS="--algo noid --wallet $W --worker fbtest --lock-cclock 1500 --lock-mclock 810";;
  prox)  MODE=prox; POOLS="stratum+tcp://127.0.0.1:$PA,stratum+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111; FEEW=fb8f3389
         ARGS="--algo prox --wallet $W --worker fbtest --password x --lock-cclock 1500";;
  innov) MODE=innov; POOLS="innov+tcp://127.0.0.1:$PA,innov+tcp://127.0.0.1:$PB"; W=1111111111111111111111111111111111111111111111111111111111111111; FEEW=fb8f3389
         ARGS="--algo prox --wallet $W --worker fbtest --lock-cclock 1500";;
  *) echo "unknown coin $COIN"; exit 2;;
esac
if [ "${FB_NOCLOCK:-0}" = 1 ]; then ARGS=$(echo "$ARGS" | sed 's/--lock-[cm]clock [0-9]*//g'); fi   # no clock locks (no admin rights / not wanted)
BIN_EXE="$BIN/freeforgem"; [ -x "$BIN_EXE" ] || BIN_EXE="$BIN/freeforgem.exe"
UW=${W:0:12}

echo "== $COIN: failover (the primary 127.0.0.1:$PA is down, the backup :$PB is up)"
mock "$MODE" "$PB" b.ev
sleep 1
# the check every 9 s (--failback-minutes 0.15), 8 s between the two good checks
FFGP_DEVFEE_TEST=2 FFGP_FAILBACK_CONFIRM_S=8 FFGP_FAILBACK_TICK_MS=500 FFG_COLOR=0 \
  "$BIN_EXE" --pool "$POOLS" $ARGS --failback-minutes 0.15 --minutes 4.2 --no-update-check --uv-dir "$OUT/uv" > miner.log 2>&1 &
MINER=$!
echo "  miner pid $MINER"
t=0; while [ $t -lt 90 ]; do grep -q " share " b.ev 2>/dev/null && [ "$(grep -c " share $UW" b.ev)" -ge 3 ] && break; sleep 1; t=$((t+1)); done
check '[ "$(grep -c " login $UW" b.ev)" -ge 1 ]' "the user logged in on the backup"
check '[ "$(grep -c " share $UW" b.ev)" -ge 3 ]' "shares reach the backup ($(grep -c " share $UW" b.ev) so far)"
grep -q "failing over to pool 2" miner.log && ok "failover logged" || bad "no failover line in the log"
# the fee slot (15 s, within the first minute) must go to the backup while the user is there
t=0; while [ $t -lt 70 ]; do grep -q " login .*$FEEW" b.ev 2>/dev/null && break; sleep 1; t=$((t+1)); done
check 'grep -q " login .*'$FEEW'" b.ev' "the dev fee logged in on the pool the user is on (the backup)"
echo "== the primary comes back"
T_UP=$(date +%s)
mock "$MODE" "$PA" a.ev
touch a.ev
t=0; while [ $t -lt 90 ]; do [ "$(grep -c " share $UW" a.ev)" -ge 3 ] && break; sleep 1; t=$((t+1)); done
check '[ "$(grep -c " share $UW" a.ev)" -ge 3 ]' "after the failback shares reach the primary ($(grep -c " share $UW" a.ev) so far, $(( $(date +%s) - T_UP )) s after it came up)"
grep -q "answered 2 checks in a row" miner.log && ok "failback logged" || bad "no failback line in the log"
# once the user is on the primary no more shares go to the backup (a short grace for answers in flight)
sleep 3; N0=$(grep -c " share $UW" b.ev); sleep 15; N1=$(grep -c " share $UW" b.ev)
check '[ "$N1" -eq "$N0" ]' "no user shares reach the backup after the failback ($N0 -> $N1)"
# the next fee slot goes to the primary
t=0; while [ $t -lt 100 ]; do grep -q " login .*$FEEW" a.ev 2>/dev/null && break; sleep 1; t=$((t+1)); done
check 'grep -q " login .*'$FEEW'" a.ev' "the dev fee follows the user back to the primary"
echo "== accounting (until the run ends)"
wait $MINER 2>/dev/null
SENT=$(( $(grep -c " share $UW" a.ev) + $(grep -c " share $UW" b.ev) ))
echo "  last status line: $(grep -a "user sub" miner.log | tail -1)"
# the user session's own log lines (the status line is printed every 10 s only, so it can be a few shares behind at the end)
ACC=$(grep -a -c ":user\] share ACCEPTED" miner.log); REJ=$(grep -a -c ":user\] share REJECTED" miner.log)
if [ "$ACC" -gt 0 ]; then
  check '[ "${REJ:-1}" -eq 0 ]' "no rejected shares ($REJ)"
  check '[ $(( SENT - ACC )) -le 3 ] && [ $(( SENT - ACC )) -ge -3 ]' "every share the pools received was answered: pools received $SENT, miner counted $ACC accepted"
else
  bad "no accepted user share in the miner log"
fi
check '! grep -a -i "CPU CHECK\|MISMATCH\|panick" miner.log' "no CPU-check failures or panics in the miner log"
echo
[ $fail -eq 0 ] && echo "RESULT $COIN: PASSED" || echo "RESULT $COIN: FAILED"
exit $fail
