//! Windows admin elevation + SeDebugPrivilege check.

use anyhow::Result;

#[cfg(windows)]
mod imp {
    use anyhow::{Result, anyhow};

    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
        SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::Shell::IsUserAnAdmin;

    pub fn is_admin() -> bool {
        unsafe { IsUserAnAdmin().as_bool() }
    }

    pub fn try_enable_debug_privilege() -> Result<bool> {
        unsafe {
            let process = GetCurrentProcess();
            let mut token: HANDLE = HANDLE(std::ptr::null_mut());
            OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
                .map_err(|e| anyhow!("OpenProcessToken failed: {e}"))?;
            let mut luid = LUID::default();
            let lookup_res = LookupPrivilegeValueW(None, SE_DEBUG_NAME, &mut luid);
            if lookup_res.is_err() {
                let _ = CloseHandle(token);
                return Err(anyhow!("LookupPrivilegeValue(SE_DEBUG_NAME) failed"));
            }
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let adjust = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
            let _ = CloseHandle(token);
            adjust.map_err(|e| anyhow!("AdjustTokenPrivileges failed: {e}"))?;
            let last = windows::Win32::Foundation::GetLastError();
            // ERROR_NOT_ALL_ASSIGNED = 1300
            Ok(last.0 != 1300)
        }
    }

    pub fn is_64bit_process() -> bool {
        cfg!(target_pointer_width = "64")
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;
    pub fn is_admin() -> bool {
        false
    }
    pub fn try_enable_debug_privilege() -> Result<bool> {
        Ok(false)
    }
    pub fn is_64bit_process() -> bool {
        cfg!(target_pointer_width = "64")
    }
}

pub fn is_admin() -> bool {
    imp::is_admin()
}

pub fn try_enable_debug_privilege() -> Result<bool> {
    imp::try_enable_debug_privilege()
}

pub fn is_64bit_process() -> bool {
    imp::is_64bit_process()
}
