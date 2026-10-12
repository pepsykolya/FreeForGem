@echo off
rem FreeForGemPear -- see LICENSE
setlocal EnableExtensions
title FreeForGemPear

rem ================================================================
rem  FreeForGemPear for Windows - EDIT THESE VALUES
rem ================================================================
rem Your PRL wallet address (prl1...)
set "WALLET="
rem Stratum pool HOST:PORT (see MANUAL.md, section "Pools")
set "POOL=pearl-eu2.luckypool.io:3360"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem ================================================================

set "PS=powershell.exe"
where pwsh.exe >nul 2>&1 && set "PS=pwsh.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" -Wallet "%WALLET%" -Pool "%POOL%" -Worker "%WORKER%" -Devices "%DEVICES%"
echo.
echo FreeForGemPear stopped (exit code %ERRORLEVEL%).
pause
