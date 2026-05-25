use std::process::ExitCode;

use anyhow::Result;
use openforge_core::ProcessSnapshot;

use crate::cli::DoctorArgs;
use crate::elevation;
use crate::term;

pub fn run(_args: &DoctorArgs) -> Result<ExitCode> {
    term::header("openforge-discover doctor");
    let mut all_ok = true;

    if elevation::is_64bit_process() {
        term::ok("64-bit process");
    } else {
        term::fail("64-bit process", "this binary is 32-bit");
        all_ok = false;
    }

    if elevation::is_admin() {
        term::ok("administrator elevation");
    } else {
        term::fail("administrator elevation", "run as administrator");
        all_ok = false;
    }

    match elevation::try_enable_debug_privilege() {
        Ok(true) => term::ok("SeDebugPrivilege enabled"),
        Ok(false) => term::warn(
            "SeDebugPrivilege",
            "not granted (may be unavailable on this account)",
        ),
        Err(e) => {
            term::fail("SeDebugPrivilege", e);
            all_ok = false;
        }
    }

    match ProcessSnapshot::capture() {
        Ok(snap) => {
            term::ok(&format!(
                "process snapshot: {} entries visible",
                snap.entries.len()
            ));
        }
        Err(e) => {
            term::fail("process snapshot", e);
            all_ok = false;
        }
    }

    if all_ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}
