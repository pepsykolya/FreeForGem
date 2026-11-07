#!/usr/bin/env bash
# FreeForGem — see LICENSE
cd "$(dirname "$0")"
. ./h-manifest.conf
[[ -f $CUSTOM_CONFIG_FILENAME ]] || { echo "No config file $CUSTOM_CONFIG_FILENAME"; exit 1; }
mkdir -p "$(dirname "$CUSTOM_LOG_BASENAME")"
# --auto-uv: results and the crash-recovery file live in /hive-config (survives miner updates and reboots), see HIVEOS.md
[[ -d /hive-config ]] && export FFGP_STATE_DIR=${FFGP_STATE_DIR:-/hive-config/freeforgem}
# GPU numbering in PCI bus order, the same as nvidia-smi and the HiveOS dashboard
export CUDA_DEVICE_ORDER=PCI_BUS_ID
# colours in the miner log (the HiveOS miner screen shows them through tee; h-stats.sh strips them); NO_COLOR=1 turns them off
[[ -z ${NO_COLOR:-} ]] && export FFG_COLOR=${FFG_COLOR:-1}
# no filename globbing of the arguments: a CMFD pool URL contains '?' (cmfd+tls://IP:PORT?pin=...)
set -f
# A saved uv.json of FreeForGemPear (/hive-config/freeforgempear) is imported on the first start (see HIVEOS.md)
# CMFD: the 6.4 GB model is shared with FreeForgeMiner in /hive/miners/custom/cmfd-model (downloaded once per rig)
# tee -i: `miner stop` sends SIGINT to the whole pipeline; tee must outlive the miner so that its shutdown (the owner's
# offsets/clocks put back on every GPU) is logged and never writes into a closed pipe
CFG="$(< "$CUSTOM_CONFIG_FILENAME")"
# Optional, off by default: install the unlocked NVIDIA driver for CMP cards before mining. Turn it on with the word
# cmp-unlock-driver (or --cmp-unlock-driver) in the flight sheet field "Extra config arguments". The token is NOT passed to
# the miner (it would stop with "unknown option"). `freeforgem --cmp-driver` is idempotent: it replaces the NVIDIA kernel
# driver and reboots the rig only if the unlocked build is not in place yet; on a rig that has it, without a CMP card or
# without root it changes nothing and mining goes on with the current driver. See MANUAL.md section 20, HIVEOS.md.
case " $CFG " in
  *" cmp-unlock-driver "*|*" --cmp-unlock-driver "*)
    CFG=" $CFG "
    for _ in 1 2; do CFG="${CFG// --cmp-unlock-driver / }"; CFG="${CFG// cmp-unlock-driver / }"; done   # twice: a repeated token shares its space
    echo "freeforgem: cmp-unlock-driver requested - checking/installing the CMP unlock driver"
    ./freeforgem --cmp-driver 2>&1 | tee -i --append "${CUSTOM_LOG_BASENAME}.log"
    rc=${PIPESTATUS[0]}
    [[ $rc == 0 ]] || echo "freeforgem: --cmp-driver returned $rc - keeping the current driver, mining continues"
    ;;
esac
./freeforgem $CFG 2>&1 | tee -i --append "${CUSTOM_LOG_BASENAME}.log"
