param(
    [string]$Version,
    [ValidateSet('CurrentUser', 'AllUsers', 'Both')]
    [string]$Mode = 'CurrentUser',
    [switch]$AllowDirtyEnvironment,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-ExistingInstallations {
    $roots = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )

    $entries = foreach ($root in $roots) {
        Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
            Where-Object {
                $_.PSObject.Properties.Match('DisplayName').Count -gt 0 -and $_.DisplayName -eq 'ProtoSwitch'
            } |
            Select-Object DisplayName, DisplayVersion, InstallLocation, PSPath
    }

    return @($entries | Where-Object { $null -ne $_ })
}

function Test-StartupShortcut {
    $startupDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Startup'
    $shortcut = Join-Path $startupDir 'ProtoSwitch.lnk'
    $legacy = Join-Path $startupDir 'ProtoSwitch.cmd'
    return (Test-Path $shortcut) -or (Test-Path $legacy)
}

function Test-ScheduledTask {
    $taskName = 'ProtoSwitch Watcher'
    $process = Start-Process -FilePath 'cmd.exe' `
        -ArgumentList '/c', "schtasks /Query /TN ""$taskName"" >nul 2>&1" `
        -Wait `
        -PassThru `
        -WindowStyle Hidden
    return $process.ExitCode -eq 0
}

function Assert-CleanEnvironment {
    $protoswitchProcesses = @(Get-Process protoswitch -ErrorAction SilentlyContinue)
    if ($protoswitchProcesses.Count -gt 0) {
        throw 'Smoke installer expects a clean machine: protoswitch.exe is already running.'
    }

    $installs = @(Get-ExistingInstallations)
    if ($installs.Count -gt 0) {
        throw 'Smoke installer expects a clean machine: existing ProtoSwitch installer entry found.'
    }

    if (Test-StartupShortcut) {
        throw 'Smoke installer expects a clean machine: existing ProtoSwitch startup shortcut found.'
    }

    if (Test-ScheduledTask) {
        throw 'Smoke installer expects a clean machine: existing ProtoSwitch scheduled task found.'
    }
}

function Backup-Directory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$BackupPath
    )

    if (Test-Path $Path) {
        Move-Item -LiteralPath $Path -Destination $BackupPath
        return $true
    }

    return $false
}

function Restore-Directory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$BackupPath
    )

    if (Test-Path $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }

    if (Test-Path $BackupPath) {
        Move-Item -LiteralPath $BackupPath -Destination $Path
    }
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [string]$WorkingDirectory = $repoRoot
    )

    if ($DryRun) {
        Write-Host "[dry-run] $FilePath $($Arguments -join ' ')"
        return ''
    }

    $output = & $FilePath @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        $rendered = ($output | Out-String).Trim()
        throw "$FilePath failed with exit code $LASTEXITCODE. $rendered"
    }

    return ($output | Out-String)
}

function Invoke-Process {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    if ($DryRun) {
        Write-Host "[dry-run] $FilePath $($Arguments -join ' ')"
        return
    }

    $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -Wait -PassThru -NoNewWindow
    if ($process.ExitCode -ne 0) {
        throw "$FilePath failed with exit code $($process.ExitCode)"
    }
}

function Assert-PathExists {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if ($DryRun) {
        Write-Host "[dry-run] assert exists $Path"
        return
    }

    if (-not (Test-Path $Path)) {
        throw "Expected path not found: $Path"
    }
}

function Assert-AutostartState {
    param(
        [Parameter(Mandatory = $true)]
        [pscustomobject]$DoctorReport,
        [Parameter(Mandatory = $true)]
        [bool]$ExpectedInstalled
    )

    if ([bool]$DoctorReport.autostart.installed -ne $ExpectedInstalled) {
        throw "Unexpected autostart state. Expected $ExpectedInstalled, got $($DoctorReport.autostart.installed)."
    }
}

function Invoke-SmokeRun {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('CurrentUser', 'AllUsers')]
        [string]$Scope,
        [Parameter(Mandatory = $true)]
        [string]$InstallerPath,
        [Parameter(Mandatory = $true)]
        [string]$TempRoot
    )

    $scopeRoot = Join-Path $TempRoot $Scope
    $installRoot = Join-Path $scopeRoot 'install-root'
    $installDir = Join-Path $installRoot 'ProtoSwitch'
    $installLog = Join-Path $scopeRoot 'install.log'
    $uninstallLog = Join-Path $scopeRoot 'uninstall.log'
    $exePath = Join-Path $installDir 'protoswitch.exe'
    $uninstallExe = Join-Path $installDir 'unins000.exe'

    if (-not $DryRun) {
        New-Item -ItemType Directory -Path $scopeRoot -Force | Out-Null
    }

    $installerArgs = @(
        '/VERYSILENT',
        '/SUPPRESSMSGBOXES',
        '/NORESTART',
        '/SP-',
        '/TASKS=autostart',
        "/DIR=$installDir",
        "/LOG=$installLog"
    )

    if ($Scope -eq 'CurrentUser') {
        $installerArgs += '/CURRENTUSER'
    } else {
        $installerArgs += '/ALLUSERS'
    }

    Write-Host "Installer smoke: $Scope"
    Invoke-Process -FilePath $InstallerPath -Arguments $installerArgs

    foreach ($path in @(
        $exePath,
        (Join-Path $installDir 'README.md'),
        (Join-Path $installDir 'CHANGELOG.md'),
        (Join-Path $installDir 'QUICKSTART.txt'),
        $uninstallExe
    )) {
        Assert-PathExists -Path $path
    }

    Invoke-External -FilePath $exePath -Arguments @('--version') | Out-Null

    $doctorJson = Invoke-External -FilePath $exePath -Arguments @('doctor', '--json')
    $doctor = if ($DryRun) { $null } else { $doctorJson | ConvertFrom-Json }
    if (-not $DryRun) {
        if (-not $doctor.config_exists) {
            throw 'Installer smoke expected config.toml to exist after silent init.'
        }
        Assert-AutostartState -DoctorReport $doctor -ExpectedInstalled $true
    }

    $statusJson = Invoke-External -FilePath $exePath -Arguments @('status', '--json')
    $status = if ($DryRun) { $null } else { $statusJson | ConvertFrom-Json }
    if (-not $DryRun) {
        if ($status.config.app_version -ne $Version) {
            throw "Unexpected app version in status output: $($status.config.app_version)"
        }
    }

    Invoke-External -FilePath $exePath -Arguments @('autostart', 'remove') | Out-Null
    $doctorAfterRemoveJson = Invoke-External -FilePath $exePath -Arguments @('doctor', '--json')
    $doctorAfterRemove = if ($DryRun) { $null } else { $doctorAfterRemoveJson | ConvertFrom-Json }
    if (-not $DryRun) {
        Assert-AutostartState -DoctorReport $doctorAfterRemove -ExpectedInstalled $false
    }

    Invoke-External -FilePath $exePath -Arguments @('autostart', 'install') | Out-Null
    $doctorAfterInstallJson = Invoke-External -FilePath $exePath -Arguments @('doctor', '--json')
    $doctorAfterInstall = if ($DryRun) { $null } else { $doctorAfterInstallJson | ConvertFrom-Json }
    if (-not $DryRun) {
        Assert-AutostartState -DoctorReport $doctorAfterInstall -ExpectedInstalled $true
    }

    Invoke-Process -FilePath $uninstallExe -Arguments @(
        '/VERYSILENT',
        '/SUPPRESSMSGBOXES',
        '/NORESTART',
        "/LOG=$uninstallLog"
    )

    if (-not $DryRun) {
        if (Test-Path $installDir) {
            $leftovers = @(Get-ChildItem -LiteralPath $installDir -Force -ErrorAction SilentlyContinue)
            if ($leftovers.Count -gt 0) {
                throw "Installer smoke found leftover files after uninstall in $installDir"
            }
        }

        if (Test-StartupShortcut) {
            throw 'Installer smoke expected startup shortcut to be removed after uninstall.'
        }

        if (Test-ScheduledTask) {
            throw 'Installer smoke expected scheduled task to be removed after uninstall.'
        }
    }
}

if (-not $Version) {
    $Version = Get-PackageVersion
}

$installerPath = Join-Path $repoRoot "dist\$Version\ProtoSwitch-Setup-x64.exe"
if (-not (Test-Path $installerPath)) {
    throw "Installer artifact not found: $installerPath. Build distribution first."
}

$tempRoot = Join-Path $env:TEMP "ProtoSwitch-installer-smoke-$Version"
$configDir = Join-Path $env:APPDATA 'ProtoSwitch'
$localStateDir = Join-Path $env:LOCALAPPDATA 'ProtoSwitch'
$configBackup = Join-Path $tempRoot 'backup-config'
$stateBackup = Join-Path $tempRoot 'backup-local'
$configMoved = $false
$stateMoved = $false

if ($Mode -eq 'AllUsers' -or $Mode -eq 'Both') {
    $isAdmin = Test-IsAdministrator
    if ($Mode -eq 'AllUsers' -and -not $isAdmin) {
        throw 'AllUsers smoke requires an elevated PowerShell session.'
    }
}

if (-not $AllowDirtyEnvironment) {
    Assert-CleanEnvironment
}

if (-not $DryRun) {
    if (Test-Path $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }

    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    $configMoved = Backup-Directory -Path $configDir -BackupPath $configBackup
    $stateMoved = Backup-Directory -Path $localStateDir -BackupPath $stateBackup
}

try {
    switch ($Mode) {
        'CurrentUser' {
            Invoke-SmokeRun -Scope 'CurrentUser' -InstallerPath $installerPath -TempRoot $tempRoot
        }
        'AllUsers' {
            Invoke-SmokeRun -Scope 'AllUsers' -InstallerPath $installerPath -TempRoot $tempRoot
        }
        'Both' {
            Invoke-SmokeRun -Scope 'CurrentUser' -InstallerPath $installerPath -TempRoot $tempRoot
            if (Test-IsAdministrator) {
                Invoke-SmokeRun -Scope 'AllUsers' -InstallerPath $installerPath -TempRoot $tempRoot
            } else {
                Write-Warning 'Skipping AllUsers smoke because the current PowerShell session is not elevated.'
            }
        }
    }
}
finally {
    if (-not $DryRun) {
        Restore-Directory -Path $configDir -BackupPath $configBackup
        Restore-Directory -Path $localStateDir -BackupPath $stateBackup

        if (-not $configMoved -and (Test-Path $configDir)) {
            Remove-Item -LiteralPath $configDir -Recurse -Force
        }

        if (-not $stateMoved -and (Test-Path $localStateDir)) {
            Remove-Item -LiteralPath $localStateDir -Recurse -Force
        }
    }
}

Write-Host 'Installer smoke completed.'
