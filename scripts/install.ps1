# Install the Parakeet engine into Talon on Windows.
# 1. Junction-links this repo's plugin\ into %APPDATA%\talon\user\parakeet.
# 2. Downloads the prebuilt sidecar binary from the latest GitHub Release.
#    If the download fails, falls back to `cargo build --release`.
#    Pass -Build (or set FORCE_BUILD=1) to always build from source.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\install.ps1 [-Build]

param(
    [switch]$Build
)

$ErrorActionPreference = "Stop"

$GhRepo = if ($env:PARAKEET_GH_REPO) { $env:PARAKEET_GH_REPO } else { "fmcurti/parakeet-talon" }

$scriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoDir    = (Resolve-Path (Join-Path $scriptDir "..")).Path
$pluginSrc  = Join-Path $repoDir "plugin"
$sidecarDir = Join-Path $repoDir "sidecar-rs"
$talonUser  = Join-Path $env:APPDATA "talon\user"
$target     = Join-Path $talonUser "parakeet"
$binOut     = Join-Path $sidecarDir "target\release\parakeet-sidecar.exe"

$forceBuild = $Build.IsPresent -or ($env:FORCE_BUILD -eq "1")

if (-not (Test-Path (Join-Path $env:APPDATA "talon"))) {
    Write-Error "Talon directory not found at $env:APPDATA\talon. Install Talon first."
}

New-Item -ItemType Directory -Force -Path $talonUser | Out-Null

# --- Junction plugin\ into Talon's user dir ---
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

# --- Prebuilt binary ---
function Install-Prebuilt {
    $asset = "parakeet-sidecar-windows-x86_64.zip"
    $url   = "https://github.com/$GhRepo/releases/latest/download/$asset"
    $tmp   = New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP ("parakeet-install-" + [guid]::NewGuid()))
    Write-Host "fetching $url"
    try {
        $prev = $ProgressPreference
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile (Join-Path $tmp $asset)
        $ProgressPreference = $prev
    } catch {
        Write-Host "  prebuilt not available (repo may have no release yet)"
        Remove-Item -Recurse -Force $tmp
        return $false
    }
    # Optional checksum.
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile (Join-Path $tmp "$asset.sha256") -ErrorAction Stop
        $expected = (Get-Content (Join-Path $tmp "$asset.sha256") | Select-Object -First 1).Split()[0].ToLower()
        $actual   = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
        if ($expected -ne $actual) {
            Write-Error "checksum mismatch for $asset"
        }
    } catch {
        # No checksum file; continue without verification.
    }
    $releaseDir = Join-Path $sidecarDir "target\release"
    New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $releaseDir -Force
    Remove-Item -Recurse -Force $tmp
    Write-Host "installed prebuilt binary: $binOut"
    return $true
}

function Build-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Error "cargo not on PATH. Either install Rust from https://rustup.rs, or ensure the prebuilt release is reachable."
    }
    Write-Host "building sidecar (cargo build --release)"
    Push-Location $sidecarDir
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally {
        Pop-Location
    }
}

if ($forceBuild) {
    Build-FromSource
} else {
    if (-not (Install-Prebuilt)) {
        Build-FromSource
    }
}

if (-not (Test-Path $binOut)) {
    Write-Error "expected binary at $binOut"
}

Write-Host ""
Write-Host "done."
Write-Host "binary: $binOut"
Write-Host "restart Talon to activate. On first run the sidecar downloads ~2.5 GB of model weights."
