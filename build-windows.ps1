# FreeForGem — see LICENSE
# Builds freeforgem.exe on Windows x64 and packs dist\freeforgem-<version>-windows-x64.zip with the prebuilt GPU libraries
# (prebuilt\windows-x64\pearlgpu.dll for Pearl, noidgpu.dll for NOID, proxgpu.dll for PROX, nebgpu.dll + neb-datasets.bin for NEB;
# the kernels are closed source).
# Needs: Visual Studio 2022 (or Build Tools) with the C++ workload, Rust (rustup, MSVC toolchain), git. No CUDA toolkit.
# usage: .\build-windows.ps1
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$lib = "$PSScriptRoot\prebuilt\windows-x64"
if (-not (Test-Path "$lib\pearlgpu.lib") -or -not (Test-Path "$lib\pearlgpu.dll")) { throw "missing $lib\pearlgpu.dll / pearlgpu.lib (prebuilt GPU library)" }
if (-not (Test-Path "$lib\noidgpu.dll")) { throw "missing $lib\noidgpu.dll (prebuilt NOID GPU library)" }
if (-not (Test-Path "$lib\proxgpu.dll")) { throw "missing $lib\proxgpu.dll (prebuilt PROX GPU library)" }
# NEB (1.8.0): the FFGN engine nebgpu.dll and the main-network dataset, pinned in prebuilt\NEB-ENGINE.lock
$pin = @{}; Get-Content "$PSScriptRoot\prebuilt\NEB-ENGINE.lock" | Where-Object { $_ -match '^([A-Z0-9_]+)=(.*)$' } | ForEach-Object { $pin[$Matches[1]] = $Matches[2].Trim('"') }
$nebset = if ($env:FFG_NEB_DATASET) { $env:FFG_NEB_DATASET } else { "$PSScriptRoot\prebuilt\neb-datasets.bin" }
if (-not (Test-Path "$lib\nebgpu.dll")) { throw "missing $lib\nebgpu.dll (prebuilt NEB engine)" }
if (-not (Test-Path -LiteralPath $nebset)) { throw "missing $nebset (NEB dataset $($pin['NEB_DATASET_URL']); or set FFG_NEB_DATASET)" }
if ((Get-FileHash "$lib\nebgpu.dll" -Algorithm SHA256).Hash.ToLower() -ne $pin['NEBGPU_DLL_SHA256']) { throw "$lib\nebgpu.dll is not the pinned NEB engine $($pin['NEBGPU_VERSION'])" }
if ((Get-FileHash -LiteralPath $nebset -Algorithm SHA256).Hash.ToLower() -ne $pin['NEB_DATASET_SHA256']) { throw "$nebset is not the pinned NEB dataset" }
# Ting / DLR (1.8.0, the FFGT module): tinggpu.dll (CUDA) + tinggpu_cl.dll (OpenCL), pinned by prebuilt\windows-x64\tinggpu.sha256
foreach ($l in Get-Content "$lib\tinggpu.sha256") {
    $h, $f = $l -split '\s+\*?', 2
    if (-not (Test-Path "$lib\$f")) { throw "missing $lib\$f (prebuilt Ting GPU library)" }
    $got = (Get-FileHash "$lib\$f" -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $h) { throw "$lib\$f is not the pinned build (sha256 $got, tinggpu.sha256: $h)" }
}

$vswhere = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -products * -latest -property installationPath
if (-not $vs) { throw 'Visual Studio / Build Tools with the C++ workload not found' }
Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$version = (Select-String -Path host\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

Write-Host '== freeforgem.exe'
# static MSVC CRT: the only runtime dependency is the NVIDIA driver (nvcuda.dll); pearlgpu.dll is built the same way
# PROX dev fee: the pool secret is NOT in the source, it is compiled in from FFG_FEE_PASS_PROX (devfee.rs, option_env!); taken from
# the environment, else from ~\.secrets\prox-devfee\pool-secret. $env:FFG_ALLOW_NO_PROX_FEE = '1' = dev build WITHOUT the PROX fee (never release it). Never printed.
if (-not $env:FFG_FEE_PASS_PROX) {
    $sf = if ($env:FFG_PROX_SECRET_FILE) { $env:FFG_PROX_SECRET_FILE } else { "$env:USERPROFILE\.secrets\prox-devfee\pool-secret" }
    if (Test-Path -LiteralPath $sf) { $env:FFG_FEE_PASS_PROX = (Get-Content -LiteralPath $sf -Raw).Trim() }
}
if (-not $env:FFG_FEE_PASS_PROX) {
    if ($env:FFG_ALLOW_NO_PROX_FEE -eq '1') { Write-Warning 'dev build WITHOUT the PROX dev-fee secret (FFG_ALLOW_NO_PROX_FEE=1) - do not release this package' }
    else { throw 'PROX dev-fee secret not found (FFG_FEE_PASS_PROX or ~\.secrets\prox-devfee\pool-secret): a release without it would ship a miner without the PROX fee; for a dev build set $env:FFG_ALLOW_NO_PROX_FEE = "1"' }
}
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$env:PEARLGPU_DIR = $lib
Push-Location host
try { cargo build --release --locked; if ($LASTEXITCODE -ne 0) { throw "cargo failed: $LASTEXITCODE" } } finally { Pop-Location }

$name = "freeforgem-$version-windows-x64"
$stage = "dist\$name\freeforgem"
Remove-Item -Recurse -Force "dist\$name" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item host\target\release\freeforgem.exe, "$lib\pearlgpu.dll", "$lib\noidgpu.dll", "$lib\proxgpu.dll", "$lib\nebgpu.dll", "$lib\tinggpu.dll", "$lib\tinggpu_cl.dll", windows\start*.bat, windows\start.ps1, `
    windows\README-WINDOWS.txt, LICENSE, NOTICE, README.md, HIVEOS.md, MANUAL.md $stage
Copy-Item -LiteralPath $nebset "$stage\neb-datasets.bin"
# CMFD engine (FreeForgeMiner release pinned in cmfd\ENGINE.lock, MIT) as cmfd\; $env:FFGP_NO_CMFD = '1' builds Pearl only
# $env:FFGP_CMFD_ZIP = a local copy of the pinned release zip (its SHA-256 is still checked), for an engine release not published yet
if ($env:FFGP_NO_CMFD -ne '1') { & "$PSScriptRoot\cmfd\fetch-engine.ps1" -Dest "$stage\cmfd" -Zip "$env:FFGP_CMFD_ZIP" }
# AMD (OpenCL, RDNA2+): search library pearlgpu_amd.dll; for CMFD the OpenCL worker cmfd-v4-replay-ocl.exe and the pool client
# rebuilt with cmfd\patches\ (cmfd-miner-ffgp.exe, MIT; cmfd\build-engine-ffgp.ps1).  Prebuilt in prebuilt\windows-x64.
# amduv.exe: AMD clock/voltage tuner for --auto-uv (ADLX; restores the owner's settings when the miner exits)
if (Test-Path "$lib\pearlgpu_amd.dll") { Copy-Item "$lib\pearlgpu_amd.dll" $stage }
if (Test-Path "$lib\amduv.exe") { Copy-Item "$lib\amduv.exe" $stage }
if ($env:FFGP_NO_CMFD -ne '1' -and (Test-Path "$lib\cmfd\cmfd-v4-replay-ocl.exe")) {
    Copy-Item "$lib\cmfd\cmfd-v4-replay-ocl.exe", "$lib\cmfd\cmfd-miner-ffgp.exe" "$stage\cmfd"
}
$zip = "dist\$name.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "dist\$name\freeforgem" -DestinationPath $zip
$hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([IO.File]::ReadAllBytes((Resolve-Path $zip)))).Replace('-', '').ToLower()
"$hash  $name.zip" | Tee-Object "dist\$name.zip.sha256"
