//! Admin elevation: detection + relaunch-as-admin.

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;

    use crate::error::{AppError, AppResult};

    pub fn is_elevated() -> bool {
        unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
    }

    pub fn relaunch_as_admin() -> AppResult<()> {
        let exe = std::env::current_exe().map_err(AppError::Io)?;
        let exe_wide: Vec<u16> = OsString::from(exe.as_os_str())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let verb: Vec<u16> = OsString::from("runas")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let res = ShellExecuteW(
                Some(HWND(std::ptr::null_mut())),
                windows::core::PCWSTR(verb.as_ptr()),
                windows::core::PCWSTR(exe_wide.as_ptr()),
                windows::core::PCWSTR(std::ptr::null()),
                windows::core::PCWSTR(std::ptr::null()),
                SW_SHOWNORMAL,
            );
            // ShellExecuteW returns HINSTANCE > 32 on success.
            let raw = res.0 as isize;
            if raw <= 32 {
                return Err(AppError::Win32(format!(
                    "ShellExecuteW(runas) failed: HINSTANCE={raw}"
                )));
            }
        }
        std::process::exit(0);
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::error::{AppError, AppResult};

    pub fn is_elevated() -> bool {
        false
    }
    pub fn relaunch_as_admin() -> AppResult<()> {
        Err(AppError::Other("relaunch_as_admin not supported".into()))
    }
}

pub use imp::{is_elevated, relaunch_as_admin};
