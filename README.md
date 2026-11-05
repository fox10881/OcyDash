<img width="3837" height="1977" alt="image" src="https://github.com/user-attachments/assets/881e0919-9427-444d-88c2-436f5b22a1d5" />

<img width="1827" height="81" alt="Screenshot 2026-11-05 052022" src="https://github.com/user-attachments/assets/702adcdf-e088-45c8-93f4-c18297b31ecb" />



# OcyDash: The Ultimate Custom Driver for Ocypus AIO Coolers

A highly optimized, ultra-lightweight custom driver and UI dashboard built in
Rust for the **Ocypus Sigma PRO L24 240MM AIO Liquid CPU Cooler**
(3.5" LCD, 640×480).

![Rust](https://img.shields.io/badge/built%20with-Rust-orange)
![Platform](https://img.shields.io/badge/platform-Windows-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![RAM](https://img.shields.io/badge/RAM-~136%20MB-brightgreen)
![CPU](https://img.shields.io/badge/CPU-0%25-brightgreen)

---

## Why OcyDash? (The Performance Leap)

The stock monitoring app is an Electron wrapper that keeps a full Chromium
browser alive 24/7 just to push a JPEG to a 640×480 panel — plus a sensor
helper that can silently deadlock for good. OcyDash does the same job in
native Rust — measured side-by-side in Task Manager:

| | Official Software | **OcyDash (headless)** |
|---|---|---|
| RAM footprint | **~430+ MB** | **~136 MB** |
| CPU (idle) | constant background churn | **absolute 0%** |
| Animated backgrounds | + a ~98%-of-a-core ffmpeg child | **0% — predecoded, no ffmpeg at all** |
| Background processes | multiple helpers | **one** service |

OcyDash deliberately trades RAM for CPU: a **layer-caching + GIF
pre-decoding pipeline** keeps everything expensive resident in memory so the
steady-state loop is almost pure `memcpy` — the static widget layer is
composited once, media frames are decoded once at load, JPEG buffers are
reused, and the render gate only repaints when pixels would actually change.
That stable ~136 MB buys you **0.00% CPU on the Task Manager counter** — a
>3× footprint win over the vendor bloatware *and* an idle processor.

It stays honest because OcyDash is built as a **dual-mode architecture**:

- **Headless tray service** — the default mode. No GUI framework, no GPU
  context, no Chromium. It owns the serial port, streams frames to the LCD,
  and keeps that flat ~136 MB / ~0% profile around the clock.
- **On-demand GUI editor** — a separate process spawned only when you click
  *Open Editor*. Edit your theme, close the window, and every byte of GUI
  memory is freed while streaming continues uninterrupted.

## Key Features

- **Zero-Bloat Background Streaming** — runs silently in the system tray;
  right-click for menu, left-click opens the editor.
- **Advanced GUI Editor** — drag-and-drop widgets on the real 640×480 canvas,
  8-way resize handles that scale the font, per-widget colors/alignment/
  decimals, and full theme customization.
- **Rich Hardware Monitoring** — native HWiNFO telemetry via direct FFI
  into `HWiNFO64.dll` (no WMI polling, no flaky .NET helper). Choose
  exactly which sensor to track — CPU, GPU, RAM, Fans, Power — with
  per-widget manual dropdown overrides.
- **Dynamic Progress Bars** — custom-rendered progress pills for
  CPU/GPU/RAM loads with **4-tier conditional fill colors**
  (`<25%` / `<50%` / `<75%` / `≥75%`), plus track, border and rounded-corner
  customization.
- **Self-Contained Themes** — saving a theme automatically copies its
  background media (Images/GIFs/Videos) into the theme folder and cleans up
  orphans — share a folder, get a complete theme.
- **Full Arabic Language Support** — proper RTL shaping, contextual letter
  forms and ligatures via `cosmic-text`. Type Arabic into any custom-text
  widget.
- **Run on Startup** — one checkbox registers a highest-privilege Task
  Scheduler logon task, so the elevated tray service starts silently with
  Windows — no UAC prompt at boot.
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

> **Requirements:** Windows 10/11 x64, **administrator rights** — OcyDash
> embeds a `requireAdministrator` manifest because the HWiNFO sensor driver
> is kernel-level (one consent at launch; the startup task bypasses it at
> logon). Everything needed ships in the installer — the bundled
> `HWiNFO64.dll` feeds sensors directly via native FFI, and the vendor's
> `SystemInfos.exe` helper remains only as a dormant fallback.

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
