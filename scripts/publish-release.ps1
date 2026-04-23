param(
    [string]$Version,
    [switch]$Draft,
    [switch]$DryRun
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

function Read-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return [System.IO.File]::ReadAllText($Path, $utf8NoBom)
}

function Write-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

function Get-ReleaseNotes {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ChangelogPath,
        [Parameter(Mandatory = $true)]
        [string]$VersionValue
    )

    $content = Read-Utf8File -Path $ChangelogPath
    $escapedVersion = [Regex]::Escape("v$VersionValue")
    $pattern = "(?ms)^## \[$escapedVersion\][^\r\n]*\r?\n(?<body>.*?)(?=^## \[|\z)"
    $match = [Regex]::Match($content, $pattern)
    if (-not $match.Success) {
        throw "Release notes for v$VersionValue not found in CHANGELOG.md"
    }

    return $match.Groups['body'].Value.Trim()
}

function Assert-CommandAvailable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command not found: $Name"
    }
}

function Assert-GitTagExists {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag
    )

    Push-Location $repoRoot
    try {
        git rev-parse --verify "refs/tags/$Tag" *> $null
        if ($LASTEXITCODE -ne 0) {
            throw "Git tag not found: $Tag"
        }
    }
    finally {
        Pop-Location
    }
}

function Invoke-Gh {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    if ($DryRun) {
        Write-Host "[dry-run] gh $($Arguments -join ' ')"
        return ''
    }

    $output = & gh @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        $rendered = ($output | Out-String).Trim()
        throw "gh $($Arguments[0]) failed: $rendered"
    }

    return ($output | Out-String)
}

function Test-ReleaseExists {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag
    )

    if ($DryRun) {
        return $false
    }

    try {
        $null = & gh release view $Tag --json tagName 2>$null
        return $LASTEXITCODE -eq 0
    }
    catch {
        return $false
    }
}

function Assert-ReleaseBodyEncoding {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag,
        [Parameter(Mandatory = $true)]
        [string[]]$ExpectedAssets,
        [Parameter(Mandatory = $true)]
        [bool]$ExpectedPrerelease
    )

    if ($DryRun) {
        return
    }

    $releaseJson = Invoke-Gh -Arguments @('release', 'view', $Tag, '--json', 'body,isPrerelease,name,assets,url')
    $release = $releaseJson | ConvertFrom-Json

    if ([string]::IsNullOrEmpty($release.body)) {
        throw "Release $Tag has an empty body."
    }

    if ([int][char]$release.body[0] -eq 65279) {
        throw "Release $Tag body starts with a UTF-8 BOM marker."
    }

    if ([bool]$release.isPrerelease -ne $ExpectedPrerelease) {
        throw "Unexpected prerelease flag for $Tag."
    }

    $assetNames = @($release.assets | ForEach-Object { $_.name })
    foreach ($expectedName in $ExpectedAssets) {
        if ($assetNames -notcontains $expectedName) {
            throw "Release $Tag is missing asset $expectedName."
        }
    }
}

if (-not $Version) {
    $Version = Get-PackageVersion
}

$tag = "v$Version"
$title = "ProtoSwitch $tag"
$isPrerelease = $Version.Contains('-')
$distDir = Join-Path $repoRoot "dist\$Version"
$changelogPath = Join-Path $repoRoot 'CHANGELOG.md'
$assetFiles = @(Get-ChildItem $distDir -File | Sort-Object Name)
$assetNames = @($assetFiles | ForEach-Object { $_.Name })
$assetPaths = @($assetFiles | ForEach-Object { $_.FullName })

foreach ($path in @($changelogPath) + $assetPaths) {
    if (-not (Test-Path $path)) {
        throw "Required file not found: $path"
    }
}

if ($assetPaths.Count -eq 0) {
    throw "No release assets found in $distDir"
}

Assert-CommandAvailable -Name 'gh'
if (-not $DryRun) {
    Invoke-Gh -Arguments @('auth', 'status') | Out-Null
}

Assert-GitTagExists -Tag $tag

$notes = Get-ReleaseNotes -ChangelogPath $changelogPath -VersionValue $Version
$notesFile = Join-Path $env:TEMP "ProtoSwitch-release-notes-$Version.md"
Write-Utf8File -Path $notesFile -Content $notes

Write-Host "Prepared release notes: $notesFile"
Write-Host "Publishing $tag from $distDir"

if (Test-ReleaseExists -Tag $tag) {
    $editArgs = @('release', 'edit', $tag, '--title', $title, '--notes-file', $notesFile)
    if ($isPrerelease) {
        $editArgs += '--prerelease'
    }
    if ($Draft) {
        $editArgs += '--draft'
    }
    Invoke-Gh -Arguments $editArgs | Out-Null
    Invoke-Gh -Arguments (@('release', 'upload', $tag) + $assetPaths + @('--clobber')) | Out-Null
} else {
    $createArgs = @('release', 'create', $tag) + $assetPaths + @('--title', $title, '--notes-file', $notesFile)
    if ($isPrerelease) {
        $createArgs += '--prerelease'
    }
    if ($Draft) {
        $createArgs += '--draft'
    }
    Invoke-Gh -Arguments $createArgs | Out-Null
}

Assert-ReleaseBodyEncoding -Tag $tag -ExpectedAssets $assetNames -ExpectedPrerelease $isPrerelease

Write-Host "Release $tag is ready."
