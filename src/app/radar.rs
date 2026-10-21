//! The overlay itself: map, range rings, markers and alerts.

use std::collections::HashSet;
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureId, Vec2, epaint,
    epaint::Vertex,
};

use super::snapshot::{Item, Key, Marker, Snapshot};
use super::textures::{draw_icon, map_uv};
use super::{BG, DANGER, OK, RING, RadarApp, TEXT_DIM};
use crate::data::{MobClass, Resource};
use crate::runtime::{Source, lock};

pub(super) fn resource_color(r: Resource) -> Color32 {
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

impl RadarApp {
    pub(super) fn radar(&mut self, ui: &mut egui::Ui, snap: &Snapshot, dt: f32) {
        let ctx = ui.ctx().clone();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
        let rect = resp.rect;
        let fade = self.settings.edge_fade;
        let bg_alpha = self.settings.background_opacity.clamp(0.0, 1.0);
        if bg_alpha > 0.0 {
            painter.add(faded_mesh(rect, fade, TextureId::default(), |_, f| {
                (
                    epaint::WHITE_UV,
                    Color32::from_rgba_unmultiplied(BG.r(), BG.g(), BG.b(), (bg_alpha * f * 255.0) as u8),
                )
            }));
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

        // Map background, drawn over the window (not the zone) so it can fade at the edges.
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
            let alpha = self.settings.map_opacity.clamp(0.0, 1.0);
            // Inverse of `to_screen`; affine, so per-vertex texture coordinates are exact.
            let to_world = |p: Pos2| -> [f32; 2] {
                let (a, b) = ((p.x - center.x) / k, (p.y - center.y) / k);
                [local[0] + (a + b) / 2.0, local[1] + (a - b) / 2.0]
            };
            painter.add(faded_mesh(rect, fade, tex.id(), |p, f| {
                let w = to_world(p);
                let uv = map_uv(tex, (w[0] - x0) / e, (w[1] - y0) / e);
                (uv, Color32::from_rgba_unmultiplied(shade, shade, shade, (alpha * f * 255.0) as u8))
            }));
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

    /// The only status ever shown: a red note when nothing is being captured.
    fn capture_warning(&self, painter: &egui::Painter, rect: Rect) {
        let Source::Live { manager, .. } = &self.shared.source else { return };
        if !manager.state().active.is_empty() {
            return;
        }
        let at = rect.left_top() + Vec2::new(14.0, 14.0);
        painter.circle_filled(at, 4.0, DANGER);
        painter.text(
            at + Vec2::new(10.0, 0.0),
            Align2::LEFT_CENTER,
            "Not capturing — F1 → Network",
            FontId::proportional(12.0),
            DANGER,
        );
    }

    fn draw_marker(&mut self, painter: &egui::Painter, at: Pos2, marker: &Marker, scale: f32) {
        match *marker {
            Marker::Resource { resource, tier, enchant, size, living } => {
                let s = 9.0 * scale;
                if let Some(tex) = self.icons.resource(painter.ctx(), resource, tier, enchant) {
                    let s = 13.0 * scale;
                    draw_icon(painter, tex, at, s * 2.0);
                    if self.settings.show_stack_count && size > 0 {
                        let pos = at + Vec2::new(s - 2.0, s - 2.0);
                        painter.text(
                            pos,
                            Align2::RIGHT_BOTTOM,
                            size.to_string(),
                            FontId::proportional(9.5 * scale),
                            Color32::WHITE,
                        );
                    }
                    return;
                }
                let fill = resource_color(resource);
                let border = enchant_color(enchant)
                    .map_or(Stroke::new(1.0, Color32::from_black_alpha(160)), |c| Stroke::new(2.0 * scale, c));
                if living {
                    painter.circle_filled(at, s + 1.0, fill);
                    painter.circle_stroke(at, s + 1.0, border);
                } else {
                    let r = Rect::from_center_size(at, Vec2::splat(s * 2.0));
                    painter.rect_filled(r, 4.0 * scale, fill);
                    painter.rect_stroke(r, 4.0 * scale, border, StrokeKind::Outside);
                }
                painter.text(
                    at,
                    Align2::CENTER_CENTER,
                    tier.to_string(),
                    FontId::proportional(11.0 * scale),
                    Color32::WHITE,
                );
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
                    painter.rect_filled(
                        Rect::from_min_size(bar.min, Vec2::new(14.0 * frac, 2.0)),
                        1.0,
                        health_color(frac),
                    );
                }
                if self.settings.mob_tiers && tier > 0 {
                    painter.text(
                        at + Vec2::new(r + 3.0, 0.0),
                        Align2::LEFT_CENTER,
                        format!("T{tier}"),
                        FontId::proportional(9.5),
                        TEXT_DIM,
                    );
                }
            }
            Marker::Mist { solo, enchant } => {
                if let Some(tex) = self.icons.get(painter.ctx(), &format!("mist_{enchant}")) {
                    draw_icon(painter, tex, at, 22.0 * scale);
                    return;
                }
                let color = if solo { Color32::from_rgb(64, 224, 208) } else { Color32::from_rgb(199, 125, 255) };
                let r = 8.0 * scale;
                painter.add(Shape::convex_polygon(diamond(at, r), color.gamma_multiply(0.35), Stroke::new(2.0, color)));
                if enchant > 0 {
                    painter.text(
                        at,
                        Align2::CENTER_CENTER,
                        enchant.to_string(),
                        FontId::proportional(10.0 * scale),
                        Color32::WHITE,
                    );
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
                painter.rect_stroke(
                    rect.shrink(1.5),
                    8.0,
                    Stroke::new(3.0, DANGER.gamma_multiply(0.4 + 0.6 * pulse)),
                    StrokeKind::Inside,
                );
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
                let galley =
                    painter.layout_no_wrap(format!("{title}    {who}"), FontId::proportional(15.0), Color32::WHITE);
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
                let layer =
                    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("threat-flash")));
                layer.rect_filled(ctx.content_rect(), 0.0, DANGER.gamma_multiply(0.45 * (1.0 - age / 0.45)));
            }
        }
        if animating {
            ctx.request_repaint();
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

/// A mesh covering `rect` whose vertices are denser near the edges, where `vertex`
/// gets an opacity factor `f` rising smoothly from 0 at the edge to 1 at `fade` px in.
fn faded_mesh(rect: Rect, fade: f32, tex: TextureId, vertex: impl Fn(Pos2, f32) -> (Pos2, Color32)) -> Shape {
    const STEPS: usize = 8;
    let stops = |lo: f32, hi: f32| -> Vec<f32> {
        let f = fade.min((hi - lo) / 2.0).max(0.0);
        let mut v: Vec<f32> = (0..=STEPS).map(|i| lo + f * i as f32 / STEPS as f32).collect();
        v.extend((0..=STEPS).rev().map(|i| hi - f * i as f32 / STEPS as f32));
        v.dedup();
        v
    };
    let edge = |d: f32| if fade <= 0.0 { 1.0 } else { smoothstep((d / fade).clamp(0.0, 1.0)) };
    let (xs, ys) = (stops(rect.left(), rect.right()), stops(rect.top(), rect.bottom()));
    let mut mesh = Mesh::with_texture(tex);
    for &y in &ys {
        for &x in &xs {
            let f = edge((x - rect.left()).min(rect.right() - x)) * edge((y - rect.top()).min(rect.bottom() - y));
            let pos = Pos2::new(x, y);
            let (uv, color) = vertex(pos, f);
            mesh.vertices.push(Vertex { pos, uv, color });
        }
    }
    let n = xs.len() as u32;
    for row in 0..ys.len() as u32 - 1 {
        for col in 0..n - 1 {
            let i = row * n + col;
            mesh.add_triangle(i, i + 1, i + n + 1);
            mesh.add_triangle(i, i + n + 1, i + n);
        }
    }
    Shape::mesh(mesh)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// 1, 2, 2.5 or 5 times a power of ten, at least `raw`.
fn nice_step(raw: f32) -> f32 {
    let mag = 10f32.powf(raw.max(1.0).log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_steps() {
        assert_eq!(nice_step(16.6), 20.0);
        assert_eq!(nice_step(9.0), 10.0);
        assert_eq!(nice_step(22.0), 25.0);
    }
}
