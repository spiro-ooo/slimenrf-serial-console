//! View rendering, split out of `app.rs`. These are `impl App` methods that live
//! in their own module for navigability; behaviour is unchanged.

use eframe::egui;

use crate::app::App;
use crate::theme::{
    danger_button, darken, desc, primary, section, ACCENT, ACCENT_AMBER, CARD_BG, HUE_CONN,
    HUE_INFO, HUE_MAG, HUE_RESET, HUE_SENSOR, HUE_SYSTEM, HUE_TEMP, HUE_TEST, MUTED,
};


impl App {
    pub(crate) fn tracker_panel(&mut self, ui: &mut egui::Ui) {
        let en = self.is_connected();

        // Common Tasks card
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT, 0.4)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("COMMON TASKS").size(13.0).strong().color(MUTED));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if primary(ui, en, "Calibrate", ACCENT)
                        .on_hover_text("Sends: calibrate")
                        .clicked()
                    {
                        self.send_cmd("calibrate".into());
                    }
                    desc(ui, "Lay the tracker flat and still, then zero the gyroscope.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(ui, en, "Pair", ACCENT)
                        .on_hover_text("Sends: pair")
                        .clicked()
                    {
                        self.send_cmd("pair".into());
                    }
                    desc(ui, "Put the tracker into pairing mode.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if primary(ui, en, "Update (DFU)", ACCENT_AMBER)
                        .on_hover_text("Sends: dfu")
                        .clicked()
                    {
                        self.send_cmd("dfu".into());
                    }
                    desc(ui, "Reboot into the bootloader to flash firmware.");
                });
            });

        ui.add_space(10.0);
        self.dfu_update_card(ui);

        ui.add_space(10.0);
        ui.label(egui::RichText::new("ALL COMMANDS").size(13.0).strong().color(MUTED));
        ui.separator();
        ui.add_space(2.0);

        ui.add_enabled_ui(en, |ui| {
            section(ui, "t_info", "Device information", HUE_INFO, true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("info").on_hover_text("Firmware version, IDs and current settings").clicked() { self.send_cmd("info".into()); }
                    if ui.button("uptime").on_hover_text("Time since the tracker last booted").clicked() { self.send_cmd("uptime".into()); }
                    if ui.button("battery").on_hover_text("Battery voltage and charge level").clicked() { self.send_cmd("battery".into()); }
                    if ui.button("nvs").on_hover_text("Dump stored non-volatile settings (NVS)").clicked() { self.send_cmd("nvs".into()); }
                    if ui.button("help").on_hover_text("List every console command the firmware supports").clicked() { self.send_cmd("help".into()); }
                    if ui.button("ping").on_hover_text("Check the tracker is alive and responding").clicked() { self.send_cmd("ping".into()); }
                    if ui.button("meow").on_hover_text("").clicked() { self.send_cmd("meow".into()); }
                });
            });

            section(ui, "t_sensor", "Sensors & calibration", HUE_SENSOR, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("scan").on_hover_text("Detect and identify the attached IMU(s)").clicked() { self.send_cmd("scan".into()); }
                    if ui.button("calibrate (ZRO)").on_hover_text("Zero the gyroscope — keep the tracker flat and still").clicked() { self.send_cmd("calibrate".into()); }
                    if ui.button("6-side").on_hover_text("Six-sided accelerometer calibration — rest it on each face when prompted").clicked() { self.send_cmd("6-side".into()); }
                    if ui.button("range").on_hover_text("Show the IMU accelerometer/gyro full-scale range").clicked() { self.send_cmd("range".into()); }
                    if ui.button("range reset").on_hover_text("Restore the default IMU full-scale range").clicked() { self.send_cmd("range reset".into()); }
                });
                ui.add_space(4.0);
                egui::Grid::new("t_sensor_params")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("debug duration (1–60 s):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.debug_dur).desired_width(50.0).hint_text("1"));
                            if ui.button("debug").on_hover_text("Stream raw sensor data for N seconds (1–60)").clicked() {
                                let d = self.tf.debug_dur.trim().to_owned();
                                if d.is_empty() { self.send_cmd("debug".into()); } else { self.send_cmd(format!("debug {d}")); }
                            }
                        });
                        ui.end_row();

                        ui.label("gyro sensitivity (deg diff) X/Y/Z:");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.sens_x).desired_width(56.0).hint_text("x"));
                            ui.add(egui::TextEdit::singleline(&mut self.tf.sens_y).desired_width(56.0).hint_text("y"));
                            ui.add(egui::TextEdit::singleline(&mut self.tf.sens_z).desired_width(56.0).hint_text("z"));
                            if ui.button("set sens").on_hover_text("Set per-axis gyro sensitivity correction").clicked() {
                                let x = self.tf.sens_x.trim().to_owned();
                                let y = self.tf.sens_y.trim().to_owned();
                                let z = self.tf.sens_z.trim().to_owned();
                                if x.is_empty() || y.is_empty() || z.is_empty() {
                                    self.push_info("Enter all three sens values (X, Y, Z).".into());
                                } else {
                                    self.send_cmd(format!("sens {x},{y},{z}"));
                                }
                            }
                            if ui.button("sens reset").on_hover_text("Clear gyro sensitivity correction").clicked() { self.send_cmd("sens reset".into()); }
                        });
                        ui.end_row();

                        ui.label("auto-calibrate sensitivity:");
                        ui.horizontal(|ui| {
                            ui.label("spin");
                            egui::ComboBox::from_id_salt("t_sens_auto_axis")
                                .selected_text(format!("{} axis", self.tf.sens_auto_axis))
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.tf.sens_auto_axis, "x".to_owned(), "x axis");
                                    ui.selectable_value(&mut self.tf.sens_auto_axis, "y".to_owned(), "y axis");
                                    ui.selectable_value(&mut self.tf.sens_auto_axis, "z".to_owned(), "z axis");
                                });
                            ui.add(egui::TextEdit::singleline(&mut self.tf.sens_auto_rev).desired_width(44.0));
                            ui.label("rotations");
                            if ui.button("sens auto").on_hover_text("Auto-calibrate gyro sensitivity on one axis: spin the tracker steadily about the chosen axis for the given number of full rotations (default 5, max 100).").clicked() {
                                let axis = self.tf.sens_auto_axis.clone();
                                let rev = self.tf.sens_auto_rev.trim().to_owned();
                                if rev.is_empty() {
                                    self.send_cmd(format!("sens auto {axis}"));
                                } else {
                                    self.send_cmd(format!("sens auto {axis} {rev}"));
                                }
                            }
                        });
                        ui.end_row();
                    });
            });

            section(ui, "t_mag", "Magnetometer", HUE_MAG, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("status (mag)").on_hover_text("Show magnetometer state").clicked() { self.send_cmd("mag".into()); }
                    if ui.button("mag on").on_hover_text("Enable the magnetometer for heading correction").clicked() { self.send_cmd("mag on".into()); }
                    if ui.button("mag off").on_hover_text("Disable the magnetometer").clicked() { self.send_cmd("mag off".into()); }
                    if ui.button("mag clear").on_hover_text("Erase the stored magnetometer calibration").clicked() { self.send_cmd("mag clear".into()); }
                    if ui.button("mag cal").on_hover_text("Start magnetometer calibration — rotate through all orientations").clicked() { self.send_cmd("mag cal".into()); }
                });
            });

            section(ui, "t_tcal", "Temperature calibration (tcal)", HUE_TEMP, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal status").on_hover_text("Show temperature-calibration state").clicked() { self.send_cmd("tcal status".into()); }
                    if ui.button("tcal on").on_hover_text("Enable temperature compensation").clicked() { self.send_cmd("tcal on".into()); }
                    if ui.button("tcal off").on_hover_text("Disable temperature compensation").clicked() { self.send_cmd("tcal off".into()); }
                    if ui.button("tcal dump").on_hover_text("Print the temperature-calibration table").clicked() { self.send_cmd("tcal dump".into()); }
                    if ui.button("tcal check").on_hover_text("Report calibration coverage / quality").clicked() { self.send_cmd("tcal check".into()); }
                    if ui.button("tcal clear").on_hover_text("Erase the temperature-calibration table").clicked() { self.send_cmd("tcal clear".into()); }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("tcal auto on").on_hover_text("Collect temperature-calibration data automatically").clicked() { self.send_cmd("tcal auto on".into()); }
                    if ui.button("tcal auto off").on_hover_text("Stop automatic temperature-calibration collection").clicked() { self.send_cmd("tcal auto off".into()); }
                    if ui.button("tcal boot on").on_hover_text("Recalibrate temperature on every boot").clicked() { self.send_cmd("tcal boot on".into()); }
                    if ui.button("tcal boot off").on_hover_text("Don't recalibrate temperature on boot").clicked() { self.send_cmd("tcal boot off".into()); }
                });
                egui::Grid::new("t_tcal_params")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("test temp (°C):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.tcal_test).desired_width(60.0).hint_text("current"));
                            if ui.button("tcal test").on_hover_text("Predict the gyro offset at a given temperature (°C)").clicked() {
                                let t = self.tf.tcal_test.trim().to_owned();
                                if t.is_empty() { self.send_cmd("tcal test".into()); } else { self.send_cmd(format!("tcal test {t}")); }
                            }
                        });
                        ui.end_row();

                        ui.label("remove sample index:");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.tcal_remove).desired_width(50.0).hint_text("0"));
                            if ui.button("tcal remove").on_hover_text("Delete one temperature-calibration sample by index").clicked() {
                                let i = self.tf.tcal_remove.trim().to_owned();
                                if i.is_empty() { self.push_info("Enter an index to remove.".into()); } else { self.send_cmd(format!("tcal remove {i}")); }
                            }
                        });
                        ui.end_row();
                    });
            });

            section(ui, "t_conn", "Connection & pairing", HUE_CONN, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("pair").on_hover_text("Enter pairing mode to bond with a receiver").clicked() { self.send_cmd("pair".into()); }
                    if danger_button(ui, "clear pairing", "Forget the paired receiver") { self.send_cmd("clear".into()); }
                    ui.separator();
                    if ui.button("tdma on").on_hover_text("Enable TDMA time-slotted radio scheduling").clicked() { self.send_cmd("tdma on".into()); }
                    if ui.button("tdma off").on_hover_text("Disable TDMA scheduling").clicked() { self.send_cmd("tdma off".into()); }
                });
                ui.add_space(4.0);
                egui::Grid::new("t_conn_params")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("receiver address (16 hex):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.set_addr).desired_width(170.0).hint_text("0011223344556677"));
                            if ui.button("set").on_hover_text("Bond to a receiver by its 16 hex-digit address").clicked() {
                                let a = self.tf.set_addr.trim().to_owned();
                                if a.is_empty() { self.push_info("Enter a 16 hex-digit address.".into()); } else { self.send_cmd(format!("set {a}")); }
                            }
                        });
                        ui.end_row();

                        ui.label("RF channel (1–100):");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.tf.channel).desired_width(56.0).hint_text("25"));
                            if ui.button("set channel").on_hover_text("Set the RF channel (1–100)").clicked() {
                                let c = self.tf.channel.trim().to_owned();
                                if c.is_empty() { self.push_info("Enter a channel 1–100.".into()); } else { self.send_cmd(format!("channel {c}")); }
                            }
                            if ui.button("clearchannel").on_hover_text("Reset the RF channel to the firmware default").clicked() { self.send_cmd("clearchannel".into()); }
                        });
                        ui.end_row();
                    });
            });

            section(ui, "t_system", "System", HUE_SYSTEM, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reboot").on_hover_text("Restart the tracker firmware").clicked() { self.send_cmd("reboot".into()); }
                    if danger_button(ui, "shutdown", "Power the tracker off") { self.send_cmd("shutdown".into()); }
                    if danger_button(ui, "dfu (UF2)", "Reboot into the UF2 bootloader to flash firmware") { self.send_cmd("dfu".into()); }
                    if danger_button(ui, "dfu ota", "Reboot into over-the-air (BLE) update mode") { self.send_cmd("dfu ota".into()); }
                });
            });

            section(ui, "t_reset", "Reset / clear (careful)", HUE_RESET, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("reset zro").on_hover_text("Clear the stored gyroscope zero offset").clicked() { self.send_cmd("reset zro".into()); }
                    if ui.button("reset acc").on_hover_text("Clear the accelerometer calibration").clicked() { self.send_cmd("reset acc".into()); }
                    if ui.button("reset sens").on_hover_text("Clear gyro sensitivity correction").clicked() { self.send_cmd("reset sens".into()); }
                    if ui.button("reset tcal").on_hover_text("Clear the temperature-calibration table").clicked() { self.send_cmd("reset tcal".into()); }
                    if ui.button("reset mag").on_hover_text("Clear the magnetometer calibration").clicked() { self.send_cmd("reset mag".into()); }
                    if ui.button("reset bat").on_hover_text("Reset battery-gauge learning").clicked() { self.send_cmd("reset bat".into()); }
                    if ui.button("reset fusion").on_hover_text("Reset the sensor-fusion filter state").clicked() { self.send_cmd("reset fusion".into()); }
                    if danger_button(ui, "reset all", "Erase ALL calibration and settings") { self.send_cmd("reset all".into()); }
                });
            });

            section(ui, "t_test", "Test mode", HUE_TEST, false, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("test on").on_hover_text("Enter test / diagnostic mode").clicked() { self.send_cmd("test on".into()); }
                    if ui.button("test off").on_hover_text("Leave test mode").clicked() { self.send_cmd("test off".into()); }
                });
            });
        });

        ui.add_space(8.0);
    }
}