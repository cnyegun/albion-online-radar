//! README screenshots, rendered off-screen from staged scenes. No window opens:
//! egui runs headless and its meshes are rasterized in software.
//!
//!     cargo test --release -- --ignored readme_screenshots

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, TextureId, Vec2, epaint};

use super::snapshot::{Item, Key, Marker, Snapshot};
use super::textures::{Icons, MapCache};
use super::{RadarApp, Tab, style};
use crate::data::{GameData, MobClass, PvpType, Resource};
use crate::game::{Game, Threat, ZoneView};
use crate::runtime::{Shared, Source, lock};
use crate::settings::Settings;

const PPP: f32 = 1.5;

// --- Staged scenes ----------------------------------------------------------------

struct Scene {
    zone: &'static str,
    local: [f32; 2],
    items: Vec<Item>,
    threats: usize,
    banner: Option<Threat>,
    /// Point at the first item so its tooltip shows.
    hover: bool,
    settings_tab: Option<Tab>,
}

fn res(id: i64, pos: [f32; 2], resource: Resource, tier: u8, enchant: u8, size: i64) -> Item {
    Item {
        key: Key::Res(id),
        pos,
        marker: Marker::Resource { resource, tier, enchant, size, living: false },
        label: format!("{} T{tier}.{enchant}  ·  {size} left", resource.name()),
    }
}

fn mob(id: i64, pos: [f32; 2], class: MobClass, tier: u8, health: u8, name: &str) -> Item {
    Item { key: Key::Mob(id), pos, marker: Marker::Mob { class: Some(class), tier, health }, label: name.into() }
}

/// A black-zone gathering spot: enchanted nodes of every kind around you.
fn gathering_items(local: [f32; 2]) -> Vec<Item> {
    use Resource::*;
    let at = |dx: f32, dy: f32| [local[0] + dx, local[1] + dy];
    let mut items = vec![
        res(1, at(9.0, -4.0), Ore, 8, 3, 6),
        res(2, at(6.0, -12.0), Ore, 7, 2, 4),
        res(3, at(17.0, -6.0), Ore, 6, 4, 5),
        res(4, at(-11.0, 6.0), Fiber, 8, 2, 3),
        res(5, at(-14.0, 12.0), Fiber, 7, 4, 4),
        res(6, at(-7.0, 15.0), Fiber, 6, 3, 6),
        res(7, at(-20.0, -10.0), Wood, 8, 1, 5),
        res(8, at(-25.0, -4.0), Wood, 7, 3, 6),
        res(9, at(-18.0, -18.0), Wood, 6, 2, 4),
        res(10, at(4.0, 22.0), Rock, 8, 3, 3),
        res(11, at(10.0, 27.0), Rock, 7, 1, 5),
        res(12, at(22.0, 4.0), Hide, 8, 4, 2),
        res(13, at(28.0, -16.0), Ore, 5, 4, 7),
        res(14, at(-30.0, 20.0), Fiber, 5, 3, 6),
        res(15, at(2.0, -26.0), Wood, 7, 4, 3),
        res(16, at(-4.0, -31.0), Rock, 6, 3, 5),
        res(17, at(33.0, 4.0), Hide, 7, 2, 4),
        res(18, at(-34.0, -24.0), Ore, 8, 1, 4),
    ];
    items.push(Item {
        key: Key::Mist(30),
        pos: at(22.0, -28.0),
        marker: Marker::Mist { solo: true, enchant: 3 },
        label: "Solo mist portal .3".into(),
    });
    items.extend([
        mob(40, at(-2.0, 34.0), MobClass::Boss, 8, 255, "Keeper Druid Boss T8"),
        mob(41, at(30.0, 24.0), MobClass::Enchanted, 7, 200, "Keeper Earthmother T7"),
        mob(42, at(26.0, 30.0), MobClass::Normal, 7, 255, "Keeper Ripper T7"),
        mob(43, at(-26.0, 30.0), MobClass::Normal, 7, 140, "Keeper Healer T7"),
    ]);
    items
}

fn scenes() -> Vec<(&'static str, [f32; 2], Scene)> {
    let local = [40.0, -60.0];
    let size = [900.0, 560.0];
    let gather = |hover, threats, banner, settings_tab| Scene {
        zone: "1359",
        local,
        items: gathering_items(local),
        threats,
        banner,
        hover,
        settings_tab,
    };
    vec![
        ("radar", size, gather(true, 0, None, None)),
        (
            "alert",
            size,
            gather(
                false,
                3,
                Some(Threat {
                    name: "Vexmoor".into(),
                    guild: "Black Lantern".into(),
                    alliance: String::new(),
                    item_power: Some(1412),
                    hostile: true,
                }),
                None,
            ),
        ),
        ("settings", [1000.0, 680.0], gather(false, 0, None, Some(Tab::Resources))),
    ]
}

// --- Headless egui -------------------------------------------------------------------

fn app(settings: Settings) -> RadarApp {
    let data = Arc::new(GameData::load().unwrap());
    let shared = Shared::new(Game::new(data), settings.clone(), Source::Replay);
    RadarApp {
        ignore_text: String::new(),
        saved: settings.clone(),
        settings,
        shared,
        dirty_since: None,
        smooth: HashMap::new(),
        local_smooth: None,
        maps: MapCache::new(Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/maps")))),
        icons: Icons::default(),
        show_settings: false,
        tab: Tab::Resources,
        net_selection: HashSet::new(),
        net_message: None,
        last_frame: Instant::now(),
        last_prune: Instant::now(),
        applied_click_through: None,
        frames: 0,
    }
}

fn render(scene: &Scene, size: [f32; 2]) -> image::RgbImage {
    let mut settings = Settings { range: 40.0, show_stack_count: true, icon_scale: 1.15, ..Settings::default() };
    for f in settings.static_resources.iter_mut().chain(&mut settings.living_resources) {
        f.min_tier = 6;
    }
    // Some non-default filters for the settings shot; the staged scenes all pass them.
    settings.static_resources[4].min_enchant = 1;
    settings.living_resources[3].min_tier = 7;
    let mut app = app(settings);
    app.show_settings = scene.settings_tab.is_some();
    app.tab = scene.settings_tab.unwrap_or(Tab::Resources);
    if let Some(threat) = scene.banner.clone() {
        lock(&app.shared.alert).banner = Some((threat, Instant::now()));
    }

    let data = GameData::load().unwrap();
    let zone = data.zone(scene.zone).expect("zone");
    let snap = Snapshot {
        zone: Some(ZoneView { id: scene.zone.into(), name: zone.name.clone(), pvp: PvpType::Black, tier: zone.tier }),
        local: scene.local,
        bounds: zone.bounds,
        items: scene.items.iter().map(clone_item).collect(),
        threats: scene.threats,
    };

    let ctx = egui::Context::default();
    style(&ctx);
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::from(size));
    let mut textures = Textures::default();
    let hover = scene.hover.then(|| hover_target(&app, &snap, screen)).flatten();
    let mut last = None;
    for frame in 0..40 {
        let mut input =
            egui::RawInput { screen_rect: Some(screen), time: Some(frame as f64 / 60.0), ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(PPP);
        if let Some(p) = hover {
            input.events.push(egui::Event::PointerMoved(p));
        }
        let out = ctx.run_ui(input, |ui| {
            app.maps.poll(ui.ctx());
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| app.radar(ui, &snap, 1.0 / 60.0));
            app.settings_window(ui.ctx());
        });
        textures.apply(&out.textures_delta);
        last = Some((ctx.tessellate(out.shapes, out.pixels_per_point), out.pixels_per_point));
        std::thread::sleep(Duration::from_millis(15));
    }
    let (prims, ppp) = last.unwrap();
    rasterize(&prims, &textures, size, ppp)
}

/// Screen position of the first resource, where the tooltip should appear.
fn hover_target(app: &RadarApp, snap: &Snapshot, rect: Rect) -> Option<Pos2> {
    let item = snap.items.first()?;
    let radius = (rect.width().min(rect.height()) / 2.0 - 16.0).max(40.0);
    let k = radius / app.settings.range * std::f32::consts::FRAC_1_SQRT_2;
    let (dx, dy) = (item.pos[0] - snap.local[0], item.pos[1] - snap.local[1]);
    Some(rect.center() + Vec2::new(k * (dx + dy), k * (dx - dy)))
}

fn clone_item(i: &Item) -> Item {
    let marker = match i.marker {
        Marker::Resource { resource, tier, enchant, size, living } => {
            Marker::Resource { resource, tier, enchant, size, living }
        }
        Marker::Mob { class, tier, health } => Marker::Mob { class, tier, health },
        Marker::Mist { solo, enchant } => Marker::Mist { solo, enchant },
    };
    Item { key: i.key, pos: i.pos, marker, label: i.label.clone() }
}

// --- Software rasterizer ---------------------------------------------------------

struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

#[derive(Default)]
struct Textures(HashMap<TextureId, Texture>);

impl Textures {
    fn apply(&mut self, delta: &epaint::textures::TexturesDelta) {
        for (id, d) in delta.set.iter().flat_map(|(id, ds)| ds.iter().map(move |d| (id, d))) {
            let epaint::ImageData::Color(img) = &d.image;
            match d.pos {
                None => {
                    self.0.insert(*id, Texture { size: img.size, pixels: img.pixels.clone() });
                }
                Some([x0, y0]) => {
                    let tex = self.0.get_mut(id).expect("patch to a known texture");
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            tex.pixels[(y0 + y) * tex.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.0.remove(id);
        }
    }
}

impl Texture {
    /// Bilinear, clamped; premultiplied RGBA in 0..1.
    fn sample(&self, uv: Pos2) -> [f32; 4] {
        let [w, h] = self.size;
        let x = (uv.x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (uv.y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let px = |x: usize, y: usize| self.pixels[y * w + x].to_array().map(|c| f32::from(c) / 255.0);
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

fn rasterize(prims: &[epaint::ClippedPrimitive], textures: &Textures, size: [f32; 2], ppp: f32) -> image::RgbImage {
    let (w, h) = ((size[0] * ppp).round() as usize, (size[1] * ppp).round() as usize);
    // Premultiplied RGBA in gamma space, the way egui blends.
    let mut buf = vec![[0.0f32; 4]; w * h];
    for p in prims {
        let epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
        let Some(tex) = textures.0.get(&mesh.texture_id) else { continue };
        let clip = [
            (p.clip_rect.min.x * ppp).max(0.0) as usize,
            (p.clip_rect.min.y * ppp).max(0.0) as usize,
            ((p.clip_rect.max.x * ppp).ceil() as usize).min(w),
            ((p.clip_rect.max.y * ppp).ceil() as usize).min(h),
        ];
        for tri in mesh.indices.as_chunks::<3>().0 {
            let mut v: [&epaint::Vertex; 3] = std::array::from_fn(|i| &mesh.vertices[tri[i] as usize]);
            if edge(v[0].pos, v[1].pos, v[2].pos) < 0.0 {
                v.swap(1, 2);
            }
            let p = v.map(|v| v.pos * ppp);
            let area = edge(p[0], p[1], p[2]);
            if area < 1e-6 {
                continue;
            }
            // Top-left rule: a pixel on an edge shared by two triangles is drawn once.
            let owns = |a: Pos2, b: Pos2| b.y > a.y || (b.y == a.y && b.x < a.x);
            let own = [owns(p[1], p[2]), owns(p[2], p[0]), owns(p[0], p[1])];
            let x_lo = (p.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(clip[0]);
            let y_lo = (p.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(clip[1]);
            let x_hi = (p.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(clip[2]);
            let y_hi = (p.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(clip[3]);
            let colors = v.map(|v| v.color.to_array().map(|c| f32::from(c) / 255.0));
            for y in y_lo..y_hi {
                for x in x_lo..x_hi {
                    let q = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let b = [edge(p[1], p[2], q) / area, edge(p[2], p[0], q) / area, edge(p[0], p[1], q) / area];
                    if (0..3).any(|i| b[i] < 0.0 || (b[i] == 0.0 && !own[i])) {
                        continue;
                    }
                    let uv = Pos2::new(
                        b[0] * v[0].uv.x + b[1] * v[1].uv.x + b[2] * v[2].uv.x,
                        b[0] * v[0].uv.y + b[1] * v[1].uv.y + b[2] * v[2].uv.y,
                    );
                    let t = tex.sample(uv);
                    let src: [f32; 4] = std::array::from_fn(|i| {
                        (b[0] * colors[0][i] + b[1] * colors[1][i] + b[2] * colors[2][i]) * t[i]
                    });
                    let dst = &mut buf[y * w + x];
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    // The real window is see-through; put it on a dark backdrop.
    let backdrop = [16.0 / 255.0, 18.0 / 255.0, 22.0 / 255.0];
    image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let px = buf[y as usize * w + x as usize];
        image::Rgb(std::array::from_fn(|i| {
            ((px[i] + backdrop[i] * (1.0 - px[3])).clamp(0.0, 1.0) * 255.0).round() as u8
        }))
    })
}

fn edge(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[test]
#[ignore = "writes docs/images; run on demand"]
fn readme_screenshots() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
    std::fs::create_dir_all(&out).unwrap();
    for (name, size, scene) in scenes() {
        render(&scene, size).save(out.join(format!("{name}.png"))).unwrap();
    }
}
