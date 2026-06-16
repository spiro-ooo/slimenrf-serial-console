//! View rendering, split out of `app.rs`. These are `impl App` methods that live
//! in their own module for navigability; behaviour is unchanged.

use eframe::egui;

use crate::app::App;
use crate::theme::{
    danger_button, darken, desc, primary, section, ACCENT, ACCENT_AMBER, CARD_BG, HUE_CONN,
    HUE_DATA, HUE_INFO, HUE_REMOTE, HUE_SENSOR, HUE_STATS, HUE_SYSTEM, MUTED,
};


impl App {
    pub(crate) fn receiver_panel(&mut self, ui: &mut egui::Ui) {
        let en = self.is_connected();

        // Common Tasks card
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT, 0.4)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("COMMON TASKS").size(13.0).strong().color(MUTED));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if primary(ui, en, "Pair a tracker", ACCENT)
                        .on_hover_text("Sends: pair")
                        .clicked()
                    {
                        self.send_cmd("pair".into());
                    }
                    desc(ui, "Listen for nearby trackers and bond them.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(ui, en, "Calibrate all", ACCENT)
                        .on_hover_text("Sends: send all calibrate")
                        .clicked()
                    {
                        self.send_cmd("send all calibrate".into());
                    }
                    desc(ui, "Lay all trackers flat and still, then zero them at once.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(ui, en, "Enter DFU", ACCENT_AMBER)
                        .on_hover_text("Sends: dfu")
                        .clicked()
                    {
                        self.send_cmd("dfu".into());
                    }
                    desc(ui, "Reboot into the bootloader. If the command doesn't work, hold a magnet to the dongle while plugging in.");
                });
            });

        ui.add_space(10.0);
        self.rdfu_update_card(ui);

        ui.add_space(10.0);
        ui.label(egui::RichText::new("ALL COMMANDS").size(13.0).strong().color(MUTED));
        ui.separator();
        ui.add_space(2.0);

        ui.add_enabled_ui(en, |ui| {
            section(ui, "r_info", "Device information", HUE_INFO, true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("info").on_hover_text("Firmware version, IDs and settings").clicked() { self.send_cmd("info".into()); }
                    if ui.button("uptime").on_hover_text("Time since the receiver booted").clicked() { self.send_cmd("uptime".into()); }
                    if ui.button("list (paired)").on_hover_text("List bonded trackers and their slots").clicked() { self.send_cmd("list".into()); }
                    if ui.button("help").on_hover_text("List every console command the firmware supports").clicked() { self.send_cmd("help".into()); }
                    if ui.button("meow").on_hover_text("").clicked() { self.send_cmd("meow".into()); }
                });
            });

            section(ui, "r_paired", "Paired devices", HUE_CONN, false, |ui| {
                egui::Grid::new("r_paired_params")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("add address (12 hex):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.rf.add_addr).desired_width(150.0).hint_text("001122334455"));
                            if ui.button("add").on_hover_text("Manually bond a tracker by its 12 hex-digit address").clicked() {
                                let a = self.rf.add_addr.trim().to_owned();
                                if a.is_empty() { self.push_info("Enter a 12 hex-digit address.".into()); } else { self.send_cmd(format!("add {a}")); }
                            }
                        });
                        ui.end_row();

                        ui.label("pair count (blank = until timeout):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.rf.pair_count).desired_width(50.0).hint_text("all"));
                            if ui.button("pair").on_hover_text("Listen for trackers in pairing mode (optionally a fixed count)").clicked() {
                                let c = self.rf.pair_count.trim().to_owned();
                                if c.is_empty() { self.send_cmd("pair".into()); } else { self.send_cmd(format!("pair {c}")); }
                            }
                            if ui.button("exit pairing").on_hover_text("Stop listening for new trackers").clicked() { self.send_cmd("exit".into()); }
                        });
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("remove last").on_hover_text("Unbond the most recently added tracker").clicked() { self.send_cmd("remove".into()); }
                    if danger_button(ui, "clear all pairings", "Forget every bonded tracker") { self.send_cmd("clear".into()); }
                });
            });

            section(ui, "r_stats", "Statistics", HUE_STATS, false, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("stats (toggle)").on_hover_text("Toggle live link-statistics output").clicked() { self.send_cmd("stats".into()); }
                    ui.separator();
                    ui.label("for N seconds:");
                    ui.add(egui::TextEdit::singleline(&mut self.rf.stats_sec).desired_width(50.0).hint_text("30"));
                    if ui.button("stats N").on_hover_text("Print link statistics for N seconds, then stop").clicked() {
                        let s = self.rf.stats_sec.trim().to_owned();
                        if s.is_empty() { self.push_info("Enter a duration in seconds.".into()); } else { self.send_cmd(format!("stats {s}")); }
                    }
                    if ui.button("resetstats").on_hover_text("Zero the statistics counters").clicked() { self.send_cmd("resetstats".into()); }
                });
            });

            section(ui, "r_channel", "RF channel (local receiver)", HUE_SENSOR, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("channel (1–100):");
                    ui.add(egui::TextEdit::singleline(&mut self.rf.channel).desired_width(56.0).hint_text("25"));
                    if ui.button("set channel").on_hover_text("Set the receiver's RF channel (1–100)").clicked() {
                        let c = self.rf.channel.trim().to_owned();
                        if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("channel {c}")); }
                    }
                    if ui.button("clearchannel").on_hover_text("Reset the RF channel to the firmware default").clicked() { self.send_cmd("clearchannel".into()); }
                    ui.separator();
                    if ui.button("rssi_scan").on_hover_text("Scan channels for RF noise / interference").clicked() { self.send_cmd("rssi_scan".into()); }
                });
            });

            section(ui, "r_system", "System", HUE_SYSTEM, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reboot").on_hover_text("Restart the receiver firmware").clicked() { self.send_cmd("reboot".into()); }
                    if danger_button(ui, "dfu (UF2)", "Reboot into the UF2 bootloader to flash firmware") { self.send_cmd("dfu".into()); }
                    if danger_button(ui, "dfu ota", "Reboot into over-the-air (BLE) update mode") { self.send_cmd("dfu ota".into()); }
                });
            });

            section(ui, "r_data", "Data collection & OTA", HUE_DATA, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("collect from tracker id:");
                    ui.add(egui::TextEdit::singleline(&mut self.rf.collect_id).desired_width(50.0).hint_text("0"));
                    if ui.button("collect").on_hover_text("Stream raw data from one tracker by id").clicked() {
                        let i = self.rf.collect_id.trim().to_owned();
                        if i.is_empty() { self.push_info("Enter a tracker id.".into()); } else { self.send_cmd(format!("collect {i}")); }
                    }
                    if ui.button("collect off").on_hover_text("Stop data collection").clicked() { self.send_cmd("collect off".into()); }
                    if ui.button("collect status").on_hover_text("Show data-collection state").clicked() { self.send_cmd("collect".into()); }
                });
                ui.horizontal(|ui| {
                    if ui.button("ota status").on_hover_text("Show over-the-air update state").clicked() { self.send_cmd("ota".into()); }
                    ui.separator();
                    ui.label("ota info id:");
                    ui.add(egui::TextEdit::singleline(&mut self.rf.ota_info).desired_width(50.0).hint_text("0"));
                    if ui.button("ota info").on_hover_text("Show OTA details for a tracker id").clicked() {
                        let i = self.rf.ota_info.trim().to_owned();
                        if i.is_empty() { self.push_info("Enter a tracker id.".into()); } else { self.send_cmd(format!("ota info {i}")); }
                    }
                    if danger_button(ui, "ota abort", "Cancel an in-progress OTA update") { self.send_cmd("ota abort".into()); }
                });
            });

            section(ui, "r_remote", "Remote commands -> tracker(s)", HUE_REMOTE, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Target:");
                    ui.selectable_value(&mut self.rf.rem_target_all, true, "All active");
                    ui.selectable_value(&mut self.rf.rem_target_all, false, "By ID");
                    let allow_id_edit = !self.rf.rem_target_all;
                    ui.add_enabled(
                        allow_id_edit,
                        egui::TextEdit::singleline(&mut self.rf.rem_target_id).desired_width(50.0).hint_text("0"),
                    );
                });

                let target = self.remote_target();
                ui.label(egui::RichText::new(format!("-> send {target} <command>")).color(MUTED).monospace());
                ui.separator();

                ui.horizontal_wrapped(|ui| {
                    if ui.button("calibrate").on_hover_text("Zero the gyroscope on the target tracker(s)").clicked() { self.send_cmd(format!("send {target} calibrate")); }
                    if ui.button("6-side").on_hover_text("Six-sided accel calibration on the target(s)").clicked() { self.send_cmd(format!("send {target} 6-side")); }
                    if ui.button("scan").on_hover_text("Re-detect the IMU on the target(s)").clicked() { self.send_cmd(format!("send {target} scan")); }
                    if ui.button("ping").on_hover_text("Check the target tracker(s) respond").clicked() { self.send_cmd(format!("send {target} ping")); }
                    if ui.button("meow").on_hover_text("").clicked() { self.send_cmd(format!("send {target} meow")); }
                    if ui.button("reboot").on_hover_text("Restart the target tracker(s)").clicked() { self.send_cmd(format!("send {target} reboot")); }
                    if ui.button("fusion reset").on_hover_text("Reset the fusion filter on the target(s)").clicked() { self.send_cmd(format!("send {target} fusion")); }
                    if danger_button(ui, "shutdown", "Power off the target tracker(s)") { self.send_cmd(format!("send {target} shutdown")); }
                    if danger_button(ui, "clear pairing", "Make the target(s) forget this receiver") { self.send_cmd(format!("send {target} clear")); }
                });

                ui.add_space(2.0);
                ui.label("Magnetometer:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("mag on").on_hover_text("Enable the magnetometer on the target(s)").clicked() { self.send_cmd(format!("send {target} mag on")); }
                    if ui.button("mag off").on_hover_text("Disable the magnetometer on the target(s)").clicked() { self.send_cmd(format!("send {target} mag off")); }
                    if ui.button("mag clear").on_hover_text("Erase magnetometer calibration on the target(s)").clicked() { self.send_cmd(format!("send {target} mag clear")); }
                    if ui.button("mag cal").on_hover_text("Start magnetometer calibration on the target(s)").clicked() { self.send_cmd(format!("send {target} mag cal")); }
                });

                ui.add_space(2.0);
                ui.label("Reset:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reset zro").on_hover_text("Clear gyro zero offset on the target(s)").clicked() { self.send_cmd(format!("send {target} reset zro")); }
                    if ui.button("reset acc").on_hover_text("Clear accelerometer calibration on the target(s)").clicked() { self.send_cmd(format!("send {target} reset acc")); }
                    if ui.button("reset bat").on_hover_text("Reset battery-gauge learning on the target(s)").clicked() { self.send_cmd(format!("send {target} reset bat")); }
                    if ui.button("reset mag").on_hover_text("Clear magnetometer calibration on the target(s)").clicked() { self.send_cmd(format!("send {target} reset mag")); }
                    if ui.button("reset tcal").on_hover_text("Clear temperature-calibration table on the target(s)").clicked() { self.send_cmd(format!("send {target} reset tcal")); }
                    if ui.button("reset fusion").on_hover_text("Reset the fusion filter on the target(s)").clicked() { self.send_cmd(format!("send {target} reset fusion")); }
                });

                ui.add_space(2.0);
                ui.label("Temperature calibration:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal on").on_hover_text("Enable temperature compensation on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal on")); }
                    if ui.button("tcal off").on_hover_text("Disable temperature compensation on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal off")); }
                    if ui.button("tcal auto on").on_hover_text("Auto-collect temperature data on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal auto on")); }
                    if ui.button("tcal auto off").on_hover_text("Stop auto temperature collection on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal auto off")); }
                    if ui.button("tcal boot on").on_hover_text("Recalibrate temperature each boot on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal boot on")); }
                    if ui.button("tcal boot off").on_hover_text("Don't recalibrate temperature on boot on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal boot off")); }
                    if ui.button("tcal clear").on_hover_text("Erase temperature-calibration table on the target(s)").clicked() { self.send_cmd(format!("send {target} tcal clear")); }
                });

                ui.add_space(2.0);
                ui.label("Scheduling / test / bootloader:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tdma on").on_hover_text("Enable TDMA scheduling on the target(s)").clicked() { self.send_cmd(format!("send {target} tdma on")); }
                    if ui.button("tdma off").on_hover_text("Disable TDMA scheduling on the target(s)").clicked() { self.send_cmd(format!("send {target} tdma off")); }
                    if ui.button("test on").on_hover_text("Enter test mode on the target(s)").clicked() { self.send_cmd(format!("send {target} test on")); }
                    if ui.button("test off").on_hover_text("Leave test mode on the target(s)").clicked() { self.send_cmd(format!("send {target} test off")); }
                    if danger_button(ui, "dfu", "Reboot the target(s) into the UF2 bootloader") { self.send_cmd(format!("send {target} dfu")); }
                    if danger_button(ui, "dfu ota", "Reboot the target(s) into OTA (BLE) update mode") { self.send_cmd(format!("send {target} dfu ota")); }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("sens X/Y/Z:");
                    ui.add(egui::TextEdit::singleline(&mut self.rf.rem_sens_x).desired_width(56.0).hint_text("x"));
                    ui.add(egui::TextEdit::singleline(&mut self.rf.rem_sens_y).desired_width(56.0).hint_text("y"));
                    ui.add(egui::TextEdit::singleline(&mut self.rf.rem_sens_z).desired_width(56.0).hint_text("z"));
                    if ui.button("send sens").on_hover_text("Set per-axis gyro sensitivity on the target(s)").clicked() {
                        let x = self.rf.rem_sens_x.trim().to_owned();
                        let y = self.rf.rem_sens_y.trim().to_owned();
                        let z = self.rf.rem_sens_z.trim().to_owned();
                        if x.is_empty() || y.is_empty() || z.is_empty() {
                            self.push_info("Enter all three sens values.".into());
                        } else {
                            self.send_cmd(format!("send {target} sens {x},{y},{z}"));
                        }
                    }
                    if ui.button("send sens reset").on_hover_text("Clear gyro sensitivity on the target(s)").clicked() { self.send_cmd(format!("send {target} sens reset")); }
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Channel commands apply to ALL trackers + receiver (firmware restriction):")
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.rf.rem_channel).desired_width(56.0).hint_text("25"));
                    if ui.button("send all channel").on_hover_text("Set the RF channel on every device at once (1–100)").clicked() {
                        let c = self.rf.rem_channel.trim().to_owned();
                        if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("send all channel {c}")); }
                    }
                    if ui.button("send all clearchannel").on_hover_text("Reset the RF channel to default on every device").clicked() { self.send_cmd("send all clearchannel".into()); }
                });
            });
        });

        ui.add_space(8.0);
    }
}
