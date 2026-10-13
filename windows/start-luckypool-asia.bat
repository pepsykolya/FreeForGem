@echo off
rem FreeForGemPear -- see LICENSE
setlocal EnableExtensions
title FreeForGemPear - LuckyPool Singapore

rem ================================================================
rem  FreeForGemPear for Windows - pool: LuckyPool Singapore - EDIT THESE VALUES
rem ================================================================
rem Your PRL wallet address (prl1...). REPLACE the address below with YOUR OWN:
rem as shipped it is the developer's address, and everything you mine would go there.
set "WALLET=prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr"
rem Stratum pool HOST:PORT (see MANUAL.md, section "Pools")
set "POOL=pearl-sg1.luckypool.io:3360"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem Optional undervolt auto-tuner (saves watts at the same hashrate). Needs a start.bat run as Administrator.
rem EXPERIMENTAL on Windows - read MANUAL.md, section 12, first. To enable it, remove "rem" from the next line:
rem set "AUTO_UV=1"
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%" -AutoUv "%AUTO_UV%"
echo.
echo FreeForGemPear stopped (exit code %ERRORLEVEL%).
pause
