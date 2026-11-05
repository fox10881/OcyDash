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
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
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
/// cap total decoded GIF bytes — frames decimate to fit (~66 frames)
const GIF_MAX_BYTES: usize = 80 * 1024 * 1024;
/// GIF file-size cap — theme paths are same-user writable; a multi-GB
/// file would be slurped whole before decode even starts
const GIF_MAX_FILE: u64 = 256 * 1024 * 1024;
/// cumulative RAW decoded-frame bytes cap — a small file can still
/// decompress to GBs of RGBA (GIF is a codec bomb by nature)
const GIF_MAX_DECODE: usize = 1024 * 1024 * 1024;

pub enum Bg {
    Color([u8; 3]),
    Static {
        rgba: Vec<u8>,
    },
    Animated {
        frames: Vec<(Vec<u8>, u32)>,
        idx: usize,
        last: Instant,
    },
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
        Self {
            w,
            h,
            src: None,
            err: None,
        }
    }

    /// Would the next `blit` produce NEW pixels? Animated → next frame's
    /// delay has elapsed; Video → a decoded frame is queued; static
    /// sources → never. Lets the render loop skip everything when the
    /// bg frame itself hasn't advanced.
    pub fn advance_pending(&self) -> bool {
        match &self.src {
            Some(Bg::Animated { frames, idx, last }) => {
                !frames.is_empty() && last.elapsed().as_millis() as u32 >= frames[*idx].1.max(30)
            }
            Some(Bg::Video { ring, .. }) => !ring.lock().unwrap().is_empty(),
            _ => false,
        }
    }

    pub fn set(&mut self, cfg: &Background) {
        self.src = None;
        self.err = None;
        self.src = Some(match cfg {
            Background::Color { color } => Bg::Color(*color),
            Background::Image { path } => {
                // GIFs ALWAYS predecode into RAM — even huge ones. The
                // panel's serial wire caps display at ~2.5fps anyway, so
                // load_gif decimates to fit the byte cap (delays summed →
                // playback speed preserved). This trades ~60MB RAM for a
                // permanently idle ffmpeg (0% decode cpu) + memcpy blits.
                match load_image(path, self.w, self.h) {
                    Ok(frames) if frames.len() == 1 => Bg::Static {
                        rgba: frames.into_iter().next().unwrap().0,
                    },
                    Ok(frames) => Bg::Animated {
                        frames,
                        idx: 0,
                        last: Instant::now(),
                    },
                    Err(e) => {
                        self.err = Some(e);
                        Bg::Color([10, 10, 16])
                    }
                }
            }
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
        let vf =
            format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps={fps}");
        // theme JSON is same-user writable — `path` is untrusted input.
        // `-protocol_whitelist file` blocks every non-file demuxer the
        // string could smuggle in (lavfi: filtergraph exec, pipe:/concat:
        // for arbitrary file reads, http:/rtsp: for outbound connects)
        // — sanitize() additionally pins paths to `X:\` local form.
        let child = Command::new(ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-protocol_whitelist",
                "file",
                "-stream_loop",
                "-1",
                "-i",
                path,
                "-vf",
                &vf,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            // ffmpeg is console-subsystem — suppress its window from a
            // windowless (windows_subsystem) parent
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
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
        // pace consumption at the output fps — ffmpeg blocks on the pipe
        // once we stop draining, so its decode CPU collapses to ~idle
        let frame_dur = Duration::from_millis(1000 / fps as u64);
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
                        drop(q);
                        std::thread::sleep(frame_dur);
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
        Bg::Video {
            ring,
            pool,
            stop,
            child: Some(child),
        }
    }

    /// Current base canvas: fills `out` (w*h*4). Cheap — copy or fill.
    pub fn blit(&mut self, out: &mut Vec<u8>) {
        match &mut self.src {
            Some(Bg::Color(c)) => {
                for px in out.as_chunks_mut::<4>().0 {
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
                // advance as many frames as have fully elapsed — render
                // ticks slower than the gif's delays skip frames instead
                // of slowing the animation down
                let mut el = last.elapsed().as_millis() as u32;
                let mut advanced = false;
                loop {
                    let d = frames[*idx].1.max(30);
                    if el < d {
                        break;
                    }
                    el -= d;
                    *idx = (*idx + 1) % frames.len();
                    advanced = true;
                }
                if advanced {
                    *last = Instant::now() - Duration::from_millis(el as u64);
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

/// Killing the ffmpeg child must happen on EVERY Bg::Video drop — not
/// just engine teardown. `set()` does `self.src = None` on each theme
/// switch; without this Drop the orphaned ffmpeg + reader thread kept
/// decoding until process exit (job objects only reap with the parent).
impl Drop for Bg {
    fn drop(&mut self) {
        if let Bg::Video { stop, child, .. } = self {
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
    let flen = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if flen == 0 || flen > GIF_MAX_FILE {
        return Err(format!("gif: file size {flen} out of range"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("open {path}: {e}"))?;
    // estimated frame count = Graphic Control Extension blocks (one per
    // image frame) → uniform decimation stride to fit the RAM cap. Every
    // frame still decodes (GIF frames are dependent) but only stride-th
    // ones are kept; skipped delays fold into the previous kept frame.
    let est_frames = bytes
        .windows(3)
        .filter(|b| b == b"\x21\xF9\x04")
        .count()
        .max(1);
    let cap_frames = (GIF_MAX_BYTES / (w * h * 4)).max(1);
    let stride = est_frames.div_ceil(cap_frames).max(1);
    let dec =
        image::codecs::gif::GifDecoder::new(std::io::BufReader::new(std::io::Cursor::new(bytes)))
            .map_err(|e| format!("gif decode: {e}"))?;
    // iterate frames manually with a cumulative decoded-byte cap — the
    // 80MB kept-frames cap only applies AFTER resize, so without this a
    // hostile GIF could still decode to gigabytes of source RGBA
    let mut frames_iter = dec.into_frames();
    let mut src_bytes = 0usize;
    let mut out = Vec::new();
    let mut total = 0usize;
    // composite logical-screen frames so transparency accumulates correctly
    let mut canvas = vec![0u8; w * h * 4];
    for i in 0..usize::MAX {
        let f = match frames_iter.next() {
            Some(Ok(f)) => f,
            Some(Err(e)) => return Err(format!("gif frames: {e}")),
            None => break,
        };
        let delay = f.delay().numer_denom_ms();
        let delay_ms = delay.0 / delay.1.max(1);
        let buf = f.into_buffer();
        src_bytes += buf.as_raw().len();
        if src_bytes > GIF_MAX_DECODE {
            return Err("gif: decoded size cap exceeded".into());
        }
        let img = image::DynamicImage::ImageRgba8(buf);
        let img = cover_resize(&img, w, h).into_rgba8();
        // alpha-composite over canvas (gif frames may be partial)
        for (dst, src) in canvas
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(img.as_raw().chunks_exact(4))
        {
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
        if i % stride == 0 {
            total += canvas.len();
            out.push((canvas.clone(), delay_ms));
        } else if let Some(last) = out.last_mut() {
            // skipped frame's delay accrues to the kept frame → timing stays exact
            last.1 = last.1.saturating_add(delay_ms);
        }
        // safety net if the estimate under-counted: thin kept frames
        // (drop every 2nd, fold delays) rather than blow the RAM budget
        while total > GIF_MAX_BYTES && out.len() > 1 {
            let mut thinned: Vec<(Vec<u8>, u32)> = Vec::with_capacity(out.len() / 2 + 1);
            let mut it = out.into_iter();
            while let Some((px0, d0)) = it.next() {
                match it.next() {
                    Some((_px1, d1)) => thinned.push((px0, d0.saturating_add(d1))),
                    None => thinned.push((px0, d0)),
                }
            }
            out = thinned;
            total = out.iter().map(|(p, _)| p.len()).sum();
        }
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
    let (sw, sh) = (
        (iw as f32 * scale).round() as u32,
        (ih as f32 * scale).round() as u32,
    );
    let img = img.resize(sw, sh, image::imageops::FilterType::Triangle);
    let x = sw.saturating_sub(w as u32) / 2;
    let y = sh.saturating_sub(h as u32) / 2;
    img.crop_imm(x, y, w as u32, h as u32)
}
