//! View rendering, split out of `app.rs`. These are `impl App` methods that live
//! in their own module for navigability; behaviour is unchanged.

use eframe::egui;

use crate::app::App;
use crate::serial::Mode;
use crate::theme::{ACCENT, ACCENT_AMBER, MUTED};


impl App {
    pub(crate) fn connection_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            if let Some(tex) = &self.icon_texture {
                ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(26.0, 26.0)));
                ui.add_space(4.0);
            }
            ui.heading("SlimeNRF Serial Control");
            ui.separator();
            if self.is_connected() {
                let name = self
                    .conn.connection
                    .as_ref()
                    .map(|c| c.port_name.clone())
                    .unwrap_or_default();
                ui.colored_label(egui::Color32::from_rgb(80, 200, 120), format!("* {name}"));
            } else if self.conn.connection.is_some() {
                ui.colored_label(egui::Color32::from_rgb(220, 190, 90), "... connecting…");
            } else {
                ui.colored_label(egui::Color32::GRAY, "- disconnected");
            }
        });

        ui.add_space(4.0);

        // Precompute dropdown contents so the combo closure doesn't borrow self.
        // (port_name, friendly label, id string, is_slimenrf_or_bootloader)
        let port_items: Vec<(String, String, String, bool)> = self
            .conn.ports
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.label(),
                    p.ids(),
                    p.guessed_mode.is_some() || p.in_bootloader(),
                )
            })
            .collect();
        let current = self.conn.selected_port.clone();
        let current_label = self
            .current_port_info()
            .map(|p| p.label())
            .or_else(|| current.clone())
            .unwrap_or_else(|| "<no port detected>".to_owned());

        ui.horizontal_wrapped(|ui| {
            ui.label("Port:");
            let mut new_selection: Option<String> = None;
            egui::ComboBox::from_id_salt("port_combo")
                .selected_text(current_label)
                .width(380.0)
                .show_ui(ui, |ui| {
                    if port_items.is_empty() {
                        ui.label("(no serial ports found)");
                    }
                    for (name, label, ids, known) in &port_items {
                        let selected = current.as_deref() == Some(name.as_str());
                        let text = if *known {
                            egui::RichText::new(label).strong()
                        } else {
                            egui::RichText::new(label)
                        };
                        ui.horizontal(|ui| {
                            if ui.selectable_label(selected, text).clicked() {
                                new_selection = Some(name.clone());
                            }
                            ui.label(egui::RichText::new(format!("[{ids}]")).weak().small());
                        });
                    }
                });
            if let Some(sel) = new_selection {
                self.conn.selected_port = Some(sel);
            }

            if ui.button("Refresh").clicked() {
                self.refresh_ports();
            }

            ui.separator();
            ui.label("Baud:");
            egui::ComboBox::from_id_salt("baud_combo")
                .selected_text(self.conn.baud.to_string())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for b in [9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1_000_000] {
                        ui.selectable_value(&mut self.conn.baud, b, b.to_string());
                    }
                })
                .response
                .on_hover_text("Ignored for USB devices — they enumerate as virtual COM ports.");

            ui.separator();
            if self.conn.connection.is_some() {
                if ui.button("Disconnect").clicked() {
                    self.disconnect();
                }
            } else {
                let enabled = self.conn.selected_port.is_some();
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(
                            egui::RichText::new("Connect").strong().color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT),
                    )
                    .clicked()
                {
                    self.connect(ctx);
                }
            }

            ui.checkbox(&mut self.conn.line_ending_crlf, "CRLF")
                .on_hover_text("Append \\r\\n instead of \\n to each command");
        });

        ui.add_space(2.0);

        let detected = self.current_port_info().and_then(|p| p.guessed_mode);
        ui.horizontal(|ui| {
            ui.label("Mode:");
            ui.selectable_value(&mut self.mode, Mode::Tracker, "Tracker");
            ui.selectable_value(&mut self.mode, Mode::Receiver, "Receiver");
            if let Some(m) = detected {
                ui.separator();
                ui.label(egui::RichText::new(format!("auto-detected: {}", m.label())).weak());
            }
        });

        // Details of the selected port, so it's clear what each COM/tty actually is.
        if let Some(info) = self.current_port_info() {
            let name = info.display_name();
            let ids = info.ids();
            let serial = info.serial_number.clone();
            let boot = info.bootloader;
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Device:").color(MUTED));
                ui.label(egui::RichText::new(name).strong());
                ui.label(egui::RichText::new(format!("· USB {ids}")).color(MUTED));
                if let Some(sn) = serial {
                    if !sn.is_empty() {
                        ui.label(egui::RichText::new(format!("· s/n {sn}")).color(MUTED));
                    }
                }
            });
            match boot {
                Some(crate::serial::BootloaderKind::NordicDfu) => {
                    ui.label(
                        egui::RichText::new(
                            "Receiver is in DFU mode — use the \"Update receiver firmware\" panel below. (The serial console won't respond here.)",
                        )
                        .color(ACCENT_AMBER),
                    );
                }
                Some(crate::serial::BootloaderKind::Uf2) => {
                    ui.label(
                        egui::RichText::new(
                            "This board is in UF2 bootloader mode — use the firmware updater, not the console.",
                        )
                        .color(ACCENT_AMBER),
                    );
                }
                None => {}
            }
        }

        ui.add_space(6.0);
    }
}
