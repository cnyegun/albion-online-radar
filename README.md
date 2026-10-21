# OpenRadar

A radar overlay for Albion Online on Linux (GNOME). It reads the game's network traffic and shows nearby
resources, mobs and mist portals on the map. It alerts you when a player comes near. It doesn't modify or inject
anything into the game.

## Install

```bash
sudo dnf install libpcap-devel alsa-lib-devel   # Fedora
sudo apt install libpcap-dev libasound2-dev     # Debian/Ubuntu

./install.sh
```

This builds the radar, installs it to `~/.local/bin` with an app-grid entry, and asks for sudo once so it can
capture packets without root. Run it again after pulling changes. `./install.sh --uninstall` removes it.

## Run

Start OpenRadar from the app grid, then change zone once in game so the radar knows where you are. Only one radar
runs at a time.

Clicks pass through the radar to the game. Use Alt+Tab to focus it, then:

| Key | Action |
|---|---|
| F1 | Settings |
| F2 | Click-through on/off (off lets you zoom and hover) |
| Ctrl+Q | Quit |
| Alt+Space | Move, resize |

The radar runs through XWayland by default so it can stay on top and remember its position. This looks blurry
on monitors with fractional scaling; turn off X11 mode in Settings → Display there and use Alt+Space → Always on Top
instead.

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
./install.sh
```

## Credits

Based on [Albion-Online-OpenRadar](https://github.com/Nouuu/Albion-Online-OpenRadar) by Nouuu (MIT).
Game data from [ao-data/ao-bin-dumps](https://github.com/ao-data/ao-bin-dumps).
