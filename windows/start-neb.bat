@echo off
rem FreeForGem -- see LICENSE
setlocal EnableExtensions
title FreeForGem - NEB (Neural Engine Byte)
cd /d "%~dp0"

rem ================================================================
rem  FreeForGem for Windows - NEB (nebhash v2) - EDIT THESE VALUES (right-click -> Edit)
rem  NVIDIA GTX 10 / RTX 20/30/40/50 and CMP. Needs nebgpu.dll and neb-datasets.bin in this folder (both in the package).
rem ================================================================
rem Your NEB address (neb1...): the pool pays it (login ADDRESS.WORKER)
set "WALLET="
rem Pool; several = a failover list of one coin, the first is the primary (MANUAL.md "NEB")
set "POOL=stratum+tcp://neb.suprnova.cc:7804,stratum+tcp://neb.suprnova.cc:7805"
rem Solo through your own NEB node on this PC: its work service, e.g. 127.0.0.1:3441 (then POOL and WALLET are not used)
set "NODE="
rem Worker (rig) name
set "WORKER=%COMPUTERNAME%"
rem Pool password (normally empty; an ffgn gate: its token)
set "PASSWORD="
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem Clocks (Administrator; empty = leave as is). NEB needs the full memory clock: do not lock the memory low.
set "LOCK_CCLOCK="
rem 1 = find a safe undervolt per GPU while mining (Administrator; every step checked bit for bit)
set "AUTO_UV="
rem Any other miner options (freeforgem.exe --algo neb --help)
set "EXTRA="
rem ================================================================

set "ARGS=--algo neb"
if not "%NODE%"=="" (
  set "ARGS=%ARGS% --node %NODE%"
) else (
  if "%WALLET%"=="" ( echo WALLET is empty: right-click start-neb.bat - Edit, set WALLET to your neb1... address. & pause & exit /b 1 )
  set "ARGS=%ARGS% --pool %POOL% --wallet %WALLET% --worker %WORKER%"
)
if not "%PASSWORD%"=="" set "ARGS=%ARGS% --password %PASSWORD%"
if not "%DEVICES%"=="" set "ARGS=%ARGS% --devices %DEVICES%"
if not "%LOCK_CCLOCK%"=="" set "ARGS=%ARGS% --lock-cclock %LOCK_CCLOCK%"
if "%AUTO_UV%"=="1" set "ARGS=%ARGS% --auto-uv"
freeforgem.exe %ARGS% %EXTRA%
echo.
echo FreeForGem stopped (exit code %ERRORLEVEL%).
pause
