#!/usr/bin/env bash
# FreeForGem — see LICENSE
# Called by HiveOS with the flight sheet variables; writes the miner arguments to $CUSTOM_CONFIG_FILENAME.
#   Pool URL       (CUSTOM_URL)          host:port or stratum+tcp://host:port (Pearl); cmfd+tls://IP:PORT?pin=HEX (Common Foundry);
#                                        several URLs of one coin, separated by spaces or commas = a failover list (1.7.8): the first
#                                        is the primary, the others are backups (back to the primary after 2 good checks);
#                                        stratum+ssl://HOST:PORT of a NOID pool (NOID; --algo noid in Extra config if the host
#                                        name has no "noid" in it)
#   Wallet/worker  (CUSTOM_TEMPLATE)     %WAL%.%WORKER_NAME%  (or just %WAL%)
# The coin follows the pool URL (cmfd+tls:// = CMFD, a NOID pool = NOID, a Proxima pool or http://NODE = PROX);
# --algo pearl|cmfd|noid|prox in Extra config forces it.
#   Extra config   (CUSTOM_USER_CONFIG)  extra arguments, e.g. --devices 0,1
cd "$(dirname "$0")"
. ./h-manifest.conf

urls=${CUSTOM_URL//,/ }                    # a list: primary first, then backups (1.7.8)
url=${urls%% *}                            # the first URL (coin detection, solo)
poolargs=
for u in $urls; do poolargs="$poolargs --pool $u"; done
poolargs=${poolargs# }
[[ -z $poolargs ]] && poolargs="--pool $url"
tpl=${CUSTOM_TEMPLATE:-}
wallet=${tpl%%.*}
worker=${WORKER_NAME:-rig}
[[ $tpl == *.* ]] && worker=${tpl#*.}

[[ -z $url ]] && echo "freeforgem: Pool URL is empty in the flight sheet" >&2
[[ -z $wallet ]] && echo "freeforgem: Wallet template is empty in the flight sheet" >&2

# PROX (Proxima): the flight sheet's Pass field is the pool secret (proximamine; InnovLab innov+tls://: x); a Pool URL
# http://NODE:PORT mines solo through `proxi node mine` (the Pass field = the node's mine.seeker.token)
extra=${CUSTOM_USER_CONFIG:-}
if [[ $url == http://* ]]; then
  echo "--algo prox --solo $url --token ${CUSTOM_PASS:-} --seeker-name $worker $extra" > "$CUSTOM_CONFIG_FILENAME"
  exit 0
fi
pass=
if [[ $url == *prox* || $url == innov* || " $extra " == *" --algo prox "* || " $extra " == *" --algo proxima "* ]]; then
  [[ -n ${CUSTOM_PASS:-} ]] && pass=" --password $CUSTOM_PASS"
fi
echo "$poolargs --wallet $wallet --worker $worker$pass $extra" > "$CUSTOM_CONFIG_FILENAME"
