# FreeForGemPear -- see LICENSE
# Windows build of cmfd-miner-ffgp.exe (see build-engine-ffgp.sh): upstream Common Foundry at FreeForgeMiner's pinned commit
# + FreeForgeMiner's patches + cmfd\patches\ (MIT), static CRT.  Needs Visual Studio C++ build tools, Rust (MSVC), git.
#   .\cmfd\build-engine-ffgp.ps1 [-Out DIR]
param([string]$Out = '')
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$lock = @{}
Get-Content (Join-Path $here 'ENGINE.lock') | Where-Object { $_ -match '^[A-Z0-9_]+=' } | ForEach-Object { $k, $v = $_ -split '=', 2; $lock[$k] = $v }
if (-not $Out) { $Out = Join-Path $here '..\dist\cmfd-engine-ffgp\windows' }
$work = Join-Path $here '..\dist\cmfd-src'
New-Item -ItemType Directory -Force -Path $work, $Out | Out-Null
$ffm = Join-Path $work 'freeforgeminer'
if (-not (Test-Path $ffm)) { git clone -q https://github.com/pepsykolya/freeforgeminer.git $ffm }
git -C $ffm checkout -q --detach $lock.ENGINE_COMMIT
$bs = Get-Content (Join-Path $ffm 'build.sh')
$upstream = ($bs | Where-Object { $_ -match '^UPSTREAM=' }) -replace '^UPSTREAM=', ''
$ucommit = ($bs | Where-Object { $_ -match '^UPSTREAM_COMMIT=' }) -replace '^UPSTREAM_COMMIT=', ''
$src = Join-Path $work 'CommonFoundry-ffgp'
if (-not (Test-Path $src)) { git clone -q --filter=blob:none $upstream $src }
git -C $src am --abort 2>$null
git -C $src checkout -q --detach $ucommit
$patches = @(Get-ChildItem (Join-Path $ffm 'patches\*.patch') | Sort-Object Name | ForEach-Object FullName) + @(Get-ChildItem (Join-Path $here 'patches\*.patch') | Sort-Object Name | ForEach-Object FullName)
git -C $src -c user.name=build -c user.email=build@local am -q @patches
if ($LASTEXITCODE -ne 0) { throw 'patches do not apply' }
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -products * -latest -property installationPath
Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Push-Location $src
try {
    $env:CMFD_BUILD_SOURCE_COMMIT = (git rev-parse HEAD).Trim()
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    cargo build --release -p cmfd-miner --features production-mainnet
    if ($LASTEXITCODE -ne 0) { throw "cargo failed: $LASTEXITCODE" }
} finally { Pop-Location }
Copy-Item (Join-Path $src 'target\release\cmfd-miner.exe') (Join-Path $Out 'cmfd-miner-ffgp.exe')
Write-Host "built: $Out\cmfd-miner-ffgp.exe"
