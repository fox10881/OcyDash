//! Isolated sensor-feed test — strips EVERYTHING except the helper
//! spawn + bin read. Usage: `cargo run --release --example test_sensors`
//! Run from an elevated shell (the helper's manifest demands admin).
//!
//! Tries a matrix of spawn variations sequentially and prints, once per
//! second: declared json len, nonzero byte count, systemTime, and the
//! first cpu temperature — so a live stream is obvious at a glance.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HELPER: &str =
    r"E:\Project\Ocypus Display rust\vendor\SystemInfos\SystemInfos.exe";
const WATCH_SECS: u64 = 50;

struct Mode {
    name: &'static str,
    cwd: bool,
    env: bool,
    stdio_null: bool,
    no_window: bool,
}

fn temp_bin(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}.bin"))
}

fn scan<'a>(json: &'a str, key: &str) -> String {
    let pat = format!("\"{key}\":\"");
    let Some(i) = json.find(&pat) else { return String::new() };
    let rest = &json[i + pat.len()..];
    let end = rest.find('"').unwrap_or(rest.len().min(32));
    rest[..end].to_string()
}

fn spawn(mode: &Mode, bin_name: &str) -> Option<Child> {
    let mut c = Command::new(HELPER);
    c.args([bin_name, "1800", "0"]);
    if mode.cwd {
        c.current_dir(PathBuf::from(HELPER).parent().unwrap());
    }
    if mode.env {
        let t = std::env::temp_dir();
        c.env("TEMP", &t).env("TMP", &t);
    }
    if mode.stdio_null {
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    }
    if mode.no_window {
        c.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    match c.spawn() {
        Ok(ch) => {
            println!("  spawned pid={}", ch.id());
            Some(ch)
        }
        Err(e) => {
            println!("  SPAWN FAILED: {e}");
            None
        }
    }
}

fn watch(bin: &PathBuf) -> bool {
    let start = Instant::now();
    let mut last_time = String::new();
    let mut advanced = 0u32;
    loop {
        let t = start.elapsed().as_secs();
        let mut line = format!("[t+{t:>3}s] ");
        match fs::read(bin) {
            Ok(raw) if raw.len() >= 4 => {
                let n = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
                let nz = raw.iter().filter(|b| **b != 0).count();
                line.push_str(&format!("len={n} nonzero={nz}"));
                if n > 0 && n + 4 <= raw.len() {
                    if let Ok(j) = std::str::from_utf8(&raw[4..4 + n]) {
                        let st = scan(j, "systemTime");
                        let cpu = scan(j, "Tctl/Tdie");
                        if !st.is_empty() && st != last_time {
                            advanced += 1;
                            last_time = st.clone();
                        }
                        line.push_str(&format!(" systemTime={st} cpu={cpu} advancing={advanced}"));
                        if advanced >= 3 {
                            println!("{line}");
                            return true;
                        }
                    }
                }
            }
            _ => line.push_str("missing"),
        }
        println!("{line}");
        if t >= WATCH_SECS {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() {
    let modes = [
        Mode { name: "plain    ", cwd: false, env: false, stdio_null: false, no_window: false },
        Mode { name: "cwd      ", cwd: true,  env: false, stdio_null: false, no_window: false },
        Mode { name: "env      ", cwd: false, env: true,  stdio_null: false, no_window: false },
        Mode { name: "stdio    ", cwd: false, env: false, stdio_null: true,  no_window: false },
        Mode { name: "nowin    ", cwd: false, env: false, stdio_null: false, no_window: true  },
        Mode { name: "full(app)", cwd: true,  env: true,  stdio_null: true,  no_window: true  },
    ];
    let only: Option<String> = std::env::args().nth(1);
    for (i, m) in modes.iter().enumerate() {
        if let Some(o) = &only {
            if !m.name.trim().starts_with(o.as_str()) && !o.parse::<usize>().map(|n| n == i).unwrap_or(false) {
                continue;
            }
        }
        let bin_name = format!("IsoTest{i}");
        let bin = temp_bin(&bin_name);
        let _ = fs::remove_file(&bin);
        println!("=== mode {} [{}] bin={} ===", i, m.name, bin.display());
        let mut child = spawn(m, &bin_name);
        let ok = watch(&bin);
        println!("  --> {}", if ok { "LIVE — values advancing" } else { "DEAD — no write in window" });
        if let Some(c) = &mut child {
            let _ = c.kill();
        }
        // settle so the single-client driver frees before the next mode
        std::thread::sleep(Duration::from_millis(1500));
    }
}
