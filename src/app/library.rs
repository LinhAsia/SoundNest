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
        let active_tag_filter = self.active_audio_tag_filter();
        let filtered = self
            .sounds
            .iter()
            .filter(|sound| Self::library_sound_query_matches(sound, &self.library_audio_query))
            .filter(|sound| Self::sound_tag_matches_filter(&sound.tags, active_tag_filter))
            .filter(|sound| !self.library_favorites_only_audio || sound.favorite)
            .cloned()
            .collect::<Vec<_>>();
        let (favorites, regular): (Vec<_>, Vec<_>) =
            filtered.into_iter().partition(|sound| sound.favorite);
        favorites.into_iter().chain(regular).collect()
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

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(
                RichText::new(self.t("library.tag_filter"))
                    .size(12.0)
                    .color(Self::muted_text_color()),
            );
            let active_filter = self.library_audio_tag_filter.clone();
            let all_active = active_filter.is_none();
            if Self::tag_chip_button(ui, &self.t("library.tag_all"), all_active).clicked() {
                self.library_audio_tag_filter = None;
            }

            for tag in tags {
                let active = active_filter.as_deref().is_some_and(|value| value == tag);
                if Self::tag_chip_button(ui, &tag, active).clicked() {
                    self.library_audio_tag_filter = if active { None } else { Some(tag) };
                }
            }
        });
    }

pub(super) fn draw_sound_tag_picker(ui: &mut Ui, tags: &[String], current_tags: &mut String) -> bool {
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
                                    let cursor_secs = self.preview_cursor_secs_for(&self.sounds[index]);
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
                        if self.selected == Some(pending_sound_id) {
                            pending_preview_to_play = Some((pending_sound_id, start_position_secs));
                        }
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
        } else if !self.audio_preload_inflight.is_empty() || self.pending_preview_after_preload.is_some() {
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
        let text_color = Self::strong_text_color();
        let muted_color = Self::muted_text_color();
        let border_color = Self::border_color();

        // 1. Header with inline folder creation
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(self.t("library.create_folder"))
                    .font(FontId::new(13.0, FontFamily::Proportional))
                    .color(text_color)
                    .strong(),
            );
            ui.add_space(8.0);
            
            let response = Frame::new()
                .fill(Self::input_fill())
                .stroke(Stroke::new(1.0, border_color))
                .corner_radius(12.0)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.add_sized(
                        [180.0, 20.0],
                        egui::TextEdit::singleline(&mut self.new_folder_name)
                            .frame(false)
                            .hint_text("Ten thu muc...")
                    )
                })
                .inner;

            ui.add_space(8.0);
            let create_btn = ui.add(
                Button::new(Self::icon(0xe145, 14.0, Color32::WHITE))
                    .fill(Color32::from_rgb(227, 82, 149))
                    .corner_radius(10.0)
            );
            Self::decorate_button_response(ui, &create_btn);

            if create_btn.clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                let name = self.new_folder_name.trim().to_owned();
                if !name.is_empty() {
                    let new_folder = crate::storage::Folder {
                        id: Uuid::new_v4(),
                        name,
                    };
                    self.folders.push(new_folder);
                    self.new_folder_name.clear();
                    let _ = self.storage.save_folders(&self.folders);
                }
            }
        });

        ui.add_space(16.0);

        // 2. Folders Grid/List
        if self.folders.is_empty() {
            // Large empty state button to create folder
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new(self.t("library.no_folders"))
                        .color(muted_color)
                        .size(13.5),
                );
                ui.add_space(16.0);
                
                let big_create_btn = ui.add(
                    Button::new(
                        egui::RichText::new(format!("+ {}", self.t("library.create_folder")))
                            .size(13.0)
                            .color(Color32::WHITE)
                    )
                    .fill(Color32::from_rgb(227, 82, 149))
                    .corner_radius(16.0)
                    .min_size(vec2(160.0, 36.0))
                );
                Self::decorate_button_response(ui, &big_create_btn);

                if big_create_btn.clicked() {
                    let default_name = "Thu muc moi".to_owned();
                    let new_folder = crate::storage::Folder {
                        id: Uuid::new_v4(),
                        name: default_name,
                    };
                    self.folders.push(new_folder);
                    let _ = self.storage.save_folders(&self.folders);
                }
            });
        } else {
            let layout_width = ui.clip_rect().width().min(ui.available_width());
            let columns = 3.max(self.library_grid_columns.saturating_sub(1)); // slightly larger cards than sounds
            let spacing = 16.0;
            let total_gap_width = spacing * (columns.saturating_sub(1)) as f32;
            let target_grid_width = (layout_width - 28.0).max(total_gap_width + 100.0);
            let card_width = ((target_grid_width - total_gap_width) / columns as f32).max(60.0);
            let card_height = 80.0;
            let side_padding = ((layout_width - (card_width * columns as f32 + total_gap_width)) * 0.5).max(0.0);

            let mut navigate_to = None;
            let mut delete_folder_id = None;
            let mut rename_folder_id = None;
            let mut rename_commit = None;
            let mut finish_editing = false;

            for row in self.folders.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.add_space(side_padding);
                    for (col_index, folder) in row.iter().enumerate() {
                        if col_index > 0 {
                            ui.add_space(spacing);
                        }

                        let is_editing = self.editing_folder_id == Some(folder.id);

                        Frame::new()
                            .fill(Self::surface_fill())
                            .stroke(Stroke::new(1.0, border_color))
                            .corner_radius(18.0)
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width(card_width);
                                ui.set_height(card_height);

                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        // Folder Icon (clickable to open)
                                        let icon_btn = ui.add(
                                            Button::new(Self::icon(0xe2c7, 22.0, Color32::from_rgb(227, 82, 149)))
                                                .fill(Color32::TRANSPARENT)
                                                .frame(false)
                                        );
                                        if icon_btn.clicked() {
                                            navigate_to = Some(folder.id);
                                        }
                                        
                                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                            // Delete Button
                                            let del_btn = ui.add(
                                                Button::new(Self::icon(0xe872, 12.0, muted_color))
                                                    .fill(Color32::TRANSPARENT)
                                                    .frame(false)
                                            );
                                            Self::decorate_button_response(ui, &del_btn);
                                            if del_btn.clicked() {
                                                delete_folder_id = Some(folder.id);
                                            }

                                            ui.add_space(4.0);

                                            // Rename Button
                                            if !is_editing {
                                                let edit_btn = ui.add(
                                                    Button::new(Self::icon(0xe254, 12.0, muted_color))
                                                        .fill(Color32::TRANSPARENT)
                                                        .frame(false)
                                                );
                                                Self::decorate_button_response(ui, &edit_btn);
                                                if edit_btn.clicked() {
                                                    rename_folder_id = Some(folder.id);
                                                }
                                            }
                                        });
                                    });

                                    ui.add_space(8.0);

                                    if is_editing {
                                        let rename_edit = ui.add_sized(
                                            [ui.available_width() - 8.0, 20.0],
                                            egui::TextEdit::singleline(&mut self.folder_rename_name)
                                        );
                                        if rename_edit.lost_focus() || (rename_edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                            let new_name = self.folder_rename_name.trim().to_owned();
                                            if !new_name.is_empty() {
                                                rename_commit = Some((folder.id, new_name));
                                            }
                                            finish_editing = true;
                                        }
                                    } else {
                                        // Folder Name button
                                        let name_btn = ui.add(
                                            Button::new(
                                                egui::RichText::new(&folder.name)
                                                    .color(text_color)
                                                    .strong()
                                            )
                                            .fill(Color32::TRANSPARENT)
                                            .frame(false)
                                        );
                                        if name_btn.clicked() {
                                            navigate_to = Some(folder.id);
                                        }
                                    }
                                });
                            });
                    }
                });
                ui.add_space(spacing);
            }

            if let Some(fid) = navigate_to {
                self.library_current_folder = Some(fid);
                self.editing_folder_id = None;
            }

            if let Some(fid) = delete_folder_id {
                self.folders.retain(|f| f.id != fid);
                let _ = self.storage.save_folders(&self.folders);
                for sound in &mut self.sounds {
                    if sound.folder_id == Some(fid) {
                        sound.folder_id = None;
                    }
                }
                let _ = self.storage.save_library(&self.sounds);
                if self.library_current_folder == Some(fid) {
                    self.library_current_folder = None;
                }
                self.editing_folder_id = None;
            }

            if let Some(fid) = rename_folder_id {
                self.editing_folder_id = Some(fid);
                if let Some(f) = self.folders.iter().find(|f| f.id == fid) {
                    self.folder_rename_name = f.name.clone();
                }
            }

            if let Some((fid, new_name)) = rename_commit {
                if let Some(f) = self.folders.iter_mut().find(|f| f.id == fid) {
                    f.name = new_name;
                    let _ = self.storage.save_folders(&self.folders);
                }
            }

            if finish_editing {
                self.editing_folder_id = None;
            }
        }
    }
}
