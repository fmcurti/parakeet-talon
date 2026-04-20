# Install the Parakeet engine into Talon on Windows.
# Junction-links this repo's plugin/ into %APPDATA%\talon\user\parakeet and sets up a venv.
#
# Run from any directory:   powershell -ExecutionPolicy Bypass -File scripts\install.ps1

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoDir   = (Resolve-Path (Join-Path $scriptDir "..")).Path
$pluginSrc = Join-Path $repoDir "plugin"
$talonUser = Join-Path $env:APPDATA "talon\user"
$target    = Join-Path $talonUser "parakeet"

if (-not (Test-Path (Join-Path $env:APPDATA "talon"))) {
    Write-Error "Talon directory not found at $env:APPDATA\talon. Install Talon first."
}

$python = Get-Command python -ErrorAction SilentlyContinue
if (-not $python) { $python = Get-Command py -ErrorAction SilentlyContinue }
if (-not $python) {
    Write-Error "python not on PATH. Install Python 3.10+ (https://python.org) and re-run."
}

New-Item -ItemType Directory -Force -Path $talonUser | Out-Null

if (Test-Path $target) {
    $item = Get-Item $target -Force
    $isLink = ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
    $currentTarget = $null
    if ($isLink) { $currentTarget = $item.Target | Select-Object -First 1 }

    if ($isLink -and $currentTarget -eq $pluginSrc) {
        Write-Host "link already in place: $target -> $pluginSrc"
    } else {
        $backup = "$target.bak." + [int](Get-Date -UFormat %s)
        Write-Host "moving existing $target -> $backup"
        Rename-Item -Path $target -NewName $backup
        New-Item -ItemType Junction -Path $target -Target $pluginSrc | Out-Null
        Write-Host "linked $target -> $pluginSrc"
    }
} else {
    New-Item -ItemType Junction -Path $target -Target $pluginSrc | Out-Null
    Write-Host "linked $target -> $pluginSrc"
}

$venvDir = Join-Path $pluginSrc ".venv"
$venvPy  = Join-Path $venvDir "Scripts\python.exe"
if (-not (Test-Path $venvPy)) {
    Write-Host "creating venv at $venvDir"
    & $python.Source -m venv $venvDir
}

$pip = Join-Path $venvDir "Scripts\pip.exe"
& $pip install --upgrade pip
& $pip install -r (Join-Path $pluginSrc "requirements.txt")

Write-Host ""
Write-Host "done."
Write-Host "restart Talon to activate, and select 'parakeet' in the tray Active Engine menu if needed."
