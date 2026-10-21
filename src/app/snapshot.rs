//! What to draw this frame, copied out of the game state under its lock.

use crate::data::{Bounds, MobClass, Resource};
use crate::game::{Game, ZoneView};
use crate::settings::Settings;

// --- Per-frame snapshot ---------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Key {
    Res(i64),
    Mob(i64),
    Mist(i64),
}

pub(super) enum Marker {
    Resource { resource: Resource, tier: u8, enchant: u8, size: i64, living: bool },
    Mob { class: Option<MobClass>, tier: u8, health: u8 },
    Mist { solo: bool, enchant: u8 },
}

pub(super) struct Item {
    pub(super) key: Key,
    pub(super) pos: [f32; 2],
    pub(super) marker: Marker,
    pub(super) label: String,
}

pub(super) struct Snapshot {
    pub(super) zone: Option<ZoneView>,
    pub(super) local: [f32; 2],
    pub(super) bounds: Option<Bounds>,
    pub(super) items: Vec<Item>,
}

pub(super) fn snapshot(game: &Game, s: &Settings) -> Snapshot {
    let zone = game.zone();
    let mut items = Vec::new();

    for (id, h) in &game.harvestables {
        if h.size <= 0 || !s.resource_filter(h.resource, false).accepts(h.tier, h.enchant) {
            continue;
        }
        items.push(Item {
            key: Key::Res(*id),
            pos: h.pos,
            marker: Marker::Resource {
                resource: h.resource,
                tier: h.tier,
                enchant: h.enchant,
                size: h.size,
                living: false,
            },
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
pub(super) fn pretty_mob_name(unique: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mob_names() {
        assert_eq!(pretty_mob_name("T6_MOB_KEEPER_DRUID_BOSS"), "Keeper Druid Boss");
        assert_eq!(pretty_mob_name("KEEPCRITTER_FOREST_GREEN_HIDE"), "Keepcritter Forest Green Hide");
    }
}
