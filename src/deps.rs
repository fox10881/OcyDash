//! Locate bundled/vendor binaries in BOTH layouts without hardcoding a
//! single location:
//!
//! - **production**: paths relative to the running exe —
//!   `<exe>\ffmpeg.exe`, `<exe>\SystemInfos\SystemInfos.exe`
//! - **dev**: the same relative paths tried against every ANCESTOR of the
//!   exe dir — `target\release\ocydash.exe` walks up to the workspace
//!   root, so `<ws>\vendor\ffmpeg\ffmpeg.exe` and
//!   `<ws>\vendor\SystemInfos\SystemInfos.exe` resolve naturally.

use std::path::PathBuf;

/// First existing candidate: all `rels` against the exe dir first
/// (nearest/production wins), then each ancestor upward (dev layout).
/// Two guards against plant-and-load-elevated:
/// - the filesystem root itself is NEVER searched (drive roots are
///   standard-user writable — a planted `C:\vendor\...\HWiNFO64.dll`
///   would otherwise LoadLibrary into the elevated process);
/// - the climb stops once a directory containing `Cargo.toml` has been
///   searched — that's the dev-workspace root; anything above it is
///   outside the project's trust boundary.
pub fn resolve(rels: &[&str]) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for anc in dir.ancestors().take_while(|a| a.parent().is_some()) {
        for rel in rels {
            let p = anc.join(rel);
            if p.exists() {
                return Some(p);
            }
        }
        if anc.join("Cargo.toml").is_file() {
            break;
        }
    }
    None
}

/// Absolute path inside `C:\Windows\System32` (or wherever the OS puts
/// it). `Command::new("taskkill")` resolves via CWD/PATH search — a
/// same-named exe planted in a writable dir would run ELEVATED. Every
/// system binary we spawn must go through this (or `windir`).
pub fn system32(name: &str) -> PathBuf {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if n > 0 && (n as usize) < buf.len() {
        PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])).join(name)
    } else {
        PathBuf::from(r"C:\Windows\System32").join(name)
    }
}

/// Absolute path inside the Windows directory (for `explorer.exe` —
/// it lives in `C:\Windows`, not System32).
pub fn windir(name: &str) -> PathBuf {
    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
    let mut buf = [0u16; 260];
    let n = unsafe { GetWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if n > 0 && (n as usize) < buf.len() {
        PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])).join(name)
    } else {
        PathBuf::from(r"C:\Windows").join(name)
    }
}

/// Escape a value for interpolation inside a single-quoted PowerShell
/// string literal — `'`-quoted literals only escape `'` by doubling it.
/// Paths like `C:\Users\o'brien\...` (apostrophes are legal in Windows
/// account names) would otherwise break quoting → PS injection.
pub fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}
