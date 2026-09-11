param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$tauriConfigPath = Join-Path $workspaceRoot "src-tauri\tauri.conf.json"
$packagePath = Join-Path $workspaceRoot "package.json"
$uiPackagePath = Join-Path $workspaceRoot "apps\desktop-ui\package.json"
$cargoManifestPath = Join-Path $workspaceRoot "Cargo.toml"

$tauriConfig = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
$package = Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json
$uiPackage = Get-Content -LiteralPath $uiPackagePath -Raw | ConvertFrom-Json
$cargoManifest = Get-Content -LiteralPath $cargoManifestPath -Raw
$cargoVersionMatch = [regex]::Match($cargoManifest, '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')

if (-not $cargoVersionMatch.Success) {
    throw "Unable to read the workspace version from Cargo.toml."
}

$version = [string]$tauriConfig.version
$versions = @(
    [string]$package.version,
    [string]$uiPackage.version,
    [string]$cargoVersionMatch.Groups[1].Value
)
if ($versions | Where-Object { $_ -ne $version }) {
    throw "Release versions are inconsistent. Expected every manifest to use $version."
}
if ($version -notmatch '-beta\.') {
    throw "Refusing to prepare an unsigned release whose version is not marked as Beta: $version"
}

Push-Location $workspaceRoot
try {
    if (-not $SkipBuild) {
        & npm.cmd test
        if ($LASTEXITCODE -ne 0) { throw "Tests failed." }
        & npm.cmd run package:windows
        if ($LASTEXITCODE -ne 0) { throw "Windows packaging failed." }
    }

    $bundleDirectory = Join-Path $workspaceRoot "target\release\bundle\nsis"
    $installer = Get-ChildItem -LiteralPath $bundleDirectory -Filter "ScreenHarbor_${version}_*_setup.exe" -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if (-not $installer) {
        $installer = Get-ChildItem -LiteralPath $bundleDirectory -Filter "ScreenHarbor_${version}_*.exe" -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTimeUtc -Descending |
            Select-Object -First 1
    }
    if (-not $installer) {
        throw "No ScreenHarbor $version NSIS installer was found in $bundleDirectory."
    }

    $signatureStatus = "Unavailable"
    try {
        Import-Module Microsoft.PowerShell.Security -ErrorAction Stop
        $signatureStatus = [string](Get-AuthenticodeSignature -LiteralPath $installer.FullName).Status
    }
    catch {
        Write-Warning "Authenticode status could not be inspected in this PowerShell host: $($_.Exception.Message)"
    }
    if ($signatureStatus -ne "NotSigned" -and $signatureStatus -ne "Unavailable") {
        Write-Warning "Expected an unsigned Beta installer, but signature status is $signatureStatus."
    }

    $releaseDirectory = Join-Path $workspaceRoot "release\$version"
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $releaseInstaller = Join-Path $releaseDirectory $installer.Name
    Copy-Item -LiteralPath $installer.FullName -Destination $releaseInstaller -Force

    $hash = (Get-FileHash -LiteralPath $releaseInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksumPath = "$releaseInstaller.sha256"
    Set-Content -LiteralPath $checksumPath -Value "$hash  $($installer.Name)" -Encoding ascii

    $manifest = [ordered]@{
        product = "ScreenHarbor"
        version = $version
        channel = "beta"
        platform = "windows-x86_64"
        installer = $installer.Name
        sizeBytes = (Get-Item -LiteralPath $releaseInstaller).Length
        sha256 = $hash
        signatureStatus = $signatureStatus
        createdAtUtc = [DateTime]::UtcNow.ToString("o")
    }
    $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $releaseDirectory "release-manifest.json") -Encoding utf8
    Copy-Item -LiteralPath (Join-Path $workspaceRoot "CHANGELOG.md") -Destination $releaseDirectory -Force
    Copy-Item -LiteralPath (Join-Path $workspaceRoot "PRIVACY.md") -Destination $releaseDirectory -Force
    Copy-Item -LiteralPath (Join-Path $workspaceRoot "LICENSE.txt") -Destination $releaseDirectory -Force
    Copy-Item -LiteralPath (Join-Path $workspaceRoot "README.md") -Destination $releaseDirectory -Force

    Write-Host "Beta release prepared: $releaseDirectory"
    Write-Host "Installer: $($installer.Name)"
    Write-Host "SHA-256: $hash"
    Write-Host "Signature: $signatureStatus"
}
finally {
    Pop-Location
}
