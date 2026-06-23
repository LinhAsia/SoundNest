use super::*;
use eframe::egui::Grid;

impl SoundFxApp {
    pub(super) fn draw_folders_tree_view(&mut self, ui: &mut egui::Ui) {
        let clipboard_paste_ready = self.clipboard_has_supported_audio();
        let mut select_folder_id = None;
        let mut delete_folder_id = None;
        let mut rename_folder_id = None;
        let mut rename_commit = None;
        let mut finish_editing = false;
        let mut toggle_folder_id = None;
        let mut clear_selected_folder = false;

        if self.external_library_drop_active(ui.ctx())
            && self.library_drop_target_folder.is_none()
            && self.library_drop_target_root
        {
            self.draw_external_drop_preview_row(ui, 0, None);
            ui.add_space(8.0);
        }

        for folder in self.visible_child_folders(None) {
            self.draw_folder_tree_node(
                ui,
                &folder,
                0,
                &mut select_folder_id,
                &mut delete_folder_id,
                &mut rename_folder_id,
                &mut rename_commit,
                &mut finish_editing,
                &mut toggle_folder_id,
                &mut clear_selected_folder,
                clipboard_paste_ready,
            );
        }

        if self.library_tab == LibraryTab::Sounds {
            self.draw_root_inline_sounds(ui);
        }

        self.apply_folder_tree_actions(
            select_folder_id,
            delete_folder_id,
            rename_folder_id,
            rename_commit,
            finish_editing,
            toggle_folder_id,
            clear_selected_folder,
        );
    }

    fn draw_root_inline_sounds(&mut self, ui: &mut Ui) {
        let sounds = self.direct_sounds_for_folder(None);
        if sounds.is_empty() {
            return;
        }

        let visible_count = self.visible_folder_sound_count(None, sounds.len(), ui.ctx());
        ui.add_space(10.0);
        self.draw_folder_loading_hint(ui, visible_count, sounds.len());
        if self.library_sound_view == LibrarySoundView::Grid {
            let modal_open = self.has_modal_panel();
            let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
            self.draw_library_sound_grid_content(
                ui,
                &sounds[..visible_count],
                ui.available_width().max(180.0),
                modal_open,
                false,
                titlebar_drag_active,
            );
        } else {
            self.draw_inline_folder_sound_rows(ui, &sounds[..visible_count]);
        }
    }

    pub(super) fn draw_folders_grid_view(&mut self, ui: &mut egui::Ui) {
        let clipboard_paste_ready = self.clipboard_has_supported_audio();
        let mut select_folder_id = None;
        let mut delete_folder_id = None;
        let mut rename_folder_id = None;
        let mut rename_commit = None;
        let mut finish_editing = false;
        let mut toggle_folder_id = None;
        let mut clear_selected_folder = false;
        let spacing = 12.0;
        let columns = self.library_grid_columns.clamp(3, 8).min(4);
        let available_width = ui.available_width().max(360.0);
        let total_gap_width = spacing * (columns.saturating_sub(1)) as f32;
        let card_width = ((available_width - total_gap_width) / columns as f32).max(180.0);
        let visible_folders = self.visible_child_folders(self.library_current_folder);

        if !visible_folders.is_empty() {
            Grid::new("library-folder-grid")
                .num_columns(columns)
                .spacing(vec2(spacing, spacing))
                .min_col_width(card_width)
                .show(ui, |ui| {
                    for (index, folder) in visible_folders.iter().enumerate() {
                        self.draw_folder_grid_card(
                            ui,
                            folder,
                            0,
                            card_width,
                            &mut select_folder_id,
                            &mut delete_folder_id,
                            &mut rename_folder_id,
                            &mut rename_commit,
                            &mut finish_editing,
                            &mut toggle_folder_id,
                            &mut clear_selected_folder,
                            clipboard_paste_ready,
                        );
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        }

        let sounds = if self.library_tab == LibraryTab::Sounds {
            self.direct_sounds_for_folder(self.library_current_folder)
        } else {
            Vec::new()
        };

        if visible_folders.is_empty() && sounds.is_empty() {
            ui.add_space(8.0);
            ui.label(
                RichText::new(self.t("library.empty_folder"))
                    .size(13.0)
                    .color(Self::muted_text_color()),
            );
        }

        self.apply_folder_tree_actions(
            select_folder_id,
            delete_folder_id,
            rename_folder_id,
            rename_commit,
            finish_editing,
            toggle_folder_id,
            clear_selected_folder,
        );

        if self.library_tab == LibraryTab::Sounds && !sounds.is_empty() {
            ui.add_space(12.0);
            let visible_count = self.visible_folder_sound_count(
                self.library_current_folder,
                sounds.len(),
                ui.ctx(),
            );
            self.draw_folder_loading_hint(ui, visible_count, sounds.len());
            if self.library_sound_view == LibrarySoundView::Grid {
                let modal_open = self.has_modal_panel();
                let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
                self.draw_library_sound_grid_content(
                    ui,
                    &sounds[..visible_count],
                    available_width,
                    modal_open,
                    false,
                    titlebar_drag_active,
                );
            } else {
                self.draw_inline_folder_sound_rows(ui, &sounds[..visible_count]);
            }
        }
    }

    fn apply_folder_tree_actions(
        &mut self,
        select_folder_id: Option<Uuid>,
        delete_folder_id: Option<Uuid>,
        rename_folder_id: Option<Uuid>,
        rename_commit: Option<(Uuid, String)>,
        finish_editing: bool,
        toggle_folder_id: Option<Uuid>,
        clear_selected_folder: bool,
    ) {
        if clear_selected_folder {
            if let Some(folder_id) = self.library_current_folder {
                self.stop_library_preview_if_hidden_by_folder(folder_id);
            }
            self.library_current_folder = None;
            self.editing_folder_id = None;
        } else if let Some(folder_id) = select_folder_id {
            if self.library_search_active() {
                self.library_audio_query.clear();
            }
            self.library_current_folder = Some(folder_id);
            self.editing_folder_id = None;
        }
        if let Some(folder_id) = toggle_folder_id {
            if !self.library_collapsed_folders.insert(folder_id) {
                self.library_collapsed_folders.remove(&folder_id);
            } else {
                self.stop_library_preview_if_hidden_by_folder(folder_id);
            }
        }
        if let Some(folder_id) = delete_folder_id {
            if self.total_sound_count_for_folder(folder_id) > 0 {
                self.show_delete_folder_confirm = Some(folder_id);
            } else {
                self.delete_folder_branch(folder_id);
            }
        }
        if let Some(folder_id) = rename_folder_id {
            self.editing_folder_id = Some(folder_id);
            if let Some(folder) = self.folders.iter().find(|folder| folder.id == folder_id) {
                self.folder_rename_name = folder.name.clone();
            }
        }
        if let Some((folder_id, new_name)) = rename_commit
            && let Some(folder) = self
                .folders
                .iter_mut()
                .find(|folder| folder.id == folder_id)
        {
            folder.name = new_name;
            let _ = self.storage.save_folders(&self.folders);
        }
        if finish_editing {
            self.editing_folder_id = None;
        }
    }
}
