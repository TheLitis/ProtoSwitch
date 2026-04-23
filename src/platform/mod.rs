use std::path::Path;

use crate::model::AutostartMethod;

#[cfg(windows)]
#[path = "../windows.rs"]
mod windows_impl;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
pub use windows_impl::AutostartStatus;
#[cfg(target_os = "linux")]
pub use linux::AutostartStatus;
#[cfg(target_os = "macos")]
pub use macos::AutostartStatus;

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
        return windows_impl::install_autostart(executable);
    }

    #[cfg(target_os = "linux")]
    {
        return linux::install_autostart(executable);
    }

    #[cfg(target_os = "macos")]
    {
        return macos::install_autostart(executable);
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        return Err(anyhow::anyhow!("Unsupported operating system"));
    }
}

pub fn remove_autostart() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        return windows_impl::remove_autostart();
    }

    #[cfg(target_os = "linux")]
    {
        return linux::remove_autostart();
    }

    #[cfg(target_os = "macos")]
    {
        return macos::remove_autostart();
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        return Err(anyhow::anyhow!("Unsupported operating system"));
    }
}

pub fn query_autostart() -> AutostartStatus {
    #[cfg(windows)]
    {
        return windows_impl::query_autostart();
    }

    #[cfg(target_os = "linux")]
    {
        return linux::query_autostart();
    }

    #[cfg(target_os = "macos")]
    {
        return macos::query_autostart();
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        unreachable!()
    }
}
