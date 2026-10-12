#!/usr/bin/env bash
# FreeForGemPear — see LICENSE
cd "$(dirname "$0")"
. ./h-manifest.conf
[[ -f $CUSTOM_CONFIG_FILENAME ]] || { echo "No config file $CUSTOM_CONFIG_FILENAME"; exit 1; }
mkdir -p "$(dirname "$CUSTOM_LOG_BASENAME")"
# GPU numbering in PCI bus order, the same as nvidia-smi and the HiveOS dashboard
export CUDA_DEVICE_ORDER=PCI_BUS_ID
./freeforgempear $(< "$CUSTOM_CONFIG_FILENAME") 2>&1 | tee --append "${CUSTOM_LOG_BASENAME}.log"
