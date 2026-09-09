use super::*;

impl SoundFxApp {
    pub(super) fn main_frame_corner_radius(rect: Rect) -> CornerRadius {
        let max_radius = ((rect.width().min(rect.height()) * 0.5) - 1.0).max(0.0);
        CornerRadius::same(APP_FRAME_RADIUS.min(max_radius).round().clamp(0.0, 255.0) as u8)
    }

    pub(super) fn main_frame_inner_margin(rect: Rect) -> i8 {
        let padding = (rect.width().min(rect.height()) * 0.04).clamp(18.0, 30.0);
        padding.round() as i8
    }

    pub(super) fn render_root_view(&mut self, ctx: &Context, live_ui_overlay_alpha: f32) {
        if self.overlay_only_mode
            && !self.recorder.snapshot().running
            && !self.pitch_monitor.snapshot().running
        {
            self.overlay_only_mode = false;
        }

        if self.overlay_only_mode {
            if self.recorder.snapshot().running && self.center_record_overlay_next_frame {
                Self::apply_overlay_only_viewport(ctx, vec2(430.0, 118.0));
            } else if self.pitch_monitor.snapshot().running && self.center_pitch_overlay_next_frame
            {
                let overlay_size = if self.pitch_overlay_animation {
                    vec2(276.0, 276.0)
                } else {
                    vec2(430.0, 104.0)
                };
                Self::apply_overlay_only_viewport(ctx, overlay_size);
            }
            self.app_frame_rect = None;
            CentralPanel::default()
                .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(0.0))
                .show(ctx, |_ui| {});
            self.render_record_overlay_viewport(ctx);
            self.render_pitch_overlay_viewport(ctx);
            return;
        }

        let root_fill = Color32::TRANSPARENT;
        CentralPanel::default()
            .frame(Frame::new().fill(root_fill).inner_margin(0.0))
            .show(ctx, |ui| {
                let frame_rect = ui.max_rect();
                let frame_radius = Self::main_frame_corner_radius(frame_rect);
                let frame_margin = Self::main_frame_inner_margin(frame_rect);
                ui.painter()
                    .rect_filled(frame_rect, frame_radius, Self::page_fill());
                ui.painter().rect_stroke(
                    frame_rect,
                    frame_radius,
                    Stroke::new(1.0, Self::border_color()),
                    StrokeKind::Inside,
                );
                let frame_response = Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .shadow(Shadow::NONE)
                    .corner_radius(frame_radius)
                    .outer_margin(Margin::same(APP_OUTER_MARGIN as i8))
                    .inner_margin(Margin::same(frame_margin))
                    .show(ui, |ui| {
                        self.draw_titlebar(ui, ctx);
                        ui.add_space(14.0);

                        let content_height = ui.available_height();
                        if self.app_view == AppView::Library {
                            self.draw_library_grid(ui);
                        } else if self.editing_from_folder.is_some() {
                            ui.allocate_ui_with_layout(
                                vec2(ui.available_width(), content_height),
                                egui::Layout::top_down(Align::Min),
                                |ui| self.draw_editor(ui, ctx),
                            );
                        } else {
                            ui.horizontal_top(|ui| {
                                let library_width =
                                    (ui.available_width() * 0.31).clamp(280.0, 360.0);
                                ui.allocate_ui_with_layout(
                                    vec2(library_width, content_height),
                                    egui::Layout::top_down(Align::Min),
                                    |ui| self.draw_library(ui),
                                );
                                ui.add_space(12.0);
                                ui.allocate_ui_with_layout(
                                    vec2(ui.available_width(), content_height),
                                    egui::Layout::top_down(Align::Min),
                                    |ui| self.draw_editor(ui, ctx),
                                );
                            });
                        }
                    });
                self.app_frame_rect = Some(frame_rect);

                if live_ui_overlay_alpha > 0.0 {
                    ui.painter().rect(
                        frame_response.response.rect,
                        frame_radius,
                        Color32::from_rgba_premultiplied(
                            0,
                            0,
                            0,
                            (255.0 * live_ui_overlay_alpha).round().clamp(0.0, 255.0) as u8,
                        ),
                        Stroke::new(
                            1.0,
                            Color32::from_rgba_premultiplied(
                                30,
                                25,
                                36,
                                (124.0 * live_ui_overlay_alpha).round().clamp(0.0, 255.0) as u8,
                            ),
                        ),
                        StrokeKind::Inside,
                    );
                }
            });

        self.render_playlist_bottom_bar(ctx);
        self.render_modal_backdrop(ctx);
        self.render_download_panel(ctx);
        self.render_myinstants_panel(ctx);
        self.render_playlist_panel(ctx);
        self.render_import_panel(ctx);
        self.render_record_panel(ctx);
        self.render_record_review_panel(ctx);
        self.render_stream_panel(ctx);
        self.render_settings_panel(ctx);
        self.render_update_notice(ctx);
        self.render_video_viewer_panel(ctx);
        self.render_pitch_monitor(ctx);
        self.render_trim_commit_panel(ctx);
        self.render_delete_folder_confirm_panel(ctx);
        self.render_pitch_overlay_viewport(ctx);
        self.render_custom_window_resize_handles(ctx);
        self.maybe_start_pending_processed_export();
    }

    pub(super) fn handle_external_file_hover(&mut self, ctx: &Context) {
        let is_library_sound_drop =
            self.app_view == AppView::Library && self.library_tab == LibraryTab::Sounds;
        let external_file_hover = (self.app_view == AppView::Editor || is_library_sound_drop)
            && !self.has_modal_panel()
            && ctx.input(|input| !input.raw.hovered_files.is_empty());
        if external_file_hover {
            ctx.request_repaint_after(Duration::from_millis(16));
            let pointer_over_drop = if is_library_sound_drop {
                self.external_drop_pointer_pos(ctx)
                    .and_then(|pos| self.resolve_library_drop_target_at(pos))
                    .is_some()
            } else {
                self.external_drop_pointer_pos(ctx)
                    .zip(self.editor_drop_rect)
                    .is_some_and(|(pos, rect)| rect.contains(pos))
            };
            self.editor_drop_armed = pointer_over_drop;
            if pointer_over_drop {
                ctx.set_cursor_icon(egui::CursorIcon::Copy);
            }
        } else if !ctx.input(|input| !input.raw.dropped_files.is_empty()) {
            self.editor_drop_armed = false;
        }

        if !self.is_transition_active() {
            self.handle_dropped_files(ctx);
        }
    }
    pub(super) fn draw_titlebar(&mut self, ui: &mut Ui, ctx: &Context) {
        let titlebar_rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 44.0));
        let titlebar_drag = ui.interact(
            titlebar_rect,
            ui.id().with("titlebar-drag"),
            Sense::click_and_drag(),
        );
        self.titlebar_drag_rect = Some(titlebar_rect);
        if titlebar_drag.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if titlebar_drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if titlebar_drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let downloader_snapshot = self.downloader.snapshot();
        let download_titlebar_active = downloader_snapshot.running && !self.show_download_panel;
        ui.horizontal(|ui| {
            let drag_width = (ui.available_width() - 718.0).max(180.0);
            ui.allocate_ui_with_layout(
                    vec2(drag_width, 44.0),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        Frame::new()
                            .fill(if self.dark_theme {
                                Color32::from_rgba_premultiplied(35, 29, 41, 236)
                            } else {
                                Color32::from_rgba_premultiplied(255, 246, 250, 238)
                            })
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(12.0)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    Self::paint_titlebar_blob(ui);
                                    ui.add_space(10.0);
                                    Self::paint_titlebar_wave(ui);
                                });
                            });
                    },
                );

            ui.add_space(8.0);
            let theme_response = ui.add_sized(
                [42.0, 30.0],
                Self::titlebar_button(
                    RichText::new(" ")
                        .family(FontFamily::Proportional)
                        .size(15.5)
                        .color(Color32::TRANSPARENT),
                    self.dark_theme,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &theme_response);
            if theme_response.clicked() {
                self.dark_theme = !self.dark_theme;
                let _ = self.storage.save_dark_theme(self.dark_theme);
                Self::apply_theme(ctx, self.dark_theme);
            }
            Self::paint_theme_titlebar_icon(ui.painter(), theme_response.rect, self.dark_theme);

            if let Some(status) = &self.status {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(status)
                        .size(12.0)
                        .color(Color32::from_rgb(171, 54, 91)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);

                if Self::icon_titlebar(ui, [38.0, 30.0], 0xe5cd, false, false).clicked() {
                    self.request_close(ctx);
                }

                if Self::icon_titlebar(ui, [38.0, 30.0], 0xe15b, false, false).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }

                let library_btn = ui.add_sized(
                    [92.0, 30.0],
                    Self::titlebar_button(
                        RichText::new("Sound Library").size(11.5),
                        self.app_view == AppView::Library,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &library_btn);
                if library_btn.clicked() {
                    if self.app_view == AppView::Library {
                        self.app_view = AppView::Editor;
                    } else {
                        self.app_view = AppView::Library;
                    }
                    self.library_tab = LibraryTab::Sounds;
                    self.library_current_folder = None;
                }

                let timeline_response = ui.add_sized(
                    [92.0, 30.0],
                    Self::titlebar_button(
                        RichText::new("Timeline").size(11.5),
                        self.timeline_mode_active_sound_id().is_some(),
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &timeline_response);
                if timeline_response.clicked() {
                    self.toggle_timeline_mode(ctx);
                }

                let spn_response =
                    Self::icon_titlebar(ui, [42.0, 30.0], 0xe405, self.show_pitch_panel, false)
                        .on_hover_text(self.t("title.pitch_monitor"));
                if spn_response.clicked() {
                    self.refresh_pitch_capture_devices();
                    self.show_pitch_panel = !self.show_pitch_panel;
                }

                let stream_response =
                    Self::icon_titlebar(ui, [42.0, 30.0], 0xe029, self.show_stream_panel, false)
                        .on_hover_text(self.t("title.stream_input"));
                if stream_response.clicked() {
                    self.refresh_stream_input_capture_devices();
                    self.show_stream_panel = !self.show_stream_panel;
                }

                if matches!(
                    self.update_status,
                    crate::services::updater_service::UpdateStatus::Available { .. }
                        | crate::services::updater_service::UpdateStatus::Downloading { .. }
                        | crate::services::updater_service::UpdateStatus::ReadyToRestart { .. }
                ) {
                    let update_tooltip = match &self.update_status {
                        crate::services::updater_service::UpdateStatus::Available { version, .. } => {
                            format!("Cập nhật mới: v{version}")
                        }
                        crate::services::updater_service::UpdateStatus::Downloading { progress, .. } => {
                            format!("Đang tải: {:.0}%", progress * 100.0)
                        }
                        crate::services::updater_service::UpdateStatus::ReadyToRestart { .. } => {
                            "Đã tải xong, khởi động lại để cập nhật".to_string()
                        }
                        _ => String::new(),
                    };
                    let update_btn = Self::icon_titlebar(ui, [42.0, 30.0], 0xe8d7, false, true)
                        .on_hover_text(update_tooltip);
                    if update_btn.clicked() {
                        self.show_settings_panel = true;
                    }
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe8b8, self.show_settings_panel, false)
                    .clicked()
                {
                    self.show_settings_panel = true;
                    if self.settings_startup_candidate.is_none() {
                        self.settings_startup_candidate = self.selected;
                    }
                }

                let language_response = ui.add_sized(
                    [42.0, 30.0],
                    Self::titlebar_button(
                        RichText::new(self.localization.current_code().to_uppercase()).size(11.5),
                        false,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &language_response);
                if language_response.clicked() {
                    let language_code = if self.localization.current_code() == "en" {
                        "vi"
                    } else {
                        "en"
                    };
                    self.localization.set_current_code(language_code);
                    let _ = self.storage.save_language_code(language_code);
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe061, false, false).clicked() {
                    let reopen_recording_review = self
                        .recording_draft
                        .as_ref()
                        .is_some_and(|draft| draft.mode == RecordingDraftMode::Recording)
                        && !self.recorder.snapshot().running;
                    if reopen_recording_review {
                        self.show_record_panel = false;
                        self.show_record_review_panel = true;
                    } else {
                        self.refresh_record_capture_devices();
                        self.show_record_review_panel = false;
                        self.show_record_panel = true;
                    }
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe8b6, self.show_myinstants_panel, false)
                    .clicked()
                {
                    self.show_myinstants_panel = true;
                }

                let playlist_response = Self::icon_titlebar(
                    ui,
                    [42.0, 30.0],
                    0xe05f,
                    self.show_playlist_panel,
                    false,
                );
                if self.playlist_playing_id.is_some() {
                    let pulse = ((ctx.input(|input| input.time) as f32 * 4.2).sin() * 0.5 + 0.5)
                        .clamp(0.0, 1.0);
                    ui.painter().rect_stroke(
                        playlist_response.rect.expand(1.0),
                        14.0,
                        Stroke::new(
                            1.5,
                            Color32::from_rgba_premultiplied(
                                214,
                                51,
                                132,
                                (72.0 + pulse * 90.0) as u8,
                            ),
                        ),
                        StrokeKind::Outside,
                    );
                }
                if playlist_response.on_hover_text(self.t("playlist.title")).clicked() {
                    self.show_playlist_panel = !self.show_playlist_panel;
                }

                if let Some(clip_url) = self.clipboard_download_url.clone()
                    && !downloader_snapshot.running
                    && self.last_downloaded_clipboard_url.as_deref() != Some(&clip_url)
                {
                    let label = self.t("download.download_clipboard");
                    let hint = format!(
                        "{}: {}",
                        self.t("download.clipboard_hint"),
                        Self::truncate_middle_ascii(&clip_url, 45)
                    );
                    let quick_btn = ui.add_sized(
                        [108.0, 30.0],
                        Self::action_button(
                            RichText::new(format!("⬇ {label}")).size(11.5),
                            true,
                            false,
                        ),
                    ).on_hover_text(hint);
                    Self::decorate_button_response(ui, &quick_btn);
                    if quick_btn.clicked() {
                        let target_url = clip_url.clone();
                        self.download_url = target_url.clone();
                        self.last_downloaded_clipboard_url = Some(target_url.clone());
                        self.quick_download_active = true;
                        match self.downloader.start_audio_download(target_url) {
                            Ok(()) => {
                                self.status = Some("Downloading sound...".to_owned());
                            }
                            Err(error) => {
                                self.quick_download_active = false;
                                self.set_error_status(error);
                            }
                        }
                    }
                }

                let download_response = Self::icon_titlebar(ui, [42.0, 30.0], 0xe2c4, self.show_download_panel, false);
                if download_titlebar_active {
                    let pulse = ((ctx.input(|input| input.time) as f32 * 4.2).sin() * 0.5 + 0.5)
                        .clamp(0.0, 1.0);
                    ui.painter().rect_stroke(
                        download_response.rect.expand(1.0),
                        14.0,
                        Stroke::new(
                            1.5,
                            Color32::from_rgba_premultiplied(
                                227,
                                82,
                                149,
                                (72.0 + pulse * 90.0) as u8,
                            ),
                        ),
                        StrokeKind::Outside,
                    );
                    for idx in 0..3 {
                        ui.painter().circle_filled(
                            Pos2::new(
                                download_response.rect.center().x - 8.0 + idx as f32 * 8.0,
                                download_response.rect.bottom() - 5.0,
                            ),
                            1.3 + idx as f32 * 0.12,
                            Color32::from_rgba_premultiplied(
                                255,
                                255,
                                255,
                                (84.0 + pulse * 140.0) as u8,
                            ),
                        );
                    }
                }
                if download_response.clicked() {
                    self.show_download_panel = !self.show_download_panel;
                    if self.show_download_panel && self.download_url.trim().is_empty() {
                        if let Some(url) = &self.clipboard_download_url {
                            self.download_url = url.clone();
                        }
                    }
                }
            });
        });
    }

    pub(super) fn render_custom_window_resize_handles(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
            return;
        }

        let rect = ctx.screen_rect();
        let edge = 12.0;
        let corner = 28.0;
        let handles = [
            (
                "resize-n",
                Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + edge)),
                egui::viewport::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                "resize-s",
                Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - edge), rect.max),
                egui::viewport::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                "resize-w",
                Rect::from_min_max(rect.min, egui::pos2(rect.min.x + edge, rect.max.y)),
                egui::viewport::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                "resize-e",
                Rect::from_min_max(egui::pos2(rect.max.x - edge, rect.min.y), rect.max),
                egui::viewport::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                "resize-nw",
                Rect::from_min_size(rect.min, vec2(corner, corner)),
                egui::viewport::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                "resize-ne",
                Rect::from_min_max(
                    egui::pos2(rect.max.x - corner, rect.min.y),
                    egui::pos2(rect.max.x, rect.min.y + corner),
                ),
                egui::viewport::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "resize-sw",
                Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.max.y - corner),
                    egui::pos2(rect.min.x + corner, rect.max.y),
                ),
                egui::viewport::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "resize-se",
                Rect::from_min_max(
                    egui::pos2(rect.max.x - corner, rect.max.y - corner),
                    rect.max,
                ),
                egui::viewport::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (id, handle_rect, direction, cursor) in handles {
            egui::Area::new(egui::Id::new(id))
                .order(egui::Order::Foreground)
                .fixed_pos(handle_rect.min)
                .interactable(true)
                .show(ctx, |ui| {
                    let (_, response) =
                        ui.allocate_exact_size(handle_rect.size(), Sense::click_and_drag());
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(cursor);
                    }
                    if response.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                    }
                });
        }
    }

    pub(super) fn paint_titlebar_blob(ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 24.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let center = rect.center();
        for (radius_x, radius_y, tint, phase) in [
            (
                15.5,
                8.8,
                Color32::from_rgba_premultiplied(255, 173, 216, 160),
                0.0,
            ),
            (
                12.0,
                7.0,
                Color32::from_rgba_premultiplied(246, 124, 181, 148),
                0.9,
            ),
            (
                8.6,
                5.8,
                Color32::from_rgba_premultiplied(227, 82, 149, 164),
                1.8,
            ),
        ] {
            let mut points = Vec::with_capacity(36);
            for step in 0..36 {
                let angle = step as f32 / 36.0 * std::f32::consts::TAU;
                let wobble = 1.0 + 0.18 * (angle * 3.0 + phase).sin() + 0.08 * (angle * 5.0).cos();
                points.push(egui::pos2(
                    center.x + angle.cos() * radius_x * wobble,
                    center.y + angle.sin() * radius_y * wobble,
                ));
            }
            painter.add(egui::Shape::convex_polygon(points, tint, Stroke::NONE));
        }
    }

    pub(super) fn paint_titlebar_wave(ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(130.0, 18.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let bars: [f32; 12] = [
            0.22, 0.48, 0.84, 0.52, 0.28, 0.74, 0.94, 0.62, 0.34, 0.56, 0.3, 0.78,
        ];
        let bar_width = rect.width() / bars.len() as f32;
        for (index, bar) in bars.into_iter().enumerate() {
            let x = rect.left() + (index as f32 + 0.5) * bar_width;
            let half = bar * rect.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                egui::pos2(x - bar_width * 0.22, rect.center().y - half),
                egui::pos2(x + bar_width * 0.22, rect.center().y + half),
            );
            painter.rect_filled(wave_rect, 2.0, Color32::from_rgb(227, 82, 149));
        }
    }
}
