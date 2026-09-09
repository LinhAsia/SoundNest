use super::*;
use crate::services::audio_similarity::{
    compute_sound_similarity, compute_waveform_similarity, find_library_duplicates,
};

impl SoundFxApp {
    pub(super) fn scan_library_duplicates(&mut self) {
        self.duplicate_scanned = true;
        self.duplicate_pairs = find_library_duplicates(&self.sounds, self.duplicate_threshold);
    }

    pub(super) fn render_duplicate_panel(&mut self, ctx: &Context) {
        if !self.show_duplicate_panel {
            return;
        }

        let mut open_panel = self.show_duplicate_panel;
        let mut close_request = false;
        let mut trigger_scan = false;
        let mut delete_sound_action: Option<Uuid> = None;
        let mut remove_pair_index: Option<usize> = None;
        let mut play_sound_action: Option<Uuid> = None;
        let mut stop_audio_action = false;

        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(860.0, 620.0), vec2(480.0, 360.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("sound-duplicate-panel"))
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
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                // Header
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe028, 20.0, Color32::from_rgb(242, 140, 56)).strong());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(self.t("duplicates.title"))
                            .size(17.0)
                            .color(Self::strong_text_color())
                            .strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(10.0);

                // Tab selection
                ui.horizontal(|ui| {
                    let scan_btn = ui.add_sized(
                        [140.0, 30.0],
                        Self::action_button(
                            RichText::new(self.t("duplicates.tab_scan")).size(12.5),
                            self.duplicate_tab == DuplicateTab::LibraryScan,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &scan_btn);
                    if scan_btn.clicked() {
                        self.duplicate_tab = DuplicateTab::LibraryScan;
                    }

                    ui.add_space(6.0);

                    let compare_btn = ui.add_sized(
                        [160.0, 30.0],
                        Self::action_button(
                            RichText::new(self.t("duplicates.tab_compare")).size(12.5),
                            self.duplicate_tab == DuplicateTab::CompareTwo,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &compare_btn);
                    if compare_btn.clicked() {
                        self.duplicate_tab = DuplicateTab::CompareTwo;
                    }
                });

                ui.add_space(12.0);

                match self.duplicate_tab {
                    DuplicateTab::LibraryScan => {
                        // Scan toolbar
                        ui.horizontal(|ui| {
                            let scan_btn = ui.add_sized(
                                [130.0, 32.0],
                                Button::new(
                                    RichText::new(self.t("duplicates.scan_now"))
                                        .size(12.5)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(227, 82, 149))
                                .corner_radius(10.0),
                            );
                            Self::decorate_button_response(ui, &scan_btn);
                            if scan_btn.clicked() {
                                trigger_scan = true;
                            }

                            ui.add_space(12.0);

                            ui.label(
                                RichText::new(self.t("duplicates.threshold_label"))
                                    .size(12.0)
                                    .color(Self::muted_text_color()),
                            );

                            let mut threshold_percent = (self.duplicate_threshold * 100.0).round();
                            let slider = egui::Slider::new(&mut threshold_percent, 75.0..=99.0)
                                .suffix("%")
                                .step_by(1.0);
                            let response = ui.add_sized([110.0, 24.0], slider);
                            if response.changed() {
                                self.duplicate_threshold = threshold_percent / 100.0;
                                if self.duplicate_scanned {
                                    trigger_scan = true;
                                }
                            }

                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                if self.duplicate_scanned {
                                    let summary_text = if self.duplicate_pairs.is_empty() {
                                        self.t("duplicates.no_duplicates_found")
                                    } else {
                                        format!(
                                            "{} {}",
                                            self.duplicate_pairs.len(),
                                            self.t("duplicates.pairs_found")
                                        )
                                    };
                                    ui.label(
                                        RichText::new(summary_text)
                                            .size(12.0)
                                            .color(if self.duplicate_pairs.is_empty() {
                                                Color32::from_rgb(100, 190, 120)
                                            } else {
                                                Color32::from_rgb(242, 140, 56)
                                            })
                                            .strong(),
                                    );
                                }
                            });
                        });

                        ui.add_space(10.0);

                        // Pairs list
                        let scroll_height = ui.available_height().max(120.0);
                        egui::ScrollArea::vertical()
                            .id_salt("duplicate_pairs_scroll")
                            .max_height(scroll_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if !self.duplicate_scanned {
                                    ui.add_space(40.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(Self::icon(0xe028, 36.0, Self::muted_text_color()));
                                        ui.add_space(10.0);
                                        ui.label(
                                            RichText::new(self.t("duplicates.hint_scan"))
                                                .size(13.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    });
                                } else if self.duplicate_pairs.is_empty() {
                                    ui.add_space(40.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(Self::icon(0xe876, 36.0, Color32::from_rgb(100, 190, 120)));
                                        ui.add_space(10.0);
                                        ui.label(
                                            RichText::new(self.t("duplicates.clean_library"))
                                                .size(13.5)
                                                .color(Self::strong_text_color()),
                                        );
                                    });
                                } else {
                                    let pairs = self.duplicate_pairs.clone();
                                    for (pair_idx, pair) in pairs.iter().enumerate() {
                                        let sound_a = self.sounds.iter().find(|s| s.id == pair.sound_a_id);
                                        let sound_b = self.sounds.iter().find(|s| s.id == pair.sound_b_id);

                                        if let (Some(a), Some(b)) = (sound_a, sound_b) {
                                            Frame::new()
                                                .fill(Self::surface_fill())
                                                .stroke(Stroke::new(1.0, Self::border_color()))
                                                .corner_radius(14.0)
                                                .inner_margin(Margin::symmetric(14, 10))
                                                .show(ui, |ui| {
                                                    let avail_w = ui.available_width();
                                                    let actions_w = 172.0f32;
                                                    let badge_w = 76.0f32;
                                                    let col_w = ((avail_w - actions_w - badge_w - 24.0) * 0.5).max(180.0);

                                                    ui.horizontal(|ui| {
                                                        // Sound A Column
                                                        ui.allocate_ui_with_layout(
                                                            vec2(col_w, 48.0),
                                                            egui::Layout::top_down(Align::Min),
                                                            |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let is_playing_a = self.audio.as_ref().is_some_and(|aud| aud.is_playing(a.id));
                                                                    let play_btn = ui.add_sized(
                                                                        [24.0, 24.0],
                                                                        Button::new(Self::icon(
                                                                            if is_playing_a { 0xe034 } else { 0xe037 },
                                                                            13.0,
                                                                            Color32::WHITE,
                                                                        ))
                                                                        .fill(if is_playing_a {
                                                                            Color32::from_rgb(227, 82, 149)
                                                                        } else {
                                                                            Color32::from_rgb(70, 60, 75)
                                                                        })
                                                                        .corner_radius(6.0),
                                                                    );
                                                                    if play_btn.clicked() {
                                                                        if is_playing_a {
                                                                            stop_audio_action = true;
                                                                        } else {
                                                                            play_sound_action = Some(a.id);
                                                                        }
                                                                    }

                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(col_w - 32.0);
                                                                        ui.add(
                                                                            egui::Label::new(
                                                                                RichText::new(&a.name)
                                                                                    .size(11.5)
                                                                                    .color(Self::strong_text_color())
                                                                                    .strong(),
                                                                            )
                                                                            .truncate(),
                                                                        );
                                                                        ui.label(
                                                                            RichText::new(format!(
                                                                                "{} • {}",
                                                                                format_time(a.duration_secs),
                                                                                a.asset_file.split('.').last().unwrap_or("")
                                                                            ))
                                                                            .size(10.0)
                                                                            .color(Self::muted_text_color()),
                                                                        );
                                                                    });
                                                                });

                                                                let (wave_rect, _) = ui.allocate_exact_size(vec2(col_w, 18.0), Sense::hover());
                                                                Self::paint_mini_waveform_bars(ui.painter(), wave_rect, &a.waveform, Color32::from_rgb(100, 180, 240));
                                                            },
                                                        );

                                                        ui.add_space(8.0);

                                                        // Match badge pill
                                                        ui.allocate_ui_with_layout(
                                                            vec2(badge_w, 48.0),
                                                            egui::Layout::top_down(Align::Center),
                                                            |ui| {
                                                                ui.add_space(6.0);
                                                                let match_pct = (pair.similarity * 100.0).clamp(0.0, 100.0);
                                                                let pill_bg = if match_pct >= 95.0 {
                                                                    Color32::from_rgb(190, 45, 65)
                                                                } else {
                                                                    Color32::from_rgb(200, 110, 30)
                                                                };
                                                                ui.label(
                                                                    RichText::new(format!("{match_pct:.1}%"))
                                                                        .size(13.0)
                                                                        .color(Color32::WHITE)
                                                                        .strong(),
                                                                );
                                                                ui.label(
                                                                    RichText::new(self.t("duplicates.match_label"))
                                                                        .size(9.0)
                                                                        .color(pill_bg),
                                                                );
                                                            },
                                                        );

                                                        ui.add_space(8.0);

                                                        // Sound B Column
                                                        ui.allocate_ui_with_layout(
                                                            vec2(col_w, 48.0),
                                                            egui::Layout::top_down(Align::Min),
                                                            |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let is_playing_b = self.audio.as_ref().is_some_and(|aud| aud.is_playing(b.id));
                                                                    let play_btn = ui.add_sized(
                                                                        [24.0, 24.0],
                                                                        Button::new(Self::icon(
                                                                            if is_playing_b { 0xe034 } else { 0xe037 },
                                                                            13.0,
                                                                            Color32::WHITE,
                                                                        ))
                                                                        .fill(if is_playing_b {
                                                                            Color32::from_rgb(227, 82, 149)
                                                                        } else {
                                                                            Color32::from_rgb(70, 60, 75)
                                                                        })
                                                                        .corner_radius(6.0),
                                                                    );
                                                                    if play_btn.clicked() {
                                                                        if is_playing_b {
                                                                            stop_audio_action = true;
                                                                        } else {
                                                                            play_sound_action = Some(b.id);
                                                                        }
                                                                    }

                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(col_w - 32.0);
                                                                        ui.add(
                                                                            egui::Label::new(
                                                                                RichText::new(&b.name)
                                                                                    .size(11.5)
                                                                                    .color(Self::strong_text_color())
                                                                                    .strong(),
                                                                            )
                                                                            .truncate(),
                                                                        );
                                                                        ui.label(
                                                                            RichText::new(format!(
                                                                                "{} • {}",
                                                                                format_time(b.duration_secs),
                                                                                b.asset_file.split('.').last().unwrap_or("")
                                                                            ))
                                                                            .size(10.0)
                                                                            .color(Self::muted_text_color()),
                                                                        );
                                                                    });
                                                                });

                                                                let (wave_rect, _) = ui.allocate_exact_size(vec2(col_w, 18.0), Sense::hover());
                                                                Self::paint_mini_waveform_bars(ui.painter(), wave_rect, &b.waveform, Color32::from_rgb(240, 140, 180));
                                                            },
                                                        );

                                                        ui.add_space(8.0);

                                                        // Action buttons Column (fixed 172px, right aligned, perfect vertical column)
                                                        ui.allocate_ui_with_layout(
                                                            vec2(actions_w, 48.0),
                                                            egui::Layout::right_to_left(Align::Center),
                                                            |ui| {
                                                                let ignore_btn = ui.add_sized(
                                                                    [50.0, 26.0],
                                                                    Button::new(RichText::new(self.t("duplicates.ignore")).size(11.0))
                                                                        .corner_radius(6.0),
                                                                );
                                                                if ignore_btn.clicked() {
                                                                    remove_pair_index = Some(pair_idx);
                                                                }

                                                                ui.add_space(4.0);

                                                                let del_b_btn = ui.add_sized(
                                                                    [52.0, 26.0],
                                                                    Button::new(
                                                                        RichText::new(self.t("duplicates.delete_b"))
                                                                            .size(11.0)
                                                                            .color(Color32::WHITE),
                                                                    )
                                                                    .fill(Color32::from_rgb(180, 50, 60))
                                                                    .corner_radius(6.0),
                                                                );
                                                                if del_b_btn.clicked() {
                                                                    delete_sound_action = Some(b.id);
                                                                    remove_pair_index = Some(pair_idx);
                                                                }

                                                                ui.add_space(4.0);

                                                                let del_a_btn = ui.add_sized(
                                                                    [52.0, 26.0],
                                                                    Button::new(
                                                                        RichText::new(self.t("duplicates.delete_a"))
                                                                            .size(11.0)
                                                                            .color(Color32::WHITE),
                                                                    )
                                                                    .fill(Color32::from_rgb(180, 50, 60))
                                                                    .corner_radius(6.0),
                                                                );
                                                                if del_a_btn.clicked() {
                                                                    delete_sound_action = Some(a.id);
                                                                    remove_pair_index = Some(pair_idx);
                                                                }
                                                            },
                                                        );
                                                    });
                                                });

                                            ui.add_space(6.0);
                                        }
                                    }
                                }
                            });
                    }
                    DuplicateTab::CompareTwo => {
                        // Manual 2-sound compare
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(360.0);
                                ui.label(
                                    RichText::new(self.t("duplicates.pick_sound_a"))
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                                let default_sound_text = self.t("duplicates.select_sound");
                                let current_a_name = self
                                    .compare_sound_a
                                    .and_then(|id| self.sounds.iter().find(|s| s.id == id))
                                    .map(|s| s.name.as_str())
                                    .unwrap_or(&default_sound_text);

                                egui::ComboBox::from_id_salt("compare_sound_a_select")
                                    .selected_text(current_a_name)
                                    .width(340.0)
                                    .show_ui(ui, |ui| {
                                        for sound in &self.sounds {
                                            let is_selected = self.compare_sound_a == Some(sound.id);
                                            if ui.selectable_label(is_selected, &sound.name).clicked() {
                                                self.compare_sound_a = Some(sound.id);
                                            }
                                        }
                                    });
                            });

                            ui.add_space(20.0);

                            ui.vertical(|ui| {
                                ui.set_width(360.0);
                                ui.label(
                                    RichText::new(self.t("duplicates.pick_sound_b"))
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                                let default_sound_text_b = self.t("duplicates.select_sound");
                                let current_b_name = self
                                    .compare_sound_b
                                    .and_then(|id| self.sounds.iter().find(|s| s.id == id))
                                    .map(|s| s.name.as_str())
                                    .unwrap_or(&default_sound_text_b);

                                egui::ComboBox::from_id_salt("compare_sound_b_select")
                                    .selected_text(current_b_name)
                                    .width(340.0)
                                    .show_ui(ui, |ui| {
                                        for sound in &self.sounds {
                                            let is_selected = self.compare_sound_b == Some(sound.id);
                                            if ui.selectable_label(is_selected, &sound.name).clicked() {
                                                self.compare_sound_b = Some(sound.id);
                                            }
                                        }
                                    });
                            });
                        });

                        ui.add_space(16.0);

                        let sound_a = self.compare_sound_a.and_then(|id| self.sounds.iter().find(|s| s.id == id));
                        let sound_b = self.compare_sound_b.and_then(|id| self.sounds.iter().find(|s| s.id == id));

                        if let (Some(a), Some(b)) = (sound_a, sound_b) {
                            let wave_sim = compute_waveform_similarity(&a.waveform, &b.waveform);
                            let total_sim = compute_sound_similarity(a.duration_secs, &a.waveform, b.duration_secs, &b.waveform);
                            let dur_diff = (a.duration_secs - b.duration_secs).abs();

                            Frame::new()
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(16.0)
                                .inner_margin(Margin::same(18))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(format!("{} ({})", a.name, format_time(a.duration_secs)))
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(100, 180, 240))
                                                    .strong(),
                                            );
                                            let (rect_a, _) = ui.allocate_exact_size(vec2(ui.available_width() * 0.46, 50.0), Sense::hover());
                                            Self::paint_mini_waveform_bars(ui.painter(), rect_a, &a.waveform, Color32::from_rgb(100, 180, 240));

                                            let is_playing_a = self.audio.as_ref().is_some_and(|aud| aud.is_playing(a.id));
                                            if ui.button(if is_playing_a { "Stop A" } else { "Play A" }).clicked() {
                                                if is_playing_a { stop_audio_action = true; } else { play_sound_action = Some(a.id); }
                                            }
                                        });

                                        ui.separator();

                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(format!("{} ({})", b.name, format_time(b.duration_secs)))
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(240, 140, 180))
                                                    .strong(),
                                            );
                                            let (rect_b, _) = ui.allocate_exact_size(vec2(ui.available_width() * 0.90, 50.0), Sense::hover());
                                            Self::paint_mini_waveform_bars(ui.painter(), rect_b, &b.waveform, Color32::from_rgb(240, 140, 180));

                                            let is_playing_b = self.audio.as_ref().is_some_and(|aud| aud.is_playing(b.id));
                                            if ui.button(if is_playing_b { "Stop B" } else { "Play B" }).clicked() {
                                                if is_playing_b { stop_audio_action = true; } else { play_sound_action = Some(b.id); }
                                            }
                                        });
                                    });

                                    ui.add_space(14.0);
                                    ui.separator();
                                    ui.add_space(8.0);

                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{}: {:.1}%  |  {}: {:.1}%  |  {}: {:.2}s",
                                                self.t("duplicates.total_similarity"),
                                                total_sim * 100.0,
                                                self.t("duplicates.wave_similarity"),
                                                wave_sim * 100.0,
                                                self.t("duplicates.duration_diff"),
                                                dur_diff
                                            ))
                                            .size(13.5)
                                            .color(if total_sim >= 0.88 {
                                                Color32::from_rgb(242, 100, 100)
                                            } else {
                                                Self::strong_text_color()
                                            })
                                            .strong(),
                                        );
                                    });
                                });
                        } else {
                            ui.vertical_centered(|ui| {
                                ui.add_space(40.0);
                                ui.label(
                                    RichText::new(self.t("duplicates.select_both_hint"))
                                        .size(13.0)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        }
                    }
                }
            });

        if close_request || !open_panel {
            self.show_duplicate_panel = false;
        }

        if trigger_scan {
            self.scan_library_duplicates();
        }

        if let Some(sound_id) = delete_sound_action {
            self.delete_sound_by_id(sound_id);
            // Re-scan or clean up pairs referencing deleted sound
            self.duplicate_pairs.retain(|p| p.sound_a_id != sound_id && p.sound_b_id != sound_id);
        }

        if let Some(idx) = remove_pair_index {
            if idx < self.duplicate_pairs.len() {
                self.duplicate_pairs.remove(idx);
            }
        }

        if stop_audio_action {
            if let Some(audio) = self.audio.as_mut() {
                audio.stop();
            }
        }

        if let Some(sound_id) = play_sound_action {
            self.preview_sound(sound_id);
        }
    }

    pub(super) fn paint_mini_waveform_bars(
        painter: &egui::Painter,
        rect: Rect,
        waveform: &[f32],
        color: Color32,
    ) {
        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.center().y),
                    Pos2::new(rect.right(), rect.center().y),
                ],
                Stroke::new(1.0, color.linear_multiply(0.3)),
            );
            return;
        }

        let bar_count = (rect.width() / 4.0).clamp(20.0, 120.0) as usize;
        let step = (waveform.len() as f32 / bar_count as f32).max(1.0);
        let bar_width = rect.width() / bar_count as f32;

        for i in 0..bar_count {
            let sample_idx = ((i as f32 * step).round() as usize).min(waveform.len() - 1);
            let level = waveform[sample_idx].clamp(0.06, 1.0);
            let half_h = (level * rect.height() * 0.45).max(1.5);
            let center_x = rect.left() + (i as f32 + 0.5) * bar_width;
            let bar = Rect::from_min_max(
                Pos2::new(center_x - (bar_width * 0.35).max(0.6), rect.center().y - half_h),
                Pos2::new(center_x + (bar_width * 0.35).max(0.6), rect.center().y + half_h),
            );
            painter.rect_filled(bar, 1.0, color);
        }
    }
}
