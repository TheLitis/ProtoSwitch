param(
    [string]$BinaryPath,
    [switch]$ConfirmLiveMutation
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8NoBom
[Console]::InputEncoding = $utf8NoBom
[Console]::OutputEncoding = $utf8NoBom

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

function Resolve-BinaryPath {
    param([string]$RequestedPath)

    if ($RequestedPath) {
        $resolved = Resolve-Path $RequestedPath -ErrorAction Stop
        return $resolved.Path
    }

    $candidates = @(
        (Join-Path $repoRoot 'target\release\protoswitch.exe'),
        (Join-Path $repoRoot 'target\debug\protoswitch.exe')
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw 'protoswitch.exe не найден. Перед live e2e соберите проект через cargo build --release.'
}

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try {
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Start-TcpAcceptJob {
    $port = Get-FreeTcpPort
    $job = Start-Job -ScriptBlock {
        param($Port)
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, [int]$Port)
        $listener.Start()
        try {
            while ($true) {
                $client = $listener.AcceptTcpClient()
                $client.Dispose()
            }
        }
        finally {
            $listener.Stop()
        }
    } -ArgumentList $port
    Start-Sleep -Milliseconds 250
    return [pscustomobject]@{
        Port = $port
        Job = $job
    }
}

function Start-HttpFixtureJob {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Body
    )

    $port = Get-FreeTcpPort
    $job = Start-Job -ScriptBlock {
        param($Port, $FixtureBody)
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, [int]$Port)
        $listener.Start()
        $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($FixtureBody)
        try {
            while ($true) {
                $client = $listener.AcceptTcpClient()
                try {
                    $stream = $client.GetStream()
                    $buffer = New-Object byte[] 2048
                    $null = $stream.Read($buffer, 0, $buffer.Length)
                    $header = [System.Text.Encoding]::ASCII.GetBytes(
                        "HTTP/1.1 200 OK`r`nContent-Type: text/plain; charset=utf-8`r`nContent-Length: $($bodyBytes.Length)`r`nConnection: close`r`n`r`n"
                    )
                    $stream.Write($header, 0, $header.Length)
                    $stream.Write($bodyBytes, 0, $bodyBytes.Length)
                    $stream.Flush()
                }
                finally {
                    $client.Dispose()
                }
            }
        }
        finally {
            $listener.Stop()
        }
    } -ArgumentList $port, $Body
    Start-Sleep -Milliseconds 250
    return [pscustomobject]@{
        Url = "http://127.0.0.1:$port/candidate.txt"
        Job = $job
    }
}

function Stop-BackgroundJob {
    param($Entry)

    if (-not $Entry) {
        return
    }

    try {
        Stop-Job -Job $Entry.Job -ErrorAction SilentlyContinue | Out-Null
    }
    finally {
        Remove-Job -Job $Entry.Job -Force -ErrorAction SilentlyContinue | Out-Null
    }
}

function Find-TelegramDataDir {
    $candidates = @()
    if ($env:APPDATA) {
        $candidates += (Join-Path $env:APPDATA 'Telegram Desktop\tdata')
    }
    if ($env:LOCALAPPDATA) {
        $candidates += (Join-Path $env:LOCALAPPDATA 'Telegram Desktop\tdata')
        $candidates += (Join-Path $env:LOCALAPPDATA 'Programs\Telegram Desktop\tdata')
    }

    foreach ($candidate in $candidates) {
        if (Test-Path (Join-Path $candidate 'settingss')) {
            return $candidate
        }
        if (Test-Path (Join-Path $candidate 'settings')) {
            return $candidate
        }
    }

    throw 'Не удалось найти Telegram Desktop tdata.'
}

function Find-TelegramExecutable {
    $running = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($running -and $running.Path) {
        return $running.Path
    }

    $candidates = @()
    if ($env:APPDATA) {
        $candidates += (Join-Path $env:APPDATA 'Telegram Desktop\Telegram.exe')
    }
    if ($env:LOCALAPPDATA) {
        $candidates += (Join-Path $env:LOCALAPPDATA 'Telegram Desktop\Telegram.exe')
        $candidates += (Join-Path $env:LOCALAPPDATA 'Programs\Telegram Desktop\Telegram.exe')
    }

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw 'Не удалось найти Telegram.exe.'
}

function Get-TelegramSettingsPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TdataDir
    )

    $modern = Join-Path $TdataDir 'settingss'
    if (Test-Path $modern) {
        return $modern
    }

    $legacy = Join-Path $TdataDir 'settings'
    if (Test-Path $legacy) {
        return $legacy
    }

    throw 'В Telegram tdata не найден settingss/settings.'
}

Add-Type -Namespace ProtoSwitchLive -Name User32 -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetForegroundWindow();
'@

function Get-ForegroundHandle {
    return [int64][ProtoSwitchLive.User32]::GetForegroundWindow()
}

function Invoke-ProtoSwitch {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,
        [Parameter(Mandatory = $true)]
        [string]$AppDataRoot,
        [Parameter(Mandatory = $true)]
        [string]$LocalAppDataRoot,
        [Parameter(Mandatory = $true)]
        [string]$ExePath
    )

    $oldAppData = $env:APPDATA
    $oldLocalAppData = $env:LOCALAPPDATA
    try {
        $env:APPDATA = $AppDataRoot
        $env:LOCALAPPDATA = $LocalAppDataRoot
        $output = & $ExePath @Arguments 2>&1
        if ($LASTEXITCODE -ne 0) {
            $rendered = ($output | Out-String).Trim()
            throw "$ExePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE. $rendered"
        }
        return ($output | Out-String).Trim()
    }
    finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }
}

function New-ProxyObject {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Port,
        [Parameter(Mandatory = $true)]
        [string]$Secret
    )

    return [ordered]@{
        kind = 'mt_proto'
        server = '127.0.0.1'
        port = $Port
        secret = $Secret
        username = $null
        password = $null
    }
}

function New-ProxyRecord {
    param(
        [Parameter(Mandatory = $true)]
        [hashtable]$Proxy,
        [Parameter(Mandatory = $true)]
        [string]$Source
    )

    return [ordered]@{
        proxy = $Proxy
        source = $Source
        captured_at = (Get-Date).ToUniversalTime().ToString('o')
    }
}

function Write-IsolatedConfig {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,
        [Parameter(Mandatory = $true)]
        [string]$Version,
        [Parameter(Mandatory = $true)]
        [string]$TelegramDataDir,
        [Parameter(Mandatory = $true)]
        [string]$FixtureUrl
    )

    $escapedDataDir = $TelegramDataDir.Replace('\', '\\')
    $escapedFixtureUrl = $FixtureUrl.Replace('\', '\\')
    $content = @"
app_version = "$Version"

[telegram]
client = "desktop"
backend_mode = "managed"
data_dir = "$escapedDataDir"

[provider]
source_url = "$escapedFixtureUrl"
fetch_attempts = 1
fetch_retry_delay_ms = 1
enable_socks5_fallback = false

[[provider.sources]]
name = "live fixture"
url = "$escapedFixtureUrl"
kind = "telegram_link_list"
enabled = true

[[provider.sources]]
name = "mtproto.ru"
url = "https://mtproto.ru/personal.php"
kind = "mtproto_ru_page"
enabled = false

[[provider.sources]]
name = "SoliSpirit MTProto"
url = "https://raw.githubusercontent.com/SoliSpirit/mtproto/master/all_proxies.txt"
kind = "telegram_link_list"
enabled = false

[[provider.sources]]
name = "Argh94 MTProto"
url = "https://raw.githubusercontent.com/Argh94/Proxy-List/main/MTProto.txt"
kind = "telegram_link_list"
enabled = false

[[provider.sources]]
name = "Proxifly SOCKS5"
url = "https://cdn.jsdelivr.net/gh/proxifly/free-proxy-list@main/proxies/protocols/socks5/data.txt"
kind = "socks5_url_list"
enabled = false

[[provider.sources]]
name = "Argh94 SOCKS5"
url = "https://raw.githubusercontent.com/Argh94/Proxy-List/main/SOCKS5.txt"
kind = "socks5_url_list"
enabled = false

[[provider.sources]]
name = "hookzof SOCKS5"
url = "https://raw.githubusercontent.com/hookzof/socks5_list/master/proxy.txt"
kind = "socks5_url_list"
enabled = false

[watcher]
check_interval_secs = 30
connect_timeout_secs = 1
failure_threshold = 1
history_size = 4
auto_cleanup_dead_proxies = true

[autostart]
enabled = false
method = "startup_folder"
"@
    Write-Utf8NoBom -Path $ConfigPath -Content $content
}

function Write-IsolatedState {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StatePath,
        [hashtable]$CurrentProxyRecord,
        [hashtable]$PendingProxyRecord,
        [bool]$TelegramRunning,
        [string]$WatcherMode = 'watching'
    )

    $state = [ordered]@{
        current_proxy = $CurrentProxyRecord
        pending_proxy = $PendingProxyRecord
        last_fetch_at = $null
        last_apply_at = $null
        current_proxy_status = ''
        source_status = ''
        backend_status = ''
        backend_route = ''
        backend_restart_required = $false
        watcher = [ordered]@{
            mode = $WatcherMode
            failure_streak = 0
            telegram_running = $TelegramRunning
            last_check_at = $null
            next_check_at = $null
        }
        recent_proxies = @()
        last_error = $null
    }

    $json = $state | ConvertTo-Json -Depth 8
    Write-Utf8NoBom -Path $StatePath -Content $json
}

if (-not $IsWindows) {
    throw 'Этот live e2e работает только на Windows.'
}

if (-not [Environment]::UserInteractive) {
    throw 'Нужна интерактивная Windows-сессия.'
}

if (-not $ConfirmLiveMutation) {
    throw 'Для live e2e нужен явный флаг -ConfirmLiveMutation.'
}

$resolvedBinary = Resolve-BinaryPath -RequestedPath $BinaryPath
$versionOutput = (& $resolvedBinary --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось получить версию через $resolvedBinary --version"
}
$version = ($versionOutput -split '\s+')[-1]

$telegramProcess = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $telegramProcess) {
    throw 'Перед live e2e нужно запустить Telegram Desktop.'
}

$telegramExe = Find-TelegramExecutable
$telegramTdata = Find-TelegramDataDir
$telegramSettings = Get-TelegramSettingsPath -TdataDir $telegramTdata

$tempRoot = Join-Path $env:TEMP "ProtoSwitch-live-e2e-$PID"
$tempAppData = Join-Path $tempRoot 'appdata'
$tempLocalAppData = Join-Path $tempRoot 'localappdata'
$configDir = Join-Path $tempAppData 'ProtoSwitch'
$localDir = Join-Path $tempLocalAppData 'ProtoSwitch'
$configPath = Join-Path $configDir 'config.toml'
$statePath = Join-Path $localDir 'state.json'
$backupPath = Join-Path $tempRoot ([System.IO.Path]::GetFileName($telegramSettings) + '.bak')
$healthyListener = $null
$replacementListener = $null
$fixture = $null

if (Test-Path $tempRoot) {
    Remove-Item -Recurse -Force $tempRoot
}

New-Item -ItemType Directory -Force -Path $configDir, (Join-Path $localDir 'logs') | Out-Null
Copy-Item $telegramSettings $backupPath -Force

try {
    $fixture = Start-HttpFixtureJob -Body 'placeholder'
    Write-IsolatedConfig -ConfigPath $configPath -Version $version -TelegramDataDir $telegramTdata -FixtureUrl $fixture.Url

    $healthyListener = Start-TcpAcceptJob
    Write-IsolatedState `
        -StatePath $statePath `
        -CurrentProxyRecord (New-ProxyRecord -Proxy (New-ProxyObject -Port $healthyListener.Port -Secret 'healthy-secret') -Source 'live-test') `
        -PendingProxyRecord $null `
        -TelegramRunning $true `
        -WatcherMode 'watching'

    $foregroundBeforeHealthy = Get-ForegroundHandle
    Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('watch', '--headless', '--once') | Out-Null
    $foregroundAfterHealthy = Get-ForegroundHandle
    if ($foregroundBeforeHealthy -ne $foregroundAfterHealthy) {
        throw 'Watcher изменил foreground window в healthy-сценарии.'
    }

    $healthyDoctor = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('doctor', '--json') | ConvertFrom-Json
    $healthyStatus = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('status', '--json') | ConvertFrom-Json
    if ($healthyDoctor.backend_restart_required) {
        throw 'Healthy-сценарий неожиданно запросил перезапуск Telegram.'
    }
    if ($healthyDoctor.backend_status -match 'manual fallback|ручной fallback|tg://') {
        throw "Healthy-сценарий ушёл в ручной fallback: $($healthyDoctor.backend_status)"
    }
    if ($healthyStatus.state.pending_proxy) {
        throw 'Healthy-сценарий неожиданно создал pending proxy.'
    }
    if ($healthyStatus.state.watcher.mode -ne 'watching') {
        throw "Healthy-сценарий вернул неожиданный watcher.mode: $($healthyStatus.state.watcher.mode)"
    }

    Stop-BackgroundJob -Entry $fixture
    Stop-BackgroundJob -Entry $healthyListener
    $fixture = $null
    $healthyListener = $null

    $replacementListener = Start-TcpAcceptJob
    $candidateLink = "tg://proxy?server=127.0.0.1&port=$($replacementListener.Port)&secret=replacement-secret"
    $fixture = Start-HttpFixtureJob -Body $candidateLink
    Write-IsolatedConfig -ConfigPath $configPath -Version $version -TelegramDataDir $telegramTdata -FixtureUrl $fixture.Url
    Write-IsolatedState `
        -StatePath $statePath `
        -CurrentProxyRecord (New-ProxyRecord -Proxy (New-ProxyObject -Port (Get-FreeTcpPort) -Secret 'dead-secret') -Source 'dead-current') `
        -PendingProxyRecord (New-ProxyRecord -Proxy (New-ProxyObject -Port $replacementListener.Port -Secret 'replacement-secret') -Source 'pending-live') `
        -TelegramRunning $true `
        -WatcherMode 'waiting_for_telegram'

    $foregroundBeforePending = Get-ForegroundHandle
    Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('watch', '--headless', '--once') | Out-Null
    $foregroundAfterPending = Get-ForegroundHandle
    if ($foregroundBeforePending -ne $foregroundAfterPending) {
        throw 'Watcher изменил foreground window в pending-сценарии.'
    }

    $pendingDoctor = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('doctor', '--json') | ConvertFrom-Json
    $pendingStatus = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('status', '--json') | ConvertFrom-Json
    if ($pendingDoctor.backend_restart_required) {
        throw 'Pending-сценарий неожиданно запросил перезапуск Telegram.'
    }
    if ($pendingDoctor.backend_route -notmatch 'settingss') {
        throw "Pending-сценарий не использовал managed settings path: $($pendingDoctor.backend_route)"
    }
    if ($pendingDoctor.backend_status -match 'manual fallback|ручной fallback|tg://') {
        throw "Pending-сценарий ушёл в ручной fallback: $($pendingDoctor.backend_status)"
    }
    if ($pendingStatus.state.pending_proxy) {
        throw 'Pending-сценарий не очистил pending proxy после apply.'
    }
    if ($pendingStatus.state.watcher.mode -ne 'watching') {
        throw "Pending-сценарий вернул неожиданный watcher.mode: $($pendingStatus.state.watcher.mode)"
    }
    if ([int]$pendingStatus.state.current_proxy.proxy.port -ne $replacementListener.Port) {
        throw "Pending-сценарий записал неожиданный current proxy port: $($pendingStatus.state.current_proxy.proxy.port)"
    }

    $finalDoctor = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('doctor', '--json') | ConvertFrom-Json
    $finalStatus = Invoke-ProtoSwitch -ExePath $resolvedBinary -AppDataRoot $tempAppData -LocalAppDataRoot $tempLocalAppData -Arguments @('status', '--json') | ConvertFrom-Json
    if ($finalDoctor.backend_restart_required) {
        throw 'Managed apply оставил backend_restart_required.'
    }
    if ($finalStatus.state.watcher.mode -ne 'watching') {
        throw "После рестарта watcher.mode не вернулся в watching: $($finalStatus.state.watcher.mode)"
    }

    Write-Host 'Live Windows e2e completed.'
}
finally {
    Stop-BackgroundJob -Entry $fixture
    Stop-BackgroundJob -Entry $healthyListener
    Stop-BackgroundJob -Entry $replacementListener
    if (Test-Path $backupPath) {
        Copy-Item $backupPath $telegramSettings -Force
    }
    if (Test-Path $tempRoot) {
        Remove-Item -Recurse -Force $tempRoot
    }
}
