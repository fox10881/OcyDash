//! Native HWiNFO telemetry — direct FFI into the vendored `HWiNFO64.dll`
//! engine, bypassing the flaky `SystemInfos.exe` .NET wrapper entirely.
//!
//! The wrapper deadlocks on this hardware: its worker thread silently
//! retries `HWInfo.Init` then sleeps forever (WriteLog is a no-op), while
//! a parallel WMI task prints the misleading [DEBUG] PCI lines. Calling
//! the DLL ordinals directly from Rust works deterministically.
//!
//! Exported ordinals (from the decompiled CPUIDSDK delegates):
//!   863 Init(i32) -> i32 (0 = ok)      416 DeInit()
//!   723 GetNumberOfDetectedSensors     366 ReadDataFromSensor(i32)
//!   840 GetSensorName(i32, buf, sz)    906 GetSensorValue(ty, sensor, idx, &mut result)
//!
//! SENSOR_READ_RESULT mirrors the managed struct — bool(u32) IsValid,
//! double value, char szUnits[32], then SENSOR_INFO with sensor-location
//! enum, two 128-char name fields, reading-location enum + name, and the
//! reading-type union. ~464 bytes total — a smaller buffer gets heap-
//! corrupted by the callee (verified empirically: earlier truncated
//! layouts crashed at DeInit / mid-enumeration).

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const ORD_INIT: u32 = 863;
const ORD_DEINIT: u32 = 416;
const ORD_COUNT: u32 = 723;
const ORD_READ: u32 = 366;
const ORD_VALUE: u32 = 906;

// EREADING_TYPE
pub const TY_TEMP: u32 = 1;
pub const TY_FAN: u32 = 3;
pub const TY_POWER: u32 = 5;
pub const TY_USAGE: u32 = 7;
pub const TY_OTHER: u32 = 8;

// ESENSOR_LOCATION
pub const LOC_SYSTEM: u32 = 10;
pub const LOC_CPU: u32 = 11;
pub const LOC_MOBO: u32 = 12;
pub const LOC_GPU: u32 = 13;
pub const LOC_COOLER: u32 = 22;

type InitFn = unsafe extern "C" fn(i32) -> i32;
type DeInitFn = unsafe extern "C" fn();
type CountFn = unsafe extern "C" fn() -> i32;
type ReadFn = unsafe extern "C" fn(i32);
type ValueFn = unsafe extern "C" fn(u32, i32, i32, *mut c_void) -> bool;

#[repr(C)]
struct SensorReadResult {
    is_valid: u32,
    value: f64,
    units: [u8; 32],
    sensor_loc: u32,
    _sensor_loc2: u32,
    _loc_index: i32,
    loc_name: [u8; 128],
    _loc_name2: [u8; 128],
    reading_loc: u32,
    _rloc_index: i32,
    _rloc_index2: i32,
    rloc_name: [u8; 128],
    _rtype: u32,
    rsubtype: u32,
}

/// One live reading extracted from the driver.
#[derive(Debug, Clone)]
pub struct Reading {
    /// ESENSOR_LOCATION — routes the reading to a layout category
    pub loc: u32,
    /// EREADING_TYPE (1=temp,3=fan,5=power,7=usage,8=other)
    pub ty: u32,
    /// OTHER subtype (OTHER_MEMORY_* etc.) — only meaningful for ty=8
    pub subtype: u32,
    /// device the sensor sits on ("AMD Ryzen 7 9800X3D", "NVIDIA …")
    pub sensor_name: String,
    /// the reading's own label ("CPU (Tctl/Tdie)", "GPU Fan1", …)
    pub name: String,
    pub value: f64,
    pub units: String,
}

/// A cached (sensor,type,index) triple that returned a valid reading
/// during discovery — per-tick collection re-reads only these, not the
/// full 18×8×128 discovery sweep the managed helper ran every second.
#[derive(Clone, Copy)]
struct Slot {
    sensor: i32,
    ty: u32,
    idx: i32,
    /// hot slots drive the auto-picked display fields — collected every
    /// tick. Cold slots (override dropdowns, secondary rails) refresh
    /// every FULL_EVERY ticks; their last values are re-emitted between
    /// reads. Each driver call is ~0.37ms, so this is where CPU lives.
    hot: bool,
}

/// Cold slots refresh every Nth collect — their values feed manual
/// widget overrides where a ~10s lag is invisible.
const FULL_EVERY: u64 = 60;

pub struct Hwinfo {
    _lib: isize, // keep the module loaded (never DeInit — see probe notes)
    count: CountFn,
    read: ReadFn,
    value: ValueFn,
    slots: Vec<Slot>,
    /// last emitted value per slot — cold slots re-emit cached values
    /// between their scheduled reads
    cache: Vec<Reading>,
    /// sensors owning ≥1 hot slot — (sensor, refresh_interval). Each
    /// ReadDataFromSensor is a REAL hardware poll (~6.5ms), so slow-moving
    /// devices (mobo fans, memory, cooler) refresh every 3rd tick while
    /// CPU/GPU stay every-tick. GetSensorValue itself is ~free.
    hot_sensors: Vec<(i32, u64)>,
    /// sensors owning ≥1 slot at all — ReadDataFromSensor on full ticks
    all_sensors: Vec<i32>,
    tick: u64,
}

fn cstr(b: &[u8]) -> String {
    let e = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..e]).trim().to_string()
}

/// Discovery filter — keep only readings that map onto a SysSample list.
/// C-states, VID rails, effective clocks, disk SMART and chipset misc are
/// 90% of the sweep and never displayed; skipping them is the difference
/// between ~4% and ~1% CPU on the sensor thread.
fn keep(loc: u32, ty: u32, name: &str) -> bool {
    match loc {
        LOC_CPU => {
            matches!(ty, TY_TEMP | TY_POWER | TY_USAGE | TY_FAN)
                // C-state residencies + per-thread Utility duplicates —
                // the utilization list only needs the real usage entries
                && !name.contains("Residency")
        }
        LOC_GPU => matches!(ty, TY_TEMP | TY_FAN | TY_POWER | TY_USAGE),
        LOC_MOBO => matches!(ty, TY_TEMP | TY_FAN),
        LOC_COOLER => ty == TY_FAN,
        LOC_SYSTEM => ty == TY_OTHER && name.starts_with("Physical Memory"),
        _ => false,
    }
}

/// Hot slots = the fields extract()/pick() auto-selects for the panel —
/// collected every tick. Everything else is cold (~10s cadence).
fn is_hot(loc: u32, name: &str) -> bool {
    match loc {
        LOC_CPU => {
            name.contains("Tctl/Tdie")
                || name.contains("Die (average)")
                || name.contains("CCD1")
                || name.contains("Total CPU Usage")
                || name.contains("Package Power")
        }
        LOC_GPU => {
            name == "GPU Temperature"
                || name == "GPU Utilization"
                || name == "GPU Power"
                || name == "GPU Fan1"
        }
        LOC_MOBO => name == "CPU" || name == "System 2",
        // cooler fans + the 3 memory readings surviving keep()
        LOC_COOLER | LOC_SYSTEM => true,
        _ => false,
    }
}

impl Hwinfo {
    /// Load the DLL, Init(0), and discover every valid reading slot.
    /// Must be called elevated (HWiNFO_214 is a kernel driver) and as
    /// the single client — run after the helper purge/settle.
    pub fn init() -> Result<Hwinfo, String> {
        let path = dll_path().ok_or("HWiNFO64.dll not found in any layout")?;
        unsafe {
            let w: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let lib = LoadLibraryW(w.as_ptr());
            if lib.is_null() {
                return Err(format!("LoadLibraryW failed: {}", path.display()));
            }
            let sym = |ord: u32| -> *mut c_void {
                GetProcAddress(lib, ord as *const u8)
                    .map(|f| f as *mut c_void)
                    .unwrap_or(std::ptr::null_mut())
            };
            let init: InitFn = std::mem::transmute(sym(ORD_INIT));
            let count: CountFn = std::mem::transmute(sym(ORD_COUNT));
            let read: ReadFn = std::mem::transmute(sym(ORD_READ));
            let value: ValueFn = std::mem::transmute(sym(ORD_VALUE));
            for (ord, name) in [
                (ORD_INIT, "Init"),
                (ORD_COUNT, "Count"),
                (ORD_READ, "Read"),
                (ORD_VALUE, "Value"),
            ] {
                if sym(ord).is_null() {
                    FreeLibrary(lib);
                    return Err(format!("export ordinal {ord} ({name}) missing"));
                }
            }
            // Init acquires the SINGLE-CLIENT driver slot — on every
            // error path from here we must DeInit + FreeLibrary or the
            // slot stays held and the SystemInfos.exe fallback can never
            // acquire it (sensors dead until process restart)
            let rc = init(0);
            if rc != 0 {
                FreeLibrary(lib);
                return Err(format!("HWi32_Init(0) -> {rc}"));
            }
            let n = count();
            if n <= 0 {
                let deinit: Option<DeInitFn> = (!sym(ORD_DEINIT).is_null())
                    .then(|| std::mem::transmute::<*mut c_void, DeInitFn>(sym(ORD_DEINIT)));
                if let Some(d) = deinit {
                    d();
                }
                FreeLibrary(lib);
                return Err(format!("{n} sensors detected"));
            }
            // discovery sweep — one pass, cached forever. Filtered to the
            // categories the UI actually renders (229 slots -> ~60): every
            // kept slot is one driver round-trip per tick, so each dropped
            // voltage rail / C-state / disk SMART entry is real CPU saved.
            let mut slots = Vec::new();
            let mut cache = Vec::new();
            let mut all_sensors = Vec::new();
            for i in 0..n.min(32) {
                read(i);
                let mut any = false;
                for ty in [TY_TEMP, TY_FAN, TY_POWER, TY_USAGE, TY_OTHER] {
                    for idx in 0..128 {
                        let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                        if !(value(ty, i, idx, &mut r as *mut _ as *mut c_void) && r.is_valid != 0)
                        {
                            continue;
                        }
                        let name = cstr(&r.rloc_name);
                        if !keep(r.sensor_loc, ty, &name) {
                            continue;
                        }
                        let hot = is_hot(r.sensor_loc, &name);
                        slots.push(Slot {
                            sensor: i,
                            ty,
                            idx,
                            hot,
                        });
                        cache.push(Reading {
                            loc: r.sensor_loc,
                            ty,
                            subtype: r.rsubtype,
                            sensor_name: cstr(&r.loc_name),
                            name,
                            value: r.value,
                            units: cstr(&r.units),
                        });
                        any = true;
                    }
                }
                if any {
                    all_sensors.push(i);
                }
            }
            // hot sensors + their refresh interval (1 = every tick)
            let mut seen: std::collections::BTreeMap<i32, u64> = Default::default();
            for (k, sl) in slots.iter().enumerate() {
                if !sl.hot {
                    continue;
                }
                let int = match cache[k].loc {
                    // ~6.5ms per hardware poll — spread the cost. 2s CPU/GPU
                    // updates still feel live on a cooler LCD; slow sensors
                    // (mobo fans / memory / cooler) drift on 4s.
                    LOC_CPU | LOC_GPU => 2,
                    _ => 4,
                };
                seen.entry(sl.sensor).or_insert(int);
            }
            let hot_sensors: Vec<(i32, u64)> = seen.into_iter().collect();
            let hot_n = slots.iter().filter(|s| s.hot).count();
            crate::sensors::slog(&format!(
                "hwinfo native: {} sensors, {} slots ({} hot) ({})",
                n,
                slots.len(),
                hot_n,
                path.display()
            ));
            Ok(Hwinfo {
                _lib: lib as isize,
                count,
                read,
                value,
                slots,
                cache,
                hot_sensors,
                all_sensors,
                tick: 0,
            })
        }
    }

    /// Collect readings. Only `ReadDataFromSensor` costs (~6.5ms hardware
    /// poll) — `GetSensorValue` on a just-read sensor is ~free, so every
    /// tick refreshes ALL slots of every sensor polled this tick. Sensor
    /// cadence: CPU/GPU every 2nd tick, mobo/memory/cooler every 4th,
    /// everything (cold sensors) every FULL_EVERY-th. Manual overrides on
    /// a polled sensor therefore stay ~2s-fresh; only never-polled cold
    /// sensors lag.
    pub fn collect(&mut self) -> &[Reading] {
        self.tick += 1;
        let full = self.tick % FULL_EVERY == 1;
        let mut refreshed = [false; 32];
        unsafe {
            if full {
                for &s in &self.all_sensors {
                    (self.read)(s);
                    if (s as usize) < refreshed.len() {
                        refreshed[s as usize] = true;
                    }
                }
            } else {
                for &(s, int) in &self.hot_sensors {
                    if self.tick.is_multiple_of(int) {
                        (self.read)(s);
                        if (s as usize) < refreshed.len() {
                            refreshed[s as usize] = true;
                        }
                    }
                }
            }
            for k in 0..self.slots.len() {
                let sl = self.slots[k];
                // sensor is i32 and discovery bounds it to 0..32 — guard
                // anyway so a bad index can never wrap into an OOB panic
                let si = sl.sensor as usize;
                if !full && (sl.sensor < 0 || si >= refreshed.len() || !refreshed[si]) {
                    continue;
                }
                let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                if (self.value)(sl.ty, sl.sensor, sl.idx, &mut r as *mut _ as *mut c_void)
                    && r.is_valid != 0
                {
                    self.cache[k] = Reading {
                        loc: r.sensor_loc,
                        ty: sl.ty,
                        subtype: r.rsubtype,
                        sensor_name: cstr(&r.loc_name),
                        name: cstr(&r.rloc_name),
                        value: r.value,
                        units: cstr(&r.units),
                    };
                }
            }
        }
        &self.cache
    }
}

/// First existing HWiNFO64.dll across all layouts (installed SystemInfos
/// dir → vendored copy → workspace SDK tree → vendor install dir).
fn dll_path() -> Option<PathBuf> {
    crate::deps::resolve(&[
        r"SystemInfos\HWiNFO64.dll",
        r"vendor\SystemInfos\HWiNFO64.dll",
        r"resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release\HWiNFO64.dll",
    ])
    .or_else(|| {
        let p = PathBuf::from(
            r"C:\Program Files\Ocypus Display\resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release\HWiNFO64.dll",
        );
        p.exists().then_some(p)
    })
}
