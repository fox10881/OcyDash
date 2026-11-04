//! Scene renderer — composites background canvas + widget texts into an
//! RGBA buffer in *user space* (orientation-corrected), then rotates the
//! final buffer into the panel's native 640x480 for JPEG encoding.
//!
//! Text is shaped with cosmic-text (rustybuzz + swash): full RTL and
//! Arabic ligature support, plus automatic fallback to every installed
//! system font (Segoe UI / Tahoma cover Arabic on Windows).
//! Widgets are centered on their real ink bounding box — not the font's
//! line metrics — so text always sits exactly inside its box.

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Wrap,
};
use jpeg_encoder::{ColorType, Encoder};

use crate::background::BgEngine;
use crate::protocol::JPEG_MAX_KB;
use crate::sensors::SysSample;
use crate::theme::{Align, Theme};

#[derive(Clone, Copy)]
pub struct Rgba(pub u8, pub u8, pub u8);

/// A shaped string: per-glyph image positions + the union ink bounds.
struct TextLayout {
    /// (image_x, image_y, cache_key) in layout space
    glyphs: Vec<(i32, i32, CacheKey)>,
    min_x: f32,
    min_y: f32,
    /// ink width / height
    tw: f32,
    th: f32,
}

pub struct Renderer {
    pub w: usize,
    pub h: usize,
    font_system: FontSystem,
    swash: SwashCache,
    pub px: Vec<u8>,
}

/// Build the font system from a CURATED font list instead of the whole
/// Windows font directory — `load_system_fonts()` parses ~2000 faces and
/// costs tens of MB of RSS; this list covers Latin, Arabic (Tahoma/Segoe
/// UI/Nirmala), CJK (YaHei) and emoji at a fraction of that.
fn build_font_system() -> FontSystem {
    const FONTS: &[&str] = &[
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\segoeuib.ttf",
        r"C:\Windows\Fonts\tahoma.ttf",
        r"C:\Windows\Fonts\tahomabd.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\arialbd.ttf",
        r"C:\Windows\Fonts\cour.ttf",
        r"C:\Windows\Fonts\times.ttf",
        r"C:\Windows\Fonts\Nirmala.ttf",
        r"C:\Windows\Fonts\NirmalaB.ttf",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\seguiemj.ttf",
    ];
    let mut db = fontdb::Database::new();
    for f in FONTS {
        let p = std::path::PathBuf::from(f);
        if p.exists() {
            // Source::File keeps the DB entry path-only — font bytes are
            // read lazily the first time the face is actually used
            db.load_font_source(fontdb::Source::File(p));
        }
    }
    if db.is_empty() {
        db.load_system_fonts();
    }
    db.set_sans_serif_family("Segoe UI");
    db.set_serif_family("Times New Roman");
    db.set_monospace_family("Courier New");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

impl Renderer {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            font_system: build_font_system(),
            swash: SwashCache::new(),
            px: vec![0; w * h * 4],
        }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        self.px = vec![0; w * h * 4];
    }

    /// Shape `txt` at `px` and compute real ink bounds + glyph placements.
    fn layout(&mut self, txt: &str, px: f32) -> TextLayout {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(px, px * 1.2));
        {
            let mut b = buffer.borrow_with(&mut self.font_system);
            b.set_size(Some(8192.0), Some(8192.0));
            b.set_wrap(Wrap::None);
            b.set_text(
                txt,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                None,
            );
        }
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut glyphs = Vec::new();
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for run in buffer.layout_runs() {
            for g in run.glyphs {
                let phys = g.physical((0.0, run.line_y), 1.0);
                if let Some(img) = self
                    .swash
                    .get_image(&mut self.font_system, phys.cache_key)
                    .as_ref()
                {
                    let ix = phys.x + img.placement.left;
                    let iy = phys.y - img.placement.top;
                    min_x = min_x.min(ix as f32);
                    min_y = min_y.min(iy as f32);
                    max_x = max_x.max(ix as f32 + img.placement.width as f32);
                    max_y = max_y.max(iy as f32 + img.placement.height as f32);
                    glyphs.push((ix, iy, phys.cache_key));
                }
            }
        }
        if glyphs.is_empty() {
            return TextLayout {
                glyphs,
                min_x: 0.0,
                min_y: 0.0,
                tw: 0.0,
                th: 0.0,
            };
        }
        TextLayout {
            glyphs,
            min_x,
            min_y,
            tw: max_x - min_x,
            th: max_y - min_y,
        }
    }

    /// Ink size (w,h) of `s` at `px` — used to scale font to a resized box.
    pub fn text_size(&mut self, s: &str, px: f32) -> (f32, f32) {
        let l = self.layout(s, px);
        (l.tw, l.th)
    }

    fn blit_glyph(&mut self, x: i32, y: i32, key: CacheKey, c: Rgba) {
        let img = match self
            .swash
            .get_image(&mut self.font_system, key)
            .as_ref()
        {
            Some(i) => i,
            None => return,
        };
        let (pw, ph) = (img.placement.width as i32, img.placement.height as i32);
        match img.content {
            SwashContent::Mask => {
                let mut i = 0usize;
                for gy in 0..ph {
                    let dy = y + gy;
                    if dy < 0 || dy >= self.h as i32 {
                        i += pw as usize;
                        continue;
                    }
                    for gx in 0..pw {
                        let a = img.data[i] as u32;
                        i += 1;
                        let dx = x + gx;
                        if a == 0 || dx < 0 || dx >= self.w as i32 {
                            continue;
                        }
                        let di = ((dy as usize) * self.w + dx as usize) * 4;
                        let inv = 255 - a;
                        self.px[di] =
                            ((c.0 as u32 * a + self.px[di] as u32 * inv) / 255) as u8;
                        self.px[di + 1] =
                            ((c.1 as u32 * a + self.px[di + 1] as u32 * inv) / 255) as u8;
                        self.px[di + 2] =
                            ((c.2 as u32 * a + self.px[di + 2] as u32 * inv) / 255) as u8;
                        self.px[di + 3] = 255;
                    }
                }
            }
            SwashContent::Color => {
                let mut i = 0usize;
                for gy in 0..ph {
                    let dy = y + gy;
                    if dy < 0 || dy >= self.h as i32 {
                        i += pw as usize * 4;
                        continue;
                    }
                    for gx in 0..pw {
                        let (r, g, b, a) = (
                            img.data[i] as u32,
                            img.data[i + 1] as u32,
                            img.data[i + 2] as u32,
                            img.data[i + 3] as u32,
                        );
                        i += 4;
                        let dx = x + gx;
                        if a == 0 || dx < 0 || dx >= self.w as i32 {
                            continue;
                        }
                        let di = ((dy as usize) * self.w + dx as usize) * 4;
                        let inv = 255 - a;
                        self.px[di] = ((r * a + self.px[di] as u32 * inv) / 255) as u8;
                        self.px[di + 1] =
                            ((g * a + self.px[di + 1] as u32 * inv) / 255) as u8;
                        self.px[di + 2] =
                            ((b * a + self.px[di + 2] as u32 * inv) / 255) as u8;
                        self.px[di + 3] = 255;
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_layout(&mut self, l: &TextLayout, x: f32, y: f32, c: Rgba) {
        for &(gx, gy, key) in &l.glyphs {
            self.blit_glyph((x + gx as f32) as i32, (y + gy as f32) as i32, key, c);
        }
    }

    /// Horizontal inset of a rounded-rect edge at row `dy` (top/bottom arcs).
    fn row_inset(dy: i32, h: i32, r: i32) -> i32 {
        if r <= 0 {
            return 0;
        }
        let t = if dy < r {
            (r - dy) as f64
        } else if dy > h - r - 1 {
            (dy - (h - r - 1)) as f64
        } else {
            return 0;
        };
        (r as f64 - (r as f64 * r as f64 - t * t).sqrt()).ceil() as i32
    }

    /// Paint one horizontal span on scanline `yy`.
    fn fill_row(&mut self, yy: i32, x0: i32, x1: i32, c: Rgba) {
        if yy < 0 || yy >= self.h as i32 {
            return;
        }
        let x0 = x0.max(0);
        let x1 = x1.min(self.w as i32);
        for xx in x0..x1 {
            let i = ((yy as usize) * self.w + xx as usize) * 4;
            self.px[i] = c.0;
            self.px[i + 1] = c.1;
            self.px[i + 2] = c.2;
            self.px[i + 3] = 255;
        }
    }

    /// Filled (optionally rounded) rectangle — span-based, cheap.
    fn fill_rounded(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: Rgba) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.min(w / 2).min(h / 2).max(0);
        for dy in 0..h {
            let inset = Self::row_inset(dy, h, r);
            self.fill_row(y + dy, x + inset, x + w - inset, c);
        }
    }

    /// Single shared progress-bar renderer for every bar widget.
    ///
    /// Per scanline the track's rounded shape is the mask that decides the
    /// fill's LEFT edge (so a tiny fill hugs the track's left cap and can
    /// never poke a corner outside it), while the fill's RIGHT edge is its
    /// own pill arc — `fill_radius = (h/2).min(fill_w/2)` — until the tip
    /// reaches the track's right cap zone, where the mask takes over so a
    /// 100% fill covers the track end-to-end.
    fn draw_progress_bar(
        &mut self,
        bx: i32,
        by: i32,
        bw: i32,
        bh: i32,
        frac: f64,
        border: Rgba,
        bg: Rgba,
        fill: Rgba,
        rounded: bool,
    ) {
        if bw < 4 || bh < 4 {
            return;
        }
        let r = if rounded { bh / 2 } else { 0 };
        // 1px border ring
        self.fill_rounded(bx, by, bw, bh, r, border);

        let (ix, iy, iw, ih) = (bx + 1, by + 1, bw - 2, bh - 2);
        let ir = (r - 1).max(0); // track corner radius
        let fill_w = (iw as f64 * frac) as i32;
        // fill pill radius: never more than half the fill's own width
        let fill_r = if rounded { (ih / 2).min(fill_w / 2) } else { 0 };
        let cap_start = ix + iw - ir; // x where the track's right arc begins
        let nominal = ix + fill_w; // fill's flat right edge

        for dy in 0..ih {
            let t_inset = Self::row_inset(dy, ih, ir);
            let tr_x0 = ix + t_inset;
            let tr_x1 = ix + iw - t_inset;
            if tr_x0 >= tr_x1 {
                continue;
            }
            // track row
            self.fill_row(iy + dy, tr_x0, tr_x1, bg);
            if fill_w <= 0 {
                continue;
            }
            let x1 = if nominal < cap_start {
                // tip inside the flat zone: fill's own pill arc
                nominal - Self::row_inset(dy, ih, fill_r)
            } else {
                // tip inside/past the track's right cap: the mask supplies
                // the curve, fill covers up to it
                nominal.min(tr_x1)
            };
            if x1 > tr_x0 {
                self.fill_row(iy + dy, tr_x0, x1, fill);
            }
        }
    }

    /// Progress bar widget: bar + centered % text.
    fn draw_bar(&mut self, w: &crate::theme::Widget, frac: f64, s: &SysSample, now: f64) {
        self.draw_progress_bar(
            w.x as i32,
            w.y as i32,
            w.w as i32,
            w.h as i32,
            frac,
            Rgba(w.bar_border[0], w.bar_border[1], w.bar_border[2]),
            Rgba(w.bar_bg[0], w.bar_bg[1], w.bar_bg[2]),
            Rgba(w.bar_fill[0], w.bar_fill[1], w.bar_fill[2]),
            w.bar_rounded,
        );
        // % text centered in the bar, capped to the bar height
        let txt = w.text(s, now);
        let px = w.font_size.min((w.h as i32 - 8).max(6) as f32).max(6.0);
        let lay = self.layout(&txt, px);
        if !lay.glyphs.is_empty() {
            let ox = w.x + (w.w - lay.tw) / 2.0 - lay.min_x;
            let oy = w.y + (w.h - lay.th) / 2.0 - lay.min_y;
            self.draw_layout(&lay, ox + 1.0, oy + 1.0, Rgba(0, 0, 0));
            self.draw_layout(&lay, ox, oy, Rgba(w.color[0], w.color[1], w.color[2]));
        }
    }

    /// Compose the full scene: background + every visible widget.
    /// Font size is always honored — the box auto-grows instead.
    /// Returns (id, needed_w, needed_h) for widgets whose box must grow.
    pub fn draw(
        &mut self,
        theme: &Theme,
        bg: &mut BgEngine,
        s: &SysSample,
        now_secs: f64,
    ) -> Vec<(u32, f32, f32)> {
        const PAD_X: f32 = 16.0;
        const PAD_Y: f32 = 10.0;
        let mut grows = Vec::new();
        bg.blit(&mut self.px);
        for w in &theme.widgets {
            if !w.visible {
                continue;
            }
            // progress-bar mode: box strictly defines bar dimensions
            if w.bar {
                if let Some(frac) = w.bar_percent(s) {
                    self.draw_bar(w, frac, s, now_secs);
                }
                continue;
            }
            let txt = w.text(s, now_secs);
            if txt.is_empty() {
                continue;
            }
            let px = w.font_size.max(6.0);
            let lay = self.layout(&txt, px);
            if lay.glyphs.is_empty() {
                continue;
            }
            // effective box = stored box grown to fit the text ink (never shrink)
            let bw = w.w.max(lay.tw + PAD_X);
            let bh = w.h.max(lay.th + PAD_Y);
            if bw > w.w || bh > w.h {
                grows.push((w.id, bw, bh));
            }
            // center the ink box inside the (grown) widget box
            let ox = match w.align {
                Align::Left => w.x - lay.min_x,
                Align::Center => w.x + (bw - lay.tw) / 2.0 - lay.min_x,
                Align::Right => w.x + bw - lay.tw - lay.min_x,
            };
            let oy = w.y + (bh - lay.th) / 2.0 - lay.min_y;
            // subtle dark halo for readability over bright bgs
            self.draw_layout(&lay, ox + 2.0, oy + 2.0, Rgba(0, 0, 0));
            self.draw_layout(&lay, ox, oy, Rgba(w.color[0], w.color[1], w.color[2]));
        }
        grows
    }

    /// Encode px buffer as JPEG, auto-dropping quality under the cap.
    pub fn encode_jpeg(&self, quality: u8, buf: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut q = quality;
        loop {
            let mut out = Vec::with_capacity(JPEG_MAX_KB * 1024);
            let enc = Encoder::new(&mut out, q);
            if enc
                .encode(buf, w as u16, h as u16, ColorType::Rgba)
                .is_err()
            {
                break;
            }
            if out.len() <= JPEG_MAX_KB * 1024 || q <= 40 {
                return out;
            }
            q = q.saturating_sub(5).max(40);
        }
        Vec::new()
    }
}

/// Rotate an RGBA buffer CW into the panel's native 640x480.
/// src dims must be 640x480 (rot 0/180) or 480x640 (rot 90/270).
pub fn rotate_rgba(src: &[u8], sw: usize, sh: usize, deg: u16) -> (Vec<u8>, usize, usize) {
    match deg {
        0 => (src.to_vec(), sw, sh),
        180 => {
            let mut out = vec![0u8; sw * sh * 4];
            for y in 0..sh {
                for x in 0..sw {
                    let si = (y * sw + x) * 4;
                    let di = ((sh - 1 - y) * sw + (sw - 1 - x)) * 4;
                    out[di..di + 4].copy_from_slice(&src[si..si + 4]);
                }
            }
            (out, sw, sh)
        }
        90 | 270 => {
            // dest dims = sh x sw
            let (dw, dh) = (sh, sw);
            let mut out = vec![0u8; sw * sh * 4];
            for y in 0..dh {
                for x in 0..dw {
                    let (sx, sy) = if deg == 90 {
                        // CW: dest(x,y) = src(y, sh-1-x)
                        (y, sh - 1 - x)
                    } else {
                        // CCW: dest(x,y) = src(sw-1-y, x)
                        (sw - 1 - y, x)
                    };
                    let si = (sy * sw + sx) * 4;
                    let di = (y * dw + x) * 4;
                    out[di..di + 4].copy_from_slice(&src[si..si + 4]);
                }
            }
            (out, dw, dh)
        }
        _ => (src.to_vec(), sw, sh),
    }
}
