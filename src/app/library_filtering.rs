use super::*;

impl SoundFxApp {
    pub(super) fn library_search_active(&self) -> bool {
        !self.library_audio_query.trim().is_empty()
    }

    fn folder_name_matches_query(folder: &crate::storage::Folder, query: &str) -> bool {
        let query = query.trim();
        query.is_empty()
            || folder
                .name
                .to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
    }

    fn folder_matches_library_search(&self, folder_id: Uuid) -> bool {
        let query = self.library_audio_query.trim();
        if query.is_empty() {
            return true;
        }

        let Some(folder) = self.folders.iter().find(|folder| folder.id == folder_id) else {
            return false;
        };
        if Self::folder_name_matches_query(folder, query) {
            return true;
        }

        if !self.direct_sounds_for_folder(Some(folder_id)).is_empty() {
            return true;
        }

        self.folders
            .iter()
            .filter(|candidate| candidate.parent_id == Some(folder_id))
            .any(|child| self.folder_matches_library_search(child.id))
    }

    pub(super) fn visible_child_folders(
        &self,
        parent_id: Option<Uuid>,
    ) -> Vec<crate::storage::Folder> {
        let folders = self.sorted_child_folders(parent_id);
        if !self.library_search_active() {
            return folders;
        }
        folders
            .into_iter()
            .filter(|folder| self.folder_matches_library_search(folder.id))
            .collect()
    }

    pub(super) fn filtered_library_sounds(&self) -> Vec<SoundEffect> {
        if self.library_search_active() {
            self.filtered_library_sounds_for_folder(None, true)
        } else {
            self.filtered_library_sounds_for_folder(self.library_current_folder, true)
        }
    }

    pub(super) fn filtered_library_sound_indices(&self) -> Vec<usize> {
        if self.library_search_active() {
            self.filtered_library_sound_indices_for_folder(None, true)
        } else {
            self.filtered_library_sound_indices_for_folder(self.library_current_folder, true)
        }
    }

    pub(super) fn filtered_library_sound_indices_for_folder(
        &self,
        folder_id: Option<Uuid>,
        include_descendants: bool,
    ) -> Vec<usize> {
        let normalized_query = self.library_audio_query.trim().to_ascii_lowercase();
        let active_tag_filter = self
            .active_audio_tag_filter()
            .map(|value| value.to_ascii_lowercase());
        let cache_key = format!(
            "folder:{:?}|desc:{}|import:{:?}|favorites:{}|tag:{:?}|query:{}|len:{}",
            folder_id,
            include_descendants,
            self.folder_import_select_mode,
            self.library_favorites_only_audio,
            active_tag_filter,
            normalized_query,
            self.sounds.len()
        );
        if let Some(cached) = self
            .library_filtered_sound_indices_cache
            .borrow()
            .get(&cache_key)
            .cloned()
        {
            return cached;
        }

        let active_folder_ids = if include_descendants {
            folder_id.map(|root_id| self.folder_branch_ids(root_id))
        } else {
            folder_id.map(|root_id| HashSet::from([root_id]))
        };
        let mut favorites = Vec::new();
        let mut regular = Vec::new();

        for (index, sound) in self.sounds.iter().enumerate() {
            let folder_matches = if let Some(folder_id) = self.folder_import_select_mode {
                sound.folder_id != Some(folder_id)
            } else if let Some(folder_ids) = &active_folder_ids {
                sound
                    .folder_id
                    .is_some_and(|folder_id| folder_ids.contains(&folder_id))
            } else {
                true
            };
            if !folder_matches {
                continue;
            }
            if !normalized_query.is_empty()
                && !Self::library_query_matches(&sound.name, &normalized_query)
                && !sound
                    .tags
                    .iter()
                    .any(|tag| tag.to_ascii_lowercase().contains(&normalized_query))
            {
                continue;
            }
            if !Self::sound_tag_matches_filter(&sound.tags, active_tag_filter.as_deref()) {
                continue;
            }
            if self.library_favorites_only_audio && !sound.favorite {
                continue;
            }

            if sound.favorite {
                favorites.push(index);
            } else {
                regular.push(index);
            }
        }

        favorites.extend(regular);
        self.library_filtered_sound_indices_cache
            .borrow_mut()
            .insert(cache_key, favorites.clone());
        favorites
    }

    pub(super) fn filtered_library_sounds_for_folder(
        &self,
        folder_id: Option<Uuid>,
        include_descendants: bool,
    ) -> Vec<SoundEffect> {
        let active_folder_ids = if include_descendants {
            folder_id.map(|root_id| self.folder_branch_ids(root_id))
        } else {
            folder_id.map(|root_id| HashSet::from([root_id]))
        };
        let active_tag_filter = self.active_audio_tag_filter();
        let filtered = self
            .sounds
            .iter()
            .filter(|sound| {
                if let Some(folder_id) = self.folder_import_select_mode {
                    sound.folder_id != Some(folder_id)
                } else if let Some(folder_ids) = &active_folder_ids {
                    sound
                        .folder_id
                        .is_some_and(|folder_id| folder_ids.contains(&folder_id))
                } else {
                    true
                }
            })
            .filter(|sound| Self::library_sound_query_matches(sound, &self.library_audio_query))
            .filter(|sound| Self::sound_tag_matches_filter(&sound.tags, active_tag_filter))
            .filter(|sound| !self.library_favorites_only_audio || sound.favorite)
            .cloned()
            .collect::<Vec<_>>();
        let (favorites, regular): (Vec<_>, Vec<_>) =
            filtered.into_iter().partition(|sound| sound.favorite);
        favorites.into_iter().chain(regular).collect()
    }

    pub(super) fn direct_sounds_for_folder(&self, folder_id: Option<Uuid>) -> Vec<SoundEffect> {
        let active_tag_filter = self.active_audio_tag_filter();
        let filtered = self
            .sounds
            .iter()
            .filter(|sound| {
                if let Some(import_folder_id) = self.folder_import_select_mode {
                    sound.folder_id != Some(import_folder_id)
                } else {
                    sound.folder_id == folder_id
                }
            })
            .filter(|sound| Self::library_sound_query_matches(sound, &self.library_audio_query))
            .filter(|sound| Self::sound_tag_matches_filter(&sound.tags, active_tag_filter))
            .filter(|sound| !self.library_favorites_only_audio || sound.favorite)
            .cloned()
            .collect::<Vec<_>>();
        let (favorites, regular): (Vec<_>, Vec<_>) =
            filtered.into_iter().partition(|sound| sound.favorite);
        favorites.into_iter().chain(regular).collect()
    }

    pub(super) fn folder_branch_ids(&self, root_id: Uuid) -> HashSet<Uuid> {
        let mut ids = HashSet::from([root_id]);
        let mut stack = vec![root_id];
        while let Some(parent_id) = stack.pop() {
            for folder in self
                .folders
                .iter()
                .filter(|folder| folder.parent_id == Some(parent_id))
            {
                if ids.insert(folder.id) {
                    stack.push(folder.id);
                }
            }
        }
        ids
    }

    pub(super) fn sorted_child_folders(
        &self,
        parent_id: Option<Uuid>,
    ) -> Vec<crate::storage::Folder> {
        let mut folders = self
            .folders
            .iter()
            .filter(|folder| folder.parent_id == parent_id)
            .cloned()
            .collect::<Vec<_>>();
        folders.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        folders
    }

    pub(super) fn default_library_collapsed_folders(&self) -> HashSet<Uuid> {
        self.folders
            .iter()
            .filter(|folder| {
                self.folders
                    .iter()
                    .any(|candidate| candidate.parent_id == Some(folder.id))
            })
            .map(|folder| folder.id)
            .collect()
    }

    pub(super) fn reset_library_tree_state(&mut self) {
        self.library_current_folder = None;
        self.library_collapsed_folders = self.default_library_collapsed_folders();
        self.folder_import_select_mode = None;
        self.editing_folder_id = None;
        self.editing_from_folder = None;
        self.library_folder_create_open = false;
    }

    pub(super) fn folder_path_label(&self, folder_id: Uuid) -> String {
        let mut names = Vec::new();
        let mut current = Some(folder_id);
        while let Some(id) = current {
            let Some(folder) = self.folders.iter().find(|folder| folder.id == id) else {
                break;
            };
            names.push(folder.name.clone());
            current = folder.parent_id;
        }
        names.reverse();
        names.join(" / ")
    }

    pub(super) fn direct_sound_count_for_folder(&self, folder_id: Uuid) -> usize {
        self.sounds
            .iter()
            .filter(|sound| sound.folder_id == Some(folder_id))
            .count()
    }

    pub(super) fn stop_library_preview_if_hidden_by_folder(&mut self, folder_id: Uuid) {
        let Some(current_sound_id) = self
            .audio
            .as_ref()
            .and_then(|audio| audio.current_sound_id())
        else {
            return;
        };
        let hidden_branch_ids = self.folder_branch_ids(folder_id);
        let should_stop = self
            .sounds
            .iter()
            .find(|sound| sound.id == current_sound_id)
            .is_some_and(|sound| {
                sound
                    .folder_id
                    .is_some_and(|sound_folder_id| hidden_branch_ids.contains(&sound_folder_id))
            });
        if should_stop {
            self.stop_preview();
        }
    }

    pub(super) fn total_sound_count_for_folder(&self, folder_id: Uuid) -> usize {
        let ids = self.folder_branch_ids(folder_id);
        self.sounds
            .iter()
            .filter(|sound| {
                sound
                    .folder_id
                    .is_some_and(|sound_folder_id| ids.contains(&sound_folder_id))
            })
            .count()
    }

    pub(super) fn folder_drag_ghost_waveform(&self, folder_id: Uuid) -> Vec<f32> {
        let direct_count = self.direct_sound_count_for_folder(folder_id) as f32;
        let child_count = self
            .folders
            .iter()
            .filter(|folder| folder.parent_id == Some(folder_id))
            .count() as f32;
        let total_count = self.total_sound_count_for_folder(folder_id) as f32;
        let mut waveform = vec![
            (0.18 + (child_count / 12.0).min(0.82)).clamp(0.12, 1.0),
            (0.16 + (direct_count / 18.0).min(0.84)).clamp(0.12, 1.0),
            (0.14 + (total_count / 40.0).min(0.86)).clamp(0.12, 1.0),
            (0.12 + ((child_count + direct_count) / 28.0).min(0.88)).clamp(0.12, 1.0),
        ];
        waveform.extend_from_slice(&[
            0.32, 0.58, 0.46, 0.72, 0.52, 0.84, 0.48, 0.68, 0.36, 0.56, 0.42, 0.64,
        ]);
        waveform
    }

    pub(super) fn external_library_drop_active(&self, ctx: &Context) -> bool {
        self.app_view == AppView::Library
            && self.library_tab == LibraryTab::Sounds
            && !self.has_modal_panel()
            && ctx.input(|input| !input.raw.hovered_files.is_empty())
    }

    pub(super) fn resolve_library_drop_target_at(&self, pos: Pos2) -> Option<Option<Uuid>> {
        for (folder_id, rect) in self.library_drop_target_folder_rects.iter().rev() {
            if rect.contains(pos) {
                return Some(Some(*folder_id));
            }
        }
        if self
            .library_drop_target_root_rect
            .is_some_and(|rect| rect.contains(pos))
        {
            return Some(None);
        }
        None
    }

    pub(super) fn create_folder(&mut self, name: String, parent_id: Option<Uuid>) {
        let new_folder_id = self.create_folder_record(name, parent_id);
        let _ = self.storage.save_folders(&self.folders);
        self.library_current_folder = Some(new_folder_id);
        self.new_folder_name.clear();
        self.folder_name_warning = false;
    }

    pub(super) fn create_folder_record(&mut self, name: String, parent_id: Option<Uuid>) -> Uuid {
        let new_folder = crate::storage::Folder {
            id: Uuid::new_v4(),
            name,
            parent_id,
        };
        self.folders.push(new_folder.clone());
        if let Some(parent_id) = parent_id {
            self.library_collapsed_folders.remove(&parent_id);
        }
        new_folder.id
    }

    pub(super) fn delete_folder_branch(&mut self, root_id: Uuid) {
        let removed_ids = self.folder_branch_ids(root_id);
        self.folders
            .retain(|folder| !removed_ids.contains(&folder.id));
        self.library_collapsed_folders
            .retain(|folder_id| !removed_ids.contains(folder_id));
        let _ = self.storage.save_folders(&self.folders);
        self.sounds.retain(|sound| {
            !sound
                .folder_id
                .is_some_and(|folder_id| removed_ids.contains(&folder_id))
        });
        let _ = self.storage.save_library(&self.sounds);
        if self
            .library_current_folder
            .is_some_and(|folder_id| removed_ids.contains(&folder_id))
        {
            self.library_current_folder = None;
        }
        if self
            .folder_import_select_mode
            .is_some_and(|folder_id| removed_ids.contains(&folder_id))
        {
            self.folder_import_select_mode = None;
        }
        self.editing_folder_id = None;
    }

    pub(super) fn ensure_current_folder_exists(&mut self) {
        if self
            .library_current_folder
            .is_some_and(|folder_id| !self.folders.iter().any(|folder| folder.id == folder_id))
        {
            self.library_current_folder = None;
        }
    }

    pub(super) fn filtered_library_videos(&self) -> Vec<VideoAsset> {
        let filtered = self
            .video_assets
            .iter()
            .filter(|video| Self::library_query_matches(&video.name, &self.library_video_query))
            .filter(|video| !self.library_favorites_only_video || video.favorite)
            .cloned()
            .collect::<Vec<_>>();
        let (favorites, regular): (Vec<_>, Vec<_>) =
            filtered.into_iter().partition(|video| video.favorite);
        favorites.into_iter().chain(regular).collect()
    }
}
