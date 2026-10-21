//! The radar window.

mod radar;
mod settings_ui;
mod snapshot;
mod textures;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Stroke, Vec2, ViewportCommand};

use crate::runtime::{Shared, Source, lock};
use crate::settings::Settings;
use snapshot::{Key, snapshot};
use textures::{Icons, MapCache};

// --- Palette ----------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(11, 13, 16);
const PANEL: Color32 = Color32::from_rgb(17, 20, 24);
const CARD: Color32 = Color32::from_rgb(24, 28, 34);
const TEXT_DIM: Color32 = Color32::from_rgb(130, 138, 150);
const ACCENT: Color32 = Color32::from_rgb(63, 169, 245);
const DANGER: Color32 = Color32::from_rgb(255, 77, 79);
const OK: Color32 = Color32::from_rgb(61, 220, 132);
const RING: Color32 = Color32::from_rgba_premultiplied(40, 46, 54, 120);

// --- App --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Resources,
    Mobs,
    Alerts,
    Network,
    Display,
}

pub struct RadarApp {
    shared: Arc<Shared>,
    settings: Settings,
    saved: Settings,
    dirty_since: Option<Instant>,
    smooth: HashMap<Key, [f32; 2]>,
    local_smooth: Option<[f32; 2]>,
    maps: MapCache,
    icons: Icons,
    show_settings: bool,
    tab: Tab,
    ignore_text: String,
    net_selection: HashSet<String>,
    net_message: Option<String>,
    last_frame: Instant,
    last_prune: Instant,
    /// Click-through state last sent to the window.
    applied_click_through: Option<bool>,
    /// Frames drawn so far; X11 always-on-top is re-sent once the window is managed.
    frames: u32,
}

impl RadarApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>, maps_dir: Option<PathBuf>) -> Self {
        style(&cc.egui_ctx);
        shared.set_ctx(cc.egui_ctx.clone());
        let settings = shared.settings();
        let net_selection = match &shared.source {
            Source::Live { manager, .. } => manager.state().active.into_iter().collect(),
            Source::Replay => HashSet::new(),
        };
        Self {
            ignore_text: settings.ignore_list.join("\n"),
            saved: settings.clone(),
            settings,
            shared,
            dirty_since: None,
            smooth: HashMap::new(),
            local_smooth: None,
            maps: MapCache::new(maps_dir),
            icons: Icons::default(),
            show_settings: false,
            tab: Tab::Resources,
            net_selection,
            net_message: None,
            last_frame: Instant::now(),
            last_prune: Instant::now(),
            applied_click_through: None,
            frames: 0,
        }
    }

    /// Remembers window size, maximized state and (X11 only) position for the next start.
    fn track_window(&mut self, ctx: &egui::Context) {
        let (maximized, minimized) =
            ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().minimized.unwrap_or(false)));
        if minimized {
            return;
        }
        self.settings.window_maximized = maximized;
        if maximized {
            return;
        }
        let size = ctx.content_rect().size();
        self.settings.window_size = Some([size.x.round(), size.y.round()]);
        if self.settings.x11
            && let Some(r) = ctx.input(|i| i.viewport().outer_rect)
        {
            self.settings.window_pos = Some([r.min.x.round(), r.min.y.round()]);
        }
    }

    fn sync_settings(&mut self, ctx: &egui::Context) {
        // GNOME ignores the on-top request winit sends while mapping the window.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 3 && self.settings.x11 {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop),
            );
        }
        if self.applied_click_through != Some(self.settings.click_through) {
            self.applied_click_through = Some(self.settings.click_through);
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                ViewportCommand::MousePassthrough(self.settings.click_through),
            );
        }
        let shared_changed = *self.shared.settings.read().unwrap_or_else(|e| e.into_inner()) != self.settings;
        if shared_changed {
            *self.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = self.settings.clone();
        }
        if self.settings != self.saved {
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(1) {
                let _ = self.settings.save();
                self.saved = self.settings.clone();
                self.dirty_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(1100));
            }
        }
    }

    /// No title bar: F1 settings, F2 click-through, Esc close settings, Ctrl+Q quit.
    /// Focus the radar with Alt+Tab; move/resize via GNOME's window menu (Alt+Space).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (f1, f2, esc, quit) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F1),
                i.key_pressed(egui::Key::F2),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.ctrl && i.key_pressed(egui::Key::Q),
            )
        });
        if f1 {
            self.show_settings = !self.show_settings;
        }
        if f2 {
            self.settings.click_through = !self.settings.click_through;
        }
        if esc {
            self.show_settings = false;
        }
        if quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(22, 26, 32);
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(10);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(34, 39, 46));
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| s.spacing.item_spacing = Vec2::new(8.0, 6.0));
}

impl eframe::App for RadarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.maps.poll(&ctx);

        let snap = {
            let mut game = lock(&self.shared.game);
            if self.last_prune.elapsed() > Duration::from_secs(1) {
                game.prune(now);
                self.last_prune = now;
            }
            snapshot(&game, &self.settings)
        };

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.radar(ui, &snap, dt));

        self.settings_window(&ctx);
        self.track_window(&ctx);
        self.sync_settings(&ctx);
        // Keep the capture warning fresh without burning CPU.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4] // see-through window
    }
}

impl Drop for RadarApp {
    fn drop(&mut self) {
        if self.settings != self.saved {
            let _ = self.settings.save();
        }
    }
}
