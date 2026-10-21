@echo off
rem FreeForGemPear -- see LICENSE
rem Advanced start file, "the old way": nvidia-smi locks the GPU clocks and sets a power limit, then the miner runs
rem in a restart loop and writes logs\ffgp.log. Run it AS ADMINISTRATOR (right-click -> Run as administrator):
rem nvidia-smi -lgc / -lmc / -pl and the undervolt tuner (--auto-uv) all need it. See MANUAL.md, section 5 (Windows).
setlocal EnableExtensions
title FreeForGemPear - nvidia-smi clocks + power limit

rem ================================================================
rem  EDIT THESE VALUES
rem ================================================================
rem Your PRL wallet address (prl1...). REPLACE the address below with YOUR OWN:
rem as shipped it is the developer's address, and everything you mine would go there.
rem LuckyPool PRL+NOCK merge mining: append "+" and your NOCK address, e.g.  set "WALLET=prl1...+YOUR_NOCK_ADDRESS"
set "WALLET=prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr"
rem Stratum pool HOST:PORT (see MANUAL.md, section "Pools")
set "POOL=pearl-eu2.luckypool.io:3360"
rem Worker (rig) name: letters, numbers, dots, underscores, hyphens
set "WORKER=%COMPUTERNAME%"
rem Driver clock lock and power limit for ALL GPUs. The values below are for an RTX 3070 -- CHANGE THEM for your card
rem (see MANUAL.md, section 7), or make a value empty to leave that setting alone.
rem Core clock lock, MHz (nvidia-smi -lgc):
set "CCLOCK=1665"
rem Memory clock lock, MHz (nvidia-smi -lmc):
set "MCLOCK=810"
rem Power limit, W (nvidia-smi -pl):
set "PLIMIT=165"
rem Undervolt auto-tuner: 1 = on, empty = off
set "AUTO_UV=1"
rem Offset range the tuner tries, MHz. Empty = defaults per card (start from the current offset or +120, RTX 40 +200,
rem RTX 50 +300; up to +495, RTX 20xx/GTX 16xx/CMP 50HX +225, CMP 30HX/40HX +150). RTX 3070 example: 135 and 360.
rem One value for all GPUs, or a comma list in GPU order (or --devices order), e.g. UV_MAX=225,495,150 (- = default).
set "UV_MIN=135"
set "UV_MAX=360"
rem MHz per tuning step (empty = 15; 30 = about half the tuning time, coarser result):
set "UV_STEP="
rem Any other miner options, e.g. --devices 0,1
set "EXTRA="
rem ================================================================
rem To release the clock locks without a reboot:  nvidia-smi -rgc  and  nvidia-smi -rmc

net session >nul 2>&1 || echo WARNING: not running as Administrator: nvidia-smi and the undervolt tuner cannot change the GPU.
if defined CCLOCK nvidia-smi -lgc %CCLOCK%
if defined MCLOCK nvidia-smi -lmc %MCLOCK%
if defined PLIMIT nvidia-smi -pl %PLIMIT%

cd /d "%~dp0"
if not exist logs mkdir logs
set "ARGS=--pool %POOL% --wallet %WALLET% --worker %WORKER%"
if "%AUTO_UV%"=="1" set "ARGS=%ARGS% --auto-uv"
if "%AUTO_UV%"=="1" if defined UV_MIN set "ARGS=%ARGS% --uv-min %UV_MIN%"
if "%AUTO_UV%"=="1" if defined UV_MAX set "ARGS=%ARGS% --uv-max %UV_MAX%"
if "%AUTO_UV%"=="1" if defined UV_STEP set "ARGS=%ARGS% --uv-step %UV_STEP%"
if defined EXTRA set "ARGS=%ARGS% %EXTRA%"

:loop
powershell -NoProfile -Command "& { .\freeforgempear.exe %ARGS% | Tee-Object -Append -FilePath '%~dp0logs\ffgp.log' }"
echo Miner exited; restarting in 10 s (Ctrl+C to stop).
timeout /t 10
goto loop
