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

function Receive-Package([string]$Url, [string]$Destination) {
    $curl = Join-Path $env:SystemRoot "System32\curl.exe"
    if (Test-Path -LiteralPath $curl -PathType Leaf) {
        $progress = if ([Console]::IsErrorRedirected) { "--silent" } else { "--progress-bar" }
        & $curl --fail --location --show-error $progress --connect-timeout 15 --retry 2 --retry-max-time 30 --output $Destination -- $Url
        if ($LASTEXITCODE -ne 0) { throw "Download failed with curl exit code $LASTEXITCODE" }
        return
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Destination
            return
        } catch {
            $status = if ($_.Exception.Response) { [int]$_.Exception.Response.StatusCode } else { 0 }
            if ($attempt -eq 3 -or $status -notin @(0, 408, 429, 500, 502, 503, 504)) { throw }
            Start-Sleep -Seconds $attempt
        }
    }
}

function Get-PackageDigest([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $digest = [Security.Cryptography.SHA256]::Create()
    try {
        return [BitConverter]::ToString($digest.ComputeHash($stream)).Replace("-", "")
    } finally {
        $digest.Dispose()
        $stream.Dispose()
    }
}

# ── Download ──
$platformInfo = $manifest.platforms.$platform
$url = $platformInfo.url
$sha256 = $platformInfo.sha256
$size = $platformInfo.size
if (-not $url -or $sha256 -notmatch '^[a-fA-F0-9]{64}$') {
    throw "Missing download URL or SHA-256 checksum for $platform"
}
$archiveName = "kuku-$platform.zip"
$cached = Join-Path $cacheDir $archiveName
$installId = [guid]::NewGuid().ToString()
$stage = New-Item -ItemType Directory (Join-Path $binDir ".install-$installId")
$cacheFile = Join-Path $stage.FullName $archiveName
try {
    $cachedPackage = $false
    if (Test-Path -LiteralPath $cached -PathType Leaf) {
        Copy-Item -LiteralPath $cached -Destination $cacheFile
        $cachedPackage = (Get-PackageDigest $cacheFile) -eq $sha256
    }
    if ($cachedPackage) {
        Write-Host "Using verified cached $archiveName."
    } else {
        $sizeLabel = if ($size) { " ($size bytes)" } else { "" }
        Write-Host "Downloading $archiveName$sizeLabel ..."
        Receive-Package $url $cacheFile

        Write-Host "Verifying checksum..."
        $actual = Get-PackageDigest $cacheFile
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
        if (-not $entry -or $entry.Length -eq 0) { throw "Archive is missing kuku.exe" }
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
    if (-not $cachedPackage) {
        Move-Item -LiteralPath $cacheFile -Destination $cached -Force
    }
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
Write-Host "  kuku                 Set up a model or open the terminal client"
Write-Host ""
Write-Host "Docs: https://kuku.run"
