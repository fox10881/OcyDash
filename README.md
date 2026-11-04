<img width="3834" height="1986" alt="Screenshot 2026-11-04 213950" src="https://github.com/user-attachments/assets/7a84a6a8-d05f-4cb6-b0e4-9f41c62b9db4" />
<img width="1827" height="90" alt="Screenshot 2026-11-04 220021" src="https://github.com/user-attachments/assets/a28acaa7-1ba2-4b63-9b08-df1c6cb15f80" />


# OcyDash: The Ultimate Custom Driver for Ocypus AIO Coolers

A highly optimized, ultra-lightweight custom driver and UI dashboard built in
Rust for the **Ocypus Sigma PRO L24 240MM AIO Liquid CPU Cooler**
(3.5" LCD, 640×480).

![Rust](https://img.shields.io/badge/built%20with-Rust-orange)
![Platform](https://img.shields.io/badge/platform-Windows-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![RAM](https://img.shields.io/badge/RAM-~15%20MB-brightgreen)

---

## Why OcyDash? (The Performance Leap)

The stock monitoring app is an Electron wrapper that keeps a full Chromium
browser alive 24/7 just to push a JPEG to a 640×480 panel. OcyDash does the
same job in native Rust — measured side-by-side in Task Manager:

| | Official Software | **OcyDash (headless)** |
|---|---|---|
| RAM footprint | **~430+ MB** | **~15 MB** |
| CPU (idle/static theme) | constant background churn | **~0%** |
| Background processes | multiple helpers | **one** service |

That's a **~28× memory reduction** — and it stays that low forever, because
OcyDash is built as a **dual-mode architecture**:

- **Headless tray service** — the default mode. No GUI framework, no GPU
  context, no Chromium. It owns the serial port, streams frames to the LCD,
  and sips ~15 MB of RAM around the clock.
- **On-demand GUI editor** — a separate process spawned only when you click
  *Open Editor*. Edit your theme, close the window, and every byte of GUI
  memory is freed while streaming continues uninterrupted.

## Key Features

- **Zero-Bloat Background Streaming** — runs silently in the system tray;
  right-click for menu, left-click opens the editor.
- **Advanced GUI Editor** — drag-and-drop widgets on the real 640×480 canvas,
  8-way resize handles that scale the font, per-widget colors/alignment/
  decimals, and full theme customization.
- **Rich Hardware Monitoring** — direct HWiNFO integration (memory-mapped
  feed, no WMI polling). Choose exactly which sensor to track — CPU, GPU,
  RAM, Fans, Power — with per-widget manual dropdown overrides.
- **Dynamic Progress Bars** — custom-rendered, color-customizable progress
  pills for CPU/GPU/RAM loads (fill, track, border, rounded corners).
- **Self-Contained Themes** — saving a theme automatically copies its
  background media (Images/GIFs/Videos) into the theme folder and cleans up
  orphans — share a folder, get a complete theme.
- **Full Arabic Language Support** — proper RTL shaping, contextual letter
  forms and ligatures via `cosmic-text`. Type Arabic into any custom-text
  widget.
- **Run on Startup** — one checkbox registers the tray service with Windows.
- **Crash-safe children** — spawned helpers (ffmpeg, sensor feed) are bound
  to a kill-on-close job object, so nothing is ever left running.

## Installation & Usage

1. Download **`OcyDash-Setup-x.x.x.exe`** from the
   [Releases](https://github.com/fox10881/OcyDash/releases) tab.
2. Run the installer — it installs to `%ProgramFiles%\OcyDash` and creates
   Start Menu + Desktop shortcuts.
3. Launch **OcyDash** — the service starts headless: look for the **green
   disc** in your system tray.
4. Right-click the tray icon → **Open Editor** (or use the
   *OcyDash Editor* Start Menu shortcut) to design your theme — every save
   hot-reloads onto the panel instantly.
5. In the editor's top bar, tick **Run on startup** to auto-start the tray
   service at logon.

> **Requirements:** Windows 10/11 x64. The sensor feed uses the vendor's
> `SystemInfos.exe` helper for HWiNFO data — keep the official package's SDK
> components installed, or any `Ocypus*.bin` writer works.

## Build from Source

```sh
cargo run --example make-icon   # first time only — generates assets/ocydash.ico
cargo build --release           # → target\release\ocydash.exe
```

```text
ocydash.exe              headless tray service (default)
ocydash.exe --editor     layout editor
ocydash.exe --port COM5  override the serial port
```

The Windows icon + version info are embedded automatically by `build.rs`
(Windows SDK `rc.exe` auto-detected). Drop a `ffmpeg.exe` into
`vendor\ffmpeg\` before building the installer to bundle video/large-GIF
background support; `installer\ocydash.nsi` builds the setup exe with NSIS.

## How It Works

The panel speaks USB CDC-ACM @ 2,000,000 baud. Frames are wrapped in a
`55 AA | u16 len | key | payload | u16 cksum` packet; live mode (`0x11`)
carries `[u32 len | JPEG | u16 sum]` at ~80 KB/frame. OcyDash composites
your theme → rotates → JPEG-encodes → streams the latest frame to COM3 while
the sensor feed updates in the background. Full protocol notes in
[`AGENTS.md`](AGENTS.md).

## License

MIT — see [`LICENSE`](LICENSE). Community project, not affiliated with the
hardware vendor.
