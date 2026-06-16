//! View rendering, split out of `app.rs`. These are `impl App` methods that live
//! in their own module for navigability; behaviour is unchanged.

use std::path::PathBuf;

use eframe::egui;

use crate::app::{App, DfuLevel};
use crate::theme::{darken, desc, primary, ACCENT_AMBER, CARD_BG, MUTED};


impl App {
    pub(crate) fn dfu_update_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT_AMBER, 0.45)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("UPDATE ALL TRACKERS")
                            .size(13.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Live count of trackers we'd flash.
                        let n = self.tracker_ports().len();
                        let txt = if n == 1 {
                            "1 tracker detected".to_owned()
                        } else {
                            format!("{n} trackers detected")
                        };
                        ui.label(egui::RichText::new(txt).color(MUTED));
                    });
                });
                desc(ui, "Updates every tracker connected by USB. Wireless trackers aren't included.");
                ui.add_space(6.0);

                // Firmware picker row.
                ui.horizontal(|ui| {
                    ui.label("Firmware:");
                    let avail = ui.available_width();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tdfu.firmware)
                            .desired_width((avail - 96.0).max(120.0))
                            .hint_text("path to firmware .uf2"),
                    );
                    if ui.add_enabled(!self.tdfu.running, egui::Button::new("Browse…")).clicked() {
                        // Native file dialog (XDG portal on Linux — no GTK needed).
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter("UF2 firmware", &["uf2"])
                            .set_title("Select tracker firmware (.uf2)");
                        // Start in the directory of the current entry, if any.
                        let cur = self.tdfu.firmware.trim().to_owned();
                        if !cur.is_empty() {
                            if let Some(parent) = PathBuf::from(&cur).parent() {
                                if parent.is_dir() {
                                    dlg = dlg.set_directory(parent);
                                }
                            }
                        }
                        if let Some(path) = dlg.pick_file() {
                            self.tdfu.firmware = path.display().to_string();
                        }
                    }
                });

                ui.add_space(2.0);
                ui.checkbox(
                    &mut self.tdfu.include_existing,
                    "Also flash drives already in DFU mode",
                )
                .on_hover_text(
                    "Off: only trackers that enter DFU from this action are flashed.\n\
                     On: also flash any UF2 drive already mounted (e.g. a manually-reset board).",
                );

                ui.add_space(8.0);

                // Action button + spinner.
                ui.horizontal(|ui| {
                    let label = if self.tdfu.running {
                        "Updating…"
                    } else {
                        "Update all trackers"
                    };
                    if primary(ui, !self.tdfu.running, label, ACCENT_AMBER).clicked() {
                        let ctx = ui.ctx().clone();
                        self.start_dfu_update(&ctx);
                    }
                    if self.tdfu.running {
                        ui.add(egui::Spinner::new());
                    }
                    if !self.tdfu.running && !self.tdfu.log.is_empty() {
                        if ui.button("Clear log").clicked() {
                            self.tdfu.log.clear();
                            self.tdfu.result = None;
                        }
                    }
                });

                // Progress log.
                if !self.tdfu.log.is_empty() {
                    ui.add_space(6.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(24, 27, 34))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .auto_shrink([false, true])
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for (lvl, msg) in &self.tdfu.log {
                                        let color = match lvl {
                                            DfuLevel::Info => MUTED,
                                            DfuLevel::Good => {
                                                egui::Color32::from_rgb(120, 200, 130)
                                            }
                                            DfuLevel::Warn => {
                                                egui::Color32::from_rgb(222, 180, 90)
                                            }
                                        };
                                        ui.label(
                                            egui::RichText::new(msg)
                                                .color(color)
                                                .font(egui::FontId::monospace(12.0)),
                                        );
                                    }
                                });
                        });
                }
            });
    }
    pub(crate) fn rdfu_update_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .fill(CARD_BG)
            .stroke(egui::Stroke::new(1.0, darken(ACCENT_AMBER, 0.45)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("UPDATE RECEIVER FIRMWARE")
                            .size(13.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let n = self.receiver_ports().len();
                        ui.label(
                            egui::RichText::new(if n == 1 {
                                "1 receiver detected".to_owned()
                            } else {
                                format!("{n} receivers detected")
                            })
                            .color(MUTED),
                        );
                    });
                });
                desc(ui, "Flashes the receiver dongle directly — pick a .hex or DFU .zip, then Flash. To enter DFU, hold a magnet to the dongle while plugging it in.");
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("Firmware:");
                    let avail = ui.available_width();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.rdfu.package)
                            .desired_width((avail - 96.0).max(120.0))
                            .hint_text("path to firmware .hex or .zip"),
                    );
                    if ui.add_enabled(!self.rdfu.running, egui::Button::new("Browse…")).clicked() {
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter("Receiver firmware (.hex / DFU .zip)", &["hex", "zip"])
                            .set_title("Select receiver firmware");
                        let cur = self.rdfu.package.trim().to_owned();
                        if !cur.is_empty() {
                            if let Some(parent) = PathBuf::from(&cur).parent() {
                                if parent.is_dir() {
                                    dlg = dlg.set_directory(parent);
                                }
                            }
                        }
                        if let Some(path) = dlg.pick_file() {
                            self.rdfu.package = path.display().to_string();
                        }
                    }
                });

                // Advanced: SoftDevice requirement (only used when flashing a .hex).
                // Default 0x00 = no SoftDevice, correct for the ESB-based receiver.
                let is_hex = self
                    .rdfu.package
                    .trim()
                    .to_ascii_lowercase()
                    .ends_with(".hex");
                if is_hex {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("SoftDevice req:").color(MUTED));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rdfu.sd_req)
                                .desired_width(120.0)
                                .hint_text("0x00"),
                        )
                        .on_hover_text(
                            "Advanced — leave this at 0x00. It's the SoftDevice firmware-ID the image \
                             requires; the bootloader rejects a mismatch (error 0x07). This receiver has \
                             no SoftDevice, so 0x00 is correct and you shouldn't need to change it. Only \
                             touch this if flashing fails with error 0x07.",
                        );
                        ui.label(
                            egui::RichText::new("leave at 0x00")
                                .color(MUTED)
                                .small(),
                        );
                    });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = if self.rdfu.running { "Updating…" } else { "Flash receiver" };
                    if primary(ui, !self.rdfu.running, label, ACCENT_AMBER).clicked() {
                        let ctx = ui.ctx().clone();
                        self.start_rdfu_update(&ctx);
                    }
                    if self.rdfu.running {
                        ui.add(egui::Spinner::new());
                    }
                    if !self.rdfu.running && !self.rdfu.log.is_empty() {
                        if ui.button("Clear log").clicked() {
                            self.rdfu.log.clear();
                            self.rdfu.result = None;
                        }
                    }
                });

                if self.rdfu.running && self.rdfu.total > 0 {
                    let frac = (self.rdfu.done as f32 / self.rdfu.total as f32).clamp(0.0, 1.0);
                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .show_percentage()
                            .desired_width(ui.available_width()),
                    );
                }

                if !self.rdfu.log.is_empty() {
                    ui.add_space(6.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(24, 27, 34))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .auto_shrink([false, true])
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for (lvl, msg) in &self.rdfu.log {
                                        let color = match lvl {
                                            DfuLevel::Info => MUTED,
                                            DfuLevel::Good => egui::Color32::from_rgb(120, 200, 130),
                                            DfuLevel::Warn => egui::Color32::from_rgb(222, 180, 90),
                                        };
                                        ui.label(
                                            egui::RichText::new(msg)
                                                .color(color)
                                                .font(egui::FontId::monospace(12.0)),
                                        );
                                    }
                                });
                        });
                }
            });
    }
}
