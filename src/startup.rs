//! "Run on startup" via a highest-privilege Task Scheduler logon task.
//!
//! ocydash.exe now carries requireAdministrator — Windows silently ignores
//! HKCU\...\Run entries that would trigger a UAC prompt at logon, so the
//! old registry key never fired. A `/sc onlogon /rl highest` task is the
//! only silent autostart path for an elevated app. The legacy Run value
//! is cleaned up so older installs can't double-launch.

const TASK_NAME: &str = "OcyDash";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "OcyDash";

fn schtasks(args: &[&str]) -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    // absolute System32 path — name-only resolution searches CWD/PATH,
    // and a planted schtasks.exe would run elevated
    std::process::Command::new(crate::deps::system32("schtasks.exe"))
        .args(args)
        // console tool — no flashing window from the GUI/headless parent
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .map_err(|e| e.to_string())
}

/// Task exists AND is not Disabled → autostart is on. A bare /query
/// succeeds for disabled tasks too, so parse the CSV status column.
pub fn is_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(crate::deps::system32("schtasks.exe"))
        .args(["/query", "/tn", TASK_NAME, "/fo", "csv", "/nh"])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            !s.contains("Disabled")
        }
        _ => false,
    }
}

pub fn set(enabled: bool) -> Result<(), String> {
    let _ = legacy_cleanup();
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // quoted path — Program Files contains spaces; no args = headless
        let tr = format!("\"{}\"", exe.display());
        // /ru <user> /it → the task runs inside THAT user's interactive
        // logon session (no stored password). Without it the task can run
        // in a non-interactive context: TEMP resolves to a system dir,
        // the sensor helper can't reach the session, and the classic
        // "works manually, frozen after reboot" bug appears.
        let ru = runas_principal();
        let mut args = vec![
            "/create",
            "/tn",
            TASK_NAME,
            "/tr",
            tr.as_str(),
            "/sc",
            "onlogon",
        ];
        if !ru.is_empty() {
            args.extend(["/ru", ru.as_str(), "/it"]);
        }
        args.extend(["/rl", "highest", "/f"]);
        if schtasks(&args)? {
            Ok(())
        } else {
            Err("schtasks /create failed".into())
        }
    } else if schtasks(&["/delete", "/tn", TASK_NAME, "/f"])? {
        Ok(())
    } else {
        Err("schtasks /delete failed".into())
    }
}

/// `DOMAIN\user` (or bare `user`) for schtasks /ru — the account that
/// owns the interactive logon session the task must run in.
fn runas_principal() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return String::new();
    }
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

/// Remove the HKCU Run value written by builds before Task Scheduler.
fn legacy_cleanup() -> Result<(), String> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;
    key.delete_value(VALUE_NAME).map_err(|e| e.to_string())
}
