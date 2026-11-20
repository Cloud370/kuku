param(
    [string]$Source = "https://github.com/Cloud370/kuku/releases/latest/download/latest.json",
    [string]$Version = "",
    [switch]$Force,
    [switch]$NoModifyPath
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
$installId = [guid]::NewGuid().ToString()
$stage = New-Item -ItemType Directory (Join-Path $binDir ".install-$installId")
$cacheFile = Join-Path $stage.FullName $archiveName
try {
    Write-Host "Downloading $url ..."
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $cacheFile
    if ($sha256) {
        Write-Host "Verifying checksum..."
        $stream = [System.IO.File]::OpenRead($cacheFile)
        $digest = [System.Security.Cryptography.SHA256]::Create()
        try {
            $actual = [BitConverter]::ToString($digest.ComputeHash($stream)).Replace("-", "").ToLowerInvariant()
        } finally {
            $digest.Dispose()
            $stream.Dispose()
        }
        if ($actual -ne $sha256) {
            throw "Checksum mismatch: expected $sha256, got $actual"
        }
    }

    Write-Host "Installing in $binDir..."
    $stagedExe = Join-Path $stage.FullName "kuku.exe"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($cacheFile)
    try {
        $entry = $archive.GetEntry("kuku.exe")
        if (-not $entry) { throw "Archive is missing kuku.exe" }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $stagedExe)
    } finally {
        $archive.Dispose()
    }
    if (Test-Path $kukuExe) {
        $retired = Join-Path $binDir ".kuku-$installId.retired"
        [System.IO.File]::Replace($stagedExe, $kukuExe, $retired)
    } else {
        [System.IO.File]::Move($stagedExe, $kukuExe)
    }
    # Windows releases a running service's old image after that service exits.
    Get-ChildItem -LiteralPath $binDir -Filter ".kuku-*.retired" -File |
        Remove-Item -Force -ErrorAction SilentlyContinue
    Move-Item -LiteralPath $cacheFile -Destination (Join-Path $cacheDir $archiveName) -Force
} finally {
    Remove-Item -LiteralPath $stage.FullName -Recurse -Force
}

# ── PATH check ──
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $NoModifyPath -and $binDir -notin ($userPath -split ';')) {
    $newPath = "$binDir;$userPath"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$binDir;$env:Path"
    Write-Host "Added kuku to user PATH"
}

Write-Host ""
Write-Host "kuku $manifestVersion installed successfully."
Write-Host ""
Write-Host "  kuku --help          Show usage"
Write-Host "  kuku --version       Show version"
Write-Host "  kuku init            Create the user configuration"
Write-Host "  kuku                 Open the terminal client in this directory"
Write-Host ""
Write-Host "Docs: https://kuku.run"
