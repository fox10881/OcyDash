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
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const ORD_INIT: u32 = 863;
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
}

pub struct Hwinfo {
    _lib: isize, // keep the module loaded (never DeInit — see probe notes)
    count: CountFn,
    read: ReadFn,
    value: ValueFn,
    slots: Vec<Slot>,
    sensors: Vec<i32>,
}

fn cstr(b: &[u8]) -> String {
    let e = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..e]).trim().to_string()
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
                    return Err(format!("export ordinal {ord} ({name}) missing"));
                }
            }
            let rc = init(0);
            if rc != 0 {
                return Err(format!("HWi32_Init(0) -> {rc}"));
            }
            let n = count();
            if n <= 0 {
                return Err(format!("{n} sensors detected"));
            }
            // discovery sweep — one pass, cached forever
            let mut slots = Vec::new();
            let mut sensors = Vec::new();
            for i in 0..n.min(32) {
                read(i);
                let mut any = false;
                for ty in [TY_TEMP, TY_FAN, TY_POWER, TY_USAGE, TY_OTHER] {
                    for idx in 0..128 {
                        let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                        if value(ty, i, idx, &mut r as *mut _ as *mut c_void) && r.is_valid != 0 {
                            slots.push(Slot { sensor: i, ty, idx });
                            any = true;
                        }
                    }
                }
                if any {
                    sensors.push(i);
                }
            }
            crate::sensors::slog(&format!(
                "hwinfo native: {} sensors, {} live slots ({})",
                n,
                slots.len(),
                path.display()
            ));
            Ok(Hwinfo {
                _lib: lib as isize,
                count,
                read,
                value,
                slots,
                sensors,
            })
        }
    }

    /// Collect all cached readings — one ReadDataFromSensor per live
    /// sensor, then GetSensorValue per known-good slot.
    pub fn collect(&mut self) -> Vec<Reading> {
        let mut out = Vec::with_capacity(self.slots.len());
        unsafe {
            for &s in &self.sensors {
                (self.read)(s);
            }
            for sl in &self.slots {
                let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                if (self.value)(sl.ty, sl.sensor, sl.idx, &mut r as *mut _ as *mut c_void)
                    && r.is_valid != 0
                {
                    out.push(Reading {
                        loc: r.sensor_loc,
                        ty: sl.ty,
                        subtype: r.rsubtype,
                        sensor_name: cstr(&r.loc_name),
                        name: cstr(&r.rloc_name),
                        value: r.value,
                        units: cstr(&r.units),
                    });
                }
            }
        }
        out
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
