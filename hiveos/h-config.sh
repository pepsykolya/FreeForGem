#!/usr/bin/env bash
# FreeForGem — see LICENSE
# Called by HiveOS with the flight sheet variables; writes the miner arguments to $CUSTOM_CONFIG_FILENAME.
#   Pool URL       (CUSTOM_URL)          host:port or stratum+tcp://host:port (Pearl); cmfd+tls://IP:PORT?pin=HEX (Common Foundry);
#                                        several URLs of one coin, separated by spaces or commas = a failover list (1.7.8): the first
#                                        is the primary, the others are backups (back to the primary after 2 good checks);
#                                        stratum+ssl://HOST:PORT of a NOID pool (NOID; --algo noid in Extra config if the host
#                                        name has no "noid" in it)
#   Wallet/worker  (CUSTOM_TEMPLATE)     %WAL%.%WORKER_NAME%  (or just %WAL%)
# The coin follows the pool URL (cmfd+tls:// = CMFD, a NOID pool = NOID, a Proxima pool or http://NODE = PROX,
# neb.suprnova.cc / neb.ariabrain.com / work:// = NEB; crypto-connection.net, ting://GATE:PORT or node://127.0.0.1:18440 =
# Ting / DLR); --algo pearl|cmfd|noid|prox|neb|ting in Extra config forces it.
#   NEB (1.8.0): Pool URL stratum+tcp://neb.suprnova.cc:7804, Wallet template %WAL%.%WORKER_NAME% (neb1...), Pass x
#   (an ffgn gate / Aria: the Pass field is the token, sent to the FIRST pool only, owner's rule 5).
#   Ting (1.8.0): Wallet template %WAL%.%WORKER_NAME% (WAL = the Ting account, 64 hex digits); Pass = x for a pool, the rig's
#   password for an ffgt-gate (Pool URL ting://HOST:PORT, or HOST:PORT with --algo ting in Extra config);
#   node://127.0.0.1:18440 = solo on a Ting node on this rig (Extra config: --node-dir DIR).
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
# 1.8.0 split (different coins on different cards, Extra config --split ...): the flight sheet belongs to ONE coin, so its
# Pool URL / Wallet / Pass (and a solo node) are tagged with that coin and reach only its group; the other coins' pools and
# wallets come tagged from Extra config (--pool pearl=... --wallet pearl=...)
if [[ " $extra " == *" --split "* || " $extra " == *" --split="* ]]; then
  mc=pearl
  if [[ $url == http://* || $url == *prox* || $url == innov* || " $extra " == *" --algo prox "* ]]; then mc=prox
  elif [[ $url == node://* || $url == *crypto-connection* || $url == ting://* || " $extra " == *" --algo ting "* ]]; then mc=ting
  elif [[ $url == *neb.* || $url == work://* || $url == aria://* || $url == *ariabrain* || " $extra " == *" --algo neb "* ]]; then mc=neb
  elif [[ $url == cmfd+tls://* || " $extra " == *" --algo cmfd "* ]]; then mc=cmfd
  elif [[ $url == *noid* || " $extra " == *" --algo noid "* ]]; then mc=noid
  fi
  args=
  if [[ $url == http://* ]]; then
    args="--solo prox=$url --token prox=${CUSTOM_PASS:-} --seeker-name prox=$worker"
  elif [[ $url == node://* ]]; then
    args="--node ting=${url#node://}"
  else
    for u in $urls; do args="$args --pool $mc=$u"; done
    args="${args# } --wallet $mc=$wallet --worker $worker"
    [[ -n ${CUSTOM_PASS:-} && ${CUSTOM_PASS:-} != x ]] && args="$args --password $mc=$CUSTOM_PASS"
  fi
  echo "$args $extra" > "$CUSTOM_CONFIG_FILENAME"
  exit 0
fi
if [[ $url == http://* ]]; then
  echo "--algo prox --solo $url --token ${CUSTOM_PASS:-} --seeker-name $worker $extra" > "$CUSTOM_CONFIG_FILENAME"
  exit 0
fi
# Ting / DLR (1.8.0): solo on a node of this rig, or a pool / ffgt-gate with the Pass field passed through
if [[ $url == node://* ]]; then
  echo "--algo ting --node ${url#node://} $extra" > "$CUSTOM_CONFIG_FILENAME"
  exit 0
fi
if [[ $url == *crypto-connection* || $url == ting://* || " $extra " == *" --algo ting "* || " $extra " == *" --algo dlr "* ]]; then
  [[ " $extra " == *" --algo "* ]] || extra="--algo ting $extra"
  echo "$poolargs --wallet $wallet --worker $worker --password ${CUSTOM_PASS:-x} $extra" > "$CUSTOM_CONFIG_FILENAME"
  exit 0
fi
pass=
if [[ $url == *prox* || $url == innov* || " $extra " == *" --algo prox "* || " $extra " == *" --algo proxima "* ]]; then
  [[ -n ${CUSTOM_PASS:-} ]] && pass=" --password $CUSTOM_PASS"
fi
# NEB (1.8.0): the Pass field goes to the miner (a gate token / Aria password; "x" changes nothing); the miner sends it to the
# first pool only
if [[ $url == *neb.* || $url == work://* || $url == aria://* || $url == *ariabrain* || " $extra " == *" --algo neb "* || " $extra " == *" --coin neb "* || " $extra " == *" --algo nebhash "* ]]; then
  [[ -n ${CUSTOM_PASS:-} && ${CUSTOM_PASS:-} != x ]] && pass=" --password $CUSTOM_PASS"
fi
echo "$poolargs --wallet $wallet --worker $worker$pass $extra" > "$CUSTOM_CONFIG_FILENAME"
