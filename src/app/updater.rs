use super::*;
use crate::services::updater_service::{
    self, UpdateStatus, update_download_final_path, update_download_ready_path,
};
use eframe::egui::{Align2, Area, Color32, Frame, Order, ProgressBar, RichText, Stroke};
use std::sync::atomic::Ordering;

impl SoundFxApp {
    pub(crate) fn check_for_update(&mut self, ctx: &Context, automatic: bool) {
        if matches!(
            self.update_status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return;
        }

        self.update_status = UpdateStatus::Checking;
        let tx = self.update_tx.clone();
        let current_version = env!("CARGO_PKG_VERSION").to_owned();

        updater_service::check_for_update(current_version, move |result| {
            match result {
                Ok(Some(manifest)) => {
                    let _ = tx.send(UpdateActionMessage::Available {
                        version: manifest.version,
                        notes: manifest.notes,
                        url: manifest.url,
                    });
                }
                Ok(None) => {
                    let _ = tx.send(UpdateActionMessage::UpToDate);
                }
                Err(e) => {
                    // On automatic check at startup, do not spam errors if user is offline
                    if !automatic {
                        let _ = tx.send(UpdateActionMessage::Error(e));
                    } else {
                        let _ = tx.send(UpdateActionMessage::UpToDate);
                    }
                }
            }
        });

        ctx.request_repaint();
    }

    pub(crate) fn start_download_update(
        &mut self,
        ctx: &Context,
        version: String,
        download_url: String,
    ) {
        self.update_status = UpdateStatus::Downloading {
            version: version.clone(),
            progress: 0.0,
        };
        self.update_download_progress.store(0, Ordering::SeqCst);
        self.update_download_cancel.store(false, Ordering::SeqCst);

        let progress = self.update_download_progress.clone();
        let cancel = self.update_download_cancel.clone();
        let tx = self.update_tx.clone();
        let ver = version.clone();

        updater_service::start_download_update(version, download_url, progress, cancel, move |result| {
            match result {
                Ok(path) => {
                    let _ = tx.send(UpdateActionMessage::Downloaded {
                        version: ver,
                        path,
                    });
                }
                Err(e) => {
                    let _ = tx.send(UpdateActionMessage::Error(e));
                }
            }
        });

        ctx.request_repaint();
    }

    pub(crate) fn restart_and_apply_update(&mut self, new_exe_path: &Path) {
        if let Err(e) = updater_service::restart_and_apply_update(new_exe_path) {
            self.update_status = UpdateStatus::Error(format!("Failed to restart: {e}"));
        }
    }

    pub(crate) fn poll_updater_messages(&mut self, ctx: &Context) {
        while let Ok(msg) = self.update_rx.try_recv() {
            match msg {
                UpdateActionMessage::Available {
                    version,
                    notes,
                    url,
                } => {
                    let downloaded_exe = update_download_final_path(&version);
                    let ready_stamp = update_download_ready_path(&version);
                    if downloaded_exe.exists() && ready_stamp.exists() {
                        self.update_status = UpdateStatus::ReadyToRestart {
                            version: version.clone(),
                            new_exe_path: downloaded_exe,
                        };
                    } else {
                        self.update_status = UpdateStatus::Available {
                            version: version.clone(),
                            notes: notes.clone(),
                            url: url.clone(),
                        };
                    }

                    self.show_update_notice(version, notes, url);
                    ctx.request_repaint();
                }
                UpdateActionMessage::UpToDate => {
                    if self.update_status == UpdateStatus::Checking {
                        self.update_status = UpdateStatus::UpToDate;
                    }
                    ctx.request_repaint();
                }
                UpdateActionMessage::Downloaded { version, path } => {
                    self.update_status = UpdateStatus::ReadyToRestart {
                        version,
                        new_exe_path: path.clone(),
                    };
                    ctx.request_repaint();

                    // "nhấn một nút là tự cài tự mở luôn"
                    // Automatically execute restart & update when download finishes!
                    self.restart_and_apply_update(&path);
                }
                UpdateActionMessage::Error(e) => {
                    self.update_status = UpdateStatus::Error(e);
                    ctx.request_repaint();
                }
            }
        }

        // Sync download progress if currently downloading
        if let UpdateStatus::Downloading { ref version, .. } = self.update_status {
            let progress_val = self.update_download_progress.load(Ordering::Relaxed) as f32 / 1000.0;
            self.update_status = UpdateStatus::Downloading {
                version: version.clone(),
                progress: progress_val,
            };
            ctx.request_repaint();
        }
    }

    pub(crate) fn show_update_notice(
        &mut self,
        version: String,
        notes: String,
        download_url: String,
    ) {
        self.update_notice = Some(UpdateNotice {
            version,
            notes,
            download_url,
            expires_at: Instant::now() + Duration::from_secs(120),
        });
    }

    pub(crate) fn render_update_notice(&mut self, ctx: &Context) {
        let Some(notice) = self.update_notice.clone() else {
            return;
        };

        if Instant::now() >= notice.expires_at
            && !matches!(self.update_status, UpdateStatus::Downloading { .. })
        {
            self.update_notice = None;
            return;
        }

        let mut dismiss = false;
        let mut trigger_download = false;
        let mut trigger_restart: Option<PathBuf> = None;

        Area::new(egui::Id::new("app_update_floating_notice"))
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-24.0, -36.0))
            .interactable(true)
            .show(ctx, |ui| {
                let fill = Self::overlay_panel_fill();
                let stroke = Stroke::new(1.2, Color32::from_rgb(0, 180, 216));

                Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(20.0)
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(0, 0, 0, 90),
                    })
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_max_width(340.0);

                        // Header row
                        ui.horizontal(|ui| {
                            ui.label(Self::icon(0xe8d7, 20.0, Color32::from_rgb(0, 180, 216)).strong());
                            ui.add_space(4.0);
                            let title_text = format!("{} v{}", self.t("settings.new_version_available").replace("{version}", ""), notice.version);
                            ui.label(
                                RichText::new(title_text)
                                    .size(13.5)
                                    .strong()
                                    .color(Self::strong_text_color()),
                            );

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if Self::icon_titlebar(ui, [24.0, 24.0], 0xe5cd, false, false).clicked() {
                                    dismiss = true;
                                }
                            });
                        });

                        // Notes if available
                        if !notice.notes.trim().is_empty() {
                            ui.add_space(6.0);
                            let preview_notes: String = notice.notes.lines().take(3).collect::<Vec<_>>().join(" ");
                            let truncated = if preview_notes.len() > 140 {
                                format!("{}...", &preview_notes[..140])
                            } else {
                                preview_notes
                            };
                            ui.label(
                                RichText::new(truncated)
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
                            );
                        }

                        ui.add_space(10.0);

                        // Status and actions
                        match &self.update_status {
                            UpdateStatus::Downloading { progress, .. } => {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(format!(
                                                "{} ({:.0}%)",
                                                self.t("settings.downloading_update"),
                                                progress * 100.0
                                            ))
                                            .size(12.0)
                                            .color(Color32::from_rgb(0, 180, 216)),
                                        );
                                    });
                                    ui.add_space(4.0);
                                    ui.add(
                                        ProgressBar::new(*progress)
                                            .desired_width(ui.available_width().max(200.0))
                                            .show_percentage(),
                                    );
                                });
                            }
                            UpdateStatus::ReadyToRestart { new_exe_path, .. } => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new(self.t("settings.ready_to_restart"))
                                            .size(12.0)
                                            .color(Color32::from_rgb(72, 199, 142)),
                                    );
                                });
                                ui.add_space(6.0);
                                let restart_btn = ui.add_sized(
                                    [ui.available_width(), 32.0],
                                    Self::action_button(
                                        RichText::new(self.t("settings.restart_to_update")).size(12.5),
                                        false,
                                        true,
                                    ),
                                );
                                Self::decorate_button_response(ui, &restart_btn);
                                if restart_btn.clicked() {
                                    trigger_restart = Some(new_exe_path.clone());
                                }
                            }
                            UpdateStatus::Error(err) => {
                                ui.label(
                                    RichText::new(format!("Lỗi: {err}"))
                                        .size(11.5)
                                        .color(Color32::from_rgb(230, 80, 80)),
                                );
                                ui.add_space(6.0);
                                let retry_btn = ui.add_sized(
                                    [ui.available_width(), 30.0],
                                    Self::action_button(
                                        RichText::new(self.t("settings.update_now")).size(12.0),
                                        false,
                                        true,
                                    ),
                                );
                                Self::decorate_button_response(ui, &retry_btn);
                                if retry_btn.clicked() {
                                    trigger_download = true;
                                }
                            }
                            _ => {
                                let update_btn = ui.add_sized(
                                    [ui.available_width(), 32.0],
                                    Self::action_button(
                                        RichText::new(format!("🚀 {}", self.t("settings.update_now"))).size(12.5),
                                        false,
                                        true,
                                    ),
                                );
                                Self::decorate_button_response(ui, &update_btn);
                                if update_btn.clicked() {
                                    trigger_download = true;
                                }
                            }
                        }
                    });
            });

        if dismiss {
            self.update_notice = None;
        }

        if trigger_download {
            self.start_download_update(ctx, notice.version, notice.download_url);
        }

        if let Some(path) = trigger_restart {
            self.restart_and_apply_update(&path);
        }
    }
}
