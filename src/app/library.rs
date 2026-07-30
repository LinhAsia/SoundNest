use super::*;

impl SoundFxApp {
    pub(super) fn draw_folders_list_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_current_folder_exists();
        let external_drop_active = self.external_library_drop_active(ui.ctx());
        let drop_session_active =
            external_drop_active || ui.ctx().input(|input| !input.raw.dropped_files.is_empty());
        if !drop_session_active {
            self.library_drop_target_root_rect = None;
            self.library_drop_target_folder_rects.clear();
            self.library_drop_target_folder = None;
            self.library_drop_target_root = false;
        }
        if external_drop_active {
            self.library_drop_target_root_rect = None;
            self.library_drop_target_folder_rects.clear();
            self.library_drop_target_folder = None;
            self.library_drop_target_root = true;
        }
        self.library_drop_target_root_rect = Some(ui.max_rect());
        let selected_parent_label = self
            .library_current_folder
            .map(|folder_id| self.folder_path_label(folder_id))
            .unwrap_or_else(|| self.t("common.root"));

        ui.horizontal(|ui| {
            if self.library_folder_view == LibraryFolderView::Grid
                && self.library_current_folder.is_some()
            {
                let back_btn = Self::icon_action(ui, [34.0, 28.0], 0xe5c4, false, false);
                if back_btn.clicked() {
                    if let Some(folder_id) = self.library_current_folder {
                        if let Some(folder) = self.folders.iter().find(|f| f.id == folder_id) {
                            self.library_current_folder = folder.parent_id;
                        } else {
                            self.library_current_folder = None;
                        }
                    }
                }
                ui.add_space(4.0);
            }

            ui.label(Self::icon(0xe2c7, 16.0, Color32::from_rgb(242, 140, 56)));
            ui.label(
                RichText::new(self.t("library.folders"))
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(selected_parent_label.as_str())
                    .size(11.5)
                    .color(Self::muted_text_color()),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let grid_btn = ui.add_sized(
                    [58.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.grid")).size(11.5),
                        self.library_folder_view == LibraryFolderView::Grid,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &grid_btn);
                if grid_btn.clicked() {
                    self.library_folder_view = LibraryFolderView::Grid;
                    let _ = self
                        .storage
                        .save_library_folder_view(self.library_folder_view.preference_value());
                }
                ui.add_space(6.0);
                let rows_btn = ui.add_sized(
                    [58.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.rows")).size(11.5),
                        self.library_folder_view == LibraryFolderView::Rows,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &rows_btn);
                if rows_btn.clicked() {
                    self.library_folder_view = LibraryFolderView::Rows;
                    let _ = self
                        .storage
                        .save_library_folder_view(self.library_folder_view.preference_value());
                }
                ui.add_space(10.0);
                let add_folder_btn = ui.add_sized(
                    [160.0, 32.0],
                    Button::new(
                        RichText::new(if self.library_folder_create_open {
                            self.t("library.hide_add_folder")
                        } else {
                            self.t("library.add_folder")
                        })
                        .size(12.5)
                        .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(227, 82, 149))
                    .corner_radius(12.0),
                );
                Self::decorate_button_response(ui, &add_folder_btn);
                if add_folder_btn.clicked() {
                    self.library_folder_create_open = !self.library_folder_create_open;
                    self.folder_name_warning = false;
                }
            });
        });

        if let Some(import_job) = self.library_import_job.as_ref() {
            ui.add_space(10.0);
            let target_label = import_job
                .target_folder_id
                .and_then(|folder_id| {
                    self.folders
                        .iter()
                        .find(|folder| folder.id == folder_id)
                        .map(|folder| self.folder_path_label(folder.id))
                })
                .unwrap_or_else(|| self.t("common.root"));
            Frame::new()
                .fill(Color32::from_rgba_premultiplied(242, 140, 56, 20))
                .stroke(Stroke::new(1.0, Color32::from_rgb(242, 140, 56)))
                .corner_radius(16.0)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Self::icon(0xe2c7, 16.0, Color32::from_rgb(242, 140, 56)));
                        ui.label(
                            RichText::new(
                                self.t("library.importing_into")
                                    .replace("{target}", &target_label),
                            )
                            .size(12.5)
                            .color(Self::strong_text_color())
                            .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let total = import_job.total.max(import_job.completed.max(1));
                            ui.label(
                                RichText::new(format!(
                                    "{}/{}",
                                    import_job.completed.min(total),
                                    total
                                ))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    let denominator = import_job.total.max(1) as f32;
                    let progress = (import_job.completed as f32 / denominator).clamp(0.0, 1.0);
                    ui.add(
                        ProgressBar::new(progress)
                            .desired_width(ui.available_width())
                            .text(import_job.current_label.as_str()),
                    );
                });
        }

        if self.library_folder_create_open {
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.t("library.create_folder_in"))
                    .size(11.5)
                    .color(Self::muted_text_color()),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                Frame::new()
                    .fill(Self::input_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(12.0)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.add_sized(
                            [220.0, 20.0],
                            egui::Label::new(
                                RichText::new(selected_parent_label)
                                    .size(12.5)
                                    .color(Self::strong_text_color()),
                            )
                            .truncate(),
                        );
                    });
                if self.library_current_folder.is_some() {
                    let clear_btn = ui.add_sized(
                        [92.0, 30.0],
                        Self::action_button(
                            RichText::new(self.t("library.to_root")).size(12.0),
                            false,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &clear_btn);
                    if clear_btn.clicked() {
                        self.library_current_folder = None;
                    }
                }
            });
            ui.add_space(8.0);

            let border_stroke = if self.folder_name_warning {
                Stroke::new(1.5, Color32::from_rgb(220, 53, 69))
            } else {
                Stroke::new(1.0, Self::border_color())
            };
            let folder_hint = self.t("library.folder_placeholder");
            let create_edit = Frame::new()
                .fill(Self::input_fill())
                .stroke(border_stroke)
                .corner_radius(12.0)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.add_sized(
                        [220.0, 20.0],
                        egui::TextEdit::singleline(&mut self.new_folder_name)
                            .frame(false)
                            .hint_text(folder_hint),
                    )
                });
            if create_edit.inner.changed() {
                self.folder_name_warning = false;
            }
            ui.add_space(8.0);
            let create_btn = ui.add_sized(
                [160.0, 32.0],
                Button::new(
                    RichText::new(format!("+ {}", self.t("library.create_folder")))
                        .size(12.5)
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(227, 82, 149))
                .corner_radius(12.0),
            );
            Self::decorate_button_response(ui, &create_btn);
            let create_clicked = create_btn.clicked()
                || (create_edit.inner.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter)));
            if create_clicked {
                let name = self.new_folder_name.trim().to_owned();
                if name.is_empty() {
                    self.folder_name_warning = true;
                } else {
                    self.create_folder(name, self.library_current_folder);
                    self.library_folder_create_open = false;
                }
            }

            ui.add_space(14.0);
        } else {
            ui.add_space(12.0);
        }

        if self.folders.is_empty() {
            ui.label(
                RichText::new(self.t("library.no_folders"))
                    .size(13.0)
                    .color(Self::muted_text_color()),
            );
            return;
        }

        if self.library_folder_view == LibraryFolderView::Grid && !self.library_search_active() {
            self.draw_folders_grid_view(ui);
        } else {
            self.draw_folders_tree_view(ui);
        }
    }

    pub(super) fn draw_folder_tree_node(
        &mut self,
        ui: &mut egui::Ui,
        folder: &crate::storage::Folder,
        depth: usize,
        select_folder_id: &mut Option<Uuid>,
        delete_folder_id: &mut Option<Uuid>,
        rename_folder_id: &mut Option<Uuid>,
        rename_commit: &mut Option<(Uuid, String)>,
        finish_editing: &mut bool,
        toggle_folder_id: &mut Option<Uuid>,
        clear_selected_folder: &mut bool,
        clipboard_paste_ready: bool,
    ) {
        let external_drop_active = self.external_library_drop_active(ui.ctx());
        let is_selected = self.library_current_folder == Some(folder.id);
        let is_editing = self.editing_folder_id == Some(folder.id);
        let total_count = self.total_sound_count_for_folder(folder.id);
        let direct_count = self.direct_sound_count_for_folder(folder.id);
        let children = self.visible_child_folders(Some(folder.id));
        let subfolders_count = children.len();
        let has_children = !children.is_empty();
        let is_collapsed = has_children && self.library_collapsed_folders.contains(&folder.id);

        let (folder_accent, folder_accent_soft, folder_icon_code) =
            if subfolders_count > 0 && direct_count > 0 {
                (
                    Color32::from_rgb(33, 150, 243),
                    Color32::from_rgb(179, 219, 255),
                    if is_collapsed { 0xe2c7 } else { 0xe2c8 },
                )
            } else if subfolders_count > 0 {
                (
                    Color32::from_rgb(242, 140, 56),
                    Color32::from_rgb(255, 202, 145),
                    if is_collapsed { 0xe2c7 } else { 0xe2c8 },
                )
            } else if direct_count > 0 {
                (
                    Color32::from_rgb(56, 182, 163),
                    Color32::from_rgb(170, 241, 229),
                    0xe2c8,
                )
            } else {
                (
                    Self::muted_text_color(),
                    Self::muted_text_color().linear_multiply(0.5),
                    if is_collapsed { 0xe2c7 } else { 0xe2c8 },
                )
            };

        let indent = 18.0 * depth as f32;
        let show_folder_actions = is_selected || (has_children && !is_collapsed);
        let mut pointer_over_drop_target = false;
        let fill = if Self::dark_theme_enabled() {
            if is_selected || (has_children && !is_collapsed) {
                if subfolders_count > 0 && direct_count > 0 {
                    Color32::from_rgb(12, 49, 78)
                } else if subfolders_count > 0 {
                    Color32::from_rgb(63, 39, 24)
                } else if direct_count > 0 {
                    Color32::from_rgb(15, 56, 51)
                } else {
                    Color32::from_rgb(30, 30, 30)
                }
            } else {
                if subfolders_count > 0 && direct_count > 0 {
                    Color32::from_rgb(8, 32, 52)
                } else if subfolders_count > 0 {
                    Color32::from_rgb(33, 24, 18)
                } else if direct_count > 0 {
                    Color32::from_rgb(11, 37, 34)
                } else {
                    Color32::from_rgb(22, 22, 22)
                }
            }
        } else if is_selected || (has_children && !is_collapsed) {
            if subfolders_count > 0 && direct_count > 0 {
                Color32::from_rgb(205, 230, 255)
            } else if subfolders_count > 0 {
                Color32::from_rgb(255, 245, 234)
            } else if direct_count > 0 {
                Color32::from_rgb(218, 252, 246)
            } else {
                Color32::from_rgb(245, 245, 245)
            }
        } else {
            if subfolders_count > 0 && direct_count > 0 {
                Color32::from_rgb(225, 240, 255)
            } else if subfolders_count > 0 {
                Color32::from_rgb(255, 250, 245)
            } else if direct_count > 0 {
                Color32::from_rgb(233, 255, 251)
            } else {
                Color32::from_rgb(250, 250, 250)
            }
        };
        let stroke = if is_selected || (has_children && !is_collapsed) {
            folder_accent
        } else {
            folder_accent.linear_multiply(0.42)
        };

        let row = ui
            .horizontal(|ui| {
                ui.add_space(indent);
                Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, stroke))
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        let mut delete_btn_response = None;
                        let mut rename_btn_response = None;
                        let mut import_btn_response = None;
                        let mut paste_btn_response = None;
                        ui.horizontal(|ui| {
                            let icon_gap = 16.0;
                            if has_children {
                                ui.label(Self::icon(
                                    if is_collapsed { 0xe5cc } else { 0xe5cf },
                                    16.0,
                                    folder_accent,
                                ));
                            } else {
                                ui.add_space(icon_gap);
                            }
                            ui.add_space(6.0);
                            ui.label(Self::icon(folder_icon_code, 16.0, folder_accent));
                            ui.add_space(6.0);
                            if is_editing {
                                let response = ui.add_sized(
                                    [220.0, 21.0],
                                    egui::TextEdit::singleline(&mut self.folder_rename_name),
                                );
                                if response.lost_focus()
                                    || (response.has_focus()
                                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                                {
                                    let new_name = self.folder_rename_name.trim().to_owned();
                                    if !new_name.is_empty() {
                                        *rename_commit = Some((folder.id, new_name));
                                    }
                                    *finish_editing = true;
                                }
                            } else {
                                ui.add_sized(
                                    [220.0, 21.0],
                                    egui::Label::new(
                                        RichText::new(&folder.name)
                                            .size(12.5)
                                            .color(Self::strong_text_color())
                                            .strong(),
                                    )
                                    .truncate(),
                                );
                            }
                            let desc = if subfolders_count > 0 && direct_count > 0 {
                                self.t("library.folder_info_mixed")
                                    .replace("{folders}", &subfolders_count.to_string())
                                    .replace("{direct}", &direct_count.to_string())
                                    .replace("{total}", &total_count.to_string())
                            } else if subfolders_count > 0 {
                                self.t("library.folder_info_folders_only")
                                    .replace("{folders}", &subfolders_count.to_string())
                            } else if direct_count > 0 {
                                self.t("library.folder_info_sounds")
                                    .replace("{count}", &total_count.to_string())
                            } else {
                                self.t("library.empty_folder")
                            };
                            ui.add_sized(
                                [150.0, 21.0],
                                egui::Label::new(
                                    RichText::new(desc)
                                        .size(11.0)
                                        .color(Self::muted_text_color()),
                                )
                                .truncate(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                let delete_btn =
                                    Self::icon_action(ui, [30.0, 26.0], 0xe872, false, false);
                                Self::decorate_button_response(ui, &delete_btn);
                                if delete_btn.clicked() {
                                    *delete_folder_id = Some(folder.id);
                                }
                                delete_btn_response = Some(delete_btn);
                                if !is_editing {
                                    let rename_btn =
                                        Self::icon_action(ui, [30.0, 26.0], 0xe254, false, false);
                                    Self::decorate_button_response(ui, &rename_btn);
                                    if rename_btn.clicked() {
                                        *rename_folder_id = Some(folder.id);
                                    }
                                    rename_btn_response = Some(rename_btn);
                                }
                                if show_folder_actions
                                    && self.library_tab == LibraryTab::Sounds
                                    && self.folder_import_select_mode.is_none()
                                {
                                    ui.add_space(6.0);
                                    let import_btn = ui.add_sized(
                                        [92.0, 26.0],
                                        Button::new(
                                            RichText::new(format!(
                                                "+ {}",
                                                self.t("library.import_sound_to_folder")
                                            ))
                                            .size(10.8),
                                        )
                                        .fill(Color32::from_rgb(227, 82, 149))
                                        .corner_radius(6.0),
                                    );
                                    Self::decorate_button_response(ui, &import_btn);
                                    if import_btn.clicked() {
                                        self.folder_import_select_mode = Some(folder.id);
                                    }
                                    import_btn_response = Some(import_btn);
                                    ui.add_space(6.0);
                                    let paste_btn = ui
                                        .add_enabled_ui(clipboard_paste_ready, |ui| {
                                            ui.add_sized(
                                                [52.0, 26.0],
                                                Button::new(
                                                    RichText::new(self.t("common.paste"))
                                                        .size(10.8),
                                                )
                                                .fill(folder_accent)
                                                .stroke(Stroke::new(1.0, folder_accent_soft))
                                                .corner_radius(6.0),
                                            )
                                        })
                                        .inner;
                                    Self::decorate_button_response(ui, &paste_btn);
                                    if paste_btn.clicked() {
                                        match self
                                            .paste_clipboard_sounds_to_folder(folder.id, ui.ctx())
                                        {
                                            Ok(imported) => {
                                                self.status = Some(
                                                    self.t("library.pasted_into_folder").replace(
                                                        "{imported}",
                                                        &imported.to_string(),
                                                    ),
                                                );
                                            }
                                            Err(error) => self.set_error_status(error),
                                        }
                                    }
                                    paste_btn_response = Some(paste_btn);
                                }
                            });
                        });

                        (
                            delete_btn_response,
                            rename_btn_response,
                            import_btn_response,
                            paste_btn_response,
                        )
                    })
            })
            .inner;

        if !is_editing {
            let open_rect = Rect::from_min_max(
                row.response.rect.min,
                Pos2::new(
                    (row.response.rect.max.x - if show_folder_actions { 278.0 } else { 84.0 })
                        .max(row.response.rect.min.x),
                    row.response.rect.max.y,
                ),
            );
            let response = ui.interact(open_rect, ui.id().with(folder.id), Sense::click_and_drag());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            pointer_over_drop_target = external_drop_active
                && self
                    .external_drop_pointer_pos(ui.ctx())
                    .is_some_and(|pos| row.response.rect.contains(pos));
            self.library_drop_target_folder_rects
                .push((folder.id, row.response.rect));
            if pointer_over_drop_target {
                self.library_drop_target_folder = Some(folder.id);
                self.library_drop_target_root = false;
                ui.painter().rect_stroke(
                    row.response.rect.expand(2.0),
                    18.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 186, 86)),
                    StrokeKind::Outside,
                );
                ui.painter().rect_filled(
                    row.response.rect,
                    16.0,
                    Color32::from_rgba_premultiplied(242, 140, 56, 20),
                );
            }
            let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
            if titlebar_drag_active {
                self.pending_folder_drag = None;
            } else if response.drag_started() {
                self.pending_folder_drag = Some(folder.id);
            } else if response.drag_stopped() {
                self.pending_folder_drag = None;
            } else if response.dragged()
                && self.pending_folder_drag == Some(folder.id)
                && Self::pointer_primary_drag_ready(ui.ctx())
            {
                if let Err(error) = self.drag_folder_out(folder.id) {
                    self.set_error_status(error);
                }
                self.pending_folder_drag = None;
            }
            let delete_hovered = row.inner.0.as_ref().is_some_and(|value| value.hovered());
            let rename_hovered = row.inner.1.as_ref().is_some_and(|value| value.hovered());
            let import_hovered = row.inner.2.as_ref().is_some_and(|value| value.hovered());
            let paste_hovered = row.inner.3.as_ref().is_some_and(|value| value.hovered());
            if response.clicked()
                && !delete_hovered
                && !rename_hovered
                && !import_hovered
                && !paste_hovered
            {
                if is_selected {
                    *clear_selected_folder = true;
                } else {
                    *select_folder_id = Some(folder.id);
                }
                if has_children {
                    *toggle_folder_id = Some(folder.id);
                }
            }
        }

        if pointer_over_drop_target {
            ui.add_space(8.0);
            self.draw_external_drop_preview_row(ui, depth + 1, Some(folder.id));
        }

        let search_active = self.library_search_active();
        let has_sounds = self.library_tab == LibraryTab::Sounds
            && !self
                .filtered_library_sounds_for_folder(Some(folder.id), false)
                .is_empty()
            && (search_active || (is_selected && !is_collapsed));

        if has_sounds {
            ui.add_space(8.0);
            if self.library_sound_view == LibrarySoundView::Grid {
                self.draw_inline_folder_sound_grid(ui, folder, 8.0);
            } else {
                self.draw_inline_folder_sounds(ui, folder, 8.0);
            }
        }

        if search_active || !is_collapsed {
            if has_sounds {
                ui.add_space(8.0);
            }
            for child in children {
                self.draw_folder_tree_node(
                    ui,
                    &child,
                    depth + 1,
                    select_folder_id,
                    delete_folder_id,
                    rename_folder_id,
                    rename_commit,
                    finish_editing,
                    toggle_folder_id,
                    clear_selected_folder,
                    clipboard_paste_ready,
                );
            }
        }
    }

    pub(super) fn draw_folder_grid_card(
        &mut self,
        ui: &mut egui::Ui,
        folder: &crate::storage::Folder,
        _depth: usize,
        card_width: f32,
        select_folder_id: &mut Option<Uuid>,
        delete_folder_id: &mut Option<Uuid>,
        rename_folder_id: &mut Option<Uuid>,
        rename_commit: &mut Option<(Uuid, String)>,
        finish_editing: &mut bool,
        _toggle_folder_id: &mut Option<Uuid>,
        _clear_selected_folder: &mut bool,
        clipboard_paste_ready: bool,
    ) {
        let external_drop_active = self.external_library_drop_active(ui.ctx());
        let folder_accent = Color32::from_rgb(242, 140, 56);
        let is_selected = self.library_current_folder == Some(folder.id);
        let is_editing = self.editing_folder_id == Some(folder.id);
        let total_count = self.total_sound_count_for_folder(folder.id);
        let direct_count = self.direct_sound_count_for_folder(folder.id);
        let subfolders_count = self
            .folders
            .iter()
            .filter(|value| value.parent_id == Some(folder.id))
            .count();
        let has_children = subfolders_count > 0;
        let is_collapsed = has_children && self.library_collapsed_folders.contains(&folder.id);
        let show_folder_actions = is_selected || (has_children && !is_collapsed);

        let (card_accent, folder_icon_code) = if subfolders_count > 0 && direct_count > 0 {
            (
                Color32::from_rgb(33, 150, 243),
                if is_collapsed { 0xe2c7 } else { 0xe2c8 },
            )
        } else if subfolders_count > 0 {
            (folder_accent, if is_collapsed { 0xe2c7 } else { 0xe2c8 })
        } else if direct_count > 0 {
            (Color32::from_rgb(56, 182, 163), 0xe2c8)
        } else {
            (
                Self::muted_text_color(),
                if is_collapsed { 0xe2c7 } else { 0xe2c8 },
            )
        };

        let fill = if Self::dark_theme_enabled() {
            if is_selected || (has_children && !is_collapsed) {
                if subfolders_count > 0 && direct_count > 0 {
                    Color32::from_rgb(12, 49, 78)
                } else if subfolders_count > 0 {
                    Color32::from_rgb(63, 39, 24)
                } else if direct_count > 0 {
                    Color32::from_rgb(15, 56, 51)
                } else {
                    Color32::from_rgb(30, 30, 30)
                }
            } else {
                if subfolders_count > 0 && direct_count > 0 {
                    Color32::from_rgb(8, 32, 52)
                } else if subfolders_count > 0 {
                    Color32::from_rgb(33, 24, 18)
                } else if direct_count > 0 {
                    Color32::from_rgb(11, 37, 34)
                } else {
                    Color32::from_rgb(22, 22, 22)
                }
            }
        } else if is_selected || (has_children && !is_collapsed) {
            if subfolders_count > 0 && direct_count > 0 {
                Color32::from_rgb(205, 230, 255)
            } else if subfolders_count > 0 {
                Color32::from_rgb(255, 245, 234)
            } else if direct_count > 0 {
                Color32::from_rgb(218, 252, 246)
            } else {
                Color32::from_rgb(245, 245, 245)
            }
        } else {
            if subfolders_count > 0 && direct_count > 0 {
                Color32::from_rgb(225, 240, 255)
            } else if subfolders_count > 0 {
                Color32::from_rgb(255, 250, 245)
            } else if direct_count > 0 {
                Color32::from_rgb(233, 255, 251)
            } else {
                Color32::from_rgb(250, 250, 250)
            }
        };
        let stroke = if is_selected || (has_children && !is_collapsed) {
            card_accent
        } else {
            card_accent.linear_multiply(0.42)
        };
        let mut delete_btn_response = None;
        let mut rename_btn_response = None;
        let mut import_btn_response: Option<egui::Response> = None;
        let mut paste_btn_response: Option<egui::Response> = None;

        let row = Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(8.0)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(card_width - 16.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if has_children {
                            ui.label(Self::icon(
                                if is_collapsed { 0xe5cc } else { 0xe5cf },
                                16.0,
                                card_accent,
                            ));
                        } else {
                            ui.add_space(16.0);
                        }
                        ui.label(Self::icon(folder_icon_code, 16.0, card_accent));
                    });
                    ui.add_space(4.0);
                    if is_editing {
                        let response = ui.add_sized(
                            [card_width - 16.0, 20.0],
                            egui::TextEdit::singleline(&mut self.folder_rename_name),
                        );
                        if response.lost_focus()
                            || (response.has_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                        {
                            let new_name = self.folder_rename_name.trim().to_owned();
                            if !new_name.is_empty() {
                                *rename_commit = Some((folder.id, new_name));
                            }
                            *finish_editing = true;
                        }
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&folder.name)
                                    .size(13.2)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .truncate(),
                        );
                    }
                    let desc = if subfolders_count > 0 && direct_count > 0 {
                        self.t("library.folder_info_mixed")
                            .replace("{folders}", &subfolders_count.to_string())
                            .replace("{direct}", &direct_count.to_string())
                            .replace("{total}", &total_count.to_string())
                    } else if subfolders_count > 0 {
                        self.t("library.folder_info_folders_only")
                            .replace("{folders}", &subfolders_count.to_string())
                    } else if direct_count > 0 {
                        self.t("library.folder_info_sounds")
                            .replace("{count}", &total_count.to_string())
                    } else {
                        self.t("library.empty_folder")
                    };
                    ui.label(
                        RichText::new(desc)
                            .size(11.0)
                            .color(Self::muted_text_color()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        let rename_btn = Self::icon_action(ui, [30.0, 26.0], 0xe254, false, false);
                        Self::decorate_button_response(ui, &rename_btn);
                        if rename_btn.clicked() {
                            *rename_folder_id = Some(folder.id);
                        }
                        rename_btn_response = Some(rename_btn);

                        let delete_btn = Self::icon_action(ui, [30.0, 26.0], 0xe872, false, false);
                        Self::decorate_button_response(ui, &delete_btn);
                        if delete_btn.clicked() {
                            *delete_folder_id = Some(folder.id);
                        }
                        delete_btn_response = Some(delete_btn);
                    });
                });

                (
                    delete_btn_response.clone(),
                    rename_btn_response.clone(),
                    import_btn_response.clone(),
                    paste_btn_response.clone(),
                )
            });

        if !is_editing {
            let response = ui.interact(
                row.response.rect,
                ui.id().with(("grid", folder.id)),
                Sense::click_and_drag(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if show_folder_actions
                && self.library_tab == LibraryTab::Sounds
                && self.folder_import_select_mode.is_none()
                && response.hovered()
            {
                let overlay_id = ui.id().with(("grid-folder-actions-overlay", folder.id));
                let overlay_pos = egui::pos2(
                    row.response.rect.right() - 174.0,
                    row.response.rect.top() + 10.0,
                );
                egui::Area::new(overlay_id)
                    .order(egui::Order::Foreground)
                    .fixed_pos(overlay_pos)
                    .show(ui.ctx(), |ui| {
                        Frame::new()
                            .fill(Color32::from_rgba_premultiplied(24, 18, 29, 232))
                            .stroke(Stroke::new(
                                1.0,
                                Color32::from_rgba_premultiplied(255, 255, 255, 18),
                            ))
                            .corner_radius(12.0)
                            .inner_margin(Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                                    let paste_btn = ui
                                        .add_enabled_ui(clipboard_paste_ready, |ui| {
                                            ui.add_sized(
                                                [58.0, 28.0],
                                                Button::new(
                                                    RichText::new(self.t("common.paste"))
                                                        .size(10.8),
                                                )
                                                .fill(folder_accent)
                                                .corner_radius(9.0),
                                            )
                                        })
                                        .inner;
                                    Self::decorate_button_response(ui, &paste_btn);
                                    if paste_btn.clicked() {
                                        match self
                                            .paste_clipboard_sounds_to_folder(folder.id, ui.ctx())
                                        {
                                            Ok(imported) => {
                                                self.status = Some(
                                                    self.t("library.pasted_into_folder").replace(
                                                        "{imported}",
                                                        &imported.to_string(),
                                                    ),
                                                );
                                            }
                                            Err(error) => self.set_error_status(error),
                                        }
                                    }
                                    paste_btn_response = Some(paste_btn);

                                    let import_btn = ui.add_sized(
                                        [102.0, 28.0],
                                        Button::new(
                                            RichText::new(format!(
                                                "+ {}",
                                                self.t("library.import_sound_to_folder")
                                            ))
                                            .size(10.8),
                                        )
                                        .fill(Color32::from_rgb(227, 82, 149))
                                        .corner_radius(9.0),
                                    );
                                    Self::decorate_button_response(ui, &import_btn);
                                    if import_btn.clicked() {
                                        self.folder_import_select_mode = Some(folder.id);
                                    }
                                    import_btn_response = Some(import_btn);
                                });
                            });
                    });
            }
            let pointer_over_drop_target = external_drop_active
                && self
                    .external_drop_pointer_pos(ui.ctx())
                    .is_some_and(|pos| row.response.rect.contains(pos));
            self.library_drop_target_folder_rects
                .push((folder.id, row.response.rect));
            if pointer_over_drop_target {
                self.library_drop_target_folder = Some(folder.id);
                self.library_drop_target_root = false;
                ui.painter().rect_stroke(
                    row.response.rect.expand(2.0),
                    18.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 186, 86)),
                    StrokeKind::Outside,
                );
                ui.painter().rect_filled(
                    row.response.rect,
                    16.0,
                    Color32::from_rgba_premultiplied(242, 140, 56, 22),
                );
            }
            let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
            if titlebar_drag_active {
                self.pending_folder_drag = None;
            } else if response.drag_started() {
                self.pending_folder_drag = Some(folder.id);
            } else if response.drag_stopped() {
                self.pending_folder_drag = None;
            } else if response.dragged()
                && self.pending_folder_drag == Some(folder.id)
                && Self::pointer_primary_drag_ready(ui.ctx())
            {
                if let Err(error) = self.drag_folder_out(folder.id) {
                    self.set_error_status(error);
                }
                self.pending_folder_drag = None;
            }
            let delete_hovered = row.inner.0.as_ref().is_some_and(|value| value.hovered());
            let rename_hovered = row.inner.1.as_ref().is_some_and(|value| value.hovered());
            let import_hovered = row.inner.2.as_ref().is_some_and(|value| value.hovered());
            let paste_hovered = row.inner.3.as_ref().is_some_and(|value| value.hovered());
            if response.clicked()
                && !delete_hovered
                && !rename_hovered
                && !import_hovered
                && !paste_hovered
            {
                *select_folder_id = Some(folder.id);
            }
        }
    }

    fn draw_inline_folder_sounds(
        &mut self,
        ui: &mut Ui,
        folder: &crate::storage::Folder,
        left_indent: f32,
    ) {
        let sounds = self.filtered_library_sounds_for_folder(Some(folder.id), false);
        if sounds.is_empty() {
            return;
        }
        let visible_count =
            self.visible_folder_sound_count(Some(folder.id), sounds.len(), ui.ctx());
        ui.add_space(6.0);
        let content_width = (ui.available_width() - left_indent).max(180.0);
        ui.horizontal(|ui| {
            ui.add_space(left_indent);
            ui.allocate_ui_with_layout(
                vec2(content_width, 0.0),
                egui::Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(content_width.max(ui.available_width()));
                    self.draw_folder_loading_hint(ui, visible_count, sounds.len());
                    self.draw_inline_folder_sound_rows_content(ui, &sounds[..visible_count]);
                },
            );
        });
    }

    fn draw_inline_folder_sound_grid(
        &mut self,
        ui: &mut Ui,
        folder: &crate::storage::Folder,
        left_indent: f32,
    ) {
        let sounds = self.filtered_library_sounds_for_folder(Some(folder.id), false);
        if sounds.is_empty() {
            return;
        }
        let visible_count =
            self.visible_folder_sound_count(Some(folder.id), sounds.len(), ui.ctx());
        ui.add_space(6.0);
        let content_width = (ui.available_width() - left_indent).max(180.0);
        ui.horizontal(|ui| {
            ui.add_space(left_indent);
            ui.allocate_ui_with_layout(
                vec2(content_width, 0.0),
                egui::Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(content_width.max(ui.available_width()));
                    self.draw_folder_loading_hint(ui, visible_count, sounds.len());
                    let modal_open = self.has_modal_panel();
                    let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
                    self.draw_library_sound_grid_content(
                        ui,
                        &sounds[..visible_count],
                        content_width,
                        modal_open,
                        false,
                        titlebar_drag_active,
                    );
                },
            );
        });
    }

    pub(super) fn draw_inline_folder_sound_row(&mut self, ui: &mut Ui, sound: &SoundEffect) {
        let mut preview_sound = None;
        let mut copy_sound = None;
        let mut favorite_sound = None;
        let mut remove_sound_from_folder = None;
        let mut open_sound = false;
        let mut favorite_response = None;
        let mut play_response = None;
        let mut copy_response = None;
        let mut remove_response = None;
        let mut preview_clicked = false;
        let mut copy_clicked = false;
        let mut favorite_clicked = false;
        let mut remove_clicked = false;
        let is_loading = self
            .pending_preview_after_preload
            .is_some_and(|(id, _)| id == sound.id);
        let playback_progress = self
            .audio
            .as_ref()
            .and_then(|audio| audio.playback_progress(sound.id));
        let is_previewing = playback_progress.is_some();
        if is_previewing || is_loading {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        let row_padding_y = self.library_row_vertical_padding();
        let waveform_height = self.library_row_wave_height();
        let ultra_compact_row = self.library_row_thickness == LIBRARY_ROW_MIN_THICKNESS;
        let play_button_size = match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 34.0,
            2 => 36.0,
            3 => 38.0,
            4 => 40.0,
            _ => 42.0,
        };
        let row_width = ui.available_width();
        let play_column_width = play_button_size + 10.0;
        let side_panel_width = (row_width * 0.30).clamp(260.0, 360.0);
        let waveform_width = (row_width - play_column_width - side_panel_width - 42.0).max(140.0);
        let row_height = self.inline_folder_sound_row_height();
        let row_outer_height = self.inline_folder_sound_row_outer_height();
        let (row_outer_rect, _) =
            ui.allocate_exact_size(vec2(row_width, row_outer_height), Sense::hover());
        let row_rect = Rect::from_min_size(row_outer_rect.min, vec2(row_width, row_height));
        let row_hovered =
            !self.has_modal_panel() && Self::pointer_within_rect(ui.ctx(), row_rect);
        let inner_rect = row_rect.shrink2(vec2(10.0, (row_padding_y as f32 * 0.75).max(6.0)));
        let viewport_clip_rect = ui.clip_rect();
        let row_painter = ui.painter().with_clip_rect(viewport_clip_rect);
        let row_fill = if row_hovered {
            if self.dark_theme {
                Color32::from_rgb(46, 28, 42)
            } else {
                Color32::from_rgb(255, 239, 247)
            }
        } else {
            Self::surface_fill()
        };
        let row_stroke = if row_hovered {
            Color32::from_rgb(227, 82, 149)
        } else {
            Self::border_color()
        };
        row_painter.rect_filled(row_rect, 14.0, row_fill);
        row_painter.rect_stroke(
            row_rect,
            14.0,
            Stroke::new(if row_hovered { 1.5 } else { 1.0 }, row_stroke),
            StrokeKind::Inside,
        );
        let actions_width = if self.folder_import_select_mode.is_none() {
            38.0 * 3.0 + 10.0 * 2.0
        } else {
            38.0 * 2.0 + 10.0
        };
        let info_width = (side_panel_width - actions_width - 16.0).max(120.0);
        let play_rect = Rect::from_min_size(
            Pos2::new(
                inner_rect.left(),
                inner_rect.center().y - (play_button_size * 0.5),
            ),
            vec2(play_button_size, play_button_size),
        );
        let waveform_rect = Rect::from_min_max(
            Pos2::new(inner_rect.left() + play_column_width, inner_rect.top()),
            Pos2::new(
                inner_rect.left() + play_column_width + waveform_width,
                inner_rect.bottom(),
            ),
        );
        let side_panel_rect = Rect::from_min_max(
            Pos2::new(inner_rect.right() - side_panel_width, inner_rect.top()),
            inner_rect.right_bottom(),
        );
        let info_rect = Rect::from_min_max(
            side_panel_rect.left_top(),
            Pos2::new(
                side_panel_rect.left() + info_width,
                side_panel_rect.bottom(),
            ),
        );
        let actions_rect = Rect::from_min_max(
            Pos2::new(
                side_panel_rect.right() - actions_width,
                side_panel_rect.top(),
            ),
            side_panel_rect.right_bottom(),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(play_rect), |ui| {
            ui.set_clip_rect(play_rect.intersect(viewport_clip_rect));
            ui.set_width(play_rect.width());
            ui.set_min_width(play_rect.width());
            ui.set_min_size(play_rect.size());
            ui.with_layout(egui::Layout::top_down(Align::Center), |ui| {
                let play_btn = Self::icon_action(
                    ui,
                    [play_button_size, play_button_size],
                    if is_loading || is_previewing {
                        0xe5d5
                    } else {
                        0xe037
                    },
                    is_loading || is_previewing,
                    false,
                );
                if play_btn.clicked() {
                    self.pending_sound_drag = None;
                    if is_loading || is_previewing {
                        self.stop_preview();
                    } else {
                        preview_sound = Some(sound.id);
                    }
                    preview_clicked = true;
                }
                play_response = Some(play_btn);
            });
        });

        ui.scope_builder(egui::UiBuilder::new().max_rect(waveform_rect), |ui| {
            ui.set_clip_rect(waveform_rect.intersect(viewport_clip_rect));
            let waveform_preview = self.cached_library_waveform_preview(sound, 72);
            ui.add_space(((waveform_rect.height() - waveform_height).max(0.0)) * 0.5);
            Self::draw_full_width_wave_strip(
                ui,
                &waveform_preview,
                playback_progress,
                Color32::from_rgb(214, 51, 132),
                Color32::from_rgb(238, 213, 227),
                Self::panel_fill(),
                if ultra_compact_row {
                    (waveform_height - 6.0).max(14.0)
                } else {
                    waveform_height
                },
            );
        });

        ui.scope_builder(egui::UiBuilder::new().max_rect(info_rect), |ui| {
            ui.set_clip_rect(info_rect.intersect(viewport_clip_rect));
            ui.set_width(info_rect.width());
            ui.set_min_width(info_rect.width());
            let title_top = if ultra_compact_row { 6.0 } else { 8.0 };
            ui.add_space(title_top);
            ui.add(
                egui::Label::new(
                    RichText::new(&sound.name)
                        .size(if ultra_compact_row { 15.0 } else { 16.5 })
                        .color(Self::strong_text_color())
                        .strong(),
                )
                .truncate(),
            );
            ui.add_space(if ultra_compact_row { 3.0 } else { 5.0 });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                ui.label(
                    RichText::new(format_time(sound.trimmed_length()))
                        .size(11.5)
                        .color(Self::muted_text_color()),
                );
                ui.label(
                    RichText::new(format!("{:.0}%", sound.volume * 100.0))
                        .size(11.5)
                        .color(Self::muted_text_color()),
                );
                if is_previewing {
                    ui.label(Self::icon(0xe050, 14.0, Color32::from_rgb(214, 51, 132)));
                }
            });
        });

        ui.scope_builder(egui::UiBuilder::new().max_rect(actions_rect), |ui| {
            ui.set_clip_rect(actions_rect.intersect(viewport_clip_rect));
            ui.add_space(((actions_rect.height() - 32.0).max(0.0)) * 0.5);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                let favorite_btn =
                    Self::favorite_button_sized(ui, sound.favorite, [38.0, 32.0], 16.0);
                if favorite_btn.clicked() {
                    favorite_sound = Some(sound.id);
                    favorite_clicked = true;
                }
                favorite_response = Some(favorite_btn);

                let copy_btn = Self::icon_action(
                    ui,
                    [38.0, 32.0],
                    0xe14d,
                    self.sound_copy_feedback_active(ui.ctx(), sound.id),
                    self.sound_copy_feedback_active(ui.ctx(), sound.id),
                );
                if copy_btn.clicked() {
                    copy_sound = Some(sound.id);
                    copy_clicked = true;
                }
                copy_response = Some(copy_btn);

                if self.folder_import_select_mode.is_none() {
                    let remove_btn = Self::icon_action(ui, [38.0, 32.0], 0xe872, false, false);
                    Self::decorate_button_response(ui, &remove_btn);
                    if remove_btn.clicked() {
                        remove_sound_from_folder = Some(sound.id);
                        remove_clicked = true;
                    }
                    remove_response = Some(remove_btn);
                }
            });
        });

        let row_interactive_rect = Rect::from_min_max(
            Pos2::new(
                (play_rect.right() + 8.0).min(row_rect.right()),
                row_rect.top(),
            ),
            Pos2::new(actions_rect.left().min(row_rect.right()), row_rect.bottom()),
        );
        let response = ui.interact(
            row_interactive_rect,
            ui.id().with(("folder-sound-row", sound.id)),
            Sense::click_and_drag(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
        if titlebar_drag_active {
            self.pending_sound_drag = None;
        } else if Self::pointer_primary_pressed_within(ui.ctx(), row_interactive_rect) {
            self.pending_sound_drag = Some(sound.id);
        } else if self.pending_sound_drag == Some(sound.id)
            && Self::pointer_primary_drag_ready(ui.ctx())
        {
            if !self.trim_timeline_drag_capture_active() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Err(error) = self.drag_sound_file_out(ui.ctx(), sound) {
                    self.set_error_status(error);
                }
                self.pending_sound_drag = None;
            }
        } else if !ui.ctx().input(|input| input.pointer.primary_down())
            && self.pending_sound_drag == Some(sound.id)
        {
            self.pending_sound_drag = None;
        }
        let over_action = favorite_response
            .as_ref()
            .is_some_and(|value| Self::response_pointer_within(ui.ctx(), value))
            || play_response
                .as_ref()
                .is_some_and(|value| Self::response_pointer_within(ui.ctx(), value))
            || copy_response
                .as_ref()
                .is_some_and(|value| Self::response_pointer_within(ui.ctx(), value))
            || remove_response
                .as_ref()
                .is_some_and(|value| Self::response_pointer_within(ui.ctx(), value));
        let action_clicked = preview_clicked || copy_clicked || favorite_clicked || remove_clicked;
        if response.clicked() && !over_action && !action_clicked {
            if self.timeline_mode_active_sound_id().is_some() {
                preview_sound = Some(sound.id);
            } else {
                open_sound = true;
            }
        }

        if let Some(sound_id) = preview_sound {
            self.preview_sound(sound_id);
        }
        if let Some(sound_id) = copy_sound {
            if let Some(sound) = self
                .sounds
                .iter()
                .find(|candidate| candidate.id == sound_id)
                .cloned()
            {
                if let Err(error) = self.copy_sound_file_to_clipboard(&sound) {
                    self.set_error_status(error);
                } else {
                    self.mark_sound_copied(ui.ctx(), sound_id);
                    self.status = Some(self.t("library.copied_to_clipboard"));
                }
            }
        }
        if let Some(sound_id) = favorite_sound {
            self.toggle_sound_favorite(sound_id, ui.ctx());
        }
        if let Some(sound_id) = remove_sound_from_folder {
            if let Some(sound) = self
                .sounds
                .iter_mut()
                .find(|candidate| candidate.id == sound_id)
            {
                sound.folder_id = None;
            }
            self.mark_dirty(ui.ctx());
        }
        if open_sound {
            self.open_sound_from_library(sound.id);
        }
        ui.advance_cursor_after_rect(row_outer_rect);
    }

    pub(super) fn draw_library_grid(&mut self, ui: &mut Ui) {
        if !(self.app_view == AppView::Library
            && self.library_tab == LibraryTab::Sounds
            && ui.ctx().input(|input| !input.raw.hovered_files.is_empty()))
        {
            self.library_drop_target_folder = None;
            self.library_drop_target_root = false;
        }
        let modal_open = self.has_modal_panel();
        let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
        let mut columns_changed = false;
        let mut row_thickness_changed = false;
        let mut library_slider_active = false;
        ui.horizontal(|ui| {
            if let Some(_import_folder_id) = self.folder_import_select_mode {
                let back_btn = ui.add(
                    Button::new(format!("< {}", self.t("library.exit_import_mode")))
                        .fill(Color32::from_rgb(227, 82, 149))
                        .corner_radius(10.0),
                );
                Self::decorate_button_response(ui, &back_btn);
                if back_btn.clicked() {
                    if let Some(folder_id) = self.folder_import_select_mode {
                        for sound_id in self.folder_import_animating.keys() {
                            if let Some(s) = self.sounds.iter_mut().find(|s| s.id == *sound_id) {
                                s.folder_id = Some(folder_id);
                            }
                        }
                        self.folder_import_animating.clear();
                        self.folder_import_select_mode = None;
                        self.library_audio_query.clear();
                        self.library_audio_tag_filter = None;
                        self.mark_dirty(ui.ctx());
                    }
                }

                ui.add_space(12.0);
                ui.label(
                    RichText::new(self.t("library.import_select_title"))
                        .font(FontId::new(16.0, FontFamily::Proportional))
                        .color(Self::strong_text_color())
                        .strong(),
                );
            } else {
                let sounds_tab = ui.add_sized(
                    [92.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.library")).size(12.5),
                        self.library_tab == LibraryTab::Sounds,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &sounds_tab);
                if sounds_tab.clicked() {
                    self.library_tab = LibraryTab::Sounds;
                }

                ui.add_space(8.0);
                let videos_tab = ui.add_sized(
                    [84.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.video")).size(12.5),
                        self.library_tab == LibraryTab::Videos,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &videos_tab);
                if videos_tab.clicked() {
                    self.library_tab = LibraryTab::Videos;
                }
                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(18.0)
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(260.0);
                        ui.horizontal(|ui| {
                            if self.library_tab == LibraryTab::Sounds {
                                self.draw_library_tag_toggle(ui);
                                ui.add_space(8.0);
                            }
                            ui.label(Self::icon(0xe8b6, 16.0, Self::muted_text_color()));
                            let search_hint = self.t("library.search");
                            let query = if self.library_tab == LibraryTab::Videos {
                                &mut self.library_video_query
                            } else {
                                &mut self.library_audio_query
                            };
                            ui.add_sized(
                                [ui.available_width(), 24.0],
                                TextEdit::singleline(query)
                                    .frame(false)
                                    .hint_text(search_hint)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    });
            }

            if self.library_tab == LibraryTab::Sounds || self.library_tab == LibraryTab::Videos {
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let favorites_active = if self.library_tab == LibraryTab::Videos {
                        self.library_favorites_only_video
                    } else {
                        self.library_favorites_only_audio
                    };
                    let favorite_filter = ui.add_sized(
                        [40.0, 30.0],
                        Button::new(Self::icon(
                            if favorites_active { 0xe838 } else { 0xe83a },
                            18.0,
                            if favorites_active {
                                Color32::from_rgb(82, 58, 0)
                            } else {
                                Self::strong_text_color()
                            },
                        ))
                        .fill(if favorites_active {
                            Color32::from_rgb(247, 191, 64)
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(41, 34, 47, 224)
                        } else {
                            Color32::from_rgba_premultiplied(237, 231, 238, 198)
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if favorites_active {
                                Color32::from_rgb(247, 191, 64)
                            } else if self.dark_theme {
                                Color32::from_rgb(84, 69, 92)
                            } else {
                                Color32::from_rgb(221, 212, 222)
                            },
                        ))
                        .corner_radius(9.0),
                    );
                    Self::decorate_button_response(ui, &favorite_filter);
                    if favorite_filter.clicked() {
                        if self.library_tab == LibraryTab::Videos {
                            self.library_favorites_only_video = !self.library_favorites_only_video;
                        } else {
                            self.library_favorites_only_audio = !self.library_favorites_only_audio;
                        }
                    }
                    ui.add_space(8.0);
                    if self.library_tab == LibraryTab::Sounds {
                        let grid_btn = ui.add_sized(
                            [58.0, 30.0],
                            Self::action_button(
                                RichText::new(self.t("library.grid")).size(11.5),
                                self.library_sound_view == LibrarySoundView::Grid,
                                false,
                            ),
                        );
                        Self::decorate_button_response(ui, &grid_btn);
                        if grid_btn.clicked() {
                            self.library_sound_view = LibrarySoundView::Grid;
                            let _ = self.storage.save_library_sound_view(
                                self.library_sound_view.preference_value(),
                            );
                        }
                        ui.add_space(6.0);
                        let rows_btn = ui.add_sized(
                            [58.0, 30.0],
                            Self::action_button(
                                RichText::new(self.t("library.rows")).size(11.5),
                                self.library_sound_view == LibrarySoundView::Rows,
                                false,
                            ),
                        );
                        Self::decorate_button_response(ui, &rows_btn);
                        if rows_btn.clicked() {
                            self.library_sound_view = LibrarySoundView::Rows;
                            let _ = self.storage.save_library_sound_view(
                                self.library_sound_view.preference_value(),
                            );
                        }
                        ui.add_space(8.0);
                    }
                    if self.library_tab == LibraryTab::Sounds
                        && self.library_sound_view == LibrarySoundView::Grid
                    {
                        Self::with_slider_visuals(ui, |ui| {
                            let mut slider_value =
                                (LIBRARY_GRID_MIN_COLUMNS + LIBRARY_GRID_MAX_COLUMNS) as f32
                                    - self.library_grid_columns as f32;
                            let (slider_response, slider_changed) = Self::click_slider(
                                ui,
                                &mut slider_value,
                                LIBRARY_GRID_MIN_COLUMNS as f32..=LIBRARY_GRID_MAX_COLUMNS as f32,
                                1.0,
                                vec2(132.0, 28.0),
                            );
                            library_slider_active = slider_response.hovered()
                                || slider_response.dragged()
                                || slider_response.is_pointer_button_down_on();
                            if slider_response.changed() || slider_changed {
                                let reversed = slider_value.round().clamp(
                                    LIBRARY_GRID_MIN_COLUMNS as f32,
                                    LIBRARY_GRID_MAX_COLUMNS as f32,
                                ) as usize;
                                self.library_grid_columns = (LIBRARY_GRID_MIN_COLUMNS
                                    + LIBRARY_GRID_MAX_COLUMNS)
                                    - reversed;
                                self.library_grid_columns = self
                                    .library_grid_columns
                                    .clamp(LIBRARY_GRID_MIN_COLUMNS, LIBRARY_GRID_MAX_COLUMNS);
                                columns_changed = true;
                            }
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    self.library_grid_columns,
                                    self.t("library.columns")
                                ))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                            );
                        });
                        if library_slider_active {
                            self.pending_sound_drag = None;
                        }
                    } else if self.library_tab == LibraryTab::Sounds
                        && self.library_sound_view == LibrarySoundView::Rows
                    {
                        Self::with_slider_visuals(ui, |ui| {
                            let mut slider_value =
                                (LIBRARY_ROW_MIN_THICKNESS + LIBRARY_ROW_MAX_THICKNESS) as f32
                                    - self.library_row_thickness as f32;
                            let (slider_response, slider_changed) = Self::click_slider(
                                ui,
                                &mut slider_value,
                                LIBRARY_ROW_MIN_THICKNESS as f32..=LIBRARY_ROW_MAX_THICKNESS as f32,
                                1.0,
                                vec2(132.0, 28.0),
                            );
                            library_slider_active = slider_response.hovered()
                                || slider_response.dragged()
                                || slider_response.is_pointer_button_down_on();
                            if slider_response.changed() || slider_changed {
                                let reversed = slider_value.round().clamp(
                                    LIBRARY_ROW_MIN_THICKNESS as f32,
                                    LIBRARY_ROW_MAX_THICKNESS as f32,
                                ) as usize;
                                self.library_row_thickness = (LIBRARY_ROW_MIN_THICKNESS
                                    + LIBRARY_ROW_MAX_THICKNESS)
                                    - reversed;
                                self.library_row_thickness = self
                                    .library_row_thickness
                                    .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS);
                                row_thickness_changed = true;
                            }
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    self.t("library.row_label").replace(
                                        "{value}",
                                        &self.library_row_thickness.to_string(),
                                    ),
                                )
                                .size(11.5)
                                .color(Self::muted_text_color()),
                            );
                        });
                        if library_slider_active {
                            self.pending_sound_drag = None;
                        }
                    }
                });
            }
        });
        if self.library_tab == LibraryTab::Sounds && self.library_audio_tags_expanded {
            ui.add_space(10.0);
            Frame::new()
                .fill(Self::surface_fill())
                .stroke(Stroke::new(1.0, Self::border_color()))
                .corner_radius(18.0)
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    self.draw_library_tag_filter_row(ui);
                });
        }
        if columns_changed {
            let _ = self
                .storage
                .save_library_grid_columns(self.library_grid_columns);
        }
        if row_thickness_changed {
            let _ = self
                .storage
                .save_library_row_thickness(self.library_row_thickness);
        }
        ui.add_space(10.0);

        ScrollArea::vertical()
            .drag_to_scroll(false)
            .auto_shrink([true, false])
            .show(ui, |ui| {
                let viewport_width = ui.clip_rect().width().min(ui.available_width());
                ui.set_width(viewport_width);
                ui.set_max_width(viewport_width);

                if self.library_tab == LibraryTab::Videos {
                    self.draw_video_library_grid(ui);
                    return;
                }

                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        self.draw_folders_list_view(ui);
                    });
                ui.add_space(12.0);

                ui.set_max_width(viewport_width);

                if self.sounds.is_empty() {
                    self.draw_empty_editor(ui);
                    return;
                }

                if self.folder_import_select_mode.is_some() {
                    let sounds = self.filtered_library_sounds();
                    if sounds.is_empty() {
                        self.draw_empty_editor(ui);
                        return;
                    }
                    let layout_width = ui.clip_rect().width().min(ui.available_width());
                    self.draw_library_sound_grid_content(
                        ui,
                        &sounds,
                        layout_width,
                        modal_open,
                        library_slider_active,
                        titlebar_drag_active,
                    );
                }
            });
    }

    pub(super) fn draw_library_sound_grid_content(
        &mut self,
        ui: &mut Ui,
        sounds: &[SoundEffect],
        layout_width: f32,
        modal_open: bool,
        library_slider_active: bool,
        titlebar_drag_active: bool,
    ) {
        let spacing = 14.0;
        let columns = self
            .library_grid_columns
            .clamp(LIBRARY_GRID_MIN_COLUMNS, LIBRARY_GRID_MAX_COLUMNS);
        let mut open_sound = None;
        let mut preview_sound = None;
        let mut copy_sound = None;
        let mut drag_sound = None;
        let mut favorite_sound = None;
        let mut remove_sound_from_folder = None;

        let total_gap_width = spacing * (columns.saturating_sub(1)) as f32;
        let minimum_grid_width = total_gap_width + 22.0 * columns as f32;
        let target_grid_width = (layout_width - 28.0).max(minimum_grid_width);
        let card_size = ((target_grid_width - total_gap_width) / columns as f32).max(22.0);
        let grid_width = card_size * columns as f32 + total_gap_width;
        let side_padding = ((layout_width - grid_width) * 0.5).max(0.0);
        let row_count = sounds.len().div_ceil(columns);

        let preload_paths = sounds
            .iter()
            .map(|sound| self.preview_asset_path_for_sound(sound))
            .collect::<Vec<_>>();
        for path in preload_paths {
            self.schedule_audio_preload(path);
        }

        for (row_index, row) in sounds.chunks(columns).enumerate() {
            let (row_rect, _) =
                ui.allocate_exact_size(vec2(layout_width, card_size), Sense::hover());

            for (column_index, sound) in row.iter().enumerate() {
                let tile_rect = Rect::from_min_size(
                    Pos2::new(
                        row_rect.left()
                            + side_padding
                            + column_index as f32 * (card_size + spacing),
                        row_rect.top(),
                    ),
                    vec2(card_size, card_size),
                );
                let body_response = ui.interact(
                    tile_rect,
                    ui.id().with(("library-grid", sound.id)),
                    if modal_open {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let pointer_hover = !modal_open
                    && ui
                        .ctx()
                        .input(|input| input.pointer.hover_pos())
                        .is_some_and(|pos| tile_rect.contains(pos));
                let hovered = !modal_open && pointer_hover;
                let is_playing = self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.is_playing(sound.id));

                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if titlebar_drag_active {
                    self.pending_sound_drag = None;
                }
                if !modal_open
                    && !library_slider_active
                    && !titlebar_drag_active
                    && Self::pointer_primary_pressed_within(ui.ctx(), tile_rect)
                {
                    self.pending_sound_drag = Some(sound.id);
                }
                if !modal_open
                    && !library_slider_active
                    && !titlebar_drag_active
                    && self.pending_sound_drag == Some(sound.id)
                    && pointer_hover
                    && ui.ctx().input(|input| input.pointer.primary_down())
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if !modal_open
                    && !library_slider_active
                    && !titlebar_drag_active
                    && self.pending_sound_drag == Some(sound.id)
                    && Self::pointer_primary_drag_ready(ui.ctx())
                {
                    if !self.trim_timeline_drag_capture_active() {
                        drag_sound = Some(sound.id);
                        self.pending_sound_drag = None;
                    }
                }
                let mut body_clicked = false;
                if !modal_open && body_response.clicked() {
                    body_clicked = true;
                }

                let mut scale = 1.0;
                let mut opacity = 1.0;
                if let Some(start_time) = self.folder_import_animating.get(&sound.id) {
                    let elapsed = start_time.elapsed().as_secs_f32();
                    let progress = (elapsed / 0.2).clamp(0.0, 1.0);
                    scale = 1.0 - progress;
                    opacity = 1.0 - progress;
                    ui.ctx().request_repaint();
                }

                let card_center = tile_rect.center();
                let animated_size = vec2(card_size * scale, card_size * scale);
                let animated_rect = Rect::from_center_size(card_center, animated_size);

                let mut play_btn_response = None;
                let mut remove_btn_response = None;

                ui.scope_builder(egui::UiBuilder::new().max_rect(animated_rect), |ui| {
                    ui.style_mut().interaction.selectable_labels = false;
                    let fill = if hovered {
                        Color32::from_rgb(227, 82, 149)
                    } else {
                        Self::surface_fill()
                    };
                    let stroke = if hovered {
                        Color32::from_rgb(227, 82, 149)
                    } else {
                        Self::border_color()
                    };
                    let title_color = if hovered {
                        Color32::WHITE
                    } else {
                        Self::strong_text_color()
                    };
                    let meta_color = if hovered {
                        Color32::from_rgba_premultiplied(255, 255, 255, 196)
                    } else {
                        Self::muted_text_color()
                    };
                    let card_padding = (card_size * 0.12).clamp(4.0, 16.0);
                    let compact_card = card_size < 118.0;
                    let ultra_compact_card = card_size < 76.0;

                    let fill = fill.linear_multiply(opacity);
                    let stroke = stroke.linear_multiply(opacity);
                    let title_color = title_color.linear_multiply(opacity);
                    let meta_color = meta_color.linear_multiply(opacity);

                    Frame::new()
                        .fill(fill)
                        .stroke(Stroke::new(1.0, stroke))
                        .shadow(Shadow {
                            offset: [0, 12],
                            blur: 28,
                            spread: 0,
                            color: Color32::from_rgba_premultiplied(86, 43, 67, 18)
                                .linear_multiply(opacity),
                        })
                        .corner_radius(30.0)
                        .inner_margin(Margin::same(card_padding.round() as i8))
                        .show(ui, |ui| {
                            let inner_size = (card_size - card_padding * 2.0).max(8.0);
                            let action_gap = if inner_size < 132.0 { 4.0 } else { 8.0 };
                            let action_button_width =
                                ((inner_size - action_gap * 2.0) / 3.0).clamp(28.0, 46.0);
                            let action_button_height = if action_button_width < 34.0 {
                                28.0
                            } else {
                                31.0
                            };
                            let action_button_size = [action_button_width, action_button_height];
                            let action_icon_size = if action_button_width < 34.0 {
                                16.0
                            } else {
                                18.0
                            };
                            ui.set_min_size(vec2(inner_size, inner_size));
                            ui.set_width(inner_size);
                            ui.vertical(|ui| {
                                if !ultra_compact_card {
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            [inner_size - 22.0, 18.0],
                                            egui::Label::new(
                                                RichText::new(&sound.name)
                                                    .size(12.5)
                                                    .color(title_color)
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );

                                        if self.library_current_folder.is_some()
                                            && self.folder_import_select_mode.is_none()
                                        {
                                            let remove_btn = ui.add(
                                                Button::new(Self::icon(0xe5cd, 11.0, meta_color))
                                                    .fill(Color32::TRANSPARENT)
                                                    .frame(false),
                                            );
                                            Self::decorate_button_response(ui, &remove_btn);
                                            if remove_btn.clicked() {
                                                remove_sound_from_folder = Some(sound.id);
                                            }
                                            remove_btn_response = Some(remove_btn);
                                        }
                                    });
                                    ui.add_space(7.0);
                                }
                                let bucket_count =
                                    (card_size * 0.34).round().clamp(20.0, 52.0) as usize;
                                let waveform_preview =
                                    self.cached_library_waveform_preview(sound, bucket_count);
                                let w_color1 = if hovered {
                                    Color32::from_rgb(255, 214, 232)
                                } else {
                                    Color32::from_rgb(214, 51, 132)
                                };
                                let w_color2 = if hovered {
                                    Color32::from_rgb(255, 214, 232)
                                } else {
                                    Color32::from_rgb(238, 213, 227)
                                };
                                let w_fill = if hovered {
                                    Color32::from_rgba_premultiplied(255, 255, 255, 22)
                                } else {
                                    Self::panel_fill()
                                };
                                Self::draw_wave_strip(
                                    ui,
                                    &waveform_preview,
                                    None,
                                    w_color1.linear_multiply(opacity),
                                    w_color2.linear_multiply(opacity),
                                    w_fill.linear_multiply(opacity),
                                    (card_size * 0.38).clamp(18.0, 78.0),
                                );
                                if self.folder_import_select_mode.is_some() {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        let center_gap = (inner_size - action_button_width) * 0.5;
                                        if center_gap > 0.0 {
                                            ui.add_space(center_gap);
                                        }
                                        let is_loading = self
                                            .pending_preview_after_preload
                                            .is_some_and(|(id, _)| id == sound.id);
                                        let play_btn = Self::icon_action(
                                            ui,
                                            action_button_size,
                                            if is_playing {
                                                0xe047
                                            } else if is_loading {
                                                0xe5d5
                                            } else {
                                                0xe037
                                            },
                                            is_loading || is_playing,
                                            false,
                                        );
                                        if play_btn.clicked() {
                                            if is_loading || is_playing {
                                                self.stop_preview();
                                            } else {
                                                preview_sound = Some(sound.id);
                                            }
                                        }
                                        play_btn_response = Some(play_btn);
                                    });
                                } else if !compact_card {
                                    ui.add_space(9.0);
                                    ui.label(
                                        RichText::new(format_time(sound.trimmed_length()))
                                            .size(11.5)
                                            .color(meta_color),
                                    );
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = action_gap;
                                        let action_row_width =
                                            action_button_width * 3.0 + action_gap * 2.0;
                                        ui.add_space(
                                            ((inner_size - action_row_width) * 0.5).max(0.0),
                                        );
                                        if Self::favorite_button_sized(
                                            ui,
                                            sound.favorite,
                                            action_button_size,
                                            action_icon_size,
                                        )
                                        .clicked()
                                        {
                                            favorite_sound = Some(sound.id);
                                        }
                                        let is_loading = self
                                            .pending_preview_after_preload
                                            .is_some_and(|(id, _)| id == sound.id);
                                        let play_btn = Self::icon_action(
                                            ui,
                                            action_button_size,
                                            if is_playing {
                                                0xe047
                                            } else if is_loading {
                                                0xe5d5
                                            } else {
                                                0xe037
                                            },
                                            is_loading || is_playing,
                                            false,
                                        );
                                        if play_btn.clicked() {
                                            if is_loading || is_playing {
                                                self.stop_preview();
                                            } else {
                                                preview_sound = Some(sound.id);
                                            }
                                        }
                                        play_btn_response = Some(play_btn);
                                        if Self::icon_action(
                                            ui,
                                            action_button_size,
                                            0xe14d,
                                            self.sound_copy_feedback_active(ui.ctx(), sound.id),
                                            self.sound_copy_feedback_active(ui.ctx(), sound.id),
                                        )
                                        .clicked()
                                        {
                                            copy_sound = Some(sound.id);
                                        }
                                    });
                                    if self.sound_copy_feedback_active(ui.ctx(), sound.id) {
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new(self.t("common.copied"))
                                                .size(11.0)
                                                .color(meta_color),
                                        );
                                    }
                                }
                            });
                        });
                });

                if body_clicked && !self.folder_import_animating.contains_key(&sound.id) {
                    let is_over_play = play_btn_response.as_ref().is_some_and(|r| r.hovered());
                    let is_over_remove = remove_btn_response.as_ref().is_some_and(|r| r.hovered());
                    if !is_over_play && !is_over_remove {
                        if let Some(_import_folder_id) = self.folder_import_select_mode {
                            self.folder_import_animating
                                .insert(sound.id, Instant::now());
                        } else {
                            open_sound = Some(sound.id);
                        }
                    }
                }
            }

            if row_index + 1 < row_count {
                ui.add_space(spacing);
            }
        }

        let mut completed_imports = Vec::new();
        self.folder_import_animating.retain(|sound_id, start_time| {
            if start_time.elapsed().as_secs_f32() >= 0.2 {
                completed_imports.push(*sound_id);
                false
            } else {
                true
            }
        });
        if !completed_imports.is_empty() {
            if let Some(folder_id) = self.folder_import_select_mode {
                for sound_id in completed_imports {
                    if let Some(s) = self.sounds.iter_mut().find(|s| s.id == sound_id) {
                        s.folder_id = Some(folder_id);
                    }
                }
                self.mark_dirty(ui.ctx());
            }
        }

        if let Some(sound_id) = remove_sound_from_folder {
            if let Some(s) = self.sounds.iter_mut().find(|s| s.id == sound_id) {
                s.folder_id = None;
            }
            self.mark_dirty(ui.ctx());
        }

        ui.add_space(16.0);

        if let Some(sound_id) = preview_sound {
            self.preview_sound(sound_id);
        }
        if let Some(sound_id) = drag_sound {
            if let Some(sound) = self
                .sounds
                .iter()
                .find(|sound| sound.id == sound_id)
                .cloned()
            {
                if let Err(error) = self.drag_sound_file_out(ui.ctx(), &sound) {
                    self.set_error_status(error);
                }
            }
        }
        if let Some(sound_id) = copy_sound {
            if let Some(sound) = self
                .sounds
                .iter()
                .find(|sound| sound.id == sound_id)
                .cloned()
            {
                if let Err(error) = self.copy_sound_file_to_clipboard(&sound) {
                    self.set_error_status(error);
                } else {
                    self.mark_sound_copied(ui.ctx(), sound_id);
                    self.status = Some(self.t("library.copied_to_clipboard"));
                }
            }
        }
        if let Some(sound_id) = favorite_sound {
            self.toggle_sound_favorite(sound_id, ui.ctx());
        }
        if let Some(sound_id) = open_sound {
            self.open_sound_from_library(sound_id);
        }
    }

    pub(super) fn draw_video_library_grid(&mut self, ui: &mut Ui) {
        let modal_open = self.has_modal_panel();
        let videos = self.filtered_library_videos();
        if videos.is_empty() {
            Frame::new()
                .fill(Self::surface_fill())
                .stroke(Stroke::new(1.0, Self::border_color()))
                .corner_radius(28.0)
                .inner_margin(Margin::same(22))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        ui.label(Self::icon(0xe04b, 40.0, Color32::from_rgb(214, 51, 132)));
                    });
                });
            return;
        }

        let spacing = 14.0;
        let layout_width = ui.clip_rect().width().min(ui.available_width());
        let columns = self
            .library_grid_columns
            .clamp(LIBRARY_GRID_MIN_COLUMNS, LIBRARY_GRID_MAX_COLUMNS);
        let total_gap_width = spacing * (columns.saturating_sub(1)) as f32;
        let minimum_grid_width = total_gap_width + 22.0 * columns as f32;
        let target_grid_width = (layout_width - 28.0).max(minimum_grid_width);
        let card_size = ((target_grid_width - total_gap_width) / columns as f32).max(22.0);
        let grid_width = card_size * columns as f32 + total_gap_width;
        let side_padding = ((layout_width - grid_width) * 0.5).max(0.0);
        let row_count = videos.len().div_ceil(columns);
        let mut open_video: Option<VideoAsset> = None;
        let mut copy_video: Option<VideoAsset> = None;
        let mut delete_video: Option<Uuid> = None;
        let mut favorite_video: Option<Uuid> = None;

        for (row_index, row) in videos.chunks(columns).enumerate() {
            let (row_rect, _) =
                ui.allocate_exact_size(vec2(layout_width, card_size), Sense::hover());
            for (column_index, video) in row.iter().enumerate() {
                let tile_rect = Rect::from_min_size(
                    Pos2::new(
                        row_rect.left()
                            + side_padding
                            + column_index as f32 * (card_size + spacing),
                        row_rect.top(),
                    ),
                    vec2(card_size, card_size),
                );
                let tile_response = ui.interact(
                    tile_rect,
                    ui.id().with(("video-grid-tile", video.id)),
                    Sense::hover(),
                );
                let body_rect = Rect::from_min_max(
                    tile_rect.min,
                    Pos2::new(tile_rect.max.x, tile_rect.max.y - 46.0),
                );
                let body_response = ui.interact(
                    body_rect,
                    ui.id().with(("video-grid", video.id)),
                    if modal_open {
                        Sense::hover()
                    } else {
                        Sense::click()
                    },
                );
                let hovered = !modal_open && (tile_response.hovered() || body_response.hovered());
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if !modal_open && body_response.clicked() {
                    open_video = Some(video.clone());
                }

                ui.scope_builder(egui::UiBuilder::new().max_rect(tile_rect), |ui| {
                    let card_padding = (card_size * 0.12).clamp(4.0, 16.0);
                    let compact_card = card_size < 118.0;
                    let ultra_compact_card = card_size < 76.0;
                    Frame::new()
                        .fill(if hovered {
                            Color32::from_rgb(227, 82, 149)
                        } else {
                            Self::surface_fill()
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if hovered {
                                Color32::from_rgb(227, 82, 149)
                            } else {
                                Self::border_color()
                            },
                        ))
                        .shadow(Shadow {
                            offset: [0, 12],
                            blur: 28,
                            spread: 0,
                            color: Color32::from_rgba_premultiplied(86, 43, 67, 18),
                        })
                        .corner_radius(30.0)
                        .inner_margin(Margin::same(card_padding.round() as i8))
                        .show(ui, |ui| {
                            let inner_size = (card_size - card_padding * 2.0).max(8.0);
                            let action_gap = if inner_size < 158.0 { 4.0 } else { 8.0 };
                            let action_button_width =
                                ((inner_size - action_gap * 3.0) / 4.0).clamp(28.0, 46.0);
                            let action_button_height = if action_button_width < 34.0 {
                                28.0
                            } else {
                                31.0
                            };
                            let action_button_size = [action_button_width, action_button_height];
                            let action_icon_size = if action_button_width < 34.0 {
                                16.0
                            } else {
                                18.0
                            };
                            let title_color = if hovered {
                                Color32::WHITE
                            } else {
                                Self::strong_text_color()
                            };
                            let meta_color = if hovered {
                                Color32::from_rgba_premultiplied(255, 255, 255, 196)
                            } else {
                                Self::muted_text_color()
                            };

                            ui.set_min_size(vec2(inner_size, inner_size));
                            ui.set_width(inner_size);
                            ui.vertical(|ui| {
                                if !ultra_compact_card {
                                    ui.add_sized(
                                        [inner_size, 18.0],
                                        egui::Label::new(
                                            RichText::new(&video.name)
                                                .size(12.5)
                                                .color(title_color)
                                                .strong(),
                                        )
                                        .truncate(),
                                    );
                                    ui.add_space(7.0);
                                }
                                let bucket_count =
                                    (card_size * 0.34).round().clamp(20.0, 52.0) as usize;
                                let waveform_preview =
                                    Self::compact_library_waveform(&video.waveform, bucket_count);
                                Frame::new()
                                    .fill(if hovered {
                                        Color32::from_rgba_premultiplied(255, 255, 255, 22)
                                    } else {
                                        Self::panel_fill()
                                    })
                                    .corner_radius(20.0)
                                    .inner_margin(Margin::same(12))
                                    .show(ui, |ui| {
                                        ui.set_min_height((card_size * 0.38).clamp(18.0, 78.0));
                                        ui.vertical_centered(|ui| {
                                            Self::draw_wave_strip(
                                                ui,
                                                &waveform_preview,
                                                None,
                                                if hovered {
                                                    Color32::from_rgb(255, 214, 232)
                                                } else {
                                                    Color32::from_rgb(214, 51, 132)
                                                },
                                                if hovered {
                                                    Color32::from_rgb(255, 214, 232)
                                                } else {
                                                    Color32::from_rgb(238, 213, 227)
                                                },
                                                if hovered {
                                                    Color32::from_rgba_premultiplied(
                                                        255, 255, 255, 22,
                                                    )
                                                } else {
                                                    Self::panel_fill()
                                                },
                                                (card_size * 0.38).clamp(18.0, 78.0),
                                            );
                                        });
                                    });
                                if !compact_card {
                                    ui.add_space(9.0);
                                    ui.label(
                                        RichText::new(format_time(video.duration_secs))
                                            .size(11.5)
                                            .color(meta_color),
                                    );
                                    ui.add_space(10.0);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = action_gap;
                                        if Self::favorite_button_sized(
                                            ui,
                                            video.favorite,
                                            action_button_size,
                                            action_icon_size,
                                        )
                                        .clicked()
                                        {
                                            favorite_video = Some(video.id);
                                        }
                                        if Self::icon_action(
                                            ui,
                                            action_button_size,
                                            0xe89e,
                                            false,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            open_video = Some(video.clone());
                                        }
                                        if Self::icon_action(
                                            ui,
                                            action_button_size,
                                            0xe14d,
                                            self.video_copy_feedback_active(ui.ctx(), video.id),
                                            self.video_copy_feedback_active(ui.ctx(), video.id),
                                        )
                                        .clicked()
                                        {
                                            copy_video = Some(video.clone());
                                        }
                                        if Self::icon_action(
                                            ui,
                                            action_button_size,
                                            0xe872,
                                            false,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            delete_video = Some(video.id);
                                        }
                                    });
                                    if self.video_copy_feedback_active(ui.ctx(), video.id) {
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new(self.t("common.copied"))
                                                .size(11.0)
                                                .color(meta_color),
                                        );
                                    }
                                }
                            });
                        });
                });
            }
            if row_index + 1 < row_count {
                ui.add_space(spacing);
            }
        }
        ui.add_space(208.0);

        if let Some(video) = open_video {
            if let Err(error) = self.prepare_video_viewer(ui.ctx(), &video) {
                self.set_error_status(error);
            } else if let Err(error) = self.play_video_viewer_from_current_playhead() {
                self.set_error_status(error);
            }
        }
        if let Some(video) = copy_video {
            if let Err(error) = self.copy_video_file_to_clipboard(&video) {
                self.set_error_status(error);
            } else {
                self.mark_video_copied(ui.ctx(), video.id);
                self.status = Some(self.t("library.copied_to_clipboard"));
            }
        }
        if let Some(video_id) = favorite_video {
            self.toggle_video_favorite(video_id);
        }
        if let Some(video_id) = delete_video
            && let Some(index) = self
                .video_assets
                .iter()
                .position(|video| video.id == video_id)
        {
            let video = self.video_assets.remove(index);
            if let Err(error) = self.storage.remove_video(&video) {
                self.set_error_status(error);
            } else {
                let _ = self.storage.save_video_library(&self.video_assets);
            }
        }
    }

    pub(super) fn draw_library(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.add_space(2.0);
            let search_panel = Frame::new()
                .fill(Self::surface_fill())
                .stroke(Stroke::new(1.0, Self::border_color()))
                .corner_radius(18.0)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Self::icon(0xe8b6, 16.0, Self::muted_text_color()));
                        let search_hint = self.t("library.search");
                        ui.add_sized(
                            [ui.available_width(), 24.0],
                            TextEdit::singleline(&mut self.library_audio_query)
                                .frame(false)
                                .hint_text(search_hint)
                                .desired_width(f32::INFINITY),
                        )
                    })
                    .inner
                });
            let search_block_rect = search_panel.response.rect.expand2(vec2(12.0, 10.0));
            ui.add_space(12.0);
            let visible_sound_indices = self.filtered_library_sound_indices();
            if visible_sound_indices.is_empty() {
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 10],
                        blur: 22,
                        spread: 0,
                        color: Self::shadow_color(),
                    })
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("+")
                                .size(28.0)
                                .color(Color32::from_rgb(214, 51, 132)),
                        );
                    });
                return;
            }

            let modal_open = self.has_modal_panel();
            let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
            let search_drag_blocked = ui.ctx().input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .or(input.pointer.press_origin())
                    .is_some_and(|pos| search_block_rect.contains(pos))
            });
            let mut preview_request = None;
            let mut drag_request = None;
            let list_width = ui.available_width().max(220.0);

            let preload_paths = visible_sound_indices
                .iter()
                .map(|&idx| self.preview_asset_path_for_sound(&self.sounds[idx]))
                .collect::<Vec<_>>();
            for path in preload_paths {
                self.schedule_audio_preload(path);
            }

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(
                    ui,
                    self.library_list_row_height(),
                    visible_sound_indices.len(),
                    |ui, row_range| {
                        ui.set_width(list_width);
                        ui.set_min_width(list_width);
                        for row_index in row_range {
                            let sound_index = visible_sound_indices[row_index];
                            let sound = &self.sounds[sound_index];
                            let selected = self.selected == Some(sound.id);
                            let playing = self
                                .audio
                                .as_ref()
                                .is_some_and(|audio| audio.is_playing(sound.id));
                            let is_loading = self
                                .pending_preview_after_preload
                                .is_some_and(|(id, _)| id == sound.id);
                            let progress = self
                                .audio
                                .as_ref()
                                .and_then(|audio| audio.playback_progress(sound.id));
                            let row_padding_y = self.library_row_vertical_padding();
                            let waveform_height = self.library_row_wave_height();
                            let compact_row =
                                self.library_row_thickness <= LIBRARY_ROW_MIN_THICKNESS + 1;
                            let row_width = list_width;
                            let content_width = (row_width - 28.0).max(220.0);
                            if playing {
                                ui.ctx().request_repaint_after(Duration::from_millis(
                                    ACTIVE_UI_REPAINT_MS,
                                ));
                            }
                            let row_height = self.library_list_row_height();
                            let (row_rect, _) =
                                ui.allocate_exact_size(vec2(row_width, row_height), Sense::hover());
                            let visible_row_rect = row_rect.intersect(ui.clip_rect());
                            if visible_row_rect.is_negative() || visible_row_rect.height() <= 0.0 {
                                continue;
                            }
                            let row_fill = if selected {
                                if self.dark_theme {
                                    Color32::from_rgb(60, 25, 52)
                                } else {
                                    Color32::from_rgb(255, 239, 247)
                                }
                            } else {
                                Self::surface_fill()
                            };
                            let row_stroke = Stroke::new(
                                1.0,
                                if selected {
                                    Color32::from_rgb(235, 118, 171)
                                } else {
                                    Self::border_color()
                                },
                            );
                            let row_painter = ui.painter().with_clip_rect(ui.clip_rect());
                            row_painter.rect_filled(row_rect, 22.0, row_fill);
                            row_painter.rect_stroke(row_rect, 22.0, row_stroke, StrokeKind::Inside);
                            let inner_rect = row_rect.shrink2(vec2(14.0, row_padding_y as f32));
                            let inner_clip_rect = inner_rect.intersect(ui.clip_rect());
                            ui.scope_builder(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
                                ui.style_mut().interaction.selectable_labels = false;
                                ui.set_clip_rect(inner_clip_rect);
                                ui.set_width(inner_rect.width());
                                ui.set_min_width(inner_rect.width());
                                ui.set_min_size(inner_rect.size());
                                ui.allocate_ui_with_layout(
                                    vec2(content_width, inner_rect.height()),
                                    egui::Layout::top_down(Align::Min),
                                    |ui| {
                                        let title_spacing = if compact_row { 4.0 } else { 6.0 };
                                        let meta_spacing = if compact_row { 4.0 } else { 8.0 };
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&sound.name)
                                                    .size(if compact_row { 15.0 } else { 16.5 })
                                                    .color(Self::strong_text_color())
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_space(title_spacing);
                                        let waveform_preview =
                                            self.cached_library_waveform_preview(sound, 64);
                                        Self::draw_full_width_wave_strip(
                                            ui,
                                            &waveform_preview,
                                            progress,
                                            Color32::from_rgb(214, 51, 132),
                                            Color32::from_rgb(238, 213, 227),
                                            Self::panel_fill(),
                                            waveform_height,
                                        );
                                        ui.add_space(meta_spacing);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                                            ui.label(
                                                RichText::new(format_time(sound.trimmed_length()))
                                                    .size(if compact_row { 10.5 } else { 11.5 })
                                                    .color(Self::muted_text_color()),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{:.0}%",
                                                    sound.volume * 100.0
                                                ))
                                                .size(if compact_row { 10.5 } else { 11.5 })
                                                .color(Self::muted_text_color()),
                                            );
                                            if playing {
                                                ui.label(Self::icon(
                                                    0xe050,
                                                    14.0,
                                                    Color32::from_rgb(214, 51, 132),
                                                ));
                                            }
                                        });
                                    },
                                );
                            });
                            let scrollbar_gutter = 18.0;
                            let interactive_rect = Rect::from_min_max(
                                row_rect.min,
                                Pos2::new(
                                    (row_rect.max.x - scrollbar_gutter).max(row_rect.min.x),
                                    row_rect.max.y,
                                ),
                            );

                            let response = ui.interact(
                                interactive_rect,
                                ui.id().with(sound.id),
                                Sense::click_and_drag(),
                            );
                            let pointer_hover = ui
                                .ctx()
                                .input(|input| input.pointer.hover_pos())
                                .is_some_and(|pos| interactive_rect.contains(pos));
                            if search_drag_blocked {
                                self.pending_sound_drag = None;
                            }
                            if titlebar_drag_active {
                                self.pending_sound_drag = None;
                            }
                            if !modal_open
                                && !search_drag_blocked
                                && !titlebar_drag_active
                                && Self::pointer_primary_pressed_within(ui.ctx(), interactive_rect)
                            {
                                self.pending_sound_drag = Some(sound.id);
                            }
                            if !modal_open && !search_drag_blocked && pointer_hover {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                            }
                            if !modal_open
                                && !search_drag_blocked
                                && !titlebar_drag_active
                                && self.pending_sound_drag == Some(sound.id)
                                && pointer_hover
                                && ui.ctx().input(|input| input.pointer.primary_down())
                            {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            }
                            if !modal_open
                                && !search_drag_blocked
                                && !titlebar_drag_active
                                && self.pending_sound_drag == Some(sound.id)
                                && Self::pointer_primary_drag_ready(ui.ctx())
                            {
                                if !self.trim_timeline_drag_capture_active() {
                                    drag_request = Some(sound.id);
                                    self.pending_sound_drag = None;
                                }
                            }
                            if !modal_open && response.clicked() {
                                if self.timeline_mode_active_sound_id().is_some() {
                                    preview_request = Some(sound.id);
                                } else {
                                    self.selected = Some(sound.id);
                                    if is_loading || playing {
                                        self.stop_preview();
                                    } else {
                                        preview_request = Some(sound.id);
                                    }
                                }
                            }
                            ui.advance_cursor_after_rect(row_rect);
                        }
                    },
                );

            if let Some(sound_id) = preview_request {
                self.preview_sound(sound_id);
            }
            if let Some(sound_id) = drag_request
                && let Some(sound) = self
                    .sounds
                    .iter()
                    .find(|sound| sound.id == sound_id)
                    .cloned()
                && let Err(error) = self.drag_sound_file_out(ui.ctx(), &sound)
            {
                self.set_error_status(error);
            }
        });
    }

    pub(super) fn render_delete_folder_confirm_panel(&mut self, ctx: &Context) {
        let Some(folder_id) = self.show_delete_folder_confirm else {
            return;
        };

        let mut open_panel = true;
        let mut close_request = false;
        let mut delete_confirmed = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(380.0, 180.0), vec2(300.0, 160.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("delete-folder-confirm-panel"))
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
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe002, 20.0, Color32::from_rgb(220, 53, 69)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(
                    RichText::new(self.t("library.delete_confirm_title"))
                        .size(15.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );

                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.t("library.delete_confirm_warning"))
                        .size(12.0)
                        .color(Self::muted_text_color()),
                );

                ui.add_space(14.0);
                ui.horizontal_centered(|ui| {
                    let yes_response = ui.add_sized(
                        [130.0, 38.0],
                        Self::action_button(
                            RichText::new(self.t("library.delete_confirm_yes")).size(13.0),
                            false,
                            true,
                        ),
                    );
                    Self::decorate_button_response(ui, &yes_response);
                    if yes_response.clicked() {
                        delete_confirmed = true;
                    }

                    let no_response = ui.add_sized(
                        [130.0, 38.0],
                        Self::action_button(
                            RichText::new(self.t("library.delete_confirm_no")).size(13.0),
                            false,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &no_response);
                    if no_response.clicked() {
                        close_request = true;
                    }
                });
            });

        if close_request || !open_panel {
            self.show_delete_folder_confirm = None;
        }

        if delete_confirmed {
            self.show_delete_folder_confirm = None;
            self.delete_folder_branch(folder_id);
        }
    }
}
