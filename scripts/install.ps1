# Install the Parakeet engine into Talon on Windows.
# Junction-links this repo's plugin/ into %APPDATA%\talon\user\parakeet and builds the Rust sidecar.
#
# Run from any directory:   powershell -ExecutionPolicy Bypass -File scripts\install.ps1

$ErrorActionPreference = "Stop"

$scriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoDir    = (Resolve-Path (Join-Path $scriptDir "..")).Path
$pluginSrc  = Join-Path $repoDir "plugin"
$sidecarDir = Join-Path $repoDir "sidecar-rs"
$talonUser  = Join-Path $env:APPDATA "talon\user"
$target     = Join-Path $talonUser "parakeet"

if (-not (Test-Path (Join-Path $env:APPDATA "talon"))) {
    Write-Error "Talon directory not found at $env:APPDATA\talon. Install Talon first."
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not on PATH. Install Rust from https://rustup.rs and re-run."
}

New-Item -ItemType Directory -Force -Path $talonUser | Out-Null

if (Test-Path $target) {
    $item = Get-Item $target -Force
    $isLink = ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
    $currentTarget = if ($isLink) { $item.Target | Select-Object -First 1 } else { $null }

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

Write-Host "building sidecar (cargo build --release)"
Push-Location $sidecarDir
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

$bin = Join-Path $sidecarDir "target\release\parakeet-sidecar.exe"
if (-not (Test-Path $bin)) {
    Write-Error "expected binary at $bin"
}

Write-Host ""
Write-Host "done."
Write-Host "binary: $bin"
Write-Host "restart Talon to activate. On first run the sidecar downloads ~480 MB of model weights."
