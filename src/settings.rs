//! User settings, persisted as JSON in the config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::{MobClass, Resource};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceFilter {
    pub show: bool,
    pub min_tier: u8,
    pub min_enchant: u8,
}

impl Default for ResourceFilter {
    fn default() -> Self {
        Self { show: true, min_tier: 4, min_enchant: 0 }
    }
}

impl ResourceFilter {
    pub fn accepts(&self, tier: u8, enchant: u8) -> bool {
        self.show && tier >= self.min_tier && enchant >= self.min_enchant
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Visible radius in game metres.
    pub range: f32,
    pub show_map: bool,
    pub map_brightness: f32,
    /// Opacity of the map image (0 = invisible, 1 = solid).
    pub map_opacity: f32,
    /// Opacity of the empty area around the map (0 = see-through).
    pub background_opacity: f32,
    pub icon_scale: f32,
    pub show_rings: bool,

    /// Indexed like `Resource::ALL`.
    pub static_resources: [ResourceFilter; 5],
    pub living_resources: [ResourceFilter; 5],
    pub show_stack_count: bool,

    pub mobs_normal: bool,
    pub mobs_enchanted: bool,
    pub mobs_miniboss: bool,
    pub mobs_boss: bool,
    pub mobs_mistboss: bool,
    pub mobs_unknown: bool,
    pub mob_tiers: bool,
    pub mob_health: bool,
    pub mist_portals: bool,

    pub alert_flash: bool,
    pub alert_sound: bool,
    pub alert_sound_file: String,
    pub alert_volume: f32,
    pub alert_cooldown_ms: u64,
    /// Player, guild or alliance names that never alert (case-insensitive).
    pub ignore_list: Vec<String>,

    /// Last window size (points), restored at startup.
    pub window_size: Option<[f32; 2]>,
    pub window_maximized: bool,
    /// Clicks pass through the radar to the game underneath.
    pub click_through: bool,
    /// Capture interface names; empty means auto-select.
    pub capture_interfaces: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            range: 50.0,
            show_map: true,
            map_brightness: 0.8,
            map_opacity: 0.7,
            background_opacity: 0.0,
            icon_scale: 1.0,
            show_rings: true,
            static_resources: [ResourceFilter::default(); 5],
            living_resources: [ResourceFilter::default(); 5],
            show_stack_count: false,
            mobs_normal: true,
            mobs_enchanted: true,
            mobs_miniboss: true,
            mobs_boss: true,
            mobs_mistboss: true,
            mobs_unknown: false,
            mob_tiers: false,
            mob_health: true,
            mist_portals: true,
            alert_flash: true,
            alert_sound: true,
            alert_sound_file: "player.wav".into(),
            alert_volume: 0.8,
            alert_cooldown_ms: 500,
            ignore_list: Vec::new(),
            window_size: None,
            window_maximized: false,
            click_through: true,
            capture_interfaces: Vec::new(),
        }
    }
}

impl Settings {
    pub fn resource_filter(&self, resource: Resource, living: bool) -> &ResourceFilter {
        let i = Resource::ALL.iter().position(|r| *r == resource).unwrap_or(0);
        if living { &self.living_resources[i] } else { &self.static_resources[i] }
    }

    pub fn shows_mob(&self, class: Option<MobClass>) -> bool {
        match class {
            None => self.mobs_unknown,
            Some(MobClass::Living(_)) => true,
            Some(MobClass::Normal) => self.mobs_normal,
            Some(MobClass::Enchanted) => self.mobs_enchanted,
            Some(MobClass::MiniBoss) => self.mobs_miniboss,
            Some(MobClass::Boss) => self.mobs_boss,
            Some(MobClass::MistBoss) => self.mobs_mistboss,
        }
    }

    /// True when a name, guild or alliance is on the ignore list. Blank entries never match.
    pub fn is_ignored(&self, names: &[&str]) -> bool {
        self.ignore_list.iter().map(|e| e.trim()).filter(|e| !e.is_empty()).any(|entry| {
            names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).any(|n| n.eq_ignore_ascii_case(entry))
        })
    }

    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("openradar").join("native.json"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_list_matches_case_insensitively_and_skips_blanks() {
        let s = Settings { ignore_list: vec!["  my guild ".into(), "".into()], ..Settings::default() };
        assert!(s.is_ignored(&["Someone", "My Guild", ""]));
        assert!(!s.is_ignored(&["Someone", "", ""]));
    }

    #[test]
    fn partial_json_keeps_defaults() {
        let s: Settings = serde_json::from_str(r#"{"range": 80, "mobs_unknown": true}"#).unwrap();
        assert_eq!(s.range, 80.0);
        assert!(s.mobs_unknown && s.show_map);
    }
}
