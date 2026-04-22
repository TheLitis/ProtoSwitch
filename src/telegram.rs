use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, anyhow};
use sysinfo::{ProcessesToUpdate, System};

use crate::model::MtProtoProxy;

#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedProxyStatus {
    Available(String),
    Checking(String),
    Unavailable(String),
    Unknown(String),
    Missing,
}

pub fn is_running() -> anyhow::Result<bool> {
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
    resolve_socket_addr(&proxy.server, proxy.port)
        .and_then(|addr| TcpStream::connect_timeout(&addr, timeout).ok())
        .is_some()
}

#[cfg(windows)]
pub fn open_proxy_link(proxy: &MtProtoProxy) -> anyhow::Result<()> {
    let output = run_hidden_powershell_output(&apply_proxy_command(&proxy.deep_link()))
        .context("Не удалось вызвать PowerShell для tg://proxy")?;

    if !output.status.success() {
        return Err(anyhow!(render_output(&output)));
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn open_proxy_link(_proxy: &MtProtoProxy) -> anyhow::Result<()> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn probe_proxy_status(
    proxy: &MtProtoProxy,
    timeout_secs: u64,
) -> anyhow::Result<ManagedProxyStatus> {
    let output = run_hidden_powershell_output(&probe_proxy_status_command(proxy, timeout_secs))
        .context("Не удалось вызвать PowerShell для проверки proxy в Telegram")?;

    if !output.status.success() {
        return Err(anyhow!(render_output(&output)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
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

#[cfg(windows)]
pub fn detect_unavailable_proxies(
    proxies: &[MtProtoProxy],
    timeout_secs: u64,
) -> anyhow::Result<Vec<MtProtoProxy>> {
    let mut values = Vec::new();
    for proxy in proxies {
        if matches!(
            probe_proxy_status(proxy, timeout_secs)?,
            ManagedProxyStatus::Unavailable(_)
        ) {
            values.push(proxy.clone());
        }
    }
    Ok(values)
}

#[cfg(not(windows))]
pub fn detect_unavailable_proxies(
    _proxies: &[MtProtoProxy],
    _timeout_secs: u64,
) -> anyhow::Result<Vec<MtProtoProxy>> {
    Err(anyhow!("Поддерживается только Windows"))
}

#[cfg(windows)]
pub fn remove_proxies(proxies: &[MtProtoProxy]) -> anyhow::Result<usize> {
    if proxies.is_empty() {
        return Ok(0);
    }

    let output = run_hidden_powershell_output(&cleanup_proxies_command(proxies))
        .context("Не удалось вызвать PowerShell для очистки proxy в Telegram")?;

    if !output.status.success() {
        return Err(anyhow!(render_output(&output)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(0);
    }

    stdout
        .lines()
        .last()
        .unwrap_or("0")
        .trim()
        .parse::<usize>()
        .context("Не удалось разобрать результат очистки Telegram proxy list")
}

#[cfg(not(windows))]
pub fn remove_proxies(_proxies: &[MtProtoProxy]) -> anyhow::Result<usize> {
    Err(anyhow!("Поддерживается только Windows"))
}

pub fn detect_installation() -> anyhow::Result<TelegramInstallation> {
    Ok(TelegramInstallation {
        protocol_handler: tg_protocol_command(),
        executable_path: find_telegram_executable(),
    })
}

fn resolve_socket_addr(server: &str, port: u16) -> Option<SocketAddr> {
    (server, port).to_socket_addrs().ok()?.next()
}

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
        if let Ok(key) = RegKey::predef(root).open_subkey(subkey) {
            if let Ok(value) = key.get_value::<String, _>("") {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn tg_protocol_command() -> Option<String> {
    None
}

fn find_telegram_executable() -> Option<PathBuf> {
    if let Some(command) = tg_protocol_command() {
        if let Some(path) = parse_command_path(&command) {
            if path.exists() {
                return Some(path);
            }
        }
    }

    for candidate in common_telegram_paths() {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
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
            script,
        ])
        .output()
        .context("Не удалось запустить PowerShell")
}

#[cfg(windows)]
fn apply_proxy_command(value: &str) -> String {
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
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process __LINK__
$deadline = [DateTime]::UtcNow.AddMilliseconds(6500)
while ([DateTime]::UtcNow -lt $deadline) {
  $proc = Get-Process Telegram -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $proc -or $proc.MainWindowHandle -eq 0) {
    Start-Sleep -Milliseconds 120
    continue
  }
  $main = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$proc.MainWindowHandle)
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  $boxCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::GenericBox'
  )
  $boxes = $main.FindAll($scope, $boxCond)
  $box = $null
  for ($i = 0; $i -lt $boxes.Count; $i++) {
    $candidate = $boxes.Item($i)
    if (-not $candidate.Current.IsOffscreen) {
      $box = $candidate
      break
    }
  }
  if ($null -eq $box) {
    Start-Sleep -Milliseconds 120
    continue
  }
  Restore-PreviousWindow $previous
  $buttonCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    'class Ui::RoundButton'
  )
  $buttons = $box.FindAll($scope, $buttonCond)
  $primary = $null
  $bestY = -2147483648
  for ($i = 0; $i -lt $buttons.Count; $i++) {
    $candidate = $buttons.Item($i)
    if ($candidate.Current.IsOffscreen -or -not $candidate.Current.IsEnabled) {
      continue
    }
    $name = $candidate.Current.Name
    if ($name -match 'Connect|Подключ|Use|Использ') {
      $primary = $candidate
      break
    }
    $candidateY = [int]$candidate.Current.BoundingRectangle.Y
    if ($candidateY -gt $bestY) {
      $primary = $candidate
      $bestY = $candidateY
    }
  }
  if ($null -eq $primary) {
    Start-Sleep -Milliseconds 120
    continue
  }
  if (Invoke-Element $primary) {
    exit 0
  }
  Start-Sleep -Milliseconds 120
}
Restore-PreviousWindow $previous
exit 1"#;

    script.replace("__LINK__", &ps_literal(value))
}

#[cfg(windows)]
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
function Normalize-Key($server, $port, $secret) {
  ('{0}:{1}:{2}' -f $server, $port, $secret).ToLowerInvariant()
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
    return 'unknown:row found'
  }
  return 'unknown:' + $labels
}
$targetKey = Normalize-Key __SERVER__ '__PORT__' __SECRET__
$previous = [ProtoSwitchNative]::GetForegroundWindow()
Start-Process 'tg://settings/data_and_storage/proxy/settings'
$box = Wait-ForClass 'class `anonymous namespace''::ProxiesBox' 6000
if ($null -eq $box) {
  Restore-PreviousWindow $previous
  exit 1
}
Restore-PreviousWindow $previous
$deadline = [DateTime]::UtcNow.AddMilliseconds(__TIMEOUT_MS__)
$best = 'missing:not found'
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
        .replace("__SECRET__", &ps_literal(&proxy.secret))
        .replace("__TIMEOUT_MS__", &timeout_ms)
}

#[cfg(windows)]
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
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn render_output(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    "без текста ошибки".to_string()
}

#[derive(Debug, Clone)]
pub struct TelegramInstallation {
    pub protocol_handler: Option<String>,
    pub executable_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let value = apply_proxy_command("tg://proxy?server=test&port=443&secret=abcd");
        assert!(value.contains("Start-Process 'tg://proxy?server=test&port=443&secret=abcd'"));
        assert!(value.contains("Restore-PreviousWindow $previous"));
        assert!(value.contains("$buttons = $box.FindAll($scope, $buttonCond)"));
    }

    #[test]
    #[cfg(windows)]
    fn formats_probe_command_for_proxy_settings() {
        let value = probe_proxy_status_command(
            &MtProtoProxy {
                server: "example.com".to_string(),
                port: 443,
                secret: "abcd".to_string(),
            },
            4,
        );
        assert!(value.contains("tg://settings/data_and_storage/proxy/settings"));
        assert!(value.contains("Get-TextSnapshot"));
        assert!(value.contains("available:"));
    }

    #[test]
    #[cfg(windows)]
    fn formats_cleanup_command_for_proxy_settings() {
        let value = cleanup_proxies_command(&[MtProtoProxy {
            server: "example.com".to_string(),
            port: 443,
            secret: "abcd".to_string(),
        }]);
        assert!(value.contains("tg://settings/data_and_storage/proxy/settings"));
        assert!(value.contains("class `anonymous namespace''::ProxyRow"));
        assert!(value.contains("Delete"));
    }
}
