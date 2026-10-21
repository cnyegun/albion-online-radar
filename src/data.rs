//! Static game data (zones, mobs, items), embedded from `data/` (refresh with `cargo run --bin update-data`).

use std::collections::HashMap;

use serde::Deserialize;

/// Server mob type ids are the mobs.json index plus this offset.
/// Re-verify after each data refresh (upstream: tools/offset-validate).
const MOB_TYPE_OFFSET: i64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resource {
    Wood,
    Rock,
    Fiber,
    Hide,
    Ore,
}

impl Resource {
    pub const ALL: [Resource; 5] = [Resource::Wood, Resource::Rock, Resource::Fiber, Resource::Hide, Resource::Ore];

    /// Static harvestable type number (0-27) to resource.
    pub fn from_type_number(n: i64) -> Option<Self> {
        match n {
            0..=5 => Some(Resource::Wood),
            6..=10 => Some(Resource::Rock),
            11..=15 => Some(Resource::Fiber),
            16..=22 => Some(Resource::Hide),
            23..=27 => Some(Resource::Ore),
            _ => None,
        }
    }

    /// Loot type from mobs.json (`HIDE_CRITTER`, `WOOD_GUARDIAN`, ...).
    fn from_loot(loot: &str) -> Option<Self> {
        let l = loot.to_ascii_uppercase();
        if l.starts_with("SILVERCOINS") || l.starts_with("DEADRAT") {
            None
        } else if l.starts_with("HIDE") || l.starts_with("LEATHER") {
            Some(Resource::Hide)
        } else if l.starts_with("FIBER") {
            Some(Resource::Fiber)
        } else if l.starts_with("WOOD") {
            Some(Resource::Wood)
        } else if l.starts_with("ROCK") || l.starts_with("STONE") {
            Some(Resource::Rock)
        } else if l.starts_with("ORE") {
            Some(Resource::Ore)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Resource::Wood => "Wood",
            Resource::Rock => "Rock",
            Resource::Fiber => "Fiber",
            Resource::Hide => "Hide",
            Resource::Ore => "Ore",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PvpType {
    Safe,
    Yellow,
    Red,
    Black,
}

impl PvpType {
    fn parse(s: &str) -> Self {
        match s {
            "yellow" => PvpType::Yellow,
            "red" => PvpType::Red,
            "black" => PvpType::Black,
            _ => PvpType::Safe,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct Zone {
    pub name: String,
    pub pvp: PvpType,
    pub tier: u8,
    pub bounds: Option<Bounds>,
}

#[derive(Deserialize)]
struct RawZone {
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(rename = "pvpType", default)]
    pvp: String,
    #[serde(default)]
    tier: u8,
    #[serde(default)]
    bounds: Option<RawBounds>,
}

#[derive(Deserialize)]
struct RawBounds {
    min: [f32; 2],
    max: [f32; 2],
}

/// How a mob is drawn and filtered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MobClass {
    /// Animal or creature that drops a gatherable resource.
    Living(Resource),
    Normal,
    Enchanted,
    MiniBoss,
    Boss,
    MistBoss,
}

#[derive(Debug, Clone)]
pub struct MobInfo {
    pub unique_name: String,
    pub tier: u8,
    pub class: MobClass,
}

#[derive(Deserialize)]
struct RawMob {
    #[serde(default)]
    u: String,
    #[serde(default)]
    t: u8,
    #[serde(default)]
    c: String,
    #[serde(default)]
    l: Option<String>,
}

#[derive(Deserialize)]
struct RawItem {
    #[serde(default)]
    p: u32,
}

const MIST_BOSSES: [&str; 4] = ["_MOB_MISTS_FAIRYDRAGON", "_MOB_MISTS_GRIFFIN", "_MOB_MISTS_SPIDER", "_MOB_ARCANE_CRYSTALSPIDER"];

fn classify_hostile(category: &str, unique_name: &str) -> MobClass {
    let name = unique_name.to_ascii_uppercase();
    if MIST_BOSSES.iter().any(|b| name.contains(b)) {
        return MobClass::MistBoss;
    }
    if (name.contains("_VETERAN") && !name.contains("_VETERAN_CHAMPION")) || name.contains("_ELITE") {
        return MobClass::MiniBoss;
    }
    if name.contains("_BOSS") && !name.contains("MINIBOSS") {
        return MobClass::Boss;
    }
    match category.to_ascii_lowercase().as_str() {
        "boss" => MobClass::Boss,
        "miniboss" | "rd_elite" | "rd_veteran" => MobClass::MiniBoss,
        "champion" | "rd_solo" => MobClass::Enchanted,
        _ => MobClass::Normal,
    }
}

pub struct GameData {
    zones: HashMap<String, Zone>,
    mobs: Vec<MobInfo>,
    item_power: Vec<u32>,
}

impl GameData {
    pub fn load() -> anyhow::Result<Self> {
        let raw_zones: HashMap<String, RawZone> =
            serde_json::from_slice(include_bytes!("../data/zones.json"))?;
        let zones = raw_zones
            .into_iter()
            .map(|(id, z)| {
                // Roads of Avalon are full-loot regardless of what the dump says.
                let pvp = if z.kind.starts_with("TUNNEL_") { PvpType::Black } else { PvpType::parse(&z.pvp) };
                let bounds = z.bounds.map(|b| Bounds { min: b.min, max: b.max });
                (id, Zone { name: z.name, pvp, tier: z.tier, bounds })
            })
            .collect();

        let raw_mobs: Vec<RawMob> = serde_json::from_slice(include_bytes!("../data/mobs.min.json"))?;
        let mobs = raw_mobs
            .into_iter()
            .map(|m| {
                let class = match m.l.as_deref().and_then(Resource::from_loot) {
                    Some(r) => MobClass::Living(r),
                    None => classify_hostile(&m.c, &m.u),
                };
                MobInfo { unique_name: m.u, tier: m.t, class }
            })
            .collect();

        let raw_items: Vec<Option<RawItem>> =
            serde_json::from_slice(include_bytes!("../data/items.min.json"))?;
        let item_power = raw_items.into_iter().map(|i| i.map_or(0, |i| i.p)).collect();

        Ok(Self { zones, mobs, item_power })
    }

    pub fn mob(&self, type_id: i64) -> Option<&MobInfo> {
        usize::try_from(type_id - MOB_TYPE_OFFSET).ok().and_then(|i| self.mobs.get(i))
    }

    /// Exact id first, then the numeric base of compound ids like `1234-5`.
    pub fn zone(&self, id: &str) -> Option<&Zone> {
        self.zones.get(id).or_else(|| self.zones.get(id.split('-').next().unwrap_or(id)))
    }

    #[cfg(test)]
    pub fn zone_ids(&self) -> impl Iterator<Item = &String> {
        self.zones.keys()
    }

    pub fn item_power(&self, item_id: i64) -> u32 {
        usize::try_from(item_id).ok().and_then(|i| self.item_power.get(i)).copied().unwrap_or(0)
    }

    /// Average item power over weapon, armor pieces and potion slots.
    pub fn average_item_power(&self, equipment: &[i64]) -> Option<u32> {
        let powers: Vec<u32> = equipment
            .iter()
            .enumerate()
            .filter(|(slot, id)| (*slot <= 4 || *slot == 8) && **id > 0)
            .map(|(_, id)| self.item_power(*id))
            .filter(|p| *p > 0)
            .collect();
        (!powers.is_empty()).then(|| (powers.iter().sum::<u32>() as f32 / powers.len() as f32).round() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_parses() {
        let d = GameData::load().unwrap();
        let lymhurst = d.zone("1000").unwrap();
        assert_eq!(lymhurst.name, "Lymhurst");
        assert_eq!(lymhurst.pvp, PvpType::Safe);
        assert!(d.zone("1000-3").is_some(), "compound ids fall back to the base id");
        assert!(d.mobs.iter().any(|m| matches!(m.class, MobClass::Living(Resource::Hide))));
        assert!(d.mobs.iter().any(|m| m.class == MobClass::Boss));
        assert!(d.item_power(2) > 0);
    }

    #[test]
    fn hostile_classification() {
        assert_eq!(classify_hostile("static", "T6_MOB_MORGANA_CROSSBOWMAN_VETERAN"), MobClass::MiniBoss);
        assert_eq!(classify_hostile("champion", "T6_X_VETERAN_CHAMPION"), MobClass::Enchanted);
        assert_eq!(classify_hostile("", "T8_MOB_KEEPER_BOSS"), MobClass::Boss);
        assert_eq!(classify_hostile("", "T8_MOB_MISTS_GRIFFIN"), MobClass::MistBoss);
        assert_eq!(classify_hostile("roaming", "T4_MOB_X"), MobClass::Normal);
    }
}
