@echo off
rem FreeForGemPear -- see LICENSE
setlocal EnableExtensions
title FreeForGemPear - Common Foundry (CMFD)

rem ================================================================
rem  FreeForGemPear for Windows - Common Foundry (CMFD, ForgeMatrix V4) - EDIT THESE VALUES
rem  Engine: FreeForgeMiner (MIT) in the cmfd folder. Needs about 7 GB of free GPU memory per card (6.4 GB model,
rem  downloaded once into cmfd-model from the official Common Foundry servers) and an RTX 20xx or newer GPU.
rem ================================================================
rem Your CMFD wallet address: cmfd1... (wallet 1.0.16+) or the 64-hex form; both work
set "WALLET="
rem Pool URL: cmfd+tls://IP:PORT?pin=64_HEX exactly as your pool publishes it. Default: Aria / AriaBrain (pool.ariabrain.com).
set "POOL=cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9"
rem Other pools (remove "rem" from ONE line to use it instead; MANUAL.md has the full list):
rem   Midas Mining (midasmining.net, 1 %):
rem set "POOL=cmfd+tls://24.107.152.151:29745?pin=2ee4e6a028b07b300753dff3c260b79763c5e748d4e78b86cf3855e97eb2e80e"
rem   cmfd-pool.online (3 %):
rem set "POOL=cmfd+tls://109.199.124.187:29465?pin=ebe88f5e05f3a222208d551d05b6d39057b64ce8239ba7a708d487e15ac711be"
rem Worker (rig) name: 1-32 letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all GPUs, or a list like 0,1,3 (NVIDIA: indexes from nvidia-smi; AMD: order of the miner log)
set "DEVICES="
rem speed = maximum hashrate (default), eco = fewer watts (RTX 30: about -13 % power, -2 % hashrate). RTX 40/50: same in both
set "MODE=speed"
rem Optional GPU clock control (needs start-cmfd.bat run as Administrator; empty = leave as is). One value for all GPUs, or a
rem comma list in DEVICES order. CMFD needs stock clocks: run as Administrator and the miner sets an undervolt (positive core
rem offset, e.g. one tuned for Pearl) to 0 for the run and puts it back on exit; without Administrator remove it yourself.
set "LOCK_CCLOCK="
set "LOCK_MCLOCK="
set "MOFFSET="
rem Force a core offset for CMFD instead of 0, MHz (AT YOUR OWN RISK: wrong results / no shares are likely). Example: 100
rem (one value for all GPUs, or a comma list in DEVICES order; - = the default 0 for that GPU)
set "UV_OFFSET="
rem NVIDIA: undervolt (core offset) tuned for CMFD itself (run as Administrator). The first start tunes for ~10-20 min per
rem card (every step checked bit-for-bit on the real model + the official test vector), later starts apply the saved
rem result; your own offset comes back when the miner exits. Separate from the Pearl result. MANUAL.md, section 13.
rem set "AUTO_UV=1"
rem Limits of the search, MHz (empty = per-card defaults: 495, Turing/CMP 50HX 225, CMP 30HX/40HX 150); one value or a list
set "UV_MAX="
set "UV_MIN="
rem Any other miner options, e.g. --per-gpu-workers (one pool worker per GPU) or --cmfd-batch 32:
set "EXTRA="
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -CmfdMode "%MODE%" -Extra "%EXTRA%" -LockCclock "%LOCK_CCLOCK%" -LockMclock "%LOCK_MCLOCK%" -Moffset "%MOFFSET%" -UvOffset "%UV_OFFSET%" -AutoUv "%AUTO_UV%" -UvMax "%UV_MAX%" -UvMin "%UV_MIN%"
echo.
echo FreeForGemPear stopped (exit code %ERRORLEVEL%).
pause
