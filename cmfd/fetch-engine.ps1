# FreeForGemPear -- see LICENSE
# Puts the CMFD engine (FreeForgeMiner release pinned in cmfd\ENGINE.lock, MIT) into -Dest (default: dist\cmfd-engine\windows):
# downloads the release zip, checks its SHA-256, copies only the engine files (not FreeForgeMiner's own launcher).
#   .\cmfd\fetch-engine.ps1 [-Dest DIR] [-Zip <local copy>]
param([string]$Dest = '', [string]$Zip = '')
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$lock = @{}
Get-Content (Join-Path $here 'ENGINE.lock') | Where-Object { $_ -match '^[A-Z0-9_]+=' } | ForEach-Object { $k, $v = $_ -split '=', 2; $lock[$k] = $v }
if (-not $Dest) { $Dest = Join-Path $here '..\dist\cmfd-engine\windows' }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ffgp-cmfd-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    if (-not $Zip) { $Zip = Join-Path $tmp 'engine.zip'; Invoke-WebRequest -UseBasicParsing -Uri $lock.WINDOWS_URL -OutFile $Zip }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Zip).Hash.ToLower()
    if ($hash -ne $lock.WINDOWS_SHA256) { throw "CMFD engine: SHA-256 mismatch for $Zip ($hash)" }
    Expand-Archive -LiteralPath $Zip -DestinationPath $tmp -Force
    $s = Join-Path $tmp 'freeforgeminer'
    Remove-Item -Recurse -Force $Dest -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path (Join-Path $Dest 'production-mainnet') | Out-Null
    foreach ($f in 'cmfd-miner.exe', 'cmfd-v4-replay.exe', 'PREPARE-V4-INPUTS.ps1', 'V4-INPUT-CHUNKS.json', 'production-v4-rcnet-1-inputs.json',
                   'FORGEMATRIX-V4-FIXED-ARTIFACT-RECORD-V1.json', 'THIRD_PARTY_NOTICES.md', 'LICENSE-CommonFoundry') {
        Copy-Item -LiteralPath (Join-Path $s $f) -Destination $Dest
    }
    Copy-Item -LiteralPath (Join-Path $s 'LICENSE') -Destination (Join-Path $Dest 'LICENSE-FreeForgeMiner')
    Copy-Item -Path (Join-Path $s 'production-mainnet\*') -Destination (Join-Path $Dest 'production-mainnet')
    Copy-Item -LiteralPath (Join-Path $here 'README.md') -Destination $Dest
    # official test vector for --self-test (MIT, Common Foundry 1.0.16)
    New-Item -ItemType Directory -Force -Path (Join-Path $Dest 'fixtures') | Out-Null
    Copy-Item -Path (Join-Path $here 'fixtures\*') -Destination (Join-Path $Dest 'fixtures')
    "$($lock.ENGINE_NAME) $($lock.ENGINE_VERSION)`nsource $($lock.WINDOWS_URL)`nsha256 $($lock.WINDOWS_SHA256)" | Set-Content -Encoding ascii (Join-Path $Dest 'ENGINE')
    Write-Host "CMFD engine $($lock.ENGINE_NAME) $($lock.ENGINE_VERSION) -> $Dest"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
