//! Native OpenRadar: one window, no browser, no JavaScript.

mod app;
mod data;
mod game;
mod runtime;
mod settings;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use eframe::egui;

use crate::data::GameData;
use crate::game::Game;
use crate::runtime::{Shared, Source};
use crate::settings::Settings;

const USAGE: &str = "Usage: openradar [options]

  --replay FILE   Replay a pcap capture instead of capturing live (repeatable)
  --speed N       Replay speed multiplier (default 1)
  --loop          Restart the replay when it ends
  --maps DIR      Folder with <zone-id>.webp map backgrounds
  -h, --help      Show this help";

#[derive(Default)]
struct Cli {
    replay: Vec<PathBuf>,
    speed: f64,
    looping: bool,
    maps: Option<PathBuf>,
}

fn parse_args() -> Result<Cli> {
    let mut cli = Cli { speed: 1.0, ..Cli::default() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--replay" => cli.replay.push(value("--replay")?.into()),
            "--speed" => cli.speed = value("--speed")?.parse().context("--speed must be a number")?,
            "--loop" => cli.looping = true,
            "--maps" => cli.maps = Some(value("--maps")?.into()),
            "-h" | "--help" => bail!(""),
            other => bail!("unknown argument {other:?}"),
        }
    }
    if cli.speed <= 0.0 {
        bail!("--speed must be positive");
    }
    Ok(cli)
}

/// First existing maps folder: flag, env var, next to the binary, then the source tree.
fn find_maps_dir(flag: Option<PathBuf>) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    flag.into_iter()
        .chain(std::env::var_os("OPENRADAR_MAPS").map(PathBuf::from))
        .chain(exe_dir.map(|d| d.join("maps")))
        .chain(std::iter::once(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/maps"))))
        .find(|d| d.is_dir())
}

fn window_icon() -> Arc<egui::IconData> {
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png")).map(|i| i.to_rgba8());
    Arc::new(match icon {
        Ok(img) => egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() },
        Err(_) => egui::IconData::default(),
    })
}

fn run(cli: Cli) -> Result<()> {
    let data = Arc::new(GameData::load().context("load game data")?);
    let settings = Settings::load();
    let game = Game::new(Arc::clone(&data));

    let shared = if !cli.replay.is_empty() {
        let shared = Shared::new(game, settings, Source::Replay);
        runtime::start_replay(Arc::clone(&shared), cli.replay.clone(), cli.speed, cli.looping)?;
        shared
    } else {
        let slot = Arc::new(OnceLock::new());
            let (source, target) = runtime::start_live(&slot, &settings.capture_interfaces)?;
            let shared = Shared::new(game, settings, source);
            let _ = slot.set(Arc::clone(&shared));
            if let Source::Live { manager, .. } = &shared.source {
                if target.is_empty() {
                    eprintln!("No capture interface found; pick one in Settings → Network.");
                } else if let Err(e) = manager.reconfigure(&target) {
                    eprintln!("Capture: {e}");
                }
            }
        shared
    };

    let maps_dir = find_maps_dir(cli.maps);
    if maps_dir.is_none() {
        eprintln!("Map backgrounds not found; pass --maps DIR to enable them.");
    }

    let saved = shared.settings();
    let viewport = egui::ViewportBuilder::default()
        .with_title("OpenRadar")
        .with_app_id("openradar")
        .with_inner_size(saved.window_size.unwrap_or([1180.0, 780.0]))
        .with_min_inner_size([320.0, 240.0])
        .with_maximized(saved.window_maximized)
        .with_decorations(false)
        .with_transparent(true)
        .with_mouse_passthrough(saved.click_through)
        .with_icon(window_icon());
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let app_shared = Arc::clone(&shared);
    eframe::run_native("OpenRadar", options, Box::new(move |cc| Ok(Box::new(app::RadarApp::new(cc, app_shared, maps_dir)))))
        .map_err(|e| anyhow::anyhow!("window: {e}"))?;

    if let Source::Live { manager, .. } = &shared.source {
        manager.close(Duration::from_secs(2));
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            if !msg.is_empty() {
                eprintln!("{msg}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
