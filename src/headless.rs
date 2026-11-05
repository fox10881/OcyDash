//! Headless mode — the default entry point.
//!
//! No eframe/winit/GPU context is ever created here: a system-tray icon,
//! the sensor feed, the render loop and the serial engine are all that
//! runs (~23MB RSS). The eframe editor is spawned on demand as a *separate
//! process* (`--editor`), so closing its window frees every GUI byte while
//! streaming continues. Theme JSON is the IPC channel: the editor saves,
//! this loop hot-reloads (~700ms poll) and the new layout hits the panel
//! without a restart.

use crate::background::BgEngine;
use crate::display;
use crate::render::{Renderer, rotate_rgba_into};
use crate::sensors;
use crate::theme::{Theme, last_theme_path, themes_dir};
use chrono::Timelike;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// Runs forever; returns a process exit code when the user picks "Exit".
pub fn run_headless(port: Option<String>) -> i32 {
    if !single_instance() {
        eprintln!("[headless] another instance is already running — exiting");
        return 1;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let feed = sensors::start_feed(stop.clone());
    let engine = display::start(port, stop.clone());

    let mut theme = last_theme_path()
        .and_then(|p| Theme::load(&p))
        .unwrap_or_default();
    let (cw, ch) = theme.canvas_size();
    let mut bg = BgEngine::new(cw, ch);
    bg.set(&theme.background);
    let mut renderer = Renderer::new(cw, ch);

    // the TrayIcon must outlive the loop — dropping it removes the icon
    let (_tray, ids) = match build_tray() {
        Ok((tray, open_id, exit_id)) => {
            eprintln!("[tray] icon registered — right-click for menu, left-click opens editor");
            (Some(tray), Some((open_id, exit_id)))
        }
        Err(e) => {
            eprintln!("[tray] {e}");
            (None, None)
        }
    };

    eprintln!("[headless] streaming — RAM stays low; open the editor from the tray");

    let mut editor: Option<Child> = None;
    let mut watch = ThemeWatch::new();
    // cache the sample — clone only when the feed publishes a new one
    let mut last_feed_gen = 0u64;
    let mut sample = feed.latest();
    let mut theme_dirty = true;
    // RAM-for-CPU steady state: the static base layer (bg + fixed widgets)
    // is rendered once into renderer.base_px; each frame then costs a
    // memcpy + the few dynamic strings — and when the dynamic signature
    // is byte-identical we skip draw/rotate/encode entirely (~0% CPU).
    let mut base_dirty = true;
    let mut last_sig = 0u64;
    let mut rot_buf: Vec<u8> = Vec::new();
    let mut jpeg_buf: Vec<u8> = Vec::with_capacity(crate::protocol::JPEG_MAX_KB * 1024);

    'outer: loop {
        // tray-icon's win32 backend delivers events through the creating
        // thread's message queue — pump it every iteration
        if pump_messages() {
            break 'outer; // WM_QUIT
        }

        if let Some((open_id, exit_id)) = &ids {
            for ev in MenuEvent::receiver().try_iter() {
                if ev.id == *open_id {
                    spawn_editor(&mut editor);
                } else if ev.id == *exit_id {
                    break 'outer;
                }
            }
        }
        for ev in TrayIconEvent::receiver().try_iter() {
            match ev {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => spawn_editor(&mut editor),
                _ => {}
            }
        }

        // hot-reload: the editor process saves the theme file, we pick it up
        if let Some(p) = watch.poll()
            && let Some(t) = Theme::load(&p)
        {
            let (cw, ch) = t.canvas_size();
            let resized = (cw, ch) != (renderer.w, renderer.h);
            let bg_changed = serde_json::to_string(&t.background).ok()
                != serde_json::to_string(&theme.background).ok();
            if resized {
                renderer.resize(cw, ch);
                bg = BgEngine::new(cw, ch);
            }
            if resized || bg_changed {
                bg.set(&t.background);
            }
            eprintln!("[headless] theme reloaded: {}", t.name);
            theme = t;
            theme_dirty = true;
        }

        // refresh the cached sample only when the feed publishes a new one
        let fgen = feed.generation();
        if fgen != last_feed_gen {
            last_feed_gen = fgen;
            sample = feed.latest(); // Arc lists → clone is refcount bumps
        }

        // Dynamic signature — hash of every dynamic widget's text, bar
        // fraction and box. If byte-identical to the last rendered frame,
        // the pixels CANNOT have changed (bg + statics are cached in the
        // base layer) → skip draw + rotate + JPEG encode entirely.
        let now = now_secs();
        let sig = {
            use std::hash::{Hash, Hasher};
            let mut sig = std::collections::hash_map::DefaultHasher::new();
            for w in &theme.widgets {
                if !w.visible || w.is_static() {
                    continue;
                }
                (
                    w.x.to_bits(),
                    w.y.to_bits(),
                    w.w.to_bits(),
                    w.h.to_bits(),
                    w.bar,
                )
                    .hash(&mut sig);
                w.text(&sample, now).hash(&mut sig);
                if let Some(f) = w.bar_percent(&sample) {
                    f.to_bits().hash(&mut sig);
                }
            }
            theme.rotation.hash(&mut sig);
            sig.finish()
        };

        // a NEW bg frame is due (GIF delay elapsed / video frame queued) —
        // checked BEFORE any blit so the draw sees the advance
        let bg_adv = bg.advance_pending();

        if theme_dirty || base_dirty || bg_adv || sig != last_sig {
            // layered draw for EVERY bg kind: rebuild_base composites the
            // fresh bg frame + static widgets once; dynamic layer always
            // draws on top. Animated bg frames only arrive at their own
            // cadence, so between frames this is a memcpy + few strings.
            let grows = renderer.draw_layered(
                &theme,
                &mut bg,
                &sample,
                now,
                theme_dirty || base_dirty || bg_adv,
            );
            for (id, bw, bh) in grows {
                if let Some(w) = theme.widgets.iter_mut().find(|x| x.id == id) {
                    if bw > w.w {
                        w.w = bw;
                    }
                    if bh > w.h {
                        w.h = bh;
                    }
                }
            }
            // rotate into a reused buffer; rotation==0 encodes px directly
            let (ew, eh, rbuf): (usize, usize, &[u8]) = if theme.rotation == 0 {
                (renderer.w, renderer.h, &renderer.px)
            } else {
                let (w2, h2) = rotate_rgba_into(
                    &renderer.px,
                    renderer.w,
                    renderer.h,
                    theme.rotation,
                    &mut rot_buf,
                );
                (w2, h2, &rot_buf)
            };
            if renderer.encode_jpeg(theme.jpeg_quality, rbuf, ew, eh, &mut jpeg_buf) {
                engine.push_frame(jpeg_buf.clone());
            }
            base_dirty = false;
            theme_dirty = false;
            last_sig = sig;
        }

        std::thread::sleep(tick_for(&bg));
    }

    // graceful: engine writes the clear sequence then closes the port
    stop.store(true, Ordering::Relaxed);
    engine.shutdown();
    std::thread::sleep(Duration::from_millis(250));
    eprintln!("[headless] exit");
    0
}

fn now_secs() -> f64 {
    let t = chrono::Local::now();
    t.num_seconds_from_midnight() as f64 + f64::from(t.timestamp_subsec_millis()) / 1000.0
}

/// Render cadence — the loop only needs to wake often enough to catch
/// bg-frame advances; advance_pending + the dynamic signature decide
/// whether any work actually happens. Static themes tick at 2Hz (sensor
/// samples land ~1/s anyway → ~0% CPU idle).
fn tick_for(bg: &crate::background::BgEngine) -> Duration {
    match &bg.src {
        // animated bgs capped at ~4fps — the serial wire (2Mbaud) can't
        // push an 80KB JPEG faster than ~2.5fps anyway, so anything above
        // ~4fps renders frames the panel can never display
        Some(crate::background::Bg::Video { .. }) => Duration::from_millis(300),
        Some(crate::background::Bg::Animated { .. }) => Duration::from_millis(300),
        _ => Duration::from_millis(500),
    }
}

/// Spawn the eframe editor as a child process; dedupes a running one.
fn spawn_editor(child: &mut Option<Child>) {
    if let Some(c) = child.as_mut() {
        match c.try_wait() {
            Ok(None) => {
                eprintln!("[tray] editor already running");
                return;
            }
            _ => *child = None,
        }
    }
    match std::env::current_exe() {
        Ok(exe) => match Command::new(exe).arg("--editor").spawn() {
            Ok(c) => eprintln!("[tray] editor spawned (pid {})", c.id()),
            Err(e) => eprintln!("[tray] editor spawn failed: {e}"),
        },
        Err(e) => eprintln!("[tray] current_exe failed: {e}"),
    }
}

// ---------- tray ----------

/// Returns the TrayIcon (must be held) + (open_editor_id, exit_id).
fn build_tray() -> Result<
    (
        tray_icon::TrayIcon,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
    ),
    String,
> {
    let menu = Menu::new();
    let open = MenuItem::new("Open Editor", true, None);
    let exit = MenuItem::new("Exit", true, None);
    let open_id = open.id().clone();
    let exit_id = exit.id().clone();
    menu.append(&open).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| e.to_string())?;
    menu.append(&exit).map_err(|e| e.to_string())?;

    let icon = Icon::from_rgba(crate::icon::disc_rgba(32), 32, 32).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_id("ocydash")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("OcyDash — streaming to LCD")
        .with_icon(icon)
        .build()
        .map_err(|e| e.to_string())?;
    Ok((tray, open_id, exit_id))
}

// ---------- theme file watcher ----------

/// Polls `_last.json` (theme switch) + the theme file's mtime (edits).
struct ThemeWatch {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    next_check: Instant,
}

impl ThemeWatch {
    fn new() -> Self {
        let mut w = Self {
            path: None,
            mtime: None,
            next_check: Instant::now(),
        };
        let _ = w.poll(); // prime baseline — the theme was just loaded
        w
    }

    /// Some(path) when the active theme file changed on disk.
    fn poll(&mut self) -> Option<PathBuf> {
        if self.next_check > Instant::now() {
            return None;
        }
        self.next_check = Instant::now() + Duration::from_millis(700);
        let p = last_theme_path().or_else(|| {
            let d = themes_dir().join("Default.json");
            d.exists().then_some(d)
        });
        if p != self.path {
            self.path = p;
            self.mtime = None;
        }
        let p = self.path.clone()?;
        let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        if m != self.mtime {
            self.mtime = m;
            return Some(p);
        }
        None
    }
}

// ---------- platform ----------

/// Pump this thread's win32 message queue (tray backend requires it).
/// Returns true on WM_QUIT.
#[cfg(windows)]
fn pump_messages() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                return true;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    false
}

#[cfg(not(windows))]
fn pump_messages() -> bool {
    false
}

/// One headless instance owns COM3 — named mutex guards the port.
/// Global\ namespace: non-admin processes lack SeCreateGlobalPrivilege
/// and cannot squat the name to block startup.
#[cfg(windows)]
fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Global\\OcyDashHeadless\0".encode_utf16().collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if h.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return false;
    }
    // intentionally leaked — held for the process lifetime
    true
}

#[cfg(not(windows))]
fn single_instance() -> bool {
    true
}
