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
_ffgp_dir=$(dirname "${BASH_SOURCE[0]}")
. "$_ffgp_dir/h-manifest.conf"
_log="${CUSTOM_LOG_BASENAME}.log"
# ANSI colour codes (1.7.2 colours the log; the stats line itself is never coloured) are stripped before parsing
_line=$(tail -n 400 "$_log" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -aE ' total [0-9.]+ (TH|FW|MH|GH|MA)/s' | tail -n 1)
_age=$(( $(date +%s) - $(stat -c %Y "$_log" 2>/dev/null || echo 0) ))

if [[ -z $_line || $_age -gt 60 ]]; then
  khs=0
  stats=null
else
  # per-GPU TH/s in device order (devices are numbered in PCI bus order, see h-run.sh)
  mapfile -t _ids < <(grep -oP 'GPU\K[0-9]+(?= [0-9.]+)' <<< "$_line")
  mapfile -t _ths < <(grep -oP 'GPU[0-9]+ \K[0-9.]+' <<< "$_line")
  _total=$(grep -oP 'total \K[0-9.]+' <<< "$_line")
  # Pearl: TH/s -> per-GPU khs x1e9, khs x1e9; CMFD (as FreeForgeMiner reports it): FW/s as hs 1:1, khs = FW/s / 1000
  # NOID: MH/s -> per-GPU khs x1000, khs = MH/s x 1000, algo parano1d; PROX: GH/s -> khs x1e6, algo proxima
  if [[ $_line == *' FW/s'* ]]; then _mul=1; _kmul=0.001; _fmt=%.2f; _units=hs; _algo=forgematrix_v4
  elif [[ $_line == *' MH/s'* ]]; then _mul=1000; _kmul=1000; _fmt=%.1f; _units=khs; _algo=parano1d
  elif [[ $_line == *' GH/s'* ]]; then _mul=1e6; _kmul=1e6; _fmt=%.0f; _units=khs; _algo=proxima
  elif [[ $_line == *' MA/s'* ]]; then _mul=1000; _kmul=1000; _fmt=%.1f; _units=khs; _algo=nebhash
  else _mul=1e9; _kmul=1e9; _fmt=%.0f; _units=khs; _algo=$CUSTOM_ALGO; fi
  # NEB (FFGN 1.0.4): the rate over the last 5 min (per GPU, same order) when the line has it: a change of the block's dataset
  # (slot 0 Kepler / slot 1 TESS) does not show as a jump on the HiveOS graph
  if [[ $_algo == nebhash ]]; then
    _avg=$(grep -oP '\| avg [0-9]+ min \K[0-9.]+' <<< "$_line")
    _avgs=$(grep -oP '\| avg [0-9]+ min [0-9.]+ MA/s \(\K[^)]+' <<< "$_line")
    if [[ -n $_avg && -n $_avgs ]]; then
      mapfile -t _kavg < <(grep -oP '[0-9.]+' <<< "$_avgs")
      if [[ ${#_kavg[@]} -eq ${#_ids[@]} ]]; then _ths=("${_kavg[@]}"); _total=$_avg; fi
    fi
  fi
  _up=$(grep -oP '^\[\s*\K[0-9]+' <<< "$_line")
  if [[ $_line == *'user sub '* ]]; then
    _acc=$(grep -oP 'user sub [0-9]+ acc \K[0-9]+' <<< "$_line")
    _rej=$(grep -oP 'user sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
  else
    # NEB solo: shares to the node, blocks submitted
    _acc=$(grep -oP '\| shares \K[0-9]+' <<< "$_line"); _rej=$(grep -oP 'blocks sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
  fi
  mapfile -t _bus  < <(nvidia-smi --query-gpu=pci.bus --format=csv,noheader 2>/dev/null)
  mapfile -t _temp < <(nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null)
  mapfile -t _fan  < <(nvidia-smi --query-gpu=fan.speed --format=csv,noheader,nounits 2>/dev/null)
  _hs=(); _b=(); _t=(); _f=()
  for i in "${!_ids[@]}"; do
    d=${_ids[$i]}
    _hs+=("$(awk -v x="${_ths[$i]}" -v m="$_mul" -v f="$_fmt" 'BEGIN{printf f, x*m}')")
    _b+=("$(( ${_bus[$d]:-0} ))")
    _t+=("${_temp[$d]:-0}")
    f=${_fan[$d]:-0}; [[ $f =~ ^[0-9]+$ ]] || f=0; _f+=("$f")
  done
  # rig efficiency (Pearl "1157 W, 0.560 TH/W", NOID "0.460 MH/W", PROX "0.0161 GH/W"): HiveOS shows it next to the miner version
  _eff=$(grep -oP 'total .*?\K[0-9.]+ ((TH|MH|MA)/W|GH/W)' <<< "$_line" | head -n 1)
  _ver=$CUSTOM_VERSION; [[ -n $_eff ]] && _ver="$CUSTOM_VERSION $_eff"
  # a newer release found by the miner (update.rs, 1.7.5): "1.7.5 0.652 TH/W, need update!!"
  _new=; [[ -f $_ffgp_dir/update-available ]] && _new=$(tr -cd '0-9A-Za-z.' < "$_ffgp_dir/update-available" | head -c 20)
  [[ -n $_new ]] && _ver="$_ver, need update!!"
  # 1.8.0 (all coins; FFGN 1.0.3): a core clock lock that does not hold (auto-uv / auto-lock are off on those GPUs; see the log)
  _unheld=$(grep -oP '\[LOCK NOT HELD: \K[^]]+' <<< "$_line")
  [[ -n $_unheld ]] && _ver="$_ver, LOCK NOT HELD $_unheld"
  if [[ $_algo == nebhash ]]; then
    # NEB (FFGN 1.0.4/1.0.5): auto-uv guards, the dataset of the current block, and the OC profile's memory lock that the driver
    # did not let the miner lift (NEB is memory bound; this warning is NEB only, owner's rule 5)
    _uvw=$(grep -oP '\[AUTO-UV \K[^]]+' <<< "$_line" | paste -sd, -)
    [[ -n $_uvw ]] && _ver="$_ver, AUTO-UV $_uvw"
    _slot=$(grep -oP '\| slot \K[0-9]+ [A-Za-z0-9]+' <<< "$_line" | head -n 1)
    [[ -n $_slot ]] && _ver="$_ver, slot $_slot"
    _mem=$(grep -oP '\[WARNING \K[^]]*runs ~6x slower[^]]*' <<< "$_line" | head -n 1)
    [[ -n $_mem ]] && _ver="$_ver, $_mem"
  fi
  khs=$(awk -v x="${_total:-0}" -v m="$_kmul" -v f="$_fmt" 'BEGIN{printf (m < 1 ? "%.5f" : f), x*m}')
  stats=$(jq -nc \
    --argjson hs "$(printf '%s\n' "${_hs[@]}" | jq -sc .)" \
    --argjson temp "$(printf '%s\n' "${_t[@]}" | jq -sc .)" \
    --argjson fan "$(printf '%s\n' "${_f[@]}" | jq -sc .)" \
    --argjson bus "$(printf '%s\n' "${_b[@]}" | jq -sc .)" \
    --arg ver "$_ver" --arg algo "$_algo" --arg units "$_units" \
    --argjson up "${_up:-0}" --argjson acc "${_acc:-0}" --argjson rej "${_rej:-0}" \
    '{hs: $hs, hs_units: $units, temp: $temp, fan: $fan, uptime: $up, ver: $ver, ar: [$acc, $rej], algo: $algo, bus_numbers: $bus}')
fi
