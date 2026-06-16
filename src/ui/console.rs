//! View rendering, split out of `app.rs`. These are `impl App` methods that live
//! in their own module for navigability; behaviour is unchanged.

use eframe::egui;

use crate::app::App;
use crate::console::Kind;
use crate::theme::MUTED;


impl App {
    pub(crate) fn console_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("console_header").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Console");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        self.console.log.clear();
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Show:").color(MUTED));
                ui.checkbox(&mut self.console.show_tx, "sent")
                    .on_hover_text("Echo the commands you send");
                ui.checkbox(&mut self.console.show_err, "errors")
                    .on_hover_text("Failures and <err> log lines");
                ui.checkbox(&mut self.console.show_warn, "warnings")
                    .on_hover_text("Validation notices and <wrn> log lines (off by default)");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.console.autoscroll, "auto-scroll");
                });
            });
            ui.add_space(2.0);
        });

        egui::Panel::bottom("console_input").show_inside(ui, |ui| {
            ui.add_space(4.0);
            let connected = self.is_connected();
            ui.add_enabled_ui(connected, |ui| {
                ui.horizontal(|ui| {
                    let hint = if connected {
                        "raw command — Enter to send"
                    } else {
                        "connect a device to send commands"
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.console.raw_input)
                            .desired_width(ui.available_width() - 64.0)
                            .hint_text(hint),
                    );
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.button("Send").clicked();
                    if enter || clicked {
                        let cmd = self.console.raw_input.trim().to_owned();
                        if !cmd.is_empty() {
                            self.send_cmd(cmd);
                        }
                        self.console.raw_input.clear();
                        resp.request_focus();
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(self.console.autoscroll)
                .show(ui, |ui| {
                    // Tighter, terminal-like line spacing than the global default,
                    // so log output reads as a cohesive block.
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if self.console.log.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "No output yet. Connect, then try Calibrate — or type `help`.",
                            )
                            .color(MUTED),
                        );
                    }
                    let maxw = ui.available_width();
                    let mono = egui::FontId::monospace(13.0);
                    for line in &self.console.log {
                        match line.kind {
                            Kind::Tx if !self.console.show_tx => continue,
                            Kind::Warn if !self.console.show_warn => continue,
                            Kind::Err if !self.console.show_err => continue,
                            _ => {}
                        }
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = maxw;
                        let prefix = line.kind.prefix();
                        if !prefix.is_empty() {
                            job.append(
                                prefix,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: mono.clone(),
                                    color: line.kind.default_color(),
                                    ..Default::default()
                                },
                            );
                        }
                        for seg in &line.segments {
                            let color = seg.color.unwrap_or_else(|| line.kind.default_color());
                            job.append(
                                &seg.text,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: mono.clone(),
                                    color,
                                    ..Default::default()
                                },
                            );
                        }
                        ui.label(job);
                    }
                });
        });
    }
}
