param(
    [string]$Source = "https://github.com/Cloud370/kuku/releases/latest/download/latest.json",
    [string]$Version = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

# ── Resolve kuku_home ──
$kukuHome = if ($env:KUKU_HOME) { $env:KUKU_HOME } else { Join-Path $env:USERPROFILE ".kuku" }
$binDir = Join-Path $kukuHome "bin"
$cacheDir = Join-Path $kukuHome "cache"
New-Item -ItemType Directory -Force -Path $binDir, $cacheDir | Out-Null

$platform = "windows-x86_64"

# ── Fetch manifest ──
Write-Host "Fetching latest release info..."
$manifest = Invoke-RestMethod -Uri $Source
$manifestVersion = $manifest.version

if ($Version -and $manifestVersion -ne $Version) {
    Write-Host "Requested version $Version but manifest has $manifestVersion"
    exit 1
}

# ── Check current version ──
$kukuExe = Join-Path $binDir "kuku.exe"
if ((Test-Path $kukuExe) -and -not $Force) {
    try {
        $current = & $kukuExe --version 2>$null | ForEach-Object { ($_ -split '\s+')[1] }
    } catch { $current = "" }
    if ($current -eq $manifestVersion) {
        Write-Host "kuku $current is already the latest version."
        exit 0
    }
    Write-Host "Updating kuku $current -> $manifestVersion"
} else {
    Write-Host "Installing kuku $manifestVersion"
}

# ── Download ──
$platformInfo = $manifest.platforms.$platform
$url = $platformInfo.url
$sha256 = $platformInfo.sha256

if (-not $url) {
    Write-Host "No download URL found for platform $platform"
    exit 1
}

$archiveName = "kuku-$platform.zip"
$cacheFile = Join-Path $cacheDir $archiveName
Write-Host "Downloading $url ..."
Invoke-WebRequest -Uri $url -OutFile $cacheFile

# ── Verify SHA256 ──
if ($sha256) {
    Write-Host "Verifying checksum..."
    $actual = (Get-FileHash -Path $cacheFile -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $sha256) {
        Write-Host "Checksum mismatch: expected $sha256, got $actual"
        Remove-Item $cacheFile -Force
        exit 1
    }
}

# ── Extract ──
Write-Host "Extracting to $binDir..."
Expand-Archive -Path $cacheFile -DestinationPath $binDir -Force

# ── PATH check ──
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$binDir*") {
    $newPath = "$binDir;$userPath"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$binDir;$env:Path"
    Write-Host "Added kuku to user PATH"
}

Write-Host "kuku $manifestVersion installed successfully."
