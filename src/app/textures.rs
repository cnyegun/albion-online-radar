//! Textures: embedded resource icons and map backgrounds loaded from disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureId, TextureOptions, Vec2};

use crate::data::Resource;

// --- Resource icons -------------------------------------------------------------

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/icons.rs"));
}

/// Game icons for resources and mist portals, decoded on first use.
#[derive(Default)]
pub(super) struct Icons(HashMap<&'static str, Option<TextureHandle>>);

impl Icons {
    pub(super) fn get(&mut self, ctx: &egui::Context, name: &str) -> Option<TextureId> {
        let &(key, bytes) = embedded::ICONS.iter().find(|(n, _)| *n == name)?;
        self.0
            .entry(key)
            .or_insert_with(|| {
                let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
                let img =
                    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                Some(ctx.load_texture(key, img, TextureOptions::LINEAR))
            })
            .as_ref()
            .map(TextureHandle::id)
    }

    pub(super) fn resource(&mut self, ctx: &egui::Context, r: Resource, tier: u8, enchant: u8) -> Option<TextureId> {
        self.get(ctx, &resource_icon(r, tier, enchant)?)
    }
}

fn has_icon(name: &str) -> bool {
    embedded::ICONS.iter().any(|(n, _)| *n == name)
}

/// Icon name for exactly this tier and enchant, else the unenchanted icon of that tier.
fn resource_icon(r: Resource, tier: u8, enchant: u8) -> Option<String> {
    let kind = match r {
        Resource::Wood => "log",
        Resource::Rock => "rock",
        Resource::Fiber => "fiber",
        Resource::Hide => "hide",
        Resource::Ore => "ore",
    };
    [enchant, 0].into_iter().map(|e| format!("{kind}_{tier}_{e}")).find(|n| has_icon(n))
}

pub(super) fn draw_icon(painter: &egui::Painter, tex: TextureId, at: Pos2, size: f32) {
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(tex, Rect::from_center_size(at, Vec2::splat(size)), uv, Color32::WHITE);
}

// --- Map backgrounds ----------------------------------------------------------

pub(super) enum MapTex {
    Loading,
    Missing,
    Ready(TextureHandle),
}

pub(super) struct MapCache {
    dir: Option<PathBuf>,
    textures: HashMap<String, MapTex>,
    tx: Sender<(String, Option<ColorImage>)>,
    rx: Receiver<(String, Option<ColorImage>)>,
}

impl MapCache {
    pub(super) fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { dir, textures: HashMap::new(), tx, rx }
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        for (id, img) in self.rx.try_iter() {
            let tex = match img {
                Some(img) => MapTex::Ready(ctx.load_texture(format!("map-{id}"), img, TextureOptions::LINEAR)),
                None => MapTex::Missing,
            };
            self.textures.insert(id, tex);
        }
    }

    /// Returns the texture once decoded; starts a background load on first use.
    pub(super) fn get(&mut self, ctx: &egui::Context, id: &str) -> Option<&TextureHandle> {
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
                        // A transparent 1px border: sampling past the map's edge
                        // (clamped) then yields nothing instead of smeared edge pixels.
                        let rgba = img.to_rgba8();
                        let mut padded = image::RgbaImage::new(rgba.width() + 2, rgba.height() + 2);
                        image::imageops::replace(&mut padded, &rgba, 1, 1);
                        ColorImage::from_rgba_unmultiplied(
                            [padded.width() as usize, padded.height() as usize],
                            padded.as_raw(),
                        )
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

/// Texture coordinate of a point given as a 0..1 fraction of the map, skipping
/// the transparent border added at load time.
pub(super) fn map_uv(tex: &TextureHandle, u: f32, v: f32) -> Pos2 {
    let [w, h] = tex.size().map(|n| n as f32);
    Pos2::new((1.0 + u * (w - 2.0)) / w, (1.0 + v * (h - 2.0)) / h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tier_4_to_8_has_an_icon() {
        for r in Resource::ALL {
            for tier in 4..=8 {
                for enchant in 0..=4 {
                    assert!(resource_icon(r, tier, enchant).is_some(), "{r:?} T{tier}.{enchant}");
                }
            }
        }
        assert_eq!(resource_icon(Resource::Rock, 6, 4).as_deref(), Some("rock_6_0"));
        assert_eq!(resource_icon(Resource::Wood, 6, 2).as_deref(), Some("log_6_2"));
        assert_eq!(resource_icon(Resource::Ore, 1, 0), None);
        assert!((0..=4).all(|e| has_icon(&format!("mist_{e}"))));
    }

    #[test]
    fn icons_decode() {
        for (name, bytes) in embedded::ICONS {
            assert!(image::load_from_memory(bytes).is_ok(), "{name}");
        }
    }
}
