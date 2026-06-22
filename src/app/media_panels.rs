use super::*;

impl SoundFxApp {
    pub(super) fn render_record_panel(&mut self, ctx: &Context) {
        if !self.show_record_panel {
            return;
        }

        let snapshot = self.recorder.snapshot();
        if snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        let mut open_panel = self.show_record_panel;
        let mut close_request = false;
        let mut toggle_record = false;
        let mut use_selected_sound = false;
        let refresh_inputs = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(520.0, 420.0), vec2(320.0, 260.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("sound-record-panel"))
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
                        blur: 30,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe061, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(18.0)
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.record_name)
                                .desired_width(f32::INFINITY)
                                .hint_text("recording"),
                        );
                    });

                ui.add_space(12.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    ui.horizontal(|ui| {
                        let keyboard_response = Self::icon_action(
                            ui,
                            [42.0, 34.0],
                            0xe312,
                            self.capture_record_hotkey,
                            self.capture_record_hotkey,
                        );
                        if keyboard_response.clicked() {
                            if self.capture_record_hotkey {
                                self.capture_record_hotkey = false;
                                self.preview_record_hotkey = None;
                            } else {
                                self.capture_record_hotkey = true;
                                self.capture_pitch_hotkey = false;
                                self.preview_record_hotkey = None;
                            }
                        }

                        if self.capture_record_hotkey {
                            if let Some(preview_key) = self.preview_record_hotkey {
                                ui.add_space(6.0);
                                Frame::new()
                                    .fill(Color32::from_rgba_premultiplied(80, 70, 30, 255))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(255, 220, 80)))
                                    .corner_radius(12.0)
                                    .inner_margin(Margin::symmetric(10, 5))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "Pressing: {}",
                                                preview_key.to_string()
                                            ))
                                            .size(11.5)
                                            .color(Color32::from_rgb(255, 232, 96))
                                            .strong(),
                                        );
                                    });
                            } else {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Press key...")
                                        .size(12.5)
                                        .color(Self::muted_text_color()),
                                );
                            }
                        }

                        let mut key_to_remove = None;
                        for &key in &self.record_hotkeys {
                            ui.add_space(4.0);
                            let key_text = key.to_string();
                            let chip_btn = Button::new(
                                RichText::new(key_text)
                                    .size(11.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .fill(Self::surface_fill())
                            .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                            .corner_radius(12.0);

                            let response = ui.add(chip_btn);
                            Self::decorate_button_response(ui, &response);
                            if response.clicked() {
                                key_to_remove = Some(key);
                            }
                            if response.hovered() {
                                response.on_hover_text("Click to remove this hotkey");
                            }
                        }

                        if let Some(key) = key_to_remove {
                            self.record_hotkeys.retain(|&k| k != key);
                            let names: Vec<String> =
                                self.record_hotkeys.iter().map(|&k| k.to_string()).collect();
                            let _ = self.storage.save_record_hotkeys(&names);
                            if let Err(error) =
                                self.record_hotkey_manager.set_hotkeys(&self.record_hotkeys)
                            {
                                self.set_error_status(error);
                            }
                        }
                    });
                });

                ui.add_space(12.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    ui.horizontal(|ui| {
                        if Self::icon_action(
                            ui,
                            [48.0, 36.0],
                            0xe30a,
                            self.record_input_source == PitchInputSource::System,
                            self.record_input_source == PitchInputSource::System,
                        )
                        .clicked()
                        {
                            self.record_input_source = PitchInputSource::System;
                        }

                        if Self::icon_action(
                            ui,
                            [48.0, 36.0],
                            0xe029,
                            self.record_input_source == PitchInputSource::Microphone,
                            self.record_input_source == PitchInputSource::Microphone,
                        )
                        .clicked()
                        {
                            self.record_input_source = PitchInputSource::Microphone;
                        }
                    });
                });

                ui.add_space(10.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if self.record_input_source == PitchInputSource::Microphone {
                                Self::with_dark_combo_visuals(ui, |ui| {
                                    ComboBox::from_id_salt("record-input-device")
                                        .width(ui.available_width() - 4.0)
                                        .selected_text(
                                            RichText::new(
                                                self.selected_record_input_device
                                                    .as_deref()
                                                    .map(|name| {
                                                        Self::truncate_middle_ascii(name, 28)
                                                    })
                                                    .unwrap_or_else(|| "No mic".to_owned()),
                                            )
                                            .color(Self::strong_text_color()),
                                        )
                                        .show_ui(ui, |ui| {
                                            for name in &self.record_capture_devices {
                                                ui.selectable_value(
                                                    &mut self.selected_record_input_device,
                                                    Some(name.clone()),
                                                    Self::truncate_middle_ascii(name, 38),
                                                );
                                            }
                                        });
                                });
                            } else {
                                ui.add_sized(
                                    [ui.available_width(), 20.0],
                                    egui::Label::new(
                                        RichText::new("System output")
                                            .size(13.0)
                                            .color(Self::muted_text_color()),
                                    ),
                                );
                            }
                        });
                });

                ui.add_space(14.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_height(172.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(16.0);
                            Self::draw_record_wave_strip(ui, &snapshot.waveform);
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(format_time(snapshot.elapsed_secs))
                                    .size(16.0)
                                    .color(Self::strong_text_color()),
                            );
                        });
                    });

                ui.add_space(14.0);
                ui.horizontal_centered(|ui| {
                    let icon = if snapshot.running { 0xe047 } else { 0xe061 };
                    let button = ui.add_sized(
                        [160.0, 42.0],
                        Self::action_button(
                            Self::icon(icon, 18.0, Color32::WHITE),
                            snapshot.running,
                            true,
                        ),
                    );
                    Self::decorate_button_response(ui, &button);
                    if button.clicked() {
                        toggle_record = true;
                    }

                    ui.add_space(8.0);
                    let use_selected = ui
                        .add_enabled_ui(
                            !snapshot.running && self.selected_sound_index().is_some(),
                            |ui| {
                                ui.add_sized(
                                    [164.0, 42.0],
                                    Self::action_button(
                                        RichText::new("Use selected sound").size(13.0),
                                        false,
                                        false,
                                    ),
                                )
                            },
                        )
                        .inner;
                    Self::decorate_button_response(ui, &use_selected);
                    if use_selected.clicked() {
                        use_selected_sound = true;
                    }
                });
            });

        if close_request {
            open_panel = false;
            if snapshot.running {
                self.stop_recording(Some(ctx));
            }
        }
        self.show_record_panel = open_panel;

        if refresh_inputs {
            self.refresh_record_capture_devices();
        }

        if toggle_record {
            self.toggle_recording(ctx);
        }
        if use_selected_sound {
            self.open_selected_sound_for_record_export();
        }
    }

    pub(super) fn render_stream_panel(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }
        if !self.show_stream_panel {
            return;
        }

        let mic_available = self.selected_stream_input_device.is_some()
            || !self.stream_input_capture_devices.is_empty();
        if !mic_available {
            self.stream_input_microphone = false;
            self.stream_input_monitor_microphone = false;
        }
        if self.stream_input_monitor_microphone && self.stream_input_system_audio {
            self.stream_input_system_audio = false;
        }

        let snapshot = self.stream_input_router.snapshot();
        let mut close_request = false;
        let mut open_panel = self.show_stream_panel;
        let mut install_stream_driver = false;
        let mut uninstall_stream_driver = false;
        let mut routing_changed = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(344.0, 458.0), vec2(300.0, 300.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("stream-input-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_size(panel_size)
            .fixed_pos(panel_pos)
            .frame(
                Frame::new()
                    .fill(Self::overlay_panel_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16)),
            )
            .open(&mut open_panel)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("title.stream_input"))
                            .size(16.0)
                            .color(Self::strong_text_color())
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(self.t("stream.description"))
                    .size(11.5)
                    .color(Self::muted_text_color()),
                );

                ui.add_space(10.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(18.0)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.t("stream.driver"))
                                    .size(12.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                let label = if self.stream_driver_busy {
                                    self.t("settings.preparing")
                                } else if !self.stream_driver_checked {
                                    self.t("stream.checking")
                                } else if self.stream_driver_installed {
                                    self.t("settings.installed")
                                } else {
                                    self.t("settings.not_installed")
                                };
                                ui.label(
                                    RichText::new(label)
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);
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

                            let remove = ui.add_enabled(
                                !self.stream_driver_busy,
                                Self::action_button(
                                    RichText::new(self.t("settings.remove_stream_driver"))
                                        .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &remove);
                            if remove.clicked() {
                                uninstall_stream_driver = true;
                            }
                        });
                        if self.stream_driver_busy {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    RichText::new(self.t("settings.updating_stream_driver"))
                                        .size(11.0)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        }
                        if let Some(error) = self.stream_driver_error.as_ref() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(error)
                                    .size(11.0)
                                    .color(Color32::from_rgb(171, 54, 91)),
                            );
                        }
                    });

                ui.add_space(10.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(18.0)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.t("stream.route_to_virtual_mic"))
                                    .size(12.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                let routing_label = if snapshot.error.is_some() {
                                    self.t("stream.error")
                                } else if snapshot.running {
                                    self.t("stream.live")
                                } else {
                                    self.t("stream.idle")
                                };
                                ui.label(
                                    RichText::new(routing_label)
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(self.t("stream.description"))
                                .size(11.0)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(10.0);
                        let capture_system_audio_label = self.t("stream.capture_system_audio");
                        let capture_microphone_label = self.t("stream.capture_microphone");
                        let monitor_microphone_label = self.t("stream.monitor_microphone");
                        ui.add_enabled_ui(!self.stream_driver_busy, |ui| {
                            ui.add_enabled_ui(self.stream_driver_installed, |ui| {
                                let system_audio_response = ui.add_enabled(
                                    !self.stream_input_monitor_microphone,
                                    egui::Checkbox::new(
                                        &mut self.stream_input_system_audio,
                                        capture_system_audio_label,
                                    ),
                                );
                                routing_changed |= system_audio_response.changed();
                                routing_changed |= ui
                                    .add_enabled(
                                        mic_available,
                                        egui::Checkbox::new(
                                            &mut self.stream_input_microphone,
                                            capture_microphone_label,
                                        ),
                                    )
                                    .changed();
                            });
                            let before_monitor = self.stream_input_monitor_microphone;
                            routing_changed |= ui
                                .add_enabled(
                                    mic_available,
                                    egui::Checkbox::new(
                                        &mut self.stream_input_monitor_microphone,
                                        monitor_microphone_label,
                                    ),
                                )
                                .changed();
                            if !before_monitor
                                && self.stream_input_monitor_microphone
                                && self.stream_input_system_audio
                            {
                                self.stream_input_system_audio = false;
                                routing_changed = true;
                            }
                            ui.add_space(8.0);
                            if !self.stream_driver_installed
                                && (self.stream_input_system_audio || self.stream_input_microphone)
                            {
                                ui.label(
                                    RichText::new(self.t("stream.install_driver_first"))
                                        .size(11.0)
                                        .color(Self::muted_text_color()),
                                );
                                ui.add_space(6.0);
                            }
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(self.t("stream.mic_device"))
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                                let default_microphone_label = self.t("stream.default_microphone");
                                let before = self.selected_stream_input_device.clone();
                                Self::with_dark_combo_visuals(ui, |ui| {
                                    ui.add_enabled_ui(mic_available, |ui| {
                                        ComboBox::from_id_salt("stream-input-mic-device")
                                            .width(190.0)
                                            .selected_text(
                                                RichText::new(
                                                    self.selected_stream_input_device
                                                        .as_deref()
                                                        .map(|name| {
                                                            Self::truncate_middle_ascii(name, 28)
                                                        })
                                                        .unwrap_or(default_microphone_label),
                                                )
                                                .color(Self::strong_text_color()),
                                            )
                                            .show_ui(ui, |ui| {
                                                for name in &self.stream_input_capture_devices {
                                                    ui.selectable_value(
                                                        &mut self.selected_stream_input_device,
                                                        Some(name.clone()),
                                                        Self::truncate_middle_ascii(name, 38),
                                                    );
                                                }
                                            });
                                    });
                                });
                                if self.selected_stream_input_device != before {
                                    routing_changed = true;
                                }
                                if Self::icon_titlebar(ui, [30.0, 26.0], 0xe5d5, false, false)
                                    .clicked()
                                {
                                    self.refresh_stream_input_capture_devices();
                                }
                            });
                        });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(self.t("stream.pick_mic_hint"))
                                .size(11.0)
                                .color(Self::muted_text_color()),
                        );
                        if let Some(target_name) = snapshot.target_device_name.as_ref() {
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(format!("{}:", self.t("stream.target")))
                                        .size(11.0)
                                        .color(Self::muted_text_color()),
                                );
                                ui.label(
                                    RichText::new(target_name)
                                        .size(11.0)
                                        .color(Self::strong_text_color()),
                                );
                            });
                        }
                        if let Some(target_name) = snapshot.monitor_device_name.as_ref() {
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(format!("{}:", self.t("stream.monitor_target")))
                                        .size(11.0)
                                        .color(Self::muted_text_color()),
                                );
                                ui.label(
                                    RichText::new(target_name)
                                        .size(11.0)
                                        .color(Self::strong_text_color()),
                                );
                            });
                        }
                        ui.add_space(8.0);
                        let show_live_wave_label = self.t("stream.show_live_wave");
                        ui.checkbox(
                            &mut self.stream_input_show_waveform,
                            show_live_wave_label,
                        );
                        if snapshot.running {
                            ui.add_space(8.0);
                            let meter_level = Self::boost_stream_meter_level(snapshot.level);
                            ui.add(
                                ProgressBar::new(meter_level)
                                    .desired_width(ui.available_width())
                                    .show_percentage(),
                            );
                        }
                        if self.stream_input_show_waveform {
                            ui.add_space(8.0);
                            Self::draw_stream_wave_strip(ui, &snapshot.waveform, snapshot.running);
                        }
                        if let Some(error) = snapshot.error.as_ref() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(error)
                                    .size(11.0)
                                    .color(Color32::from_rgb(171, 54, 91)),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "Neu ban vua cai driver ma chua restart Windows, hay restart 1 lan roi mo app lai.",
                                )
                                .size(10.5)
                                .color(Self::muted_text_color()),
                            );
                        }
                    });
            });

        self.show_stream_panel = open_panel;
        if close_request {
            self.show_stream_panel = false;
        }
        if routing_changed {
            self.apply_stream_input_routing();
        }
        if install_stream_driver {
            self.start_stream_driver_install(ctx);
        }
        if uninstall_stream_driver {
            self.start_stream_driver_uninstall(ctx);
        }
    }

    pub(super) fn draw_settings_sound_row(
        ui: &mut Ui,
        sounds: &[SoundEffect],
        label: &str,
        candidate: &mut Option<Uuid>,
        current_name: &Option<String>,
        combo_id: &'static str,
        save_requested: &mut bool,
        clear_requested: &mut bool,
        reset_requested: &mut bool,
    ) {
        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(24.0)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(label)
                        .size(13.5)
                        .color(Self::strong_text_color())
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(current_name.clone().unwrap_or_else(|| "None".to_owned()))
                        .size(12.5)
                        .color(Self::muted_text_color()),
                );
                ui.add_space(10.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(18.0)
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.scope(|ui| {
                            if Self::dark_theme_enabled() {
                                let visuals = &mut ui.style_mut().visuals;
                                visuals.extreme_bg_color = Color32::from_rgb(28, 24, 33);
                                visuals.faint_bg_color = Color32::from_rgb(33, 28, 39);
                                visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 24, 33);
                                visuals.widgets.inactive.weak_bg_fill =
                                    Color32::from_rgb(28, 24, 33);
                                visuals.widgets.inactive.bg_stroke.color =
                                    Color32::from_rgb(88, 70, 96);
                                visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 35, 48);
                                visuals.widgets.hovered.weak_bg_fill =
                                    Color32::from_rgb(43, 35, 48);
                                visuals.widgets.hovered.bg_stroke.color =
                                    Color32::from_rgb(230, 94, 150);
                                visuals.widgets.active.bg_fill = Color32::from_rgb(53, 41, 58);
                                visuals.widgets.active.weak_bg_fill = Color32::from_rgb(53, 41, 58);
                                visuals.widgets.active.bg_stroke.color =
                                    Color32::from_rgb(230, 94, 150);
                                visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
                                visuals.selection.stroke.color = Color32::WHITE;
                                visuals.override_text_color =
                                    Some(Color32::from_rgb(246, 233, 241));
                            } else {
                                let visuals = &mut ui.style_mut().visuals;
                                visuals.extreme_bg_color = Color32::from_rgb(255, 251, 254);
                                visuals.faint_bg_color = Color32::from_rgb(247, 240, 246);
                                visuals.widgets.inactive.bg_fill = Color32::from_rgb(255, 251, 254);
                                visuals.widgets.inactive.weak_bg_fill =
                                    Color32::from_rgb(255, 251, 254);
                                visuals.widgets.inactive.bg_stroke.color =
                                    Color32::from_rgb(227, 214, 223);
                                visuals.widgets.hovered.bg_fill = Color32::from_rgb(255, 244, 250);
                                visuals.widgets.hovered.weak_bg_fill =
                                    Color32::from_rgb(255, 244, 250);
                                visuals.widgets.hovered.bg_stroke.color =
                                    Color32::from_rgb(230, 94, 150);
                                visuals.widgets.active.bg_fill = Color32::from_rgb(255, 238, 247);
                                visuals.widgets.active.weak_bg_fill =
                                    Color32::from_rgb(255, 238, 247);
                                visuals.widgets.active.bg_stroke.color =
                                    Color32::from_rgb(230, 94, 150);
                                visuals.selection.bg_fill =
                                    Color32::from_rgba_premultiplied(227, 82, 149, 48);
                                visuals.selection.stroke.color = Color32::from_rgb(79, 58, 72);
                                visuals.override_text_color = Some(Color32::from_rgb(52, 44, 51));
                            }

                            ComboBox::from_id_salt(combo_id)
                                .width(ui.available_width() - 4.0)
                                .selected_text(
                                    RichText::new(
                                        candidate
                                            .and_then(|id| {
                                                sounds.iter().find(|sound| sound.id == id).map(
                                                    |sound| {
                                                        Self::truncate_middle_ascii(&sound.name, 28)
                                                    },
                                                )
                                            })
                                            .unwrap_or_else(|| "Choose sound".to_owned()),
                                    )
                                    .color(Self::strong_text_color()),
                                )
                                .show_ui(ui, |ui| {
                                    for sound in sounds {
                                        ui.selectable_value(
                                            candidate,
                                            Some(sound.id),
                                            Self::truncate_middle_ascii(&sound.name, 34),
                                        );
                                    }
                                });
                        });
                    });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save = ui.add_sized(
                        [92.0, 34.0],
                        Self::action_button(RichText::new("Use").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &save);
                    if save.clicked() {
                        *save_requested = true;
                    }
                    let clear = ui.add_sized(
                        [92.0, 34.0],
                        Self::action_button(RichText::new("Clear").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &clear);
                    if clear.clicked() {
                        *clear_requested = true;
                    }
                    let reset = ui.add_sized(
                        [92.0, 34.0],
                        Self::action_button(RichText::new("Reset").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &reset);
                    if reset.clicked() {
                        *reset_requested = true;
                    }
                });
            });
    }

    pub(super) fn render_video_viewer_panel(&mut self, ctx: &Context) {
        let Some(viewer_snapshot) = self.video_viewer.as_ref().map(|viewer| {
            (
                viewer.video.clone(),
                viewer.audio_path.clone(),
                viewer.progress,
                viewer.frame_paths.len(),
            )
        }) else {
            return;
        };

        let (video, audio_path, stored_progress, frame_count) = viewer_snapshot;
        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing_file(&audio_path));
        let mut progress = self
            .audio
            .as_ref()
            .and_then(|audio| audio.playback_progress_for_file(&audio_path))
            .unwrap_or(stored_progress);
        if !is_playing {
            progress = progress.clamp(0.0, 1.0);
        } else {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        let frame_index = ((progress * frame_count.saturating_sub(1) as f32).round() as usize)
            .min(frame_count.saturating_sub(1));
        if let Err(error) = self.load_video_frame_texture(ctx, frame_index) {
            self.set_error_status(error);
        }
        if let Some(viewer) = self.video_viewer.as_mut() {
            viewer.progress = progress;
        }

        let frame_texture = self.video_viewer.as_ref().and_then(|viewer| {
            viewer
                .current_frame
                .as_ref()
                .map(|(_, texture, size)| (texture.clone(), *size))
        });

        let mut close_request = false;
        let mut toggle_play = false;
        let mut copy_request = false;
        let mut folder_request = false;
        let mut delete_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(760.0, 620.0), vec2(360.0, 320.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("video-viewer-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_size(panel_size)
            .fixed_pos(panel_pos)
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
                    .corner_radius(32.0)
                    .inner_margin(Margin::same(22)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe04b, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.label(
                        RichText::new(&video.name)
                            .size(15.0)
                            .color(Self::strong_text_color())
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(16.0);
                Frame::new()
                    .fill(Color32::BLACK)
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.set_min_height(430.0);
                            if let Some((texture, image_size)) = frame_texture.as_ref() {
                                let max_size = vec2(ui.available_width(), 430.0);
                                let scale = (max_size.x / image_size.x.max(1.0))
                                    .min(max_size.y / image_size.y.max(1.0))
                                    .max(0.1);
                                ui.image((texture.id(), *image_size * scale));
                            } else {
                                ui.add_space(160.0);
                                ui.label(
                                    RichText::new("Loading video")
                                        .size(15.0)
                                        .color(Color32::from_rgb(255, 222, 236)),
                                );
                            }
                        });
                    });

                ui.add_space(16.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(24.0)
                    .inner_margin(Margin::same(18))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let play = ui.add_sized(
                                [92.0, 34.0],
                                Self::action_button(
                                    RichText::new(if is_playing { "Stop" } else { "Play" })
                                        .size(13.0),
                                    is_playing,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &play);
                            if play.clicked() {
                                toggle_play = true;
                            }

                            let copy = ui.add_sized(
                                [92.0, 34.0],
                                Self::action_button(RichText::new("Copy").size(13.0), false, false),
                            );
                            Self::decorate_button_response(ui, &copy);
                            if copy.clicked() {
                                copy_request = true;
                            }

                            let folder = ui.add_sized(
                                [92.0, 34.0],
                                Self::action_button(
                                    RichText::new("Folder").size(13.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &folder);
                            if folder.clicked() {
                                folder_request = true;
                            }

                            let delete = ui.add_sized(
                                [92.0, 34.0],
                                Self::action_button(
                                    RichText::new("Delete").size(13.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &delete);
                            if delete.clicked() {
                                delete_request = true;
                            }

                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format_time(video.duration_secs))
                                        .size(12.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        });

                        ui.add_space(12.0);
                        let (bar_rect, _) =
                            ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                        ui.painter()
                            .rect_filled(bar_rect, 3.0, Color32::from_rgb(235, 224, 230));
                        let fill = Rect::from_min_max(
                            bar_rect.min,
                            Pos2::new(
                                bar_rect.left() + bar_rect.width() * progress.clamp(0.0, 1.0),
                                bar_rect.bottom(),
                            ),
                        );
                        ui.painter()
                            .rect_filled(fill, 3.0, Color32::from_rgb(214, 51, 132));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "{} / {}",
                                format_time(video.duration_secs * progress.clamp(0.0, 1.0)),
                                format_time(video.duration_secs)
                            ))
                            .size(12.0)
                            .color(Self::muted_text_color()),
                        );
                    });
            });

        if toggle_play {
            self.toggle_video_viewer_playback();
        }
        if copy_request && let Err(error) = self.copy_video_file_to_clipboard(&video) {
            self.set_error_status(error);
        }
        if folder_request
            && let Some(parent) = video.asset_path(self.storage.root_dir()).parent()
            && let Err(error) = open::that(parent)
        {
            self.set_error_status(error);
        }
        if delete_request {
            if let Some(audio) = self.audio.as_ref()
                && audio.is_playing_file(&audio_path)
            {
                self.stop_preview();
            }
            if let Some(index) = self
                .video_assets
                .iter()
                .position(|item| item.id == video.id)
            {
                let removed = self.video_assets.remove(index);
                if let Err(error) = self.storage.remove_video(&removed) {
                    self.set_error_status(error);
                } else {
                    let _ = self.storage.save_video_library(&self.video_assets);
                    self.video_viewer = None;
                }
            }
        } else if close_request {
            if let Some(audio) = self.audio.as_ref()
                && audio.is_playing_file(&audio_path)
            {
                self.stop_preview();
            }
            self.video_viewer = None;
        }
    }
}
