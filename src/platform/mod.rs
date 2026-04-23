use std::path::Path;

use crate::model::AutostartMethod;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
#[path = "../windows.rs"]
mod windows_impl;

#[cfg(target_os = "linux")]
pub use linux::AutostartStatus;
#[cfg(target_os = "macos")]
pub use macos::AutostartStatus;
#[cfg(windows)]
pub use windows_impl::AutostartStatus;

pub fn current_os_label() -> &'static str {
    #[cfg(windows)]
    {
        return "windows";
    }

    #[cfg(target_os = "linux")]
    {
        return "linux";
    }

    #[cfg(target_os = "macos")]
    {
        return "macos";
    }

    #[allow(unreachable_code)]
    "unknown"
}

pub fn install_autostart(executable: &Path) -> anyhow::Result<AutostartMethod> {
    #[cfg(windows)]
    {
        windows_impl::install_autostart(executable)
    }

    #[cfg(target_os = "linux")]
    {
        linux::install_autostart(executable)
    }

    #[cfg(target_os = "macos")]
    {
        macos::install_autostart(executable)
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(anyhow::anyhow!("Unsupported operating system"))
    }
}

pub fn remove_autostart() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows_impl::remove_autostart()
    }

    #[cfg(target_os = "linux")]
    {
        linux::remove_autostart()
    }

    #[cfg(target_os = "macos")]
    {
        macos::remove_autostart()
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(anyhow::anyhow!("Unsupported operating system"))
    }
}

pub fn query_autostart() -> AutostartStatus {
    #[cfg(windows)]
    {
        windows_impl::query_autostart()
    }

    #[cfg(target_os = "linux")]
    {
        linux::query_autostart()
    }

    #[cfg(target_os = "macos")]
    {
        macos::query_autostart()
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        unreachable!()
    }
}
