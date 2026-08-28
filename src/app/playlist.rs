use super::*;
use egui::pos2;
use std::time::{SystemTime, UNIX_EPOCH};

impl SoundFxApp {
    pub(super) fn save_current_playlists(&mut self) {
        let _ = self.storage.save_playlists(&self.playlists);
    }

    pub(super) fn play_active_playlist(&mut self, start_index: usize) {
        let Some(playlist_id) = self.selected_playlist_id else {
            return;
        };
        let Some(playlist) = self.playlists.iter().find(|p| p.id == playlist_id).cloned() else {
            return;
        };
        if playlist.sound_ids.is_empty() {
            return;
        }

        let index = start_index.min(playlist.sound_ids.len() - 1);
        self.playlist_playing_id = Some(playlist.id);
        self.playlist_current_index = index;

        let sound_id = playlist.sound_ids[index];
        self.selected = Some(sound_id);
        self.preview_sound(sound_id);
        if let Some(audio) = self.audio.as_mut() {
            audio.set_volume(self.playlist_volume);
        }
    }

    pub(super) fn stop_active_playlist(&mut self) {
        self.playlist_playing_id = None;
        self.stop_preview();
    }

    pub(super) fn advance_playlist_sound(&mut self) {
        let Some(playing_id) = self.playlist_playing_id else {
            return;
        };
        let Some(playlist) = self.playlists.iter().find(|p| p.id == playing_id).cloned() else {
            self.playlist_playing_id = None;
            return;
        };
        let total = playlist.sound_ids.len();
        if total == 0 {
            self.playlist_playing_id = None;
            return;
        }

        let next_index = if self.playlist_shuffle {
            if total == 1 {
                0
            } else {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as usize)
                    .unwrap_or(1);
                let offset = (nanos % (total - 1)) + 1;
                (self.playlist_current_index + offset) % total
            }
        } else if self.playlist_current_index + 1 < total {
            self.playlist_current_index + 1
        } else if self.playlist_loop {
            0
        } else {
            self.playlist_playing_id = None;
            return;
        };

        self.playlist_current_index = next_index;
        let sound_id = playlist.sound_ids[next_index];
        self.selected = Some(sound_id);
        self.preview_sound(sound_id);
        if let Some(audio) = self.audio.as_mut() {
            audio.set_volume(self.playlist_volume);
        }
    }

    pub(super) fn previous_playlist_sound(&mut self) {
        let Some(playing_id) = self.playlist_playing_id else {
            return;
        };
        let Some(playlist) = self.playlists.iter().find(|p| p.id == playing_id).cloned() else {
            self.playlist_playing_id = None;
            return;
        };
        let total = playlist.sound_ids.len();
        if total == 0 {
            self.playlist_playing_id = None;
            return;
        }

        let prev_index = if self.playlist_shuffle {
            if total == 1 {
                0
            } else {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as usize)
                    .unwrap_or(1);
                let offset = (nanos % (total - 1)) + 1;
                (self.playlist_current_index + total - (offset % total)) % total
            }
        } else if self.playlist_current_index > 0 {
            self.playlist_current_index - 1
        } else if self.playlist_loop {
            total - 1
        } else {
            0
        };

        self.playlist_current_index = prev_index;
        let sound_id = playlist.sound_ids[prev_index];
        self.selected = Some(sound_id);
        self.preview_sound(sound_id);
        if let Some(audio) = self.audio.as_mut() {
            audio.set_volume(self.playlist_volume);
        }
    }

    pub(super) fn render_playlist_panel(&mut self, ctx: &Context) {
        if !self.show_playlist_panel {
            return;
        }

        let mut open_panel = self.show_playlist_panel;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(840.0, 600.0), vec2(460.0, 360.0), 0.0);

        let mut close_request = false;
        let mut create_playlist_requested = false;
        let mut delete_playlist_id: Option<Uuid> = None;
        let mut move_up_action: Option<(Uuid, usize)> = None;
        let mut move_down_action: Option<(Uuid, usize)> = None;
        let mut remove_sound_action: Option<(Uuid, usize)> = None;
        let mut add_sound_action: Option<(Uuid, Uuid)> = None;
        let mut play_track_action: Option<(Uuid, usize)> = None;
        let mut stop_playlist_action = false;
        let mut toggle_shuffle = false;
        let mut toggle_loop = false;

        egui::Window::new("")
            .id(egui::Id::new("playlist-manager-panel"))
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
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                // Header
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe05f, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(self.t("playlist.title"))
                            .size(17.0)
                            .color(Self::strong_text_color())
                            .strong(),
                    );

                    if let Some(playing_id) = self.playlist_playing_id {
                        if let Some(p) = self.playlists.iter().find(|p| p.id == playing_id) {
                            ui.add_space(8.0);
                            let tag_text = format!("● {} ({})", p.name, self.playlist_current_index + 1);
                            ui.label(
                                RichText::new(tag_text)
                                    .size(11.5)
                                    .color(Color32::from_rgb(214, 51, 132)),
                            );
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let close_btn = Self::icon_action(ui, [30.0, 30.0], 0xe5cd, false, false);
                        if close_btn.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);

                let available_height = ui.available_height();

                // Two columns: Left = Playlist list, Right = Playlist sounds & player
                ui.horizontal_top(|ui| {
                    // Left Column (Playlists navigation)
                    ui.allocate_ui_with_layout(
                        vec2(240.0, available_height),
                        egui::Layout::top_down(Align::Min),
                        |ui| {
                            // New playlist input
                            let new_placeholder = self.t("playlist.new_placeholder");
                            let sounds_count_label = self.t("playlist.sounds_count");
                            let delete_label = self.t("playlist.delete");
                            let mut select_playlist_id: Option<Uuid> = None;

                            ui.horizontal(|ui| {
                                ui.set_max_width(240.0);
                                let input_response = ui.add_sized(
                                    [178.0, 28.0],
                                    egui::TextEdit::singleline(&mut self.playlist_new_name)
                                        .hint_text(&new_placeholder),
                                );
                                if input_response.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    && !self.playlist_new_name.trim().is_empty()
                                {
                                    create_playlist_requested = true;
                                }

                                let create_btn = ui.add_sized(
                                    [54.0, 28.0],
                                    Button::new(RichText::new("+").size(15.0).strong())
                                        .fill(Color32::from_rgb(214, 51, 132))
                                        .corner_radius(8.0),
                                );
                                if create_btn.clicked() && !self.playlist_new_name.trim().is_empty() {
                                    create_playlist_requested = true;
                                }
                            });

                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(self.t("playlist.title"))
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
                            );
                            ui.add_space(4.0);

                            // Playlists scroll list
                            ScrollArea::vertical()
                                .id_salt("playlists-left-list")
                                .max_width(240.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(240.0);
                                    if self.playlists.is_empty() {
                                        ui.add_space(20.0);
                                        ui.vertical_centered(|ui| {
                                            ui.label(
                                                RichText::new(self.t("playlist.no_playlists"))
                                                    .size(12.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        });
                                    }

                                    for playlist in &self.playlists {
                                        let is_selected = self.selected_playlist_id == Some(playlist.id);
                                        let is_playing = self.playlist_playing_id == Some(playlist.id);

                                        ui.horizontal(|ui| {
                                            let select_btn = ui.add_sized(
                                                [204.0, 32.0],
                                                Button::new(
                                                    RichText::new(format!(
                                                        "{}{} ({} {})",
                                                        if is_playing { "🔊 " } else { "" },
                                                        playlist.name,
                                                        playlist.sound_ids.len(),
                                                        &sounds_count_label
                                                    ))
                                                    .size(12.5)
                                                    .strong(),
                                                )
                                                .fill(if is_selected {
                                                    Color32::from_rgba_premultiplied(214, 51, 132, 40)
                                                } else {
                                                    Self::surface_fill()
                                                })
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    if is_selected {
                                                        Color32::from_rgb(214, 51, 132)
                                                    } else {
                                                        Self::border_color()
                                                    },
                                                ))
                                                .corner_radius(10.0),
                                            );
                                            if select_btn.clicked() {
                                                select_playlist_id = Some(playlist.id);
                                            }

                                            let del_btn = ui.add_sized(
                                                [24.0, 32.0],
                                                Button::new(Self::icon(0xe872, 13.0, Self::muted_text_color()))
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE)
                                                    .corner_radius(6.0),
                                            );
                                            if del_btn.on_hover_text(&delete_label).clicked() {
                                                delete_playlist_id = Some(playlist.id);
                                            }
                                        });
                                        ui.add_space(3.0);
                                    }
                                });

                            if let Some(id) = select_playlist_id {
                                self.selected_playlist_id = Some(id);
                            }
                        },
                    );

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // Right Column (Active playlist sounds and controls)
                    let right_width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        vec2(right_width, available_height),
                        egui::Layout::top_down(Align::Min),
                        |ui| {
                            let sounds_count_label = self.t("playlist.sounds_count");
                            let selected_id = self.selected_playlist_id;
                            let current_playlist = selected_id.and_then(|id| {
                                self.playlists.iter().find(|p| p.id == id).cloned()
                            });

                            let Some(playlist) = current_playlist else {
                                ui.add_space(80.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(Self::icon(0xe05f, 40.0, Self::muted_text_color()));
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(self.t("playlist.empty_hint"))
                                            .size(13.5)
                                            .color(Self::muted_text_color()),
                                    );
                                });
                                return;
                            };

                            // Playlist title & track count
                            let is_renaming = self.playlist_renaming_id == Some(playlist.id);
                            let mut start_renaming = false;
                            let mut finish_renaming = false;
                            let mut cancel_renaming = false;

                            ui.horizontal(|ui| {
                                if is_renaming {
                                    let edit_res = ui.add_sized(
                                        [180.0, 28.0],
                                        egui::TextEdit::singleline(&mut self.playlist_rename_name),
                                    );
                                    if edit_res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        finish_renaming = true;
                                    }
                                    let ok_btn = ui.add_sized(
                                        [28.0, 28.0],
                                        Button::new(Self::icon(0xe876, 14.0, Color32::from_rgb(100, 200, 100)))
                                            .fill(Self::panel_fill())
                                            .corner_radius(6.0),
                                    );
                                    if ok_btn.clicked() {
                                        finish_renaming = true;
                                    }
                                    let cancel_btn = ui.add_sized(
                                        [28.0, 28.0],
                                        Button::new(Self::icon(0xe5cd, 14.0, Self::muted_text_color()))
                                            .fill(Self::panel_fill())
                                            .corner_radius(6.0),
                                    );
                                    if cancel_btn.clicked() {
                                        cancel_renaming = true;
                                    }
                                } else {
                                    ui.label(
                                        RichText::new(&playlist.name)
                                            .size(18.0)
                                            .color(Self::strong_text_color())
                                            .strong(),
                                    );
                                    let rename_btn = ui.add_sized(
                                        [24.0, 24.0],
                                        Button::new(Self::icon(0xe3c9, 13.0, Self::muted_text_color()))
                                            .fill(Color32::TRANSPARENT)
                                            .stroke(Stroke::NONE)
                                            .corner_radius(6.0),
                                    );
                                    if rename_btn.on_hover_text(self.t("playlist.rename")).clicked() {
                                        start_renaming = true;
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "• {} {}",
                                            playlist.sound_ids.len(),
                                            &sounds_count_label
                                        ))
                                        .size(12.5)
                                        .color(Self::muted_text_color()),
                                    );
                                }
                            });

                            if start_renaming {
                                self.playlist_renaming_id = Some(playlist.id);
                                self.playlist_rename_name = playlist.name.clone();
                            }
                            if finish_renaming {
                                let new_name = self.playlist_rename_name.trim().to_owned();
                                if !new_name.is_empty() {
                                    if let Some(p) = self.playlists.iter_mut().find(|p| p.id == playlist.id) {
                                        p.name = new_name;
                                    }
                                    self.save_current_playlists();
                                }
                                self.playlist_renaming_id = None;
                            }
                            if cancel_renaming {
                                self.playlist_renaming_id = None;
                            }

                            ui.add_space(8.0);

                            // Control toolbar
                            let is_this_playing = self.playlist_playing_id == Some(playlist.id);
                            ui.horizontal(|ui| {
                                // Play / Stop button
                                let play_btn = ui.add_sized(
                                    [86.0, 32.0],
                                    Button::new(
                                        RichText::new(format!(
                                            "{} {}",
                                            if is_this_playing { "⏸" } else { "▶" },
                                            if is_this_playing {
                                                self.t("playlist.pause")
                                            } else {
                                                self.t("playlist.play")
                                            }
                                        ))
                                        .size(12.5)
                                        .strong(),
                                    )
                                    .fill(if is_this_playing {
                                        Color32::from_rgb(214, 51, 132)
                                    } else {
                                        Color32::from_rgb(214, 51, 132)
                                    })
                                    .corner_radius(16.0),
                                );
                                if play_btn.clicked() {
                                    if is_this_playing {
                                        stop_playlist_action = true;
                                    } else {
                                        play_track_action = Some((playlist.id, 0));
                                    }
                                }

                                ui.add_space(4.0);

                                // Shuffle toggle button
                                let shuffle_active = self.playlist_shuffle;
                                let shuffle_btn = ui.add_sized(
                                    [32.0, 32.0],
                                    Button::new(Self::icon(
                                        0xe043,
                                        15.0,
                                        if shuffle_active {
                                            Color32::from_rgb(214, 51, 132)
                                        } else {
                                            Self::muted_text_color()
                                        },
                                    ))
                                    .fill(if shuffle_active {
                                        Color32::from_rgba_premultiplied(214, 51, 132, 40)
                                    } else {
                                        Self::surface_fill()
                                    })
                                    .stroke(Stroke::new(
                                        1.0,
                                        if shuffle_active {
                                            Color32::from_rgb(214, 51, 132)
                                        } else {
                                            Self::border_color()
                                        },
                                    ))
                                    .corner_radius(16.0),
                                );
                                if shuffle_btn.on_hover_text(self.t("playlist.shuffle")).clicked() {
                                    toggle_shuffle = true;
                                }

                                // Loop toggle button
                                let loop_active = self.playlist_loop;
                                let loop_btn = ui.add_sized(
                                    [32.0, 32.0],
                                    Button::new(Self::icon(
                                        0xe040,
                                        15.0,
                                        if loop_active {
                                            Color32::from_rgb(214, 51, 132)
                                        } else {
                                            Self::muted_text_color()
                                        },
                                    ))
                                    .fill(if loop_active {
                                        Color32::from_rgba_premultiplied(214, 51, 132, 40)
                                    } else {
                                        Self::surface_fill()
                                    })
                                    .stroke(Stroke::new(
                                        1.0,
                                        if loop_active {
                                            Color32::from_rgb(214, 51, 132)
                                        } else {
                                            Self::border_color()
                                        },
                                    ))
                                    .corner_radius(16.0),
                                );
                                if loop_btn.on_hover_text(self.t("playlist.loop")).clicked() {
                                    toggle_loop = true;
                                }

                                ui.add_space(8.0);

                                // Add sounds toggle button
                                let add_btn = ui.add_sized(
                                    [110.0, 32.0],
                                    Button::new(
                                        RichText::new(format!("+ {}", self.t("playlist.add_sounds")))
                                            .size(12.0)
                                            .strong(),
                                    )
                                    .fill(if self.show_playlist_add_picker {
                                        Color32::from_rgba_premultiplied(214, 51, 132, 40)
                                    } else {
                                        Self::surface_fill()
                                    })
                                    .stroke(Stroke::new(
                                        1.0,
                                        if self.show_playlist_add_picker {
                                            Color32::from_rgb(214, 51, 132)
                                        } else {
                                            Self::border_color()
                                        },
                                    ))
                                    .corner_radius(16.0),
                                );
                                if add_btn.clicked() {
                                    self.show_playlist_add_picker = !self.show_playlist_add_picker;
                                }
                            });

                            ui.add_space(8.0);

                            // Sound picker dropdown if open
                            if self.show_playlist_add_picker {
                                Frame::new()
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(14.0)
                                    .inner_margin(Margin::same(12))
                                    .show(ui, |ui| {
                                        let search_hint = self.t("playlist.search_sound");
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [ui.available_width() - 30.0, 26.0],
                                                egui::TextEdit::singleline(&mut self.playlist_add_sound_search)
                                                    .hint_text(&search_hint),
                                            );
                                            let close_picker = ui.add_sized(
                                                [24.0, 24.0],
                                                Button::new(Self::icon(0xe5cd, 12.0, Self::muted_text_color()))
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE),
                                            );
                                            if close_picker.clicked() {
                                                self.show_playlist_add_picker = false;
                                            }
                                        });

                                        ui.add_space(6.0);

                                        let query = self.playlist_add_sound_search.trim().to_lowercase();
                                        let matching_sounds: Vec<_> = self
                                            .sounds
                                            .iter()
                                            .filter(|s| {
                                                query.is_empty() || s.name.to_lowercase().contains(&query)
                                            })
                                            .take(30)
                                            .cloned()
                                            .collect();

                                        let choose_label = self.t("playlist.choose");
                                        let added_label = self.t("playlist.added");

                                        ScrollArea::vertical()
                                            .id_salt("playlist-picker-sounds")
                                            .max_height(140.0)
                                            .show(ui, |ui| {
                                                for sound in matching_sounds {
                                                    let is_in = playlist.sound_ids.contains(&sound.id);
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            RichText::new(&sound.name)
                                                                .size(12.0)
                                                                .color(Self::strong_text_color()),
                                                        );
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(Align::Center),
                                                            |ui| {
                                                                let btn = ui.add_sized(
                                                                    [64.0, 24.0],
                                                                    Button::new(
                                                                        RichText::new(if is_in {
                                                                            format!("+ {}", added_label)
                                                                        } else {
                                                                            format!("+ {}", choose_label)
                                                                        })
                                                                        .size(11.0)
                                                                        .color(if is_in {
                                                                            Color32::from_rgb(100, 200, 100)
                                                                        } else {
                                                                            Self::strong_text_color()
                                                                        }),
                                                                    )
                                                                    .fill(Self::panel_fill())
                                                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                                                    .corner_radius(6.0),
                                                                );
                                                                Self::decorate_button_response(ui, &btn);
                                                                if btn.clicked() {
                                                                    add_sound_action = Some((playlist.id, sound.id));
                                                                }
                                                                ui.label(
                                                                    RichText::new(format_time(
                                                                        sound.display_duration_secs(),
                                                                    ))
                                                                    .size(11.0)
                                                                    .color(Self::muted_text_color()),
                                                                );
                                                            },
                                                        );
                                                    });
                                                    ui.add_space(2.0);
                                                }
                                            });
                                    });
                                ui.add_space(8.0);
                            }

                            // Sound list in current playlist
                            ScrollArea::vertical()
                                .id_salt("playlist-sounds-list")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if playlist.sound_ids.is_empty() {
                                        ui.add_space(40.0);
                                        ui.vertical_centered(|ui| {
                                            ui.label(
                                                RichText::new(self.t("playlist.empty"))
                                                    .size(13.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(self.t("playlist.empty_hint"))
                                                    .size(11.5)
                                                    .color(Self::muted_text_color()),
                                            );
                                        });
                                        return;
                                    }

                                    let total_tracks = playlist.sound_ids.len();
                                    let remove_label = self.t("playlist.remove");
                                    let move_down_label = self.t("playlist.move_down");
                                    let move_up_label = self.t("playlist.move_up");
                                    let play_track_label = self.t("playlist.play_track");
                                    for (idx, &sound_id) in playlist.sound_ids.iter().enumerate() {
                                        let sound_opt = self.sounds.iter().find(|s| s.id == sound_id);
                                        let is_current_track = is_this_playing && self.playlist_current_index == idx;

                                        Frame::new()
                                            .fill(if is_current_track {
                                                Color32::from_rgba_premultiplied(214, 51, 132, 30)
                                            } else {
                                                Self::surface_fill()
                                            })
                                            .stroke(Stroke::new(
                                                1.0,
                                                if is_current_track {
                                                    Color32::from_rgb(214, 51, 132)
                                                } else {
                                                    Self::border_color()
                                                },
                                            ))
                                            .corner_radius(10.0)
                                            .inner_margin(Margin::symmetric(10, 6))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    // Index or playing icon
                                                    if is_current_track {
                                                        ui.label(
                                                            Self::icon(0xe050, 14.0, Color32::from_rgb(214, 51, 132)),
                                                        );
                                                    } else {
                                                        ui.label(
                                                            RichText::new(format!("#{}", idx + 1))
                                                                .size(11.5)
                                                                .color(Self::muted_text_color())
                                                                .monospace(),
                                                        );
                                                    }
                                                    ui.add_space(4.0);

                                                    // Sound Name
                                                    let sound_name = sound_opt
                                                        .map(|s| s.name.as_str())
                                                        .unwrap_or("(Sound removed)");
                                                    ui.label(
                                                        RichText::new(sound_name)
                                                            .size(12.5)
                                                            .color(if is_current_track {
                                                                Color32::WHITE
                                                            } else {
                                                                Self::strong_text_color()
                                                            })
                                                            .strong(),
                                                    );

                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            // Remove button
                                                            let remove_btn = ui.add_sized(
                                                                [26.0, 26.0],
                                                                Button::new(
                                                                    Self::icon(0xe5cd, 13.0, Color32::from_rgb(220, 80, 80)),
                                                                )
                                                                .fill(Self::panel_fill())
                                                                .stroke(Stroke::new(1.0, Self::border_color()))
                                                                .corner_radius(6.0),
                                                            ).on_hover_text(&remove_label);
                                                            Self::decorate_button_response(ui, &remove_btn);
                                                            if remove_btn.clicked() {
                                                                remove_sound_action = Some((playlist.id, idx));
                                                            }

                                                            // Move Down button
                                                            let down_btn = ui.add_enabled_ui(idx + 1 < total_tracks, |ui| {
                                                                let res = ui.add_sized(
                                                                    [26.0, 26.0],
                                                                    Button::new(
                                                                        Self::icon(0xe5cf, 14.0, Self::strong_text_color()),
                                                                    )
                                                                    .fill(Self::panel_fill())
                                                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                                                    .corner_radius(6.0),
                                                                ).on_hover_text(&move_down_label);
                                                                Self::decorate_button_response(ui, &res);
                                                                res
                                                            }).inner;
                                                            if down_btn.clicked() {
                                                                move_down_action = Some((playlist.id, idx));
                                                            }

                                                            // Move Up button
                                                            let up_btn = ui.add_enabled_ui(idx > 0, |ui| {
                                                                let res = ui.add_sized(
                                                                    [26.0, 26.0],
                                                                    Button::new(
                                                                        Self::icon(0xe5ce, 14.0, Self::strong_text_color()),
                                                                    )
                                                                    .fill(Self::panel_fill())
                                                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                                                    .corner_radius(6.0),
                                                                ).on_hover_text(&move_up_label);
                                                                Self::decorate_button_response(ui, &res);
                                                                res
                                                            }).inner;
                                                            if up_btn.clicked() {
                                                                move_up_action = Some((playlist.id, idx));
                                                            }

                                                            // Play this track button
                                                            let play_track_btn = ui.add_sized(
                                                                [26.0, 26.0],
                                                                Button::new(
                                                                    Self::icon(0xe037, 13.0, Color32::from_rgb(214, 51, 132)),
                                                                )
                                                                .fill(Self::panel_fill())
                                                                .stroke(Stroke::new(1.0, Self::border_color()))
                                                                .corner_radius(6.0),
                                                            ).on_hover_text(&play_track_label);
                                                            Self::decorate_button_response(ui, &play_track_btn);
                                                            if play_track_btn.clicked() {
                                                                play_track_action = Some((playlist.id, idx));
                                                            }

                                                            // Duration
                                                            if let Some(sound) = sound_opt {
                                                                ui.label(
                                                                    RichText::new(format_time(
                                                                        sound.display_duration_secs(),
                                                                    ))
                                                                    .size(11.5)
                                                                    .color(Self::muted_text_color()),
                                                                );
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        ui.add_space(4.0);
                                    }
                                });
                        },
                    );
                });
            });

        if close_request || !open_panel {
            self.show_playlist_panel = false;
        }

        // Apply actions
        if create_playlist_requested {
            let name = self.playlist_new_name.trim().to_owned();
            self.playlist_new_name.clear();
            let new_playlist = Playlist {
                id: Uuid::new_v4(),
                name,
                sound_ids: Vec::new(),
            };
            let id = new_playlist.id;
            self.playlists.push(new_playlist);
            self.selected_playlist_id = Some(id);
            self.save_current_playlists();
        }

        if let Some(del_id) = delete_playlist_id {
            self.playlists.retain(|p| p.id != del_id);
            if self.selected_playlist_id == Some(del_id) {
                self.selected_playlist_id = self.playlists.first().map(|p| p.id);
            }
            if self.playlist_playing_id == Some(del_id) {
                self.stop_active_playlist();
            }
            self.save_current_playlists();
        }

        if let Some((p_id, idx)) = move_up_action {
            if let Some(playlist) = self.playlists.iter_mut().find(|p| p.id == p_id) {
                if idx > 0 && idx < playlist.sound_ids.len() {
                    playlist.sound_ids.swap(idx, idx - 1);
                    if self.playlist_playing_id == Some(p_id) {
                        if self.playlist_current_index == idx {
                            self.playlist_current_index = idx - 1;
                        } else if self.playlist_current_index == idx - 1 {
                            self.playlist_current_index = idx;
                        }
                    }
                }
            }
            self.save_current_playlists();
        }

        if let Some((p_id, idx)) = move_down_action {
            if let Some(playlist) = self.playlists.iter_mut().find(|p| p.id == p_id) {
                if idx + 1 < playlist.sound_ids.len() {
                    playlist.sound_ids.swap(idx, idx + 1);
                    if self.playlist_playing_id == Some(p_id) {
                        if self.playlist_current_index == idx {
                            self.playlist_current_index = idx + 1;
                        } else if self.playlist_current_index == idx + 1 {
                            self.playlist_current_index = idx;
                        }
                    }
                }
            }
            self.save_current_playlists();
        }

        if let Some((p_id, idx)) = remove_sound_action {
            if let Some(playlist) = self.playlists.iter_mut().find(|p| p.id == p_id) {
                if idx < playlist.sound_ids.len() {
                    playlist.sound_ids.remove(idx);
                    if self.playlist_playing_id == Some(p_id) {
                        if self.playlist_current_index >= playlist.sound_ids.len() {
                            self.playlist_current_index = 0;
                        }
                    }
                }
            }
            self.save_current_playlists();
        }

        if let Some((p_id, s_id)) = add_sound_action {
            if let Some(playlist) = self.playlists.iter_mut().find(|p| p.id == p_id) {
                playlist.sound_ids.push(s_id);
            }
            self.save_current_playlists();
        }

        if let Some((p_id, idx)) = play_track_action {
            self.selected_playlist_id = Some(p_id);
            self.play_active_playlist(idx);
        }

        if stop_playlist_action {
            self.stop_active_playlist();
        }

        if toggle_shuffle {
            self.playlist_shuffle = !self.playlist_shuffle;
        }

        if toggle_loop {
            self.playlist_loop = !self.playlist_loop;
        }
    }

    pub(super) fn render_playlist_bottom_bar(&mut self, ctx: &Context) {
        let Some(playing_id) = self.playlist_playing_id else {
            self.playlist_scrubbing_pos = None;
            self.playlist_volume_popup_open = false;
            return;
        };
        let Some(playlist) = self.playlists.iter().find(|p| p.id == playing_id).cloned() else {
            self.playlist_playing_id = None;
            self.playlist_scrubbing_pos = None;
            self.playlist_volume_popup_open = false;
            return;
        };
        let sound_id = playlist.sound_ids.get(self.playlist_current_index).copied();
        let sound_opt = sound_id.and_then(|id| self.sounds.iter().find(|s| s.id == id).cloned());

        let is_playing = sound_id.is_some_and(|id| self.audio.as_ref().is_some_and(|a| a.is_playing(id)));
        let is_paused = self.audio.as_ref().is_some_and(|a| a.is_paused());
        let position_secs = sound_id
            .and_then(|id| self.audio.as_ref().and_then(|a| a.playback_position_secs(id)))
            .unwrap_or(0.0);
        let duration_secs = sound_opt.as_ref().map(|s| s.display_duration_secs()).unwrap_or(0.0);
        let max_secs = duration_secs.max(0.1);

        let mut prev_action = false;
        let mut toggle_play_action = false;
        let mut next_action = false;
        let mut toggle_shuffle = false;
        let mut toggle_loop = false;
        let mut seek_action: Option<f32> = None;
        let mut volume_changed: Option<f32> = None;
        let mut open_panel_action = false;
        let mut close_bar_action = false;
        let mut vol_btn_rect: Option<Rect> = None;
        let mut vol_btn_hovered = false;

        let prev_label = self.t("playlist.previous");
        let next_label = self.t("playlist.next");
        let play_label = self.t("playlist.play");
        let pause_label = self.t("playlist.pause");
        let shuffle_label = self.t("playlist.shuffle");
        let loop_label = self.t("playlist.loop");
        let volume_label = self.t("playlist.volume");
        let open_panel_label = self.t("playlist.open_panel");
        let close_bar_label = self.t("playlist.close_bar");

        let screen_rect = self.app_frame_rect.unwrap_or_else(|| ctx.screen_rect());
        let bar_height = 54.0;
        let bar_width = (screen_rect.width() - 32.0).clamp(460.0, 1100.0);
        let bar_x = screen_rect.center().x - bar_width * 0.5;
        let bar_y = screen_rect.bottom() - bar_height - 12.0;
        let bar_pos = pos2(bar_x, bar_y);

        egui::Area::new(egui::Id::new("playlist-bottom-bar-popup"))
            .order(egui::Order::Middle)
            .fixed_pos(bar_pos)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(bar_width);
                ui.set_height(bar_height);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.2, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(0, 0, 0, 70),
                    })
                    .corner_radius(18.0)
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Controls cluster: Prev, Play/Pause, Next, Shuffle, Loop
                            let prev_btn = ui.add_sized(
                                [30.0, 30.0],
                                Button::new(Self::icon(0xe045, 16.0, Self::strong_text_color()))
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(15.0),
                            ).on_hover_text(&prev_label);
                            Self::decorate_button_response(ui, &prev_btn);
                            if prev_btn.clicked() {
                                prev_action = true;
                            }

                            let play_icon = if is_playing && !is_paused { 0xe034 } else { 0xe037 };
                            let play_btn = ui.add_sized(
                                [34.0, 34.0],
                                Button::new(Self::icon(play_icon, 18.0, Color32::WHITE))
                                    .fill(Color32::from_rgb(214, 51, 132))
                                    .stroke(Stroke::NONE)
                                    .corner_radius(17.0),
                            ).on_hover_text(if is_playing && !is_paused { &pause_label } else { &play_label });
                            Self::decorate_button_response(ui, &play_btn);
                            if play_btn.clicked() {
                                toggle_play_action = true;
                            }

                            let next_btn = ui.add_sized(
                                [30.0, 30.0],
                                Button::new(Self::icon(0xe044, 16.0, Self::strong_text_color()))
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(15.0),
                            ).on_hover_text(&next_label);
                            Self::decorate_button_response(ui, &next_btn);
                            if next_btn.clicked() {
                                next_action = true;
                            }

                            ui.add_space(2.0);

                            let shuffle_active = self.playlist_shuffle;
                            let shuffle_btn = ui.add_sized(
                                [26.0, 26.0],
                                Button::new(Self::icon(
                                    0xe043,
                                    14.0,
                                    if shuffle_active { Color32::from_rgb(214, 51, 132) } else { Self::muted_text_color() },
                                ))
                                .fill(if shuffle_active { Color32::from_rgba_premultiplied(214, 51, 132, 36) } else { Color32::TRANSPARENT })
                                .stroke(Stroke::NONE)
                                .corner_radius(6.0),
                            ).on_hover_text(&shuffle_label);
                            Self::decorate_button_response(ui, &shuffle_btn);
                            if shuffle_btn.clicked() {
                                toggle_shuffle = true;
                            }

                            let loop_active = self.playlist_loop;
                            let loop_btn = ui.add_sized(
                                [26.0, 26.0],
                                Button::new(Self::icon(
                                    0xe040,
                                    14.0,
                                    if loop_active { Color32::from_rgb(214, 51, 132) } else { Self::muted_text_color() },
                                ))
                                .fill(if loop_active { Color32::from_rgba_premultiplied(214, 51, 132, 36) } else { Color32::TRANSPARENT })
                                .stroke(Stroke::NONE)
                                .corner_radius(6.0),
                            ).on_hover_text(&loop_label);
                            Self::decorate_button_response(ui, &loop_btn);
                            if loop_btn.clicked() {
                                toggle_loop = true;
                            }

                            ui.add_space(6.0);

                            // Current time display (updates smoothly with drag)
                            let current_display_secs = self.playlist_scrubbing_pos.unwrap_or(position_secs).clamp(0.0, max_secs);
                            ui.label(
                                RichText::new(format_time(current_display_secs))
                                    .size(11.5)
                                    .monospace()
                                    .color(Self::muted_text_color()),
                            );

                            // Custom Timeline Slider: wide, high contrast, seek only on release!
                            let reserved_right_width = 310.0;
                            let slider_width = (ui.available_width() - reserved_right_width).max(80.0);
                            let (timeline_rect, timeline_resp) = ui.allocate_exact_size(vec2(slider_width, 18.0), Sense::click_and_drag());
                            Self::decorate_button_response(ui, &timeline_resp);

                            let is_dragged = timeline_resp.dragged();
                            let is_clicked = timeline_resp.clicked();
                            if is_dragged || is_clicked {
                                if let Some(mouse_pos) = timeline_resp.interact_pointer_pos() {
                                    let frac = ((mouse_pos.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0);
                                    let target_secs = frac * max_secs;
                                    self.playlist_scrubbing_pos = Some(target_secs);
                                }
                            }

                            if timeline_resp.drag_stopped() || (is_clicked && !is_dragged) {
                                if let Some(target_secs) = self.playlist_scrubbing_pos.take() {
                                    seek_action = Some(target_secs);
                                }
                            }

                            let display_frac = if max_secs > 0.0 {
                                (current_display_secs / max_secs).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };

                            let is_timeline_hovered = timeline_resp.hovered() || is_dragged;
                            let track_h = if is_timeline_hovered { 6.0 } else { 4.5 };
                            let track_r = track_h * 0.5;
                            let track_y = timeline_rect.center().y;

                            // Distinct dark slate background track (never blends with panel background)
                            let bg_track = Rect::from_min_max(
                                pos2(timeline_rect.left(), track_y - track_r),
                                pos2(timeline_rect.right(), track_y + track_r),
                            );
                            ui.painter().rect_filled(bg_track, track_r, Color32::from_rgb(52, 44, 58));
                            ui.painter().rect_stroke(bg_track, track_r, Stroke::new(1.0, Color32::from_rgb(76, 64, 84)), StrokeKind::Inside);

                            // Active filled magenta track
                            let fill_x = (timeline_rect.left() + timeline_rect.width() * display_frac).min(timeline_rect.right());
                            if fill_x > timeline_rect.left() {
                                let filled_track = Rect::from_min_max(
                                    pos2(timeline_rect.left(), track_y - track_r),
                                    pos2(fill_x, track_y + track_r),
                                );
                                ui.painter().rect_filled(filled_track, track_r, Color32::from_rgb(214, 51, 132));
                            }

                            // Thumb indicator
                            let thumb_radius = if is_dragged { 7.5 } else if is_timeline_hovered { 6.5 } else { 5.0 };
                            let thumb_pos = pos2(fill_x, track_y);
                            ui.painter().circle_filled(thumb_pos, thumb_radius, Color32::WHITE);
                            ui.painter().circle_stroke(thumb_pos, thumb_radius, Stroke::new(2.0, Color32::from_rgb(214, 51, 132)));

                            // Duration display
                            ui.label(
                                RichText::new(format_time(duration_secs))
                                    .size(11.5)
                                    .monospace()
                                    .color(Self::muted_text_color()),
                            );

                            ui.add_space(6.0);

                            // Volume button (triggers vertical popup slider)
                            let vol_icon = if self.playlist_volume <= 0.01 {
                                0xe04f
                            } else if self.playlist_volume < 0.5 {
                                0xe04d
                            } else {
                                0xe050
                            };
                            let vol_btn = ui.add_sized(
                                [28.0, 28.0],
                                Button::new(Self::icon(
                                    vol_icon,
                                    15.0,
                                    if self.playlist_volume_popup_open {
                                        Color32::from_rgb(214, 51, 132)
                                    } else {
                                        Self::strong_text_color()
                                    },
                                ))
                                .fill(if self.playlist_volume_popup_open {
                                    Color32::from_rgba_premultiplied(214, 51, 132, 36)
                                } else {
                                    Self::surface_fill()
                                })
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(8.0),
                            ).on_hover_text(&volume_label);
                            Self::decorate_button_response(ui, &vol_btn);
                            vol_btn_rect = Some(vol_btn.rect);
                            vol_btn_hovered = vol_btn.hovered();

                            if vol_btn.clicked() {
                                self.playlist_volume_popup_open = !self.playlist_volume_popup_open;
                            }

                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            // Track info (strictly single-line truncated to prevent overlap)
                            let track_title = sound_opt.as_ref().map(|s| s.name.as_str()).unwrap_or("(No track)");
                            ui.allocate_ui_with_layout(
                                vec2(130.0, 32.0),
                                egui::Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_max_width(130.0);
                                    let title_label = egui::Label::new(
                                        RichText::new(track_title)
                                            .size(11.0)
                                            .strong()
                                            .color(Self::strong_text_color()),
                                    )
                                    .truncate();
                                    ui.add(title_label).on_hover_text(track_title);

                                    let pl_label = egui::Label::new(
                                        RichText::new(&playlist.name)
                                            .size(9.5)
                                            .color(Self::muted_text_color()),
                                    )
                                    .truncate();
                                    ui.add(pl_label).on_hover_text(&playlist.name);
                                },
                            );

                            ui.add_space(4.0);

                            // Right actions: Open panel, Close bar (arranged linearly next to track info)
                            let open_panel_btn = ui.add_sized(
                                [26.0, 26.0],
                                Button::new(Self::icon(0xe05f, 14.0, Color32::from_rgb(214, 51, 132)))
                                    .fill(Color32::from_rgba_premultiplied(214, 51, 132, 24))
                                    .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(214, 51, 132, 60)))
                                    .corner_radius(6.0),
                            ).on_hover_text(&open_panel_label);
                            Self::decorate_button_response(ui, &open_panel_btn);
                            if open_panel_btn.clicked() {
                                open_panel_action = true;
                            }

                            ui.add_space(2.0);

                            let close_bar_btn = ui.add_sized(
                                [26.0, 26.0],
                                Button::new(Self::icon(0xe5cd, 13.0, Self::muted_text_color()))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE)
                                    .corner_radius(6.0),
                            ).on_hover_text(&close_bar_label);
                            Self::decorate_button_response(ui, &close_bar_btn);
                            if close_bar_btn.clicked() {
                                close_bar_action = true;
                            }
                        });
                    });
            });

        // Vertical Volume Slider Popup (floating right above volume button)
        let show_vol_popup = self.playlist_volume_popup_open || vol_btn_hovered;
        if show_vol_popup {
            if let Some(btn_r) = vol_btn_rect {
                let popup_width = 44.0;
                let popup_height = 136.0;
                let popup_pos = pos2(btn_r.center().x - popup_width * 0.5, btn_r.top() - popup_height - 6.0);

                let mut popup_hovered = false;
                egui::Area::new(egui::Id::new("playlist-volume-popup-area"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(popup_pos)
                    .interactable(true)
                    .show(ctx, |ui| {
                        ui.set_width(popup_width);
                        ui.set_height(popup_height);
                        Frame::new()
                            .fill(Self::panel_fill())
                            .stroke(Stroke::new(1.2, Self::border_color()))
                            .shadow(Shadow {
                                offset: [0, 8],
                                blur: 18,
                                spread: 0,
                                color: Color32::from_rgba_premultiplied(0, 0, 0, 80),
                            })
                            .corner_radius(12.0)
                            .inner_margin(Margin::symmetric(8, 10))
                            .show(ui, |ui| {
                                popup_hovered = ui.rect_contains_pointer(ui.max_rect());
                                ui.vertical_centered(|ui| {
                                    let pct = (self.playlist_volume * 100.0).round();
                                    ui.label(
                                        RichText::new(format!("{:.0}%", pct))
                                            .size(11.0)
                                            .monospace()
                                            .strong()
                                            .color(Self::strong_text_color()),
                                    );
                                    ui.add_space(6.0);

                                    // Vertical slider track: drag UP increases volume, drag DOWN decreases
                                    let track_size = vec2(18.0, 86.0);
                                    let (vol_rect, vol_resp) = ui.allocate_exact_size(track_size, Sense::click_and_drag());
                                    Self::decorate_button_response(ui, &vol_resp);

                                    if vol_resp.dragged() || vol_resp.clicked() {
                                        if let Some(pos) = vol_resp.interact_pointer_pos() {
                                            let frac = ((vol_rect.bottom() - pos.y) / vol_rect.height()).clamp(0.0, 1.0);
                                            volume_changed = Some((frac * 2.0).clamp(0.0, 2.0));
                                        }
                                    }

                                    // Mouse wheel over volume area
                                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                                    if (vol_resp.hovered() || vol_btn_hovered) && scroll.abs() > 0.1 {
                                        let delta = if scroll > 0.0 { 0.05 } else { -0.05 };
                                        volume_changed = Some((self.playlist_volume + delta).clamp(0.0, 2.0));
                                    }

                                    // Render vertical track
                                    let vol_frac = (self.playlist_volume / 2.0).clamp(0.0, 1.0);
                                    let track_x = vol_rect.center().x;
                                    let track_w = 4.5;
                                    let track_r = track_w * 0.5;

                                    let bg_v_track = Rect::from_min_max(
                                        pos2(track_x - track_r, vol_rect.top()),
                                        pos2(track_x + track_r, vol_rect.bottom()),
                                    );
                                    ui.painter().rect_filled(bg_v_track, track_r, Color32::from_rgb(52, 44, 58));
                                    ui.painter().rect_stroke(bg_v_track, track_r, Stroke::new(1.0, Color32::from_rgb(76, 64, 84)), StrokeKind::Inside);

                                    let fill_y = vol_rect.bottom() - vol_rect.height() * vol_frac;
                                    let filled_v_track = Rect::from_min_max(
                                        pos2(track_x - track_r, fill_y),
                                        pos2(track_x + track_r, vol_rect.bottom()),
                                    );
                                    ui.painter().rect_filled(filled_v_track, track_r, Color32::from_rgb(214, 51, 132));

                                    // Thumb
                                    let thumb_y = fill_y;
                                    let thumb_pt = pos2(track_x, thumb_y);
                                    let thumb_r = if vol_resp.dragged() { 7.0 } else if vol_resp.hovered() { 6.0 } else { 5.0 };
                                    ui.painter().circle_filled(thumb_pt, thumb_r, Color32::WHITE);
                                    ui.painter().circle_stroke(thumb_pt, thumb_r, Stroke::new(2.0, Color32::from_rgb(214, 51, 132)));
                                });
                            });
                    });

                if !popup_hovered && !vol_btn_hovered && ctx.input(|i| i.pointer.any_click()) {
                    self.playlist_volume_popup_open = false;
                }
            }
        }

        if prev_action {
            self.previous_playlist_sound();
        }
        if toggle_play_action {
            if let Some(audio) = self.audio.as_mut() {
                if is_playing {
                    if is_paused {
                        audio.resume();
                    } else {
                        audio.pause();
                    }
                } else {
                    self.play_active_playlist(self.playlist_current_index);
                }
            }
        }
        if next_action {
            self.advance_playlist_sound();
        }
        if toggle_shuffle {
            self.playlist_shuffle = !self.playlist_shuffle;
        }
        if toggle_loop {
            self.playlist_loop = !self.playlist_loop;
        }
        if let Some(seek_pos) = seek_action {
            if let Some(s_id) = sound_id {
                self.preview_sound_from_position(s_id, Some(seek_pos));
                if let Some(audio) = self.audio.as_mut() {
                    audio.set_volume(self.playlist_volume);
                }
            }
        }
        if let Some(new_vol) = volume_changed {
            self.playlist_volume = new_vol;
            if let Some(audio) = self.audio.as_mut() {
                audio.set_volume(new_vol);
            }
        }
        if open_panel_action {
            self.show_playlist_panel = true;
        }
        if close_bar_action {
            self.stop_active_playlist();
        }
    }
}
