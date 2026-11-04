$ErrorActionPreference = "Stop"

Write-Host "RMS Memory MCP - Installation Script" -ForegroundColor Cyan
Write-Host "------------------------------------" -ForegroundColor Cyan

# Determine Architecture
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -eq "AMD64") {
    $target = "x86_64-pc-windows-msvc"
} else {
    Write-Host "Unsupported architecture: $arch. Currently only AMD64 (x86_64) is supported on Windows." -ForegroundColor Red
    exit 1
}

Write-Host "Detected target: $target"

$updateUrl = if ($env:RMS_MEMORY_UPDATE_URL) { $env:RMS_MEMORY_UPDATE_URL.TrimEnd('/') } else { "https://update.rms-ds.com" }
$product = "rms-memory-mcp"
$repo = "max-ramas/rms-memory-mcp"
$tempZip = Join-Path $env:TEMP "rms-memory.zip"
$tempExtracted = Join-Path $env:TEMP "rms-memory-extracted"

function Get-FromUpdateCenter {
    # Size and sha256 come from the release manifest served by the update center.
    # `rms-memory update` later verifies the minisign signature of that manifest.
    $latest = Invoke-RestMethod -Uri "$updateUrl/v1/$product/latest?channel=stable&platform=windows&arch=amd64&format=zip" -UseBasicParsing
    $manifest = Invoke-RestMethod -Uri $latest.release_url -UseBasicParsing
    $entry = $manifest.artifacts | Where-Object { $_.platform -eq "windows" -and $_.arch -eq "amd64" -and $_.format -eq "zip" } | Select-Object -First 1
    if (-not $entry -or $entry.file -match '[\\/]') { throw "no Windows build in the release" }
    Write-Host "Downloading $updateUrl/dl/$product/$($latest.version)/$($entry.file)..."
    Invoke-WebRequest -Uri "$updateUrl/dl/$product/$($latest.version)/$($entry.file)" -OutFile $tempZip -UseBasicParsing
    $hash = (Get-FileHash -Algorithm SHA256 -Path $tempZip).Hash.ToLower()
    if ($hash -ne $entry.sha256.ToLower()) { Remove-Item $tempZip -Force; throw "checksum mismatch" }
    Write-Host "Installed version: $($latest.version) (from the RMS update center)"
}

function Get-FromGitHub {
    Write-Host "Falling back to GitHub Releases..."
    $response = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing
    $tag = $response.tag_name
    if ($tag -notmatch '^v[0-9]') {
        Write-Host "Error: release tag '$tag' is not a version tag (expected vX.Y.Z)." -ForegroundColor Red
        exit 1
    }
    $version = $tag.TrimStart('v')
    $downloadUrl = "https://github.com/$repo/releases/download/$tag/rms_memory_mcp_${version}_$target.zip"
    Write-Host "Downloading $downloadUrl..."
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip
}

try { Get-FromUpdateCenter } catch {
    Write-Host "Update center unavailable or no matching build: $($_.Exception.Message)" -ForegroundColor Yellow
    Get-FromGitHub
}

if (Test-Path $tempExtracted) {
    Remove-Item -Recurse -Force $tempExtracted
}
New-Item -ItemType Directory -Path $tempExtracted | Out-Null

Write-Host "Extracting..."
Expand-Archive -Path $tempZip -DestinationPath $tempExtracted -Force

$binDir = Join-Path $env:USERPROFILE ".rms-memory\bin"
if (-not (Test-Path $binDir)) {
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
}

Write-Host "Installing to $binDir"
$exePath = Join-Path $tempExtracted "rms-memory.exe"
Copy-Item -Path $exePath -Destination $binDir -Force

# Clean up
Remove-Item -Path $tempZip -Force
Remove-Item -Recurse -Force $tempExtracted

# Add to User PATH if not already present
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($userPath -notmatch [regex]::Escape($binDir)) {
    Write-Host "Adding $binDir to User PATH environment variable..."
    $newPath = $userPath + ";" + $binDir
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
    $env:PATH = $env:PATH + ";" + $binDir
    Write-Host "PATH updated successfully. You may need to restart your terminal to use 'rms-memory' globally." -ForegroundColor Yellow
}

Write-Host "Installation successful! Future updates: rms-memory update" -ForegroundColor Green
Write-Host "Running rms-memory install to hook into IDEs..." -ForegroundColor Cyan

& (Join-Path $binDir "rms-memory.exe") install
