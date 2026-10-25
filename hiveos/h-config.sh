#!/usr/bin/env bash
# FreeForGem — see LICENSE
# Called by HiveOS with the flight sheet variables; writes the miner arguments to $CUSTOM_CONFIG_FILENAME.
#   Pool URL       (CUSTOM_URL)          host:port or stratum+tcp://host:port (Pearl); cmfd+tls://IP:PORT?pin=HEX (Common Foundry);
#                                        stratum+ssl://HOST:PORT of a NOID pool (NOID; --algo noid in Extra config if the host
#                                        name has no "noid" in it)
#   Wallet/worker  (CUSTOM_TEMPLATE)     %WAL%.%WORKER_NAME%  (or just %WAL%)
# The coin follows the pool URL (cmfd+tls:// = CMFD, a NOID pool = NOID); --algo pearl|cmfd|noid in Extra config forces it.
#   Extra config   (CUSTOM_USER_CONFIG)  extra arguments, e.g. --devices 0,1
cd "$(dirname "$0")"
. ./h-manifest.conf

url=${CUSTOM_URL%% *}                      # first URL only
tpl=${CUSTOM_TEMPLATE:-}
wallet=${tpl%%.*}
worker=${WORKER_NAME:-rig}
[[ $tpl == *.* ]] && worker=${tpl#*.}

[[ -z $url ]] && echo "freeforgem: Pool URL is empty in the flight sheet" >&2
[[ -z $wallet ]] && echo "freeforgem: Wallet template is empty in the flight sheet" >&2

echo "--pool $url --wallet $wallet --worker $worker ${CUSTOM_USER_CONFIG:-}" > "$CUSTOM_CONFIG_FILENAME"
