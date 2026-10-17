@echo off
rem FreeForGemPear -- see LICENSE
setlocal EnableExtensions
title FreeForGemPear - Kryptex Singapore

rem ================================================================
rem  FreeForGemPear for Windows - pool: Kryptex Singapore - EDIT THESE VALUES
rem ================================================================
rem Your PRL wallet address (prl1...). REPLACE the address below with YOUR OWN:
rem as shipped it is the developer's address, and everything you mine would go there.
set "WALLET=prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr"
rem Stratum pool HOST:PORT (see MANUAL.md, section "Pools")
set "POOL=prl-sg.kryptex.network:7048"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem Kryptex only: custom share difficulty for big rigs, e.g. d=8388608 (empty = pool default 2097152). See MANUAL.md, "Pools".
set "PASSWORD="
rem Optional undervolt auto-tuner (saves watts at the same hashrate). Needs a start.bat run as Administrator.
rem Read MANUAL.md, section 12 ("Windows: step by step") first; also lock the core clock (LOCK_CCLOCK). To enable it, remove "rem" from the next line:
rem set "AUTO_UV=1"
rem Highest offset the undervolt tuner tries, MHz (empty = 495; RTX 40/50 often hold +450..480, at most 600):
set "UV_MAX="
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
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -AutoUv "%AUTO_UV%" -UvMax "%UV_MAX%" -Password "%PASSWORD%" -LockCclock "%LOCK_CCLOCK%" -LockMclock "%LOCK_MCLOCK%" -Moffset "%MOFFSET%"
echo.
echo FreeForGemPear stopped (exit code %ERRORLEVEL%).
pause
