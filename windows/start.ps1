#requires -Version 5.1
# FreeForGemPear -- see LICENSE
# Runs freeforgempear.exe (one process drives all selected GPUs), shows its output in this window, writes it to
# logs\miner-<date>.log and restarts the miner 15 s after any exit.
[CmdletBinding()]
param(
    [string]$Wallet,
    [string]$Pool = 'pearl-eu2.luckypool.io:3360',
    [string]$Worker = $env:COMPUTERNAME,
    [string]$Devices = '',
    [string]$AutoUv = '',
    [string]$Password = '',
    [string]$LockCclock = '',
    [string]$LockMclock = '',
    [string]$Moffset = ''
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
Set-Location -LiteralPath $root

function Fail([string]$message) { Write-Host "ERROR: $message" -ForegroundColor Red; exit 1 }

if ($root -match '\s|[^\x00-\x7F]') {
    Write-Host "WARNING: the folder path contains spaces or non-English letters: $root" -ForegroundColor Yellow
    Write-Host "         If the miner fails to start, move the folder to e.g. C:\FreeForGemPear" -ForegroundColor Yellow
}
if ([string]::IsNullOrWhiteSpace($Wallet)) {
    Write-Host 'WALLET is empty. Right-click start.bat -> Edit, and set WALLET to your PRL address (prl1...).'
    $Wallet = (Read-Host 'Or paste the wallet address now').Trim()
}
$Wallet = ($Wallet -replace '\s', '')
if ($Wallet -cnotmatch '^[0-9A-Za-z]{20,100}(\+[0-9A-Za-z]{20,100})?$') { Fail 'WALLET does not look like a PRL address (prl1...), or PRL+NOCK for LuckyPool (prl1...+NOCK_ADDRESS).' }
if ($Wallet.Split('+')[0] -ceq 'prl1pjcsjdpjzhccamllqmy6fncpjy526nrrza626y27zsa6hwqzt03ssfw8fyr') {
    Write-Host '************************************************************************' -ForegroundColor Yellow
    Write-Host ' WALLET is still the developer address from the example: you are mining' -ForegroundColor Yellow
    Write-Host ' to the developer, not to yourself. Right-click the .bat -> Edit and put' -ForegroundColor Yellow
    Write-Host ' YOUR prl1... address into WALLET. Starting in 15 s (Ctrl+C to stop)...' -ForegroundColor Yellow
    Write-Host '************************************************************************' -ForegroundColor Yellow
    Start-Sleep -Seconds 15
}
if ($Pool -cnotmatch '^[A-Za-z0-9._-]+:[0-9]{1,5}$') { Fail 'POOL must look like HOST:PORT, e.g. pearl-eu2.luckypool.io:3360.' }
if ($Wallet.Contains('+') -and $Pool -notmatch '(^|\.)luckypool\.io:') {
    Write-Host 'NOTE: PRL+NOCK merge mining works on LuckyPool only: on this pool the miner mines PRL to the prl1... part and ignores +NOCK.' -ForegroundColor Yellow
}
if ([string]::IsNullOrWhiteSpace($Worker)) { $Worker = 'rig' }
$Worker = ($Worker.Trim() -replace '[^A-Za-z0-9._-]', '-')
if ($Worker.Length -gt 32) { $Worker = $Worker.Substring(0, 32) }
if ($Devices -and $Devices -notmatch '^\s*\d+(\s*,\s*\d+)*\s*$') { Fail 'DEVICES must be empty or a list like 0,1,3.' }
$Devices = ($Devices -replace '\s', '')

foreach ($file in 'freeforgempear.exe', 'pearlgpu.dll') {
    if (-not (Test-Path -LiteralPath (Join-Path $root $file))) { Fail "$file is missing - unpack the whole archive again." }
}
$smi = Get-Command nvidia-smi.exe -ErrorAction SilentlyContinue
if (-not $smi) { Fail 'nvidia-smi.exe not found - install the NVIDIA driver (version 525 or newer).' }
Write-Host "FreeForGemPear for Windows - pool $Pool, wallet $($Wallet.Substring(0,[Math]::Min(8,$Wallet.Length)))..., worker $Worker"
& $smi.Source --query-gpu=index,name,memory.free --format=csv,noheader | ForEach-Object { Write-Host "  GPU $_" }

$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
Get-ChildItem -LiteralPath $logs -Filter 'miner-*.log' | Sort-Object LastWriteTime -Descending | Select-Object -Skip 9 |
    Remove-Item -Force -ErrorAction SilentlyContinue

$exe = Join-Path $root 'freeforgempear.exe'
$cmdLine = '"{0}" --pool {1} --wallet {2} --worker {3}' -f $exe, $Pool, $Wallet, $Worker
if ($Devices) { $cmdLine += " --devices $Devices" }
if ($Password -and $Password -cmatch '^[A-Za-z0-9=._-]{1,64}$') { $cmdLine += " --password $Password" }
if ($LockCclock -match '^[0-9]+(,[0-9]*)*$') { $cmdLine += " --lock-cclock $LockCclock" }
if ($LockMclock -match '^[0-9]+(,[0-9]*)*$') { $cmdLine += " --lock-mclock $LockMclock" }
if ($Moffset -match '^[+-]?[0-9]+(,[+-]?[0-9]*)*$') { $cmdLine += " --moffset $Moffset" }
if ($AutoUv -eq '1') {
    $cmdLine += ' --auto-uv'
}
if ($AutoUv -eq '1' -or $LockCclock -or $LockMclock -or $Moffset) {
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) { Write-Host 'WARNING: AUTO_UV / clock settings need Administrator: run start.bat as Administrator, otherwise the miner cannot change the GPU and will mine at the current clocks.' -ForegroundColor Yellow }
}
$log = Join-Path $logs ("miner-{0}.log" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
$writer = New-Object IO.StreamWriter($log, $true, (New-Object Text.UTF8Encoding($false)))
$writer.AutoFlush = $true
Write-Host "Mining. Press Ctrl+C to stop. Log: $log"
try {
    while ($true) {
        $writer.WriteLine(("--- {0} start" -f (Get-Date -Format 's')))
        # cmd merges stderr into stdout, so PowerShell sees plain text lines (the miner logs to both)
        & cmd.exe /d /c "$cmdLine 2>&1" | ForEach-Object {
            $line = "$_"
            $writer.WriteLine($line)
            $color = if ($line -match 'ERROR|error|!!|panick|DISCONNECTED|rej [1-9]') { 'Red' } elseif ($line -match 'devfee') { 'DarkGray' } else { $null }
            if ($color) { Write-Host $line -ForegroundColor $color } else { Write-Host $line }
        }
        $code = $LASTEXITCODE
        $writer.WriteLine(("--- {0} miner exited (code {1})" -f (Get-Date -Format 's'), $code))
        Write-Host ("Miner exited (code {0}), restarting in 15 s. Ctrl+C to stop." -f $code) -ForegroundColor Red
        Start-Sleep -Seconds 15
    }
} finally {
    $writer.Dispose()
}
