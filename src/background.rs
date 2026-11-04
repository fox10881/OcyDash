//! Background engine — every source is normalized to a W x H RGBA stream.
//!
//! - Color: solid fill (zero cost).
//! - Image: decoded once via `image`, cover-cropped to canvas, cached.
//! - GIF: all frames predecoded at load (delay honoured), memory-capped.
//! - Video: piped through the ffmpeg.exe bundled with the official app —
//!   `ffmpeg -i X -vf scale/crop -f rawvideo -pix_fmt rgba -` into a
//!   bounded ring buffer (≈8 frames) on a reader thread.

use crate::theme::Background;
use image::AnimationDecoder;
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// ffmpeg for video / large-GIF streaming. Resolution order:
/// `<exe>\ffmpeg.exe` (installed layout) → `vendor\ffmpeg\ffmpeg.exe`
/// relative to any ancestor (dev workspace) → the vendor app's copy.
fn ffmpeg_path() -> PathBuf {
    crate::deps::resolve(&["ffmpeg.exe", r"vendor\ffmpeg\ffmpeg.exe"])
        .unwrap_or_else(|| {
            PathBuf::from(
                r"C:\Program Files\Ocypus Display\resources\app.asar.unpacked\node_modules\@ffmpeg-installer\win32-x64\ffmpeg.exe",
            )
        })
}
/// cap video ring buffer
const VIDEO_RING: usize = 8;
/// cap total decoded GIF bytes (~64MB) — only for small predecoded GIFs
const GIF_MAX_BYTES: usize = 64 * 1024 * 1024;
/// GIFs larger than this stream through ffmpeg instead of predecoding
const GIF_STREAM_BYTES: u64 = 3 * 1024 * 1024;

pub enum Bg {
    Color([u8; 3]),
    Static { rgba: Vec<u8> },
    Animated { frames: Vec<(Vec<u8>, u32)>, idx: usize, last: Instant },
    Video {
        ring: Arc<Mutex<VecDeque<Arc<Vec<u8>>>>>,
        /// recycled frame buffers — consumed/evicted frames return here
        /// so the reader doesn't allocate 1.2MB per decoded frame
        pool: Arc<Mutex<Vec<Vec<u8>>>>,
        stop: Arc<AtomicBool>,
        child: Option<Child>,
    },
}

pub struct BgEngine {
    pub w: usize,
    pub h: usize,
    pub src: Option<Bg>,
    pub err: Option<String>,
}

impl BgEngine {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, src: None, err: None }
    }

    /// Animated sources (predecoded GIF / ffmpeg video) need a render
    /// each tick; Color/Static let the caller skip unchanged frames.
    pub fn animated(&self) -> bool {
        matches!(self.src, Some(Bg::Animated { .. } | Bg::Video { .. }))
    }

    pub fn set(&mut self, cfg: &Background) {
        self.src = None;
        self.err = None;
        self.src = Some(match cfg {
            Background::Color { color } => Bg::Color(*color),
            Background::Image { path } => {
                // Large GIFs stream through the ffmpeg ring buffer like video
                // (~8 frames ≈ 10MB) instead of predecoding every frame into
                // RAM (up to 64MB). Small GIFs keep exact-delay predecode.
                let is_gif = PathBuf::from(path)
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("gif"))
                    .unwrap_or(false);
                let big = std::fs::metadata(path)
                    .map(|m| m.len() > GIF_STREAM_BYTES)
                    .unwrap_or(false);
                if is_gif && big {
                    self.start_video(path, 15)
                } else {
                    match load_image(path, self.w, self.h) {
                        Ok(frames) if frames.len() == 1 => Bg::Static { rgba: frames.into_iter().next().unwrap().0 },
                        Ok(frames) => Bg::Animated { frames, idx: 0, last: Instant::now() },
                        Err(e) => {
                            self.err = Some(e);
                            Bg::Color([10, 10, 16])
                        }
                    }
                }
            },
            Background::Video { path, fps } => self.start_video(path, *fps),
        });
    }

    fn start_video(&mut self, path: &str, fps: u8) -> Bg {
        let ring: Arc<Mutex<VecDeque<Arc<Vec<u8>>>>> = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let ffmpeg = ffmpeg_path();
        if !ffmpeg.exists() {
            self.err = Some("ffmpeg.exe not found".into());
            return Bg::Color([10, 10, 16]);
        }
        let fps = fps.clamp(1, 30);
        let (w, h) = (self.w, self.h);
        let vf = format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps={fps}"
        );
        let child = Command::new(ffmpeg)
            .args([
                "-loglevel", "error",
                "-stream_loop", "-1",
                "-i", path,
                "-vf", &vf,
                "-f", "rawvideo",
                "-pix_fmt", "rgba",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            // ffmpeg is console-subsystem — suppress its window from a
            // windowless (windows_subsystem) parent
            .creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
            )
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                self.err = Some(format!("ffmpeg spawn: {e}"));
                return Bg::Color([10, 10, 16]);
            }
        };
        // dies with us even on taskkill /F (job object, kill-on-close)
        crate::job::protect(child.id());
        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => return Bg::Color([10, 10, 16]),
        };
        let (w, h) = (self.w, self.h);
        let ring2 = ring.clone();
        let stop2 = stop.clone();
        let pool: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let pool2 = pool.clone();
        std::thread::spawn(move || {
            let frame_bytes = w * h * 4;
            loop {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                // reuse an evicted/consumed buffer instead of allocating
                let mut buf = pool2
                    .lock()
                    .unwrap()
                    .pop()
                    .unwrap_or_else(|| vec![0u8; frame_bytes]);
                match stdout.read_exact(&mut buf) {
                    Ok(()) => {
                        let mut q = ring2.lock().unwrap();
                        if q.len() >= VIDEO_RING {
                            // evicted frame → back to the pool for reuse
                            if let Ok(old) = Arc::try_unwrap(q.pop_front().unwrap()) {
                                pool2.lock().unwrap().push(old);
                            }
                        }
                        q.push_back(Arc::new(buf));
                    }
                    Err(_) => {
                        // ffmpeg ended (short clip w/ -stream_loop should loop;
                        // on error just stop pushing)
                        pool2.lock().unwrap().push(buf);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        });
        Bg::Video { ring, pool, stop, child: Some(child) }
    }

    /// Current base canvas: fills `out` (w*h*4). Cheap — copy or fill.
    pub fn blit(&mut self, out: &mut Vec<u8>) {
        match &mut self.src {
            Some(Bg::Color(c)) => {
                for px in out.chunks_exact_mut(4) {
                    px[0] = c[0];
                    px[1] = c[1];
                    px[2] = c[2];
                    px[3] = 255;
                }
            }
            Some(Bg::Static { rgba }) => {
                if rgba.len() == out.len() {
                    out.copy_from_slice(rgba);
                }
            }
            Some(Bg::Animated { frames, idx, last }) => {
                if frames.is_empty() {
                    return;
                }
                let (_, delay) = frames[*idx];
                if last.elapsed().as_millis() as u32 >= delay.max(30) {
                    *idx = (*idx + 1) % frames.len();
                    *last = Instant::now();
                }
                let (rgba, _) = &frames[*idx];
                if rgba.len() == out.len() {
                    out.copy_from_slice(rgba);
                }
            }
            Some(Bg::Video { ring, pool, .. }) => {
                let mut q = ring.lock().unwrap();
                if let Some(f) = q.pop_front() {
                    if f.len() == out.len() {
                        out.copy_from_slice(&f);
                    }
                    // consumer holds the only ref → recycle the buffer
                    drop(q);
                    if let Ok(buf) = Arc::try_unwrap(f) {
                        pool.lock().unwrap().push(buf);
                    }
                }
            }
            None => {}
        }
    }
}

impl Drop for BgEngine {
    fn drop(&mut self) {
        if let Some(Bg::Video { stop, child, .. }) = &mut self.src {
            stop.store(true, Ordering::Relaxed);
            if let Some(c) = child {
                let _ = c.kill();
            }
        }
    }
}

/// Decode image/GIF → cover-cropped RGBA frames at (w,h).
/// GIF returns all frames with delays; others return a single frame.
fn load_image(path: &str, w: usize, h: usize) -> Result<Vec<(Vec<u8>, u32)>, String> {
    let ext = PathBuf::from(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "gif" {
        return load_gif(path, w, h);
    }
    let img = image::open(path).map_err(|e| format!("decode {path}: {e}"))?;
    let img = cover_resize(&img, w, h);
    Ok(vec![(img.into_rgba8().into_raw(), 0)])
}

fn load_gif(path: &str, w: usize, h: usize) -> Result<Vec<(Vec<u8>, u32)>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let dec = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("gif decode: {e}"))?;
    let frames = dec.into_frames().collect_frames().map_err(|e| format!("gif frames: {e}"))?;
    if frames.is_empty() {
        return Err("gif: no frames".into());
    }
    let mut out = Vec::new();
    let mut total = 0usize;
    // composite logical-screen frames so transparency accumulates correctly
    let mut canvas = vec![0u8; w * h * 4];
    for f in frames {
        let delay = f.delay().numer_denom_ms();
        let delay_ms = delay.0 / delay.1.max(1);
        let img = image::DynamicImage::ImageRgba8(f.into_buffer());
        let img = cover_resize(&img, w, h).into_rgba8();
        // alpha-composite over canvas (gif frames may be partial)
        for (dst, src) in canvas.chunks_exact_mut(4).zip(img.as_raw().chunks_exact(4)) {
            let a = src[3] as u32;
            if a == 255 {
                dst.copy_from_slice(src);
            } else if a > 0 {
                let inv = 255 - a;
                dst[0] = ((src[0] as u32 * a + dst[0] as u32 * inv) / 255) as u8;
                dst[1] = ((src[1] as u32 * a + dst[1] as u32 * inv) / 255) as u8;
                dst[2] = ((src[2] as u32 * a + dst[2] as u32 * inv) / 255) as u8;
                dst[3] = 255;
            }
        }
        total += canvas.len();
        if total > GIF_MAX_BYTES && !out.is_empty() {
            break; // drop extra frames rather than balloon RAM
        }
        out.push((canvas.clone(), delay_ms));
    }
    if out.is_empty() {
        Err("gif: empty".into())
    } else {
        Ok(out)
    }
}

/// scale-to-cover + center crop to exactly w×h.
fn cover_resize(img: &image::DynamicImage, w: usize, h: usize) -> image::DynamicImage {
    let (iw, ih) = (img.width(), img.height());
    let scale = (w as f32 / iw as f32).max(h as f32 / ih as f32);
    let (sw, sh) = ((iw as f32 * scale).round() as u32, (ih as f32 * scale).round() as u32);
    let img = img.resize(sw, sh, image::imageops::FilterType::Triangle);
    let x = sw.saturating_sub(w as u32) / 2;
    let y = sh.saturating_sub(h as u32) / 2;
    img.crop_imm(x, y, w as u32, h as u32)
}
