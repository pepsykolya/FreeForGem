@echo off
rem FreeForGem -- see LICENSE
setlocal EnableExtensions
title FreeForGem - NOID (Parano1d)

rem ================================================================
rem  FreeForGem for Windows - NOID (Parano1d) - EDIT THESE VALUES
rem  Needs an NVIDIA RTX 30/40/50 (or A100/H100) and the NVIDIA driver 580 or newer. Not for RTX 20 / GTX 16 / AMD.
rem ================================================================
rem Your NOID wallet address (o1...)
set "WALLET="
rem NOID pool: stratum+ssl://HOST:PORT (TLS, most NOID pools) or stratum+tcp://HOST:PORT. Default: InnovLab Europe.
set "POOL=stratum+ssl://eu2.innovlab.cc:19601"
rem Other pools (remove "rem" from ONE line to use it instead; MANUAL.md has the list):
rem set "POOL=stratum+ssl://noid.suprnova.cc:3340"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem eff = best MH per watt (default; recommended with LOCK_CCLOCK=1300 and AUTO_UV=1 on RTX 30),
rem speed = more MH/s (recommended LOCK_CCLOCK=1500 and AUTO_UV=1 on RTX 30: almost the same MH per watt)
set "MODE=eff"
rem Optional undervolt auto-tuner for NOID (needs start-noid.bat run as Administrator). Every step is checked bit-exactly on the
rem NOID kernel for 40 s (the long stress check), the result is saved per card and mode; the first run takes about 10 minutes.
rem Without it the miner sets a positive core offset (e.g. one tuned for Pearl) to 0 for NOID and puts it back on exit.
rem To enable it, remove "rem" from the next line:
rem set "AUTO_UV=1"
rem Tuner limits (empty = default): first step (default: RTX 3070 by the core lock, eff +105 at 1400, speed +240 at 1500; RTX 3080/3090 +180; others +120) and the highest offset tried (default per card, at most 600)
set "UV_MIN="
set "UV_MAX="
rem Optional GPU clock control (needs Administrator; empty = leave as is). One value for all GPUs, or a comma list in DEVICES order.
rem NOID recommendation for RTX 30: eff 1300, speed 1500 (MANUAL.md, NOID section)
set "LOCK_CCLOCK="
set "LOCK_MCLOCK="
set "MOFFSET="
rem Fixed core offset for NOID instead of tuning, MHz (AT YOUR OWN RISK: NOID fails silently above its edge). Example: 150
set "UV_OFFSET="
rem Any other miner options, e.g. --uv-reset to forget the saved undervolt result of the current core lock once (--uv-reset-all: all):
set "EXTRA="
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Algo noid -NoidMode "%MODE%" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -AutoUv "%AUTO_UV%" -UvMax "%UV_MAX%" -UvMin "%UV_MIN%" -UvOffset "%UV_OFFSET%" -Extra "%EXTRA%" -LockCclock "%LOCK_CCLOCK%" -LockMclock "%LOCK_MCLOCK%" -Moffset "%MOFFSET%"
echo.
echo FreeForGem stopped (exit code %ERRORLEVEL%).
pause
