@echo off
rem FreeForGemPear -- see LICENSE
setlocal EnableExtensions
title FreeForGemPear - Common Foundry (CMFD)

rem ================================================================
rem  FreeForGemPear for Windows - Common Foundry (CMFD, ForgeMatrix V4) - EDIT THESE VALUES
rem  Engine: FreeForgeMiner (MIT) in the cmfd folder. Needs about 7 GB of free GPU memory per card (6.4 GB model,
rem  downloaded once into cmfd-model from the official Common Foundry servers) and an RTX 20xx or newer GPU.
rem ================================================================
rem Your CMFD wallet address (64 hexadecimal characters)
set "WALLET="
rem Pool URL: cmfd+tls://IP:PORT?pin=64_HEX exactly as your pool publishes it (default: Aria / AriaBrain)
set "POOL=cmfd+tls://159.69.194.46:29445?pin=9dfb51083f287726117f689f87bc7a878792efcca58ca8b6f6e7c05ac5d152e9"
rem Worker (rig) name: 1-32 letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem speed = maximum hashrate (default), eco = fewer watts (RTX 30: about -13 % power, -2 % hashrate). RTX 40/50: same in both
set "MODE=speed"
rem Optional GPU clock control (needs start-cmfd.bat run as Administrator; empty = leave as is). Stock clocks are the safe
rem choice for CMFD: do NOT undervolt (wrong results / no shares). One value for all GPUs, or a comma list in DEVICES order.
set "LOCK_CCLOCK="
set "LOCK_MCLOCK="
set "MOFFSET="
rem Any other miner options, e.g. --per-gpu-workers (one pool worker per GPU) or --cmfd-batch 32:
set "EXTRA="
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -CmfdMode "%MODE%" -Extra "%EXTRA%" -LockCclock "%LOCK_CCLOCK%" -LockMclock "%LOCK_MCLOCK%" -Moffset "%MOFFSET%"
echo.
echo FreeForGemPear stopped (exit code %ERRORLEVEL%).
pause
