use super::*;

impl SoundFxApp {
    pub(super) fn available_import_roots() -> Vec<PathBuf> {
        ["C:\\", "D:\\"]
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .collect()
    }

    pub(super) fn set_import_dir(&mut self, path: Option<PathBuf>) {
        self.import_dir = path.unwrap_or_default();
        let _ = self.storage.save_import_dir(
            (!self.import_dir.as_os_str().is_empty()).then_some(self.import_dir.as_path()),
        );
        self.refresh_import_audio_entries();
    }

    pub(super) fn refresh_pitch_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.pitch_capture_devices = devices;
                if !self
                    .selected_pitch_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.pitch_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_pitch_input_device = self.pitch_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn refresh_record_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.record_capture_devices = devices;
                if !self
                    .selected_record_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.record_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_record_input_device =
                        self.record_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn refresh_stream_input_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.stream_input_capture_devices = devices;
                if !self
                    .selected_stream_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.stream_input_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_stream_input_device =
                        self.stream_input_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn begin_async_stream_driver_probe(&mut self) {
        self.stream_driver_checked = false;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = Ok(crate::stream_driver::is_stream_driver_installed());
            let _ = tx.send(StreamDriverMessage::ProbeFinished(result));
        });
    }

    pub(super) fn start_stream_driver_install(&mut self, ctx: &Context) {
        if self.stream_driver_busy || self.stream_driver_installed {
            return;
        }
        self.stream_driver_busy = true;
        self.stream_driver_error = None;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = crate::stream_driver::install_stream_driver()
                .map(|_| crate::stream_driver::is_stream_driver_installed());
            let _ = tx.send(StreamDriverMessage::Finished(result));
        });
        ctx.request_repaint();
    }

    pub(super) fn start_stream_driver_uninstall(&mut self, ctx: &Context) {
        if self.stream_driver_busy {
            return;
        }
        let _ = self.stream_input_router.configure(None);
        self.stream_driver_busy = true;
        self.stream_driver_error = None;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = crate::stream_driver::uninstall_stream_driver().map(|_| false);
            let _ = tx.send(StreamDriverMessage::Finished(result));
        });
        ctx.request_repaint();
    }

    pub(super) fn apply_stream_input_routing(&mut self) {
        let mic_available = self.selected_stream_input_device.is_some()
            || !self.stream_input_capture_devices.is_empty();
        let monitor_microphone = self.stream_input_monitor_microphone && mic_available;
        let route_system_audio = self.stream_input_system_audio && !monitor_microphone;
        let route_microphone = self.stream_input_microphone && mic_available;
        let can_route_virtual_mic =
            self.stream_driver_installed && (route_system_audio || route_microphone);
        let wants_local_mic_monitor = monitor_microphone;
        let config =
            if !self.stream_driver_busy && (can_route_virtual_mic || wants_local_mic_monitor) {
                Some(StreamInputConfig {
                    route_system_audio: self.stream_driver_installed && route_system_audio,
                    route_microphone: self.stream_driver_installed && route_microphone,
                    monitor_microphone,
                    microphone_device_name: self.selected_stream_input_device.clone(),
                })
            } else {
                None
            };

        if let Err(error) = self.stream_input_router.configure(config) {
            self.set_error_status(error);
        }
    }

    pub(super) fn gemini_api_key_field(
        ui: &mut Ui,
        label: &str,
        api_key: &mut String,
        visible: &mut bool,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label)
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let toggle = Self::icon_titlebar(
                    ui,
                    [34.0, 28.0],
                    if *visible { 0xe8f5 } else { 0xe8f4 },
                    *visible,
                    false,
                )
                .on_hover_text(if *visible {
                    "Hide API key"
                } else {
                    "Show API key"
                });
                if toggle.clicked() {
                    *visible = !*visible;
                }
            });
        });
        ui.add_space(8.0);
        let response = Self::with_input_widget_visuals(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), 32.0],
                TextEdit::singleline(api_key)
                    .desired_width(f32::INFINITY)
                    .hint_text("AIza...")
                    .password(!*visible),
            )
        });
        if response.changed() {
            changed = true;
        }
        changed
    }

    pub(super) fn poll_stream_driver_result(&mut self, ctx: &Context) {
        while let Ok(message) = self.stream_driver_rx.try_recv() {
            match message {
                StreamDriverMessage::ProbeFinished(Ok(installed)) => {
                    self.stream_driver_checked = true;
                    self.stream_driver_installed = installed;
                    self.stream_driver_error = None;
                }
                StreamDriverMessage::ProbeFinished(Err(error)) => {
                    self.stream_driver_checked = true;
                    self.stream_driver_error = Some(error);
                }
                StreamDriverMessage::Finished(Ok(installed)) => {
                    self.stream_driver_busy = false;
                    self.stream_driver_checked = true;
                    self.stream_driver_installed = installed;
                    self.stream_driver_error = None;
                    self.status = Some(if installed {
                        "stream driver installed".to_owned()
                    } else {
                        "stream driver removed".to_owned()
                    });
                }
                StreamDriverMessage::Finished(Err(error)) => {
                    self.stream_driver_busy = false;
                    self.stream_driver_checked = true;
                    self.stream_driver_error = Some(error);
                    self.begin_async_stream_driver_probe();
                }
            }
            self.apply_stream_input_routing();
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_stream_input_router(&mut self, ctx: &Context) {
        let snapshot = self.stream_input_router.snapshot();
        if snapshot.running || snapshot.error.is_some() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if let Some(error) = snapshot.error {
            self.set_error_status(error);
        }
    }

    pub(super) fn apply_theme(ctx: &Context, dark_theme: bool) {
        Self::set_theme_enabled(dark_theme);

        let mut style = (*ctx.style()).clone();
        style.visuals = if dark_theme {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        style.visuals.panel_fill = if dark_theme {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.window_fill = if dark_theme {
            Color32::from_rgb(20, 17, 24)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.extreme_bg_color = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.faint_bg_color = if dark_theme {
            Color32::from_rgb(33, 28, 39)
        } else {
            Color32::from_rgb(244, 239, 246)
        };
        style.visuals.widgets.noninteractive.bg_fill = if dark_theme {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.noninteractive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(71, 57, 78)
        } else {
            Color32::from_rgb(229, 220, 228)
        };
        style.visuals.widgets.noninteractive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(236, 233, 240)
        } else {
            Color32::from_rgb(233, 226, 234)
        };
        style.visuals.widgets.inactive.bg_fill = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.inactive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(82, 67, 90)
        } else {
            Color32::from_rgb(226, 216, 225)
        };
        style.visuals.widgets.inactive.bg_stroke.width = 1.2;
        style.visuals.widgets.inactive.fg_stroke.color = if dark_theme {
            Color32::from_rgb(236, 230, 239)
        } else {
            Color32::from_rgb(76, 58, 85)
        };
        style.visuals.widgets.inactive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(245, 242, 248)
        } else {
            Color32::from_rgb(226, 218, 228)
        };
        style.visuals.widgets.hovered.bg_fill = if dark_theme {
            Color32::from_rgb(53, 36, 52)
        } else {
            Color32::from_rgb(255, 236, 246)
        };
        style.visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
        style.visuals.widgets.hovered.bg_stroke.width = 1.2;
        style.visuals.widgets.hovered.fg_stroke.color = Color32::from_rgb(255, 252, 255);
        style.visuals.widgets.hovered.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 248, 252)
        } else {
            Color32::from_rgb(240, 229, 239)
        };
        style.visuals.widgets.active.bg_fill = if dark_theme {
            Color32::from_rgb(78, 30, 68)
        } else {
            Color32::from_rgb(255, 223, 239)
        };
        style.visuals.widgets.active.bg_stroke.color = Color32::from_rgb(214, 51, 132);
        style.visuals.widgets.active.bg_stroke.width = 1.2;
        style.visuals.widgets.active.fg_stroke.color = Color32::from_rgb(255, 252, 255);
        style.visuals.widgets.active.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 251, 253)
        } else {
            Color32::from_rgb(245, 233, 243)
        };
        style.visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.visuals.hyperlink_color = Color32::from_rgb(214, 51, 132);
        style.visuals.window_shadow.color = if dark_theme {
            Color32::from_rgba_premultiplied(0, 0, 0, 96)
        } else {
            Color32::from_rgba_premultiplied(68, 27, 56, 48)
        };
        style.interaction.selectable_labels = false;
        style.interaction.show_tooltips_only_when_still = false;
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.8;
        ctx.set_style(style);
    }

    pub(super) fn page_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 248, 248)
        }
    }

    pub(super) fn surface_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(24, 20, 29)
        } else {
            Color32::WHITE
        }
    }

    pub(super) fn panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(29, 25, 35)
        } else {
            Color32::from_rgb(252, 248, 251)
        }
    }

    pub(super) fn border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(78, 64, 87)
        } else {
            Color32::from_rgb(229, 220, 228)
        }
    }

    pub(super) fn subtle_border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(88, 72, 96)
        } else {
            Color32::from_rgb(237, 226, 234)
        }
    }

    pub(super) fn strong_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(246, 233, 241)
        } else {
            Color32::from_rgb(40, 35, 41)
        }
    }

    pub(super) fn muted_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(191, 174, 189)
        } else {
            Color32::from_rgb(118, 106, 116)
        }
    }

    pub(super) fn t(&self, key: &str) -> String {
        self.localization.text(key)
    }

    pub(super) fn render_settings_panel(&mut self, ctx: &Context) {
        if !self.show_settings_panel {
            return;
        }

        let mut open_panel = self.show_settings_panel;
        let mut close_request = false;
        let mut save_startup = false;
        let mut clear_startup = false;
        let mut reset_startup = false;
        let mut animation_changed = false;
        let mut install_stream_driver = false;
        let mut uninstall_stream_driver = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(520.0, 700.0), vec2(320.0, 360.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("settings-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_size(panel_size)
            .fixed_pos(panel_pos)
            .open(&mut open_panel)
            .frame(
                Frame::new()
                    .fill(Self::overlay_panel_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 32,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe8b8, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        let animation_label = self.t("settings.animation");
                        let response = ui.checkbox(
                            &mut self.app_transition_animation,
                            RichText::new(animation_label)
                                .size(13.0)
                                .color(Self::strong_text_color()),
                        );
                        if response.changed() {
                            animation_changed = true;
                        }
                        if self.app_transition_animation {
                            ui.add_space(12.0);
                            Self::draw_settings_sound_row(
                                ui,
                                &self.sounds,
                                &self.t("settings.startup_sound"),
                                &mut self.settings_startup_candidate,
                                &self.startup_sound_name,
                                "settings-startup-combo",
                                &mut save_startup,
                                &mut clear_startup,
                                &mut reset_startup,
                            );
                        }
                    });

                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.t("settings.stream_mic_driver"))
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(if self.stream_driver_busy {
                                        self.t("settings.preparing")
                                    } else if !self.stream_driver_checked {
                                        self.t("stream.checking")
                                    } else if self.stream_driver_installed {
                                        self.t("settings.installed")
                                    } else {
                                        self.t("settings.not_installed")
                                    })
                                    .size(12.0)
                                    .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(self.t("settings.stream_driver_description"))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let install = ui.add_enabled(
                                !self.stream_driver_busy && !self.stream_driver_installed,
                                Self::action_button(
                                    RichText::new(self.t("settings.install_stream_driver"))
                                        .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &install);
                            if install.clicked() {
                                install_stream_driver = true;
                            }

                            let uninstall = ui.add_enabled(
                                !self.stream_driver_busy,
                                Self::action_button(
                                    RichText::new(self.t("settings.remove_stream_driver"))
                                        .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &uninstall);
                            if uninstall.clicked() {
                                uninstall_stream_driver = true;
                            }
                        });
                        if self.stream_driver_busy {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    RichText::new(self.t("settings.updating_stream_driver"))
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        }
                        if let Some(error) = self.stream_driver_error.as_ref() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(error)
                                    .size(11.5)
                                    .color(Color32::from_rgb(171, 54, 91)),
                            );
                        }
                    });

                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(Self::icon(0xe8d7, 18.0, Color32::from_rgb(0, 180, 216)).strong());
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(self.t("settings.update"))
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);

                        match &self.update_status {
                            crate::services::updater_service::UpdateStatus::Idle => {
                                let check_btn = ui.add(Self::action_button(
                                    RichText::new(self.t("settings.check_update")).size(12.0),
                                    false,
                                    false,
                                ));
                                Self::decorate_button_response(ui, &check_btn);
                                if check_btn.clicked() {
                                    self.check_for_update(ctx, false);
                                }
                            }
                            crate::services::updater_service::UpdateStatus::Checking => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new(self.t("settings.checking_update"))
                                            .size(12.0)
                                            .color(Self::muted_text_color()),
                                    );
                                });
                            }
                            crate::services::updater_service::UpdateStatus::UpToDate => {
                                ui.label(
                                    RichText::new(self.t("settings.up_to_date"))
                                        .size(12.0)
                                        .color(Color32::from_rgb(72, 199, 142)),
                                );
                                ui.add_space(6.0);
                                let check_btn = ui.add(Self::action_button(
                                    RichText::new(self.t("settings.check_update")).size(12.0),
                                    false,
                                    false,
                                ));
                                Self::decorate_button_response(ui, &check_btn);
                                if check_btn.clicked() {
                                    self.check_for_update(ctx, false);
                                }
                            }
                            crate::services::updater_service::UpdateStatus::Available {
                                version,
                                notes,
                                url,
                            } => {
                                let ver = version.clone();
                                let download_url = url.clone();
                                ui.label(
                                    RichText::new(format!("{} v{}", self.t("settings.new_version_available").replace("{version}", ""), ver))
                                        .size(12.5)
                                        .color(Color32::from_rgb(0, 180, 216))
                                        .strong(),
                                );
                                if !notes.trim().is_empty() {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(notes.trim())
                                            .size(11.5)
                                            .color(Self::muted_text_color()),
                                    );
                                }
                                ui.add_space(8.0);
                                let update_btn = ui.add(Self::action_button(
                                    RichText::new(format!("🚀 {}", self.t("settings.update_now"))).size(12.5),
                                    false,
                                    true,
                                ));
                                Self::decorate_button_response(ui, &update_btn);
                                if update_btn.clicked() {
                                    self.start_download_update(ctx, ver, download_url);
                                }
                            }
                            crate::services::updater_service::UpdateStatus::Downloading {
                                progress,
                                ..
                            } => {
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
                                        .desired_width(ui.available_width().max(180.0))
                                        .show_percentage(),
                                );
                            }
                            crate::services::updater_service::UpdateStatus::ReadyToRestart {
                                new_exe_path,
                                ..
                            } => {
                                ui.label(
                                    RichText::new(self.t("settings.ready_to_restart"))
                                        .size(12.0)
                                        .color(Color32::from_rgb(72, 199, 142)),
                                );
                                ui.add_space(6.0);
                                let path = new_exe_path.clone();
                                let restart_btn = ui.add(Self::action_button(
                                    RichText::new(self.t("settings.restart_to_update")).size(12.5),
                                    false,
                                    true,
                                ));
                                Self::decorate_button_response(ui, &restart_btn);
                                if restart_btn.clicked() {
                                    self.restart_and_apply_update(&path);
                                }
                            }
                            crate::services::updater_service::UpdateStatus::Error(e) => {
                                ui.label(
                                    RichText::new(format!("Lỗi: {e}"))
                                        .size(11.5)
                                        .color(Color32::from_rgb(230, 80, 80)),
                                );
                                ui.add_space(6.0);
                                let check_btn = ui.add(Self::action_button(
                                    RichText::new(self.t("settings.check_update")).size(12.0),
                                    false,
                                    false,
                                ));
                                Self::decorate_button_response(ui, &check_btn);
                                if check_btn.clicked() {
                                    self.check_for_update(ctx, false);
                                }
                            }
                        }
                    });
            });

        self.show_settings_panel = open_panel;
        if close_request {
            self.show_settings_panel = false;
        }
        if animation_changed {
            let _ = self
                .storage
                .save_app_transition_animation(self.app_transition_animation);
            if !self.app_transition_animation {
                self.startup.phase = TransitionPhase::Live;
                self.startup.started_at = None;
                self.startup.live_started_at = None;
                self.startup.duration_sec = 0.0;
                self.startup_sound_played = true;
            }
        }
        if install_stream_driver {
            self.start_stream_driver_install(ctx);
        }
        if uninstall_stream_driver {
            self.start_stream_driver_uninstall(ctx);
        }

        if save_startup
            && let Some(sound_id) = self.settings_startup_candidate
            && let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            match self.storage.save_startup_sound(sound) {
                Ok(()) => {
                    let resolved = self.storage.resolved_startup_sound_path().ok().flatten();
                    self.startup_sound_name = Some(sound.name.clone());
                    let visual =
                        Self::load_transition_sound_visual(&self.storage, resolved.clone());
                    self.startup.sound_waveform = visual.0;
                    self.startup.sound_duration_sec = visual.1;
                    self.startup.duration_sec = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_INTRO_DURATION_SEC,
                    );
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if clear_startup {
            match self.storage.clear_startup_sound() {
                Ok(()) => {
                    self.startup_sound_name = None;
                    self.startup.sound_waveform.clear();
                    self.startup.sound_duration_sec = 0.0;
                    self.startup.duration_sec = DEFAULT_INTRO_DURATION_SEC;
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if reset_startup {
            match self.storage.reset_startup_sound() {
                Ok(()) => {
                    let resolved = self.storage.resolved_startup_sound_path().ok().flatten();
                    self.startup_sound_name = self.storage.load_startup_sound_name().ok().flatten();
                    let visual =
                        Self::load_transition_sound_visual(&self.storage, resolved.clone());
                    self.startup.sound_waveform = visual.0;
                    self.startup.sound_duration_sec = visual.1;
                    self.startup.duration_sec = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_INTRO_DURATION_SEC,
                    );
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
    }
}
