param(
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'

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
$portableStageRoot = Join-Path $distRoot 'portable-stage'
$portableDir = Join-Path $portableStageRoot 'ProtoSwitch'
$portableZip = Join-Path $distRoot 'protoswitch-portable-win-x64.zip'
$quickstart = Join-Path $repoRoot 'packaging\windows\QUICKSTART.txt'
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

New-Item -ItemType Directory -Path $portableDir -Force | Out-Null

Copy-Item (Join-Path $releaseDir 'protoswitch.exe') $portableDir
Copy-Item (Join-Path $repoRoot 'README.md') $portableDir
Copy-Item (Join-Path $repoRoot 'CHANGELOG.md') $portableDir
Copy-Item $quickstart $portableDir

Write-Host 'Creating portable zip...'
Compress-Archive -Path (Join-Path $portableStageRoot 'ProtoSwitch') -DestinationPath $portableZip -Force
Remove-Item -Recurse -Force $portableStageRoot

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
