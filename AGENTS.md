# Ocypus Sigma PRO L24 — reverse-engineering notes & build guide

## Device identity

| Field | Value |
|---|---|
| VID / PID | `0x33C3 / 0x7789` (USB CDC-ACM serial, shows as COM3) |
| Panel | `TXW818-NV3052C-3.5inch-hor`, 640x480, fw 2.9 |
| Link | serial @ **2,000,000 baud**, 8N1, DTR=1 |
| Brand config | `appConstant.js -> Ocypus_Display`: vid `33C3`, pid `7789`, render `highPerformance`, brightnessRatio 0.8 |

## Wire protocol (verified live via `tools/probe.py` / `tools/send_frame.py`)

Command frame (host→device):
```
55 AA | len_lo len_hi | key | payload... | sum_lo sum_hi
len  = payload_len + 7        (little-endian)
sum  = sum(all bytes) & 0xFFFF (little-endian)
```
Response: same layout; byte 4 echoes the key; payload = UTF-8 JSON
(`{"cmd":"info","data":{...}}`) or 1-byte status (0x00 = ok, 01-07 = errors).

Keys seen in `main/_baseClass/device.js`:
- `0x01` restart · `0x03` setLight · `0x06` getDeviceInfo · `0x0C` OTA
- `0x11` enter live/show mode (also 1.5s keepalive)
- `0x14` motionBeforeOffScreen · `0x15` motionTimeout · `0x20` setRegion
- `0x21` close · `0x23` setSerialNum · `0x25` motor · `0x26` rtPlayTimeout

Live stream (fw > 2.8, non-SPI models):
```
send cmd 0x11  → then per frame:
[u32 LE jpeg_len][jpeg bytes][u16 LE sum(size+jpeg)]
jpeg capped at ~80KB (quality auto-reduced). raw 20KB chunks are only for
file uploads/OTA. Clear = raw FF D9 FF D9.
```
SPI models (`model` contains `2.99`, `TXW813-ST7789-2.8inch`, `qspi`) stream
raw big-endian RGB565 instead — L24 is NOT SPI (model = TXW818-NV3052C).

## Sensor feed

Official app spawns `resources/main/SDK/VC#/SystemInfos/.../SystemInfos.exe
<name> <diskScanInterval> <disableDisk>`; helper writes
`%TEMP%\<name>.bin` = `u32 LE len + UTF-8 JSON` via **memory-mapped file**
(content updates live, mtime frozen — never use mtime for freshness).
`cpuLayout.temperature_list[]` → `CPU (Tctl/Tdie)`, `CPU Die (average)`,
`CPU CCD1 (Tdie)`; `systemTime` field advances ≈ every 1-2s → aliveness test.
`SystemInfos.exe` requires elevation (manifest) and is driver-protected
(can't be killed non-admin). `ocydash.exe` itself embeds
`requireAdministrator` (`assets/ocydash.manifest` → `1 24` in
`assets/ocydash.rc`), so the direct `Command::new` spawn inherits the
admin token with no UAC; the powershell `Start-Process` fallback remains
for non-elevated edge cases.

Heartbeat recovery (`SensorFeed.recover`): feed static >5s (`STALE_AFTER`)
→ `ensure()` rescans all `Ocypus*.bin` writers across temp_dir +
LOCALAPPDATA\Temp + C:\Windows\Temp → nothing alive → `taskkill /F /IM
SystemInfos.exe` (clean slate — any survivor is a zombie) → respawn.
`RESPAWN_BACKOFF` (12s) caps respawn frequency and doubles as helper
warm-up grace. Respawns are silent now that we run elevated.

## Build / run

```sh
cargo run --example make-icon      # first time: generates assets/ocydash.ico
cargo build --release
target\release\ocydash.exe            # headless tray service (default)
target\release\ocydash.exe --editor   # eframe layout editor
target\release\ocydash.exe --port COM3
```

## Tray / headless architecture

Two modes of one binary — winit can only create one EventLoop per process,
so the editor is a *separate process* spawned by the tray menu; closing it
frees all GUI memory while the service keeps streaming:

```
ocydash.exe (headless, ~27MB RSS)
  main thread: tray (tray-icon crate) + win32 PeekMessage pump +
               render tick (~15fps / video fps) + theme hot-reload (700ms)
  engine thread: COM3 serial (handshake, latest-frame slot, keepalive)
  sensor thread: %TEMP%\Ocypus*.bin → SysSample
ocydash.exe --editor (~95MB RSS while open)
  eframe window; engine=None — edits + saves theme JSON only;
  headless picks it up via mtime watch (theme file = IPC channel)
```

- `src/headless.rs`  tray icon (Open Editor/Exit), msg pump, render loop,
  theme hot-reload watcher, single-instance mutex (Local\OcyDashHeadless)
- `src/main.rs`      mode dispatch (--editor → eframe, else headless)
- `src/app.rs`       editor UI (Option<EngineHandle> — None in editor mode),
  top bar: themes/save/new/open-folder/rotate + right-aligned
  "Run on startup" checkbox (cached task state, not polled)
- `src/theme.rs`     Theme/Widget/Background model + JSON save/load +
  last_theme_path()/write_last_theme() in %APPDATA%\ocydash\themes\
  (one-time migrate of the old %APPDATA%\ocypus-display dir).
  Theme::save() localizes background media: copies the file into the
  themes dir as <theme>.<ext>, rewrites the stored path, and deletes
  stale <theme>.* media (type switch / background removed).
- `src/sensors.rs`   SysSample also carries per-category
  Vec<(name,value)> lists (cpu/gpu temps, loads, powers, fans, ram).
  Widget.sensor_idx: 0 = auto heuristic, n>0 = list entry n-1 —
  resolved via SysSample::override_value(); the Inspector shows a
  ComboBox for any WidgetKind::sensor_cat() != None.
- `src/startup.rs`   Task Scheduler logon task `OcyDash` (schtasks /create
  /sc onlogon /rl highest) — the ONLY silent autostart for an elevated app;
  Windows ignores HKCU Run entries that would trigger UAC. Legacy Run
  value is cleaned up on toggle; uninstaller deletes the task.
- `src/icon.rs`      shared green-disc artwork (tray, window icon, .ico)
- `build.rs`         embed-resource → assets/ocydash.rc embeds the app icon,
  VERSIONINFO + requireAdministrator manifest (assets/ocydash.manifest);
  auto-finds Windows SDK rc.exe if off PATH
- `installer/ocydash.nsi`  NSIS → dist/OcyDash-Setup-<ver>.exe; installs to
  %ProgramFiles%\OcyDash, Start Menu (OcyDash + OcyDash Editor + Uninstall)
  + Desktop shortcuts, Add/Remove entry, removes the Run key on uninstall
  only if it points at this install; bundles vendor/ffmpeg/ffmpeg.exe
  when present
- `src/render.rs`    cosmic-text shaping (Arabic/RTL/ligatures, curated
  fontdb::Source::File list — lazy, ~0MB until used) → RGBA →
  rotate_rgba() → JPEG (auto-q under 80KB)
- `src/background.rs` Color / image / GIF (≤3MB predecoded; larger GIFs
  stream via ffmpeg like video) / video (ffmpeg pipe → 8-frame ring)
- `src/display.rs`   serial engine (unchanged)
- `src/sensors.rs`   sensor feed (unchanged)
- `src/protocol.rs`  wire codec (unchanged)

## egui 0.37 notes

- `App::logic(ctx, frame)` = non-painting tick; `App::ui(ui, frame)` = paint.
- Panels: `egui::Panel::top/left/right/bottom(id).frame(...).show(ui, ..)`;
  `Button::selectable`, `Label::halign/truncate`, `Panel::exact_size`.
- Left sidebar is LOCKED (`exact_size(250).resizable(false)`): never size
  children from `available_width()` inside a resizable panel — it feedback-
  grows the panel and squeezes the canvas away.
- Preview is always unrotated; `theme.rotation` only applies to bytes
  going to the device.

Measured: headless ≈27MB RSS / ~0% CPU (was ~23MB CLI, ~86-158MB GUI).
Editor ≈95MB while open, freed entirely on close.

Data flow (headless): sensor sample + theme → renderer.draw → rotate →
JPEG q≤80KB → `EngineHandle.push_frame` (latest-wins slot) → engine writes
`[u32 len][jpeg][u16 sum]` to COM3.

## USBPcap status

Wireshark 4.6.9 + USBPcap 1.5.4 installed, but USBPcap's filter is not
attached to root hubs (needs admin `pnputil` or reboot). Protocol was
validated directly at the serial layer instead (identical payload bytes).
