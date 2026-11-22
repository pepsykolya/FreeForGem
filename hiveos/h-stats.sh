#!/usr/bin/env bash
# FreeForGem — see LICENSE
# Sourced by the HiveOS agent: sets $khs (total, kH/s) and $stats (JSON) from the miner's periodic log line
#   [ 270s] GPU0 68.318 | GPU1 68.2 | total 136.5 TH/s (...) | ... | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0     (Pearl)
#   [ 380s] GPU0 21.76 (237 W, 330742 FW/kWh)  GPU1 21.75 (...) | total 87.46 FW/s | ... | user sub 1 acc 1 rej 0 | ...   (CMFD)
#   [  60s] GPU1 35.512 (77 W, 0.461 MH/W)  GPU2 ... | total 101.2 MH/s, 220 W, 0.460 MH/W | hits ... | user sub 4 acc 4 rej 0 | ...  (NOID)
#   [  60s] GPU0 3.210 (199 W, 0.0161 GH/W)  GPU1 ... | total 6.420 GH/s, 398 W, 0.0161 GH/W | hits ... | user sub 9 acc 9 rej 0 (...) | ...  (PROX)
#   [  60s] GPU0 12.345 (150 W, 0.0823 MA/W)  GPU1 ... | total 24.690 MA/s, 300 W, 0.0823 MA/W | found 3 (bad 0) | user sub 3 acc 3 rej 0 (stale 0) | ...  (NEB)
#   [  60s] GPU0 12.345 ... | total 24.690 MA/s | found 3 (bad 0) | shares 3 | blocks sub 0 acc 0 rej 0 | ...      (NEB solo: shares to the node)
# Units per coin (coins.rs): TH/s Pearl, FW/s CMFD, MH/s NOID, GH/s PROX, MA/s NEB (1 MA/s = 10^6 nebhash attempts per second;
# HiveOS gets attempts/s as hashes, khs = MA/s x 1000). A new coin = one more unit below.
#   [  60s] GPU0 5.128 (102 W, 0.0503 GH/W) group 7  GPU1 ... | total 10.256 GH/s, 204 W, 0.0503 GH/W | candidates ... | user sub 9 acc 9 rej 0 ...  (Ting)
_ffgp_dir=$(dirname "${BASH_SOURCE[0]}")
. "$_ffgp_dir/h-manifest.conf"
_log="${CUSTOM_LOG_BASENAME}.log"
# 1.8.0 split (different coins on different cards, --split): every line of a group's miner carries "[group] " in front; the
# last stats line of every group is read and the groups are merged (one HiveOS miner = one algo: the group with most cards)
_tail=$(tail -n 600 "$_log" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -aE ' total [0-9.]+ (TH|FW|MH|GH|MA)/s')
# a group name starts with a letter (neb, pearl2 ...): "[3743s] " of a one-coin run past 1000 s is NOT a group (Rig01 22.11)
_line=$(grep -avE '^\[[A-Za-z][A-Za-z0-9_-]*\] ' <<< "$_tail" | tail -n 1)
mapfile -t _groups < <(grep -aoP '^\[\K[A-Za-z][A-Za-z0-9_-]*(?=\] \[)' <<< "$_tail" | awk 'NF && !s[$0]++')
# groups in the order of --split (the config file h-config wrote): of groups with as many cards, the first one is the main one
if [[ ${#_groups[@]} -gt 1 && -r ${CUSTOM_CONFIG_FILENAME:-} ]]; then
  _spec=$(grep -oP -- '--split[ =]\K.*' "$CUSTOM_CONFIG_FILENAME" 2>/dev/null | head -n 1)
  mapfile -t _groups < <(for g in "${_groups[@]}"; do c=${g%%[0-9]*}; i=$(grep -boP "(^|[ ;,])$c:" <<< "$_spec" | head -n 1 | cut -d: -f1); echo "${i:-9999} ${g}"; done | sort -n -s -k1,1 | cut -d' ' -f2)
fi
_age=$(( $(date +%s) - $(stat -c %Y "$_log" 2>/dev/null || echo 0) ))

# one stats line -> L_ids L_ths (per GPU, in the line's unit) L_total L_mul L_kmul L_fmt L_units L_algo L_up L_acc L_rej L_extra
_ffg_parse() {
  local _line=$1 _avg _avgs _kavg _unheld _uvw _slot _mem _down _zero _i _rb _nt _gl
  mapfile -t L_ids < <(grep -oP 'GPU\K[0-9]+(?= [0-9.]+)' <<< "$_line")
  mapfile -t L_ths < <(grep -oP 'GPU[0-9]+ \K[0-9.]+' <<< "$_line")
  L_total=$(grep -oP 'total \K[0-9.]+' <<< "$_line")
  # Pearl: TH/s -> per-GPU khs x1e9, khs x1e9; CMFD (as FreeForgeMiner reports it): FW/s as hs 1:1, khs = FW/s / 1000
  # NOID: MH/s -> per-GPU khs x1000, khs = MH/s x 1000, algo parano1d; PROX: GH/s -> khs x1e6, algo proxima;
  # Ting (1.8.0): GH/s with "| candidates" (PROX: "| hits") -> khs x1e6, algo ting (as FFGT)
  if [[ $_line == *' FW/s'* ]]; then L_mul=1; L_kmul=0.001; L_fmt=%.2f; L_units=hs; L_algo=forgematrix_v4
  elif [[ $_line == *' MH/s'* ]]; then L_mul=1000; L_kmul=1000; L_fmt=%.1f; L_units=khs; L_algo=parano1d
  elif [[ $_line == *' GH/s'* && $_line == *'| candidates '* ]]; then L_mul=1e6; L_kmul=1e6; L_fmt=%.0f; L_units=khs; L_algo=ting
  elif [[ $_line == *' GH/s'* ]]; then L_mul=1e6; L_kmul=1e6; L_fmt=%.0f; L_units=khs; L_algo=proxima
  elif [[ $_line == *' MA/s'* ]]; then L_mul=1000; L_kmul=1000; L_fmt=%.1f; L_units=khs; L_algo=nebhash
  else L_mul=1e9; L_kmul=1e9; L_fmt=%.0f; L_units=khs; L_algo=$CUSTOM_ALGO; fi
  # NEB (FFGN 1.0.4): the rate over the last 5 min (per GPU, same order) when the line has it: a change of the block's dataset
  # (slot 0 Kepler / slot 1 TESS) does not show as a jump on the HiveOS graph
  if [[ $L_algo == nebhash ]]; then
    _avg=$(grep -oP '\| avg [0-9]+ min \K[0-9.]+' <<< "$_line")
    _avgs=$(grep -oP '\| avg [0-9]+ min [0-9.]+ MA/s \(\K[^)]+' <<< "$_line")
    if [[ -n $_avg && -n $_avgs ]]; then
      mapfile -t _kavg < <(grep -oP '[0-9.]+' <<< "$_avgs")
      if [[ ${#_kavg[@]} -eq ${#L_ids[@]} ]]; then L_ths=("${_kavg[@]}"); L_total=$_avg; fi
    fi
  fi
  # FFGN 1.0.12 (every coin): a lost GPU (DEAD / LOST on the line: Xid 79, its engine cannot reopen the device, gone from
  # NVML) counts 0, never its last rate, and the total is the sum of the others: the HiveOS Watchdog's minimal hashrate rule
  # sees the true total
  mapfile -t _down < <(grep -oP 'GPU\K[0-9]+(?= [0-9.]+(?: \([^)]*\))? (?:DEAD|LOST)\b)' <<< "$_line")
  _zero=
  for _i in "${!L_ids[@]}"; do
    if [[ " ${_down[*]} " == *" ${L_ids[$_i]} "* ]]; then L_ths[$_i]=0; _zero=1; fi
  done
  [[ -n $_zero ]] && L_total=$(printf '%s\n' "${L_ths[@]}" | awk '{s+=$1} END{printf "%.6f", s}')
  L_up=$(grep -oP '^\[\s*\K[0-9]+' <<< "$_line")
  if [[ $_line == *'user sub '* ]]; then
    L_acc=$(grep -oP 'user sub [0-9]+ acc \K[0-9]+' <<< "$_line")
    L_rej=$(grep -oP 'user sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
  else
    # NEB solo: shares to the node, blocks submitted
    L_acc=$(grep -oP '\| shares \K[0-9]+' <<< "$_line"); L_rej=$(grep -oP 'blocks sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
  fi
  # rig efficiency (Pearl "1157 W, 0.560 TH/W", NOID "0.460 MH/W", PROX "0.0161 GH/W"): HiveOS shows it next to the miner version
  L_extra=$(grep -oP 'total .*?\K[0-9.]+ ((TH|MH|MA)/W|GH/W)' <<< "$_line" | head -n 1)
  # 1.8.0 (all coins; FFGN 1.0.3): a core clock lock that does not hold (auto-uv / auto-lock are off on those GPUs; see the log)
  _unheld=$(grep -oP '\[LOCK NOT HELD: \K[^]]+' <<< "$_line")
  [[ -n $_unheld ]] && L_extra="$L_extra, LOCK NOT HELD $_unheld"
  if [[ $L_algo == nebhash ]]; then
    # NEB (FFGN 1.0.4/1.0.5): auto-uv guards, the dataset of the current block, and the OC profile's memory lock that the driver
    # did not let the miner lift (NEB is memory bound; this warning is NEB only, owner's rule 5)
    _uvw=$(grep -oP '\[AUTO-UV \K[^]]+' <<< "$_line" | paste -sd, -)
    [[ -n $_uvw ]] && L_extra="$L_extra, AUTO-UV $_uvw"
    _slot=$(grep -oP '\| slot \K[0-9]+ [A-Za-z0-9]+' <<< "$_line" | head -n 1)
    [[ -n $_slot ]] && L_extra="$L_extra, slot $_slot"
    _mem=$(grep -oP '\[WARNING \K[^]]*runs ~6x slower[^]]*' <<< "$_line" | head -n 1)
    [[ -n $_mem ]] && L_extra="$L_extra, $_mem"
  fi
  # FFGN 1.0.12: a GPU fell off the bus (Xid 79: the rig needs a reboot) / GPUs without their digest table for 10+ min (NEB)
  _rb=$(grep -oP '\[REBOOT NEEDED: \K[^]]+' <<< "$_line" | head -n 1)
  [[ -n $_rb ]] && L_extra="$L_extra, REBOOT NEEDED ($_rb)"
  _nt=$(grep -oP '\[NO TABLE: \K[^ \]]+' <<< "$_line" | head -n 1)
  [[ -n $_nt ]] && L_extra="$L_extra, NO TABLE $_nt"
  # FFGN 1.0.12: GPUs lost (--reboot-on-lost-gpu: the rig reboots after MIN minutes of it) / the reboot asked
  _gl=$(grep -oP '\[GPU LOST: \KGPU[0-9]+' <<< "$_line" | paste -sd, -)
  [[ -n $_gl ]] && L_extra="$L_extra, GPU LOST $_gl"
  [[ $_line == *'[REBOOTING]'* ]] && L_extra="$L_extra, REBOOTING"
  L_extra=${L_extra#, }
}

if [[ ( -z $_line && ${#_groups[@]} -eq 0 ) || $_age -gt 60 ]]; then
  khs=0
  stats=null
else
  mapfile -t _bus  < <(nvidia-smi --query-gpu=pci.bus --format=csv,noheader 2>/dev/null)
  mapfile -t _temp < <(nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null)
  mapfile -t _fan  < <(nvidia-smi --query-gpu=fan.speed --format=csv,noheader,nounits 2>/dev/null)
  declare -A _rate=()
  _ver=$CUSTOM_VERSION; _up=0; _acc=0; _rej=0; _algo=; _u1=; _hu=khs; khs=0; _best=-1; _parts=; _mixed=0
  _src=()
  if [[ ${#_groups[@]} -eq 0 ]]; then _src=("$_line")
  else for g in "${_groups[@]}"; do _src+=("$(grep -aE "^\[$g\] \[" <<< "$_tail" | tail -n 1)"); done; fi
  for k in "${!_src[@]}"; do
    _l=${_src[$k]}; _gname=
    if [[ ${#_groups[@]} -gt 0 ]]; then _gname=${_groups[$k]}; _l=${_l#"[$_gname] "}; fi
    _ffg_parse "$_l"
    [[ -n $_u1 && $_u1 != "$L_units:$L_mul" ]] && _mixed=1
    [[ -z $_u1 ]] && _u1="$L_units:$L_mul"
    # per GPU: the line's HiveOS value, and the same rate in kH/s for a rig of mixed units (Rig02 21.11: plain H/s of a Pearl
    # card, 6.4e13, showed as "1000.0 GH" on the dashboard; in kH/s it shows 63.8 TH like a Pearl-only rig)
    for i in "${!L_ids[@]}"; do
      _rate[${L_ids[$i]}]="$(awk -v x="${L_ths[$i]}" -v m="$L_mul" -v f="$L_fmt" 'BEGIN{printf f, x*m}') $(awk -v x="${L_ths[$i]}" -v m="$L_mul" -v u="$L_units" 'BEGIN{printf "%.12g", x*m*(u=="khs"?1:0.001)}')"
    done
    (( ${L_up:-0} > _up )) && _up=${L_up:-0}
    _acc=$(( _acc + ${L_acc:-0} )); _rej=$(( _rej + ${L_rej:-0} ))
    # the HiveOS algo and khs: the group with most cards (the first of equals)
    if (( ${#L_ids[@]} > _best )); then
      _best=${#L_ids[@]}; _algo=$L_algo; _hu=$L_units
      khs=$(awk -v x="${L_total:-0}" -v m="$L_kmul" -v f="$L_fmt" 'BEGIN{printf (m < 1 ? "%.5f" : f), x*m}')
    fi
    if [[ -n $_gname ]]; then _parts="$_parts${_parts:+ + }$_gname${L_extra:+ $L_extra}"; elif [[ -n $L_extra ]]; then _parts=$L_extra; fi
  done
  if [[ ${#_groups[@]} -gt 0 ]]; then _ver="$CUSTOM_VERSION split: $_parts"; elif [[ -n $_parts ]]; then _ver="$CUSTOM_VERSION $_parts"; fi
  # a newer release found by the miner (update.rs, 1.7.5): "1.7.5 0.652 TH/W, need update!!"
  _new=; [[ -f $_ffgp_dir/update-available ]] && _new=$(tr -cd '0-9A-Za-z.' < "$_ffgp_dir/update-available" | head -c 20)
  [[ -n $_new ]] && _ver="$_ver, need update!!"
  # mixed units (a split of coins with other units): HiveOS takes one hs_units, so every card is given in kH/s
  _units=$_hu; [[ $_mixed == 1 ]] && _units=khs
  _hs=(); _b=(); _t=(); _f=()
  # per-GPU values in device order (devices are numbered in PCI bus order, see h-run.sh); a split lists every group's cards
  mapfile -t _order < <(printf '%s\n' "${!_rate[@]}" | sort -n)
  for d in "${_order[@]}"; do
    [[ -z $d ]] && continue
    read -r _hv _raw <<< "${_rate[$d]}"
    [[ $_mixed == 1 ]] && _hv=$_raw
    _hs+=("$_hv")
    _b+=("$(( ${_bus[$d]:-0} ))")
    _t+=("${_temp[$d]:-0}")
    f=${_fan[$d]:-0}; [[ $f =~ ^[0-9]+$ ]] || f=0; _f+=("$f")
  done
  stats=$(jq -nc \
    --argjson hs "$(printf '%s\n' "${_hs[@]}" | jq -sc .)" \
    --argjson temp "$(printf '%s\n' "${_t[@]}" | jq -sc .)" \
    --argjson fan "$(printf '%s\n' "${_f[@]}" | jq -sc .)" \
    --argjson bus "$(printf '%s\n' "${_b[@]}" | jq -sc .)" \
    --arg ver "$_ver" --arg algo "$_algo" --arg units "$_units" \
    --argjson up "${_up:-0}" --argjson acc "${_acc:-0}" --argjson rej "${_rej:-0}" \
    '{hs: $hs, hs_units: $units, temp: $temp, fan: $fan, uptime: $up, ver: $ver, ar: [$acc, $rej], algo: $algo, bus_numbers: $bus}')
fi
