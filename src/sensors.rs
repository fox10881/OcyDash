//! CPU sensor feed via the official `SystemInfos.exe` helper (HWiNFO64.dll).
//!
//! The helper writes %TEMP%\<name>.bin = [u32 LE json_len][utf8 JSON]
//! containing `cpuLayout.temperature_list[]` with direct Tctl/Tdie values.
//! NOTE: the helper uses a memory-mapped file — contents update in real
//! time while the file mtime stays frozen, so aliveness is detected by
//! watching the embedded `systemTime`/`pid` fields, never the mtime.
//!
//! We reuse any live writer (e.g. while the official app runs) and only
//! spawn a new helper when no live feed exists.

use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Vendor install of the helper — dev/legacy fallback when our bundled
/// copy (installed as <exe>\SystemInfos\SystemInfos.exe) is absent.
const HELPER_DIR: &str =
    r"C:\Program Files\Ocypus Display\resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release";
const HELPER_EXE: &str = "SystemInfos.exe";
const OUR_NAME: &str = "OcypusRust";
/// Feed is dead when its content hash stays static this long.
const STALE_AFTER: Duration = Duration::from_secs(5);
/// A spawned helper is untouchable for this long — covers HWiNFO
/// kernel-driver init (the first write can lag 30s+ on a cold load)
/// AND caps respawn frequency. Killing it earlier is exactly the
/// write-once-then-die loop the watchdog produced in production.
const SPAWN_GRACE: Duration = Duration::from_secs(40);

#[derive(Debug, Default, Clone)]
pub struct SysSample {
    /// false = feed disconnected → UI renders "---" rather than frozen
    /// numbers. Only `extract()` (a fresh parse) sets this true.
    pub alive: bool,
    /// CPU (Tctl/Tdie) °C
    pub cpu_tctl: f64,
    /// CPU Die average °C
    pub cpu_die: f64,
    /// CPU utilization % (0-100)
    pub cpu_load: f64,
    /// CPU package power W
    pub cpu_power: f64,
    /// CPU CCD1 °C
    pub cpu_ccd1: f64,
    /// GPU temp °C
    pub gpu_temp: f64,
    /// GPU utilization %
    pub gpu_load: f64,
    /// GPU power W
    pub gpu_power: f64,
    /// Motherboard CPU-fan header RPM
    pub cpu_fan: f64,
    /// Pump RPM (best-effort: "pump"/"system"/fastest non-CPU mb header)
    pub pump_fan: f64,
    /// GPU Fan1 RPM
    pub gpu_fan: f64,
    /// e.g. "AMD Ryzen 7 9800X3D"
    pub cpu_name: String,
    /// e.g. "NVIDIA GeForce RTX 5070 Ti"
    pub gpu_name: String,
    /// RAM total GB
    pub ram_total: f64,
    /// RAM used GB
    pub ram_used: f64,
    /// RAM free GB
    pub ram_free: f64,
    /// RAM utilization %
    pub ram_load: f64,

    // Full reported lists per category — (display name, value) pairs.
    // Widgets use these for the manual sensor override (sensor_idx);
    // index 0 = the automatic pick above, n>0 = entry n-1 here.
    // Arc-wrapped so SysSample::clone() (feed.latest() per call) stays
    // a handful of refcount bumps, not ~150 String allocations.
    pub cpu_temps: Arc<Vec<(String, f64)>>,
    pub cpu_loads: Arc<Vec<(String, f64)>>,
    pub cpu_powers: Arc<Vec<(String, f64)>>,
    pub cpu_fans: Arc<Vec<(String, f64)>>,
    pub gpu_temps: Arc<Vec<(String, f64)>>,
    pub gpu_loads: Arc<Vec<(String, f64)>>,
    pub gpu_powers: Arc<Vec<(String, f64)>>,
    pub gpu_fans: Arc<Vec<(String, f64)>>,
    pub ram_loads: Arc<Vec<(String, f64)>>,
}

/// Sensor categories a widget can draw from (drives the Inspector dropdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorCat {
    CpuTemp,
    CpuLoad,
    CpuPower,
    CpuFan,
    GpuTemp,
    GpuLoad,
    GpuPower,
    GpuFan,
    RamLoad,
}

impl SysSample {
    /// (name, value) entries available for a category.
    pub fn sensor_list(&self, cat: SensorCat) -> &[(String, f64)] {
        match cat {
            SensorCat::CpuTemp => &self.cpu_temps,
            SensorCat::CpuLoad => &self.cpu_loads,
            SensorCat::CpuPower => &self.cpu_powers,
            SensorCat::CpuFan => &self.cpu_fans,
            SensorCat::GpuTemp => &self.gpu_temps,
            SensorCat::GpuLoad => &self.gpu_loads,
            SensorCat::GpuPower => &self.gpu_powers,
            SensorCat::GpuFan => &self.gpu_fans,
            SensorCat::RamLoad => &self.ram_loads,
        }
    }

    /// Resolve a widget's sensor_idx: 0 → the automatic `auto` value,
    /// n>0 → list entry n-1 (falling back to `auto` when out of range).
    pub fn override_value(&self, cat: SensorCat, idx: usize, auto: f64) -> f64 {
        if idx == 0 {
            return auto;
        }
        self.sensor_list(cat)
            .get(idx - 1)
            .map(|e| e.1)
            .unwrap_or(auto)
    }
}

#[derive(Deserialize)]
struct BinFile {
    #[serde(rename = "cpuLayout", default)]
    cpu: Layout,
    #[serde(rename = "graphics", default)]
    gpu: Layout,
    #[serde(rename = "memLayout", default)]
    mem: MemLayout,
    #[serde(rename = "systemTime", default)]
    system_time: String,
}

/// memLayout has GB values as direct fields (not sensor lists).
#[derive(Deserialize, Default)]
struct MemLayout {
    #[serde(default)]
    used: f64,
    #[serde(default)]
    total: f64,
    #[serde(default)]
    free: f64,
    #[serde(default)]
    utilization_list: Vec<SensorEntry>,
}

#[derive(Deserialize, Default)]
struct Layout {
    #[serde(default)]
    temperature_list: Vec<SensorEntry>,
    #[serde(default)]
    utilization_list: Vec<SensorEntry>,
    #[serde(rename = "powerDraw_list", default)]
    power_draw_list: Vec<SensorEntry>,
    #[serde(rename = "fanSpeed_list", default)]
    fan_speed_list: Vec<SensorEntry>,
    #[serde(default)]
    name_list: Vec<SensorEntry>,
}

#[derive(Deserialize)]
struct SensorEntry {
    #[serde(default)]
    sensorname: String,
    /// Numeric for sensors, a string for name_list entries — must stay
    /// untyped or one bad entry kills the whole bin parse.
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    checked: bool,
}

/// numeric value of an entry (handles "46.6" strings too)
fn num(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// display string of an entry (name_list keeps the name in `value`)
fn text_of(e: &SensorEntry) -> String {
    match &e.value {
        serde_json::Value::String(s) => s.clone(),
        _ => e.sensorname.clone(),
    }
}

pub struct SensorFeed {
    bin_path: PathBuf,
    last_hash: u64,
    last_sample: SysSample,
    last_change: Instant,
    /// last kill+respawn attempt — respawns are silent now that ocydash
    /// runs elevated, so a hung helper is recovered, not tolerated
    last_spawn: Option<Instant>,
    /// true only while fresh parses keep arriving — a static bin or a
    /// missing writer flips it and the UI drops to "---"
    alive: bool,
    /// true when THIS process holds Local\OcyDashSensorWriter — only the
    /// holder may kill/spawn the shared helper (see claim_writer)
    writer: bool,
    /// first-writer-claim purge already ran (kills helpers + wipes stale
    /// bins) — once per process, then normal recovery takes over
    purged: bool,
    /// a valid bin parse already landed — the NEXT distinct content
    /// proves a live writer and may publish. Guards the stale-bin flash:
    /// a leftover OcypusRust.bin parses identically forever, and without
    /// a baseline there's no way to tell it apart from a real writer
    seeded: bool,
    /// our spawned helper produced at least one successful parse —
    /// a respawn chain of pure non-writers means a deeper wedge
    /// (post-S3 driver state) and triggers the device-stack escalation
    spawn_produced_write: bool,
    /// consecutive spawns that never wrote a byte — at 2 we escalate:
    /// restart GPU device stacks + the HWiNFO driver, then respawn
    stall_strikes: u32,
    /// escalation already ran for THIS dead period — re-armed when the
    /// feed next goes live, so a kernel-wedged system doesn't get
    /// hammered with device restarts every ~50s forever
    escalated: bool,
    /// logged "reader mode" once — avoids spamming sensors.log
    reader_logged: bool,
    child: Option<Child>,
}

impl SensorFeed {
    pub fn new() -> Self {
        Self {
            bin_path: bin_path(OUR_NAME),
            last_hash: 0,
            last_sample: SysSample::default(),
            last_change: Instant::now(),
            last_spawn: None,
            alive: false,
            writer: false,
            purged: false,
            seeded: false,
            spawn_produced_write: false,
            stall_strikes: 0,
            escalated: false,
            reader_logged: false,
            child: None,
        }
    }

    /// false while the feed is stale/missing — drives the "---" state
    pub fn alive(&self) -> bool {
        self.alive
    }

    /// Pick the best live feed (prefer our own writer, else any other
    /// live Ocypus*.bin writer); when nothing is alive, kill every stale
    /// SystemInfos.exe for a clean slate and respawn ours — heartbeat
    /// recovery is silent now that ocydash runs elevated.
    pub fn ensure(&mut self) {
        for p in bin_candidates() {
            if bin_alive(&p) {
                slog(&format!("live feed: {}", p.display()));
                self.bin_path = p;
                self.last_change = Instant::now();
                self.alive = true;
                // bin_alive() watched systemTime advance — content change
                // already proven, so the next parse may publish directly
                self.seeded = true;
                self.note_live();
                return;
            }
        }
        self.alive = false;
        self.recover();
    }

    /// Called whenever the feed proves live again — resets the stall
    /// bookkeeping so a FUTURE dead period may escalate once more.
    fn note_live(&mut self) {
        self.stall_strikes = 0;
        self.escalated = false;
    }

    /// Kill orphaned/zombie helpers holding a bin, then respawn ours —
    /// but ONLY in the process holding the writer mutex. Headless and
    /// the editor each run a feed; without arbitration their
    /// `taskkill /IM` calls murdered each other's helper every few
    /// seconds — the actual cause of "values load once then freeze".
    fn recover(&mut self) {
        if !self.is_writer() {
            return;
        }
        // a just-spawned helper is still initializing (kernel driver
        // load) — never kill during the grace window
        if self.in_spawn_grace() {
            return;
        }
        // kill FIRST: a live client pins the driver service in
        // STOP_PENDING, so the strike-count + escalation must come
        // after the field is dead
        let had_spawn = self.child.is_some();
        self.last_spawn = Some(Instant::now());
        slog("no live feed — killing stale helpers, respawning");
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
        }
        kill_helpers();
        // give the killed helper a beat to release the single-client
        // HWiNFO driver — respawning into a still-held slot is what
        // parked every helper in GPU enumeration writing zero-bins
        std::thread::sleep(Duration::from_millis(1500));
        // our last spawn produced zero bytes — the helper parked in
        // enumeration (post-S3 device/driver wedge). Two strikes →
        // escalate once: restart the GPU stacks + HWiNFO driver, then
        // respawn into a reset field.
        if had_spawn && !self.spawn_produced_write {
            self.stall_strikes += 1;
            if self.stall_strikes >= 2 && !self.escalated {
                self.escalated = true;
                self.stall_strikes = 0;
                escalate_recovery();
            }
        }
        self.spawn_produced_write = false;
        // drop the stale bin so the fresh writer's first parse can't
        // flash dead values onto the panel
        let _ = fs::remove_file(bin_path(OUR_NAME));
        self.spawn();
        // repoint at our own bin — we may have adopted another writer earlier
        self.bin_path = bin_path(OUR_NAME);
    }

    /// Claim the global writer role once, then keep retrying while a
    /// reader — the owner hands it over on exit (WAIT_ABANDONED).
    fn is_writer(&mut self) -> bool {
        if !self.writer {
            self.writer = claim_writer();
            if self.writer {
                slog("claimed sensor-writer role");
                if !self.purged {
                    self.purged = true;
                    purge_field();
                }
            } else if !self.reader_logged {
                self.reader_logged = true;
                slog("reader mode — another OcyDash process owns the helper");
            }
        }
        self.writer
    }

    /// True while a spawned helper is still inside its warm-up window —
    /// staleness detected during this period means "still initializing",
    /// never "kill it".
    fn in_spawn_grace(&self) -> bool {
        self.last_spawn
            .map(|t| t.elapsed() < SPAWN_GRACE)
            .unwrap_or(false)
    }

    fn spawn(&mut self) {
        let Some(exe) = helper_path() else {
            slog(&format!(
                "{HELPER_EXE} not found in any layout (bundled/dev/vendor)"
            ));
            return;
        };
        let dir = exe
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        // pin the child's TEMP/TMP to the interactive user's temp — under
        // Task Scheduler or other non-interactive contexts our own TEMP
        // can point at a system dir, and the helper would drop its bin
        // somewhere we can't reliably read
        let temp = user_temp();
        match Command::new(&exe)
            .args([OUR_NAME, "1800", "0"])
            .current_dir(&dir)
            .env("TEMP", &temp)
            .env("TMP", &temp)
            // the helper spams Console.WriteLine during PCI/GPU enum —
            // with no console attached those writes must go to NUL or a
            // full pipe buffer would deadlock it mid-enumeration
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // SystemInfos.exe is console-subsystem — never show its window
            .creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
            )
            .spawn()
        {
            Ok(c) => {
                slog(&format!("spawned {} pid={} cwd={}", exe.display(), c.id(), dir.display()));
                crate::job::protect(c.id()); // reap even on taskkill /F
                self.child = Some(c);
            }
            Err(e) => {
                // ERROR_ELEVATION_REQUIRED (740): the exe manifest demands
                // admin. Delegate to `Start-Process` — the UAC prompt is
                // raised automatically by the exe's manifest. No wait.
                slog(&format!("normal spawn failed ({e}); elevating via Start-Process"));
                let ps = format!(
                    "$env:TEMP='{}'; $env:TMP='{}'; Start-Process -FilePath '{}' -ArgumentList '{}','1800','0' -WorkingDirectory '{}' -WindowStyle Hidden",
                    temp.display(),
                    temp.display(),
                    exe.display(),
                    OUR_NAME,
                    dir.display()
                );
                match Command::new("powershell")
                    .args(["-NoProfile", "-Command", &ps])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(
                        windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
                    )
                    .spawn()
                {
                    Ok(_) => slog("elevated spawn requested (UAC may appear)"),
                    Err(e2) => slog(&format!("elevated spawn failed: {e2}")),
                }
            }
        }
    }

    /// Poll the bin file; returns Some(sample) only when content changed.
    pub fn poll(&mut self) -> Option<SysSample> {
        let raw = fs::read(&self.bin_path).ok();
        let raw = match raw {
            Some(r) if r.len() >= 4 => r,
            _ => {
                // file missing entirely -> feed dead; rescan/adopt at
                // most every 2s (last_change doubles as the ensure clock
                // here — nothing else updates it while the file is gone)
                self.alive = false;
                if self.last_change.elapsed() > Duration::from_secs(2) {
                    self.ensure();
                    self.last_change = Instant::now();
                }
                return None;
            }
        };
        let h = hash(&raw);
        if h == self.last_hash {
            if self.last_change.elapsed() > STALE_AFTER {
                // static content — dead marker for the UI either way,
                // but only hunt for a new writer outside the warm-up
                // grace (a just-spawned helper is still initializing)
                self.alive = false;
                if !self.in_spawn_grace() {
                    self.ensure();
                }
                self.last_change = Instant::now();
            }
            return None;
        }
        let n = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        if n == 0 || n + 4 > raw.len() {
            // zero-filled / truncated bin — record the hash so the SAME
            // static content registers as stale next poll; without this
            // a never-writing helper's zeros would never trip the
            // watchdog (h != last_hash forever → ensure() unreachable)
            self.last_hash = h;
            return None;
        }
        let parsed: BinFile = match serde_json::from_slice(&raw[4..4 + n]) {
            Ok(p) => p,
            Err(e) => {
                // changing-but-unparseable bytes = writer mid-update —
                // proven alive, so refresh the staleness clock instead of
                // letting a transient torn read look like a dead feed
                self.last_hash = h;
                self.last_change = Instant::now();
                self.seeded = true; // content demonstrably changed
                self.alive = true;
                self.note_live();
                // never spam: first failure only per feed
                if self.last_sample.cpu_tctl == 0.0 {
                    slog(&format!("bin parse failed: {e}"));
                }
                return None;
            }
        };
        self.last_hash = h;
        self.last_change = Instant::now();
        if !self.seeded {
            // first valid content we've ever seen — it could be a
            // leftover bin from a dead session (parses identically
            // forever). Don't publish until the bin CHANGES once —
            // a live writer always produces a second distinct parse
            // within ~2s, a stale file never does. (mtime can't be
            // used: memory-mapped writes never touch it.)
            self.seeded = true;
            return None;
        }
        self.alive = true;
        self.spawn_produced_write = true;
        self.note_live();
        self.last_sample = extract(&parsed);
        Some(self.last_sample.clone())
    }

}

/// Shared state published by the background sensor thread.
#[derive(Clone)]
pub struct SensorHandle {
    inner: Arc<Mutex<SysSample>>,
    /// bumped whenever a new sample lands — cheap "changed" signal.
    generation: Arc<AtomicU64>,
}

impl SensorHandle {
    pub fn latest(&self) -> SysSample {
        self.inner.lock().unwrap().clone()
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Spawn the background sensor thread (owns SensorFeed; blocking spawn /
/// staleness checks never stall the caller). The thread exits when `stop`
/// is set — dropping SensorFeed then reaps the helper child it spawned.
pub fn start_feed(stop: Arc<AtomicBool>) -> SensorHandle {
    let handle = SensorHandle {
        inner: Arc::new(Mutex::new(SysSample::default())),
        generation: Arc::new(AtomicU64::new(0)),
    };
    let h = handle.clone();
    std::thread::spawn(move || {
        let mut feed = SensorFeed::new();
        feed.ensure();
        let mut was_alive = false;
        while !stop.load(Ordering::Relaxed) {
            match feed.poll() {
                Some(s) => {
                    if !was_alive {
                        slog("feed live");
                    }
                    was_alive = true;
                    *h.inner.lock().unwrap() = s;
                    h.generation.fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    // alive→dead transition: publish a dead marker once so
                    // the UI drops to "---" instead of freezing on the
                    // last values forever
                    if !feed.alive() && was_alive {
                        slog("feed dead — UI shows ---");
                        was_alive = false;
                        *h.inner.lock().unwrap() = SysSample::default();
                        h.generation.fetch_add(1, Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        }
        // feed drops here → its `impl Drop` kills the SystemInfos child
    });
    handle
}

fn hash(b: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

// ---- single sensor-writer across OcyDash processes --------------------
// Headless AND the editor each run a feed — but only ONE may own the
// SystemInfos helper (it writes a single shared bin every process reads).
// The owner holds Local\OcyDashSensorWriter for its lifetime; readers
// retry the claim each dead-feed cycle and get promoted when the owner
// exits (the wait returns WAIT_ABANDONED).

static WRITER_MUTEX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

fn writer_mutex() -> usize {
    use windows_sys::Win32::System::Threading::CreateMutexW;
    *WRITER_MUTEX.get_or_init(|| unsafe {
        let name: Vec<u16> = "Local\\OcyDashSensorWriter\0"
            .encode_utf16()
            .collect();
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            0
        } else {
            h as usize
        }
    })
}

/// Try to claim the writer role. Returns true immediately for the owner
/// (recursive re-acquire is fine), false for readers while a live owner
/// holds it, true again after the owner dies (abandoned mutex).
fn claim_writer() -> bool {
    use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let h = writer_mutex();
    if h == 0 {
        return true; // mutex unavailable — act as writer (safe default)
    }
    unsafe { matches!(WaitForSingleObject(h as _, 0), WAIT_OBJECT_0 | WAIT_ABANDONED) }
}

/// A writer is alive if the embedded `systemTime` field advances within a
/// short window (memory-mapped writes don't touch mtime). Missing/corrupt
/// files bail instantly — no 2.3s sleep on dead candidates.
fn bin_alive(path: &PathBuf) -> bool {
    let t1 = match read_system_time(path) {
        Some(t) => t,
        None => return false,
    };
    std::thread::sleep(Duration::from_millis(2300));
    match read_system_time(path) {
        Some(t2) => t2 != t1,
        None => false,
    }
}

fn read_system_time(path: &PathBuf) -> Option<String> {
    let raw = fs::read(path).ok()?;
    if raw.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    if n == 0 || n + 4 > raw.len() {
        return None;
    }
    let v: BinFile = serde_json::from_slice(&raw[4..4 + n]).ok()?;
    if v.system_time.is_empty() {
        None
    } else {
        Some(v.system_time)
    }
}

fn pick(list: &[SensorEntry], needle: &str) -> f64 {
    if list.is_empty() {
        return 0.0;
    }
    // mimic official app: prefer `checked`, else name match, else first.
    if let Some(e) = list.iter().find(|e| e.checked) {
        return num(&e.value);
    }
    if let Some(e) = list.iter().find(|e| e.sensorname.contains(needle)) {
        return num(&e.value);
    }
    num(&list[0].value)
}

/// Motherboard fan header matching an exact suffix (e.g. ": CPU").
fn pick_fan(list: &[SensorEntry], suffix: &str) -> f64 {
    list.iter()
        .find(|e| e.sensorname.ends_with(suffix))
        .map(|e| num(&e.value))
        .unwrap_or(0.0)
}

/// Best-effort pump detection: name containing "pump", then "system",
/// else the fastest mb header that isn't the CPU fan (pumps spin ~3-8k).
fn pick_pump(list: &[SensorEntry]) -> f64 {
    if let Some(e) = list.iter().find(|e| e.sensorname.to_lowercase().contains("pump")) {
        return num(&e.value);
    }
    if let Some(e) = list.iter().find(|e| e.sensorname.to_lowercase().contains("system")) {
        return num(&e.value);
    }
    list.iter()
        .filter(|e| !e.sensorname.ends_with(": CPU") && !e.sensorname.ends_with("CPU_OPT"))
        .map(|e| num(&e.value))
        .fold(0.0, f64::max)
}

fn to_pairs(list: &[SensorEntry]) -> Vec<(String, f64)> {
    list.iter()
        .map(|e| (e.sensorname.clone(), num(&e.value)))
        .collect()
}

fn extract(b: &BinFile) -> SysSample {
    let cpu = &b.cpu;
    let gpu = &b.gpu;
    let mut s = SysSample {
        alive: true, // a fresh parse is the ONLY proof of a live feed
        cpu_tctl: pick(&cpu.temperature_list, "Tctl/Tdie"),
        cpu_load: pick(&cpu.utilization_list, "Total CPU Usage"),
        cpu_power: pick(&cpu.power_draw_list, "Package"),
        gpu_temp: pick(&gpu.temperature_list, "GPU"),
        gpu_load: pick(&gpu.utilization_list, "GPU"),
        gpu_power: pick(&gpu.power_draw_list, "GPU"),
        cpu_fan: pick_fan(&cpu.fan_speed_list, ": CPU"),
        pump_fan: pick_pump(&cpu.fan_speed_list),
        gpu_fan: gpu.fan_speed_list.first().map(|e| num(&e.value)).unwrap_or(0.0),
        cpu_name: cpu.name_list.first().map(text_of).unwrap_or_default(),
        gpu_name: gpu.name_list.first().map(text_of).unwrap_or_default(),
        ram_total: b.mem.total,
        ram_used: b.mem.used,
        ram_free: b.mem.free,
        ram_load: {
            let v = pick(&b.mem.utilization_list, "Memory");
            if v > 0.0 {
                v
            } else if b.mem.total > 0.0 {
                b.mem.used / b.mem.total * 100.0
            } else {
                0.0
            }
        },
        cpu_temps: Arc::new(to_pairs(&cpu.temperature_list)),
        cpu_loads: Arc::new(to_pairs(&cpu.utilization_list)),
        cpu_powers: Arc::new(to_pairs(&cpu.power_draw_list)),
        cpu_fans: Arc::new(to_pairs(&cpu.fan_speed_list)),
        gpu_temps: Arc::new(to_pairs(&gpu.temperature_list)),
        gpu_loads: Arc::new(to_pairs(&gpu.utilization_list)),
        gpu_powers: Arc::new(to_pairs(&gpu.power_draw_list)),
        gpu_fans: Arc::new(to_pairs(&gpu.fan_speed_list)),
        ram_loads: Arc::new(to_pairs(&b.mem.utilization_list)),
        ..Default::default()
    };
    for e in &cpu.temperature_list {
        if e.sensorname.contains("Die (average)") {
            s.cpu_die = num(&e.value);
        } else if e.sensorname.contains("CCD1") {
            s.cpu_ccd1 = num(&e.value);
        }
    }
    s
}

/// The interactive user's temp dir — forced onto spawned helpers via
/// TEMP/TMP env, so their bin lands exactly where we read it.
fn user_temp() -> PathBuf {
    temp_roots()
        .into_iter()
        .next()
        .unwrap_or_else(std::env::temp_dir)
}

fn bin_path(name: &str) -> PathBuf {
    user_temp().join(format!("{name}.bin"))
}

/// Append a line to %APPDATA%\ocydash\sensors.log — the release build is
/// windowless, so this file is the only way to see what the feed did in
/// production (spawns, adoptions, recoveries, transitions).
pub(crate) fn slog(msg: &str) {
    eprintln!("[sensors] {msg}");
    let Ok(ap) = std::env::var("APPDATA") else { return };
    let dir = PathBuf::from(ap).join("ocydash");
    let _ = fs::create_dir_all(&dir);
    let line = format!("{} {msg}\n", chrono::Local::now().format("%H:%M:%S"));
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("sensors.log"))
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// First existing SystemInfos.exe across all layouts: `<exe>\SystemInfos\`
/// (installed) → `vendor\SystemInfos\` or a workspace `resources\` SDK
/// tree relative to any ancestor (dev) → the vendor's own install dir.
fn helper_path() -> Option<PathBuf> {
    crate::deps::resolve(&[
        r"SystemInfos\SystemInfos.exe",
        r"vendor\SystemInfos\SystemInfos.exe",
        r"resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release\SystemInfos.exe",
    ])
    .or_else(|| {
        let p = PathBuf::from(HELPER_DIR).join(HELPER_EXE);
        p.exists().then_some(p)
    })
}

/// Helper-class image names — anything that loads HWiNFO64.dll and
/// parks on the single-client HWiNFO_214 driver slot. SensorProbe is
/// our own debug binary; it still counts as a contender.
const HELPER_IMAGES: [&str; 2] = ["SystemInfos.exe", "SensorProbe.exe"];

/// Kill every helper-class process — any survivor is a stale writer or
/// a zombie holding a bin file / the driver slot. `/T` reaps children.
/// Silent now that we run elevated (a non-admin taskkill can't touch
/// the driver-protected process).
fn kill_helpers() {
    for image in HELPER_IMAGES {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/IM", image])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// First-claim field wipe — runs ONCE when this process becomes the
/// sensor writer. Kills every helper-class process (incl. leftovers a
/// user shell couldn't touch) and deletes every stale `Ocypus*.bin` in
/// all temp roots, so no dead file can ever be adopted or flash old
/// numbers on the panel. The HWiNFO driver is single-client: an
/// orphaned writer would park our fresh spawn in GPU enumeration
/// writing a 64KB zero bin forever — clean field first.
fn purge_field() {
    slog("writer purge: killing helper-class processes, wiping stale bins");
    kill_helpers();
    // give killed helpers a beat to release the driver slot
    std::thread::sleep(Duration::from_millis(1500));
    for dir in temp_roots() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let stale = p
                .file_name()
                .map(|n| {
                    let s = n.to_string_lossy();
                    s.starts_with("Ocypus") && s.ends_with(".bin")
                })
                .unwrap_or(false);
            if stale {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// Two consecutive spawned helpers wrote nothing — the stall is below
/// our process level (verified empirically: after an S3 resume the
/// helper's GPU/monitor enumeration can deadlock in the kernel — the
/// official binary hangs identically; `amduw23g` was seen failing to
/// unload, the iGPU stuck Off Line, DISPLAY devices surprise-removed).
/// Recovery attempt: restart every display-adapter device stack and the
/// HWiNFO kernel driver, then let the normal respawn try once more on
/// a reset field. Runs at most once per dead period. A deeper wedge
/// (hung EC/kernel driver that ignores restarts) still ends at "---"
/// until reboot — logged loudly so it's diagnosable.
fn escalate_recovery() {
    slog("STALL ESCALATION: restarting display adapters + HWiNFO driver");
    // restart display adapters — wakes D3-stuck iGPUs and re-runs a
    // wedged vendor driver stack (screen may flicker once)
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | \
             ForEach-Object { pnputil /restart-device $_.InstanceId }",
        ])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // restart the HWiNFO kernel driver (service name is versioned —
    // HWiNFO_214 etc. — so resolve it dynamically); a live client must
    // be dead first or the service pins in STOP_PENDING
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_SystemDriver -Filter \"Name like 'HWiNFO%'\" \
             | Where-Object {$_.State -eq 'Running'} | ForEach-Object { \
             sc.exe stop $_.Name | Out-Null; Start-Sleep 3; \
             sc.exe start $_.Name | Out-Null }",
        ])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    slog("escalation done — respawning on reset field");
}

/// Elevated self-clean at headless startup — the app's own admin token
/// can kill what the user's shell cannot: previous ocydash.exe
/// instances (they hold the single-instance mutex AND their helper)
/// plus orphaned helper-class processes parked on the single-client
/// HWiNFO driver. Called from main() BEFORE run_headless so the fresh
/// instance inherits a clean field without any user intervention.
///
/// Only OLDER ocydash instances are killed (StartTime strictly before
/// ours) — a launch race can't make two fresh instances murder each
/// other; equal timestamps fall through to the mutex.
pub fn startup_cleanup() {
    slog("startup cleanup: killing older ocydash + helper processes");
    // older ocydash.exe instances — powershell for StartTime ordering
    let me = std::process::id();
    let ps = format!(
        "$s=Get-Process -Id {me} -ErrorAction SilentlyContinue; \
         Get-Process ocydash -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.Id -ne {me} -and $s -ne $null -and \
         $_.StartTime -lt $s.StartTime }} | Stop-Process -Force"
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    kill_helpers();
}

/// Temp roots to scan — explicit user-profile paths come first because
/// elevation / Task Scheduler can make `temp_dir()` resolve to a system
/// dir while SystemInfos.exe (and every other writer) still lands in the
/// INTERACTIVE user's `C:\Users\<user>\AppData\Local\Temp`.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(up) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(up).join(r"AppData\Local\Temp"));
    }
    if let (Ok(drive), Ok(user)) =
        (std::env::var("SystemDrive"), std::env::var("USERNAME"))
    {
        roots.push(
            PathBuf::from(drive)
                .join("Users")
                .join(user)
                .join(r"AppData\Local\Temp"),
        );
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(la).join("Temp"));
    }
    // our own env-derived temp (where OUR spawned helper writes — it
    // inherits our env) plus the system temp for SYSTEM-context writers
    roots.push(std::env::temp_dir());
    roots.push(PathBuf::from(r"C:\Windows\Temp"));
    roots
}

/// All plausible live feed files: our own bin first, then any other
/// `Ocypus*.bin` writer (official app, probe helpers) in every temp root
/// the helper can land in. Roots are deduped case-insensitively.
fn bin_candidates() -> Vec<PathBuf> {
    let mut out = vec![bin_path(OUR_NAME)];
    let mut seen = std::collections::HashSet::new();
    for dir in temp_roots() {
        if !seen.insert(dir.to_string_lossy().to_lowercase()) {
            continue;
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let is_bin = p
                    .file_name()
                    .map(|n| {
                        let s = n.to_string_lossy();
                        s.starts_with("Ocypus") && s.ends_with(".bin")
                    })
                    .unwrap_or(false);
                if is_bin && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

impl Drop for SensorFeed {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            // SystemInfos.exe may be driver-protected; best-effort kill
            // (mirrors official app behaviour).
            let _ = c.kill();
        }
    }
}
