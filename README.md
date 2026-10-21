# OpenRadar

A radar overlay for Albion Online on Linux (Wayland). It reads the game's network traffic and shows nearby
resources, mobs and mist portals on the map. It alerts you when a player comes near. It doesn't modify or inject
anything into the game.

## Build

```bash
sudo dnf install libpcap-devel alsa-lib-devel   # Fedora
sudo apt install libpcap-dev libasound2-dev     # Debian/Ubuntu

cargo build --release
sudo setcap cap_net_raw=eip target/release/openradar   # repeat after every build
```

## Run

```bash
target/release/openradar
```

Change zone once in game so the radar knows where you are.

Clicks pass through the radar to the game. Use Alt+Tab to focus it, then:

| Key | Action |
|---|---|
| F1 | Settings |
| F2 | Click-through on/off (off lets you zoom and hover) |
| Ctrl+Q | Quit |
| Alt+Space | Move, resize, always on top |

Settings are saved in `~/.config/openradar/native.json`.

## Alerts

| Zone | Alerts on |
|---|---|
| Safe | nobody |
| Yellow, Red | hostile-flagged players |
| Black | everyone |

To silence players, guilds or alliances, add them to the ignore list in Settings → Alerts.

## After a game patch

```bash
cargo run --release --bin update-data
cargo build --release
```

## Credits

Based on [Albion-Online-OpenRadar](https://github.com/Nouuu/Albion-Online-OpenRadar) by Nouuu (MIT).
Game data from [ao-data/ao-bin-dumps](https://github.com/ao-data/ao-bin-dumps).
