//! The settings window (F1).

use eframe::egui::{self, RichText, ViewportCommand};

use super::radar::resource_color;
use super::{DANGER, OK, RadarApp, TEXT_DIM, Tab};
use crate::data::Resource;
use crate::runtime::Source;

impl RadarApp {
    /// Settings open in their own normal window, so they fit no matter how
    /// small the overlay is.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("OpenRadar settings")
            .with_app_id("openradar")
            .with_inner_size([520.0, 620.0])
            .with_min_inner_size([420.0, 320.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let close = ui.input(|i| {
                i.viewport().close_requested() || i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F1)
            });
            if close {
                self.show_settings = false;
            }
            let frame = egui::Frame::new().fill(ui.visuals().window_fill).inner_margin(egui::Margin::same(14));
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Resources, "Resources"),
                        (Tab::Mobs, "Mobs"),
                        (Tab::Alerts, "Alerts"),
                        (Tab::Network, "Network"),
                        (Tab::Display, "Display"),
                    ] {
                        ui.selectable_value(&mut self.tab, tab, label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::Resources => self.resources_tab(ui),
                    Tab::Mobs => self.mobs_tab(ui),
                    Tab::Alerts => self.alerts_tab(ui),
                    Tab::Network => self.network_tab(ui),
                    Tab::Display => self.display_tab(ui),
                });
            });
        });
    }

    fn resources_tab(&mut self, ui: &mut egui::Ui) {
        for (title, living) in
            [("Static resources (trees, rocks, fiber, ore)", false), ("Living resources (animals, critters)", true)]
        {
            ui.label(RichText::new(title).strong());
            egui::Grid::new(("res-grid", living)).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                for (i, r) in Resource::ALL.iter().enumerate() {
                    let f = if living {
                        &mut self.settings.living_resources[i]
                    } else {
                        &mut self.settings.static_resources[i]
                    };
                    ui.checkbox(&mut f.show, RichText::new(r.name()).color(resource_color(*r)));
                    ui.add_enabled(f.show, egui::Slider::new(&mut f.min_tier, 1..=12).text("min tier"));
                    ui.add_enabled(f.show, egui::Slider::new(&mut f.min_enchant, 0..=4).text("min .ench"));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new("Each enchant counts as a tier: min tier 6 shows 6.0, 5.1, 4.2 and up.").color(TEXT_DIM),
        );
        ui.checkbox(&mut self.settings.show_stack_count, "Show remaining stacks next to static resources");
    }

    fn mobs_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.checkbox(&mut s.mobs_normal, "Normal mobs");
        ui.checkbox(&mut s.mobs_enchanted, "Enchanted / champions");
        ui.checkbox(&mut s.mobs_miniboss, "Mini-bosses / veterans");
        ui.checkbox(&mut s.mobs_boss, "Bosses");
        ui.checkbox(&mut s.mobs_mistboss, "Mist bosses");
        ui.checkbox(&mut s.mobs_unknown, "Unknown mobs (not in the mob table)");
        ui.checkbox(&mut s.mist_portals, "Mist portals");
        ui.separator();
        ui.checkbox(&mut s.mob_health, "Health bars on damaged mobs");
        ui.checkbox(&mut s.mob_tiers, "Tier labels");
    }

    fn alerts_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Threat alert").strong());
        ui.label(
            RichText::new("Black zone: every player. Yellow/Red: hostile-flagged only. Safe: never.")
                .color(TEXT_DIM)
                .size(11.5),
        );
        let s = &mut self.settings;
        ui.checkbox(&mut s.alert_flash, "Flash the window");
        ui.checkbox(&mut s.alert_sound, "Play a sound");
        ui.add_enabled_ui(s.alert_sound, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("sound").selected_text(&s.alert_sound_file).show_ui(ui, |ui| {
                    for name in &self.shared.sounds {
                        ui.selectable_value(&mut s.alert_sound_file, name.clone(), name);
                    }
                });
                if ui.button("Test sound").clicked() {
                    self.shared.play_sound(&s.alert_sound_file, s.alert_volume);
                }
            });
            ui.add(egui::Slider::new(&mut s.alert_volume, 0.0..=1.0).text("volume"));
            ui.add(egui::Slider::new(&mut s.alert_cooldown_ms, 0..=5000).text("cooldown (ms)"));
        });
        if ui.button("Test alert").on_hover_text("Simulate a hostile player with the current settings").clicked() {
            *self.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = self.settings.clone();
            self.shared.test_alert();
        }
        if !self.shared.has_audio() {
            ui.label(RichText::new("No audio device found — sounds are disabled.").color(DANGER));
        }
        ui.separator();
        ui.label(RichText::new("Ignore list").strong());
        ui.label(
            RichText::new("One player, guild or alliance per line; they never trigger an alert.")
                .color(TEXT_DIM)
                .size(11.5),
        );
        if ui
            .add(egui::TextEdit::multiline(&mut self.ignore_text).desired_rows(6).desired_width(f32::INFINITY))
            .changed()
        {
            self.settings.ignore_list =
                self.ignore_text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
        }
    }

    fn network_tab(&mut self, ui: &mut egui::Ui) {
        let Source::Live { manager, interfaces } = &self.shared.source else {
            ui.label("Replaying a capture file; live capture is off.");
            return;
        };
        let state = manager.state();
        if state.active.is_empty() {
            ui.label(
                RichText::new("Not capturing. Tick the adapter Albion uses (usually your Wi-Fi or Ethernet).")
                    .color(DANGER),
            );
        }
        let list = interfaces.read().unwrap_or_else(|e| e.into_inner()).clone();
        egui::Grid::new("ifaces").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for i in openradar::capture::rank_candidates(&list) {
                let mut on = self.net_selection.contains(&i.name);
                let label =
                    if i.description.is_empty() { i.name.clone() } else { format!("{} ({})", i.name, i.description) };
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        self.net_selection.insert(i.name.clone());
                    } else {
                        self.net_selection.remove(&i.name);
                    }
                }
                ui.label(RichText::new(&i.address).color(TEXT_DIM));
                let cat = openradar::capture::categorize(&i.name, &i.description);
                let active = state.active.contains(&i.name);
                ui.label(
                    RichText::new(if active { "capturing".to_owned() } else { cat.as_str().to_owned() })
                        .color(if active { OK } else { TEXT_DIM }),
                );
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                let target: Vec<_> = list.iter().filter(|i| self.net_selection.contains(&i.name)).cloned().collect();
                self.net_message = Some(match manager.reconfigure(&target) {
                    Ok(()) => "Capture updated.".into(),
                    Err(e) => format!("{e}"),
                });
                self.settings.capture_interfaces = target.into_iter().map(|i| i.name).collect();
            }
            if ui.button("Refresh list").clicked()
                && let Ok(fresh) = openradar::capture::enumerate_interfaces()
            {
                *interfaces.write().unwrap_or_else(|e| e.into_inner()) = fresh;
            }
        });
        if let Some(msg) = &self.net_message {
            ui.label(RichText::new(msg).color(TEXT_DIM));
        }
        for (name, err) in &state.last_errors {
            ui.label(RichText::new(format!("{name}: {err}")).color(DANGER).size(11.5));
        }
    }

    fn display_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.add(egui::Slider::new(&mut s.range, 15.0..=250.0).text("range (m) — or scroll on the radar"));
        ui.add(egui::Slider::new(&mut s.icon_scale, 0.6..=2.0).text("icon size"));
        ui.checkbox(&mut s.show_map, "Map background");
        ui.add_enabled(s.show_map, egui::Slider::new(&mut s.map_opacity, 0.0..=1.0).text("map opacity"));
        ui.add_enabled(s.show_map, egui::Slider::new(&mut s.map_brightness, 0.1..=1.0).text("map brightness"));
        ui.add(egui::Slider::new(&mut s.background_opacity, 0.0..=1.0).text("background opacity"));
        ui.add(egui::Slider::new(&mut s.edge_fade, 0.0..=150.0).text("edge fade (px)"));
        ui.checkbox(&mut s.show_rings, "Distance rings");
        ui.checkbox(&mut s.click_through, "Click-through (clicks go to the game; F2 toggles)");
        ui.checkbox(&mut s.x11, "X11 mode: always on top, remembers position (restart to apply)")
            .on_hover_text("Runs through XWayland. Looks blurry on monitors with fractional scaling.");
        ui.separator();
        ui.label(
            RichText::new("Focus the radar with Alt+Tab, then: F1 settings  ·  F2 click-through  ·  Ctrl+Q quit\nMove and resize: Alt+Space. Zoom by scrolling when click-through is off.")
                .color(TEXT_DIM)
                .size(11.0),
        );
        if ui.button("Quit OpenRadar").clicked() {
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close);
        }
    }
}
