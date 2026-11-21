#!/bin/bash
# FreeForGem — see LICENSE
# h-stats.sh per coin (1.8.0): feeds one stats line of each coin to hiveos/h-stats.sh and checks algo, units, khs and the
# version column.  Needs bash, jq, awk, grep -P (a HiveOS-like Linux).  No GPU (nvidia-smi missing = zeros for temp/fan/bus).
#   tests/hstats_units_test.sh
set -u
cd "$(dirname "$0")/.."
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
cp hiveos/h-stats.sh "$T/"
printf 'CUSTOM_NAME=freeforgem\nCUSTOM_VERSION=1.8.0\nCUSTOM_ALGO=pearlhash\nCUSTOM_LOG_BASENAME=%s/miner\n' "$T" > "$T/h-manifest.conf"
fail=0
check() { # name line expect_algo expect_khs expect_ver_substring
  printf '%s\n' "$2" > "$T/miner.log"
  # shellcheck disable=SC1091
  ( . "$T/h-stats.sh"; a=$(jq -r .algo <<< "$stats"); v=$(jq -r .ver <<< "$stats")
    if [[ $a == "$3" && $khs == "$4" && $v == *"$5"* ]]; then echo "ok   $1: algo $a khs $khs ver '$v'"
    else echo "FAIL $1: algo $a (want $3) khs $khs (want $4) ver '$v' (want *$5*)"; exit 1; fi ) || fail=1
}
check pearl '[ 270s] GPU0 68.318 | GPU1 68.2 | total 136.5 TH/s, 200 W, 0.683 TH/W (...) | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0' pearlhash 136500000000 '0.683 TH/W'
check pearl-unheld '[ 270s] GPU0 68.318 | total 68.3 TH/s, 100 W, 0.683 TH/W | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0  [LOCK NOT HELD: GPU0]' pearlhash 68300000000 'LOCK NOT HELD GPU0'
check noid '[  60s] GPU1 35.512 (77 W, 0.461 MH/W) | total 101.2 MH/s, 220 W, 0.460 MH/W | hits 3 | user sub 4 acc 4 rej 0 | fee sub 0 acc 0 rej 0' parano1d 101200.0 '0.460 MH/W'
check prox '[  60s] GPU0 3.210 (199 W, 0.0161 GH/W) | total 6.420 GH/s, 398 W, 0.0161 GH/W | hits 2 | user sub 9 acc 9 rej 0 | fee sub 0 acc 0 rej 0' proxima 6420000 '0.0161 GH/W'
check neb '[  60s] GPU0 12.345 (150 W, 0.0823 MA/W)  GPU1 12.345 (150 W, 0.0823 MA/W) | total 24.690 MA/s, 300 W, 0.0823 MA/W | found 3 (bad 0) | user sub 3 acc 3 rej 0 (stale 0) | fee sub 0 acc 0 rej 0' nebhash 24690.0 '0.0823 MA/W'
check neb-avg '[ 600s] GPU0 12.0 (150 W, 0.08 MA/W)  GPU1 13.0 (150 W, 0.087 MA/W) | total 25.0 MA/s, 300 W, 0.083 MA/W | avg 5 min 30.5 MA/s (15.0 15.5) | found 3 (bad 0) | user sub 3 acc 3 rej 0 (stale 0)' nebhash 30500.0 'MA/W'
check neb-solo '[  60s] GPU0 12.345 | total 12.345 MA/s | found 3 (bad 0) | shares 3 | blocks sub 0 acc 0 rej 0' nebhash 12345.0 '1.8.0'
check cmfd '[ 380s] GPU0 21.76 (237 W, 330742 FW/kWh) | total 87.46 FW/s | x | user sub 1 acc 1 rej 0 | fee sub 0 acc 0 rej 0' forgematrix_v4 0.08746 '1.8.0'
[[ $fail == 0 ]] && echo "h-stats: all coins ok" || { echo "h-stats: FAILED"; exit 1; }
