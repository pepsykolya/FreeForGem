# FreeForGemPear — see LICENSE
# Builds freeforgempear.exe on Windows x64, links the prebuilt GPU library (prebuilt\windows-x64\pearlgpu.lib; the kernel is closed
# source) and packs dist\freeforgempear-<version>-windows-x64.zip together with prebuilt\windows-x64\pearlgpu.dll.
# Needs: Visual Studio 2022 (or Build Tools) with the C++ workload, Rust (rustup, MSVC toolchain), git. No CUDA toolkit.
# usage: .\build-windows.ps1
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$lib = "$PSScriptRoot\prebuilt\windows-x64"
if (-not (Test-Path "$lib\pearlgpu.lib") -or -not (Test-Path "$lib\pearlgpu.dll")) { throw "missing $lib\pearlgpu.dll / pearlgpu.lib (prebuilt GPU library)" }

$vswhere = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -products * -latest -property installationPath
if (-not $vs) { throw 'Visual Studio / Build Tools with the C++ workload not found' }
Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$version = (Select-String -Path host\Cargo.toml -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value

Write-Host '== freeforgempear.exe (links prebuilt\windows-x64\pearlgpu.lib)'
# static MSVC CRT: the only runtime dependency is the NVIDIA driver (nvcuda.dll); pearlgpu.dll is built the same way
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$env:PEARLGPU_DIR = $lib
Push-Location host
try { cargo build --release --locked; if ($LASTEXITCODE -ne 0) { throw "cargo failed: $LASTEXITCODE" } } finally { Pop-Location }

$name = "freeforgempear-$version-windows-x64"
$stage = "dist\$name\freeforgempear"
Remove-Item -Recurse -Force "dist\$name" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item host\target\release\freeforgempear.exe, "$lib\pearlgpu.dll", windows\start*.bat, windows\start.ps1, `
    windows\README-WINDOWS.txt, LICENSE, NOTICE, README.md, HIVEOS.md, MANUAL.md $stage
# CMFD engine (FreeForgeMiner release pinned in cmfd\ENGINE.lock, MIT) as cmfd\; $env:FFGP_NO_CMFD = '1' builds Pearl only
if ($env:FFGP_NO_CMFD -ne '1') { & "$PSScriptRoot\cmfd\fetch-engine.ps1" -Dest "$stage\cmfd" }
# AMD (OpenCL, RDNA2+): search library pearlgpu_amd.dll; for CMFD the OpenCL worker cmfd-v4-replay-ocl.exe and the pool client
# rebuilt with cmfd\patches\ (cmfd-miner-ffgp.exe, MIT; cmfd\build-engine-ffgp.ps1).  Prebuilt in prebuilt\windows-x64.
if (Test-Path "$lib\pearlgpu_amd.dll") { Copy-Item "$lib\pearlgpu_amd.dll" $stage }
if ($env:FFGP_NO_CMFD -ne '1' -and (Test-Path "$lib\cmfd\cmfd-v4-replay-ocl.exe")) {
    Copy-Item "$lib\cmfd\cmfd-v4-replay-ocl.exe", "$lib\cmfd\cmfd-miner-ffgp.exe" "$stage\cmfd"
}
$zip = "dist\$name.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "dist\$name\freeforgempear" -DestinationPath $zip
$hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([IO.File]::ReadAllBytes((Resolve-Path $zip)))).Replace('-', '').ToLower()
"$hash  $name.zip" | Tee-Object "dist\$name.zip.sha256"
