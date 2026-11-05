//! Theme model — the full visual state of a display layout.
//! Serialized to JSON under `%APPDATA%\ocydash\themes\`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// User-space canvas size (before rotation). Rotation 90/270 swaps these.
pub const CANVAS_W: usize = 640;
pub const CANVAS_H: usize = 480;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Theme {
    pub name: String,
    /// Physical mount rotation of the panel, degrees CW: 0/90/180/270.
    #[serde(default)]
    pub rotation: u16,
    #[serde(default)]
    pub jpeg_quality: u8,
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    /// monotonically increasing id for new widgets
    #[serde(default)]
    pub next_id: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum Background {
    #[serde(rename = "color")]
    Color { color: [u8; 3] },
    #[serde(rename = "image")]
    Image { path: String },
    #[serde(rename = "video")]
    Video { path: String, fps: u8 },
}

impl Default for Background {
    fn default() -> Self {
        Background::Color {
            color: [8, 10, 16],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum WidgetKind {
    CpuTemp,
    GpuTemp,
    CpuDieTemp,
    CpuLoad,
    GpuLoad,
    CpuPower,
    GpuPower,
    CpuFan,
    PumpFan,
    GpuFan,
    CpuName,
    GpuName,
    RamTotal,
    RamUsed,
    RamFree,
    RamLoad,
    Clock,
    CustomText { text: String },
}

impl WidgetKind {
    pub const ALL: &'static [(&'static str, WidgetKind)] = &[
        ("CPU Temp", WidgetKind::CpuTemp),
        ("GPU Temp", WidgetKind::GpuTemp),
        ("CPU Die", WidgetKind::CpuDieTemp),
        ("CPU Load", WidgetKind::CpuLoad),
        ("GPU Load", WidgetKind::GpuLoad),
        ("CPU Power", WidgetKind::CpuPower),
        ("GPU Power", WidgetKind::GpuPower),
        ("CPU Fan", WidgetKind::CpuFan),
        ("Pump Fan", WidgetKind::PumpFan),
        ("GPU Fan", WidgetKind::GpuFan),
        ("CPU Name", WidgetKind::CpuName),
        ("GPU Name", WidgetKind::GpuName),
        ("RAM Total", WidgetKind::RamTotal),
        ("RAM Used", WidgetKind::RamUsed),
        ("RAM Free", WidgetKind::RamFree),
        ("RAM Load", WidgetKind::RamLoad),
        ("Clock", WidgetKind::Clock),
        ("Custom Text", WidgetKind::CustomText { text: String::new() }),
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::CpuTemp => "CPU Temp",
            Self::GpuTemp => "GPU Temp",
            Self::CpuDieTemp => "CPU Die",
            Self::CpuLoad => "CPU Load",
            Self::GpuLoad => "GPU Load",
            Self::CpuPower => "CPU Power",
            Self::GpuPower => "GPU Power",
            Self::CpuFan => "CPU Fan",
            Self::PumpFan => "Pump Fan",
            Self::GpuFan => "GPU Fan",
            Self::CpuName => "CPU Name",
            Self::GpuName => "GPU Name",
            Self::RamTotal => "RAM Total",
            Self::RamUsed => "RAM Used",
            Self::RamFree => "RAM Free",
            Self::RamLoad => "RAM Load",
            Self::Clock => "Clock",
            Self::CustomText { .. } => "Custom Text",
        }
    }

    /// Sensor category this kind draws its value from (drives the
    /// Inspector's hardware-override dropdown). Name/RAM-GB/Clock/Text
    /// widgets have no selectable list.
    pub fn sensor_cat(&self) -> Option<crate::sensors::SensorCat> {
        use crate::sensors::SensorCat::*;
        Some(match self {
            Self::CpuTemp | Self::CpuDieTemp => CpuTemp,
            Self::CpuLoad => CpuLoad,
            Self::CpuPower => CpuPower,
            Self::CpuFan | Self::PumpFan => CpuFan,
            Self::GpuTemp => GpuTemp,
            Self::GpuLoad => GpuLoad,
            Self::GpuPower => GpuPower,
            Self::GpuFan => GpuFan,
            Self::RamLoad => RamLoad,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}
impl Default for Align {
    fn default() -> Self {
        Align::Center
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Widget {
    pub id: u32,
    pub kind: WidgetKind,
    /// top-left position in canvas px
    pub x: f32,
    pub y: f32,
    /// bounding box (used for hit-test + alignment)
    pub w: f32,
    pub h: f32,
    #[serde(default = "df_font")]
    pub font_size: f32,
    #[serde(default = "df_color")]
    pub color: [u8; 3],
    #[serde(default = "df_true")]
    pub visible: bool,
    #[serde(default)]
    pub align: Align,
    /// append unit suffix (°C, %, W) for metric widgets
    #[serde(default = "df_true")]
    pub show_unit: bool,
    #[serde(default)]
    pub decimals: u8,
    /// user-typed replacement text (overrides CpuName/GpuName auto values;
    /// blank = use sensor data)
    #[serde(default)]
    pub text_override: String,
    /// render as progress bar instead of text (percent widgets only)
    #[serde(default)]
    pub bar: bool,
    #[serde(default = "df_bar_bg")]
    pub bar_bg: [u8; 3],
    #[serde(default = "df_bar_fill")]
    pub bar_fill: [u8; 3],
    #[serde(default = "df_bar_border")]
    pub bar_border: [u8; 3],
    #[serde(default = "df_true")]
    pub bar_rounded: bool,
    /// Manual sensor pick: 0 = auto (built-in heuristic), n>0 = the n-th
    /// entry of the category's sensor list (see sensors.rs SensorCat).
    #[serde(default)]
    pub sensor_idx: usize,
}

fn df_font() -> f32 {
    48.0
}
fn df_color() -> [u8; 3] {
    [235, 240, 250]
}
fn df_true() -> bool {
    true
}
fn df_bar_bg() -> [u8; 3] {
    [18, 22, 30]
}
fn df_bar_fill() -> [u8; 3] {
    [90, 200, 140]
}
fn df_bar_border() -> [u8; 3] {
    [70, 76, 92]
}

impl Widget {
    pub fn new(id: u32, kind: WidgetKind) -> Self {
        let font_size = match &kind {
            WidgetKind::CpuTemp | WidgetKind::GpuTemp => 96.0,
            WidgetKind::Clock => 64.0,
            WidgetKind::CpuName | WidgetKind::GpuName => 26.0,
            _ => 40.0,
        };
        let (w, h) = match &kind {
            WidgetKind::CpuName | WidgetKind::GpuName => (560.0, 40.0),
            WidgetKind::CustomText { .. } => (420.0, 60.0),
            _ => (300.0, 120.0),
        };
        Self {
            id,
            kind,
            x: 60.0,
            y: 60.0,
            w,
            h,
            font_size,
            color: df_color(),
            visible: true,
            align: Align::Center,
            show_unit: true,
            decimals: 1,
            text_override: String::new(),
            bar: false,
            bar_bg: df_bar_bg(),
            bar_fill: df_bar_fill(),
            bar_border: df_bar_border(),
            bar_rounded: true,
            sensor_idx: 0,
        }
    }

    /// value honoring the manual sensor pick (0 = auto heuristic)
    fn val(&self, s: &crate::sensors::SysSample, cat: crate::sensors::SensorCat, auto: f64) -> f64 {
        s.override_value(cat, self.sensor_idx, auto)
    }

    /// 0..1 fraction for progress-bar mode (percent widgets only).
    pub fn bar_percent(&self, s: &crate::sensors::SysSample) -> Option<f64> {
        use crate::sensors::SensorCat::*;
        let v = match self.kind {
            WidgetKind::CpuLoad => self.val(s, CpuLoad, s.cpu_load),
            WidgetKind::GpuLoad => self.val(s, GpuLoad, s.gpu_load),
            WidgetKind::RamLoad => self.val(s, RamLoad, s.ram_load),
            _ => return None,
        };
        if !s.alive {
            return Some(0.0); // dead feed → empty bar, not a frozen fill
        }
        Some((v / 100.0).clamp(0.0, 1.0))
    }

    /// whether this widget kind supports progress-bar mode
    pub fn supports_bar(&self) -> bool {
        matches!(
            self.kind,
            WidgetKind::CpuLoad | WidgetKind::GpuLoad | WidgetKind::RamLoad
        )
    }

    /// true when `text()` can never vary frame-to-frame — CustomText and
    /// name widgets pinned by a text_override qualify. Everything that
    /// reads the sensor sample or the clock stays dynamic (headless uses
    /// this to split the scene into a cached static layer + live layer).
    pub fn is_static(&self) -> bool {
        match &self.kind {
            WidgetKind::CustomText { .. } => true,
            WidgetKind::CpuName | WidgetKind::GpuName => !self.text_override.is_empty(),
            _ => false,
        }
    }

    /// Text rendered for a live sample (or a preview sample when editing).
    pub fn text(&self, s: &crate::sensors::SysSample, now_secs: f64) -> String {
        use crate::sensors::SensorCat::*;
        let d = self.decimals as usize;
        // disconnected feed → visible "---" instead of frozen numbers.
        // Clock/CustomText aren't sensor data; a name widget with an
        // override shows user text, not the feed's hardware name.
        if !s.alive {
            match &self.kind {
                WidgetKind::Clock | WidgetKind::CustomText { .. } => {}
                WidgetKind::CpuName | WidgetKind::GpuName
                    if !self.text_override.is_empty() => {}
                _ => return "---".into(),
            }
        }
        match &self.kind {
            WidgetKind::CpuTemp => {
                format!("{:.*}{}", d, self.val(s, CpuTemp, s.cpu_tctl), if self.show_unit { "°C" } else { "" })
            }
            WidgetKind::GpuTemp => {
                format!("{:.*}{}", d, self.val(s, GpuTemp, s.gpu_temp), if self.show_unit { "°C" } else { "" })
            }
            WidgetKind::CpuDieTemp => {
                format!("{:.*}{}", d, self.val(s, CpuTemp, s.cpu_die), if self.show_unit { "°C" } else { "" })
            }
            WidgetKind::CpuLoad => {
                format!("{:.*}{}", d, self.val(s, CpuLoad, s.cpu_load), if self.show_unit { "%" } else { "" })
            }
            WidgetKind::GpuLoad => {
                format!("{:.*}{}", d, self.val(s, GpuLoad, s.gpu_load), if self.show_unit { "%" } else { "" })
            }
            WidgetKind::CpuPower => {
                format!("{:.*}{}", d, self.val(s, CpuPower, s.cpu_power), if self.show_unit { "W" } else { "" })
            }
            WidgetKind::GpuPower => {
                format!("{:.*}{}", d, self.val(s, GpuPower, s.gpu_power), if self.show_unit { "W" } else { "" })
            }
            WidgetKind::CpuFan => {
                format!("{:.0}{}", self.val(s, CpuFan, s.cpu_fan), if self.show_unit { " RPM" } else { "" })
            }
            WidgetKind::PumpFan => {
                format!("{:.0}{}", self.val(s, CpuFan, s.pump_fan), if self.show_unit { " RPM" } else { "" })
            }
            WidgetKind::GpuFan => {
                format!("{:.0}{}", self.val(s, GpuFan, s.gpu_fan), if self.show_unit { " RPM" } else { "" })
            }
            WidgetKind::CpuName => {
                if self.text_override.is_empty() {
                    s.cpu_name.clone()
                } else {
                    self.text_override.clone()
                }
            }
            WidgetKind::GpuName => {
                if self.text_override.is_empty() {
                    s.gpu_name.clone()
                } else {
                    self.text_override.clone()
                }
            }
            WidgetKind::RamTotal => {
                format!("{:.*}{}", d, s.ram_total, if self.show_unit { " GB" } else { "" })
            }
            WidgetKind::RamUsed => {
                format!("{:.*}{}", d, s.ram_used, if self.show_unit { " GB" } else { "" })
            }
            WidgetKind::RamFree => {
                format!("{:.*}{}", d, s.ram_free, if self.show_unit { " GB" } else { "" })
            }
            WidgetKind::RamLoad => {
                format!("{:.*}{}", d, self.val(s, RamLoad, s.ram_load), if self.show_unit { "%" } else { "" })
            }
            WidgetKind::Clock => {
                let t = now_secs as u64;
                format!("{:02}:{:02}:{:02}", (t / 3600) % 24, (t / 60) % 60, t % 60)
            }
            WidgetKind::CustomText { text } => text.clone(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        let mut t = Theme {
            name: "Default".into(),
            rotation: 0,
            jpeg_quality: 85,
            background: Background::default(),
            widgets: Vec::new(),
            next_id: 1,
        };
        let mut w = Widget::new(0, WidgetKind::CpuTemp);
        w.x = 170.0;
        w.y = 170.0;
        w.font_size = 140.0;
        w.w = 300.0;
        w.h = 160.0;
        t.widgets.push(w);
        t.next_id = 1;
        t
    }
}

// ---------- persistence ----------

pub fn themes_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    migrate_config(&base);
    let dir = base.join("ocydash").join("themes");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// One-time move of the old `ocypus-display` config dir to `ocydash`
/// (keeps every saved theme across the rebrand).
fn migrate_config(base: &PathBuf) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let old = base.join("ocypus-display");
        let new = base.join("ocydash");
        if old.is_dir() && !new.exists() {
            let _ = fs::rename(&old, &new);
        }
    });
}

pub fn list_themes() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(themes_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().map(|x| x == "json").unwrap_or(false)
                        && p.file_name().map(|n| n != "_last.json").unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

impl Theme {
    /// Saves the theme JSON after copying any local background media next
    /// to it (`<theme>.<ext>`), so a theme folder is fully self-contained.
    pub fn save(&mut self) -> std::io::Result<PathBuf> {
        self.localize_background();
        let path = themes_dir().join(format!("{}.json", sanitize(&self.name)));
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(&path, json)?;
        Ok(path)
    }

    /// Copy an existing local background file into the themes dir as
    /// `<theme name>.<ext>`, rewrite the stored path to the copy, and
    /// delete stale `<theme>.*` media left from a previous background
    /// (different type, or background removed entirely).
    fn localize_background(&mut self) {
        let dir = themes_dir();
        let stem = sanitize(&self.name);
        let mut keep: Option<PathBuf> = None;
        if let Background::Image { path } | Background::Video { path, .. } =
            &mut self.background
        {
            let src = PathBuf::from(path.as_str());
            let ext = src
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !ext.is_empty() {
                let dst = dir.join(format!("{stem}.{ext}"));
                if src.is_file() && src != dst && fs::copy(&src, &dst).is_ok() {
                    *path = dst.to_string_lossy().into_owned();
                }
                // whatever the (possibly rewritten) path resolves to inside
                // the themes dir is the file we must not orphan-delete
                let cur = PathBuf::from(path.as_str());
                if cur.parent() == Some(dir.as_path()) {
                    keep = Some(cur);
                }
            }
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                let same_stem =
                    p.file_stem().map(|s| s == stem.as_str()).unwrap_or(false);
                let is_json = p.extension().map(|x| x == "json").unwrap_or(false);
                if same_stem && !is_json && keep.as_ref() != Some(&p) {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }

    pub fn load(path: &PathBuf) -> Option<Self> {
        let json = fs::read_to_string(path).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// user-space canvas dims for the current rotation
    pub fn canvas_size(&self) -> (usize, usize) {
        if self.rotation == 90 || self.rotation == 270 {
            (CANVAS_H, CANVAS_W)
        } else {
            (CANVAS_W, CANVAS_H)
        }
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bg_path(t: &Theme) -> String {
        match &t.background {
            Background::Image { path } | Background::Video { path, .. } => path.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn localize_copies_media_and_cleans_orphans() {
        let tmp = std::env::temp_dir().join(format!("ocydash-test-{}", std::process::id()));
        // safe: tests in this binary run before any threads touch APPDATA
        unsafe { std::env::set_var("APPDATA", &tmp) };
        let media_dir = tmp.join("src");
        fs::create_dir_all(&media_dir).unwrap();
        let gif = media_dir.join("clip.gif");
        let png = media_dir.join("pic.png");
        fs::write(&gif, b"gif").unwrap();
        fs::write(&png, b"png").unwrap();

        // save with a gif → copied + path rewritten
        let mut t = Theme {
            name: "x".into(),
            background: Background::Image {
                path: gif.to_string_lossy().into_owned(),
            },
            ..Default::default()
        };
        t.save().unwrap();
        let kept = themes_dir().join("x.gif");
        assert!(kept.exists());
        assert_eq!(bg_path(&t), kept.to_string_lossy());

        // switch file type → old stem.* orphan removed
        t.background = Background::Image {
            path: png.to_string_lossy().into_owned(),
        };
        t.save().unwrap();
        assert!(themes_dir().join("x.png").exists());
        assert!(!kept.exists());

        // background removed → all stem media cleaned
        t.background = Background::Color { color: [0, 0, 0] };
        t.save().unwrap();
        assert!(!themes_dir().join("x.png").exists());
        assert!(themes_dir().join("x.json").exists());

        let _ = fs::remove_dir_all(&tmp);
    }
}

/// Path of the last-active theme (`_last.json` stores the theme NAME).
pub fn last_theme_path() -> Option<PathBuf> {
    let f = themes_dir().join("_last.json");
    let json = fs::read_to_string(f).ok()?;
    let name: String = serde_json::from_str(&json).ok()?;
    let p = themes_dir().join(format!("{}.json", sanitize(&name)));
    p.exists().then_some(p)
}

/// Persist the last-active theme name for the next launch.
pub fn write_last_theme(name: &str) {
    let f = themes_dir().join("_last.json");
    let _ = fs::write(f, serde_json::to_string(name).unwrap_or_default());
}
