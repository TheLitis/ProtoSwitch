use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, anyhow};
use sysinfo::{ProcessesToUpdate, System};

use crate::model::MtProtoProxy;

#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};

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
    let status = Command::new("cmd")
        .args(["/C", "start", "", &proxy.deep_link()])
        .status()
        .context("Не удалось вызвать cmd /C start для tg://proxy")?;

    if !status.success() {
        return Err(anyhow!("Telegram не принял tg://proxy"));
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn open_proxy_link(_proxy: &MtProtoProxy) -> anyhow::Result<()> {
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

#[derive(Debug, Clone)]
pub struct TelegramInstallation {
    pub protocol_handler: Option<String>,
    pub executable_path: Option<PathBuf>,
}
