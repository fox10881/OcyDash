//! "Run on startup" persistence via HKCU\...\CurrentVersion\Run.
//! Writes the current exe path with NO arguments → Windows launches the
//! headless tray service (the default mode) at logon.

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "OcyDash";

pub fn is_enabled() -> bool {
    let Ok(key) =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(RUN_KEY)
    else {
        return false;
    };
    key.get_value::<String, _>(VALUE_NAME)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

pub fn set(enabled: bool) -> Result<(), String> {
    let cu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = cu.create_subkey(RUN_KEY).map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // quoted — install path contains spaces; no args = headless mode
        key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        key.delete_value(VALUE_NAME).map_err(|e| e.to_string())
    }
}
