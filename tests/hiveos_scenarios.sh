#!/bin/bash
# FreeForGem — see LICENSE
# Release gate (1.7.2): HiveOS-like scenarios on ONE real NVIDIA GPU, with a fake HiveOS OC profile (nvidia-oc.conf).
#
#   CUDA_VISIBLE_DEVICES=<n> FFG_PKG=<unpacked package dir> tests/hiveos_scenarios.sh STEP...   (or: all)
#   (FreeForGem or FreeForGemPear package; NOID steps only with FreeForGem)
#
# Needs: sudo (NVML writes), nvidia-smi, python3, network to the pools below.  Touches only the GPU of CUDA_VISIBLE_DEVICES
# (offset, locks, power limit are put back at the end: the owner's offset and PL as found, locks reset).  Kills only its own
# miner processes.  Each step prints the log lines that prove it; judge them against the "expect" comment of the step.
#
# Steps (scenario numbers of the 1.7.2 release gate):
#   s7a s7b s7c s7d  (7) lock switching: profile per lock, reuse, --uv-reset only the current lock   [run first: later steps use its profiles]
#   s1               (1) a foreign miner left lock 1400 and an offset above the edge, then FreeForGem starts
#   s2a s2b          (2) HiveOS OC apply at start / during mining (offset and lock writes)
#   s3a s3b          (3) SIGINT then SIGKILL during the start check / during the exit restore
#   s3c              Ctrl+C to the whole group with a plain `tee` (dies first): the restore must still complete (no SIGPIPE death)
#   s4               (4) the current offset is above the verified edge (Rig01 1.7.1 restart loop)
#   s5a s5b s5c s5d  (5) GPU fault: attribution + own restart + exclusion; lowered kept offset; crashed tuned offset; one of two workers
#   s5e              no restart loop: a GPU that crashes at every offset ends lowered step by step, then waits left out (no exit)
#   s2c              a changed starting offset with a verified profile: quick check + apply, no tuning
#   s4b              a verified offset that fails its quick check: steps down from it, no tuning from scratch
#   s6a s6b          (6) uv.json migration from 1.6.x / 1.7.1 files (entries of another lock)
#   s6c              a migrated result below the owner's offset FAILS its quick check -> after a restart: paused at the owner's
#                    offset, never applied as "verified" (Rig01 1.7.2 soak)
#   p5 p6 p7         clock probe without --lock-cclock (profile found / tuning while mining), HiveOS owner offset
#   t1 t2            beta3: default start by card / lock, progress tags + summary + RESULT, colours + h-stats.sh (t2: NOID)
#   n1 n2 n3 n4      NOID: fault, profiles 1400 / 1900 / 1400            c1 c2   CMFD: migration + clock probe, --lock-cclock 1500
set -u
[ -n "${CUDA_VISIBLE_DEVICES:-}" ] || { echo "set CUDA_VISIBLE_DEVICES to the ONE GPU to test"; exit 2; }
T=${FFG_TEST_DIR:-$HOME/ffg-scenarios}; L=$T/logs; mkdir -p $L
P=${FFG_PKG:?set FFG_PKG to the unpacked package directory}
BIN=${FFG_BIN:-$( [ -x $P/freeforgem ] && echo freeforgem || echo freeforgempear )}   # FreeForGem or FreeForGemPear
WAL=${FFG_WALLET:-prl1phsns9p42mdt5ft2zv90xp34yk7tckz6c7kv9ewty2ct4x8gmgqssy9c7wa}
POOL=${FFG_POOL:-pearl-ru.luckypool.io:3360}
NW=${FFG_NOID_WALLET:-o1qxh67m5rugg8t43gcn47lp775thsq4padlacn2jv9l3d7lmv7cmq4gmhdc}
NP=${FFG_NOID_POOL:-stratum+ssl://eu2.innovlab.cc:19601}
CP=${FFG_CMFD_POOL:-cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9}
CW=${FFG_CMFD_WALLET:-cmfd1xp7lr5dvjjcztsnk3m6g4j0888tmwj5fyzjh6vun6gwsyhyqqhuqyv6nnf}
CMODEL=${FFG_CMFD_MODEL:-$HOME/pearl-dev/cmfd-model}
CSTATE=${FFG_CMFD_OLD_STATE:-$HOME/pearl-dev/cmfdnvuv/state/uv.json}   # a 1.7.1 uv.json with a CMFD result (c1)
UUID=$(nvidia-smi --query-gpu=uuid --format=csv,noheader -i $CUDA_VISIBLE_DEVICES)
IDX=$(nvidia-smi --query-gpu=index --format=csv,noheader -i $UUID)
NVOFF="import ctypes,sys
n=ctypes.CDLL('libnvidia-ml.so.1'); assert n.nvmlInit_v2()==0
h=ctypes.c_void_p(); assert n.nvmlDeviceGetHandleByUUID(sys.argv[1].encode(),ctypes.byref(h))==0
if len(sys.argv)>2: assert n.nvmlDeviceSetGpcClkVfOffset(h,ctypes.c_int(int(sys.argv[2])))==0
o=ctypes.c_int(); n.nvmlDeviceGetGpcClkVfOffset(h,ctypes.byref(o)); print(o.value)"
off() { python3 -c "$NVOFF" $UUID; }
setoff() { sudo python3 -c "$NVOFF" $UUID $1 >/dev/null; }
lock() { sudo nvidia-smi -i $UUID -lgc $1,$1 >/dev/null; }
for i in $(seq 1 60); do n=$(nvidia-smi --query-compute-apps=gpu_uuid --format=csv,noheader | grep -c $UUID); [ $n = 0 ] && break; echo "waiting: $n process(es) on the GPU"; sleep 5; done
[ $n = 0 ] || { echo "the GPU is still busy, abort"; exit 1; }
ORIG=$(off); PL0=$(nvidia-smi -i $UUID --query-gpu=power.limit --format=csv,noheader,nounits | cut -d. -f1)
finish() {
  sudo nvidia-smi -i $UUID -rgc >/dev/null; sudo nvidia-smi -i $UUID -rmc >/dev/null; setoff $ORIG
  sudo nvidia-smi -i $UUID -pl $PL0 >/dev/null
  echo "== end $(date -Is): offset $(off) (owner $ORIG), PL $(nvidia-smi -i $UUID --query-gpu=power.limit --format=csv,noheader) (owner $PL0), locks reset"
}
trap finish EXIT
echo "== start $(date -Is): $UUID (NVML index $IDX), owner offset $ORIG, PL $PL0; binary $(sha256sum $P/$BIN | cut -c1-16)"
# fake HiveOS OC profiles: the list item of this GPU's NVML index carries the value, the others something else
hc() { # FILE CLOCK LCLOCK
  local c=() l=() i; for i in $(seq 0 $IDX); do c+=(0); l+=(1111); done; c[$IDX]=$2; l[$IDX]=$3
  printf 'CLOCK="%s"\nLCLOCK="%s"\nMEM=""\nLMEM="810"\nPLIMIT=""\nFAN=""\n' "${c[*]}" "${l[*]}" > $1
}
HC=$T/oc-own.conf; hc $HC $ORIG 0          # HiveOS profile = the owner's offset, no lock
HC30=$T/oc-30.conf; hc $HC30 30 0
HC14=$T/oc-1400.conf; hc $HC14 $ORIG 1400
E="CUDA_DEVICE_ORDER=PCI_BUS_ID CUDA_VISIBLE_DEVICES=$CUDA_VISIBLE_DEVICES"
run() { # TAG MAXSECS ENV... -- ARGS...
  local tag=$1 max=$2; shift 2
  local envs=(); while [ "$1" != "--" ]; do envs+=("$1"); shift; done; shift
  echo "-- $tag: ${envs[*]} freeforgem $*"
  ( cd $P && sudo env $E "${envs[@]}" timeout --foreground -s INT $max ./$BIN "$@" ) > $L/$tag.log 2>&1
  echo "   exit $?; offset after $(off)"
}
bg() { # TAG ENV... -- ARGS...   (own process group, like HiveOS screen)
  local tag=$1; shift
  local envs=(); while [ "$1" != "--" ]; do envs+=("$1"); shift; done; shift
  echo "-- $tag (bg): ${envs[*]} freeforgem $*"
  ( cd $P && exec setsid sudo env $E "${envs[@]}" ./$BIN "$@" ) > $L/$tag.log 2>&1 &
  BGPID=$!
}
waitlog() { for i in $(seq 1 $3); do grep -aqE "$2" $L/$1.log 2>/dev/null && return 0; sleep 1; done; echo "   (timeout waiting for '$2')"; return 1; }
grp() { sudo kill -$1 -- -$BGPID 2>/dev/null; }
stopbg() { grp INT; sleep 1; grp HUP; wait; echo "   stopped; offset after $(off)"; }
PEARL="--pool $POOL --wallet $WAL --worker gate"
NOID="--algo noid --pool $NP --wallet $NW --worker gate"
SHORT="--uv-min 150 --uv-max 210 --uv-step 30 --uv-warmup 20 --uv-step-secs 15 --uv-confirm-secs 30"
NSHORT="--uv-min 150 --uv-max 210 --uv-step 30 --uv-step-secs 15 --uv-confirm-secs 30"
show() { grep -aE "$2" $L/$1.log | grep -av "CUDA device" | cut -c1-240 | sed 's/^/   /'; }
stats() { grep -aE "^\[ *[0-9]+s\]" $L/$1.log | cut -c1-110 | sed 's/^/   /' | head -${2:-12}; }
ent() { sudo python3 -c "
import json;d=json.load(open('$1'))
for u,ps in d.get('profiles',{}).items():
  for k,e in ps.items(): print('   profile',k,{x:e.get(x) for x in ('offset','clk_mhz','core_lock','mem_lock','unlocked','migrated','edge','disabled_until')})
for u,ps in d.get('noid',{}).items():
  for k,e in ps.items(): print('   noid profile',k,{x:e.get(x) for x in ('offset','core_lock','clk_mhz','edge')})
for u,ps in d.get('cmfd_profiles',{}).items():
  for k,e in ps.items(): print('   cmfd profile',k,{x:e.get(x) for x in ('offset','core_lock','clk_before','migrated')})
print('   old sections left:', [k for k in ('gpus','cmfd') if d.get(k)], '; trying:', d.get('trying'))
" 2>/dev/null; }
faults() { echo "   gpu-faults.json: $(sudo cat $1/gpu-faults.json 2>/dev/null | tr -d '\n ' | cut -c1-300)"; }
P7=$T/p   # the profiles of s7 (1400 and 1900)
steps="$*"; [ "$steps" = all ] && steps="s7a s7b s7c s7d s1 s2a s2b s2c s3a s3b s3c s4 s4b s5a s5b s5c s5d s5e s6a s6b s6c p5 p6 p7 n1 n2 n3 n4 c1 c2"
[ "$BIN" = freeforgem ] || steps=$(echo " $steps " | sed "s/ n[1-4]//g")
for step in $steps; do
  echo "=== $step $(date +%T)"
  sudo nvidia-smi -i $UUID -rgc >/dev/null; setoff $ORIG
  case $step in
    s7a) # expect: tuning at 1400 starts after mining started (no wait before the [Ns] lines), profile 1400 saved
      sudo rm -rf $P7; sudo mkdir -p $P7
      run s7a 900 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $P7 --auto-uv $SHORT --minutes 3
      show s7a "auto-uv: GPU|^\[ *[0-9]+s\]" | cut -c1-150; ent $P7/uv.json ;;
    s7b) # expect: "none for the core clock lock 1900" -> tuning; profiles 1400 and 1900 afterwards
      run s7b 900 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1900 --lock-mclock 810 --uv-dir $P7 --auto-uv $SHORT --minutes 3
      show s7b "auto-uv: GPU"; ent $P7/uv.json ;;
    s7c) # expect: "using 1400/mem810", "applied without a start check", no tuning line
      run s7c 300 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $P7 --auto-uv $SHORT --minutes 0.7
      show s7c "auto-uv: GPU"; stats s7c 2 ;;
    s7d) # expect: --uv-reset forgets 1900 only, tunes 1900 again; the 1400 profile unchanged
      run s7d 900 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1900 --lock-mclock 810 --uv-dir $P7 --auto-uv --uv-reset $SHORT --minutes 3
      show s7d "auto-uv: GPU"; ent $P7/uv.json ;;
    s1) # foreign miner left lock 1400 + offset ORIG+150; HiveOS profile = ORIG.  expect: the profile value goes back first
        # ("found, the HiveOS OC profile has"), the clock picks the 1400 profile, exit puts ORIG back
      sudo rm -rf $T/s1; sudo mkdir -p $T/s1; sudo cp $P7/uv.json $T/s1/
      lock 1400; setoff $((ORIG + 150))
      run s1 300 NVIDIA_OC_CONF=$HC -- $PEARL --uv-dir $T/s1 --auto-uv $SHORT --minutes 1
      show s1 "auto-uv: GPU"; stats s1 3 ;;
    s2a) # HiveOS OC apply at the miner start (profile: offset ORIG, lock 1400), written 2 s after the start.
         # expect: the clock probe sees 1410 MHz and uses the 1400 profile
      sudo rm -rf $T/s2; sudo mkdir -p $T/s2; sudo cp $P7/uv.json $T/s2/
      bg s2a NVIDIA_OC_CONF=$HC14 -- $PEARL --uv-dir $T/s2 --auto-uv $SHORT --minutes 1.2
      sleep 2; lock 1400; setoff $ORIG; echo "   t=2 OC apply: lock 1400, offset $ORIG"
      wait; show s2a "auto-uv: GPU"; stats s2a 3 ;;
    s2b) # OC applies while mining at the 1400 profile: t=60 offset -> ORIG (profile value), t=150 lock -> 1900.
         # expect: "reset ... by an OC apply" -> verified offset back after the check / clock probe; the 1900 lock is noticed
         # ("core clock under mining load changed" or the probe) and the 1900 profile is used
      bg s2b NVIDIA_OC_CONF=$HC14 -- $PEARL --uv-dir $T/s2 --auto-uv $SHORT --minutes 5
      sleep 3; lock 1400
      sleep 57; echo "   t=60 offset $(off) -> $ORIG (OC apply)"; setoff $ORIG
      sleep 90; echo "   t=150 offset $(off) -> lock 1900 (OC apply)"; lock 1900
      wait; show s2b "auto-uv: GPU"; stats s2b 30 | tail -4 ;;
    s2c) # the starting offset changed (HiveOS profile now ORIG-75) while the 1400 profile exists.  expect: "quick check passed"
         # for the profile, no "tuning:" line
      sudo rm -rf $T/s2c; sudo mkdir -p $T/s2c; sudo cp $P7/uv.json $T/s2c/; HCm=$T/oc-minus.conf; hc $HCm $((ORIG - 75)) 0
      run s2c 300 NVIDIA_OC_CONF=$HCm -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s2c --auto-uv $SHORT --minutes 1
      show s2c "auto-uv: GPU"; stats s2c 2 ;;
    s3a) # SIGINT then SIGKILL during the start quick check (profile older than --uv-reval-hours).
         # expect: the next start puts ORIG back ("did not exit cleanly"), the interrupted offset is not a crash point after SIGINT
      sudo rm -rf $T/s3; sudo mkdir -p $T/s3; sudo cp $P7/uv.json $T/s3/
      bg s3a NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s3 --auto-uv --uv-reval-hours 0.0001 $SHORT --minutes 2
      for i in $(seq 1 90); do o=$(off); [ "$o" != "$ORIG" ] && break; sleep 1; done
      sleep 4; echo "   check running at $(off): SIGINT, SIGKILL 0.3 s later"; grp INT; sleep 0.3; grp KILL; wait; sleep 2
      echo "   after the kill: offset $(off)"; ent $T/s3/uv.json
      run s3a2 120 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s3 --auto-uv $SHORT --minutes 0.5
      show s3a2 "auto-uv|clocks:" | head -12 ;;
    s3c) # HiveOS `miner stop` = Ctrl+C to the screen group; a plain `tee` (not -i) dies first and the miner's stdout is a dead pipe.
         # expect: the offset back at ORIG, the core lock back to the HiveOS profile (1400 here: clocks near 1410 under load is not
         # measurable idle, so the marker file must be gone), exit code not 141
      sudo rm -rf $T/s3c; sudo mkdir -p $T/s3c; sudo cp $P7/uv.json $T/s3c/
      ( cd $P && exec setsid sudo env $E NVIDIA_OC_CONF=$HC14 bash -c "./$BIN $PEARL --lock-cclock 1900 --lock-mclock 810 --uv-dir $T/s3c --auto-uv $SHORT --minutes 3 2>&1 | tee $L/s3c.log; echo \"pipe status \${PIPESTATUS[*]}\" > $T/s3c/rc" ) > /dev/null 2>&1 &
      BGPID=$!
      waitlog s3c "^\[ *[0-9]+s\]" 120; echo "   mining at $(off): Ctrl+C to the group (tee dies with it)"; grp INT; sleep 1; grp HUP; wait; sleep 3
      echo "   after: offset $(off) (owner $ORIG); clocks-restore marker: $(sudo ls $T/s3c/clocks-restore.json 2>/dev/null || echo gone); uv-restore: $(sudo ls $T/s3c/uv-restore.json 2>/dev/null || echo gone); $(sudo cat $T/s3c/rc 2>/dev/null)"
      pgrep -af "worker gate" | grep -v pgrep | head -2 ;;
    s3b) # SIGINT then SIGKILL during the exit restore.  expect: the next start puts ORIG and the locks back
      bg s3b NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s3 --auto-uv $SHORT --minutes 2
      waitlog s3b "^\[ *[0-9]+s\]" 120; echo "   mining at $(off): SIGINT, SIGKILL 0.05 s later"; grp INT; sleep 0.05; grp KILL; wait; sleep 2
      echo "   after the kill: offset $(off), clocks $(nvidia-smi -i $UUID --query-gpu=clocks.gr --format=csv,noheader)"
      run s3b2 120 NVIDIA_OC_CONF=$HC -- $PEARL --uv-dir $T/s3 --minutes 0.3
      show s3b2 "auto-uv|clocks:" | head -8 ;;
    s4) # the card is at ORIG+150 (above the verified edge), no HiveOS profile (the found offset is the owner's).
        # expect: "verified at these clocks ... is below your offset: mining at" the profile value; never "keeping it"
      sudo rm -rf $T/s4; sudo mkdir -p $T/s4; sudo cp $P7/uv.json $T/s4/
      setoff $((ORIG + 150))
      bg s4 -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s4 --auto-uv $SHORT --minutes 1
      waitlog s4 "^\[ *[0-9]+s\]" 120; echo "   offset while mining: $(off)"; stopbg; show s4 "auto-uv: GPU" ;;
    s4b) # the 1900 profile says +330 (never verified at it): its quick check fails.  expect: "one step down" lines from +330
         # until one passes (saved), no "tuning:" line
      sudo rm -rf $T/s4b; sudo mkdir -p $T/s4b
      sudo python3 -c "
import json;d=json.load(open('$P7/uv.json'))
for u,ps in d['profiles'].items():
  for k,e in ps.items():
    if e.get('core_lock')==1900: e['offset']=330; e['verified_at']=0
json.dump(d,open('$T/s4b/uv.json','w'),indent=1)"
      run s4b 900 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1900 --lock-mclock 810 --uv-dir $T/s4b --auto-uv $SHORT --uv-max 345 --minutes 4
      show s4b "auto-uv: GPU"; ent $T/s4b/uv.json ;;
    s5a) # no auto-uv: injected fault (fake Xid 13) twice.  expect: "FAILED: Xid 13", own restart, then "LEFT OUT", exit
      sudo rm -rf $T/s5a; sudo mkdir -p $T/s5a
      run s5a 300 NVIDIA_OC_CONF=$HC FFGP_STATE_DIR=$T/s5a FFG_TEST_FAULT=0:15 -- $PEARL --minutes 2
      show s5a "faults:|worker stopped|restarting|Error|FreeForGem 1"; faults $T/s5a ;;
    s5b) # auto-uv keeps the owner's offset (paused profile); faults there 2 starts in a row.  expect: "CRASHED ... running it at"
      sudo rm -rf $T/s5b; sudo mkdir -p $T/s5b
      sudo python3 -c "
import json,time;d=json.load(open('$P7/uv.json'))
for u,ps in d['profiles'].items():
  for k,e in ps.items(): e['disabled_until']=int(time.time())+20*3600
json.dump(d,open('$T/s5b/uv.json','w'),indent=1)"
      run s5b 400 NVIDIA_OC_CONF=$HC FFG_TEST_FAULT=0:15 FFG_TEST_FAULT_UNTIL=2 -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s5b --auto-uv $SHORT --minutes 0.8
      show s5b "faults:|auto-uv: GPU|worker stopped|restarting|Error|FreeForGem 1"; faults $T/s5b; stats s5b 2 ;;
    s5c) # a fault at the tuned (reused) offset.  expect: "crashed while mining at ...: one step below it ... quick check (no tuning
         # from scratch)", no "tuning:" line
      sudo rm -rf $T/s5c; sudo mkdir -p $T/s5c; sudo cp $P7/uv.json $T/s5c/
      run s5c 900 NVIDIA_OC_CONF=$HC FFG_TEST_FAULT=0:15 FFG_TEST_FAULT_UNTIL=1 -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s5c --auto-uv $SHORT --minutes 2.5
      show s5c "faults:|auto-uv: GPU|worker stopped|restarting|Error|FreeForGem 1"; faults $T/s5c; ent $T/s5c/uv.json ;;
    s5d) # two workers (--devices 0,0: a simulated second device on the same GPU), one of them faults.
         # expect: "1 GPU worker(s) keep mining", hashrate lines go on with one worker DEAD, no restart
      sudo rm -rf $T/s5d; sudo mkdir -p $T/s5d
      run s5d 200 NVIDIA_OC_CONF=$HC FFGP_STATE_DIR=$T/s5d FFG_TEST_FAULT=0:20 FFG_TEST_DUP_DEVICES=1 -- $PEARL --devices 0,0 --minutes 1.2
      show s5d "faults:|worker stopped|restarting|Error"; stats s5d 12 | tail -4 ;;
    s5e) # a GPU that faults at every offset (auto-uv, kept offset): lowered 2 crash starts at a time down to 0, then left out;
         # the single-GPU miner waits instead of exiting (no restart loop).  expect: "CRASHED ... running it at" several times,
         # "LEFT OUT", "waiting"
      sudo rm -rf $T/s5e; sudo mkdir -p $T/s5e
      sudo python3 -c "
import json,time;d=json.load(open('$P7/uv.json'))
for u,ps in d['profiles'].items():
  for k,e in ps.items(): e['disabled_until']=int(time.time())+20*3600
json.dump(d,open('$T/s5e/uv.json','w'),indent=1)"
      bg s5e NVIDIA_OC_CONF=$HC FFG_TEST_FAULT=0:10 -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/s5e --auto-uv $SHORT --minutes 5
      waitlog s5e "waiting [0-9]+ min" 600; sleep 3; echo "   offset while waiting: $(off)"; stopbg
      show s5e "faults:|CRASHED|LEFT OUT|runs at the lowered|restarting" | cut -c1-200; faults $T/s5e ;;
    s6a) # a 1.6.x uv.json (one entry, no lock, no cap; clk 1410) and --lock-cclock 1900.  expect: migrated as ~1410, not used
         # for 1900 ("none for the core clock lock 1900": tuning)
      sudo rm -rf $T/s6; sudo mkdir -p $T/s6
      sudo python3 -c "
import json;d={'version':1,'gpus':{'$UUID':{'name':'x','bus':'x','driver':'$(nvidia-smi --query-gpu=driver_version --format=csv,noheader -i $UUID)','kernel':'k','orig_offset':$ORIG,'offset':495,'clk_mhz':1410,'watts_before':100,'watts_after':80,'tested_at':1,'verified_at':1,'disabled_until':0,'refs':[],'m':0}}}
json.dump(d,open('$T/s6/uv.json','w'))"
      bg s6a NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1900 --lock-mclock 810 --uv-dir $T/s6 --auto-uv $SHORT --minutes 1
      waitlog s6a "tuning:" 90; sleep 2; stopbg; show s6a "auto-uv"; ent $T/s6/uv.json ;;
    s6b) # a 1.7.1 uv.json tuned at lock 1500 (clk 1500): fits 1500 only, and only after a check
      [ -f "${FFG_OLD171:-$HOME/ffg171-test/uv/uv.json}" ] || { echo "   (no 1.7.1 uv.json: FFG_OLD171)"; continue; }
      sudo rm -rf $T/s6b; sudo mkdir -p $T/s6b; sudo cp ${FFG_OLD171:-$HOME/ffg171-test/uv/uv.json} $T/s6b/uv.json
      run s6b 300 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1500 --lock-mclock 810 --uv-dir $T/s6b --auto-uv $SHORT --minutes 1.2
      show s6b "auto-uv"; ent $T/s6b/uv.json ;;
    s6c) # the 1.7.1 result (lock 1500, +150 below the owner's +175) with broken reference digests: its quick check fails.
         # expect run 1: "FAILED the quick check ... back to +175, auto-uv paused"; run 2: "paused ... keeping +175", no "applied
         # without a start check", the card mines at +175
      [ -f "${FFG_OLD171:-$HOME/ffg171-test/uv/uv.json}" ] || { echo "   (no 1.7.1 uv.json: FFG_OLD171)"; continue; }
      sudo rm -rf $T/s6c; sudo mkdir -p $T/s6c
      sudo python3 -c "
import json;d=json.load(open('${FFG_OLD171:-$HOME/ffg171-test/uv/uv.json}'))
for e in d['gpus'].values(): e['refs']=['00'*32]*4; e['disabled_until']=0
d.pop('trying',None); json.dump(d,open('$T/s6c/uv.json','w'),indent=1)"
      run s6c1 300 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1500 --lock-mclock 810 --uv-dir $T/s6c --auto-uv $SHORT --minutes 1.5
      show s6c1 "auto-uv: GPU"; ent $T/s6c/uv.json
      bg s6c2 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1500 --lock-mclock 810 --uv-dir $T/s6c --auto-uv $SHORT --minutes 1
      waitlog s6c2 "^\[ *[0-9]+s\]" 120; sleep 15; echo "   offset while mining after the restart: $(off) (owner $ORIG)"; stopbg
      show s6c2 "auto-uv: GPU"; echo "   'applied without a start check' lines: $(grep -ac 'applied without a start check' $L/s6c2.log)" ;;
    p5) # lock 1400 set outside the miner: the clock under mining load picks the 1400 profile
      lock 1400; run p5 300 NVIDIA_OC_CONF=$HC -- $PEARL --uv-dir $P7 --auto-uv $SHORT --minutes 1
      show p5 "auto-uv: GPU"; stats p5 3 ;;
    p6) # lock 1550 set outside the miner: no profile -> mining at the owner's offset first, then tuned in the background
      sudo rm -rf $T/p6; sudo mkdir -p $T/p6; sudo cp $P7/uv.json $T/p6/
      lock 1550; run p6 900 NVIDIA_OC_CONF=$HC -- $PEARL --uv-dir $T/p6 --auto-uv $SHORT --minutes 3.5
      show p6 "auto-uv: GPU|^\[ *[0-9]+s\]" | cut -c1-150; ent $T/p6/uv.json ;;
    p7) # HiveOS profile Core Clock +30: the owner's offset is +30, not ORIG; put back on exit
      sudo rm -rf $T/p7; sudo mkdir -p $T/p7; sudo cp $P7/uv.json $T/p7/
      run p7 300 NVIDIA_OC_CONF=$HC30 -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/p7 --auto-uv $SHORT --minutes 0.5
      show p7 "auto-uv: GPU" ;;
    n1) # NOID, no auto-uv: injected fault -> attributed, own restart (2nd run without the fault)
      sudo rm -rf $T/n; sudo mkdir -p $T/n
      run n1 300 NVIDIA_OC_CONF=$HC FFGP_STATE_DIR=$T/n FFG_TEST_FAULT=0:15 FFG_TEST_FAULT_UNTIL=1 -- $NOID --minutes 0.6
      show n1 "faults:|worker stopped|restarting|Error|FreeForGem 1|noid offset"; faults $T/n ;;
    n2) # NOID profile at --lock-cclock 1400 (the tuning starts once mining runs)
      sudo rm -rf $T/nu; sudo mkdir -p $T/nu
      run n2 900 NVIDIA_OC_CONF=$HC -- $NOID --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/nu --auto-uv $NSHORT --minutes 4
      show n2 "auto-uv \(noid\)|noid offset" | head -30; ent $T/nu/uv.json ;;
    n3) # NOID at 1900: no profile -> tuning; the 1400 profile stays
      bg n3 NVIDIA_OC_CONF=$HC -- $NOID --lock-cclock 1900 --lock-mclock 810 --uv-dir $T/nu --auto-uv $NSHORT --minutes 3
      waitlog n3 "tuning the NOID" 120; sleep 3; stopbg; show n3 "auto-uv \(noid\)"; ent $T/nu/uv.json ;;
    n4) # NOID back at 1400: its profile at once
      run n4 300 NVIDIA_OC_CONF=$HC -- $NOID --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/nu --auto-uv $NSHORT --minutes 0.6
      show n4 "auto-uv \(noid\)|noid offset" ;;
    c1) # CMFD: a 1.7.1 uv.json (no lock) migrates; the clock at the base picks it; quick check; mining
      [ -f "$CSTATE" ] && [ -d "$CMODEL" ] || { echo "   (no CMFD model / old state: FFG_CMFD_MODEL, FFG_CMFD_OLD_STATE)"; continue; }
      sudo rm -rf $T/cst; sudo mkdir -p $T/cmfd-work $T/cst; sudo cp $CSTATE $T/cst/uv.json
      run c1 1200 FFGP_CMFD_WORK=$T/cmfd-work NVIDIA_OC_CONF=$HC -- --algo cmfd --pool "$CP" --wallet $CW --worker gate --minutes 3 --cmfd-model $CMODEL --cmfd-skip-prepare --uv-dir $T/cst --auto-uv
      show c1 "auto-uv \(cmfd\)|cmfd offset|WARN" | tail -8; stats c1 3; ent $T/cst/uv.json; echo "   cmfd-v4-replay left: $(pgrep -c cmfd-v4-replay)" ;;
    c2) # CMFD with --lock-cclock 1500: no profile for it -> tuning starts; the other profile stays
      [ -d "$T/cst" ] || { echo "   (run c1 first)"; continue; }
      bg c2 FFGP_CMFD_WORK=$T/cmfd-work NVIDIA_OC_CONF=$HC -- --algo cmfd --pool "$CP" --wallet $CW --worker gate --minutes 3 --cmfd-model $CMODEL --cmfd-skip-prepare --uv-dir $T/cst --auto-uv --lock-cclock 1500
      waitlog c2 "none for|tuning" 300; sleep 5; stopbg; sleep 3
      show c2 "auto-uv \(cmfd\)" | tail -6; ent $T/cst/uv.json; echo "   cmfd-v4-replay left: $(pgrep -c cmfd-v4-replay)" ;;
    t1) # beta3: default start of an RTX 3070 by the lock (1400: +270, no --uv-min), per-GPU tag in the stats line, summary line,
        # RESULT line, colours (FFG_COLOR=1, as h-run.sh) on the other lines only; h-stats.sh parses the coloured log
      sudo rm -rf $T/t1; sudo mkdir -p $T/t1
      run t1 900 FFG_COLOR=1 NVIDIA_OC_CONF=$HC -- $PEARL --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/t1 --auto-uv --uv-step 30 --uv-warmup 20 --uv-step-secs 15 --uv-confirm-secs 30 --minutes 5
      show t1 "auto-uv: GPU|auto-uv: tuning|auto-uv: checking" | head -24; stats t1 8; ent $T/t1/uv.json
      echo "   coloured lines: $(grep -ac $'\x1b\[' $L/t1.log); stats lines with ESC: $(grep -aE '^.{0,8}\[ *[0-9]+s\]' $L/t1.log | grep -ac $'\x1b')"
      H=$T/hive; mkdir -p $H; cp $P/hive/h-stats.sh $H/ 2>/dev/null || cp $P/h-stats.sh $H/
      printf 'CUSTOM_LOG_BASENAME=%s
CUSTOM_VERSION=test
CUSTOM_ALGO=pearl
' $L/t1 > $H/h-manifest.conf
      ( . $H/h-stats.sh; echo "   h-stats: khs=$khs stats=$stats" | cut -c1-240 ) ;;
    t2) # beta3 NOID: default start at 1400 (+105, no --uv-min), base measured once / short, RESULT never worse than the base
      sudo rm -rf $T/t2; sudo mkdir -p $T/t2
      run t2 900 FFG_COLOR=1 NVIDIA_OC_CONF=$HC -- $NOID --lock-cclock 1400 --lock-mclock 810 --uv-dir $T/t2 --auto-uv --uv-step 30 --uv-step-secs 15 --uv-confirm-secs 30 --minutes 5
      show t2 "auto-uv \(noid\)|noid offset|RESULT" | sed 's/\x1b\[[0-9;]*m//g' | head -30; stats t2 4; ent $T/t2/uv.json ;;
    *) echo "   unknown step $step" ;;
  esac
done
