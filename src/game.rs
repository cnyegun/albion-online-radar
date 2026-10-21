//! World state rebuilt from decoded Photon messages: where you are, which
//! zone, and every resource, mob, mist portal and player the server told us about.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openradar::photon::{EventData, OperationRequest, OperationResponse, Params, Value};

use crate::data::{GameData, MobClass, PvpType, Resource};

mod ev {
    pub const LEAVE: i64 = 1;
    pub const MOVE: i64 = 3;
    pub const HEALTH_UPDATE: i64 = 6;
    pub const HEALTH_UPDATES: i64 = 7;
    pub const NEW_CHARACTER: i64 = 29;
    pub const NEW_SIMPLE_HARVESTABLE_LIST: i64 = 39;
    pub const NEW_HARVESTABLE: i64 = 40;
    pub const HARVESTABLE_CHANGE_STATE: i64 = 46;
    pub const MOB_CHANGE_STATE: i64 = 47;
    pub const EQUIPMENT_CHANGED: i64 = 90;
    pub const REGENERATION_HEALTH_CHANGED: i64 = 91;
    pub const NEW_MOB: i64 = 123;
    pub const MOUNTED: i64 = 211;
    pub const CHANGE_FLAGGING_FINISHED: i64 = 365;
    pub const MISTS_PLAYER_JOINED_INFO: i64 = 522;
}

mod op {
    pub const JOIN: i64 = 2;
    pub const MOVE_LEGACY: i64 = 21;
    pub const MOVE: i64 = 22;
    pub const LEGACY_MAP_CHANGE: i64 = 35;
    pub const CHANGE_CLUSTER: i64 = 41;
    pub const MISTS_USE_STATIC_ENTRANCE: i64 = 477;
}

const STALE_AFTER: Duration = Duration::from_secs(300);
const HARVESTABLE_RANGE: f32 = 80.0;
const MAX_PLAYERS: usize = 100;
const MAP_CHANGE_DEBOUNCE: Duration = Duration::from_secs(4);
const MIST_CHOICE_TTL: Duration = Duration::from_secs(30);
const MIST_CHAIN_TTL: Duration = Duration::from_secs(30 * 60);
pub const FACTION_HOSTILE: u8 = 255;

// --- Parameter access -------------------------------------------------------

fn int(p: &Params, k: u8) -> Option<i64> {
    p.get(&k).and_then(Value::as_i64)
}

fn string(p: &Params, k: u8) -> Option<&str> {
    match p.get(&k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(f64::from(*f)),
        Value::Double(d) => Some(*d),
        other => other.as_i64().map(|i| i as f64),
    }
}

/// Any numeric array (byte buffers included) as integers.
fn ints(v: Option<&Value>) -> Option<Vec<i64>> {
    Some(match v? {
        Value::ByteArray(b) => b.iter().map(|x| i64::from(*x)).collect(),
        Value::ShortArray(a) => a.iter().map(|x| i64::from(*x)).collect(),
        Value::IntArray(a) => a.iter().map(|x| i64::from(*x)).collect(),
        Value::LongArray(a) => a.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_i64).collect(),
        _ => return None,
    })
}

fn floats(v: Option<&Value>) -> Option<Vec<f32>> {
    Some(match v? {
        Value::FloatArray(a) => a.clone(),
        Value::DoubleArray(a) => a.iter().map(|x| *x as f32).collect(),
        Value::Array(a) => a.iter().filter_map(num).map(|x| x as f32).collect(),
        // Older builds sent positions as raw little-endian float buffers.
        Value::ByteArray(b) => b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        _ => return None,
    })
}

fn pos(v: Option<&Value>) -> Option<[f32; 2]> {
    let f = floats(v)?;
    (f.len() >= 2 && f[0].is_finite() && f[1].is_finite()).then(|| [f[0], f[1]])
}

// --- Entities ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Harvestable {
    pub resource: Resource,
    pub tier: u8,
    pub enchant: u8,
    /// Remaining stacks; 0 means depleted.
    pub size: i64,
    pub pos: [f32; 2],
    updated: Instant,
}

#[derive(Debug, Clone)]
pub struct Mob {
    pub type_id: i64,
    /// `None` when the type id is not in the mob table.
    pub class: Option<MobClass>,
    pub name: String,
    pub tier: u8,
    pub enchant: u8,
    pub pos: [f32; 2],
    /// Current health, 0-255.
    pub health: u8,
    pub max_hp: i64,
    updated: Instant,
}

#[derive(Debug, Clone)]
pub struct MistPortal {
    pub pos: [f32; 2],
    pub solo: bool,
    pub enchant: u8,
    updated: Instant,
}

#[derive(Debug, Clone)]
pub struct Player {
    pub name: String,
    pub guild: String,
    pub alliance: String,
    /// 0 passive, 1-6 faction city, 255 hostile.
    pub faction: u8,
    pub item_power: Option<u32>,
    updated: Instant,
}

impl Player {
    pub fn is_hostile(&self) -> bool {
        self.faction == FACTION_HOSTILE
    }
}

/// A player the zone rules consider dangerous. Ignore lists are applied by the caller.
#[derive(Debug, Clone)]
pub struct Threat {
    pub name: String,
    pub guild: String,
    pub alliance: String,
    pub item_power: Option<u32>,
    pub hostile: bool,
}

#[derive(Debug, Clone)]
struct MistZone {
    name: String,
    pvp: PvpType,
    origin: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ZoneView {
    pub id: String,
    pub name: String,
    pub pvp: PvpType,
    pub tier: u8,
}

/// Should a player with `faction` alert in a zone of type `pvp`?
pub fn is_threat(faction: u8, pvp: PvpType) -> bool {
    match pvp {
        PvpType::Safe => false,
        PvpType::Black => true,
        PvpType::Yellow | PvpType::Red => faction == FACTION_HOSTILE,
    }
}

fn is_plain_mist(id: &str) -> bool {
    id.starts_with("@MISTS@")
}

fn is_sanctuary(id: &str) -> bool {
    id.starts_with("@MISTSDUNGEON@")
}

pub struct Game {
    data: Arc<GameData>,
    pub local: [f32; 2],
    pub map_id: Option<String>,
    pub harvestables: HashMap<i64, Harvestable>,
    pub mobs: HashMap<i64, Mob>,
    pub mists: HashMap<i64, MistPortal>,
    pub players: HashMap<i64, Player>,
    mist_zones: HashMap<String, MistZone>,
    pending_mist_choice: Option<(Instant, bool)>,
    last_mist: Option<(String, PvpType, Instant)>,
    last_map_change: Option<Instant>,
    threats: Vec<Threat>,
    /// Bumped on every state change so the UI knows when to redraw.
    pub revision: u64,
}

impl Game {
    pub fn new(data: Arc<GameData>) -> Self {
        Self {
            data,
            local: [0.0, 0.0],
            map_id: None,
            harvestables: HashMap::new(),
            mobs: HashMap::new(),
            mists: HashMap::new(),
            players: HashMap::new(),
            mist_zones: HashMap::new(),
            pending_mist_choice: None,
            last_mist: None,
            last_map_change: None,
            threats: Vec::new(),
            revision: 0,
        }
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    /// Threats detected since the last call.
    pub fn take_threats(&mut self) -> Vec<Threat> {
        std::mem::take(&mut self.threats)
    }

    pub fn zone(&self) -> Option<ZoneView> {
        let id = self.map_id.as_ref()?;
        if let Some(m) = self.mist_zones.get(id) {
            return Some(ZoneView { id: id.clone(), name: m.name.clone(), pvp: m.pvp, tier: 0 });
        }
        Some(match self.data.zone(id) {
            Some(z) => ZoneView { id: id.clone(), name: z.name.clone(), pvp: z.pvp, tier: z.tier },
            // Unknown zones count as safe, so they never alert.
            None => ZoneView { id: id.clone(), name: id.clone(), pvp: PvpType::Safe, tier: 0 },
        })
    }

    pub fn clear_entities(&mut self) {
        self.harvestables.clear();
        self.mobs.clear();
        self.mists.clear();
        self.players.clear();
        self.revision += 1;
    }

    /// Drops far-away resources and anything not updated for five minutes.
    pub fn prune(&mut self, now: Instant) {
        let [lx, ly] = self.local;
        let before = self.harvestables.len() + self.mobs.len() + self.mists.len() + self.players.len();
        self.harvestables.retain(|_, h| {
            let (dx, dy) = (h.pos[0] - lx, h.pos[1] - ly);
            now.duration_since(h.updated) < STALE_AFTER && (dx * dx + dy * dy).sqrt() <= HARVESTABLE_RANGE
        });
        self.mobs.retain(|_, m| now.duration_since(m.updated) < STALE_AFTER);
        self.mists.retain(|_, m| now.duration_since(m.updated) < STALE_AFTER);
        self.players.retain(|_, p| now.duration_since(p.updated) < STALE_AFTER);
        if before != self.harvestables.len() + self.mobs.len() + self.mists.len() + self.players.len() {
            self.revision += 1;
        }
    }

    // --- Map changes --------------------------------------------------------

    fn apply_map_change(&mut self, new_id: &str) {
        let now = Instant::now();
        let previous = self.map_id.replace(new_id.to_owned()).unwrap_or_default();
        self.last_map_change = Some(now);

        if is_plain_mist(new_id) {
            let choice = self.pending_mist_choice.take().filter(|(at, _)| now.duration_since(*at) <= MIST_CHOICE_TTL);
            let mut forced = choice.map(|(_, lethal)| if lethal { PvpType::Black } else { PvpType::Yellow });
            let origin = if is_sanctuary(&previous) {
                self.last_mist.clone().filter(|(_, _, at)| now.duration_since(*at) <= MIST_CHAIN_TTL).map(|(o, pvp, _)| {
                    forced = forced.or(Some(pvp));
                    o
                })
            } else if is_plain_mist(&previous) {
                self.mist_zones.get(&previous).cloned().map(|m| {
                    forced = forced.or(Some(m.pvp));
                    m.origin
                })
            } else {
                (!previous.is_empty()).then_some(previous)
            };
            if let Some(origin) = origin
                && let Some(z) = self.data.zone(&origin)
            {
                // Mists entered from a red zone are lethal.
                let inherited = if z.pvp == PvpType::Red { PvpType::Black } else { z.pvp };
                let pvp = forced.unwrap_or(inherited);
                let name = format!("Mist of {}", z.name);
                self.mist_zones.insert(new_id.to_owned(), MistZone { name, pvp, origin: origin.clone() });
                self.last_mist = Some((origin, pvp, now));
            }
        } else if is_sanctuary(new_id) {
            if let Some((origin, pvp, at)) = self.last_mist.clone()
                && now.duration_since(at) <= MIST_CHAIN_TTL
                && let Some(z) = self.data.zone(&origin)
            {
                let name = format!("Knightfall Abbey (Mist of {})", z.name);
                self.mist_zones.insert(new_id.to_owned(), MistZone { name, pvp, origin });
            }
        } else {
            self.mist_zones.clear();
            self.pending_mist_choice = None;
            self.last_mist = None;
        }
        self.revision += 1;
    }

    // --- Dispatch -----------------------------------------------------------

    pub fn on_event(&mut self, e: &EventData) {
        let p = &e.parameters;
        let Some(code) = int(p, 252) else { return };
        let id = int(p, 0).unwrap_or(-1);
        match code {
            ev::LEAVE => {
                let removed = self.players.remove(&id).is_some() | self.mobs.remove(&id).is_some() | self.mists.remove(&id).is_some();
                if removed {
                    self.revision += 1;
                }
            }
            ev::MOVE => {
                let (Some(x), Some(y)) = (p.get(&4).and_then(num), p.get(&5).and_then(num)) else { return };
                let now = Instant::now();
                if let Some(m) = self.mobs.get_mut(&id) {
                    m.pos = [x as f32, y as f32];
                    m.updated = now;
                    self.revision += 1;
                } else if let Some(m) = self.mists.get_mut(&id) {
                    m.pos = [x as f32, y as f32];
                    m.updated = now;
                    self.revision += 1;
                }
            }
            ev::NEW_CHARACTER => self.new_player(id, p),
            ev::NEW_SIMPLE_HARVESTABLE_LIST => self.new_harvestable_batch(p),
            ev::NEW_HARVESTABLE => self.new_harvestable(id, p),
            ev::HARVESTABLE_CHANGE_STATE => self.harvestable_change(p),
            ev::MOB_CHANGE_STATE => {
                if let (Some(m), Some(enchant)) = (self.mobs.get_mut(&id), int(p, 1)) {
                    m.enchant = enchant.clamp(0, 4) as u8;
                    m.updated = Instant::now();
                    self.revision += 1;
                }
            }
            ev::REGENERATION_HEALTH_CHANGED => {
                self.touch_player(id);
                if let (Some(m), Some(c)) = (self.mobs.get_mut(&id), int(p, 2)) {
                    m.health = c.clamp(0, 255) as u8;
                }
                self.revision += 1;
            }
            ev::HEALTH_UPDATE => self.health_update(id, int(p, 3)),
            ev::HEALTH_UPDATES => {
                if let Some(hps) = ints(p.get(&3)) {
                    for hp in hps {
                        self.health_update(id, Some(hp));
                    }
                }
            }
            ev::EQUIPMENT_CHANGED => {
                if let (Some(eq), Some(pl)) = (ints(p.get(&2)).filter(|e| !e.is_empty()), self.players.get_mut(&id)) {
                    pl.item_power = self.data.average_item_power(&eq).or(pl.item_power);
                    pl.updated = Instant::now();
                    self.revision += 1;
                }
            }
            ev::NEW_MOB => self.new_mob(p),
            ev::MOUNTED => self.touch_player(id),
            ev::CHANGE_FLAGGING_FINISHED => {
                let Some(faction) = int(p, 1) else { return };
                let pvp = self.zone().map_or(PvpType::Safe, |z| z.pvp);
                let Some(pl) = self.players.get_mut(&id) else { return };
                let was_hostile = pl.is_hostile();
                pl.faction = faction.clamp(0, 255) as u8;
                pl.updated = Instant::now();
                if !was_hostile && pl.is_hostile() && is_threat(pl.faction, pvp) {
                    let threat = Threat {
                        name: pl.name.clone(),
                        guild: pl.guild.clone(),
                        alliance: pl.alliance.clone(),
                        item_power: pl.item_power,
                        hostile: true,
                    };
                    self.threats.push(threat);
                }
                self.revision += 1;
            }
            ev::MISTS_PLAYER_JOINED_INFO => {
                if matches!(p.get(&3), Some(Value::Bool(true)))
                    && let Some(new_id) = string(p, 2)
                    && self.map_id.as_deref() != Some(new_id)
                {
                    let new_id = new_id.to_owned();
                    self.apply_map_change(&new_id);
                }
            }
            _ => {}
        }
    }

    pub fn on_request(&mut self, r: &OperationRequest) {
        let p = &r.parameters;
        match int(p, 253) {
            Some(op::MOVE | op::MOVE_LEGACY) => {
                if let Some(at) = pos(p.get(&1)) {
                    self.local = at;
                    self.revision += 1;
                }
            }
            Some(op::MISTS_USE_STATIC_ENTRANCE) if int(p, 1) == Some(8) => {
                self.pending_mist_choice = Some((Instant::now(), p.contains_key(&2)));
            }
            _ => {}
        }
    }

    pub fn on_response(&mut self, r: &OperationResponse) {
        let p = &r.parameters;
        match int(p, 253) {
            Some(op::CHANGE_CLUSTER) => {
                if let Some(new_id) = string(p, 0)
                    && self.map_id.as_deref() != Some(new_id)
                {
                    let new_id = new_id.to_owned();
                    self.apply_map_change(&new_id);
                    self.clear_entities();
                }
            }
            Some(op::LEGACY_MAP_CHANGE) => {
                let debounced = self.map_id.is_some() && self.last_map_change.is_some_and(|t| t.elapsed() < MAP_CHANGE_DEBOUNCE);
                if let Some(new_id) = string(p, 0)
                    && !debounced
                    && self.map_id.as_deref() != Some(new_id)
                {
                    let new_id = new_id.to_owned();
                    self.apply_map_change(&new_id);
                }
            }
            Some(op::JOIN) => {
                if let Some(at) = pos(p.get(&9)) {
                    self.local = at;
                }
                if let Some(new_id) = string(p, 8) {
                    let new_id = new_id.to_owned();
                    self.apply_map_change(&new_id);
                }
                self.clear_entities();
            }
            _ => {}
        }
    }

    // --- Handlers -----------------------------------------------------------

    fn new_player(&mut self, id: i64, p: &Params) {
        let now = Instant::now();
        let name = string(p, 1).unwrap_or_default().to_owned();
        let guild = string(p, 8).unwrap_or_default().to_owned();
        let alliance = string(p, 51).unwrap_or_default().to_owned();
        let faction = int(p, 53).unwrap_or(0).clamp(0, 255) as u8;
        let item_power = ints(p.get(&40)).and_then(|eq| self.data.average_item_power(&eq));

        if let Some(pl) = self.players.get_mut(&id) {
            if !name.is_empty() {
                pl.name = name.clone();
            }
            if !guild.is_empty() {
                pl.guild = guild.clone();
            }
            if p.contains_key(&51) {
                pl.alliance = alliance.clone();
            }
            if p.contains_key(&53) {
                pl.faction = faction;
            }
            pl.item_power = item_power.or(pl.item_power);
            pl.updated = now;
        } else if self.players.len() < MAX_PLAYERS {
            self.players.insert(
                id,
                Player {
                    name: name.clone(),
                    guild: guild.clone(),
                    alliance: alliance.clone(),
                    faction,
                    item_power,
                    updated: now,
                },
            );
        }
        self.revision += 1;

        if let Some(zone) = self.zone()
            && is_threat(faction, zone.pvp)
        {
            self.threats.push(Threat { name, guild, alliance, item_power, hostile: faction == FACTION_HOSTILE });
        }
    }

    fn upsert_harvestable(&mut self, id: i64, resource: Resource, tier: i64, enchant: i64, size: i64, at: [f32; 2]) {
        let now = Instant::now();
        let tier = tier.clamp(0, 8) as u8;
        let enchant = enchant.clamp(0, 4) as u8;
        self.harvestables
            .entry(id)
            .and_modify(|h| {
                h.resource = resource;
                h.enchant = enchant;
                h.size = size;
            })
            .or_insert(Harvestable { resource, tier, enchant, size, pos: at, updated: now });
        self.revision += 1;
    }

    /// Event 39: batch spawn of static resources (no enchant info; event 46 follows).
    fn new_harvestable_batch(&mut self, p: &Params) {
        let (Some(ids), Some(types), Some(tiers), Some(xy), Some(sizes)) =
            (ints(p.get(&0)), ints(p.get(&1)), ints(p.get(&2)), floats(p.get(&3)), ints(p.get(&4)))
        else {
            return;
        };
        for (i, id) in ids.iter().enumerate() {
            let (Some(t), Some(tier), Some(size), Some(x), Some(y)) =
                (types.get(i), tiers.get(i), sizes.get(i), xy.get(i * 2), xy.get(i * 2 + 1))
            else {
                break;
            };
            if let Some(res) = Resource::from_type_number(*t) {
                self.upsert_harvestable(*id, res, *tier, 0, *size, [*x, *y]);
            }
        }
    }

    /// Event 40: single resource spawn, including corpses of living resources.
    fn new_harvestable(&mut self, id: i64, p: &Params) {
        let Some(at) = pos(p.get(&8)) else { return };
        let type_number = int(p, 5).unwrap_or(-1);
        let mobile_type = int(p, 6).filter(|t| *t != 65535 && *t != -1);
        // Living resources carry their real kind in the mob table, not the type number.
        let living = mobile_type.and_then(|t| match self.data.mob(t)?.class {
            MobClass::Living(r) => Some(r),
            _ => None,
        });
        let Some(res) = living.or_else(|| Resource::from_type_number(type_number)) else { return };
        let tier = int(p, 7).unwrap_or(0);
        self.upsert_harvestable(id, res, tier, int(p, 11).unwrap_or(0), int(p, 10).unwrap_or(0), at);
    }

    /// Event 46: size/enchant change; a missing size means depleted.
    fn harvestable_change(&mut self, p: &Params) {
        let Some(id) = int(p, 0) else { return };
        let Some(size) = int(p, 1) else {
            if self.harvestables.remove(&id).is_some() {
                self.revision += 1;
            }
            return;
        };
        if let Some(h) = self.harvestables.get_mut(&id) {
            h.size = size;
            if let Some(e) = int(p, 2) {
                h.enchant = e.clamp(0, 4) as u8;
            }
            h.updated = Instant::now();
            self.revision += 1;
        }
    }

    fn new_mob(&mut self, p: &Params) {
        let (Some(id), Some(type_id)) = (int(p, 0), int(p, 1)) else { return };
        let at = pos(p.get(&7)).unwrap_or([0.0, 0.0]);
        let enchant = int(p, 34).unwrap_or(0).clamp(0, 4) as u8;
        let now = Instant::now();

        if let Some(name) = string(p, 33)
            && name.to_ascii_uppercase().starts_with("MISTS_")
        {
            let solo = name.to_ascii_lowercase().contains("solo");
            self.mists.entry(id).or_insert(MistPortal { pos: at, solo, enchant, updated: now }).updated = now;
            self.revision += 1;
            return;
        }
        if self.mobs.contains_key(&id) {
            return;
        }
        // Fort/dungeon NPCs spawn with a placeholder health of ~5: treat as full.
        let health = int(p, 2).unwrap_or(255);
        let health = if health < 10 { 255 } else { health.min(255) as u8 };
        let info = self.data.mob(type_id);
        self.mobs.insert(
            id,
            Mob {
                type_id,
                class: info.map(|i| i.class),
                name: info.map(|i| i.unique_name.clone()).unwrap_or_default(),
                tier: info.map_or(0, |i| i.tier),
                enchant,
                pos: at,
                health,
                max_hp: int(p, 13).unwrap_or(0),
                updated: now,
            },
        );
        self.revision += 1;
    }

    /// Any sign of life keeps a player from going stale (so later faction changes still alert).
    fn touch_player(&mut self, id: i64) {
        if let Some(pl) = self.players.get_mut(&id) {
            pl.updated = Instant::now();
        }
    }

    fn health_update(&mut self, id: i64, current: Option<i64>) {
        self.touch_player(id);
        let Some(m) = self.mobs.get_mut(&id) else { return };
        match current {
            Some(hp) if hp > 0 => {
                if m.max_hp > 0 {
                    m.health = ((hp as f64 / m.max_hp as f64) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
            _ => {
                self.mobs.remove(&id);
            }
        }
        self.revision += 1;
    }

    #[cfg(test)]
    fn pvp(&self, id: &str) -> Option<PvpType> {
        self.mist_zones.get(id).map(|m| m.pvp).or_else(|| self.data.zone(id).map(|z| z.pvp))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use openradar::capture::decode::udp_payload;
    use openradar::capture::pcap_file::read_pcap;
    use openradar::photon::{
        PhotonHandler, PhotonParser, post_process_event, post_process_request, post_process_response,
    };

    use super::*;

    struct Feed<'a>(&'a mut Game);

    impl PhotonHandler for Feed<'_> {
        fn on_event(&mut self, mut e: EventData) {
            post_process_event(&mut e);
            self.0.on_event(&e);
        }
        fn on_request(&mut self, mut r: OperationRequest) {
            post_process_request(&mut r);
            self.0.on_request(&r);
        }
        fn on_response(&mut self, mut r: OperationResponse) {
            post_process_response(&mut r);
            self.0.on_response(&r);
        }
    }

    fn game() -> Game {
        Game::new(Arc::new(GameData::load().unwrap()))
    }

    fn replay(g: &mut Game, fixture: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/photon").join(fixture);
        let pcap = read_pcap(&path).unwrap();
        let mut parser = PhotonParser::new();
        for frame in &pcap.packets {
            if let Some(payload) = udp_payload(pcap.link_type, frame) {
                parser.receive_packet(payload, &mut Feed(g));
            }
        }
    }

    fn params(pairs: Vec<(u8, Value)>) -> Params {
        pairs.into_iter().collect()
    }

    #[test]
    fn join_sets_map_and_position() {
        let mut g = game();
        replay(&mut g, "router/join-finished.pcap");
        assert!(g.map_id.is_some());
        assert_ne!(g.local, [0.0, 0.0]);
    }

    #[test]
    fn harvestables_are_tracked() {
        let mut g = game();
        replay(&mut g, "harvestables/batch-spawn.pcap");
        replay(&mut g, "harvestables/single-spawn.pcap");
        assert!(!g.harvestables.is_empty());
        assert!(g.harvestables.values().all(|h| (1..=8).contains(&h.tier) && h.enchant <= 4));
    }

    #[test]
    fn mobs_and_players_are_tracked() {
        let mut g = game();
        replay(&mut g, "mobs/spawn.pcap");
        assert!(!g.mobs.is_empty());
        assert!(g.mobs.values().any(|m| m.class.is_some()));
        replay(&mut g, "players/spawn.pcap");
        assert!(g.players.values().any(|p| !p.name.is_empty()));
    }

    #[test]
    fn whole_capture_set_replays_cleanly() {
        let mut g = game();
        for f in ["move_heavy.pcap", "generic_events.pcap", "move_map_change.pcap", "fragments.pcap", "operations.pcap"] {
            replay(&mut g, f);
        }
        assert!(g.map_id.is_some());
        g.prune(Instant::now());
    }

    #[test]
    fn threat_rules() {
        assert!(!is_threat(255, PvpType::Safe));
        assert!(!is_threat(0, PvpType::Red));
        assert!(is_threat(255, PvpType::Yellow));
        assert!(is_threat(0, PvpType::Black));
    }

    #[test]
    fn black_zone_spawn_raises_threat_and_leave_removes() {
        let mut g = game();
        let black = g.data.zones_with(PvpType::Black);
        g.apply_map_change(&black);
        let spawn = EventData {
            code: 1,
            parameters: params(vec![
                (0, Value::Int(42)),
                (1, Value::String("Ganker".into())),
                (8, Value::String("Bad Guild".into())),
                (252, Value::Short(ev::NEW_CHARACTER as i16)),
            ]),
        };
        g.on_event(&spawn);
        let threats = g.take_threats();
        assert_eq!(threats.len(), 1);
        assert_eq!(threats[0].name, "Ganker");
        assert!(g.take_threats().is_empty());

        g.on_event(&EventData { code: 1, parameters: params(vec![(0, Value::Int(42)), (252, Value::Byte(1))]) });
        assert!(g.players.is_empty());
    }

    #[test]
    fn mists_inherit_origin_and_lethal_choice() {
        let mut g = game();
        let red = g.data.zones_with(PvpType::Red);
        g.apply_map_change(&red);
        g.apply_map_change("@MISTS@abc");
        assert_eq!(g.pvp("@MISTS@abc"), Some(PvpType::Black), "red origin makes the mist lethal");

        let safe = g.data.zones_with(PvpType::Yellow);
        g.apply_map_change(&safe);
        g.on_request(&OperationRequest {
            operation_code: 1,
            parameters: params(vec![(1, Value::Byte(8)), (2, Value::Byte(1)), (253, Value::Short(op::MISTS_USE_STATIC_ENTRANCE as i16))]),
        });
        g.apply_map_change("@MISTS@def");
        assert_eq!(g.zone().unwrap().pvp, PvpType::Black);
        assert!(g.zone().unwrap().name.starts_with("Mist of "));

        g.apply_map_change(&safe);
        assert!(g.mist_zones.is_empty());
    }

    #[test]
    fn harvestable_depletion_and_range_prune() {
        let mut g = game();
        g.upsert_harvestable(1, Resource::Ore, 5, 1, 3, [10.0, 10.0]);
        g.upsert_harvestable(2, Resource::Wood, 4, 0, 2, [500.0, 500.0]);
        g.prune(Instant::now());
        assert!(g.harvestables.contains_key(&1) && !g.harvestables.contains_key(&2));
        g.harvestable_change(&params(vec![(0, Value::Int(1))]));
        assert!(g.harvestables.is_empty());
    }
}

#[cfg(test)]
impl GameData {
    /// Any zone id with the given pvp type.
    pub fn zones_with(&self, pvp: PvpType) -> String {
        self.zone_ids().find(|id| self.zone(id).is_some_and(|z| z.pvp == pvp)).unwrap().clone()
    }
}
