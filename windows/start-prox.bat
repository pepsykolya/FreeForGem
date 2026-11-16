@echo off
rem FreeForGem -- see LICENSE
setlocal EnableExtensions
title FreeForGem - PROX (Proxima)
cd /d "%~dp0"

rem ================================================================
rem  FreeForGem for Windows - PROX (Proxima, txid-blake2b) - EDIT THESE VALUES
rem  NVIDIA GTX 10 / RTX 20/30/40/50. Pool mining (WALLET + PASSWORD) or solo through your node (SOLO + TOKEN).
rem ================================================================
rem Your PROX address (the pool login is ADDRESS.WORKER)
set "WALLET="
rem Pool and its password (proximamine: the secret of your account; InnovLab: x, WALLET = your holder id)
rem Backup pools (1.7.8): a comma separated list of one protocol, the first one is the primary (the miner fails over and comes back, MANUAL.md "Pool lists")
set "POOL=stratum+ssl://pool.proximamine.com:3334"
rem set "POOL=innov+tls://eu3.innovlab.cc:19701"
set "PASSWORD="
rem Worker (rig) name
set "WORKER=%COMPUTERNAME%"
rem Solo instead of a pool: the node's seeker address and token (mine.seeker.listen / mine.seeker.token), e.g. http://127.0.0.1:8100
set "SOLO="
set "TOKEN="
rem GPUs to use: empty = all NVIDIA GPUs, or a list like 0,1,3 (indexes from nvidia-smi)
set "DEVICES="
rem Recommended (needs Administrator): lock the core clock and let auto-uv find a lower voltage for it. RTX 3070: 1500
rem (2.66 GH/s at 87 W, offset +120). Empty = leave the clocks as they are. AUTO_UV=1 needs a LOCK_CCLOCK to be most useful.
set "LOCK_CCLOCK="
set "AUTO_UV="
set "LOCK_MCLOCK="
rem Any other miner options
set "EXTRA="
rem ================================================================

set "ARGS=--algo prox"
if not "%SOLO%"=="" (
  set "ARGS=%ARGS% --solo %SOLO% --token %TOKEN% --seeker-name %WORKER%"
) else (
  if "%WALLET%"=="" ( echo WALLET is empty: right-click start-prox.bat - Edit, set WALLET and PASSWORD. & pause & exit /b 1 )
  set "ARGS=%ARGS% --pool %POOL% --wallet %WALLET% --worker %WORKER% --password %PASSWORD%"
)
if not "%DEVICES%"=="" set "ARGS=%ARGS% --devices %DEVICES%"
if not "%LOCK_CCLOCK%"=="" set "ARGS=%ARGS% --lock-cclock %LOCK_CCLOCK%"
if not "%LOCK_MCLOCK%"=="" set "ARGS=%ARGS% --lock-mclock %LOCK_MCLOCK%"
if "%AUTO_UV%"=="1" set "ARGS=%ARGS% --auto-uv"
freeforgem.exe %ARGS% %EXTRA%
echo.
echo FreeForGem stopped (exit code %ERRORLEVEL%).
pause
