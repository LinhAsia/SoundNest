use super::*;

impl SoundFxApp {
    pub(super) fn render_pitch_monitor(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }
        if !self.show_pitch_panel {
            return;
        }

        let snapshot = self.pitch_monitor.snapshot();
        if snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if let Some(error) = snapshot.error.clone() {
            self.set_error_status(error);
        }

        let mut toggle = None;
        let refresh_inputs = false;
        let mut close_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(430.0, 440.0), vec2(390.0, 400.0), 0.0);
        egui::Window::new("")
            .id(egui::Id::new("pitch-monitor-panel"))
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
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(390.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Pitch Detect")
                                .size(15.0)
                                .color(Self::strong_text_color())
                                .strong(),
                        );
                        ui.label(
                            RichText::new("SPN")
                                .size(11.0)
                                .color(Self::muted_text_color()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                        let icon = if snapshot.running { 0xe047 } else { 0xe037 };
                        if Self::icon_action(
                            ui,
                            [56.0, 34.0],
                            icon,
                            snapshot.running,
                            snapshot.running,
                        )
                        .clicked()
                        {
                            toggle = Some(!snapshot.running);
                        }
                    });
                });

                ui.add_space(14.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Hotkey")
                                    .size(11.5)
                                    .color(Self::muted_text_color())
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                                let keyboard_response = Self::icon_action(
                                    ui,
                                    [48.0, 34.0],
                                    0xe312,
                                    self.capture_pitch_hotkey,
                                    self.capture_pitch_hotkey,
                                );
                                if keyboard_response.clicked() {
                                    if self.capture_pitch_hotkey {
                                        self.capture_pitch_hotkey = false;
                                        self.preview_pitch_hotkey = None;
                                    } else {
                                        self.capture_pitch_hotkey = true;
                                        self.capture_record_hotkey = false;
                                        self.preview_pitch_hotkey = None;
                                    }
                                }

                                if self.capture_pitch_hotkey {
                                    let capture_text =
                                        if let Some(preview_key) = self.preview_pitch_hotkey {
                                            format!("Pressing: {}", preview_key.to_string())
                                        } else {
                                            "Press key...".to_owned()
                                        };
                                    Frame::new()
                                        .fill(Color32::from_rgba_premultiplied(80, 70, 30, 255))
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(255, 220, 80)))
                                        .corner_radius(12.0)
                                        .inner_margin(Margin::symmetric(10, 5))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(capture_text)
                                                    .size(11.5)
                                                    .color(Color32::from_rgb(255, 232, 96))
                                                    .strong(),
                                            );
                                        });
                                }

                                let mut key_to_remove = None;
                                for &key in &self.pitch_hotkeys {
                                    let chip_btn = Button::new(
                                        RichText::new(key.to_string())
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
                                    self.pitch_hotkeys.retain(|&k| k != key);
                                    let names: Vec<String> =
                                        self.pitch_hotkeys.iter().map(|&k| k.to_string()).collect();
                                    let _ = self.storage.save_pitch_hotkeys(&names);
                                    if let Err(error) = self
                                        .record_hotkey_manager
                                        .set_secondary_hotkeys(&self.pitch_hotkeys)
                                    {
                                        self.set_error_status(error);
                                    }
                                }
                            });
                        });
                });

                ui.add_space(10.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Input")
                                    .size(11.5)
                                    .color(Self::muted_text_color())
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if Self::icon_action(
                                    ui,
                                    [48.0, 34.0],
                                    0xe30a,
                                    self.pitch_input_source == PitchInputSource::System,
                                    self.pitch_input_source == PitchInputSource::System,
                                )
                                .clicked()
                                {
                                    self.pitch_input_source = PitchInputSource::System;
                                }

                                if Self::icon_action(
                                    ui,
                                    [48.0, 34.0],
                                    0xe029,
                                    self.pitch_input_source == PitchInputSource::Microphone,
                                    self.pitch_input_source == PitchInputSource::Microphone,
                                )
                                .clicked()
                                {
                                    self.pitch_input_source = PitchInputSource::Microphone;
                                }
                            });

                            ui.add_space(8.0);
                            Frame::new()
                                .fill(Self::input_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(16.0)
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    if self.pitch_input_source == PitchInputSource::Microphone {
                                        Self::with_dark_combo_visuals(ui, |ui| {
                                            ComboBox::from_id_salt("pitch-input-device")
                                                .width(ui.available_width() - 4.0)
                                                .selected_text(
                                                    RichText::new(
                                                        self.selected_pitch_input_device
                                                            .as_deref()
                                                            .map(|name| {
                                                                Self::truncate_middle_ascii(
                                                                    name, 36,
                                                                )
                                                            })
                                                            .unwrap_or_else(|| "No mic".to_owned()),
                                                    )
                                                    .color(Self::strong_text_color()),
                                                )
                                                .show_ui(ui, |ui| {
                                                    for name in &self.pitch_capture_devices {
                                                        ui.selectable_value(
                                                            &mut self.selected_pitch_input_device,
                                                            Some(name.clone()),
                                                            Self::truncate_middle_ascii(name, 48),
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
                });

                ui.add_space(10.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Display")
                                    .size(11.5)
                                    .color(Self::muted_text_color())
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            Self::with_slider_visuals(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(Self::icon(0xe8b5, 16.0, Self::muted_text_color()));
                                    let slider_width = (ui.available_width() - 72.0).max(160.0);
                                    let (speed_response, _) = Self::click_slider(
                                        ui,
                                        &mut self.pitch_update_hz,
                                        1.0..=12.0,
                                        0.5,
                                        vec2(slider_width, 24.0),
                                    );
                                    ui.label(
                                        RichText::new(format!("{:.1}/s", self.pitch_update_hz))
                                            .size(13.0)
                                            .color(Self::strong_text_color()),
                                    );
                                    if speed_response.changed() {
                                        let _ =
                                            self.storage.save_pitch_update_hz(self.pitch_update_hz);
                                    }
                                });
                            });

                            ui.add_space(10.0);
                            Frame::new()
                                .fill(Self::input_fill())
                                .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                                .corner_radius(14.0)
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing = vec2(28.0, 8.0);
                                        let animation_label = self.t("settings.animation");
                                        let sharp_label = self.t("pitch.sharp");
                                        let animation_changed = ui
                                            .checkbox(
                                                &mut self.pitch_overlay_animation,
                                                RichText::new(animation_label)
                                                    .size(13.0)
                                                    .color(Self::strong_text_color()),
                                            )
                                            .changed();
                                        let sharp_changed = ui
                                            .checkbox(
                                                &mut self.pitch_show_sharps,
                                                RichText::new(sharp_label)
                                                    .size(13.0)
                                                    .color(Self::strong_text_color()),
                                            )
                                            .changed();
                                        if animation_changed {
                                            let _ = self.storage.save_overlay_animation(
                                                self.pitch_overlay_animation,
                                            );
                                            self.center_pitch_overlay_next_frame = snapshot.running;
                                            self.pitch_overlay_native_visuals_applied = false;
                                            ctx.request_repaint();
                                        }
                                        if sharp_changed {
                                            let _ = self
                                                .storage
                                                .save_pitch_show_sharps(self.pitch_show_sharps);
                                            ctx.request_repaint();
                                        }
                                    });
                                });
                        });
                });

                if let Some(error) = snapshot.error.as_deref() {
                    ui.add_space(10.0);
                    ui.add_sized(
                        [ui.available_width(), 16.0],
                        egui::Label::new(
                            RichText::new(Self::truncate_middle_ascii(error, 34))
                                .size(11.0)
                                .color(Color32::from_rgb(189, 62, 117)),
                        )
                        .truncate(),
                    );
                }
            });
        if close_request {
            self.show_pitch_panel = false;
        }

        if let Some(should_start) = toggle {
            if should_start {
                if self.pitch_input_source == PitchInputSource::Microphone
                    && self.selected_pitch_input_device.is_none()
                {
                    self.set_error_status(self.t("pitch.no_microphone_input"));
                } else {
                    match self.pitch_monitor.start(PitchMonitorConfig {
                        source: self.pitch_input_source,
                        input_device_name: if self.pitch_input_source
                            == PitchInputSource::Microphone
                        {
                            self.selected_pitch_input_device.clone()
                        } else {
                            None
                        },
                        updates_per_second: self.pitch_update_hz,
                    }) {
                        Ok(()) => {
                            self.center_pitch_overlay_next_frame = true;
                            self.pitch_overlay_pos = None;
                            self.pitch_overlay_native_visuals_applied = false;
                            self.show_pitch_panel = false;
                            let overlay_size = if self.pitch_overlay_animation {
                                vec2(276.0, 276.0)
                            } else {
                                vec2(430.0, 104.0)
                            };
                            Self::apply_overlay_only_viewport(ctx, overlay_size);
                            self.overlay_only_mode = true;
                            self.clear_status();
                        }
                        Err(error) => self.set_error_status(error),
                    }
                }
            } else {
                self.pitch_monitor.stop();
                self.pitch_overlay_native_visuals_applied = false;
                self.clear_status();
            }
        }
        if refresh_inputs {
            self.refresh_pitch_capture_devices();
        }
    }

    pub(super) fn render_pitch_overlay_viewport(&mut self, ctx: &Context) {
        let snapshot = self.pitch_monitor.snapshot();
        if !snapshot.running {
            return;
        }

        let mut should_stop = false;
        let overlay_size = if self.pitch_overlay_animation {
            vec2(276.0, 276.0)
        } else {
            vec2(430.0, 104.0)
        };
        let overlay_pos = if self.overlay_only_mode {
            let anchored = Pos2::ZERO;
            self.pitch_overlay_pos = Some(anchored);
            anchored
        } else if self.center_pitch_overlay_next_frame || self.pitch_overlay_pos.is_none() {
            let centered = self.centered_overlay_pos(ctx, overlay_size);
            self.pitch_overlay_pos = Some(centered);
            centered
        } else {
            self.clamp_overlay_pos(
                ctx,
                overlay_size,
                self.pitch_overlay_pos.unwrap_or_default(),
            )
        };
        ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        let area_id = egui::Id::new("pitch-overlay-panel");
        egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .current_pos(overlay_pos)
            .constrain_to(self.popup_safe_rect(ctx))
            .interactable(true)
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    overlay_size,
                    egui::Layout::top_down(Align::Min),
                    |ui| {
                        if self.pitch_overlay_animation {
                            self.render_pitch_blob_overlay(ui, ctx, &snapshot, &mut should_stop);
                        } else {
                            self.render_pitch_pill_overlay(ui, ctx, &snapshot, &mut should_stop);
                        }
                    },
                );
            });
        if let Some(state) = egui::AreaState::load(ctx, area_id) {
            self.pitch_overlay_pos =
                Some(self.clamp_overlay_pos(ctx, overlay_size, state.left_top_pos()));
        }
        self.center_pitch_overlay_next_frame = false;
        self.pitch_overlay_native_visuals_applied = false;

        if should_stop {
            self.pitch_monitor.stop();
            self.overlay_only_mode = false;
            self.center_window_next_frame = true;
            Self::restore_main_viewport(ctx);
            self.clear_status();
        }
    }

    pub(super) fn render_pitch_pill_overlay(
        &mut self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &PitchSnapshot,
        should_stop: &mut bool,
    ) {
        let rect = ui.max_rect().shrink2(vec2(10.0, 14.0));
        let painter = ui.painter_at(rect);
        let glow_color = Color32::from_rgba_premultiplied(227, 82, 149, 26);
        painter.circle_filled(
            Pos2::new(rect.left() + 74.0, rect.center().y),
            44.0,
            glow_color,
        );
        painter.circle_filled(
            Pos2::new(rect.right() - 108.0, rect.center().y),
            58.0,
            Color32::from_rgba_premultiplied(236, 126, 183, 18),
        );

        let capsule = Frame::new()
            .fill(Color32::from_rgba_premultiplied(255, 252, 254, 168))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_premultiplied(255, 255, 255, 112),
            ))
            .shadow(Shadow {
                offset: [0, 18],
                blur: 36,
                spread: 0,
                color: Color32::from_rgba_premultiplied(78, 40, 63, 30),
            })
            .corner_radius(44.0)
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                let content_size = vec2(rect.width(), rect.height());
                ui.set_min_size(content_size);
                let drag_rect = Rect::from_min_max(
                    ui.min_rect().min,
                    Pos2::new(ui.min_rect().max.x - 46.0, ui.min_rect().max.y),
                );
                let _drag_response = ui.interact(
                    drag_rect,
                    ui.id().with("pitch-overlay-drag"),
                    Sense::click_and_drag(),
                );
                if self.overlay_only_mode {
                    if _drag_response.drag_started() {
                        overlay_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                } else {
                    let overlay_rect = self.popup_safe_rect(overlay_ctx);
                    Self::update_overlay_drag_position(
                        overlay_ctx,
                        overlay_rect,
                        &_drag_response,
                        vec2(430.0, 104.0),
                        &mut self.pitch_overlay_pos,
                    );
                }
                let top_highlight = Rect::from_min_max(
                    Pos2::new(ui.min_rect().left() + 18.0, ui.min_rect().top() + 1.0),
                    Pos2::new(ui.min_rect().right() - 58.0, ui.min_rect().top() + 14.0),
                );
                ui.painter().rect_filled(
                    top_highlight,
                    14.0,
                    Color32::from_rgba_premultiplied(255, 255, 255, 34),
                );

                let body_height = (content_size.y - 10.0).max(62.0);
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    vec2(content_size.x, body_height),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(52.0, 52.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let center = ui.max_rect().center();
                                ui.painter().circle_filled(
                                    center,
                                    18.0,
                                    Color32::from_rgba_premultiplied(227, 82, 149, 40),
                                );
                                ui.painter().circle_filled(
                                    center,
                                    8.0 + snapshot.level * 6.0,
                                    Color32::from_rgb(227, 82, 149),
                                );
                            },
                        );
                        ui.add_space(6.0);
                        ui.allocate_ui_with_layout(
                            vec2(110.0, 52.0),
                            egui::Layout::top_down_justified(Align::Min),
                            |ui| {
                                ui.add_space(1.0);
                                ui.label(
                                    RichText::new(self.display_pitch_note(&snapshot.note))
                                        .size(33.0)
                                        .color(Color32::from_rgb(44, 38, 46))
                                        .strong(),
                                );
                                ui.add_space(-2.0);
                                ui.label(
                                    RichText::new(self.pitch_overlay_meta(snapshot))
                                        .size(11.5)
                                        .color(Color32::from_rgb(117, 101, 113)),
                                );
                            },
                        );
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            vec2(176.0, 38.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                Self::draw_pitch_overlay_wave_strip(ui, snapshot);
                            },
                        );
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            vec2(34.0, 34.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let close =
                                    Self::icon_titlebar(ui, [34.0, 30.0], 0xe5cd, false, true);
                                if close.clicked() {
                                    *should_stop = true;
                                }
                            },
                        );
                    },
                );
            });
        let _ = capsule.response;
    }

    pub(super) fn render_pitch_blob_overlay(
        &mut self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &PitchSnapshot,
        should_stop: &mut bool,
    ) {
        let rect = ui.max_rect().shrink2(vec2(4.0, 4.0));
        let drag_response = ui.interact(
            rect,
            ui.id().with("pitch-blob-overlay-drag"),
            Sense::click_and_drag(),
        );
        if self.overlay_only_mode {
            if drag_response.drag_started() {
                overlay_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        } else {
            let overlay_rect = self.popup_safe_rect(overlay_ctx);
            Self::update_overlay_drag_position(
                overlay_ctx,
                overlay_rect,
                &drag_response,
                vec2(276.0, 276.0),
                &mut self.pitch_overlay_pos,
            );
        }
        if drag_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_response.dragged() || drag_response.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let painter = ui.painter_at(rect);
        let center = rect.center();
        let time = overlay_ctx.input(|input| input.time) as f32;
        let aura = snapshot.level.clamp(0.04, 1.0);
        let waveform_energy = if snapshot.waveform.is_empty() {
            aura
        } else {
            snapshot.waveform.iter().copied().sum::<f32>() / snapshot.waveform.len() as f32
        };
        let glow = aura.max(waveform_energy).clamp(0.05, 1.0);
        let base = rect.width().min(rect.height()) * 0.34;
        let half_w = base * (1.0 + glow * 0.16);
        let half_h = base * (0.92 + glow * 0.24);
        let wobble = 0.052 + glow * 0.16;
        let exponent = 4.0 - glow * 1.35;

        let deep_shadow = Color32::from_rgb(4, 4, 8);
        let ink = Color32::from_rgb(10, 10, 14);
        let charcoal = Color32::from_rgb(18, 18, 24);
        let graphite = Color32::from_rgb(28, 28, 36);
        let berry = Color32::from_rgb(232, 73, 154);
        let magenta = Color32::from_rgb(255, 118, 186);
        let petal = Color32::from_rgb(255, 238, 247);

        for (spread, alpha) in [(1.42, 22), (1.2, 34), (1.04, 56)] {
            let shadow_points = Self::squircle_points(
                Pos2::new(center.x, center.y + 14.0 + glow * 8.0),
                half_w * spread,
                half_h * spread,
                exponent,
                wobble * 0.72,
                time - spread,
            );
            painter.add(egui::Shape::convex_polygon(
                shadow_points,
                Color32::from_rgba_premultiplied(
                    deep_shadow.r(),
                    deep_shadow.g(),
                    deep_shadow.b(),
                    alpha,
                ),
                Stroke::NONE,
            ));
        }

        let outer_points = Self::squircle_points(
            center,
            half_w * 1.02,
            half_h * 1.02,
            exponent,
            wobble * 1.12,
            time,
        );
        painter.add(egui::Shape::convex_polygon(
            outer_points,
            Color32::from_rgba_premultiplied(charcoal.r(), charcoal.g(), charcoal.b(), 124),
            Stroke::new(
                1.4,
                Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 94),
            ),
        ));

        let blob_points =
            Self::squircle_points(center, half_w, half_h, exponent, wobble, time + 0.4);
        painter.add(egui::Shape::convex_polygon(
            blob_points.clone(),
            Color32::from_rgba_premultiplied(ink.r(), ink.g(), ink.b(), 238),
            Stroke::new(
                1.2,
                Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 112),
            ),
        ));

        let highlight_points = Self::squircle_points(
            Pos2::new(center.x, center.y - half_h * 0.2),
            half_w * 0.76,
            half_h * 0.38,
            exponent,
            wobble * 0.5,
            time + 1.1,
        );
        painter.add(egui::Shape::convex_polygon(
            highlight_points,
            Color32::from_rgba_premultiplied(graphite.r(), graphite.g(), graphite.b(), 112),
            Stroke::NONE,
        ));

        let clip_rect = Rect::from_center_size(center, vec2(half_w * 1.52, half_h * 1.46));
        let clip = painter.with_clip_rect(clip_rect);
        let wave = if snapshot.waveform.is_empty() {
            vec![0.08; 40]
        } else {
            snapshot.waveform.clone()
        };
        let band_rect = Rect::from_center_size(
            Pos2::new(center.x, center.y + half_h * 0.54),
            vec2(half_w * 1.08, half_h * 0.22),
        );
        let bar_width = band_rect.width() / wave.len().max(1) as f32;
        for (index, value) in wave.iter().enumerate() {
            let pulse = (time * 5.2 + index as f32 * 0.46).sin() * 0.06;
            let amplitude = (value + pulse).clamp(0.06, 1.0);
            let x = band_rect.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * band_rect.height() * 0.52;
            let bar_rect = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.18, band_rect.center().y - half),
                Pos2::new(x + bar_width * 0.18, band_rect.center().y + half),
            );
            clip.rect_filled(
                bar_rect,
                4.0,
                Color32::from_rgba_premultiplied(berry.r(), berry.g(), berry.b(), 176),
            );
        }

        for index in 0..8 {
            let angle = time * 0.84 + index as f32 * 0.82;
            let orbit = base * (0.64 + (index % 3) as f32 * 0.12) + glow * 18.0;
            let note_pos = Pos2::new(
                center.x + angle.cos() * orbit,
                center.y + angle.sin() * orbit * 0.72,
            );
            let note_alpha = (60.0 + glow * 92.0).clamp(0.0, 160.0) as u8;
            let note_color = if index % 2 == 0 {
                Color32::from_rgba_premultiplied(255, 205, 230, note_alpha)
            } else {
                Color32::from_rgba_premultiplied(235, 112, 176, note_alpha)
            };
            Self::paint_music_note(
                &painter,
                note_pos,
                0.5 + (index % 3) as f32 * 0.1,
                angle.sin() * 0.22,
                note_color,
            );
        }

        let note_pos = Pos2::new(center.x, center.y - 12.0);
        painter.text(
            note_pos,
            egui::Align2::CENTER_CENTER,
            self.display_pitch_note(&snapshot.note),
            egui::FontId::proportional(42.0),
            petal,
        );

        let sharp_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 50.0, rect.top() + 18.0),
            vec2(34.0, 26.0),
        );
        let sharp_response = ui.interact(
            sharp_rect,
            ui.id().with("pitch-blob-overlay-sharp"),
            Sense::click(),
        );
        let close_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 18.0, rect.top() + 18.0),
            vec2(26.0, 26.0),
        );
        let close_response = ui.interact(
            close_rect,
            ui.id().with("pitch-blob-overlay-close"),
            Sense::click(),
        );
        let show_controls =
            drag_response.hovered() || close_response.hovered() || sharp_response.hovered();
        if sharp_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if close_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if show_controls {
            let sharp_active = self.pitch_show_sharps;
            painter.rect_filled(
                sharp_rect,
                13.0,
                if sharp_active {
                    Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 228)
                } else if sharp_response.hovered() {
                    Color32::from_rgba_premultiplied(36, 36, 44, 232)
                } else {
                    Color32::from_rgba_premultiplied(18, 18, 24, 188)
                },
            );
            painter.rect_stroke(
                sharp_rect,
                13.0,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(
                        magenta.r(),
                        magenta.g(),
                        magenta.b(),
                        if sharp_active { 230 } else { 112 },
                    ),
                ),
                StrokeKind::Outside,
            );
            painter.text(
                sharp_rect.center(),
                egui::Align2::CENTER_CENTER,
                "S#",
                egui::FontId::proportional(12.5),
                if sharp_active {
                    Color32::from_rgb(20, 12, 18)
                } else {
                    petal
                },
            );
            painter.circle_filled(
                Pos2::new(sharp_rect.right() - 6.0, sharp_rect.top() + 6.0),
                2.6,
                if sharp_active {
                    Color32::from_rgb(255, 245, 250)
                } else {
                    Color32::from_rgba_premultiplied(255, 255, 255, 70)
                },
            );

            painter.circle_filled(
                close_rect.center(),
                13.0,
                if close_response.hovered() {
                    Color32::from_rgba_premultiplied(36, 36, 44, 232)
                } else {
                    Color32::from_rgba_premultiplied(18, 18, 24, 188)
                },
            );
            painter.circle_stroke(
                close_rect.center(),
                13.0,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 132),
                ),
            );
            painter.text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                Self::icon(0xe5cd, 16.0, petal).text(),
                egui::FontId::new(16.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
                petal,
            );
        }
        if sharp_response.clicked() {
            self.pitch_show_sharps = !self.pitch_show_sharps;
            let _ = self.storage.save_pitch_show_sharps(self.pitch_show_sharps);
            ui.ctx().request_repaint();
        }
        if close_response.clicked() {
            *should_stop = true;
        }
    }

    pub(super) fn pitch_overlay_meta(&self, snapshot: &PitchSnapshot) -> String {
        format!(
            "{}  {:.0}%",
            if self.pitch_input_source == PitchInputSource::Microphone {
                Self::truncate_middle_ascii(
                    self.selected_pitch_input_device.as_deref().unwrap_or("MIC"),
                    12,
                )
            } else {
                "SYS".to_owned()
            },
            (snapshot.confidence * 100.0).round()
        )
    }

    pub(super) fn display_pitch_note(&self, note: &str) -> String {
        if !note.contains('/') {
            return note.to_owned();
        }

        let Some((sharp_name, flat_with_octave)) = note.split_once('/') else {
            return note.to_owned();
        };
        let Some(sharp_digit) = sharp_name
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_digit() || *ch == '-')
            .map(|(index, _)| index)
        else {
            return note.to_owned();
        };
        let Some(first_digit) = flat_with_octave
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_digit() || *ch == '-')
            .map(|(index, _)| index)
        else {
            return note.to_owned();
        };

        let sharp_name = &sharp_name[..sharp_digit];
        let flat_name = &flat_with_octave[..first_digit];
        let octave = &flat_with_octave[first_digit..];
        if self.pitch_show_sharps {
            format!("{sharp_name}{octave}")
        } else {
            format!("{flat_name}{octave}")
        }
    }

    pub(super) fn draw_pitch_overlay_wave_strip(ui: &mut Ui, snapshot: &PitchSnapshot) {
        let desired = vec2(ui.available_width().max(150.0), 38.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            19.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 84),
        );
        painter.rect_stroke(
            rect,
            19.0,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 76)),
            StrokeKind::Outside,
        );

        let waveform = if snapshot.waveform.is_empty() {
            vec![0.04; 40]
        } else {
            snapshot.waveform.clone()
        };
        let inner = rect.shrink2(vec2(10.0, 8.0));
        let bar_width = inner.width() / waveform.len().max(1) as f32;
        for (index, level) in waveform.iter().enumerate() {
            let x = inner.left() + index as f32 * bar_width;
            let height = inner.height() * level.clamp(0.06, 1.0);
            let bar = Rect::from_min_max(
                Pos2::new(x + bar_width * 0.22, inner.center().y - height * 0.5),
                Pos2::new(x + bar_width * 0.78, inner.center().y + height * 0.5),
            );
            painter.rect_filled(bar, 3.0, Color32::from_rgb(227, 82, 149));
        }
    }
}
