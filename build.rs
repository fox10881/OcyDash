//! Embeds the Windows application icon + version info via the .rc script.
//! Locates the Windows SDK's rc.exe automatically if it isn't on PATH.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/ocydash.rc");
    println!("cargo:rerun-if-changed=assets/ocydash.ico");

    if which_on_path("rc.exe").is_none() {
        if let Some(dir) = sdk_rc_dir() {
            let path = std::env::var("PATH").unwrap_or_default();
            // safe: the build-script process is still single-threaded here
            unsafe { std::env::set_var("PATH", format!("{dir};{path}")) };
        }
    }
    let _ = embed_resource::compile("assets/ocydash.rc", embed_resource::NONE);
}

fn which_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|f| f.exists())
    })
}

/// Newest Windows Kits 10 bin\<ver>\x64 dir containing rc.exe.
fn sdk_rc_dir() -> Option<String> {
    let kits = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut vers: Vec<String> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    vers.sort();
    vers.reverse();
    for v in vers {
        let dir = format!(r"C:\Program Files (x86)\Windows Kits\10\bin\{v}\x64");
        if std::path::Path::new(&dir).join("rc.exe").exists() {
            return Some(dir);
        }
    }
    None
}
