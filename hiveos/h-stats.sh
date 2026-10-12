#!/usr/bin/env bash
# FreeForGemPear — see LICENSE
# Sourced by the HiveOS agent: sets $khs (total, kH/s) and $stats (JSON) from the miner's periodic log line
#   [ 270s] GPU0 68.318 | GPU1 68.2 | total 136.5 TH/s (...) | ... | user sub 6 acc 6 rej 0 | fee sub 0 acc 0 rej 0
_ffgp_dir=$(dirname "${BASH_SOURCE[0]}")
. "$_ffgp_dir/h-manifest.conf"
_log="${CUSTOM_LOG_BASENAME}.log"
_line=$(tail -n 400 "$_log" 2>/dev/null | grep -a ' total .* TH/s' | tail -n 1)
_age=$(( $(date +%s) - $(stat -c %Y "$_log" 2>/dev/null || echo 0) ))

if [[ -z $_line || $_age -gt 60 ]]; then
  khs=0
  stats=null
else
  # per-GPU TH/s in device order (devices are numbered in PCI bus order, see h-run.sh)
  mapfile -t _ids < <(grep -oP 'GPU\K[0-9]+(?= [0-9.]+)' <<< "$_line")
  mapfile -t _ths < <(grep -oP 'GPU[0-9]+ \K[0-9.]+' <<< "$_line")
  _total=$(grep -oP 'total \K[0-9.]+' <<< "$_line")
  _up=$(grep -oP '^\[\s*\K[0-9]+' <<< "$_line")
  _acc=$(grep -oP 'user sub [0-9]+ acc \K[0-9]+' <<< "$_line")
  _rej=$(grep -oP 'user sub [0-9]+ acc [0-9]+ rej \K[0-9]+' <<< "$_line")
  mapfile -t _bus  < <(nvidia-smi --query-gpu=pci.bus --format=csv,noheader 2>/dev/null)
  mapfile -t _temp < <(nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null)
  mapfile -t _fan  < <(nvidia-smi --query-gpu=fan.speed --format=csv,noheader,nounits 2>/dev/null)
  _hs=(); _b=(); _t=(); _f=()
  for i in "${!_ids[@]}"; do
    d=${_ids[$i]}
    _hs+=("$(awk -v x="${_ths[$i]}" 'BEGIN{printf "%.0f", x*1e9}')")
    _b+=("$(( ${_bus[$d]:-0} ))")
    _t+=("${_temp[$d]:-0}")
    f=${_fan[$d]:-0}; [[ $f =~ ^[0-9]+$ ]] || f=0; _f+=("$f")
  done
  khs=$(awk -v x="${_total:-0}" 'BEGIN{printf "%.0f", x*1e9}')
  stats=$(jq -nc \
    --argjson hs "$(printf '%s\n' "${_hs[@]}" | jq -sc .)" \
    --argjson temp "$(printf '%s\n' "${_t[@]}" | jq -sc .)" \
    --argjson fan "$(printf '%s\n' "${_f[@]}" | jq -sc .)" \
    --argjson bus "$(printf '%s\n' "${_b[@]}" | jq -sc .)" \
    --arg ver "$CUSTOM_VERSION" --arg algo "$CUSTOM_ALGO" \
    --argjson up "${_up:-0}" --argjson acc "${_acc:-0}" --argjson rej "${_rej:-0}" \
    '{hs: $hs, hs_units: "khs", temp: $temp, fan: $fan, uptime: $up, ver: $ver, ar: [$acc, $rej], algo: $algo, bus_numbers: $bus}')
fi
