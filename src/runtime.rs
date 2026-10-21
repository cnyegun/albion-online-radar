//! Packet pipeline: capture (or pcap replay) -> Photon decode -> game state,
//! plus threat alerts. Runs on capture threads; the UI only reads the result.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use openradar::capture::{self, Category, Manager, NetworkInterface};
use openradar::photon::{
    EventData, OperationRequest, OperationResponse, PhotonHandler, PhotonParser, post_process_event,
    post_process_request, post_process_response,
};

use crate::game::{Game, Threat};
use crate::settings::Settings;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct AlertState {
    pub flash_at: Option<Instant>,
    /// Latest threat, shown as a banner for a few seconds.
    pub banner: Option<(Threat, Instant)>,
    last_sound: Option<Instant>,
}

pub enum Source {
    Live { manager: Arc<Manager>, interfaces: RwLock<Vec<NetworkInterface>> },
    Replay,
}

pub struct Shared {
    pub game: Mutex<Game>,
    pub settings: RwLock<Settings>,
    pub alert: Mutex<AlertState>,
    pub source: Source,
    pub sounds: Vec<String>,
    audio: Option<openradar::audio::Player>,
    ctx: OnceLock<eframe::egui::Context>,
}

/// Feeds decoded messages into the game state.
struct Feed<'a> {
    game: &'a mut Game,
}

impl PhotonHandler for Feed<'_> {
    fn on_event(&mut self, mut e: EventData) {
        post_process_event(&mut e);
        self.game.on_event(&e);
    }
    fn on_request(&mut self, mut r: OperationRequest) {
        post_process_request(&mut r);
        self.game.on_request(&r);
    }
    fn on_response(&mut self, mut r: OperationResponse) {
        post_process_response(&mut r);
        self.game.on_response(&r);
    }
}

const SOUNDS: [(&str, &[u8]); 8] = [
    ("player.wav", include_bytes!("../assets/sounds/player.wav")),
    ("buzzer.wav", include_bytes!("../assets/sounds/buzzer.wav")),
    ("brass.wav", include_bytes!("../assets/sounds/brass.wav")),
    ("coin.wav", include_bytes!("../assets/sounds/coin.wav")),
    ("drums.wav", include_bytes!("../assets/sounds/drums.wav")),
    ("piano.wav", include_bytes!("../assets/sounds/piano.wav")),
    ("pop.wav", include_bytes!("../assets/sounds/pop.wav")),
    ("vibraphone.wav", include_bytes!("../assets/sounds/vibraphone.wav")),
];

impl Shared {
    pub fn new(game: Game, settings: Settings, source: Source) -> Arc<Self> {
        let audio = openradar::audio::new_player(SOUNDS.iter().map(|(n, b)| ((*n).to_owned(), b.to_vec())));
        Arc::new(Self {
            game: Mutex::new(game),
            settings: RwLock::new(settings),
            alert: Mutex::default(),
            source,
            sounds: SOUNDS.iter().map(|(n, _)| (*n).to_owned()).collect(),
            audio,
            ctx: OnceLock::new(),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_ctx(&self, ctx: eframe::egui::Context) {
        let _ = self.ctx.set(ctx);
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    pub fn play_sound(&self, file: &str, volume: f32) {
        if let Some(a) = &self.audio {
            a.play(file, f64::from(volume));
        }
    }

    /// Decodes one UDP payload into the game state and raises alerts.
    pub fn handle_payload(&self, parser: &mut PhotonParser, payload: &[u8]) {
        let threats = {
            let mut game = lock(&self.game);
            parser.receive_packet(payload, &mut Feed { game: &mut game });
            game.take_threats()
        };
        if !threats.is_empty() {
            self.raise(threats);
        }
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }

    /// Fires the alert path with a made-up hostile, for previewing alert settings.
    pub fn test_alert(&self) {
        self.raise(vec![Threat {
            name: "Test Player".into(),
            guild: "Test Guild".into(),
            alliance: String::new(),
            item_power: Some(1350),
            hostile: true,
        }]);
    }

    fn raise(&self, threats: Vec<Threat>) {
        let s = self.settings.read().unwrap_or_else(|e| e.into_inner());
        let Some(threat) = threats.into_iter().rfind(|t| !s.is_ignored(&[&t.name, &t.guild, &t.alliance])) else {
            return;
        };
        let now = Instant::now();
        let mut alert = lock(&self.alert);
        if s.alert_flash {
            alert.flash_at = Some(now);
        }
        alert.banner = Some((threat, now));
        let cooldown = Duration::from_millis(s.alert_cooldown_ms);
        if s.alert_sound && alert.last_sound.is_none_or(|t| now.duration_since(t) >= cooldown) {
            alert.last_sound = Some(now);
            self.play_sound(&s.alert_sound_file, s.alert_volume);
        }
    }
}

/// Picks interfaces to capture on: saved selection first, then physical
/// Ethernet/Wi-Fi/ExitLag adapters, preferring private addresses but falling
/// back to public ones (some networks hand out public IPs directly).
pub fn choose_interfaces(saved: &[String], all: &[NetworkInterface]) -> Vec<NetworkInterface> {
    let chosen: Vec<_> = saved.iter().filter_map(|n| all.iter().find(|i| &i.name == n).cloned()).collect();
    if !chosen.is_empty() {
        return chosen;
    }
    let physical: Vec<_> = capture::rank_candidates(all)
        .into_iter()
        .filter(|i| {
            matches!(
                capture::categorize(&i.name, &i.description),
                Category::Ethernet | Category::Wifi | Category::Exitlag
            )
        })
        .collect();
    let private: Vec<_> = physical.iter().filter(|i| capture::is_rfc1918(&i.address)).cloned().collect();
    if private.is_empty() { physical } else { private }
}

/// Starts live capture and returns the source handle.
pub fn start_live(
    shared_slot: &Arc<OnceLock<Arc<Shared>>>,
    saved: &[String],
) -> Result<(Source, Vec<NetworkInterface>)> {
    let all = capture::enumerate_interfaces().context("list capture interfaces")?;
    let manager = Arc::new(Manager::new());
    let parser = Mutex::new(PhotonParser::new());
    let slot = Arc::clone(shared_slot);
    manager.on_packet(Arc::new(move |payload: &[u8]| {
        if let Some(shared) = slot.get() {
            shared.handle_payload(&mut lock(&parser), payload);
        }
    }));
    let target = choose_interfaces(saved, &all);
    Ok((Source::Live { manager, interfaces: RwLock::new(all) }, target))
}

/// Replays pcaps in order, in real time (scaled by `speed`), optionally looping.
pub fn start_replay(shared: Arc<Shared>, paths: Vec<PathBuf>, speed: f64, looping: bool) -> Result<()> {
    let files = paths
        .iter()
        .map(|p| capture::pcap_file::read_pcap(p).with_context(|| format!("read {}", p.display())))
        .collect::<Result<Vec<_>>>()?;
    std::thread::Builder::new().name("replay".into()).spawn(move || {
        loop {
            for pcap in &files {
                let mut parser = PhotonParser::new();
                let start = Instant::now();
                let t0 = pcap.times.first().copied().unwrap_or(0.0);
                for (frame, t) in pcap.packets.iter().zip(&pcap.times) {
                    let due = Duration::from_secs_f64(((t - t0) / speed).max(0.0));
                    if let Some(wait) = due.checked_sub(start.elapsed()) {
                        std::thread::sleep(wait);
                    }
                    if let Some(payload) = capture::decode::udp_payload(pcap.link_type, frame) {
                        shared.handle_payload(&mut parser, payload);
                    }
                }
            }
            if !looping {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, addr: &str) -> NetworkInterface {
        NetworkInterface { name: name.into(), description: String::new(), address: addr.into() }
    }

    #[test]
    fn falls_back_to_public_physical_adapters() {
        let all = [iface("lo", "127.0.0.1"), iface("wlp1s0", "86.50.43.158"), iface("docker0", "172.17.0.1")];
        let got = choose_interfaces(&[], &all);
        assert_eq!(got.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["wlp1s0"]);
    }

    #[test]
    fn prefers_private_and_saved() {
        let all = [iface("wlan0", "86.1.1.1"), iface("eth0", "192.168.1.4")];
        assert_eq!(choose_interfaces(&[], &all)[0].name, "eth0");
        assert_eq!(choose_interfaces(&["wlan0".into()], &all)[0].name, "wlan0");
    }
}
