// Direct FFI probe of HWiNFO64.dll — bypasses the broken SystemInfos.exe
// .NET wrapper entirely. Struct layout mirrors the decompiled CPUIDSDK
// SENSOR_READ_RESULT (464 bytes — a smaller buffer = heap corruption).
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::mem::MaybeUninit;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const ORD_INIT: u32 = 863;
const ORD_DEINIT: u32 = 416;
const ORD_COUNT: u32 = 723;
const ORD_READ: u32 = 366;
const ORD_NAME: u32 = 840;
const ORD_VALUE: u32 = 906;

type InitFn = unsafe extern "C" fn(i32) -> i32;
type DeInitFn = unsafe extern "C" fn() -> i32;
type CountFn = unsafe extern "C" fn() -> i32;
type ReadFn = unsafe extern "C" fn(i32);
type NameFn = unsafe extern "C" fn(i32, *mut u8, i32);
type ValueFn = unsafe extern "C" fn(u32, i32, i32, *mut c_void) -> bool;

#[repr(C)]
struct SensorReadResult {
    is_valid: u32,
    value: f64,
    units: [u8; 32],
    // SENSOR_INFO
    sensor_loc: u32,      // ESENSOR_LOCATION
    sensor_loc2: u32,
    loc_index: i32,
    loc_name: [u8; 128],
    loc_name2: [u8; 128],
    reading_loc: u32,     // EREADING_LOCATION
    rloc_index: i32,
    rloc_index2: i32,
    rloc_name: [u8; 128],
    rtype: u32,           // EREADING_TYPE
    rsubtype: u32,        // union subtype (eOtherType etc.)
}

fn sym(h: *mut c_void, ord: u32) -> *mut c_void {
    unsafe { GetProcAddress(h, ord as *const u8).map(|f| f as *mut c_void).unwrap_or(std::ptr::null_mut()) }
}

fn cstr(b: &[u8]) -> String {
    let e = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..e]).to_string()
}

fn main() {
    unsafe {
        // probe the vendored copy first, then the installed SDK copy
        let cands = [
            r"E:\Project\Ocypus Display rust\vendor\SystemInfos\HWiNFO64.dll",
            r"C:\Program Files\Ocypus Display\resources\main\SDK\VC#\SystemInfos\vs2008\bin\x64\Release\HWiNFO64.dll",
        ];
        let mut h = std::ptr::null_mut();
        for c in cands {
            let w: Vec<u16> = c.encode_utf16().chain(std::iter::once(0)).collect();
            h = LoadLibraryW(w.as_ptr());
            if !h.is_null() {
                println!("loaded {c}");
                break;
            }
        }
        if h.is_null() {
            eprintln!("LoadLibraryW failed for all candidates");
            return;
        }
        let init: InitFn = std::mem::transmute(sym(h, ORD_INIT));
        let count: CountFn = std::mem::transmute(sym(h, ORD_COUNT));
        let read: ReadFn = std::mem::transmute(sym(h, ORD_READ));
        let value_fn: ValueFn = std::mem::transmute(sym(h, ORD_VALUE));
        let rc = init(0);
        println!("Init(0) -> {rc}");
        let n = count();
        println!("GetNumberOfDetectedSensors -> {n}");
        if n <= 0 {
            return;
        }
        let only_type: u32 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if only_type == 99 {
            // timing mode: how expensive is one full tick's worth of calls?
            let t0 = std::time::Instant::now();
            let mut hits = 0u32;
            for _ in 0..20 {
                for i in 0..n.min(30) {
                    read(i);
                    for ty in 1u32..=8 {
                        for idx in 0..128 {
                            let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                            if value_fn(ty, i, idx, &mut r as *mut _ as *mut c_void)
                                && r.is_valid != 0
                            {
                                hits += 1;
                            }
                        }
                    }
                }
            }
            let ms = t0.elapsed().as_millis();
            println!("20 sweeps x {hits} readings: {ms}ms total, {:.1}ms/sweep", ms as f64 / 20.0);
            return;
        }
        if only_type == 97 {
            // separate the cost: ReadDataFromSensor vs GetSensorValue
            let t0 = std::time::Instant::now();
            for _ in 0..50 {
                for i in 0..n.min(30) {
                    read(i);
                }
            }
            let rd = t0.elapsed().as_millis() as f64 / 50.0;
            let mut hits = 0;
            let t1 = std::time::Instant::now();
            for _ in 0..50 {
                for i in 0..6 {
                    for ty in 1u32..=8 {
                        for idx in 0..20 {
                            let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                            if value_fn(ty, i, idx, &mut r as *mut _ as *mut c_void)
                                && r.is_valid != 0
                            {
                                hits += 1;
                            }
                        }
                    }
                }
            }
            let vs = t1.elapsed().as_millis() as f64 / 50.0;
            println!("read-all-sensors: {rd:.1}ms | 6x8x20 value probes ({hits} hits): {vs:.1}ms");
            return;
        }
        if only_type == 98 {
            // time ONLY valid slots — the production collect pattern
            let mut valid: Vec<(i32, u32, i32)> = Vec::new();
            let mut sensors = Vec::new();
            for i in 0..n.min(30) {
                read(i);
                let mut any = false;
                for ty in 1u32..=8 {
                    for idx in 0..128 {
                        let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                        if value_fn(ty, i, idx, &mut r as *mut _ as *mut c_void) && r.is_valid != 0 {
                            valid.push((i, ty, idx));
                            any = true;
                        }
                    }
                }
                if any { sensors.push(i); }
            }
            println!("{} valid slots on {} sensors", valid.len(), sensors.len());
            for rounds in [50usize] {
                let t0 = std::time::Instant::now();
                let mut hits = 0;
                for _ in 0..rounds {
                    for &s in &sensors { read(s); }
                    for &(i, ty, idx) in &valid {
                        let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                        if value_fn(ty, i, idx, &mut r as *mut _ as *mut c_void) && r.is_valid != 0 {
                            hits += 1;
                        }
                    }
                }
                let ms = t0.elapsed().as_millis() as f64;
                println!("{rounds} collects x {hits} reads: {ms:.0}ms total = {:.2}ms/collect", ms / rounds as f64);
            }
            return;
        }
        for i in 0..n.min(30) {
            read(i);
            for ty in 1u32..=8 {
                if only_type != 0 && ty != only_type {
                    continue;
                }
                for idx in 0..128 {
                    let mut r = MaybeUninit::<SensorReadResult>::zeroed().assume_init();
                    if value_fn(ty, i, idx, &mut r as *mut _ as *mut c_void) && r.is_valid != 0 {
                        println!(
                            "[{i:>3}] ty={ty} idx={idx:>3} loc={:>3}({}) rloc={:>4}({}) sub={:>3} val={:>12.3} {}",
                            r.sensor_loc,
                            cstr(&r.loc_name),
                            r.reading_loc,
                            cstr(&r.rloc_name),
                            r.rsubtype,
                            r.value,
                            cstr(&r.units)
                        );
                    }
                }
            }
        }
        println!("done");
    }
}
