// Release builds are window-subsystem: no console window for the tray
// service or the editor. Debug builds keep the console for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! OcyDash — tray/headless service + on-demand egui editor driving the
//! Sigma PRO L24 LCD.
//!
//! Modes:
//!   ocydash.exe            → headless: tray icon + render/sensor/
//!                            serial threads (~27MB RSS, no GUI libs)
//!   ocydash.exe --editor   → eframe layout editor (spawned by the
//!                            tray; edits the theme JSON that the
//!                            headless loop hot-reloads)
//!   [--port COMx]          → override device port (headless)
//!
//! The eframe editor never touches COM3 — the headless process owns the
//! port exclusively, so closing the editor frees all GUI memory while
//! streaming continues uninterrupted.

mod app;
mod background;
mod display;
mod headless;
mod icon;
mod job;
mod protocol;
mod render;
mod sensors;
mod startup;
mod theme;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    if std::env::args().any(|a| a == "--editor" || a == "--edit") {
        return run_editor();
    }
    let code = headless::run_headless(parse_port_arg());
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn run_editor() -> eframe::Result<()> {
    if !editor_single_instance() {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let feed = sensors::start_feed(stop.clone());

    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OcyDash — Editor")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(eframe::egui::IconData {
                rgba: icon::disc_rgba(64),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "OcyDash — Editor",
        opts,
        Box::new(move |cc| {
            // no EngineHandle — the editor only edits/saves the theme JSON;
            // the headless service owns COM3 and hot-reloads it
            Ok(Box::new(app::EditorApp::new(cc, feed, None, stop)))
        }),
    )
}

/// Only one editor instance at a time: named mutex, and if it is already
/// held we raise the existing "OcyDash — Editor" window and exit quietly.
fn editor_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    let name: Vec<u16> = "Local\\OcyDashEditor\0".encode_utf16().collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if h.is_null() {
        return true; // lock failed — don't block the user
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let title: Vec<u16> = "OcyDash — Editor\0".encode_utf16().collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }
        unsafe { CloseHandle(h) };
        return false;
    }
    let _ = h; // intentionally never CloseHandle — mutex held until exit
    true
}

fn parse_port_arg() -> Option<String> {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--port" {
            return it.next();
        }
        if let Some(p) = a.strip_prefix("--port=") {
            return Some(p.to_string());
        }
    }
    None
}
