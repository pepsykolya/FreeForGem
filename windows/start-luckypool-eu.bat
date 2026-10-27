@echo off
rem FreeForGem -- see LICENSE
setlocal EnableExtensions
title FreeForGem - LuckyPool Europe

rem ================================================================
rem  FreeForGem for Windows - pool: LuckyPool Europe - EDIT THESE VALUES
rem ================================================================
rem Your PRL wallet address (prl1...). REPLACE the address below with YOUR OWN:
rem as shipped it is the developer's address, and everything you mine would go there.
set "WALLET=prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr"
rem LuckyPool also pays Nockchain (NOCK) for the same work (merge mining, no hashrate loss): add "+" and your
rem NOCK address after the PRL address, e.g.  set "WALLET=prl1...+YOUR_NOCK_ADDRESS"
rem Stratum pool HOST:PORT (see MANUAL.md, section "Pools")
set "POOL=pearl-eu2.luckypool.io:3360"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all GPUs, or a list like 0,1,3 (NVIDIA: indexes from nvidia-smi; AMD: order of the miner log)
set "DEVICES="
rem Stratum password (normally empty)
set "PASSWORD="
rem Optional undervolt auto-tuner (saves watts at the same hashrate). Needs a start.bat run as Administrator.
rem Read MANUAL.md, section 12 ("Windows: step by step") first; also lock the core clock (LOCK_CCLOCK). To enable it, remove "rem" from the next line:
rem set "AUTO_UV=1"
rem Undervolt tuner settings (used with AUTO_UV=1; empty = default). Examples per card: MANUAL.md, section 12.
rem Starting offset, MHz (empty = RTX 3070/3060 Ti by the core lock, RTX 3080/3090 +180, else the card's current offset or +120;
rem RTX 40: at least +200, RTX 50: at least +300; a start that fails steps down 30 MHz). One value for all GPUs, or a comma list in DEVICES order:
set "UV_MIN="
rem Highest offset the tuner tries, MHz (at most 600). Empty = per card: RTX 20xx/GTX 16xx/CMP 50HX 225, CMP 30HX/40HX 150,
rem others 495. One value for all GPUs, or a comma list in DEVICES order, e.g. 225,495,150 (- = that GPU's default):
set "UV_MAX="
rem MHz per tuning step (empty = 15; 30 = about half the tuning time, coarser result):
set "UV_STEP="
rem 1 = after a failed step keep the last good step (empty = last good step minus one step, the safer margin):
set "UV_KEEP_LAST="
rem Re-check the offset every N hours while mining (empty = 6, 0 = never):
set "UV_REVAL_HOURS="
rem Fixed offset instead of tuning, MHz (checked once, no search; leave AUTO_UV off). Example: 240  or per GPU: 240,-,150
set "UV_OFFSET="
rem Any other miner options, e.g. --uv-reset to forget the saved undervolt result of the current core lock once (--uv-reset-all: all):
set "EXTRA="
rem Optional GPU clock control (needs start.bat run as Administrator; empty = leave as is).
rem One value for all GPUs, or a comma list in DEVICES order. Example: 1500  or  1500,1500,1815
set "LOCK_CCLOCK="
rem Lock memory clock (MHz):
set "LOCK_MCLOCK="
rem Memory clock offset (MHz, may be negative; needs NVIDIA driver 555+):
set "MOFFSET="
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -AutoUv "%AUTO_UV%" -UvMax "%UV_MAX%" -UvMin "%UV_MIN%" -UvStep "%UV_STEP%" -UvKeepLast "%UV_KEEP_LAST%" -UvRevalHours "%UV_REVAL_HOURS%" -UvOffset "%UV_OFFSET%" -Extra "%EXTRA%" -Password "%PASSWORD%" -LockCclock "%LOCK_CCLOCK%" -LockMclock "%LOCK_MCLOCK%" -Moffset "%MOFFSET%"
echo.
echo FreeForGem stopped (exit code %ERRORLEVEL%).
pause
