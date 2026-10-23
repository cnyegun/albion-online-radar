# OpenRadar

**See every enchanted node before you're close enough to see it.** OpenRadar is a native radar overlay for
Albion Online on Linux. It sits on top of the game and shows the resources, mobs and mist portals around you,
using the game's real icons and the zone's map. When a hostile player shows up, it warns you before they reach
you.

![OpenRadar in a T8 black zone: enchanted ore, fiber, wood, rock and hide around the player, with a tooltip
on a T8.3 ore node](docs/images/radar.png)

- **Every enchant at a glance.** Nodes are drawn with their in-game icons, so .1 green, .2 blue, .3 purple and
  .4 gold stand out right away. Hover a node to see its tier, enchant, charges left and distance.
- **Filter for what pays.** Set a minimum tier and enchant per resource. Each enchant counts as one tier, so
  "min 6" shows 6.0, 5.1 and 4.2 alike.
- **Know when to run.** Hostile players trigger a banner with their name, guild and item power, plus a red
  flash and a sound. The number in the corner is how many threats are nearby.
- **Mobs and mists too.** Bosses, enchanted mobs and mist portals are marked, and mobs show their health.
- **Native and light.** One small Rust binary with no browser and no Electron. It only reads network traffic and
  doesn't modify or inject anything into the game.

![A hostile player alert: a red banner showing the player's name, guild and item power, and a threat count of
3](docs/images/alert.png)

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

![The settings window, Resources tab: minimum tier and enchant sliders for static and living
resources](docs/images/settings.png)

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

## Screenshots

The images above are rendered off-screen from staged scenes, so no real players or screens appear in them.
To regenerate them after a UI change:

```bash
cargo test --release -- --ignored readme_screenshots
```

## Credits

Based on [Albion-Online-OpenRadar](https://github.com/Nouuu/Albion-Online-OpenRadar) by Nouuu (MIT).
Game data from [ao-data/ao-bin-dumps](https://github.com/ao-data/ao-bin-dumps).
