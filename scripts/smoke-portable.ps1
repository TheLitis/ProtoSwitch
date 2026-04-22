param(
    [string]$Version
)

$ErrorActionPreference = 'Stop'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot

if (-not $Version) {
    $cargoMetadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $package = $cargoMetadata.packages | Where-Object { $_.name -eq 'protoswitch' } | Select-Object -First 1
    $Version = $package.version
}

$portableZip = Join-Path $repoRoot "dist\$Version\protoswitch-portable-win-x64.zip"
$workRoot = Join-Path $env:TEMP "ProtoSwitch-smoke-$Version"
$exePath = Join-Path $workRoot 'ProtoSwitch\protoswitch.exe'

if (Test-Path $workRoot) {
    Remove-Item -Recurse -Force $workRoot
}

Expand-Archive -Path $portableZip -DestinationPath $workRoot -Force

& $exePath --version
& $exePath init --non-interactive --no-autostart
& $exePath doctor
& $exePath status --plain
& $exePath autostart install
& $exePath autostart remove
