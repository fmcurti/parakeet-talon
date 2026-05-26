# Install the local STT engines into Talon on Windows.
# 1. Junction-links this repo's plugin\ into %APPDATA%\talon\user\parakeet.
# 2. Downloads the prebuilt sidecar binaries (parakeet + qwen) from the latest
#    GitHub Release. If a prebuilt is missing, falls back to `cargo build
#    --release` (which builds the whole workspace).
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
$releaseDir = Join-Path $sidecarDir "target\release"
$talonUser  = Join-Path $env:APPDATA "talon\user"
$target     = Join-Path $talonUser "parakeet"

# Sidecar binaries to install (one per engine).
$Bins = @("parakeet-sidecar", "qwen-sidecar")

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

# --- Prebuilt binary (per engine) ---
function Install-Prebuilt {
    param([string]$Bin)
    $asset = "$Bin-windows-x86_64.zip"
    $url   = "https://github.com/$GhRepo/releases/latest/download/$asset"
    $tmp   = New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP ("parakeet-install-" + [guid]::NewGuid()))
    Write-Host "fetching $url"
    try {
        $prev = $ProgressPreference
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile (Join-Path $tmp $asset)
        $ProgressPreference = $prev
    } catch {
        Write-Host "  prebuilt not available for $Bin"
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
    New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $releaseDir -Force
    Remove-Item -Recurse -Force $tmp
    Write-Host "installed prebuilt binary: $(Join-Path $releaseDir ($Bin + '.exe'))"
    return $true
}

function Build-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Error "cargo not on PATH. Either install Rust from https://rustup.rs, or ensure the prebuilt release is reachable."
    }
    Write-Host "building sidecars (cargo build --release)"
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
    $needBuild = $false
    foreach ($bin in $Bins) {
        if (-not (Install-Prebuilt -Bin $bin)) { $needBuild = $true }
    }
    if ($needBuild) {
        Write-Host "one or more prebuilt binaries unavailable; building from source"
        Build-FromSource
    }
}

# --- Report what we ended up with ---
$present = @()
$missing = @()
foreach ($bin in $Bins) {
    if (Test-Path (Join-Path $releaseDir ($bin + ".exe"))) { $present += $bin } else { $missing += $bin }
}
if ($present.Count -eq 0) {
    Write-Error "no sidecar binaries were installed under $releaseDir"
}

Write-Host ""
Write-Host "done."
foreach ($bin in $present) {
    Write-Host "binary: $(Join-Path $releaseDir ($bin + '.exe'))"
}
if ($missing.Count -gt 0) {
    Write-Host "note: missing $($missing -join ', ') (that engine won't appear in Talon)"
}
Write-Host "restart Talon, then pick an engine from the tray menu."
Write-Host "on first use each engine downloads its model: ~2.5 GB Parakeet, ~1.7 GB Qwen."
