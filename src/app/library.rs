use super::*;
use crate::audio::{AudioEngine, calculate_normalization_gain};
use crate::downloader::{YoutubeAudioDownloader, YoutubeSearchResult};
use crate::gemini_tts;
use crate::hotkey::{GlobalHotkeyManager, Hotkey};
use crate::localization::Localization;
use crate::myinstants::{MyinstantsClient, MyinstantsResult};
use crate::pitch::{
    PitchInputSource, PitchMonitor, PitchMonitorConfig, PitchSnapshot, analyze_pitch_file,
    list_capture_devices,
};
use crate::platform;
use crate::record_video;
use crate::recorder::{Recorder, RecorderConfig};
use crate::storage::{GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset, format_time};
use crate::stream_input::{StreamInputConfig, StreamInputRouter};
use anyhow::{Context as _, Result};
#[cfg(windows)]
use clipboard_win::{Clipboard, Setter, formats::FileList};
use eframe::egui::{
    self, Align, Align2, Button, CentralPanel, Checkbox, Color32, ComboBox, Context, CornerRadius,
    DragValue, FontFamily, FontId, Frame, Margin, Pos2, ProgressBar, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextEdit, TextureHandle, Ui, Vec2, ViewportCommand, vec2,
};
use eframe::epaint::Shadow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

impl SoundFxApp {
    pub(super) fn filtered_library_sounds(&self) -> Vec<SoundEffect> {
        self.filtered_library_sounds_for_folder(self.library_current_folder, true)
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

    pub(super) fn create_folder(&mut self, name: String, parent_id: Option<Uuid>) {
        let new_folder = crate::storage::Folder {
            id: Uuid::new_v4(),
            name,
            parent_id,
        };
        self.folders.push(new_folder.clone());
        if let Some(parent_id) = parent_id {
            self.library_collapsed_folders.remove(&parent_id);
        }
        self.library_current_folder = Some(new_folder.id);
        self.new_folder_name.clear();
        self.folder_name_warning = false;
        let _ = self.storage.save_folders(&self.folders);
    }

    pub(super) fn delete_folder_branch(&mut self, root_id: Uuid) {
        let removed_ids = self.folder_branch_ids(root_id);
        self.folders
            .retain(|folder| !removed_ids.contains(&folder.id));
        self.library_collapsed_folders
            .retain(|folder_id| !removed_ids.contains(folder_id));
        let _ = self.storage.save_folders(&self.folders);
        for sound in &mut self.sounds {
            if sound
                .folder_id
                .is_some_and(|folder_id| removed_ids.contains(&folder_id))
            {
                sound.folder_id = None;
            }
        }
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

    pub(super) fn active_audio_tag_filter(&self) -> Option<&str> {
        if self.app_view == AppView::Library {
            self.library_audio_tag_filter.as_deref()
        } else {
            None
        }
    }

    pub(super) fn mark_sound_copied(&mut self, ctx: &Context, sound_id: Uuid) {
        self.copied_sound_feedback_until
            .insert(sound_id, ctx.input(|input| input.time) + 1.15);
    }

    pub(super) fn mark_video_copied(&mut self, ctx: &Context, video_id: Uuid) {
        self.copied_video_feedback_until
            .insert(video_id, ctx.input(|input| input.time) + 1.15);
    }

    pub(super) fn sound_copy_feedback_active(&self, ctx: &Context, sound_id: Uuid) -> bool {
        self.copied_sound_feedback_until
            .get(&sound_id)
            .is_some_and(|until| *until > ctx.input(|input| input.time))
    }

    pub(super) fn video_copy_feedback_active(&self, ctx: &Context, video_id: Uuid) -> bool {
        self.copied_video_feedback_until
            .get(&video_id)
            .is_some_and(|until| *until > ctx.input(|input| input.time))
    }

    pub(super) fn prune_copy_feedback(&mut self, ctx: &Context) {
        let now = ctx.input(|input| input.time);
        self.copied_sound_feedback_until
            .retain(|_, until| *until > now);
        self.copied_video_feedback_until
            .retain(|_, until| *until > now);
    }

    pub(super) fn toggle_sound_favorite(&mut self, sound_id: Uuid, ctx: &Context) {
        if let Some(sound) = self.sounds.iter_mut().find(|sound| sound.id == sound_id) {
            sound.favorite = !sound.favorite;
            self.mark_dirty(ctx);
        }
    }

    pub(super) fn toggle_video_favorite(&mut self, video_id: Uuid) {
        if let Some(video) = self
            .video_assets
            .iter_mut()
            .find(|video| video.id == video_id)
        {
            video.favorite = !video.favorite;
            let _ = self.storage.save_video_library(&self.video_assets);
        }
    }

    pub(super) fn favorite_button_sized(
        ui: &mut Ui,
        active: bool,
        size: [f32; 2],
        icon_size: f32,
    ) -> egui::Response {
        let fill = if active {
            Color32::from_rgb(247, 191, 64)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(29, 25, 35)
        } else {
            Color32::WHITE
        };
        let stroke = if active {
            Color32::from_rgb(247, 191, 64)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(83, 69, 92)
        } else {
            Color32::from_rgb(227, 217, 226)
        };
        let icon = if active { 0xe838 } else { 0xe83a };
        let icon_color = if active {
            Color32::from_rgb(60, 48, 12)
        } else {
            Self::strong_text_color()
        };
        let response = ui.add_sized(
            size,
            Button::new(Self::icon(icon, icon_size, icon_color))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(16.0),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn library_query_matches(name: &str, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        name.to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
    }

    pub(super) fn normalize_tag(tag: &str) -> Option<String> {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() { None } else { Some(tag) }
    }

    pub(super) fn parse_tags(text: &str) -> Vec<String> {
        let mut tags = Vec::new();
        let mut seen = HashSet::new();
        for raw_tag in text.split(|ch| matches!(ch, ',' | ';' | '\n')) {
            let Some(tag) = Self::normalize_tag(raw_tag) else {
                continue;
            };
            if seen.insert(tag.clone()) {
                tags.push(tag);
            }
        }
        tags
    }

    pub(super) fn join_tags(tags: &[String]) -> String {
        tags.join(", ")
    }

    pub(super) fn sound_tag_matches_filter(tags: &[String], filter: Option<&str>) -> bool {
        match filter {
            Some(filter) => tags.iter().any(|tag| tag.eq_ignore_ascii_case(filter)),
            None => true,
        }
    }

    pub(super) fn has_sound_tag(&self, tag: &str) -> bool {
        self.sounds.iter().any(|sound| {
            sound
                .tags
                .iter()
                .any(|sound_tag| sound_tag.eq_ignore_ascii_case(tag))
        })
    }

    pub(super) fn reconcile_library_audio_tag_filter(&mut self) {
        let Some(active_filter) = self.library_audio_tag_filter.as_deref() else {
            return;
        };
        if !self.has_sound_tag(active_filter) {
            self.library_audio_tag_filter = None;
        }
    }

    pub(super) fn library_sound_query_matches(sound: &SoundEffect, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        let query = query.to_ascii_lowercase();
        sound.name.to_ascii_lowercase().contains(&query)
            || sound
                .tags
                .iter()
                .any(|tag| tag.to_ascii_lowercase().contains(&query))
    }

    pub(super) fn distinct_sound_tags(&self) -> Vec<String> {
        let mut tags = self
            .sounds
            .iter()
            .flat_map(|sound| sound.tags.iter().cloned())
            .collect::<Vec<_>>();
        tags.sort_unstable_by_key(|tag| tag.to_ascii_lowercase());
        let mut deduped = Vec::new();
        let mut seen = HashSet::new();
        for tag in tags {
            let key = tag.to_ascii_lowercase();
            if seen.insert(key) {
                deduped.push(tag);
            }
        }
        deduped
    }

    pub(super) fn tag_chip_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
        let fill = if active {
            Color32::from_rgb(227, 82, 149)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgba_premultiplied(41, 34, 47, 224)
        } else {
            Color32::from_rgba_premultiplied(237, 231, 238, 198)
        };
        let stroke = if active {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(84, 69, 92)
        } else {
            Color32::from_rgb(221, 212, 222)
        };
        let text_color = if active {
            Color32::WHITE
        } else {
            Self::strong_text_color()
        };
        let response = ui.add(
            Button::new(RichText::new(label).size(11.5).color(text_color))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(999.0),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn apply_tag_to_input(input: &mut String, tag: &str, active: bool) {
        let mut tags = Self::parse_tags(input);
        let Some(normalized) = Self::normalize_tag(tag) else {
            return;
        };
        if active {
            tags.retain(|value| !value.eq_ignore_ascii_case(&normalized));
        } else if !tags
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&normalized))
        {
            tags.push(normalized);
        }
        *input = Self::join_tags(&tags);
    }

    pub(super) fn refresh_import_audio_entries(&mut self) {
        self.import_audio_entries.clear();
        if self.import_dir.as_os_str().is_empty() || !self.import_dir.exists() {
            return;
        }

        let Ok(entries) = fs::read_dir(&self.import_dir) else {
            return;
        };

        self.import_audio_entries = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| !Self::should_skip_import_path(path))
            .filter(|path| path.is_dir() || is_supported_audio(path))
            .collect();

        self.import_audio_entries.sort_by(|left, right| {
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| left.file_name().cmp(&right.file_name()))
        });
    }

    pub(super) fn should_skip_import_path(path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return true;
        };
        if name.starts_with('$') {
            return true;
        }
        if matches!(
            name,
            "System Volume Information" | "Recovery" | "Config.Msi" | "MSOCache"
        ) {
            return true;
        }
        #[cfg(windows)]
        if let Ok(metadata) = fs::metadata(path) {
            let attrs = metadata.file_attributes();
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
            if attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                return true;
            }
        }
        false
    }

    pub(super) fn poll_library_hydration_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.library_hydration_rx.try_recv() {
            match message {
                LibraryHydrationMessage::Ready(hydrated_sounds) => {
                    for hydrated in hydrated_sounds {
                        if let Some(existing) =
                            self.sounds.iter_mut().find(|sound| sound.id == hydrated.id)
                        {
                            if existing.waveform.is_empty() {
                                existing.waveform = hydrated.waveform.clone();
                            }
                            if existing.duration_secs <= 0.0 {
                                existing.duration_secs = hydrated.duration_secs;
                            }
                            if existing.trim_end_secs <= 0.0 {
                                existing.trim_end_secs = hydrated.trim_end_secs;
                            }
                            existing.clamp_trim();
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_transition_analysis_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.transition_analysis_rx.try_recv() {
            match message {
                TransitionAnalysisMessage::StartupReady {
                    waveform,
                    duration_sec,
                } => {
                    self.startup.sound_waveform = waveform;
                    self.startup.sound_duration_sec = duration_sec;
                    if self.startup.phase == TransitionPhase::Intro {
                        self.startup.duration_sec = duration_sec.max(DEFAULT_INTRO_DURATION_SEC);
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn import_downloaded_sound(&mut self, path: &Path, remove_source: bool) {
        self.import_paths(vec![path.to_path_buf()]);
        if remove_source {
            let _ = fs::remove_file(path);
        }
    }

    pub(super) fn draw_library_tag_filter_row(&mut self, ui: &mut Ui) {
        self.reconcile_library_audio_tag_filter();
        let tags = self.distinct_sound_tags();
        if tags.is_empty() {
            return;
        }

        let active_filter = self.library_audio_tag_filter.clone();
        let active_hidden_tag = active_filter
            .as_deref()
            .filter(|tag| {
                !self.library_audio_tags_expanded
                    && !tags
                        .iter()
                        .take(LIBRARY_TAG_COLLAPSED_COUNT)
                        .any(|value| value.eq_ignore_ascii_case(tag))
            })
            .map(str::to_owned);
        let mut visible_tags = if self.library_audio_tags_expanded {
            tags.clone()
        } else {
            tags.iter()
                .take(LIBRARY_TAG_COLLAPSED_COUNT)
                .cloned()
                .collect::<Vec<_>>()
        };
        if let Some(tag) = active_hidden_tag
            && !visible_tags
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&tag))
        {
            visible_tags.push(tag);
        }
        let has_hidden_tags = tags.len() > visible_tags.len();

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(
                RichText::new(self.t("library.tag_filter"))
                    .size(12.0)
                    .color(Self::muted_text_color()),
            );
            let all_active = active_filter.is_none();
            if Self::tag_chip_button(ui, &self.t("library.tag_all"), all_active).clicked() {
                self.library_audio_tag_filter = None;
            }

            for tag in visible_tags {
                let active = active_filter.as_deref().is_some_and(|value| value == tag);
                if Self::tag_chip_button(ui, &tag, active).clicked() {
                    self.library_audio_tag_filter = if active { None } else { Some(tag) };
                }
            }

            if has_hidden_tags || self.library_audio_tags_expanded {
                let toggle_label = if self.library_audio_tags_expanded {
                    format!(
                        "{} Less",
                        Self::icon(0xe5ce, 13.0, Self::strong_text_color()).text()
                    )
                } else {
                    format!(
                        "{} More",
                        Self::icon(0xe5cf, 13.0, Self::strong_text_color()).text()
                    )
                };
                if Self::tag_chip_button(ui, &toggle_label, false).clicked() {
                    self.library_audio_tags_expanded = !self.library_audio_tags_expanded;
                }
            }
        });
    }

    pub(super) fn draw_sound_tag_picker(
        ui: &mut Ui,
        tags: &[String],
        current_tags: &mut String,
    ) -> bool {
        if tags.is_empty() {
            return false;
        }

        let selected = Self::parse_tags(current_tags);
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for tag in tags {
                let active = selected.iter().any(|value| value.eq_ignore_ascii_case(tag));
                if Self::tag_chip_button(ui, tag, active).clicked() {
                    Self::apply_tag_to_input(current_tags, tag, active);
                    changed = true;
                }
            }
        });
        changed
    }

    pub(super) fn poll_myinstants_waveform_jobs(&mut self) {
        while let Ok(message) = self.myinstants_waveform_rx.try_recv() {
            match message {
                MyinstantsWaveformMessage::Ready {
                    audio_url,
                    path,
                    waveform,
                } => {
                    self.myinstants_waveform_jobs.remove(&audio_url);
                    self.myinstants_preview_files
                        .insert(audio_url.clone(), path);
                    self.myinstants_waveforms.insert(audio_url, waveform);
                }
                MyinstantsWaveformMessage::Failed { audio_url, error } => {
                    self.myinstants_waveform_jobs.remove(&audio_url);
                    if self.status.is_none() {
                        self.set_error_status(error);
                    }
                }
            }
        }
    }

    pub(super) fn poll_normalize_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        while let Ok(message) = self.normalize_rx.try_recv() {
            match message {
                NormalizeMessage::Finished { sound_id, result } => {
                    self.normalize_inflight.remove(&sound_id);
                    match result {
                        Ok(gain) => {
                            if let Some(index) =
                                self.sounds.iter().position(|sound| sound.id == sound_id)
                            {
                                self.sounds[index].volume = gain;
                                self.mark_dirty(ctx);
                                self.schedule_processed_export(sound_id);
                                if self
                                    .audio
                                    .as_ref()
                                    .is_some_and(|audio| audio.is_playing(sound_id))
                                {
                                    let cursor_secs =
                                        self.preview_cursor_secs_for(&self.sounds[index]);
                                    self.preview_sound_from_position(sound_id, Some(cursor_secs));
                                }
                                changed = true;
                            }
                        }
                        Err(error) => {
                            self.set_error_status(error);
                        }
                    }
                }
            }
        }

        if changed {
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_audio_preload_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        let mut pending_preview_to_play = None;
        while let Ok(message) = self.audio_preload_rx.try_recv() {
            match message {
                AudioPreloadMessage::Finished { asset_path, result } => {
                    let asset_path_for_match = asset_path.clone();
                    self.audio_preload_inflight.remove(&asset_path);
                    if let Ok((channels, sample_rate, samples)) = result
                        && let Some(audio) = self.audio.as_mut()
                    {
                        audio.insert_cached_audio(asset_path, channels, sample_rate, samples);
                        changed = true;
                    }
                    if let Some((pending_sound_id, start_position_secs)) =
                        self.pending_preview_after_preload
                        && self
                            .sounds
                            .iter()
                            .find(|sound| sound.id == pending_sound_id)
                            .is_some_and(|sound| {
                                self.preview_asset_path_for_sound(sound) == asset_path_for_match
                            })
                    {
                        self.pending_preview_after_preload = None;
                        pending_preview_to_play = Some((pending_sound_id, start_position_secs));
                    }
                }
            }
        }

        if let Some((sound_id, start_position_secs)) = pending_preview_to_play {
            self.preview_sound_from_position(sound_id, start_position_secs);
            return;
        }

        if changed {
            ctx.request_repaint();
        } else if !self.audio_preload_inflight.is_empty()
            || self.pending_preview_after_preload.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
    }

    pub(super) fn render_import_panel(&mut self, ctx: &Context) {
        if !self.show_import_panel {
            return;
        }

        let mut open_panel = self.show_import_panel;
        let mut go_up = false;
        let mut next_dir = None;
        let mut import_file = None;
        let mut close_request = false;
        let mut switch_root = None;
        let roots = Self::available_import_roots();
        let browsing_root = self.import_dir.as_os_str().is_empty() || !self.import_dir.exists();
        let path_label = if browsing_root {
            "Drive".to_owned()
        } else {
            Self::truncate_middle_ascii(self.import_dir.to_string_lossy().as_ref(), 42)
        };
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(640.0, 560.0), vec2(320.0, 280.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("sound-import-panel"))
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
                    ui.label(Self::icon(0xe2c8, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if Self::icon_action(ui, [42.0, 34.0], 0xe2c8, browsing_root, false).clicked() {
                        self.set_import_dir(None);
                    }
                    if !browsing_root
                        && Self::icon_action(ui, [42.0, 34.0], 0xe5d8, false, false).clicked()
                    {
                        go_up = true;
                    }
                    for root in &roots {
                        let label = root.to_string_lossy();
                        let active = !browsing_root && self.import_dir.starts_with(root);
                        let response = ui.add_sized(
                            [74.0, 38.0],
                            Self::action_button(
                                RichText::new(label.as_ref()).size(14.0),
                                active,
                                active,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            switch_root = Some(root.clone());
                        }
                    }
                    ui.add_space(6.0);
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.add_sized(
                                [ui.available_width().max(120.0), 18.0],
                                egui::Label::new(
                                    RichText::new(path_label.as_str())
                                        .size(13.0)
                                        .color(Self::muted_text_color()),
                                )
                                .truncate(),
                            );
                        });
                });

                ui.add_space(14.0);

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if browsing_root {
                            for root in &roots {
                                let label = root.to_string_lossy();
                                let row = Frame::new()
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(22.0)
                                    .inner_margin(Margin::symmetric(16, 14))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(Self::icon(
                                                0xe2c8,
                                                18.0,
                                                Color32::from_rgb(214, 51, 132),
                                            ));
                                            ui.add_space(10.0);
                                            ui.add_sized(
                                                [ui.available_width(), 18.0],
                                                egui::Label::new(
                                                    RichText::new(label.as_ref())
                                                        .size(14.0)
                                                        .color(Self::strong_text_color()),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                                let response = ui.interact(
                                    row.response.rect,
                                    ui.id().with(root),
                                    Sense::click(),
                                );
                                if response.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if response.clicked() {
                                    switch_root = Some(root.clone());
                                }
                                ui.add_space(10.0);
                            }
                            return;
                        }

                        for path in &self.import_audio_entries {
                            let is_dir = path.is_dir();
                            let label = path
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or(if is_dir { "folder" } else { "sound" });
                            let row = Frame::new()
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(22.0)
                                .inner_margin(Margin::symmetric(16, 14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(Self::icon(
                                            if is_dir { 0xe2c8 } else { 0xeb82 },
                                            18.0,
                                            Color32::from_rgb(214, 51, 132),
                                        ));
                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [ui.available_width(), 18.0],
                                            egui::Label::new(
                                                RichText::new(label)
                                                    .size(14.0)
                                                    .color(Self::strong_text_color()),
                                            )
                                            .truncate(),
                                        );
                                    });
                                });
                            let response =
                                ui.interact(row.response.rect, ui.id().with(&path), Sense::click());
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if response.clicked() {
                                if is_dir {
                                    next_dir = Some(path.clone());
                                } else {
                                    import_file = Some(path.clone());
                                }
                            }
                            ui.add_space(10.0);
                        }
                    });
            });

        if close_request {
            open_panel = false;
        }
        self.show_import_panel = open_panel;
        if go_up {
            if let Some(parent) = self.import_dir.parent() {
                self.set_import_dir(Some(parent.to_path_buf()));
            }
        }
        if let Some(root) = switch_root {
            self.set_import_dir(Some(root));
        }
        if let Some(dir) = next_dir {
            self.set_import_dir(Some(dir));
        }
        if let Some(path) = import_file {
            self.import_paths(vec![path]);
            self.show_import_panel = false;
        }
    }

    pub(super) fn draw_folders_list_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_current_folder_exists();
        let selected_parent_label = self
            .library_current_folder
            .map(|folder_id| self.folder_path_label(folder_id))
            .unwrap_or_else(|| "Root".to_owned());

        let add_folder_btn = ui.add_sized(
            [160.0, 32.0],
            Button::new(
                RichText::new(if self.library_folder_create_open {
                    "Hide Add Folder"
                } else {
                    "+ Add Folder"
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

        if self.library_folder_create_open {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Create folder in:")
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
                        Self::action_button(RichText::new("To root").size(12.0), false, false),
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

        let mut select_folder_id = None;
        let mut delete_folder_id = None;
        let mut rename_folder_id = None;
        let mut rename_commit = None;
        let mut finish_editing = false;
        let mut toggle_folder_id = None;

        for folder in self.sorted_child_folders(None) {
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
            );
        }

        if let Some(folder_id) = select_folder_id {
            self.library_current_folder = Some(folder_id);
            self.editing_folder_id = None;
        }
        if let Some(folder_id) = toggle_folder_id {
            if !self.library_collapsed_folders.insert(folder_id) {
                self.library_collapsed_folders.remove(&folder_id);
            }
        }
        if let Some(folder_id) = delete_folder_id {
            self.delete_folder_branch(folder_id);
        }
        if let Some(folder_id) = rename_folder_id {
            self.editing_folder_id = Some(folder_id);
            if let Some(folder) = self.folders.iter().find(|folder| folder.id == folder_id) {
                self.folder_rename_name = folder.name.clone();
            }
        }
        if let Some((folder_id, new_name)) = rename_commit {
            if let Some(folder) = self
                .folders
                .iter_mut()
                .find(|folder| folder.id == folder_id)
            {
                folder.name = new_name;
                let _ = self.storage.save_folders(&self.folders);
            }
        }
        if finish_editing {
            self.editing_folder_id = None;
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
    ) {
        let is_selected = self.library_current_folder == Some(folder.id);
        let is_editing = self.editing_folder_id == Some(folder.id);
        let total_count = self.total_sound_count_for_folder(folder.id);
        let direct_count = self.direct_sound_count_for_folder(folder.id);
        let children = self.sorted_child_folders(Some(folder.id));
        let has_children = !children.is_empty();
        let is_collapsed = has_children && self.library_collapsed_folders.contains(&folder.id);
        let indent = 18.0 * depth as f32;
        let fill = if is_selected {
            if Self::dark_theme_enabled() {
                Color32::from_rgb(58, 29, 50)
            } else {
                Color32::from_rgb(255, 239, 247)
            }
        } else {
            Self::surface_fill()
        };
        let stroke = if is_selected {
            Color32::from_rgb(227, 82, 149)
        } else {
            Self::border_color()
        };

        let row = Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(16.0)
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                let mut delete_btn_response = None;
                let mut rename_btn_response = None;
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.label(Self::icon(
                        if has_children {
                            if is_collapsed { 0xe5cc } else { 0xe5cf }
                        } else {
                            0xe2c8
                        },
                        18.0,
                        Color32::from_rgb(227, 82, 149),
                    ));
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        if is_editing {
                            let response = ui.add_sized(
                                [ui.available_width().max(120.0), 20.0],
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
                        ui.label(
                            RichText::new(if direct_count == total_count {
                                format!("{total_count} sounds")
                            } else {
                                format!("{direct_count} direct / {total_count} total")
                            })
                            .size(11.0)
                            .color(Self::muted_text_color()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let delete_btn = ui.add(
                            Button::new(Self::icon(0xe872, 12.0, Self::muted_text_color()))
                                .fill(Color32::TRANSPARENT)
                                .frame(false),
                        );
                        Self::decorate_button_response(ui, &delete_btn);
                        if delete_btn.clicked() {
                            *delete_folder_id = Some(folder.id);
                        }
                        delete_btn_response = Some(delete_btn);
                        if !is_editing {
                            let rename_btn = ui.add(
                                Button::new(Self::icon(0xe254, 12.0, Self::muted_text_color()))
                                    .fill(Color32::TRANSPARENT)
                                    .frame(false),
                            );
                            Self::decorate_button_response(ui, &rename_btn);
                            if rename_btn.clicked() {
                                *rename_folder_id = Some(folder.id);
                            }
                            rename_btn_response = Some(rename_btn);
                        }
                    });
                });

                (delete_btn_response, rename_btn_response)
            });

        if !is_editing {
            let open_rect = Rect::from_min_max(
                row.response.rect.min,
                Pos2::new(
                    (row.response.rect.max.x - 84.0).max(row.response.rect.min.x),
                    row.response.rect.max.y,
                ),
            );
            let response = ui.interact(open_rect, ui.id().with(folder.id), Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let delete_hovered = row.inner.0.as_ref().is_some_and(|value| value.hovered());
            let rename_hovered = row.inner.1.as_ref().is_some_and(|value| value.hovered());
            if response.clicked() && !delete_hovered && !rename_hovered {
                *select_folder_id = Some(folder.id);
                if has_children {
                    *toggle_folder_id = Some(folder.id);
                }
            }
        }

        if is_selected && self.library_tab == LibraryTab::Sounds && !is_collapsed {
            ui.add_space(8.0);
            self.draw_inline_folder_sounds(ui, folder, (indent + 8.0).min(24.0));
        }

        if !is_collapsed {
            ui.add_space(8.0);
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
                );
            }
        }
    }

    fn draw_inline_folder_sounds(
        &mut self,
        ui: &mut Ui,
        folder: &crate::storage::Folder,
        left_indent: f32,
    ) {
        ui.add_space(6.0);
        let panel_width = (ui.available_width() - left_indent).max(180.0);
        ui.horizontal(|ui| {
            ui.add_space(left_indent);
            ui.allocate_ui_with_layout(
                vec2(panel_width, 0.0),
                egui::Layout::top_down(Align::Min),
                |ui| {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(panel_width.max(ui.available_width()));
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(self.folder_path_label(folder.id))
                                        .size(12.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                if self.folder_import_select_mode.is_none() {
                                    ui.add_space(10.0);
                                    let import_btn = ui.add(
                                        Button::new(format!(
                                            "+ {}",
                                            self.t("library.import_sound_to_folder")
                                        ))
                                        .fill(Color32::from_rgb(227, 82, 149))
                                        .corner_radius(10.0),
                                    );
                                    Self::decorate_button_response(ui, &import_btn);
                                    if import_btn.clicked() {
                                        self.folder_import_select_mode = Some(folder.id);
                                    }
                                }
                            });
                            ui.add_space(10.0);

                            let sounds =
                                self.filtered_library_sounds_for_folder(Some(folder.id), false);
                            if sounds.is_empty() {
                                ui.label(
                                    RichText::new("No sounds in this folder yet.")
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                                return;
                            }

                            for (index, sound) in sounds.iter().enumerate() {
                                self.draw_inline_folder_sound_row(ui, sound);
                                if index + 1 < sounds.len() {
                                    ui.add_space(8.0);
                                }
                            }
                        });
                },
            );
        });
    }

    fn draw_inline_folder_sound_row(&mut self, ui: &mut Ui, sound: &SoundEffect) {
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

        let row = Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(16.0)
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&sound.name)
                                    .size(12.8)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .truncate(),
                        );
                        ui.add_space(6.0);
                        let waveform_samples = self.sound_waveform_samples(sound);
                        let waveform_preview = Self::library_sound_waveform_preview_from_samples(
                            sound,
                            &waveform_samples,
                            48,
                        );
                        Self::draw_wave_strip(
                            ui,
                            &waveform_preview,
                            None,
                            Color32::from_rgb(214, 51, 132),
                            Color32::from_rgb(238, 213, 227),
                            Self::panel_fill(),
                            34.0,
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if self.folder_import_select_mode.is_none() {
                            let remove_btn = ui.add(
                                Button::new(Self::icon(0xe5cd, 12.0, Self::muted_text_color()))
                                    .fill(Color32::TRANSPARENT)
                                    .frame(false),
                            );
                            Self::decorate_button_response(ui, &remove_btn);
                            if remove_btn.clicked() {
                                remove_sound_from_folder = Some(sound.id);
                                remove_clicked = true;
                            }
                            remove_response = Some(remove_btn);
                        }
                        let copy_btn = Self::icon_action(
                            ui,
                            [36.0, 30.0],
                            0xe14d,
                            self.sound_copy_feedback_active(ui.ctx(), sound.id),
                            self.sound_copy_feedback_active(ui.ctx(), sound.id),
                        );
                        if copy_btn.clicked() {
                            copy_sound = Some(sound.id);
                            copy_clicked = true;
                        }
                        copy_response = Some(copy_btn);
                        let is_loading = self
                            .pending_preview_after_preload
                            .is_some_and(|(id, _)| id == sound.id);
                        let play_btn = Self::icon_action(
                            ui,
                            [36.0, 30.0],
                            if is_loading { 0xe5d5 } else { 0xe037 },
                            is_loading,
                            false,
                        );
                        if play_btn.clicked() {
                            if is_loading {
                                self.stop_preview();
                            } else {
                                preview_sound = Some(sound.id);
                            }
                            preview_clicked = true;
                        }
                        play_response = Some(play_btn);
                        let favorite_btn =
                            Self::favorite_button_sized(ui, sound.favorite, [36.0, 30.0], 16.0);
                        if favorite_btn.clicked() {
                            favorite_sound = Some(sound.id);
                            favorite_clicked = true;
                        }
                        favorite_response = Some(favorite_btn);
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(format_time(sound.trimmed_length()))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                        );
                    });
                });
            });

        let open_rect = Rect::from_min_max(
            row.response.rect.min,
            Pos2::new(
                (row.response.rect.max.x - 176.0).max(row.response.rect.min.x),
                row.response.rect.max.y,
            ),
        );
        let response = ui.interact(
            open_rect,
            ui.id().with(("folder-sound-row", sound.id)),
            Sense::click(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let over_action = favorite_response
            .as_ref()
            .is_some_and(|value| value.hovered())
            || play_response.as_ref().is_some_and(|value| value.hovered())
            || copy_response.as_ref().is_some_and(|value| value.hovered())
            || remove_response
                .as_ref()
                .is_some_and(|value| value.hovered());
        let action_clicked = preview_clicked || copy_clicked || favorite_clicked || remove_clicked;
        if response.clicked() && !over_action && !action_clicked {
            open_sound = true;
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
                    self.status = Some("Copied to clipboard".to_owned());
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
    }
}
