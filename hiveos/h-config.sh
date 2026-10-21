#!/usr/bin/env bash
# FreeForGemPear — see LICENSE
# Called by HiveOS with the flight sheet variables; writes the miner arguments to $CUSTOM_CONFIG_FILENAME.
#   Pool URL       (CUSTOM_URL)          host:port or stratum+tcp://host:port (Pearl); cmfd+tls://IP:PORT?pin=HEX (Common Foundry)
#   Wallet/worker  (CUSTOM_TEMPLATE)     %WAL%.%WORKER_NAME%  (or just %WAL%)
# The coin follows the pool URL (cmfd+tls:// = CMFD); --algo pearl|cmfd in Extra config forces it.
#   Extra config   (CUSTOM_USER_CONFIG)  extra arguments, e.g. --devices 0,1
cd "$(dirname "$0")"
. ./h-manifest.conf

url=${CUSTOM_URL%% *}                      # first URL only; the miner drops stratum+tcp:// and refuses SSL with a clear error
tpl=${CUSTOM_TEMPLATE:-}
wallet=${tpl%%.*}
worker=${WORKER_NAME:-rig}
[[ $tpl == *.* ]] && worker=${tpl#*.}

[[ -z $url ]] && echo "freeforgempear: Pool URL is empty in the flight sheet" >&2
[[ -z $wallet ]] && echo "freeforgempear: Wallet template is empty in the flight sheet" >&2

echo "--pool $url --wallet $wallet --worker $worker ${CUSTOM_USER_CONFIG:-}" > "$CUSTOM_CONFIG_FILENAME"
