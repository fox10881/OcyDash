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
pub fn resolve(rels: &[&str]) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for anc in dir.ancestors() {
        for rel in rels {
            let p = anc.join(rel);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}
