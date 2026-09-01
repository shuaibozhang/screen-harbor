param(
    [string]$ArchivePath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-Sha256([string]$Path) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
}

$releaseTag = "autobuild-2026-08-29-13-12"
$archiveName = "ffmpeg-n9.0.1-11-ge47273f4d9-win64-lgpl-shared-9.0.zip"
$archiveSha256 = "e452726c9282e9d8b640dd29f012db91bf6133b490b5490a37e8b0a4d3ec7ae9"
$distributionName = "ffmpeg-n9.0.1-11-ge47273f4d9-win64-lgpl-shared-9.0"
$downloadUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$releaseTag/$archiveName"
$ffmpegSourceName = "ffmpeg-source-e47273f4d9.tar.gz"
$ffmpegSourceUrl = "https://github.com/FFmpeg/FFmpeg/archive/e47273f4d9.tar.gz"
$ffmpegSourceSha256 = "6491dae95e3cf3cdbac02933b55860e782b0c4f0a6bd8f37cef30fded259283c"
$buildSourceName = "ffmpeg-build-recipes-autobuild-2026-08-29-13-12.tar.gz"
$buildSourceUrl = "https://github.com/BtbN/FFmpeg-Builds/archive/refs/tags/autobuild-2026-08-29-13-12.tar.gz"
$buildSourceSha256 = "b5b047817ce469c847a7324011f02c540f1acc3660cfc03c9429ceb05ebe6ca4"
$expectedRuntimeFiles = @(
    "ffmpeg.exe",
    "avcodec-63.dll",
    "avdevice-63.dll",
    "avfilter-12.dll",
    "avformat-63.dll",
    "avutil-61.dll",
    "swresample-7.dll",
    "swscale-10.dll",
    "LICENSE.txt",
    "THIRD-PARTY-NOTICES.md",
    $ffmpegSourceName,
    $buildSourceName,
    "manifest.json"
)

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$runtimeDirectory = Join-Path $workspaceRoot "src-tauri\resources\ffmpeg"
$noticePath = Join-Path $workspaceRoot "third_party\ffmpeg\NOTICE.md"

if (Test-Path -LiteralPath $runtimeDirectory) {
    $missing = @($expectedRuntimeFiles | Where-Object {
        -not (Test-Path -LiteralPath (Join-Path $runtimeDirectory $_) -PathType Leaf)
    })
    $manifestPath = Join-Path $runtimeDirectory "manifest.json"
    if ($missing.Count -eq 0) {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ($manifest.archiveSha256 -eq $archiveSha256 -and $manifest.releaseTag -eq $releaseTag) {
            $invalidFiles = @()
            foreach ($entry in $manifest.files.PSObject.Properties) {
                $filePath = Join-Path $runtimeDirectory $entry.Name
                $actualFileHash = Get-Sha256 $filePath
                if ($actualFileHash -ne $entry.Value) {
                    $invalidFiles += $entry.Name
                }
            }
            if ($invalidFiles.Count -eq 0) {
                Write-Host "FFmpeg runtime is already provisioned and verified."
                exit 0
            }
            throw "Existing FFmpeg runtime failed per-file SHA-256 verification. Remove only '$runtimeDirectory' and rerun provisioning."
        }
    }
    throw "Existing FFmpeg runtime is incomplete or has a different version. Remove only '$runtimeDirectory' and rerun provisioning."
}

if (-not (Test-Path -LiteralPath $noticePath -PathType Leaf)) {
    throw "FFmpeg third-party notice is missing: $noticePath"
}

$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("frameflow-ffmpeg-" + [guid]::NewGuid().ToString("N"))
$stagingDirectory = Join-Path (Split-Path -Parent $runtimeDirectory) ("ffmpeg.staging-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temporaryRoot | Out-Null

try {
    $localArchive = Join-Path $temporaryRoot $archiveName
    if ($ArchivePath) {
        $resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
        Copy-Item -LiteralPath $resolvedArchive -Destination $localArchive
    } else {
        Write-Host "Downloading pinned FFmpeg LGPL runtime..."
        Invoke-WebRequest -Uri $downloadUrl -OutFile $localArchive
    }

    $actualSha256 = Get-Sha256 $localArchive
    if ($actualSha256 -ne $archiveSha256) {
        throw "FFmpeg archive SHA-256 mismatch. Expected $archiveSha256, received $actualSha256."
    }

    $ffmpegSourcePath = Join-Path $temporaryRoot $ffmpegSourceName
    $buildSourcePath = Join-Path $temporaryRoot $buildSourceName
    Write-Host "Downloading corresponding FFmpeg source and build recipes..."
    Invoke-WebRequest -Uri $ffmpegSourceUrl -OutFile $ffmpegSourcePath
    Invoke-WebRequest -Uri $buildSourceUrl -OutFile $buildSourcePath
    if ((Get-Sha256 $ffmpegSourcePath) -ne $ffmpegSourceSha256) {
        throw "FFmpeg source archive SHA-256 mismatch."
    }
    if ((Get-Sha256 $buildSourcePath) -ne $buildSourceSha256) {
        throw "FFmpeg build recipe archive SHA-256 mismatch."
    }

    $extractDirectory = Join-Path $temporaryRoot "extract"
    Expand-Archive -LiteralPath $localArchive -DestinationPath $extractDirectory
    $distributionDirectory = Join-Path $extractDirectory $distributionName
    $sourceBin = Join-Path $distributionDirectory "bin"
    $sourceExecutable = Join-Path $sourceBin "ffmpeg.exe"
    if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
        throw "Pinned FFmpeg archive does not contain the expected runtime layout."
    }

    $versionOutput = (& $sourceExecutable -version 2>&1 | Out-String)
    if ($versionOutput -notmatch "n9\.0\.1-11-ge47273f4d9-20260829") {
        throw "FFmpeg runtime version does not match the pinned release."
    }
    if ($versionOutput -match "--enable-gpl" -or $versionOutput -match "--enable-nonfree") {
        throw "FFmpeg runtime unexpectedly enables GPL or nonfree components."
    }
    $encoderOutput = (& $sourceExecutable -hide_banner -encoders 2>&1 | Out-String)
    foreach ($encoder in @("h264_mf", "aac_mf", "libvpx-vp9", "libopus", "libwebp")) {
        if ($encoderOutput -notmatch "\b$encoder\b") {
            throw "FFmpeg runtime does not provide required encoder '$encoder'."
        }
    }

    New-Item -ItemType Directory -Path $stagingDirectory | Out-Null
    foreach ($file in $expectedRuntimeFiles | Where-Object { $_ -like "*.dll" -or $_ -eq "ffmpeg.exe" }) {
        Copy-Item -LiteralPath (Join-Path $sourceBin $file) -Destination (Join-Path $stagingDirectory $file)
    }
    Copy-Item -LiteralPath (Join-Path $distributionDirectory "LICENSE.txt") -Destination (Join-Path $stagingDirectory "LICENSE.txt")
    Copy-Item -LiteralPath $noticePath -Destination (Join-Path $stagingDirectory "THIRD-PARTY-NOTICES.md")
    Copy-Item -LiteralPath $ffmpegSourcePath -Destination (Join-Path $stagingDirectory $ffmpegSourceName)
    Copy-Item -LiteralPath $buildSourcePath -Destination (Join-Path $stagingDirectory $buildSourceName)

    $fileHashes = [ordered]@{}
    Get-ChildItem -LiteralPath $stagingDirectory -File | Sort-Object Name | ForEach-Object {
        $fileHashes[$_.Name] = Get-Sha256 $_.FullName
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        releaseTag = $releaseTag
        ffmpegVersion = "n9.0.1-11-ge47273f4d9-20260829"
        variant = "win64-lgpl-shared-9.0"
        archiveName = $archiveName
        archiveSha256 = $archiveSha256
        downloadUrl = $downloadUrl
        sourceArchives = [ordered]@{
            ffmpeg = [ordered]@{ name = $ffmpegSourceName; sha256 = $ffmpegSourceSha256; url = $ffmpegSourceUrl }
            buildRecipes = [ordered]@{ name = $buildSourceName; sha256 = $buildSourceSha256; url = $buildSourceUrl }
        }
        files = $fileHashes
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stagingDirectory "manifest.json") -Encoding utf8
    Move-Item -LiteralPath $stagingDirectory -Destination $runtimeDirectory
    Write-Host "Provisioned FFmpeg LGPL runtime at $runtimeDirectory"
}
finally {
    $resolvedTemp = [System.IO.Path]::GetFullPath($temporaryRoot)
    $systemTemp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if ($resolvedTemp.StartsWith($systemTemp, [System.StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedTemp)) {
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
    if (Test-Path -LiteralPath $stagingDirectory) {
        $resolvedStaging = [System.IO.Path]::GetFullPath($stagingDirectory)
        $resolvedResources = [System.IO.Path]::GetFullPath((Split-Path -Parent $runtimeDirectory))
        if ($resolvedStaging.StartsWith($resolvedResources, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
        }
    }
}
