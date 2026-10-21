//! Rebuilds `data/*.json` from the public ao-data/ao-bin-dumps repository.
//! Run after an Albion patch, then rebuild the radar (the data is embedded).
//!
//!     cargo run --release --bin update-data [-- --out DIR]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

const BASE: &str = "https://raw.githubusercontent.com/ao-data/ao-bin-dumps/refs/heads/master";

/// Item kinds in items.json that carry item power.
const ITEM_KINDS: [&str; 14] = [
    "hideoutitem",
    "trackingitem",
    "farmableitem",
    "simpleitem",
    "consumableitem",
    "consumablefrominventoryitem",
    "equipmentitem",
    "weapon",
    "mount",
    "furnitureitem",
    "journalitem",
    "labourercontract",
    "mountskin",
    "crystalleagueitem",
];

fn fetch(path: &str) -> Result<Vec<u8>> {
    let url = format!("{BASE}/{path}");
    eprintln!("  downloading {path}");
    let mut res = ureq::get(&url).call().with_context(|| format!("GET {url}"))?;
    Ok(res.body_mut().with_config().limit(512 * 1024 * 1024).read_to_vec()?)
}

fn fetch_json(path: &str) -> Result<Value> {
    serde_json::from_slice(&fetch(path)?).with_context(|| format!("parse {path}"))
}

/// XML-derived JSON holds a single child as an object and several as an array.
fn list(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    }
}

fn attr<'a>(v: &'a Value, name: &str) -> Option<&'a str> {
    v.get(format!("@{name}")).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Leading integer of an attribute, like JavaScript's `parseInt` (0 when absent).
fn int_attr(v: &Value, name: &str) -> i64 {
    let s = attr(v, name).unwrap_or("").trim();
    let end =
        s.char_indices().find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && *c == '-'))).map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0)
}

/// `data/items.min.json`: array indexed by item id; `{n: name, p: item power}` or null.
/// items.txt is the canonical id list; items.json supplies item power.
fn build_items() -> Result<Value> {
    let items = fetch_json("items.json")?;
    let root = items.get("items").context("items.json: no `items` root")?;
    let mut power = std::collections::HashMap::<String, i64>::new();
    for kind in ITEM_KINDS {
        for item in list(root.get(kind)) {
            let Some(name) = attr(item, "uniquename") else { continue };
            let base = int_attr(item, "itempower");
            power.insert(name.to_owned(), base);
            for ench in list(item.get("enchantments").and_then(|e| e.get("enchantment"))) {
                let level = int_attr(ench, "enchantmentlevel");
                let p = int_attr(ench, "itempower");
                power.insert(format!("{name}@{level}"), if p != 0 { p } else { base });
            }
        }
    }

    // Lines look like "8952: T7_HEAD_PLATE_AVALON@2   : Avalonian Plate Helmet".
    let txt = String::from_utf8_lossy(&fetch("formatted/items.txt")?).into_owned();
    let mut out: Vec<Value> = Vec::new();
    for line in txt.lines() {
        let Some((id, rest)) = line.split_once(':') else { continue };
        let (Ok(id), Some(name)) = (id.trim().parse::<usize>(), rest.split_whitespace().next()) else { continue };
        let name = name.trim_end_matches(':');
        if out.len() <= id {
            out.resize(id + 1, Value::Null);
        }
        out[id] = json!({ "n": name, "p": power.get(name).copied().unwrap_or(0) });
    }
    if out.len() < 1000 {
        bail!("items.txt yielded only {} ids; format changed?", out.len());
    }
    Ok(Value::Array(out))
}

/// `data/mobs.min.json`: array indexed by `typeId - 16`; `{u, t, c?, l?}`.
fn build_mobs() -> Result<Value> {
    let raw = fetch_json("mobs.json")?;
    let root = raw.get("Mobs").unwrap_or(&raw);
    let mobs = list(root.get("Mob"));
    if mobs.len() < 1000 {
        bail!("mobs.json yielded only {} mobs; format changed?", mobs.len());
    }
    let out = mobs
        .into_iter()
        .map(|m| {
            let mut o = Map::new();
            o.insert("u".into(), json!(attr(m, "uniquename").unwrap_or("")));
            o.insert("t".into(), json!(int_attr(m, "tier")));
            if let Some(c) = attr(m, "mobtypecategory").or_else(|| attr(m, "category")) {
                o.insert("c".into(), json!(c));
            }
            if let Some(l) = m.get("Loot").and_then(|l| l.get("Harvestable")).and_then(|h| attr(h, "type")) {
                o.insert("l".into(), json!(l));
            }
            Value::Object(o)
        })
        .collect();
    Ok(Value::Array(out))
}

/// Zone PvP rules, from the cluster type.
fn pvp_type(kind: &str) -> &'static str {
    let t = kind.to_ascii_uppercase();
    let safe_exact = [
        "HIDEOUT",
        "TUNNEL_HIDEOUT",
        "TUNNEL_HIDEOUT_DEEP",
        "SAFEAREA",
        "STARTAREA",
        "STARTINGCITY",
        "TUTORIAL",
        "PASSAGE_SAFEAREA",
        "DUNGEON_SAFEAREA",
        "TUNNEL_ROYAL",
    ];
    if t.is_empty()
        || t.starts_with("PLAYERCITY_")
        || t.contains("ISLAND")
        || safe_exact.contains(&t.as_str())
        || t.contains("EXPEDITION")
        || t.starts_with("ARENA_")
    {
        "safe"
    } else if t.contains("RED") {
        "red"
    } else if t.contains("YELLOW") || (t.contains("HELL") && t.contains("NON_LETHAL")) {
        "yellow"
    } else if t.contains("BLACK")
        || t.starts_with("TUNNEL_")
        || t.contains("CORRUPTED_DUNGEON")
        || (t.contains("HELL") && t.contains("LETHAL"))
    {
        "black"
    } else {
        "safe"
    }
}

/// Tier from the first `_T<n>_` in a cluster file name like `1000_CTY_FR_AUTO_T1_NON`.
fn file_tier(file: &str) -> i64 {
    let parts: Vec<&str> = file.split('_').collect();
    let inner = parts.get(1..parts.len().saturating_sub(1)).unwrap_or(&[]);
    inner.iter().find_map(|p| p.strip_prefix('T').and_then(|n| n.parse().ok())).unwrap_or(0)
}

fn bounds(s: Option<&str>) -> Option<[f64; 2]> {
    let v: Vec<f64> = s?.split_whitespace().map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (v.len() == 2 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1]])
}

/// `data/zones.json`: `{id: {name, type, pvpType, tier, bounds?}}`.
fn build_zones() -> Result<Value> {
    let world = fetch_json("cluster/world.json")?;
    let clusters = list(world.pointer("/world/clusters/cluster"));
    let mut out = Map::new();
    for c in clusters {
        let Some(id) = attr(c, "id") else { continue };
        if id.to_ascii_lowercase().contains("debug") {
            continue;
        }
        let kind = attr(c, "type").unwrap_or("");
        let file = attr(c, "file").unwrap_or("");
        let tier = file_tier(file);
        let mut z = Map::new();
        z.insert("name".into(), json!(attr(c, "displayname").unwrap_or(id)));
        z.insert("type".into(), json!(kind));
        z.insert("pvpType".into(), json!(pvp_type(kind)));
        z.insert("tier".into(), json!(tier));
        if let (Some(min), Some(max)) = (bounds(attr(c, "minimapBoundsMin")), bounds(attr(c, "minimapBoundsMax"))) {
            z.insert("bounds".into(), json!({ "min": min, "max": max }));
        }
        out.insert(id.to_owned(), Value::Object(z));
    }
    if out.len() < 500 {
        bail!("world.json yielded only {} zones; format changed?", out.len());
    }
    Ok(Value::Object(out))
}

fn main() -> Result<()> {
    let mut out = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/data"));
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = args.next().context("--out needs a directory")?.into(),
            _ => bail!("usage: update-data [--out DIR]"),
        }
    }
    std::fs::create_dir_all(&out)?;
    let start = Instant::now();

    // Build everything first so a failure never leaves a half-updated set.
    let files = [("items.min.json", build_items()?), ("mobs.min.json", build_mobs()?), ("zones.json", build_zones()?)];
    for (name, value) in &files {
        let bytes = serde_json::to_vec(value)?;
        let count = value.as_array().map_or_else(|| value.as_object().map_or(0, Map::len), Vec::len);
        std::fs::write(out.join(name), &bytes)?;
        eprintln!("  wrote {name}: {count} entries, {} KB", bytes.len() / 1024);
    }
    eprintln!("Done in {:.1}s. Rebuild the radar to embed the new data.", start.elapsed().as_secs_f32());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pvp_rules() {
        for (kind, want) in [
            ("PLAYERCITY_SAFEAREA_02", "safe"),
            ("OPENPVP_YELLOW", "yellow"),
            ("OPENPVP_RED", "red"),
            ("OPENPVP_BLACK_1", "black"),
            ("TUNNEL_DEEP", "black"),
            ("TUNNEL_ROYAL", "safe"),
            ("TUNNEL_ROYAL_RED", "red"),
            ("DUNGEON_HELL_5V5_NON_LETHAL", "yellow"),
            ("DUNGEON_HELL_5V5_LETHAL", "black"),
            ("", "safe"),
        ] {
            assert_eq!(pvp_type(kind), want, "{kind}");
        }
    }

    #[test]
    fn parsing_helpers() {
        assert_eq!(file_tier("1000_CTY_FR_AUTO_T1_NON"), 1);
        assert_eq!(file_tier("4202_WRL_HL_AUTO_T6_BLK.cluster.xml"), 6);
        assert_eq!(file_tier("T5_START_X"), 0);
        assert_eq!(bounds(Some("-305 -305.5")), Some([-305.0, -305.5]));
        assert_eq!(bounds(Some("1 2 3")), None);
        assert_eq!(int_attr(&json!({"@tier": "12abc"}), "tier"), 12);
        assert_eq!(int_attr(&json!({}), "tier"), 0);
    }
}
