//! The radar window.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Mesh, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, ViewportCommand, epaint::Vertex,
};

use crate::data::{MobClass, Resource};
use crate::game::{Game, ZoneView};
use crate::runtime::{Shared, Source, lock};
use crate::settings::Settings;

// --- Palette ----------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(11, 13, 16);
const PANEL: Color32 = Color32::from_rgb(17, 20, 24);
const CARD: Color32 = Color32::from_rgb(24, 28, 34);
const TEXT_DIM: Color32 = Color32::from_rgb(130, 138, 150);
const ACCENT: Color32 = Color32::from_rgb(63, 169, 245);
const DANGER: Color32 = Color32::from_rgb(255, 77, 79);
const OK: Color32 = Color32::from_rgb(61, 220, 132);
const RING: Color32 = Color32::from_rgba_premultiplied(40, 46, 54, 120);

fn resource_color(r: Resource) -> Color32 {
    match r {
        Resource::Wood => Color32::from_rgb(160, 112, 60),
        Resource::Rock => Color32::from_rgb(142, 142, 158),
        Resource::Fiber => Color32::from_rgb(124, 179, 66),
        Resource::Hide => Color32::from_rgb(192, 138, 90),
        Resource::Ore => Color32::from_rgb(79, 139, 214),
    }
}

fn enchant_color(e: u8) -> Option<Color32> {
    match e {
        1 => Some(Color32::from_rgb(61, 220, 132)),
        2 => Some(Color32::from_rgb(63, 169, 245)),
        3 => Some(Color32::from_rgb(180, 107, 255)),
        4 => Some(Color32::from_rgb(255, 213, 74)),
        _ => None,
    }
}

// --- Map backgrounds ----------------------------------------------------------

enum MapTex {
    Loading,
    Missing,
    Ready(TextureHandle),
}

struct MapCache {
    dir: Option<PathBuf>,
    textures: HashMap<String, MapTex>,
    tx: Sender<(String, Option<ColorImage>)>,
    rx: Receiver<(String, Option<ColorImage>)>,
}

impl MapCache {
    fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { dir, textures: HashMap::new(), tx, rx }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for (id, img) in self.rx.try_iter() {
            let tex = match img {
                Some(img) => MapTex::Ready(ctx.load_texture(format!("map-{id}"), img, TextureOptions::LINEAR)),
                None => MapTex::Missing,
            };
            self.textures.insert(id, tex);
        }
    }

    /// Returns the texture once decoded; starts a background load on first use.
    fn get(&mut self, ctx: &egui::Context, id: &str) -> Option<&TextureHandle> {
        if !self.textures.contains_key(id) {
            self.textures.insert(id.to_owned(), MapTex::Loading);
            let Some(dir) = self.dir.clone() else {
                self.textures.insert(id.to_owned(), MapTex::Missing);
                return None;
            };
            let (tx, id, ctx) = (self.tx.clone(), id.to_owned(), ctx.clone());
            std::thread::spawn(move || {
                let base = id.split('-').next().unwrap_or(&id).to_owned();
                let img = [id.clone(), base]
                    .iter()
                    .map(|n| dir.join(format!("{n}.webp")))
                    .find(|p| p.is_file())
                    .and_then(|p| image::open(p).ok())
                    .map(|img| {
                        let rgba = img.to_rgba8();
                        ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
                    });
                let _ = tx.send((id, img));
                ctx.request_repaint();
            });
        }
        match self.textures.get(id) {
            Some(MapTex::Ready(t)) => Some(t),
            _ => None,
        }
    }
}

// --- Per-frame snapshot ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Res(i64),
    Mob(i64),
    Mist(i64),
}

enum Marker {
    Resource { resource: Resource, tier: u8, enchant: u8, size: i64, living: bool },
    Mob { class: Option<MobClass>, tier: u8, health: u8 },
    Mist { solo: bool, enchant: u8 },
}

struct Item {
    key: Key,
    pos: [f32; 2],
    marker: Marker,
    label: String,
}

struct Snapshot {
    zone: Option<ZoneView>,
    local: [f32; 2],
    bounds: Option<crate::data::Bounds>,
    items: Vec<Item>,
}

fn snapshot(game: &Game, s: &Settings) -> Snapshot {
    let zone = game.zone();
    let mut items = Vec::new();

    for (id, h) in &game.harvestables {
        if h.size <= 0 || !s.resource_filter(h.resource, false).accepts(h.tier, h.enchant) {
            continue;
        }
        items.push(Item {
            key: Key::Res(*id),
            pos: h.pos,
            marker: Marker::Resource { resource: h.resource, tier: h.tier, enchant: h.enchant, size: h.size, living: false },
            label: format!("{} T{}.{}  ·  {} left", h.resource.name(), h.tier, h.enchant, h.size),
        });
    }
    for (id, m) in &game.mobs {
        let marker = match m.class {
            Some(MobClass::Living(r)) => {
                if !s.resource_filter(r, true).accepts(m.tier, m.enchant) {
                    continue;
                }
                Marker::Resource { resource: r, tier: m.tier, enchant: m.enchant, size: 0, living: true }
            }
            class => {
                if !s.shows_mob(class) {
                    continue;
                }
                Marker::Mob { class, tier: m.tier, health: m.health }
            }
        };
        let name = if m.name.is_empty() { format!("Unknown mob #{}", m.type_id) } else { pretty_mob_name(&m.name) };
        let hp = if m.max_hp > 0 { format!("  ·  {}%", (u32::from(m.health) * 100) / 255) } else { String::new() };
        let ench = if m.enchant > 0 { format!(".{}", m.enchant) } else { String::new() };
        items.push(Item { key: Key::Mob(*id), pos: m.pos, marker, label: format!("{name} T{}{ench}{hp}", m.tier) });
    }
    if s.mist_portals {
        for (id, m) in &game.mists {
            let kind = if m.solo { "Solo mist" } else { "Group mist" };
            items.push(Item {
                key: Key::Mist(*id),
                pos: m.pos,
                marker: Marker::Mist { solo: m.solo, enchant: m.enchant },
                label: format!("{kind} portal .{}", m.enchant),
            });
        }
    }
    let bounds = zone.as_ref().and_then(|z| game.data().zone(&z.id)).and_then(|z| z.bounds);
    Snapshot { zone, local: game.local, bounds, items }
}

/// `T6_MOB_KEEPER_DRUID_BOSS` -> `Keeper Druid Boss`.
fn pretty_mob_name(unique: &str) -> String {
    let trimmed = unique.split_once("_MOB_").map_or(unique, |(_, rest)| rest);
    trimmed
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

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
    show_settings: bool,
    tab: Tab,
    ignore_text: String,
    net_selection: HashSet<String>,
    net_message: Option<String>,
    last_frame: Instant,
    last_prune: Instant,
    rate: (u64, Instant, f32),
    /// Click-through state last sent to the window.
    applied_click_through: Option<bool>,
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
            show_settings: false,
            tab: Tab::Resources,
            net_selection,
            net_message: None,
            last_frame: Instant::now(),
            last_prune: Instant::now(),
            rate: (0, Instant::now(), 0.0),
            applied_click_through: None,
        }
    }

    /// Remembers window size and maximized state for the next start. Wayland
    /// does not let apps know or choose their position, so that is left to GNOME.
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
    }

    fn sync_settings(&mut self, ctx: &egui::Context) {
        if self.applied_click_through != Some(self.settings.click_through) {
            self.applied_click_through = Some(self.settings.click_through);
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::MousePassthrough(self.settings.click_through));
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

    fn update_rate(&mut self) {
        let packets = self.shared.stats.packets.load(Ordering::Relaxed);
        let elapsed = self.rate.1.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.rate = (packets, Instant::now(), (packets - self.rate.0) as f32 / elapsed);
        }
    }

    // --- Top bar ------------------------------------------------------------------

    fn radar(&mut self, ui: &mut egui::Ui, snap: &Snapshot, dt: f32) {
        let ctx = ui.ctx().clone();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
        let rect = resp.rect;
        let bg_alpha = (self.settings.background_opacity.clamp(0.0, 1.0) * 255.0) as u8;
        if bg_alpha > 0 {
            painter.rect_filled(rect, 8.0, Color32::from_rgba_unmultiplied(BG.r(), BG.g(), BG.b(), bg_alpha));
        }
        self.shortcuts(&ctx);

        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.settings.range = (self.settings.range * (-scroll * 0.0025).exp()).clamp(15.0, 250.0);
            }
        }

        // Smooth motion toward the latest known positions.
        let alpha = 1.0 - (-dt * 14.0).exp();
        let mut moving = false;
        let mut step = |cur: &mut [f32; 2], target: [f32; 2]| {
            let (dx, dy) = (target[0] - cur[0], target[1] - cur[1]);
            if dx.abs() + dy.abs() > 40.0 {
                *cur = target;
            } else {
                cur[0] += dx * alpha;
                cur[1] += dy * alpha;
                moving |= dx.abs() + dy.abs() > 0.02;
            }
        };
        let local = self.local_smooth.get_or_insert(snap.local);
        step(local, snap.local);
        let local = *local;
        let live: HashSet<Key> = snap.items.iter().map(|i| i.key).collect();
        self.smooth.retain(|k, _| live.contains(k));
        for item in &snap.items {
            let cur = self.smooth.entry(item.key).or_insert(item.pos);
            step(cur, item.pos);
        }
        if moving {
            ctx.request_repaint();
        }

        let center = rect.center();
        let radius = (rect.width().min(rect.height()) / 2.0 - 16.0).max(40.0);
        let ppu = radius / self.settings.range;
        let k = ppu * std::f32::consts::FRAC_1_SQRT_2;
        let to_screen = |w: [f32; 2]| -> Pos2 {
            let (dx, dy) = (w[0] - local[0], w[1] - local[1]);
            center + Vec2::new(k * (dx + dy), k * (dx - dy))
        };
        let painter = painter.with_clip_rect(rect.shrink(1.0));

        // Map background: a textured quad over the zone bounds.
        if self.settings.show_map
            && let Some(zone) = &snap.zone
            && let Some(tex) = self.maps.get(&ctx, &zone.id)
        {
            let (x0, y0, e) = match snap.bounds {
                Some(b) => {
                    let e = (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]);
                    ((b.min[0] + b.max[0]) / 2.0 - e / 2.0, (b.min[1] + b.max[1]) / 2.0 - e / 2.0, e)
                }
                None => (-412.5, -412.5, 825.0),
            };
            let shade = (self.settings.map_brightness.clamp(0.05, 1.0) * 255.0) as u8;
            let alpha = (self.settings.map_opacity.clamp(0.0, 1.0) * 255.0) as u8;
            let tint = Color32::from_rgba_unmultiplied(shade, shade, shade, alpha);
            let mut mesh = Mesh::with_texture(tex.id());
            for (w, uv) in [([x0, y0], [0.0, 0.0]), ([x0 + e, y0], [1.0, 0.0]), ([x0 + e, y0 + e], [1.0, 1.0]), ([x0, y0 + e], [0.0, 1.0])] {
                mesh.vertices.push(Vertex { pos: to_screen(w), uv: Pos2::new(uv[0], uv[1]), color: tint });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(Shape::mesh(mesh));
        }

        if self.settings.show_rings {
            let step = nice_step(self.settings.range / 3.0);
            let mut r = step;
            while r * ppu < rect.size().length() / 2.0 {
                painter.circle_stroke(center, r * ppu, Stroke::new(1.0, RING));
                painter.text(
                    center + Vec2::new(r * ppu * 0.707 + 3.0, -r * ppu * 0.707),
                    Align2::LEFT_BOTTOM,
                    format!("{r:.0}m"),
                    FontId::proportional(10.0),
                    Color32::from_rgb(80, 88, 100),
                );
                r += step;
            }
        }

        // Entities.
        let scale = self.settings.icon_scale.clamp(0.5, 2.5);
        let pointer = resp.hover_pos();
        let mut hovered: Option<(f32, Pos2, &Item)> = None;
        let mut draw_order: Vec<&Item> = snap.items.iter().collect();
        draw_order.sort_by_key(|i| match i.marker {
            Marker::Mob { class: None | Some(MobClass::Normal), .. } => 0,
            Marker::Resource { .. } => 1,
            Marker::Mob { .. } => 2,
            Marker::Mist { .. } => 3,
        });
        for item in draw_order {
            let at = to_screen(self.smooth.get(&item.key).copied().unwrap_or(item.pos));
            if !rect.expand(30.0).contains(at) {
                continue;
            }
            self.draw_marker(&painter, at, &item.marker, scale);
            if let Some(p) = pointer {
                let d = p.distance(at);
                if d < 14.0 * scale && hovered.is_none_or(|(best, _, _)| d < best) {
                    hovered = Some((d, at, item));
                }
            }
        }

        // You.
        painter.circle_filled(center, 9.0, Color32::from_rgba_premultiplied(255, 255, 255, 18));
        painter.circle_filled(center, 4.5, Color32::WHITE);
        painter.circle_stroke(center, 4.5, Stroke::new(1.5, BG));

        if let Some((_, at, item)) = hovered {
            let dist = {
                let (dx, dy) = (item.pos[0] - snap.local[0], item.pos[1] - snap.local[1]);
                (dx * dx + dy * dy).sqrt()
            };
            let text = format!("{}  ·  {dist:.0}m", item.label);
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.5), Color32::WHITE);
            let pos = at + Vec2::new(12.0, -galley.size().y - 10.0);
            let bg = Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(7.0, 4.0));
            painter.rect_filled(bg, 5.0, Color32::from_rgba_premultiplied(10, 12, 15, 235));
            painter.rect_stroke(bg, 5.0, Stroke::new(1.0, Color32::from_rgb(55, 62, 72)), StrokeKind::Inside);
            painter.galley(pos, galley, Color32::WHITE);
        }

        self.capture_warning(&painter, rect);
        self.alert_overlay(&ctx, &painter, rect);
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

    /// The only status ever shown: a red note when nothing is being captured.
    fn capture_warning(&self, painter: &egui::Painter, rect: Rect) {
        let Source::Live { manager, .. } = &self.shared.source else { return };
        if !manager.state().active.is_empty() {
            return;
        }
        let at = rect.left_top() + Vec2::new(14.0, 14.0);
        painter.circle_filled(at, 4.0, DANGER);
        painter.text(at + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, "Not capturing — F1 → Network", FontId::proportional(12.0), DANGER);
    }

    fn draw_marker(&self, painter: &egui::Painter, at: Pos2, marker: &Marker, scale: f32) {
        match *marker {
            Marker::Resource { resource, tier, enchant, size, living } => {
                let s = 9.0 * scale;
                let fill = resource_color(resource);
                let border = enchant_color(enchant).map_or(Stroke::new(1.0, Color32::from_black_alpha(160)), |c| Stroke::new(2.0 * scale, c));
                if living {
                    painter.circle_filled(at, s + 1.0, fill);
                    painter.circle_stroke(at, s + 1.0, border);
                } else {
                    let r = Rect::from_center_size(at, Vec2::splat(s * 2.0));
                    painter.rect_filled(r, 4.0 * scale, fill);
                    painter.rect_stroke(r, 4.0 * scale, border, StrokeKind::Outside);
                }
                painter.text(at, Align2::CENTER_CENTER, tier.to_string(), FontId::proportional(11.0 * scale), Color32::WHITE);
                if self.settings.show_stack_count && size > 0 {
                    painter.text(
                        at + Vec2::new(s + 3.0, -s),
                        Align2::LEFT_TOP,
                        size.to_string(),
                        FontId::proportional(9.5 * scale),
                        Color32::from_rgb(200, 205, 215),
                    );
                }
            }
            Marker::Mob { class, tier, health } => {
                let (r, color) = match class {
                    Some(MobClass::Boss) => (7.0, DANGER),
                    Some(MobClass::MiniBoss) => (5.5, Color32::from_rgb(255, 159, 67)),
                    Some(MobClass::Enchanted) => (4.5, Color32::from_rgb(180, 107, 255)),
                    Some(MobClass::MistBoss) => (8.0, Color32::from_rgb(255, 92, 240)),
                    Some(MobClass::Normal) => (3.5, Color32::from_rgb(154, 165, 177)),
                    _ => (3.0, Color32::from_rgb(92, 99, 112)),
                };
                let r = r * scale;
                if class == Some(MobClass::MistBoss) {
                    painter.add(Shape::convex_polygon(diamond(at, r), color, Stroke::new(1.0, BG)));
                } else {
                    painter.circle_filled(at, r, color);
                    painter.circle_stroke(at, r, Stroke::new(1.0, BG));
                }
                if class == Some(MobClass::Boss) {
                    painter.circle_stroke(at, r + 3.0, Stroke::new(1.5, DANGER.gamma_multiply(0.6)));
                }
                if self.settings.mob_health && health < 250 {
                    let frac = f32::from(health) / 255.0;
                    let bar = Rect::from_min_size(at + Vec2::new(-7.0, r + 3.0), Vec2::new(14.0, 2.0));
                    painter.rect_filled(bar, 1.0, Color32::from_rgb(40, 44, 52));
                    painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new(14.0 * frac, 2.0)), 1.0, health_color(frac));
                }
                if self.settings.mob_tiers && tier > 0 {
                    painter.text(at + Vec2::new(r + 3.0, 0.0), Align2::LEFT_CENTER, format!("T{tier}"), FontId::proportional(9.5), TEXT_DIM);
                }
            }
            Marker::Mist { solo, enchant } => {
                let color = if solo { Color32::from_rgb(64, 224, 208) } else { Color32::from_rgb(199, 125, 255) };
                let r = 8.0 * scale;
                painter.add(Shape::convex_polygon(diamond(at, r), color.gamma_multiply(0.35), Stroke::new(2.0, color)));
                if enchant > 0 {
                    painter.text(at, Align2::CENTER_CENTER, enchant.to_string(), FontId::proportional(10.0 * scale), Color32::WHITE);
                }
            }
        }
    }

    fn alert_overlay(&self, ctx: &egui::Context, painter: &egui::Painter, rect: Rect) {
        let alert = lock(&self.shared.alert);
        let now = Instant::now();
        let mut animating = false;
        if let Some((threat, at)) = &alert.banner {
            let age = now.duration_since(*at).as_secs_f32();
            if age < 6.0 {
                animating = true;
                let pulse = 0.5 + 0.5 * (age * 6.0).sin();
                painter.rect_stroke(rect.shrink(1.5), 8.0, Stroke::new(3.0, DANGER.gamma_multiply(0.4 + 0.6 * pulse)), StrokeKind::Inside);
                let who = {
                    let mut s = threat.name.clone();
                    if !threat.guild.is_empty() {
                        s += &format!("  [{}]", threat.guild);
                    }
                    if let Some(ip) = threat.item_power {
                        s += &format!("  ·  IP {ip}");
                    }
                    s
                };
                let title = if threat.hostile { "HOSTILE NEARBY" } else { "PLAYER NEARBY" };
                let galley = painter.layout_no_wrap(format!("{title}    {who}"), FontId::proportional(15.0), Color32::WHITE);
                let pos = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() + 14.0);
                let bg = Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(14.0, 8.0));
                let fade = if age > 5.0 { 6.0 - age } else { 1.0 };
                painter.rect_filled(bg, 8.0, Color32::from_rgb(150, 25, 30).gamma_multiply(fade));
                painter.galley(pos, galley, Color32::WHITE.gamma_multiply(fade));
            }
        }
        if let Some(at) = alert.flash_at {
            let age = now.duration_since(at).as_secs_f32();
            if age < 0.45 {
                animating = true;
                let layer = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("threat-flash")));
                layer.rect_filled(ctx.content_rect(), 0.0, DANGER.gamma_multiply(0.45 * (1.0 - age / 0.45)));
            }
        }
        if animating {
            ctx.request_repaint();
        }
    }

    // --- Settings --------------------------------------------------------------------------

    /// Settings open in their own normal window, so they fit no matter how
    /// small the overlay is.
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("OpenRadar settings")
            .with_app_id("openradar")
            .with_inner_size([520.0, 620.0])
            .with_min_inner_size([420.0, 320.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let close = ui.input(|i| {
                i.viewport().close_requested() || i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F1)
            });
            if close {
                self.show_settings = false;
            }
            let frame = egui::Frame::new().fill(ui.visuals().window_fill).inner_margin(egui::Margin::same(14));
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Resources, "Resources"),
                        (Tab::Mobs, "Mobs"),
                        (Tab::Alerts, "Alerts"),
                        (Tab::Network, "Network"),
                        (Tab::Display, "Display"),
                    ] {
                        ui.selectable_value(&mut self.tab, tab, label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::Resources => self.resources_tab(ui),
                    Tab::Mobs => self.mobs_tab(ui),
                    Tab::Alerts => self.alerts_tab(ui),
                    Tab::Network => self.network_tab(ui),
                    Tab::Display => self.display_tab(ui),
                });
            });
        });
    }

    fn resources_tab(&mut self, ui: &mut egui::Ui) {
        for (title, living) in [("Static resources (trees, rocks, fiber, ore)", false), ("Living resources (animals, critters)", true)] {
            ui.label(RichText::new(title).strong());
            egui::Grid::new(("res-grid", living)).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                for (i, r) in Resource::ALL.iter().enumerate() {
                    let f = if living { &mut self.settings.living_resources[i] } else { &mut self.settings.static_resources[i] };
                    ui.checkbox(&mut f.show, RichText::new(r.name()).color(resource_color(*r)));
                    ui.add_enabled(f.show, egui::Slider::new(&mut f.min_tier, 1..=8).text("min tier"));
                    ui.add_enabled(f.show, egui::Slider::new(&mut f.min_enchant, 0..=4).text("min .ench"));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
        ui.checkbox(&mut self.settings.show_stack_count, "Show remaining stacks next to static resources");
    }

    fn mobs_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.checkbox(&mut s.mobs_normal, "Normal mobs");
        ui.checkbox(&mut s.mobs_enchanted, "Enchanted / champions");
        ui.checkbox(&mut s.mobs_miniboss, "Mini-bosses / veterans");
        ui.checkbox(&mut s.mobs_boss, "Bosses");
        ui.checkbox(&mut s.mobs_mistboss, "Mist bosses");
        ui.checkbox(&mut s.mobs_unknown, "Unknown mobs (not in the mob table)");
        ui.checkbox(&mut s.mist_portals, "Mist portals");
        ui.separator();
        ui.checkbox(&mut s.mob_health, "Health bars on damaged mobs");
        ui.checkbox(&mut s.mob_tiers, "Tier labels");
    }

    fn alerts_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Threat alert").strong());
        ui.label(RichText::new("Black zone: every player. Yellow/Red: hostile-flagged only. Safe: never.").color(TEXT_DIM).size(11.5));
        let s = &mut self.settings;
        ui.checkbox(&mut s.alert_flash, "Flash the window");
        ui.checkbox(&mut s.alert_sound, "Play a sound");
        ui.add_enabled_ui(s.alert_sound, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("sound").selected_text(&s.alert_sound_file).show_ui(ui, |ui| {
                    for name in &self.shared.sounds {
                        ui.selectable_value(&mut s.alert_sound_file, name.clone(), name);
                    }
                });
                if ui.button("Test sound").clicked() {
                    self.shared.play_sound(&s.alert_sound_file, s.alert_volume);
                }
            });
            ui.add(egui::Slider::new(&mut s.alert_volume, 0.0..=1.0).text("volume"));
            ui.add(egui::Slider::new(&mut s.alert_cooldown_ms, 0..=5000).text("cooldown (ms)"));
        });
        if ui.button("Test alert").on_hover_text("Simulate a hostile player with the current settings").clicked() {
            *self.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = self.settings.clone();
            self.shared.test_alert();
        }
        if !self.shared.has_audio() {
            ui.label(RichText::new("No audio device found — sounds are disabled.").color(DANGER));
        }
        ui.separator();
        ui.label(RichText::new("Ignore list").strong());
        ui.label(RichText::new("One player, guild or alliance per line; they never trigger an alert.").color(TEXT_DIM).size(11.5));
        if ui.add(egui::TextEdit::multiline(&mut self.ignore_text).desired_rows(6).desired_width(f32::INFINITY)).changed() {
            self.settings.ignore_list = self.ignore_text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
        }
    }

    fn network_tab(&mut self, ui: &mut egui::Ui) {
        let Source::Live { manager, interfaces } = &self.shared.source else {
            ui.label("Replaying a capture file; live capture is off.");
            return;
        };
        let state = manager.state();
        if state.active.is_empty() {
            ui.label(RichText::new("Not capturing. Tick the adapter Albion uses (usually your Wi-Fi or Ethernet).").color(DANGER));
        }
        let list = interfaces.read().unwrap_or_else(|e| e.into_inner()).clone();
        egui::Grid::new("ifaces").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for i in openradar::capture::rank_candidates(&list) {
                let mut on = self.net_selection.contains(&i.name);
                let label = if i.description.is_empty() { i.name.clone() } else { format!("{} ({})", i.name, i.description) };
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        self.net_selection.insert(i.name.clone());
                    } else {
                        self.net_selection.remove(&i.name);
                    }
                }
                ui.label(RichText::new(&i.address).color(TEXT_DIM));
                let cat = openradar::capture::categorize(&i.name, &i.description);
                let active = state.active.contains(&i.name);
                ui.label(RichText::new(if active { "capturing".to_owned() } else { cat.as_str().to_owned() }).color(if active { OK } else { TEXT_DIM }));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                let target: Vec<_> = list.iter().filter(|i| self.net_selection.contains(&i.name)).cloned().collect();
                self.net_message = Some(match manager.reconfigure(&target) {
                    Ok(()) => "Capture updated.".into(),
                    Err(e) => format!("{e}"),
                });
                self.settings.capture_interfaces = target.into_iter().map(|i| i.name).collect();
            }
            if ui.button("Refresh list").clicked()
                && let Ok(fresh) = openradar::capture::enumerate_interfaces()
            {
                *interfaces.write().unwrap_or_else(|e| e.into_inner()) = fresh;
            }
        });
        if let Some(msg) = &self.net_message {
            ui.label(RichText::new(msg).color(TEXT_DIM));
        }
        for (name, err) in &state.last_errors {
            ui.label(RichText::new(format!("{name}: {err}")).color(DANGER).size(11.5));
        }
    }

    fn display_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.add(egui::Slider::new(&mut s.range, 15.0..=250.0).text("range (m) — or scroll on the radar"));
        ui.add(egui::Slider::new(&mut s.icon_scale, 0.6..=2.0).text("icon size"));
        ui.checkbox(&mut s.show_map, "Map background");
        ui.add_enabled(s.show_map, egui::Slider::new(&mut s.map_opacity, 0.0..=1.0).text("map opacity"));
        ui.add_enabled(s.show_map, egui::Slider::new(&mut s.map_brightness, 0.1..=1.0).text("map brightness"));
        ui.add(egui::Slider::new(&mut s.background_opacity, 0.0..=1.0).text("background opacity"));
        ui.checkbox(&mut s.show_rings, "Distance rings");
        ui.checkbox(&mut s.click_through, "Click-through (clicks go to the game; F2 toggles)");
        ui.separator();
        ui.label(
            RichText::new("Focus the radar with Alt+Tab, then: F1 settings  ·  F2 click-through  ·  Ctrl+Q quit\nMove, resize, always on top: Alt+Space. Zoom by scrolling when click-through is off.")
                .color(TEXT_DIM)
                .size(11.0),
        );
        if ui.button("Quit OpenRadar").clicked() {
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close);
        }
    }
}

fn diamond(at: Pos2, r: f32) -> Vec<Pos2> {
    vec![at + Vec2::new(0.0, -r), at + Vec2::new(r, 0.0), at + Vec2::new(0.0, r), at + Vec2::new(-r, 0.0)]
}

fn health_color(frac: f32) -> Color32 {
    if frac > 0.6 {
        OK
    } else if frac > 0.3 {
        Color32::from_rgb(255, 193, 7)
    } else {
        DANGER
    }
}

/// 1, 2, 2.5 or 5 times a power of ten, at least `raw`.
fn nice_step(raw: f32) -> f32 {
    let mag = 10f32.powf(raw.max(1.0).log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag)
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
        self.update_rate();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mob_names_and_steps() {
        assert_eq!(pretty_mob_name("T6_MOB_KEEPER_DRUID_BOSS"), "Keeper Druid Boss");
        assert_eq!(pretty_mob_name("KEEPCRITTER_FOREST_GREEN_HIDE"), "Keepcritter Forest Green Hide");
        assert_eq!(nice_step(16.6), 20.0);
        assert_eq!(nice_step(9.0), 10.0);
        assert_eq!(nice_step(22.0), 25.0);
    }
}
