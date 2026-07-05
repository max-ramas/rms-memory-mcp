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

$repo = "max-ramas/rms-memory-mcp"
$latestReleaseUrl = "https://api.github.com/repos/$repo/releases/latest"

Write-Host "Fetching latest release information..."
$response = Invoke-RestMethod -Uri $latestReleaseUrl -UseBasicParsing
$tag = $response.tag_name

if (-not $tag) {
    Write-Host "Error: Could not determine latest release from GitHub." -ForegroundColor Red
    exit 1
}

Write-Host "Latest release: $tag"

$downloadUrl = "https://github.com/$repo/releases/download/$tag/rms-memory-$target.zip"
Write-Host "Downloading $downloadUrl..."

$tempZip = Join-Path $env:TEMP "rms-memory.zip"
$tempExtracted = Join-Path $env:TEMP "rms-memory-extracted"

Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip

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

Write-Host "Installation successful!" -ForegroundColor Green
Write-Host "Running rms-memory install to hook into IDEs..." -ForegroundColor Cyan

& (Join-Path $binDir "rms-memory.exe") install
