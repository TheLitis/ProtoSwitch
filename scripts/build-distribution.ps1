param(
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8NoBom
[Console]::InputEncoding = $utf8NoBom
[Console]::OutputEncoding = $utf8NoBom

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot
$cargoMetadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$package = $cargoMetadata.packages | Where-Object { $_.name -eq 'protoswitch' } | Select-Object -First 1

if (-not $package) {
    throw 'Package protoswitch not found in cargo metadata.'
}

$version = $package.version
$versionInfoVersion = if ($version -match '^(\d+)\.(\d+)\.(\d+)-beta\.(\d+)$') {
    "$($Matches[1]).$($Matches[2]).$($Matches[3]).$($Matches[4])"
} elseif ($version -match '^(\d+)\.(\d+)\.(\d+)$') {
    "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"
} else {
    throw "Unsupported version format: $version"
}
$releaseDir = Join-Path $repoRoot 'target\release'
$distRoot = Join-Path $repoRoot "dist\$version"
$portableZip = Join-Path $distRoot 'protoswitch-portable-win-x64.zip'
$quickstart = Join-Path $repoRoot 'packaging\windows\QUICKSTART.txt'
$portablePackager = Join-Path $repoRoot 'scripts\package-portable.py'
$installerScript = Join-Path $repoRoot 'packaging\windows\ProtoSwitch.iss'

function Resolve-Iscc {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        'C:\Program Files\Inno Setup 6\ISCC.exe'
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw 'ISCC.exe not found. Install JRSoftware.InnoSetup via winget or add ISCC.exe to PATH.'
}

Write-Host "Building ProtoSwitch $version release binary..."
cargo build --release --locked
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

if (Test-Path $distRoot) {
    Remove-Item -Recurse -Force $distRoot
}

Write-Host 'Creating portable zip...'
python $portablePackager `
    --repo-root $repoRoot `
    --version $version `
    --platform win `
    --arch x64 `
    --binary (Join-Path $releaseDir 'protoswitch.exe') `
    --format zip
if ($LASTEXITCODE -ne 0) {
    throw "portable packaging failed with exit code $LASTEXITCODE"
}

if (-not $SkipInstaller) {
    $iscc = Resolve-Iscc
    Write-Host "Building installer with $iscc ..."
    & $iscc `
        "/DAppVersion=$version" `
        "/DAppNumericVersion=$versionInfoVersion" `
        "/DRepoRoot=$repoRoot" `
        "/DReleaseDir=$releaseDir" `
        "/DOutputDir=$distRoot" `
        $installerScript
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC build failed with exit code $LASTEXITCODE"
    }
}

Write-Host 'Artifacts:'
Get-ChildItem $distRoot | ForEach-Object { Write-Host $_.FullName }
