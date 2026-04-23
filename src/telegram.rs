use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Output};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use std::thread::sleep;
use std::time::Duration;

use anyhow::anyhow;
#[cfg(windows)]
use anyhow::Context;
use sysinfo::{ProcessesToUpdate, System};

use crate::model::{MtProtoProxy, ProxyKind, TelegramBackendMode, TelegramConfig};
use crate::tdesktop::{
    DesktopProxyMode, DesktopProxySettings, detect_telegram_data_dir, load_proxy_settings,
    resolve_telegram_data_dir, write_proxy_settings_override,
};
#[cfg(windows)]
use crate::text::{decode_bytes, decode_output};

#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum ManagedProxyStatus {
    Available(String),
    Checking(String),
    Unavailable(String),
    Unknown(String),
    Missing,
}

#[derive(Debug, Clone)]
pub struct ManagedSettingsStatus {
    pub data_dir: PathBuf,
    pub selected_label: String,
    pub mode_label: String,
    pub proxy_count: usize,
}

#[derive(Debug, Clone)]
pub struct ManagedApplyResult {
    pub settings_path: PathBuf,
    pub settings_status: ManagedSettingsStatus,
    pub immediate: bool,
    pub used_fallback: bool,
    pub fallback_error: Option<String>,
}

#[cfg(test)]
fn telegram_running_override() -> &'static Mutex<Option<bool>> {
    static OVERRIDE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) struct TelegramRunningOverrideGuard;

#[cfg(test)]
impl Drop for TelegramRunningOverrideGuard {
    fn drop(&mut self) {
        *telegram_running_override().lock().unwrap() = None;
    }
}

#[cfg(test)]
pub(crate) fn override_is_running(value: bool) -> TelegramRunningOverrideGuard {
    *telegram_running_override().lock().unwrap() = Some(value);
    TelegramRunningOverrideGuard
}

pub fn is_running() -> anyhow::Result<bool> {
    #[cfg(test)]
    if let Some(value) = *telegram_running_override().lock().unwrap() {
        return Ok(value);
    }

    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);

    let running = system.processes().values().any(|process| {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        name == "telegram.exe" || name == "telegram"
    });

    Ok(running)
}

pub fn check_proxy(proxy: &MtProtoProxy, timeout_secs: u64) -> bool {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    match proxy.kind {
        ProxyKind::MtProto => resolve_socket_addr(&proxy.server, proxy.port)
            .and_then(|addr| TcpStream::connect_timeout(&addr, timeout).ok())
            .is_some(),
        ProxyKind::Socks5 => check_socks5_proxy(proxy, timeout),
    }
}

#[cfg(windows)]
pub fn open_proxy_link(proxy: &MtProtoProxy, timeout_secs: u64) -> anyhow::Result<()> {
    let output =
        run_hidden_powershell_output(&apply_proxy_command(&proxy.deep_link(), timeout_secs))
            .context("Не удалось вызвать PowerShell для tg://proxy")?;

    if !output.status.success() {
        return Err(anyhow!(decode_output(&output)));
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn open_proxy_link(_proxy: &MtProtoProxy, _timeout_secs: u64) -> anyhow::Result<()> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn probe_proxy_status(
    proxy: &MtProtoProxy,
    timeout_secs: u64,
) -> anyhow::Result<ManagedProxyStatus> {
    let output = run_hidden_powershell_output(&probe_proxy_status_command_v2(proxy, timeout_secs))
        .context("Не удалось вызвать PowerShell для проверки proxy в Telegram")?;

    if !output.status.success() {
        return Err(anyhow!(decode_output(&output)));
    }

    let stdout = decode_bytes(&output.stdout).trim().to_string();
    let line = stdout.lines().last().unwrap_or_default().trim();
    parse_probe_status_line(line)
}

#[cfg(not(windows))]
pub fn probe_proxy_status(
    _proxy: &MtProtoProxy,
    _timeout_secs: u64,
) -> anyhow::Result<ManagedProxyStatus> {
    Err(anyhow!("Поддерживается только Windows"))
}

pub fn settle_proxy_status(
    proxy: &MtProtoProxy,
    timeout_secs: u64,
) -> anyhow::Result<ManagedProxyStatus> {
    let attempts = match proxy.kind {
        ProxyKind::MtProto => 8,
        ProxyKind::Socks5 => 6,
    };
    let pause = Duration::from_millis(700);
    let mut last_status = ManagedProxyStatus::Missing;

    for attempt in 0..attempts {
        let status = probe_proxy_status(proxy, timeout_secs)?;
        match status {
            ManagedProxyStatus::Available(_) | ManagedProxyStatus::Unavailable(_) => {
                return Ok(status);
            }
            ManagedProxyStatus::Checking(_)
            | ManagedProxyStatus::Unknown(_)
            | ManagedProxyStatus::Missing => {
                last_status = status;
                if attempt + 1 < attempts {
                    sleep(pause);
                }
            }
        }
    }

    Ok(last_status)
}

pub fn detect_installation() -> anyhow::Result<TelegramInstallation> {
    Ok(TelegramInstallation {
        protocol_handler: tg_protocol_command(),
        executable_path: find_telegram_executable(),
        data_dir: detect_telegram_data_dir(),
    })
}

pub fn managed_settings_status(config: &TelegramConfig) -> anyhow::Result<ManagedSettingsStatus> {
    let data_dir = resolve_telegram_data_dir(config)?;
    let settings = load_proxy_settings(config)?;

    Ok(ManagedSettingsStatus {
        data_dir,
        selected_label: settings.selected_label(),
        mode_label: managed_mode_label(settings.mode).to_string(),
        proxy_count: settings.list.len(),
    })
}

pub fn apply_managed_proxy(
    config: &TelegramConfig,
    proxy: &MtProtoProxy,
    owned: &[MtProtoProxy],
    cleanup_owned: bool,
    allow_ui_fallback: bool,
    timeout_secs: u64,
) -> anyhow::Result<ManagedApplyResult> {
    let mut settings =
        load_proxy_settings(config).unwrap_or_else(|_| DesktopProxySettings::default());
    settings.upsert_managed_proxy(proxy, owned, cleanup_owned);
    let settings_path = write_proxy_settings_override(config, &settings)?;
    let settings_status = ManagedSettingsStatus {
        data_dir: settings_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| settings_path.clone()),
        selected_label: settings.selected_label(),
        mode_label: managed_mode_label(settings.mode).to_string(),
        proxy_count: settings.list.len(),
    };

    if matches!(config.backend_mode, TelegramBackendMode::Managed) || !is_running().unwrap_or(false)
    {
        return Ok(ManagedApplyResult {
            settings_path,
            settings_status,
            immediate: false,
            used_fallback: false,
            fallback_error: None,
        });
    }

    if allow_ui_fallback {
        let fallback_error = open_proxy_link(proxy, timeout_secs)
            .err()
            .map(|error| error.to_string());
        return Ok(ManagedApplyResult {
            settings_path,
            settings_status,
            immediate: fallback_error.is_none(),
            used_fallback: fallback_error.is_none(),
            fallback_error,
        });
    }

    Ok(ManagedApplyResult {
        settings_path,
        settings_status,
        immediate: false,
        used_fallback: false,
        fallback_error: None,
    })
}

pub fn cleanup_managed_proxies(
    config: &TelegramConfig,
    owned: &[MtProtoProxy],
) -> anyhow::Result<usize> {
    if owned.is_empty() {
        return Ok(0);
    }

    let mut settings = load_proxy_settings(config)?;
    let removed = settings.cleanup_owned(owned);
    if removed == 0 {
        return Ok(0);
    }

    let _ = write_proxy_settings_override(config, &settings)?;
    Ok(removed)
}

fn resolve_socket_addr(server: &str, port: u16) -> Option<SocketAddr> {
    (server, port).to_socket_addrs().ok()?.next()
}

fn check_socks5_proxy(proxy: &MtProtoProxy, timeout: Duration) -> bool {
    let Some(addr) = resolve_socket_addr(&proxy.server, proxy.port) else {
        return false;
    };

    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(stream) => stream,
        Err(_) => return false,
    };

    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let uses_auth = proxy
        .username
        .as_ref()
        .map(|value| !value.is_empty())
        .unwrap_or(false)
        || proxy
            .password
            .as_ref()
            .map(|value| !value.is_empty())
            .unwrap_or(false);

    let greeting = if uses_auth {
        [0x05_u8, 0x01, 0x02]
    } else {
        [0x05_u8, 0x01, 0x00]
    };

    if std::io::Write::write_all(&mut stream, &greeting).is_err() {
        return false;
    }

    let mut response = [0_u8; 2];
    if std::io::Read::read_exact(&mut stream, &mut response).is_err() {
        return false;
    }

    if response[0] != 0x05 {
        return false;
    }

    if uses_auth {
        if response[1] != 0x02 {
            return false;
        }
        let username = proxy.username.clone().unwrap_or_default().into_bytes();
        let password = proxy.password.clone().unwrap_or_default().into_bytes();
        if username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
            return false;
        }

        let mut payload = Vec::with_capacity(3 + username.len() + password.len());
        payload.push(0x01);
        payload.push(username.len() as u8);
        payload.extend_from_slice(&username);
        payload.push(password.len() as u8);
        payload.extend_from_slice(&password);
        if std::io::Write::write_all(&mut stream, &payload).is_err() {
            return false;
        }

        let mut auth_response = [0_u8; 2];
        if std::io::Read::read_exact(&mut stream, &mut auth_response).is_err() {
            return false;
        }

        auth_response == [0x01, 0x00]
    } else {
        response[1] == 0x00
    }
}

#[cfg(any(windows, test))]
fn parse_probe_status_line(line: &str) -> anyhow::Result<ManagedProxyStatus> {
    let Some((kind, value)) = line.split_once(':') else {
        return Err(anyhow!("Не удалось разобрать статус Telegram proxy"));
    };

    Ok(match kind {
        "available" => ManagedProxyStatus::Available(value.to_string()),
        "checking" => ManagedProxyStatus::Checking(value.to_string()),
        "unavailable" => ManagedProxyStatus::Unavailable(value.to_string()),
        "unknown" => ManagedProxyStatus::Unknown(value.to_string()),
        "missing" => ManagedProxyStatus::Missing,
        _ => return Err(anyhow!("Неизвестный статус Telegram proxy: {line}")),
    })
}

#[cfg(windows)]
fn tg_protocol_command() -> Option<String> {
    for (root, subkey) in [
        (
            HKEY_CURRENT_USER,
            "Software\\Classes\\tg\\shell\\open\\command",
        ),
        (HKEY_CLASSES_ROOT, "tg\\shell\\open\\command"),
    ] {
        if let Ok(key) = RegKey::predef(root).open_subkey(subkey)
            && let Ok(value) = key.get_value::<String, _>("")
        {
            return Some(value);
        }
    }
    None
}

#[cfg(not(windows))]
fn tg_protocol_command() -> Option<String> {
    None
}

fn find_telegram_executable() -> Option<PathBuf> {
    if let Some(command) = tg_protocol_command()
        && let Some(path) = parse_command_path(&command)
        && path.exists()
    {
        return Some(path);
    }

    common_telegram_paths()
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn parse_command_path(command: &str) -> Option<PathBuf> {
    if let Some(rest) = command.strip_prefix('"') {
        let path = rest.split('"').next()?;
        return Some(PathBuf::from(path));
    }

    Some(Path::new(command.split_whitespace().next()?).to_path_buf())
}

fn common_telegram_paths() -> Vec<PathBuf> {
    let mut values = Vec::new();
    for key in [
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ] {
        if let Ok(root) = std::env::var(key) {
            let base = PathBuf::from(root);
            values.push(base.join("Telegram Desktop").join("Telegram.exe"));
            values.push(
                base.join("Programs")
                    .join("Telegram Desktop")
                    .join("Telegram.exe"),
            );
        }
    }
    values
}

#[cfg(windows)]
fn run_hidden_powershell_output(script: &str) -> anyhow::Result<Output> {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-STA",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &with_utf8_powershell(script),
        ])
        .output()
        .context("Не удалось запустить PowerShell")
}

#[cfg(windows)]
fn with_utf8_powershell(script: &str) -> String {
    format!(
        "$utf8NoBom = [System.Text.UTF8Encoding]::new($false)\n$OutputEncoding = $utf8NoBom\n[Console]::InputEncoding = $utf8NoBom\n[Console]::OutputEncoding = $utf8NoBom\n{script}"
    )
}

#[cfg(windows)]
fn apply_proxy_command(value: &str, timeout_secs: u64) -> String {
    let timeout_ms = (timeout_secs.max(3) + 4).saturating_mul(1_000).to_string();
    let script = r#"$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ProtoSwitchNative {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
}
"@
function Invoke-Element($element) {
  $pattern = $null
  if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
    return $true
  }
  return $false
}
function Restore-PreviousWindow($handle) {
  if ($handle -eq [IntPtr]::Zero) {
    return
  }
  Start-Sleep -Milliseconds 120
  [ProtoSwitchNative]::ShowWindowAsync($handle, 9) | Out-Null
  [ProtoSwitchNative]::SetForegroundWindow($handle) | Out-Null
}
function Get-MainWindow() {
  $proc = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $proc -or $proc.MainWindowHandle -eq 0) {
    return $null
  }
  [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$proc.MainWindowHandle)
}
function Wait-ForDialog($timeoutMs) {
  $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $classes = @(
    'class Ui::GenericBox',
    'class `anonymous namespace''::ProxyBox'
  )
  while ([DateTime]::UtcNow -lt $deadline) {
    $main = Get-MainWindow
    if ($null -eq $main) {
      Start-Sleep -Milliseconds 120
      continue
    }
    foreach ($className in $classes) {
      $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ClassNameProperty,
        $className
      )
      $items = $main.FindAll($scope, $cond)
      for ($i = 0; $i -lt $items.Count; $i++) {
        $candidate = $items.Item($i)
        if (-not $candidate.Current.IsOffscreen) {
          return [PSCustomObject]@{ Main = $main; Element = $candidate }
        }
      }
    }
    Start-Sleep -Milliseconds 120
  }
  return $null
}
function Get-TextSnapshot($element) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Text
  )
  $items = $element.FindAll($scope, $cond)
  $values = @()
  for ($i = 0; $i -lt $items.Count; $i++) {
    $name = $items.Item($i).Current.Name
    if (-not [string]::IsNullOrWhiteSpace($name)) {
      $values += $name
    }
  }
  ($values -join ' | ')
}
function Get-VisibleButtons($element) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button
  )
  $items = $element.FindAll($scope, $cond)
  $values = @()
  for ($i = 0; $i -lt $items.Count; $i++) {
    $candidate = $items.Item($i)
    if ($candidate.Current.IsOffscreen -or -not $candidate.Current.IsEnabled) {
      continue
    }
    $values += $candidate
  }
  $values
}
function Find-NamedButton($element, $pattern) {
  $buttons = Get-VisibleButtons $element
  foreach ($candidate in $buttons) {
    $name = $candidate.Current.Name
    if ($name -match $pattern) {
      return $candidate
    }
  }
  return $null
}
function Find-PrimaryButton($element) {
  $buttons = Get-VisibleButtons $element
  $pattern = 'Add|Connect|Use|Enable|Save|Done|Добав|Подключ|Использ|Включ|Сохран|Готов'
  foreach ($candidate in $buttons) {
    $name = $candidate.Current.Name
    if ($name -match $pattern) {
      return $candidate
    }
  }
  $primary = $null
  $bestScore = -2147483648
  foreach ($candidate in $buttons) {
    $rect = $candidate.Current.BoundingRectangle
    $candidateScore = ([int]$rect.Y * 10000) + [int]$rect.X
    if ($candidateScore -gt $bestScore) {
      $primary = $candidate
      $bestScore = $candidateScore
    }
  }
  return $primary
}
function Close-Dialog($element) {
  $cancel = Find-NamedButton $element 'Cancel|Close|Dismiss|Отмена|Закры|Позже'
  if ($null -ne $cancel -and (Invoke-Element $cancel)) {
    return $true
  }
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::IconButton'
  )
  $items = $element.FindAll($scope, $cond)
  $target = $null
  $bestScore = -2147483648
  for ($i = 0; $i -lt $items.Count; $i++) {
    $candidate = $items.Item($i)
    if ($candidate.Current.IsOffscreen -or -not $candidate.Current.IsEnabled) {
      continue
    }
    $rect = $candidate.Current.BoundingRectangle
    $candidateScore = ([int]$rect.X * 10000) - [int]$rect.Y
    if ($candidateScore -gt $bestScore) {
      $target = $candidate
      $bestScore = $candidateScore
    }
  }
  if ($null -ne $target) {
    return (Invoke-Element $target)
  }
  return $false
}
function Get-DialogStatus($element) {
  $labels = Get-TextSnapshot $element
  $normalized = $labels.ToLowerInvariant()
  if ($normalized -match 'not available|timed out|timeout|failed|error|denied|недоступ|не работает|не удалось') {
    return 'unavailable:' + $labels
  }
  if ($normalized -match 'checking|connecting|loading|wait|провер|ожид|подключение') {
    return 'checking:' + $labels
  }
  if ($normalized -match 'available|connected|online|working|доступ|работает|успеш|подключен') {
    return 'available:' + $labels
  }
  if ([string]::IsNullOrWhiteSpace($labels)) {
    return 'unknown:диалог открыт'
  }
  return 'unknown:' + $labels
}
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process __LINK__
$dialog = Wait-ForDialog 6500
if ($null -eq $dialog) {
  Restore-PreviousWindow $previous
  Write-Output 'диалог proxy не найден'
  exit 1
}
$check = Find-NamedButton $dialog.Element 'Check|Status|Провер|Статус'
$requiresPositiveStatus = $null -ne $check
if ($requiresPositiveStatus) {
  if (-not (Invoke-Element $check)) {
    Restore-PreviousWindow $previous
    Write-Output 'кнопка проверки статуса не ответила'
    exit 1
  }
  Start-Sleep -Milliseconds 350
}
$deadline = [DateTime]::UtcNow.AddMilliseconds(__TIMEOUT_MS__)
$lastStatus = 'unknown:диалог открыт'
while ([DateTime]::UtcNow -lt $deadline) {
  $lastStatus = Get-DialogStatus $dialog.Element
  if ($lastStatus.StartsWith('available:')) {
    $primary = Find-PrimaryButton $dialog.Element
    if ($null -eq $primary) {
      Restore-PreviousWindow $previous
      Write-Output 'кнопка подтверждения не найдена'
      exit 1
    }
    if (Invoke-Element $primary) {
      Restore-PreviousWindow $previous
      Write-Output $lastStatus
      exit 0
    }
    Restore-PreviousWindow $previous
    Write-Output 'кнопка подтверждения не ответила'
    exit 1
  }
  if ($lastStatus.StartsWith('unavailable:')) {
    Close-Dialog $dialog.Element | Out-Null
    Restore-PreviousWindow $previous
    Write-Output $lastStatus
    exit 2
  }
  if (-not $requiresPositiveStatus) {
    $primary = Find-PrimaryButton $dialog.Element
    if ($null -eq $primary) {
      Restore-PreviousWindow $previous
      Write-Output 'кнопка подтверждения не найдена'
      exit 1
    }
    if (Invoke-Element $primary) {
      Restore-PreviousWindow $previous
      Write-Output $lastStatus
      exit 0
    }
    Restore-PreviousWindow $previous
    Write-Output 'кнопка подтверждения не ответила'
    exit 1
  }
  Start-Sleep -Milliseconds 280
}
Close-Dialog $dialog.Element | Out-Null
Restore-PreviousWindow $previous
Write-Output $lastStatus
exit 2"#;

    script
        .replace("__LINK__", &ps_literal(value))
        .replace("__TIMEOUT_MS__", &timeout_ms)
}

#[cfg(all(windows, test))]
fn probe_proxy_status_command(proxy: &MtProtoProxy, timeout_secs: u64) -> String {
    let timeout_ms = (timeout_secs.max(3) + 3).saturating_mul(1_000).to_string();
    let script = r#"$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ProtoSwitchNative {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
}
"@
function Invoke-Element($element) {
  $pattern = $null
  if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
    return $true
  }
  return $false
}
function Restore-PreviousWindow($handle) {
  if ($handle -eq [IntPtr]::Zero) {
    return
  }
  Start-Sleep -Milliseconds 120
  [ProtoSwitchNative]::ShowWindowAsync($handle, 9) | Out-Null
  [ProtoSwitchNative]::SetForegroundWindow($handle) | Out-Null
}
function Normalize-Key($kind, $server, $port, $secret, $user, $pass) {
  ('{0}:{1}:{2}:{3}:{4}:{5}' -f $kind, $server, $port, $secret, $user, $pass).ToLowerInvariant()
}
function Get-MainWindow() {
  $proc = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $proc -or $proc.MainWindowHandle -eq 0) {
    return $null
  }
  [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$proc.MainWindowHandle)
}
function Wait-ForClass($className, $timeoutMs) {
  $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
  while ([DateTime]::UtcNow -lt $deadline) {
    $main = Get-MainWindow
    if ($null -eq $main) {
      Start-Sleep -Milliseconds 120
      continue
    }
    $cond = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::ClassNameProperty,
      $className
    )
    $scope = [System.Windows.Automation.TreeScope]::Descendants
    $items = $main.FindAll($scope, $cond)
    for ($i = 0; $i -lt $items.Count; $i++) {
      $candidate = $items.Item($i)
      if (-not $candidate.Current.IsOffscreen) {
        return [PSCustomObject]@{ Main = $main; Element = $candidate }
      }
    }
    Start-Sleep -Milliseconds 120
  }
  return $null
}
function Get-VisibleProxyRows($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class `anonymous namespace''::ProxyRow'
  )
  $rows = $main.FindAll($scope, $cond)
  $result = @()
  for ($i = 0; $i -lt $rows.Count; $i++) {
    $row = $rows.Item($i)
    if ($row.Current.IsOffscreen) {
      continue
    }
    $result += [PSCustomObject]@{
      Element = $row
      Y = [int]$row.Current.BoundingRectangle.Y
    }
  }
  $result | Sort-Object Y
}
function Get-TextSnapshot($element) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Text
  )
  $items = $element.FindAll($scope, $cond)
  $values = @()
  for ($i = 0; $i -lt $items.Count; $i++) {
    $name = $items.Item($i).Current.Name
    if (-not [string]::IsNullOrWhiteSpace($name)) {
      $values += $name
    }
  }
  ($values -join ' | ')
}
function Get-RowMenuButton($main, $rowY) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::IconButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  $target = $null
  $bestDistance = 99999
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    $distance = [math]::Abs([int]$button.Current.BoundingRectangle.Y - [int]$rowY)
    if ($distance -le 8 -and $distance -lt $bestDistance) {
      $target = $button
      $bestDistance = $distance
    }
  }
  $target
}
function Get-MenuItemByNames($main, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
  )
  $items = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    if ($item.Current.IsOffscreen) {
      continue
    }
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        return $item
      }
    }
  }
  return $null
}
function Get-EditorValue($editor, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit
  )
  $items = $editor.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        $pattern = $null
        if ($item.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
          return ([System.Windows.Automation.ValuePattern]$pattern).Current.Value
        }
      }
    }
  }
  return ''
}
function Close-Editor($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::RoundButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    if ($button.Current.Name -match 'Cancel|Отмена|Close|Закрыть') {
      Invoke-Element $button | Out-Null
      Start-Sleep -Milliseconds 150
      return
    }
  }
}
function Open-RowEditor($main, $rowY) {
  $menu = Get-RowMenuButton $main $rowY
  if ($null -eq $menu -or -not (Invoke-Element $menu)) {
    return $null
  }
  Start-Sleep -Milliseconds 180
  $edit = Get-MenuItemByNames $main @('Edit', 'Редактировать')
  if ($null -eq $edit -or -not (Invoke-Element $edit)) {
    return $null
  }
  Start-Sleep -Milliseconds 220
  Wait-ForClass 'class `anonymous namespace''::ProxyBox' 1800
}
function Get-StatusResult($labels) {
  $normalized = $labels.ToLowerInvariant()
  if ($normalized -match 'not available|недоступ|не доступен') {
    return 'unavailable:' + $labels
  }
  if ($normalized -match 'checking|провер') {
    return 'checking:' + $labels
  }
  if ($normalized -match 'available|connected|online|доступ|подключ') {
    return 'available:' + $labels
  }
  if ([string]::IsNullOrWhiteSpace($labels)) {
    return 'unknown:строка найдена'
  }
  return 'unknown:' + $labels
}
$targetKey = Normalize-Key __KIND__ '__SERVER__' '__PORT__' __SECRET__ __USER__ __PASS__
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process 'tg://settings/data_and_storage/proxy/settings'
$box = Wait-ForClass 'class `anonymous namespace''::ProxiesBox' 6000
if ($null -eq $box) {
  Restore-PreviousWindow $previous
  exit 1
}
Restore-PreviousWindow $previous
$deadline = [DateTime]::UtcNow.AddMilliseconds(__TIMEOUT_MS__)
$best = 'missing:не найден'
while ([DateTime]::UtcNow -lt $deadline) {
  $main = Get-MainWindow
  if ($null -eq $main) {
    Start-Sleep -Milliseconds 250
    continue
  }
  $rows = Get-VisibleProxyRows $main
  foreach ($row in $rows) {
    $labels = Get-TextSnapshot $row.Element
    $editor = Open-RowEditor $main $row.Y
    if ($null -eq $editor) {
      continue
    }
    $server = Get-EditorValue $editor.Element @('Hostname', 'Host', 'Server', 'Сервер')
    $port = Get-EditorValue $editor.Element @('Port')
    $secret = Get-EditorValue $editor.Element @('Secret', 'Секрет')
    Close-Editor $editor.Main
    if ([string]::IsNullOrWhiteSpace($server) -or [string]::IsNullOrWhiteSpace($port) -or [string]::IsNullOrWhiteSpace($secret)) {
      continue
    }
    $rowKey = Normalize-Key $server $port $secret
    if ($rowKey -ne $targetKey) {
      continue
    }
    $best = Get-StatusResult $labels
    if ($best.StartsWith('available:') -or $best.StartsWith('unavailable:')) {
      Write-Output $best
      exit 0
    }
  }
  Start-Sleep -Milliseconds 350
}
Write-Output $best
exit 0"#;

    script
        .replace("__SERVER__", &ps_literal(&proxy.server))
        .replace("__PORT__", &proxy.port.to_string())
        .replace(
            "__SECRET__",
            &ps_literal(proxy.secret.as_deref().unwrap_or_default()),
        )
        .replace("__TIMEOUT_MS__", &timeout_ms)
}

#[cfg(all(windows, test))]
fn cleanup_proxies_command(proxies: &[MtProtoProxy]) -> String {
    let payload = serde_json::to_string(proxies).unwrap_or_else(|_| "[]".to_string());
    let script = r#"$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ProtoSwitchNative {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
}
"@
function Invoke-Element($element) {
  $pattern = $null
  if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
    return $true
  }
  return $false
}
function Restore-PreviousWindow($handle) {
  if ($handle -eq [IntPtr]::Zero) {
    return
  }
  Start-Sleep -Milliseconds 120
  [ProtoSwitchNative]::ShowWindowAsync($handle, 9) | Out-Null
  [ProtoSwitchNative]::SetForegroundWindow($handle) | Out-Null
}
function Get-MainWindow() {
  $proc = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $proc -or $proc.MainWindowHandle -eq 0) {
    return $null
  }
  [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$proc.MainWindowHandle)
}
function Wait-ForClass($className, $timeoutMs) {
  $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
  while ([DateTime]::UtcNow -lt $deadline) {
    $main = Get-MainWindow
    if ($null -eq $main) {
      Start-Sleep -Milliseconds 120
      continue
    }
    $cond = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::ClassNameProperty,
      $className
    )
    $scope = [System.Windows.Automation.TreeScope]::Descendants
    $items = $main.FindAll($scope, $cond)
    for ($i = 0; $i -lt $items.Count; $i++) {
      $candidate = $items.Item($i)
      if (-not $candidate.Current.IsOffscreen) {
        return [PSCustomObject]@{ Main = $main; Element = $candidate }
      }
    }
    Start-Sleep -Milliseconds 120
  }
  return $null
}
function Normalize-Key($server, $port, $secret) {
  ('{0}:{1}:{2}' -f $server, $port, $secret).ToLowerInvariant()
}
function Get-VisibleProxyRows($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class `anonymous namespace''::ProxyRow'
  )
  $rows = $main.FindAll($scope, $cond)
  $result = @()
  for ($i = 0; $i -lt $rows.Count; $i++) {
    $row = $rows.Item($i)
    if ($row.Current.IsOffscreen) {
      continue
    }
    $result += [PSCustomObject]@{
      Element = $row
      Y = [int]$row.Current.BoundingRectangle.Y
    }
  }
  $result | Sort-Object Y
}
function Get-RowMenuButton($main, $rowY) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::IconButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  $target = $null
  $bestDistance = 99999
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    $distance = [math]::Abs([int]$button.Current.BoundingRectangle.Y - [int]$rowY)
    if ($distance -le 8 -and $distance -lt $bestDistance) {
      $target = $button
      $bestDistance = $distance
    }
  }
  $target
}
function Get-MenuItemByNames($main, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
  )
  $items = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    if ($item.Current.IsOffscreen) {
      continue
    }
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        return $item
      }
    }
  }
  return $null
}
function Get-EditorValue($editor, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit
  )
  $items = $editor.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        $pattern = $null
        if ($item.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
          return ([System.Windows.Automation.ValuePattern]$pattern).Current.Value
        }
      }
    }
  }
  return ''
}
function Close-Editor($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::RoundButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    if ($button.Current.Name -match 'Cancel|Отмена|Close|Закрыть') {
      Invoke-Element $button | Out-Null
      Start-Sleep -Milliseconds 150
      return
    }
  }
}
$targets = ConvertFrom-Json __PAYLOAD__
if ($null -eq $targets -or $targets.Count -eq 0) {
  Write-Output 0
  exit 0
}
$remaining = @{}
foreach ($target in $targets) {
  $key = Normalize-Key $target.server $target.port $target.secret
  $remaining[$key] = $true
}
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process 'tg://settings/data_and_storage/proxy/settings'
$box = Wait-ForClass 'class `anonymous namespace''::ProxiesBox' 6000
if ($null -eq $box) {
  Restore-PreviousWindow $previous
  exit 1
}
Restore-PreviousWindow $previous
$removed = 0
for ($pass = 0; $pass -lt 12 -and $remaining.Count -gt 0; $pass++) {
  $main = Get-MainWindow
  if ($null -eq $main) {
    break
  }
  $rows = Get-VisibleProxyRows $main
  $deleted = $false
  foreach ($row in $rows) {
    $main = Get-MainWindow
    if ($null -eq $main) {
      break
    }
    $menu = Get-RowMenuButton $main $row.Y
    if ($null -eq $menu -or -not (Invoke-Element $menu)) {
      continue
    }
    Start-Sleep -Milliseconds 180
    $edit = Get-MenuItemByNames $main @('Edit', 'Редактировать')
    if ($null -eq $edit -or -not (Invoke-Element $edit)) {
      continue
    }
    Start-Sleep -Milliseconds 220
    $editor = Wait-ForClass 'class `anonymous namespace''::ProxyBox' 1800
    if ($null -eq $editor) {
      continue
    }
    $server = Get-EditorValue $editor.Element @('Hostname', 'Host', 'Server', 'Сервер')
    $port = Get-EditorValue $editor.Element @('Port')
    $secret = Get-EditorValue $editor.Element @('Secret', 'Секрет')
    Close-Editor $editor.Main
    if ([string]::IsNullOrWhiteSpace($server) -or [string]::IsNullOrWhiteSpace($port) -or [string]::IsNullOrWhiteSpace($secret)) {
      continue
    }
    $key = Normalize-Key $server $port $secret
    if (-not $remaining.ContainsKey($key)) {
      continue
    }
    $main = Get-MainWindow
    if ($null -eq $main) {
      break
    }
    $menu = Get-RowMenuButton $main $row.Y
    if ($null -eq $menu -or -not (Invoke-Element $menu)) {
      continue
    }
    Start-Sleep -Milliseconds 180
    $delete = Get-MenuItemByNames $main @('Delete', 'Удалить')
    if ($null -eq $delete -or -not (Invoke-Element $delete)) {
      continue
    }
    $remaining.Remove($key) | Out-Null
    $removed++
    $deleted = $true
    Start-Sleep -Milliseconds 220
    break
  }
  if (-not $deleted) {
    break
  }
}
Write-Output $removed
exit 0"#;

    script.replace("__PAYLOAD__", &ps_literal(&payload))
}

#[cfg(windows)]
fn probe_proxy_status_command_v2(proxy: &MtProtoProxy, timeout_secs: u64) -> String {
    let timeout_ms = (timeout_secs.max(3) + 3).saturating_mul(1_000).to_string();
    let script = r#"$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ProtoSwitchNative {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
}
"@
function Invoke-Element($element) {
  $pattern = $null
  if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
    return $true
  }
  return $false
}
function Restore-PreviousWindow($handle) {
  if ($handle -eq [IntPtr]::Zero) {
    return
  }
  Start-Sleep -Milliseconds 120
  [ProtoSwitchNative]::ShowWindowAsync($handle, 9) | Out-Null
  [ProtoSwitchNative]::SetForegroundWindow($handle) | Out-Null
}
function Normalize-Key($kind, $server, $port, $secret, $user, $pass) {
  ('{0}:{1}:{2}:{3}:{4}:{5}' -f $kind, $server, $port, $secret, $user, $pass).ToLowerInvariant()
}
function Get-MainWindow() {
  $proc = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $proc -or $proc.MainWindowHandle -eq 0) {
    return $null
  }
  [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$proc.MainWindowHandle)
}
function Wait-ForClass($className, $timeoutMs) {
  $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
  while ([DateTime]::UtcNow -lt $deadline) {
    $main = Get-MainWindow
    if ($null -eq $main) {
      Start-Sleep -Milliseconds 120
      continue
    }
    $cond = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::ClassNameProperty,
      $className
    )
    $scope = [System.Windows.Automation.TreeScope]::Descendants
    $items = $main.FindAll($scope, $cond)
    for ($i = 0; $i -lt $items.Count; $i++) {
      $candidate = $items.Item($i)
      if (-not $candidate.Current.IsOffscreen) {
        return [PSCustomObject]@{ Main = $main; Element = $candidate }
      }
    }
    Start-Sleep -Milliseconds 120
  }
  return $null
}
function Get-VisibleProxyRows($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class `anonymous namespace''::ProxyRow'
  )
  $rows = $main.FindAll($scope, $cond)
  $result = @()
  for ($i = 0; $i -lt $rows.Count; $i++) {
    $row = $rows.Item($i)
    if ($row.Current.IsOffscreen) {
      continue
    }
    $result += [PSCustomObject]@{
      Element = $row
      Y = [int]$row.Current.BoundingRectangle.Y
    }
  }
  $result | Sort-Object Y
}
function Get-TextSnapshot($element) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Text
  )
  $items = $element.FindAll($scope, $cond)
  $values = @()
  for ($i = 0; $i -lt $items.Count; $i++) {
    $name = $items.Item($i).Current.Name
    if (-not [string]::IsNullOrWhiteSpace($name)) {
      $values += $name
    }
  }
  ($values -join ' | ')
}
function Get-RowMenuButton($main, $rowY) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::IconButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  $target = $null
  $bestDistance = 99999
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    $distance = [math]::Abs([int]$button.Current.BoundingRectangle.Y - [int]$rowY)
    if ($distance -le 8 -and $distance -lt $bestDistance) {
      $target = $button
      $bestDistance = $distance
    }
  }
  $target
}
function Get-MenuItemByNames($main, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
  )
  $items = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    if ($item.Current.IsOffscreen) {
      continue
    }
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        return $item
      }
    }
  }
  return $null
}
function Get-EditorValue($editor, $names) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit
  )
  $items = $editor.FindAll($scope, $cond)
  for ($i = 0; $i -lt $items.Count; $i++) {
    $item = $items.Item($i)
    foreach ($name in $names) {
      if ($item.Current.Name -eq $name) {
        $pattern = $null
        if ($item.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
          return ([System.Windows.Automation.ValuePattern]$pattern).Current.Value
        }
      }
    }
  }
  return ''
}
function Close-Editor($main) {
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::RoundButton'
  )
  $buttons = $main.FindAll($scope, $cond)
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $button = $buttons.Item($i)
    if ($button.Current.IsOffscreen) {
      continue
    }
    if ($button.Current.Name -match 'Cancel|РћС‚РјРµРЅР°|Close|Р—Р°РєСЂС‹С‚СЊ') {
      Invoke-Element $button | Out-Null
      Start-Sleep -Milliseconds 150
      return
    }
  }
}
function Open-RowEditor($main, $rowY) {
  $menu = Get-RowMenuButton $main $rowY
  if ($null -eq $menu -or -not (Invoke-Element $menu)) {
    return $null
  }
  Start-Sleep -Milliseconds 180
  $edit = Get-MenuItemByNames $main @('Edit', 'Р РµРґР°РєС‚РёСЂРѕРІР°С‚СЊ')
  if ($null -eq $edit -or -not (Invoke-Element $edit)) {
    return $null
  }
  Start-Sleep -Milliseconds 220
  Wait-ForClass 'class `anonymous namespace''::ProxyBox' 1800
}
function Get-StatusResult($labels) {
  $normalized = $labels.ToLowerInvariant()
  if ($normalized -match 'not available|РЅРµРґРѕСЃС‚СѓРї|РЅРµ РґРѕСЃС‚СѓРїРµРЅ') {
    return 'unavailable:' + $labels
  }
  if ($normalized -match 'checking|РїСЂРѕРІРµСЂ') {
    return 'checking:' + $labels
  }
  if ($normalized -match 'available|connected|online|РґРѕСЃС‚СѓРї|РїРѕРґРєР»СЋС‡') {
    return 'available:' + $labels
  }
  if ([string]::IsNullOrWhiteSpace($labels)) {
    return 'unknown:строка найдена'
  }
  return 'unknown:' + $labels
}
$targetKey = Normalize-Key __KIND__ '__SERVER__' '__PORT__' __SECRET__ __USER__ __PASS__
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process 'tg://settings/data_and_storage/proxy/settings'
$box = Wait-ForClass 'class `anonymous namespace''::ProxiesBox' 6000
if ($null -eq $box) {
  Restore-PreviousWindow $previous
  exit 1
}
Restore-PreviousWindow $previous
$deadline = [DateTime]::UtcNow.AddMilliseconds(__TIMEOUT_MS__)
$best = 'missing:не найден'
while ([DateTime]::UtcNow -lt $deadline) {
  $main = Get-MainWindow
  if ($null -eq $main) {
    Start-Sleep -Milliseconds 250
    continue
  }
  $rows = Get-VisibleProxyRows $main
  foreach ($row in $rows) {
    $labels = Get-TextSnapshot $row.Element
    $editor = Open-RowEditor $main $row.Y
    if ($null -eq $editor) {
      continue
    }
    $server = Get-EditorValue $editor.Element @('Hostname', 'Host', 'Server', 'РЎРµСЂРІРµСЂ')
    $port = Get-EditorValue $editor.Element @('Port')
    $secret = Get-EditorValue $editor.Element @('Secret', 'РЎРµРєСЂРµС‚')
    $user = Get-EditorValue $editor.Element @('Username', 'User', 'Login', 'Логин', 'Имя пользователя')
    $pass = Get-EditorValue $editor.Element @('Password', 'Pass', 'Пароль')
    Close-Editor $editor.Main
    if ([string]::IsNullOrWhiteSpace($server) -or [string]::IsNullOrWhiteSpace($port)) {
      continue
    }
    $kind = if ([string]::IsNullOrWhiteSpace($secret)) { 'socks5' } else { 'mtproto' }
    $rowKey = Normalize-Key $kind $server $port $secret $user $pass
    if ($rowKey -ne $targetKey) {
      continue
    }
    $best = Get-StatusResult $labels
    if ($best.StartsWith('available:') -or $best.StartsWith('unavailable:')) {
      Write-Output $best
      exit 0
    }
  }
  Start-Sleep -Milliseconds 350
}
Write-Output $best
exit 0"#;

    script
        .replace(
            "__KIND__",
            match proxy.kind {
                ProxyKind::MtProto => "'mtproto'",
                ProxyKind::Socks5 => "'socks5'",
            },
        )
        .replace("__SERVER__", &ps_literal(&proxy.server))
        .replace("__PORT__", &proxy.port.to_string())
        .replace(
            "__SECRET__",
            &ps_literal(proxy.secret.as_deref().unwrap_or("")),
        )
        .replace(
            "__USER__",
            &ps_literal(proxy.username.as_deref().unwrap_or("")),
        )
        .replace(
            "__PASS__",
            &ps_literal(proxy.password.as_deref().unwrap_or("")),
        )
        .replace("__TIMEOUT_MS__", &timeout_ms)
}

#[cfg(windows)]
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[derive(Debug, Clone)]
pub struct TelegramInstallation {
    pub protocol_handler: Option<String>,
    pub executable_path: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

fn managed_mode_label(mode: DesktopProxyMode) -> &'static str {
    match mode {
        DesktopProxyMode::System => "системный",
        DesktopProxyMode::Enabled => "включён",
        DesktopProxyMode::Disabled => "выключен",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_test_running_override() {
        let _guard = override_is_running(true);
        assert!(is_running().unwrap());
    }

    #[test]
    fn parses_probe_status_lines() {
        assert!(matches!(
            parse_probe_status_line("available:Connected"),
            Ok(ManagedProxyStatus::Available(value)) if value == "Connected"
        ));
        assert!(matches!(
            parse_probe_status_line("unavailable:not available"),
            Ok(ManagedProxyStatus::Unavailable(value)) if value == "not available"
        ));
        assert!(matches!(
            parse_probe_status_line("missing:not found"),
            Ok(ManagedProxyStatus::Missing)
        ));
    }

    #[test]
    #[cfg(windows)]
    fn formats_powershell_command_for_tg_link() {
        let value = apply_proxy_command("tg://proxy?server=test&port=443&secret=abcd", 4);
        assert!(value.contains("Start-Process 'tg://proxy?server=test&port=443&secret=abcd'"));
        assert!(value.contains("Restore-PreviousWindow $previous"));
        assert!(value.contains("Check|Status|Провер|Статус"));
        assert!(
            value.contains(
                "Add|Connect|Use|Enable|Save|Done|Добав|Подключ|Использ|Включ|Сохран|Готов"
            )
        );
        assert!(!value.contains("__TIMEOUT_MS__"));
    }

    #[test]
    #[cfg(windows)]
    fn formats_probe_command_for_proxy_settings() {
        let value =
            probe_proxy_status_command(&MtProtoProxy::mtproto("example.com", 443, "abcd"), 4);
        assert!(value.contains("tg://settings/data_and_storage/proxy/settings"));
        assert!(value.contains("Get-TextSnapshot"));
        assert!(value.contains("available:"));
    }

    #[test]
    #[cfg(windows)]
    fn formats_cleanup_command_for_proxy_settings() {
        let value = cleanup_proxies_command(&[MtProtoProxy::mtproto("example.com", 443, "abcd")]);
        assert!(value.contains("tg://settings/data_and_storage/proxy/settings"));
        assert!(value.contains("class `anonymous namespace''::ProxyRow"));
        assert!(value.contains("Delete"));
    }
}
