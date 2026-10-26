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
./freeforgem $(< "$CUSTOM_CONFIG_FILENAME") 2>&1 | tee -i --append "${CUSTOM_LOG_BASENAME}.log"
