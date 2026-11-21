@echo off
rem FreeForGem -- see LICENSE
setlocal EnableExtensions
title FreeForGem - Ting (Daler / DLR)
cd /d "%~dp0"

rem ================================================================
rem  FreeForGem for Windows - Ting (Daler / DLR, ting-sha256) - EDIT THESE VALUES
rem  NVIDIA GTX 10 and newer (CUDA), AMD (OpenCL). Pool mining (ACCOUNT) or through your ffgt-gate (POOL + PASSWORD).
rem ================================================================
rem Your Ting account (64 hex characters; Ting ID app or `ting wallet account`), without "&net=..."
set "ACCOUNT="
rem Pool HOST:PORT (a comma list = failover, the first is the primary), or your ffgt-gate as ting://HOST:PORT
set "POOL=crypto-connection.net:4073"
rem Password: x for a pool, the rig's password for an ffgt-gate
set "PASSWORD=x"
rem Worker (rig) name
set "WORKER=%COMPUTERNAME%"
rem GPUs to use: empty = all, or a list like 0,1
set "DEVICES="
rem Recommended (needs Administrator): lock the core clock and let auto-uv find a lower voltage for it. RTX 3070: 1500-1800.
rem The memory clock is locked low by default (SHA-256 does not use it).
set "LOCK_CCLOCK="
set "AUTO_UV="
rem Any other miner options
set "EXTRA="
rem ================================================================

if "%ACCOUNT%"=="" ( echo ACCOUNT is empty: right-click start-ting.bat - Edit, set ACCOUNT. & pause & exit /b 1 )
set "ARGS=--algo ting --pool %POOL% --wallet %ACCOUNT% --worker %WORKER% --password %PASSWORD%"
if not "%DEVICES%"=="" set "ARGS=%ARGS% --devices %DEVICES%"
if not "%LOCK_CCLOCK%"=="" set "ARGS=%ARGS% --lock-cclock %LOCK_CCLOCK%"
if "%AUTO_UV%"=="1" set "ARGS=%ARGS% --auto-uv"
freeforgem.exe %ARGS% %EXTRA%
echo.
echo FreeForGem stopped (exit code %ERRORLEVEL%).
pause
