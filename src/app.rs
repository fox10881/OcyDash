//! egui application — canvas preview, widget editor, theme manager.
//! A render tick composites the scene and pushes JPEGs to the engine.

use crate::background::BgEngine;
use crate::display::{EngineHandle, EngineStats};
use crate::render::{rotate_rgba, Renderer};
use crate::sensors::{SensorHandle, SysSample};
use crate::theme::{
    list_themes, themes_dir, Align, Background, Theme, Widget, WidgetKind,
};
use crate::theme::{CANVAS_H, CANVAS_W};
use chrono::Timelike;
use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions, Vec2};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const IMG_FILT: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];
const VID_FILT: &[&str] = &["mp4", "mkv", "webm", "mov", "avi"];

// Segoe MDL2 Assets glyphs (Private Use Area) — Windows' own icon font.
const ICON_DEL: &str = "\u{E74D}"; // trash
const ICON_REFRESH: &str = "\u{E72C}"; // refresh
const ICON_FOLDER: &str = "\u{E8B7}"; // open folder

// fixed widget-row metrics inside the locked 250px sidebar —
// never derived from available_width (that feedback-grew the panel)
const LABEL_W: f32 = 202.0;
const ROW_H: f32 = 24.0;

/// Hit zone on a widget box: interior = move, edges/corners = 8-way resize.
#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Interior,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Zone {
    fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon::*;
        match self {
            Zone::Interior => Move,
            Zone::N | Zone::S => ResizeVertical,
            Zone::E | Zone::W => ResizeHorizontal,
            Zone::NE | Zone::SW => ResizeNeSw,
            Zone::NW | Zone::SE => ResizeNwSe,
        }
    }
}

/// Which edge/corner zone (or interior) of `w` contains canvas point (cx,cy)?
/// `hz` = edge grab thickness in canvas px.
fn zone_of(w: &crate::theme::Widget, cx: f32, cy: f32, hz: f32) -> Option<Zone> {
    if cx < w.x - hz || cx > w.x + w.w + hz || cy < w.y - hz || cy > w.y + w.h + hz {
        return None;
    }
    let l = cx < w.x + hz;
    let r = cx > w.x + w.w - hz;
    let t = cy < w.y + hz;
    let b = cy > w.y + w.h - hz;
    Some(match (t, b, l, r) {
        (true, false, true, false) => Zone::NW,
        (true, false, false, true) => Zone::NE,
        (false, true, true, false) => Zone::SW,
        (false, true, false, true) => Zone::SE,
        (true, false, _, _) => Zone::N,
        (false, true, _, _) => Zone::S,
        (_, _, true, false) => Zone::W,
        (_, _, false, true) => Zone::E,
        _ => Zone::Interior,
    })
}

/// Load Segoe MDL2 (icon font, always present on Win10/11) as a fallback
/// font so PUA icon glyphs render instead of missing-char boxes.
fn register_icon_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for cand in [r"C:\Windows\Fonts\segmdl2.ttf", r"C:\Windows\Fonts\seguisym.ttf"] {
        if let Ok(bytes) = std::fs::read(cand) {
            fonts.font_data.insert(
                "icons".into(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .push("icons".into());
            eprintln!("[ui] icon font: {}", cand);
            break;
        }
    }
    ctx.set_fonts(fonts);
}

pub struct EditorApp {
    feed: SensorHandle,
    /// None in editor mode — the headless service owns COM3
    engine: Option<EngineHandle>,
    stop: Arc<AtomicBool>,

    theme: Theme,
    bg: BgEngine,
    renderer: Renderer,

    preview: Option<TextureHandle>,

    selected: Option<u32>,
    theme_files: Vec<PathBuf>,
    theme_pick: String,
    drag_mode: Option<(u32, Zone)>,

    last_gen: u64,
    dirty_render: bool,
    dirty_save: bool,
    last_save: Instant,
    last_frame_at: Instant,
    status: String,
    /// cached HKCU Run-key state (read once — not polled per frame)
    autorun: bool,
}

impl EditorApp {
    pub fn new(
        cc: &eframe::CreationContext,
        feed: SensorHandle,
        engine: Option<EngineHandle>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        register_icon_font(&cc.egui_ctx);
        // restore last-used theme if present
        let mut theme = Theme::default();
        let files = list_themes();
        if let Some(last) = last_theme_file() {
            if let Some(t) = Theme::load(&last) {
                theme = t;
            }
        }
        let (cw, ch) = theme.canvas_size();
        let mut bg = BgEngine::new(cw, ch);
        bg.set(&theme.background);
        Self {
            feed,
            engine,
            stop,
            theme,
            bg,
            renderer: Renderer::new(cw, ch),
            preview: None,
            selected: None,
            theme_files: files,
            theme_pick: String::new(),
            drag_mode: None,
            last_gen: 0,
            dirty_render: true,
            dirty_save: false,
            last_save: Instant::now(),
            last_frame_at: Instant::now() - Duration::from_secs(10),
            status: String::new(),
            autorun: crate::startup::is_enabled(),
        }
    }

    fn now_secs(&self) -> f64 {
        let t = chrono::Local::now();
        t.num_seconds_from_midnight() as f64 + f64::from(t.timestamp_subsec_millis()) / 1000.0
    }

    fn apply_rotation(&mut self, deg: u16) {
        if self.theme.rotation == deg {
            return;
        }
        self.theme.rotation = deg;
        let (cw, ch) = self.theme.canvas_size();
        self.renderer.resize(cw, ch);
        self.bg = BgEngine::new(cw, ch);
        self.bg.set(&self.theme.background);
        self.mark_dirty();
    }

    fn set_background(&mut self, bg: Background) {
        self.theme.background = bg.clone();
        self.bg.set(&bg);
        if let Some(e) = &self.bg.err {
            self.status = e.clone();
        }
        self.mark_dirty();
    }

    fn mark_dirty(&mut self) {
        self.dirty_render = true;
        self.dirty_save = true;
    }

    fn load_theme(&mut self, path: &PathBuf) {
        if let Some(t) = Theme::load(path) {
            self.theme = t;
            let (cw, ch) = self.theme.canvas_size();
            self.renderer.resize(cw, ch);
            self.bg = BgEngine::new(cw, ch);
            self.bg.set(&self.theme.background);
            self.selected = None;
            self.drag_mode = None;
            self.mark_dirty();
        }
    }

    /// one render tick → preview texture + device frame
    fn render_tick(&mut self, ctx: &egui::Context) {
        let sample = self.feed.latest();
        let now = self.now_secs();
        let grows = self.renderer.draw(&self.theme, &mut self.bg, &sample, now);
        // persist auto-grown boxes (box always wraps its text)
        if !grows.is_empty() {
            for (id, bw, bh) in grows {
                if let Some(w) = self.theme.widgets.iter_mut().find(|x| x.id == id) {
                    if bw > w.w {
                        w.w = bw;
                    }
                    if bh > w.h {
                        w.h = bh;
                    }
                }
            }
            self.dirty_save = true;
        }

        // Preview is ALWAYS the un-rotated user-space frame — rotation is
        // applied only to the bytes that go to the device.
        let img = ColorImage::from_rgba_unmultiplied(
            [self.renderer.w, self.renderer.h],
            &self.renderer.px,
        );
        match &mut self.preview {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.preview = Some(ctx.load_texture("preview", img, TextureOptions::LINEAR)),
        }

        let (rgba, w, h) = rotate_rgba(
            &self.renderer.px,
            self.renderer.w,
            self.renderer.h,
            self.theme.rotation,
        );
        let mut jpeg = Vec::new();
        if self
            .renderer
            .encode_jpeg(self.theme.jpeg_quality, &rgba, w, h, &mut jpeg)
        {
            if let Some(eng) = &self.engine {
                eng.push_frame(jpeg);
            }
            self.last_frame_at = Instant::now();
        }
    }

    // ---------- UI pieces ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .resizable(false)
            .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("OcyDash").strong());
                ui.separator();

                ui.label("Theme:");
                let name_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.theme.name).desired_width(120.0),
                );
                if name_resp.changed() {
                    self.mark_dirty();
                }
                egui::ComboBox::from_id_salt("theme_pick")
                    .selected_text(if self.theme_pick.is_empty() {
                        "load…".to_string()
                    } else {
                        self.theme_pick.clone()
                    })
                    .show_ui(ui, |ui| {
                        let mut picked = None;
                        for p in &self.theme_files {
                            let n = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                            if ui.selectable_label(false, &n).clicked() {
                                picked = Some((p.clone(), n));
                            }
                        }
                        if let Some((pp, n)) = picked {
                            self.load_theme(&pp);
                            self.theme_pick = n;
                        }
                    });
                if ui.button(ICON_REFRESH).on_hover_text("Rescan theme folder").clicked() {
                    self.theme_files = list_themes();
                }
                if ui.button("Save").clicked() {
                    if let Err(e) = self.theme.save() {
                        self.status = format!("save failed: {e}");
                    } else {
                        self.status = format!("saved {}", self.theme.name);
                        self.theme_files = list_themes();
                    }
                }
                if ui.button("New").clicked() {
                    let _ = self.theme.save();
                    let name = format!("{} copy", self.theme.name);
                    self.theme = Theme::default();
                    self.theme.name = name;
                    self.renderer.resize(CANVAS_W, CANVAS_H);
                    self.bg = BgEngine::new(CANVAS_W, CANVAS_H);
                    self.bg.set(&self.theme.background);
                    self.selected = None;
                    self.drag_mode = None;
                    self.mark_dirty();
                }

                if ui
                    .button(ICON_FOLDER)
                    .on_hover_text("Open themes folder")
                    .clicked()
                {
                    let _ = std::process::Command::new("explorer")
                        .arg(themes_dir())
                        .spawn();
                }

                ui.separator();
                ui.label("Rotate:");
                for d in [0u16, 90, 180, 270] {
                    if ui
                        .selectable_label(self.theme.rotation == d, format!("{d}°"))
                        .clicked()
                    {
                        self.apply_rotation(d);
                    }
                }

                // right-aligned: HKCU Run-key toggle for the headless service
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .checkbox(&mut self.autorun, "Run on startup")
                        .changed()
                    {
                        match crate::startup::set(self.autorun) {
                            Ok(()) => {
                                self.status = format!(
                                    "startup {}",
                                    if self.autorun { "enabled" } else { "disabled" }
                                );
                            }
                            Err(e) => {
                                self.autorun = !self.autorun;
                                self.status = format!("startup failed: {e}");
                            }
                        }
                    }
                });
            });
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("left")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .exact_size(250.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.heading("Background");
                let is_color = matches!(self.theme.background, Background::Color { .. });
                if ui.selectable_label(is_color, "Solid color").clicked() && !is_color {
                    self.set_background(Background::Color { color: [8, 10, 16] });
                }
                if let Background::Color { color } = &mut self.theme.background {
                    let mut c = *color;
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        *color = c;
                        self.mark_dirty();
                    }
                }
                let is_img = matches!(self.theme.background, Background::Image { .. });
                if ui.selectable_label(is_img, "Image / GIF").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("image", IMG_FILT).pick_file() {
                        self.set_background(Background::Image {
                            path: p.to_string_lossy().into(),
                        });
                    }
                }
                let is_vid = matches!(self.theme.background, Background::Video { .. });
                if ui.selectable_label(is_vid, "Video").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("video", VID_FILT).pick_file() {
                        self.set_background(Background::Video {
                            path: p.to_string_lossy().into(),
                            fps: 15,
                        });
                    }
                }

                // file/fps editors for current bg (no &mut self inside match)
                let mut pending: Option<Background> = None;
                match &mut self.theme.background {
                    Background::Image { path } => {
                        ui.horizontal(|ui| {
                            if ui.button("…").on_hover_text("change file").clicked() {
                                if let Some(p) = rfd::FileDialog::new().add_filter("image", IMG_FILT).pick_file() {
                                    pending = Some(Background::Image {
                                        path: p.to_string_lossy().into(),
                                    });
                                }
                            }
                            ui.small(short_path(path));
                        });
                    }
                    Background::Video { path, fps } => {
                        let mut f = *fps;
                        let mut fpath = None;
                        ui.horizontal(|ui| {
                            if ui.button("…").on_hover_text("change file").clicked() {
                                if let Some(p) = rfd::FileDialog::new().add_filter("video", VID_FILT).pick_file() {
                                    fpath = Some(p.to_string_lossy().into_owned());
                                }
                            }
                            ui.small(short_path(path));
                        });
                        let r = ui.add(egui::Slider::new(&mut f, 1..=30).text("fps"));
                        if r.changed() {
                            *fps = f;
                            self.dirty_save = true;
                        }
                        // rebuild the ffmpeg pipe only when the drag ends
                        if fpath.is_some() || r.drag_stopped() {
                            pending = Some(Background::Video {
                                path: fpath.unwrap_or_else(|| path.clone()),
                                fps: f,
                            });
                        }
                    }
                    Background::Color { .. } => {}
                }
                if let Some(bg) = pending {
                    self.set_background(bg);
                }
                if let Some(e) = &self.bg.err {
                    ui.colored_label(Color32::YELLOW, e.clone());
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Widgets");
                egui::ComboBox::from_label("add…").show_ui(ui, |ui| {
                    for (label, kind) in WidgetKind::ALL {
                        if ui.selectable_label(false, *label).clicked() {
                            let id = self.theme.alloc_id();
                            let mut w = Widget::new(id, kind.clone());
                            // spawn centered on the canvas
                            let (cw, ch) = self.theme.canvas_size();
                            w.x = cw as f32 / 2.0 - w.w / 2.0;
                            w.y = ch as f32 / 2.0 - w.h / 2.0;
                            self.theme.widgets.push(w);
                            self.selected = Some(id);
                            self.mark_dirty();
                        }
                    }
                });

                ui.add_space(6.0);
                let mut to_delete = None;
                let mut sel_toggle = None;
                let ids: Vec<u32> = self.theme.widgets.iter().map(|w| w.id).collect();
                for id in ids {
                    let idx = self.theme.widgets.iter().position(|w| w.id == id).unwrap();
                    let w = &self.theme.widgets[idx];
                    let label = format!("{} · {}", w.kind.label(), w.id);
                    let sel = self.selected == Some(id);
                    ui.horizontal(|ui| {
                        // fixed-size row rect — selection painted manually so
                        // text stays left-aligned (Button centres its text)
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(LABEL_W, ROW_H),
                            egui::Sense::click(),
                        );
                        let radius = egui::CornerRadius::same(4);
                        if sel {
                            let v = &ui.visuals().selection;
                            ui.painter().rect_filled(rect, radius, v.bg_fill);
                            ui.painter().rect_stroke(
                                rect,
                                radius,
                                v.stroke,
                                egui::StrokeKind::Inside,
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_stroke(
                                rect,
                                radius,
                                ui.visuals().widgets.hovered.bg_stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        if resp.clicked() {
                            sel_toggle = Some(id);
                        }
                        // left-aligned text, ellipsis-truncated to the row width
                        let inner = rect.shrink2(egui::vec2(6.0, 0.0));
                        let mut row_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(inner)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        let tcol = if sel {
                            egui::Color32::WHITE
                        } else {
                            ui.visuals().text_color()
                        };
                        row_ui.add(
                            egui::Label::new(RichText::new(label).color(tcol))
                                .halign(egui::Align::Min)
                                .truncate(),
                        );
                        if ui.small_button(ICON_DEL).clicked() {
                            to_delete = Some(id);
                        }
                    });
                }
                if let Some(id) = sel_toggle {
                    self.selected = if self.selected == Some(id) { None } else { Some(id) };
                }
                if let Some(id) = to_delete {
                    self.theme.widgets.retain(|w| w.id != id);
                    if self.selected == Some(id) {
                        self.selected = None;
                    }
                    self.mark_dirty();
                }
            });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("right")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .default_size(280.0)
            .min_size(260.0)
            .resizable(true)
            .show(ui, |ui| {
                ui.heading("Inspector");
                let Some(id) = self.selected else {
                    ui.label("Click a widget on the canvas\nto select it.");
                    self.global_controls(ui);
                    return;
                };
                let Some(idx) = self.theme.widgets.iter().position(|w| w.id == id) else {
                    self.selected = None;
                    return;
                };
                let mut ch = false;
                {
                    let w = &mut self.theme.widgets[idx];
                    ui.label(format!("{} (id {})", w.kind.label(), w.id));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("x");
                        ch |= ui.add(egui::DragValue::new(&mut w.x).speed(1.0)).changed();
                        ui.label("y");
                        ch |= ui.add(egui::DragValue::new(&mut w.y).speed(1.0)).changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("w");
                        ch |= ui.add(egui::DragValue::new(&mut w.w).speed(1.0)).changed();
                        ui.label("h");
                        ch |= ui.add(egui::DragValue::new(&mut w.h).speed(1.0)).changed();
                    });
                    ch |= ui
                        .add(egui::Slider::new(&mut w.font_size, 8.0..=220.0).text("font px"))
                        .changed();
                    let mut c = w.color;
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        w.color = c;
                        ch = true;
                    }
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("align")
                            .selected_text(format!("{:?}", w.align))
                            .show_ui(ui, |ui| {
                                for a in [Align::Left, Align::Center, Align::Right] {
                                    if ui.selectable_label(w.align == a, format!("{a:?}")).clicked() {
                                        w.align = a;
                                        ch = true;
                                    }
                                }
                            });
                        ch |= ui.checkbox(&mut w.visible, "visible").changed();
                    });
                    ui.horizontal(|ui| {
                        ch |= ui.checkbox(&mut w.show_unit, "unit").changed();
                        ch |= ui
                            .add(egui::Slider::new(&mut w.decimals, 0..=2).text("decimals"))
                            .changed();
                    });
                    match &mut w.kind {
                        WidgetKind::CustomText { text } => {
                            ui.label("text:");
                            ch |= ui.text_edit_singleline(text).changed();
                        }
                        WidgetKind::CpuName | WidgetKind::GpuName => {
                            ui.horizontal(|ui| {
                                ui.label("label:");
                                ch |= ui
                                    .text_edit_singleline(&mut w.text_override)
                                    .changed();
                                if ui.small_button("reset").clicked() {
                                    w.text_override.clear();
                                    ch = true;
                                }
                            });
                            ui.small("blank = auto-detected name");
                        }
                        _ => {}
                    }

                    // hardware-linked widgets: manual sensor override
                    // (0 = Auto heuristic, n>0 = the n-th reported entry)
                    if let Some(cat) = w.kind.sensor_cat() {
                        let sample = self.feed.latest();
                        let list = sample.sensor_list(cat);
                        ui.horizontal(|ui| {
                            ui.label("sensor:");
                            let sel = if w.sensor_idx == 0 {
                                "Auto".to_string()
                            } else {
                                list.get(w.sensor_idx - 1)
                                    .map(|e| e.0.clone())
                                    .unwrap_or_else(|| "Auto".into())
                            };
                            egui::ComboBox::from_id_salt(("sensor_pick", w.id))
                                .selected_text(sel)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(w.sensor_idx == 0, "Auto")
                                        .clicked()
                                    {
                                        w.sensor_idx = 0;
                                        ch = true;
                                    }
                                    for (i, (name, _)) in list.iter().enumerate() {
                                        if ui
                                            .selectable_label(w.sensor_idx == i + 1, name)
                                            .clicked()
                                        {
                                            w.sensor_idx = i + 1;
                                            ch = true;
                                        }
                                    }
                                });
                        });
                    }

                    // progress-bar mode (percent widgets only)
                    if w.supports_bar() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.heading("Progress bar");
                        ch |= ui.checkbox(&mut w.bar, "enable bar mode").changed();
                        if w.bar {
                            ui.horizontal(|ui| {
                                ui.label("fill");
                                ch |= ui.color_edit_button_srgb(&mut w.bar_fill).changed();
                                ui.label("bg");
                                ch |= ui.color_edit_button_srgb(&mut w.bar_bg).changed();
                                ui.label("border");
                                ch |= ui.color_edit_button_srgb(&mut w.bar_border).changed();
                            });
                            ch |= ui.checkbox(&mut w.bar_rounded, "rounded corners").changed();
                            ui.small("box = exact bar dimensions");
                        }
                    }
                }
                if ch {
                    self.mark_dirty();
                }
                ui.add_space(12.0);
                if ui.button("Delete widget").clicked() {
                    self.theme.widgets.retain(|x| x.id != id);
                    self.selected = None;
                    self.mark_dirty();
                }

                ui.add_space(12.0);
                self.global_controls(ui);
            });
    }

    fn global_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Global");
        if ui
            .add(egui::Slider::new(&mut self.theme.jpeg_quality, 40..=100).text("JPEG quality"))
            .changed()
        {
            self.mark_dirty();
        }
        let dir = themes_dir();
        ui.small(format!("themes: {}", dir.display()));
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()))
            .show(ui, |ui| {
            let avail = ui.available_size();
            let (cw, ch) = (self.renderer.w as f32, self.renderer.h as f32);
            let scale = (avail.x / cw).min(avail.y / ch).min(2.0).max(0.25);
            let size = Vec2::new(cw * scale, ch * scale);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

            if let Some(tex) = &self.preview {
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
            }

            // widget outlines — always in un-rotated user space
            const DOT: f32 = 5.0; // handle dot size (screen px)
            let dot_pos = |r: egui::Rect, z: Zone| -> egui::Pos2 {
                match z {
                    Zone::NW => r.left_top(),
                    Zone::N => r.center_top(),
                    Zone::NE => r.right_top(),
                    Zone::E => r.right_center(),
                    Zone::SE => r.right_bottom(),
                    Zone::S => r.center_bottom(),
                    Zone::SW => r.left_bottom(),
                    Zone::W => r.left_center(),
                    Zone::Interior => r.center(),
                }
            };
            for w in &self.theme.widgets {
                let r = egui::Rect::from_min_size(
                    rect.min + Vec2::new(w.x * scale, w.y * scale),
                    Vec2::new(w.w * scale, w.h * scale),
                );
                let sel = self.selected == Some(w.id);
                let col = if sel {
                    Color32::from_rgb(90, 200, 250)
                } else {
                    Color32::from_rgba_unmultiplied(120, 120, 140, 90)
                };
                ui.painter().rect_stroke(
                    r,
                    2.0,
                    (if sel { 2.0 } else { 1.0 }, col),
                    egui::StrokeKind::Inside,
                );
                if sel {
                    // 8 resize dots: corners + edge midpoints
                    for z in [Zone::NW, Zone::N, Zone::NE, Zone::E, Zone::SE, Zone::S, Zone::SW, Zone::W] {
                        let c = dot_pos(r, z);
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(c, Vec2::splat(DOT)),
                            1.5,
                            col,
                        );
                    }
                }
            }

            // hit-test: selected widget gets all 9 zones; other widgets
            // only the interior (move). Topmost wins.
            let hz = (8.0 / scale).clamp(4.0, 14.0); // edge grab, canvas px
            let sel_id = self.selected;
            let widgets = &self.theme.widgets;
            let hit_test = move |cx: f32, cy: f32| -> Option<(u32, Zone)> {
                if let Some(id) = sel_id {
                    if let Some(w) = widgets.iter().find(|w| w.id == id) {
                        if let Some(z) = zone_of(w, cx, cy, hz) {
                            return Some((id, z));
                        }
                    }
                }
                for w in widgets.iter().rev() {
                    if w.visible && cx >= w.x && cx <= w.x + w.w && cy >= w.y && cy <= w.y + w.h {
                        return Some((w.id, Zone::Interior));
                    }
                }
                None
            };
            let canvas_pos = |pos: egui::Pos2| ((pos.x - rect.min.x) / scale, (pos.y - rect.min.y) / scale);

            // cursor feedback: during drag use the drag zone, else hover zone
            let hover_zone = if let Some((_, z)) = self.drag_mode {
                Some(z)
            } else {
                ui.ctx()
                    .input(|i| i.pointer.latest_pos())
                    .filter(|p| rect.contains(*p))
                    .and_then(|p| {
                        let (cx, cy) = canvas_pos(p);
                        hit_test(cx, cy).map(|(_, z)| z)
                    })
            };
            if let Some(z) = hover_zone {
                ui.ctx().set_cursor_icon(z.cursor());
            }

            // drag start → pick widget + zone, hit-tested at the *press
            // origin* (before the pointer crossed the drag threshold)
            if resp.drag_started() {
                let origin = ui
                    .ctx()
                    .input(|i| i.pointer.press_origin())
                    .or_else(|| resp.interact_pointer_pos());
                if let Some(pos) = origin {
                    let (cx, cy) = canvas_pos(pos);
                    self.drag_mode = hit_test(cx, cy);
                    if let Some((id, _)) = self.drag_mode {
                        self.selected = Some(id);
                    }
                }
            }

            // click → select topmost
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let (cx, cy) = canvas_pos(pos);
                    self.selected = hit_test(cx, cy).map(|(id, _)| id);
                }
            }

            // drag → move or 8-way resize. drag_delta() is already the
            // per-frame delta — do NOT subtract it again.
            if resp.dragged() {
                let inc = resp.drag_delta() / scale;
                if let Some((id, z)) = self.drag_mode {
                    const MINW: f32 = 20.0;
                    const MINH: f32 = 16.0;
                    let sample = self.feed.latest();
                    let now = self.now_secs();
                    if let Some(w) = self.theme.widgets.iter_mut().find(|w| w.id == id) {
                        match z {
                            Zone::Interior => {
                                w.x = (w.x + inc.x).max(-w.w + 20.0).min(cw - 20.0);
                                w.y = (w.y + inc.y).max(-w.h + 20.0).min(ch - 20.0);
                            }
                            z => {
                                let (mut x, mut y, mut ww, mut hh) = (w.x, w.y, w.w, w.h);
                                if matches!(z, Zone::W | Zone::NW | Zone::SW) {
                                    let nx = (x + inc.x).clamp(0.0, x + ww - MINW);
                                    ww += x - nx;
                                    x = nx;
                                }
                                if matches!(z, Zone::E | Zone::NE | Zone::SE) {
                                    ww = (ww + inc.x).clamp(MINW, cw - x);
                                }
                                if matches!(z, Zone::N | Zone::NW | Zone::NE) {
                                    let ny = (y + inc.y).clamp(0.0, y + hh - MINH);
                                    hh += y - ny;
                                    y = ny;
                                }
                                if matches!(z, Zone::S | Zone::SE | Zone::SW) {
                                    hh = (hh + inc.y).clamp(MINH, ch - y);
                                }
                                w.x = x;
                                w.y = y;
                                w.w = ww;
                                w.h = hh;
                                // box resize drives the font: scale font_size
                                // so the current text fills the new box.
                                // (bar widgets: box = bar dims, text is capped
                                // to bar height — no font scaling)
                                let txt = if w.bar { String::new() } else { w.text(&sample, now) };
                                if !txt.is_empty() {
                                    let (tw, th) = self
                                        .renderer
                                        .text_size(&txt, w.font_size.max(6.0));
                                    if tw > 0.0 && th > 0.0 {
                                        let s = ((w.w - 16.0) / tw)
                                            .min((w.h - 10.0) / th)
                                            .clamp(0.05, 20.0);
                                        w.font_size =
                                            (w.font_size * s).clamp(6.0, 400.0);
                                    }
                                }
                            }
                        }
                        self.mark_dirty();
                    }
                }
            } else {
                self.drag_mode = None;
            }

            ui.painter().text(
                rect.min + Vec2::new(6.0, 6.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "{}×{} · preview 0° · device rot {}°",
                    cw as u32, ch as u32, self.theme.rotation
                ),
                egui::FontId::proportional(13.0),
                Color32::from_rgba_unmultiplied(180, 190, 210, 160),
            );
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .resizable(false)
            .show(ui, |ui| {
            let st: Option<EngineStats> = self.engine.as_ref().map(|e| e.stats());
            let s: SysSample = self.feed.latest();
            ui.horizontal(|ui| {
                match &st {
                    Some(st) => {
                        let (txt, col) = if st.connected {
                            (format!("● {} live", st.port), Color32::from_rgb(90, 220, 130))
                        } else {
                            (format!("○ disconnected {}", st.last_err), Color32::from_rgb(240, 90, 80))
                        };
                        ui.colored_label(col, txt);
                        ui.separator();
                        ui.small(format!(
                            "frames {} · jpeg {:.0}KB",
                            st.frames_sent,
                            st.last_jpeg_bytes as f32 / 1024.0
                        ));
                    }
                    None => {
                        ui.colored_label(
                            Color32::from_rgb(140, 170, 240),
                            "editor — saves stream via headless service",
                        );
                    }
                }
                ui.separator();
                ui.small(format!(
                    "CPU {:.1}°C · GPU {:.1}°C · load {:.0}% · {:.0}W",
                    s.cpu_tctl, s.gpu_temp, s.cpu_load, s.cpu_power
                ));
                if !self.status.is_empty() {
                    ui.separator();
                    ui.small(self.status.clone());
                }
            });
        });
    }
}

impl eframe::App for EditorApp {
    /// Non-painting logic: runs even when the window is hidden, so the
    /// device keeps receiving frames while minimized.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.stop.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // re-render when: sensor data changed, theme dirty, bg animating,
        // or heartbeat so the device keeps receiving frames
        let feed_gen = self.feed.generation();
        let animated = matches!(
            self.bg.src,
            Some(crate::background::Bg::Animated { .. }) | Some(crate::background::Bg::Video { .. })
        ) || self
            .theme
            .widgets
            .iter()
            .any(|w| matches!(w.kind, WidgetKind::Clock) && w.visible);
        let heartbeat = self.last_frame_at.elapsed() > Duration::from_millis(800);

        if self.dirty_render || feed_gen != self.last_gen || animated || heartbeat {
            self.last_gen = feed_gen;
            self.dirty_render = false;
            self.render_tick(ctx);
        }

        // debounced autosave
        if self.dirty_save && self.last_save.elapsed() > Duration::from_millis(1200) {
            let _ = self.theme.save();
            self.dirty_save = false;
            self.last_save = Instant::now();
        }

        ctx.request_repaint_after(if animated {
            Duration::from_millis(66)
        } else {
            Duration::from_millis(250)
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.top_bar(ui);
        self.status_bar(ui);
        self.left_panel(ui);
        self.right_panel(ui);
        self.canvas(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(eng) = &self.engine {
            eng.shutdown();
        }
        let _ = self.theme.save();
    }
}

fn short_path(p: &str) -> String {
    PathBuf::from(p)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Persist last-active theme name across launches.
fn last_theme_file() -> Option<PathBuf> {
    crate::theme::last_theme_path()
}

impl Drop for EditorApp {
    fn drop(&mut self) {
        crate::theme::write_last_theme(&self.theme.name);
    }
}
