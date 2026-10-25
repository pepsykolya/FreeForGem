#!/usr/bin/env bash
# FreeForGem — see LICENSE
# Sourced by the HiveOS agent: sets $khs (total, kH/s) and $stats (JSON) from the miner's periodic log line
#   [ 270s] GPU0 68.318 | GPU1 68.2 | total 136.5 TH/s (...) | ... | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0     (Pearl)
#   [ 380s] GPU0 21.76 (237 W, 330742 FW/kWh)  GPU1 21.75 (...) | total 87.46 FW/s | ... | user sub 1 acc 1 rej 0 | ...   (CMFD)
#   [  60s] GPU1 35.512 (77 W, 0.461 MH/W)  GPU2 ... | total 101.2 MH/s, 220 W, 0.460 MH/W | hits ... | user sub 4 acc 4 rej 0 | ...  (NOID)
_ffgp_dir=$(dirname "${BASH_SOURCE[0]}")
. "$_ffgp_dir/h-manifest.conf"
_log="${CUSTOM_LOG_BASENAME}.log"
_line=$(tail -n 400 "$_log" 2>/dev/null | grep -aE ' total [0-9.]+ (TH|FW|MH)/s' | tail -n 1)
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
  # NOID: MH/s -> per-GPU khs x1000, khs = MH/s x 1000, algo parano1d
  if [[ $_line == *' FW/s'* ]]; then _mul=1; _kmul=0.001; _fmt=%.2f; _units=hs; _algo=forgematrix_v4
  elif [[ $_line == *' MH/s'* ]]; then _mul=1000; _kmul=1000; _fmt=%.1f; _units=khs; _algo=parano1d
  else _mul=1e9; _kmul=1e9; _fmt=%.0f; _units=khs; _algo=$CUSTOM_ALGO; fi
  _up=$(grep -oP '^\[\s*\K[0-9]+' <<< "$_line")
  _acc=$(grep -oP 'user sub [0-9]+ acc \K[0-9]+' <<< "$_line")
  _rej=$(grep -oP 'user sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
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
  khs=$(awk -v x="${_total:-0}" -v m="$_kmul" -v f="$_fmt" 'BEGIN{printf (m < 1 ? "%.5f" : f), x*m}')
  stats=$(jq -nc \
    --argjson hs "$(printf '%s\n' "${_hs[@]}" | jq -sc .)" \
    --argjson temp "$(printf '%s\n' "${_t[@]}" | jq -sc .)" \
    --argjson fan "$(printf '%s\n' "${_f[@]}" | jq -sc .)" \
    --argjson bus "$(printf '%s\n' "${_b[@]}" | jq -sc .)" \
    --arg ver "$CUSTOM_VERSION" --arg algo "$_algo" --arg units "$_units" \
    --argjson up "${_up:-0}" --argjson acc "${_acc:-0}" --argjson rej "${_rej:-0}" \
    '{hs: $hs, hs_units: $units, temp: $temp, fan: $fan, uptime: $up, ver: $ver, ar: [$acc, $rej], algo: $algo, bus_numbers: $bus}')
fi
