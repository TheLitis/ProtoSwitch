param(
    [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8NoBom
[Console]::InputEncoding = $utf8NoBom
[Console]::OutputEncoding = $utf8NoBom

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot

function Get-PackageVersion {
    $cargoMetadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $package = $cargoMetadata.packages | Where-Object { $_.name -eq 'protoswitch' } | Select-Object -First 1
    if (-not $package) {
        throw 'Package protoswitch not found in cargo metadata.'
    }

    return $package.version
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $output = & $FilePath @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        $rendered = ($output | Out-String).Trim()
        throw "$FilePath failed with exit code $LASTEXITCODE. $rendered"
    }

    return ($output | Out-String).Trim()
}

function Assert-ContainsText {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Text,
        [Parameter(Mandatory = $true)]
        [string]$Label,
        [Parameter(Mandatory = $true)]
        [string]$Expected
    )

    if (-not $Text.Contains($Expected)) {
        throw "$Label does not contain expected text: $Expected"
    }
}

function Decode-Utf8Literal {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Base64
    )

    return [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Base64))
}

if (-not $Version) {
    $Version = Get-PackageVersion
}

$portableZip = Join-Path $repoRoot "dist\$Version\protoswitch-portable-win-x64.zip"
$workRoot = Join-Path $env:TEMP "ProtoSwitch-smoke-$Version"
$stageRoot = Join-Path $workRoot 'ProtoSwitch'
$exePath = Join-Path $stageRoot 'protoswitch.exe'

if (Test-Path $workRoot) {
    Remove-Item -Recurse -Force $workRoot
}

Expand-Archive -Path $portableZip -DestinationPath $workRoot -Force

Assert-ContainsText -Text (Get-Content -Raw -Encoding utf8 (Join-Path $stageRoot 'README.md')) -Label 'README.md' -Expected (Decode-Utf8Literal '0KfRgtC+INCV0YHRgtGMINCh0LXQudGH0LDRgQ==')
Assert-ContainsText -Text (Get-Content -Raw -Encoding utf8 (Join-Path $stageRoot 'CHANGELOG.md')) -Label 'CHANGELOG.md' -Expected (Decode-Utf8Literal '0JLRgdC1INC30LDQvNC10YLQvdGL0LUg0LjQt9C80LXQvdC10L3QuNGP')
Assert-ContainsText -Text (Get-Content -Raw -Encoding utf8 (Join-Path $stageRoot 'QUICKSTART.txt')) -Label 'QUICKSTART.txt' -Expected (Decode-Utf8Literal '0JHRi9GB0YLRgNGL0Lkg0YHRgtCw0YDRgg==')

$versionOutput = Invoke-External -FilePath $exePath -Arguments @('--version')
Invoke-External -FilePath $exePath -Arguments @('init', '--non-interactive', '--no-autostart') | Out-Null
$doctorOutput = Invoke-External -FilePath $exePath -Arguments @('doctor')
$statusOutput = Invoke-External -FilePath $exePath -Arguments @('status', '--plain')

Assert-ContainsText -Text $doctorOutput -Label 'doctor' -Expected (Decode-Utf8Literal '0KHRgtCw0YLRg9GBIHByb3h5')
Assert-ContainsText -Text $doctorOutput -Label 'doctor' -Expected (Decode-Utf8Literal '0KHRgtCw0YLRg9GBINC40YHRgtC+0YfQvdC40LrQsA==')
Assert-ContainsText -Text $statusOutput -Label 'status' -Expected (Decode-Utf8Literal '0KHRgtCw0YLRg9GBIHByb3h5')

Invoke-External -FilePath $exePath -Arguments @('autostart', 'install') | Out-Null
Invoke-External -FilePath $exePath -Arguments @('autostart', 'remove') | Out-Null
Invoke-External -FilePath $exePath -Arguments @('shutdown') | Out-Null

Write-Host 'Portable smoke completed.'
