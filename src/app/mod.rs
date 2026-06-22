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
use crate::storage::{
    Folder, GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset, format_time,
};
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
const AUDIO_FILTERS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];
const APP_FRAME_RADIUS: f32 = 30.0;
const APP_OUTER_MARGIN: f32 = 0.0;
const LIVE_UI_FADE_SEC: f32 = 0.32;
const TRANSITION_POINT_COUNT: usize = 240;
const MATERIAL_ICONS_FONT: &str = "material_icons";
const ACTIVE_UI_REPAINT_MS: u64 = 8;
const JOB_POLL_REPAINT_MS: u64 = 90;
const DEFAULT_INTRO_DURATION_SEC: f32 = 1.35;
const DEFAULT_OUTRO_DURATION_SEC: f32 = 0.72;
const DEFAULT_OUTRO_PLAYBACK_DURATION_SEC: f32 = 2.0;
const DEFAULT_OUTRO_FADE_START_RATIO: f32 = 0.0;
const TRANSITION_WAVE_BUCKETS: usize = 160;
const LIBRARY_GRID_MIN_COLUMNS: usize = 3;
const LIBRARY_GRID_MAX_COLUMNS: usize = 8;
const LIBRARY_ROW_MIN_THICKNESS: usize = 1;
const LIBRARY_ROW_MAX_THICKNESS: usize = 5;
const RECORD_EXPORT_VIDEO_FPS_OPTIONS: [u32; 3] = [
    record_video::LOW_VIDEO_FPS,
    record_video::STANDARD_VIDEO_FPS,
    record_video::HIGH_VIDEO_FPS,
];
static DARK_THEME_ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) use message::*;
pub(crate) use state::*;

impl SoundFxApp {
    fn library_list_row_height(&self) -> f32 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 108.0,
            2 => 116.0,
            3 => 124.0,
            4 => 134.0,
            _ => 144.0,
        }
    }

    fn library_row_wave_height(&self) -> f32 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 22.0,
            2 => 26.0,
            3 => 30.0,
            4 => 36.0,
            _ => 42.0,
        }
    }

    fn library_row_vertical_padding(&self) -> i8 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 4,
            2 => 6,
            3 => 8,
            4 => 10,
            _ => 12,
        }
    }
}

impl SoundFxApp {
    fn with_initial_selection(mut self) -> Self {
        self.selected = self.sounds.first().map(|sound| sound.id);
        self.preload_selected_sound_audio();
        let _ = self.record_hotkey_manager.set_hotkeys(&self.record_hotkeys);
        let _ = self
            .record_hotkey_manager
            .set_secondary_hotkeys(&self.pitch_hotkeys);
        self
    }

    fn begin_async_transition_analysis(&mut self, startup_sound_path: Option<PathBuf>) {
        let Some(path) = startup_sound_path else {
            return;
        };
        let tx = self.transition_analysis_tx.clone();
        thread::spawn(move || {
            let result = Storage::new()
                .and_then(|storage| storage.analyze_audio_preview(&path, TRANSITION_WAVE_BUCKETS))
                .map(
                    |(waveform, duration_sec)| TransitionAnalysisMessage::StartupReady {
                        waveform,
                        duration_sec,
                    },
                );
            if let Ok(message) = result {
                let _ = tx.send(message);
            }
        });
    }

    fn begin_async_library_hydration(&mut self) {
        if self
            .sounds
            .iter()
            .all(|sound| !sound.waveform.is_empty() && sound.duration_secs > 0.0)
        {
            return;
        }

        let tx = self.library_hydration_tx.clone();
        let mut sounds = self.sounds.clone();
        thread::spawn(move || {
            let result = Storage::new().and_then(|storage| {
                let mut changed = false;
                for sound in &mut sounds {
                    if storage.hydrate_sound(sound)? {
                        changed = true;
                    }
                }
                if changed {
                    let _ = storage.save_library(&sounds);
                }
                Ok::<_, anyhow::Error>(sounds)
            });
            if let Ok(sounds) = result {
                let _ = tx.send(LibraryHydrationMessage::Ready(sounds));
            }
        });
    }

    fn set_error_status(&mut self, error: impl ToString) {
        self.status = Some(error.to_string());
    }

    fn selected_sound_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.sounds.iter().position(|sound| sound.id == selected)
    }

    fn is_transition_active(&self) -> bool {
        self.startup.phase != TransitionPhase::Live
    }

    fn center_window_if_needed(&mut self, ctx: &Context) {
        if !self.center_window_next_frame {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Self::desired_window_size()));
        if let Some(center_cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(center_cmd);
        }
        self.center_window_next_frame = false;
    }

    fn desired_window_size() -> Vec2 {
        vec2(1500.0, 920.0)
    }

    fn popup_safe_rect(&self, ctx: &Context) -> Rect {
        if self.overlay_only_mode {
            ctx.screen_rect().shrink(4.0)
        } else {
            self.modal_safe_rect(ctx)
        }
    }

    fn centered_overlay_pos(&self, ctx: &Context, size: Vec2) -> Pos2 {
        let rect = self.popup_safe_rect(ctx);
        let center = rect.center();
        Pos2::new(
            (center.x - size.x * 0.5).round(),
            (center.y - size.y * 0.5).round(),
        )
    }

    fn clamp_overlay_pos(&self, ctx: &Context, size: Vec2, pos: Pos2) -> Pos2 {
        let rect = self.popup_safe_rect(ctx);
        let max_x = (rect.right() - size.x).max(rect.left());
        let max_y = (rect.bottom() - size.y).max(rect.top());
        Pos2::new(
            pos.x.round().clamp(rect.left(), max_x),
            pos.y.round().clamp(rect.top(), max_y),
        )
    }

    fn update_overlay_drag_position(
        ctx: &Context,
        rect: Rect,
        response: &egui::Response,
        size: Vec2,
        position: &mut Option<Pos2>,
    ) {
        let drag_origin_id = response.id.with("overlay-drag-origin");
        if response.drag_started() || response.clicked() {
            ctx.data_mut(|data| {
                data.insert_temp(drag_origin_id, position.unwrap_or_default());
            });
        }
        if (response.dragged()
            || (response.is_pointer_button_down_on() && response.drag_delta().length_sq() > 0.0))
            && let Some(origin) = ctx.data(|data| data.get_temp::<Pos2>(drag_origin_id))
        {
            let max_x = (rect.right() - size.x).max(rect.left());
            let max_y = (rect.bottom() - size.y).max(rect.top());
            let next = origin + response.drag_delta();
            *position = Some(Pos2::new(
                next.x.round().clamp(rect.left(), max_x),
                next.y.round().clamp(rect.top(), max_y),
            ));
        }
        if !ctx.input(|input| input.pointer.primary_down()) {
            ctx.data_mut(|data| {
                data.remove::<Pos2>(drag_origin_id);
            });
        }
    }

    fn enforce_square_window_if_needed(&mut self, _ctx: &Context) {}

    fn custom_transition_duration_secs(
        storage: &Storage,
        sound_path: &Path,
        fallback_secs: f32,
    ) -> f32 {
        if !sound_path.exists() {
            return fallback_secs;
        }

        storage
            .audio_duration_secs(sound_path)
            .unwrap_or(fallback_secs)
            .max(0.05)
    }

    fn custom_transition_duration_secs_opt(
        storage: &Storage,
        sound_path: Option<&Path>,
        fallback_secs: f32,
    ) -> f32 {
        match sound_path {
            Some(path) => Self::custom_transition_duration_secs(storage, path, fallback_secs),
            None => fallback_secs,
        }
    }

    fn load_transition_sound_visual(
        storage: &Storage,
        sound_path: Option<PathBuf>,
    ) -> (Vec<f32>, f32) {
        let Some(sound_path) = sound_path else {
            return (Vec::new(), 0.0);
        };

        storage
            .analyze_audio_preview(&sound_path, TRANSITION_WAVE_BUCKETS)
            .unwrap_or_else(|_| (Vec::new(), 0.0))
    }

    fn outro_transition_visual(&self) -> Option<(Vec<f32>, f32)> {
        let audio = self.audio.as_ref()?;
        if !audio.has_active_playback() {
            return None;
        }
        if let Some(sound_id) = audio.current_sound_id() {
            let sound = self.sounds.iter().find(|sound| sound.id == sound_id)?;
            let waveform = if sound.waveform.is_empty() {
                Self::load_transition_sound_visual(
                    &self.storage,
                    Some(sound.playback_asset_path(self.storage.root_dir())),
                )
                .0
            } else {
                sound.waveform.clone()
            };
            return Some((waveform, DEFAULT_OUTRO_PLAYBACK_DURATION_SEC));
        }

        if let Some(path) = audio.current_file_path() {
            let (waveform, _) = Self::load_transition_sound_visual(&self.storage, Some(path));
            return Some((waveform, DEFAULT_OUTRO_PLAYBACK_DURATION_SEC));
        }

        Some((Vec::new(), DEFAULT_OUTRO_PLAYBACK_DURATION_SEC))
    }

    fn update_outro_audio_fade(&mut self, progress: f32) {
        if let Some(audio) = self.audio.as_mut() {
            let fade_start = DEFAULT_OUTRO_FADE_START_RATIO.clamp(0.0, 1.0);
            let fade_progress = ((progress - fade_start) / (1.0 - fade_start)).clamp(0.0, 1.0);
            let volume = 1.0 - Self::ease_in_out_cubic(fade_progress);
            audio.set_volume(volume);
        }
    }

    fn request_close(&mut self, ctx: &Context) {
        if !self.app_transition_animation {
            self.startup.close_sent = true;
            if let Some(audio) = self.audio.as_mut() {
                audio.stop();
            }
            if self.recorder.snapshot().running {
                self.stop_recording_for_close(ctx);
            }
            if self.pending_save {
                let _ = self.storage.save_library(&self.sounds);
                self.pending_save = false;
            }
            self.finalize_close_cleanup(ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let Some((sound_waveform, sound_duration_sec)) = self.outro_transition_visual() else {
            self.startup.close_sent = true;
            if let Some(audio) = self.audio.as_mut() {
                audio.stop();
            }
            if self.recorder.snapshot().running {
                self.stop_recording_for_close(ctx);
            }
            if self.pending_save {
                let _ = self.storage.save_library(&self.sounds);
                self.pending_save = false;
            }
            self.finalize_close_cleanup(ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };

        if self.startup.phase == TransitionPhase::Outro {
            return;
        }

        if self.recorder.snapshot().running {
            self.stop_recording_for_close(ctx);
        }
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }

        self.startup.phase = TransitionPhase::Outro;
        self.startup.started_at = None;
        self.startup.live_started_at = None;
        self.startup.duration_sec = DEFAULT_OUTRO_PLAYBACK_DURATION_SEC;
        self.startup.sound_waveform = sound_waveform;
        self.startup.sound_duration_sec = sound_duration_sec;
        self.update_outro_audio_fade(0.0);
        ctx.request_repaint();
    }

    fn stop_recording_for_close(&mut self, _ctx: &Context) {
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        if let Some(path) = self.recorder.take_completed_path() {
            let _ = fs::remove_file(path);
        }
    }

    fn finalize_close_cleanup(&mut self, _ctx: &Context) {
        self.show_download_panel = false;
        self.close_recording_review(true);
        self.video_viewer = None;
        self.pitch_monitor.stop();
        self.pitch_overlay_native_visuals_applied = false;
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
    }

    fn play_file_if_exists(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.play_file(path)?;
        }
        Ok(())
    }

    fn add_sound(&mut self) {
        self.refresh_import_audio_entries();
        self.show_import_panel = true;
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>) {
        self.library_current_folder = None;
        self.folder_import_select_mode = None;
        if let Err(error) = self.import_paths_to_folder(paths, None) {
            self.set_error_status(error);
        }
    }

    fn begin_import_paths_to_folder(
        &mut self,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
    ) {
        if paths.is_empty() {
            return;
        }
        if self.library_import_job.is_some() {
            self.set_error_status("Another import is already running");
            return;
        }

        let total = paths
            .iter()
            .map(|path| Self::count_importable_audio_files(path))
            .sum::<usize>();
        let job_id = Uuid::new_v4();
        self.library_import_job = Some(ActiveLibraryImport {
            job_id,
            target_folder_id,
            completed: 0,
            total,
            current_label: "Preparing import...".to_owned(),
        });
        self.status = Some(if total > 0 {
            format!("Importing {total} sound(s)...")
        } else {
            "Scanning dropped files...".to_owned()
        });

        let tx = self.library_import_tx.clone();
        let root_dir = self.storage.root_dir().to_path_buf();
        thread::spawn(move || {
            let result =
                Self::run_library_import_job(root_dir, paths, target_folder_id, &tx, job_id)
                    .map_err(|error| error.to_string());
            let _ = tx.send(LibraryImportMessage::Finished { job_id, result });
        });
    }

    fn import_paths_to_folder(
        &mut self,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
    ) -> Result<usize> {
        let mut imported = Vec::new();
        let mut ignored = 0usize;
        let mut folders_changed = false;

        for path in paths {
            self.import_path_entry(
                &path,
                target_folder_id,
                &mut imported,
                &mut ignored,
                &mut folders_changed,
            )?;
        }

        if imported.is_empty() {
            if ignored > 0 && self.status.is_none() {
                anyhow::bail!("No supported audio files were found");
            }
            return Ok(0);
        }

        imported.reverse();
        let imported_count = imported.len();
        for sound in imported {
            self.selected = Some(sound.id);
            self.sounds.insert(0, sound);
        }

        self.library_audio_tag_filter = None;
        if target_folder_id.is_some() {
            self.library_current_folder = target_folder_id;
        }
        let _ = folders_changed;
        self.save_now();
        Ok(imported_count)
    }

    fn import_path_entry(
        &mut self,
        path: &Path,
        target_folder_id: Option<Uuid>,
        imported: &mut Vec<SoundEffect>,
        ignored: &mut usize,
        folders_changed: &mut bool,
    ) -> Result<bool> {
        if Self::should_skip_import_path(path) {
            *ignored += 1;
            return Ok(false);
        }

        if path.is_dir() {
            if !Self::path_contains_supported_audio(path) {
                *ignored += 1;
                return Ok(false);
            }

            let folder_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Folder")
                .to_owned();
            let folder_id = self.create_folder_record(folder_name, target_folder_id);
            *folders_changed = true;

            for child_path in Self::sorted_directory_entries(path)? {
                let _ = self.import_path_entry(
                    &child_path,
                    Some(folder_id),
                    imported,
                    ignored,
                    folders_changed,
                )?;
            }
            return Ok(true);
        }

        if !is_supported_audio(path) {
            *ignored += 1;
            return Ok(false);
        }

        let mut sound = self.storage.import_sound(path)?;
        sound.folder_id = target_folder_id;
        imported.push(sound);
        Ok(true)
    }

    fn count_importable_audio_files(path: &Path) -> usize {
        if Self::should_skip_import_path(path) {
            return 0;
        }
        if path.is_file() {
            return usize::from(is_supported_audio(path));
        }
        if !path.is_dir() {
            return 0;
        }

        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| Self::count_importable_audio_files(&entry.path()))
            .sum()
    }

    fn run_library_import_job(
        root_dir: PathBuf,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
        tx: &Sender<LibraryImportMessage>,
        job_id: Uuid,
    ) -> Result<LibraryImportResult> {
        let mut imported_sounds = Vec::new();
        let mut imported_folders = Vec::new();
        let mut ignored_count = 0usize;
        let total = paths
            .iter()
            .map(|path| Self::count_importable_audio_files(path))
            .sum::<usize>();
        let mut completed = 0usize;

        let _ = tx.send(LibraryImportMessage::Progress {
            job_id,
            completed,
            total,
            current_label: "Preparing import...".to_owned(),
        });

        for path in paths {
            Self::import_path_entry_background(
                &root_dir,
                &path,
                target_folder_id,
                &mut imported_sounds,
                &mut imported_folders,
                &mut ignored_count,
                &mut completed,
                total,
                tx,
                job_id,
            )?;
        }

        Ok(LibraryImportResult {
            target_folder_id,
            imported_sounds,
            imported_folders,
            ignored_count,
        })
    }

    fn import_path_entry_background(
        root_dir: &Path,
        path: &Path,
        target_folder_id: Option<Uuid>,
        imported_sounds: &mut Vec<SoundEffect>,
        imported_folders: &mut Vec<Folder>,
        ignored_count: &mut usize,
        completed: &mut usize,
        total: usize,
        tx: &Sender<LibraryImportMessage>,
        job_id: Uuid,
    ) -> Result<bool> {
        if Self::should_skip_import_path(path) {
            *ignored_count += 1;
            return Ok(false);
        }

        if path.is_dir() {
            if !Self::path_contains_supported_audio(path) {
                *ignored_count += 1;
                return Ok(false);
            }

            let folder_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Folder")
                .to_owned();
            let folder_id = Uuid::new_v4();
            imported_folders.push(Folder {
                id: folder_id,
                name: folder_name,
                parent_id: target_folder_id,
            });

            for child_path in Self::sorted_directory_entries(path)? {
                let _ = Self::import_path_entry_background(
                    root_dir,
                    &child_path,
                    Some(folder_id),
                    imported_sounds,
                    imported_folders,
                    ignored_count,
                    completed,
                    total,
                    tx,
                    job_id,
                )?;
            }
            return Ok(true);
        }

        if !is_supported_audio(path) {
            *ignored_count += 1;
            return Ok(false);
        }

        let mut sound = Storage::import_sound_at(root_dir, path)?;
        sound.folder_id = target_folder_id;
        imported_sounds.push(sound);
        *completed = completed.saturating_add(1);
        let current_label = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "sound".to_owned());
        let _ = tx.send(LibraryImportMessage::Progress {
            job_id,
            completed: *completed,
            total,
            current_label,
        });
        Ok(true)
    }

    fn sorted_directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("unable to read {}", path.display()))?
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| left.file_name().cmp(&right.file_name()))
        });
        Ok(entries)
    }

    fn path_contains_supported_audio(path: &Path) -> bool {
        if Self::should_skip_import_path(path) {
            return false;
        }
        if path.is_file() {
            return is_supported_audio(path);
        }
        if !path.is_dir() {
            return false;
        }

        let Ok(entries) = fs::read_dir(path) else {
            return false;
        };
        for entry in entries.flatten() {
            if Self::path_contains_supported_audio(&entry.path()) {
                return true;
            }
        }
        false
    }

    fn open_sound_from_library(&mut self, sound_id: Uuid) {
        self.selected = Some(sound_id);
        self.editing_from_folder = self.library_current_folder;
        self.app_view = AppView::Editor;
    }

    fn open_selected_sound_for_record_export(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            self.set_error_status("Pick a sound first");
            return;
        };
        let selected_sound = self.sounds[index].clone();
        let source_path = match self.storage.drag_sound_source_path(&selected_sound) {
            Ok(path) => path,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };
        if !source_path.exists() {
            self.set_error_status(format!("unable to open {}", source_path.display()));
            return;
        }
        let mut review_sound = match self
            .storage
            .analyze_sound_as_effect(&source_path, &selected_sound.name)
        {
            Ok(sound) => sound,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };
        review_sound.waveform = Self::center_waveform_visual(&review_sound.waveform);
        let preview_id = review_sound.id;
        let preview_start = review_sound.trim_start_secs;
        let preview_duration = review_sound.safe_duration();
        let source_is_temporary = source_path != selected_sound.asset_path(self.storage.root_dir());
        self.trim_timeline_zoom = 1.0;
        self.recording_draft = Some(RecordingDraft {
            sound: review_sound,
            source_path,
            source_is_temporary,
            keep_vocal: false,
            vocal_separated_path: None,
            keep_music: false,
            music_separated_path: None,
        });
        self.show_record_panel = false;
        self.show_record_review_panel = true;
        self.app_view = AppView::Editor;
        self.set_preview_cursor_secs(preview_id, preview_start, preview_duration);
        self.preview_recording_draft_from_position(Some(preview_start));
        self.clear_status();
    }

    fn truncate_middle_ascii(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(6) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(3) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push_str("...");
        compact.push_str("...");
        compact.push_str("...");
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    fn center_waveform_visual(waveform: &[f32]) -> Vec<f32> {
        if waveform.len() < 8 {
            return waveform.to_vec();
        }

        let peak = waveform.iter().copied().fold(0.0f32, f32::max);
        if peak <= f32::EPSILON {
            return waveform.to_vec();
        }

        let threshold = (peak * 0.035).clamp(0.006, 0.04);
        let Some(first) = waveform.iter().position(|level| *level >= threshold) else {
            return waveform.to_vec();
        };
        let Some(last) = waveform.iter().rposition(|level| *level >= threshold) else {
            return waveform.to_vec();
        };
        let active = &waveform[first..=last];
        if active.len() >= waveform.len() {
            return waveform.to_vec();
        }

        let mut centered = vec![0.0; waveform.len()];
        let offset = (waveform.len() - active.len()) / 2;
        centered[offset..offset + active.len()].copy_from_slice(active);
        centered
    }

    fn recording_output_path(&self) -> PathBuf {
        let stamp = format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0)
        );
        let stem = self
            .record_name
            .trim()
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => '_',
            })
            .collect::<String>()
            .trim_matches('_')
            .to_owned();
        let file_name = if stem.is_empty() {
            format!("recording-{stamp}.wav")
        } else {
            format!("{stem}-{stamp}.wav")
        };
        self.storage.root_dir().join("recordings").join(file_name)
    }

    fn normalize_record_export_video_fps(fps: u32) -> u32 {
        match fps {
            record_video::LOW_VIDEO_FPS => record_video::LOW_VIDEO_FPS,
            record_video::HIGH_VIDEO_FPS => record_video::HIGH_VIDEO_FPS,
            _ => record_video::STANDARD_VIDEO_FPS,
        }
    }

    fn stop_recording(&mut self, ctx: Option<&Context>) {
        let was_overlay_only = self.overlay_only_mode;
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        self.overlay_only_mode = false;
        if was_overlay_only {
            self.center_window_next_frame = true;
            if let Some(ctx) = ctx {
                Self::restore_main_viewport(ctx);
            } else {
                platform::hide_native_window_by_title("Sound FX");
            }
        }
        if let Some(path) = self.recorder.take_completed_path() {
            self.open_recording_review(&path);
        }
    }

    fn start_recording(&mut self, ctx: &Context, reveal_main_window: bool) {
        if self.recording_review_pending_path.is_some() {
            self.status = Some("Preparing recorded audio...".to_owned());
            return;
        }
        if self.record_input_source == PitchInputSource::Microphone
            && self.selected_record_input_device.is_none()
        {
            self.set_error_status(self.t("pitch.no_microphone_input"));
            return;
        }

        self.close_recording_review(true);
        self.stop_preview();
        match self.recorder.start(RecorderConfig {
            source: self.record_input_source,
            input_device_name: if self.record_input_source == PitchInputSource::Microphone {
                self.selected_record_input_device.clone()
            } else {
                None
            },
            output_path: self.recording_output_path(),
        }) {
            Ok(()) => {
                self.center_record_overlay_next_frame = true;
                self.record_overlay_pos = None;
                self.record_overlay_native_visuals_applied = false;
                self.show_record_panel = false;
                if reveal_main_window {
                    Self::reveal_window(ctx);
                } else {
                    Self::apply_overlay_only_viewport(ctx, vec2(430.0, 118.0));
                    self.overlay_only_mode = true;
                    self.record_overlay_pending_visible = true;
                    ctx.request_repaint();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn toggle_recording(&mut self, ctx: &Context) {
        if self.recorder.snapshot().running {
            self.stop_recording(Some(ctx));
        } else {
            self.start_recording(ctx, false);
        }
    }

    fn preview_recording_draft_from_position(&mut self, start_position_secs: Option<f32>) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };

        self.myinstants_preview_audio_url = None;

        let (keep_vocal, keep_music, source_path, vocal_separated_path, music_separated_path) = {
            (
                draft.keep_vocal,
                draft.keep_music,
                draft.source_path.clone(),
                draft.vocal_separated_path.clone(),
                draft.music_separated_path.clone(),
            )
        };

        let source_path = if keep_music {
            if let Some(existing) = music_separated_path {
                existing
            } else {
                self.start_music_separation_if_needed();
                self.status = Some(self.t("editor.music_preview_original"));
                source_path
            }
        } else if keep_vocal {
            if let Some(existing) = vocal_separated_path {
                existing
            } else {
                self.start_vocal_separation_if_needed();
                self.status = Some(self.t("editor.vocal_preview_original"));
                source_path
            }
        } else {
            source_path
        };

        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };

        let draft = self.recording_draft.as_ref().unwrap();
        let playback = match start_position_secs {
            Some(start_position_secs) => {
                audio.play_from(&draft.sound, &source_path, start_position_secs)
            }
            None => audio.play(&draft.sound, &source_path),
        };

        match playback {
            Ok(()) => {
                if !((keep_vocal
                    && self.recording_draft.as_ref().is_some_and(|draft| {
                        draft.keep_vocal && draft.vocal_separated_path.is_none()
                    }))
                    || (keep_music
                        && self.recording_draft.as_ref().is_some_and(|draft| {
                            draft.keep_music && draft.music_separated_path.is_none()
                        })))
                {
                    self.clear_status();
                }
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn close_recording_review(&mut self, discard_audio: bool) {
        if let Some(path) = self.recording_review_pending_path.take()
            && discard_audio
        {
            let _ = fs::remove_file(path);
        }
        if let Some(draft) = self.recording_draft.take() {
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(draft.sound.id))
            {
                self.stop_preview();
            }
            if discard_audio && draft.source_is_temporary {
                let _ = fs::remove_file(draft.source_path);
            }
        }
        self.show_record_review_panel = false;
    }

    fn hide_recording_review(&mut self) {
        if let Some(draft) = self.recording_draft.as_ref()
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(draft.sound.id))
        {
            self.stop_preview();
        }
        self.show_record_review_panel = false;
    }

    fn save_recording_review_to_library(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        let keep_review_open = self.active_record_video_export.is_some();
        let keep_vocal = draft.keep_vocal;
        let keep_music = draft.keep_music;
        let source_path = draft.source_path.clone();
        let sound = draft.sound.clone();
        let vocal_separated_path = draft.vocal_separated_path.clone();
        let music_separated_path = draft.music_separated_path.clone();

        let export_path = if keep_music {
            let music_path = if let Some(existing) = music_separated_path {
                existing
            } else {
                self.start_music_separation_if_needed();
                self.set_error_status(self.t("editor.music_processing"));
                return;
            };
            match self
                .storage
                .export_processed_sound_from_path(&music_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        } else if keep_vocal {
            let vocal_path = if let Some(existing) = vocal_separated_path {
                existing
            } else {
                self.start_vocal_separation_if_needed();
                self.set_error_status(self.t("editor.vocal_processing"));
                return;
            };
            match self
                .storage
                .export_processed_sound_from_path(&vocal_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        } else {
            match self
                .storage
                .export_processed_sound_from_path(&source_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        };

        match self.storage.import_sound(&export_path) {
            Ok(mut imported) => {
                imported.name = sound.name.clone();
                imported.folder_id = None;
                self.app_view = AppView::Library;
                self.library_tab = LibraryTab::Sounds;
                self.library_current_folder = None;
                self.folder_import_select_mode = None;
                self.selected = Some(imported.id);
                self.sounds.insert(0, imported);
                self.save_now();
                let _ = fs::remove_file(&export_path);
                if !keep_review_open {
                    self.close_recording_review(true);
                }
                self.clear_status();
            }
            Err(error) => {
                let _ = fs::remove_file(&export_path);
                self.set_error_status(error);
            }
        }
    }

    fn export_recording_review_video(&mut self) {
        if self.active_record_video_export.is_some() {
            return;
        }

        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        let keep_vocal = draft.keep_vocal;
        let keep_music = draft.keep_music;
        let source_path = draft.source_path.clone();
        let vocal_separated_path = draft.vocal_separated_path.clone();
        let music_separated_path = draft.music_separated_path.clone();
        let sound = draft.sound.clone();
        let video_name = format!("{} SPN", draft.sound.name);

        self.stop_preview();

        let root_dir = self.storage.root_dir().to_path_buf();
        let show_sharps = self.record_export_video_sharps;
        let animated = self.record_export_video_animation;
        let export_fps = Self::normalize_record_export_video_fps(self.record_export_video_fps);
        let (tx, rx) = mpsc::channel();
        self.active_record_video_export = Some(RecordVideoExportState {
            progress: 0.04,
            stage: self.t("record.preparing"),
            receiver: rx,
        });
        self.clear_status();

        let record_preparing_audio = self.t("record.preparing_audio");
        let record_checking_ffmpeg = self.t("record.checking_ffmpeg");
        let record_analyzing_pitch = self.t("record.analyzing_pitch");
        let vocal_processing_error = self.t("editor.vocal_processing");
        let music_processing_error = self.t("editor.music_processing");

        thread::spawn(move || {
            let send_progress = |progress: f32, stage: &str| {
                let _ = tx.send(RecordVideoExportMessage::Progress {
                    progress,
                    stage: stage.to_owned(),
                });
            };

            let mut processed_audio_to_clean: Option<PathBuf> = None;
            let mut video_to_clean: Option<PathBuf> = None;
            let result = (|| -> Result<RecordVideoExportResult> {
                send_progress(0.08, record_preparing_audio.as_str());
                let storage = Storage::new()?;

                let audio_source = if keep_music {
                    if let Some(existing) = music_separated_path {
                        existing
                    } else {
                        anyhow::bail!(music_processing_error.clone())
                    }
                } else if keep_vocal {
                    if let Some(existing) = vocal_separated_path {
                        existing
                    } else {
                        anyhow::bail!(vocal_processing_error.clone())
                    }
                } else {
                    source_path.clone()
                };

                let processed_audio =
                    storage.export_processed_sound_from_path(&audio_source, &sound)?;
                processed_audio_to_clean = Some(processed_audio.clone());

                send_progress(0.18, record_checking_ffmpeg.as_str());
                let downloader = YoutubeAudioDownloader::new(&root_dir)?;
                let ffmpeg_path = downloader.ensure_ffmpeg_available()?;

                send_progress(0.28, record_analyzing_pitch.as_str());
                let (duration_secs, frames) =
                    analyze_pitch_file(&processed_audio, export_fps, show_sharps)?;

                let video_path = record_video::export_record_pitch_video(
                    &root_dir,
                    &ffmpeg_path,
                    &processed_audio,
                    &frames,
                    duration_secs,
                    export_fps,
                    animated,
                    |progress, stage| send_progress(progress, stage),
                )?;
                video_to_clean = Some(video_path.clone());

                Ok(RecordVideoExportResult {
                    processed_audio_path: processed_audio,
                    video_path,
                    duration_secs,
                    video_fps: export_fps,
                    video_name,
                })
            })();

            if result.is_err() {
                if let Some(path) = processed_audio_to_clean {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = video_to_clean {
                    let _ = fs::remove_file(path);
                }
            }

            let _ = tx.send(RecordVideoExportMessage::Finished(
                result.map_err(|error| error.to_string()),
            ));
        });
    }

    #[allow(dead_code)]
    fn truncate_middle(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(3) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(1) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push_str("...");
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    fn preview_sound(&mut self, sound_id: Uuid) {
        self.preview_sound_from_position(sound_id, None);
    }

    fn preview_asset_path_for_sound(&self, sound: &SoundEffect) -> PathBuf {
        if sound.needs_processed_export()
            && Storage::processed_export_exists(self.storage.root_dir(), sound)
        {
            Storage::processed_export_path(self.storage.root_dir(), sound)
        } else {
            sound.playback_asset_path(self.storage.root_dir())
        }
    }

    fn preview_sound_from_position(&mut self, sound_id: Uuid, start_position_secs: Option<f32>) {
        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };

        let asset_path = self.preview_asset_path_for_sound(&sound);
        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };
        self.myinstants_preview_audio_url = None;

        if !audio.has_cached_audio(&asset_path) {
            if let Some(error) = self.audio_preload_failures.get(&asset_path) {
                self.pending_preview_after_preload = None;
                self.set_error_status(format!("Unable to load audio preview: {error}"));
                return;
            }
            self.schedule_audio_preload(asset_path.clone());
            self.pending_preview_after_preload = Some((sound.id, start_position_secs));
            self.status = Some(format!("Loading preview for {}...", sound.name));
            return;
        }

        self.pending_preview_after_preload = None;
        let playback = if sound.needs_processed_export()
            && Storage::processed_export_exists(self.storage.root_dir(), &sound)
        {
            let start_position_secs = start_position_secs.unwrap_or(sound.trim_start_secs);
            audio.play_processed_file(&sound, &asset_path, start_position_secs)
        } else {
            match start_position_secs {
                Some(start_position_secs) => {
                    audio.play_from(&sound, &asset_path, start_position_secs)
                }
                None => audio.play(&sound, &asset_path),
            }
        };

        match playback {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    fn preview_cursor_secs_for(&self, sound: &SoundEffect) -> f32 {
        self.preview_cursor
            .and_then(|(sound_id, secs)| (sound_id == sound.id).then_some(secs))
            .unwrap_or(sound.trim_start_secs)
            .clamp(sound.trim_start_secs, sound.trim_end_secs)
    }

    fn set_preview_cursor_secs(&mut self, sound_id: Uuid, secs: f32, duration_secs: f32) {
        self.preview_cursor = Some((sound_id, secs.clamp(0.0, duration_secs)));
    }

    fn start_vocal_separation_job(
        &mut self,
        kind: SeparationStemKind,
        target: VocalSeparationTarget,
        source_path: PathBuf,
        output_dir: PathBuf,
    ) {
        if self.vocal_separation_running {
            return;
        }

        let tx = self.vocal_separation_tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.vocal_separation_running = true;
        self.vocal_separation_cancel = Some(Arc::clone(&cancel));
        self.vocal_separation_target = Some(target.clone());
        self.vocal_separation_kind = Some(kind);
        match &target {
            VocalSeparationTarget::RecordingReview { source_path } => {
                if let Some(draft) = self.recording_draft.as_ref()
                    && draft.source_path == *source_path
                {
                    self.vocal_waveform_cache
                        .borrow_mut()
                        .remove(&draft.sound.id);
                    self.music_waveform_cache
                        .borrow_mut()
                        .remove(&draft.sound.id);
                }
            }
            VocalSeparationTarget::LibrarySound { sound_id, .. } => {
                self.vocal_waveform_cache.borrow_mut().remove(sound_id);
                self.music_waveform_cache.borrow_mut().remove(sound_id);
            }
        }
        self.vocal_separation_started_at = Some(Instant::now());
        self.vocal_separation_last_result = None;
        self.clear_status();
        thread::spawn(move || {
            let result = match kind {
                SeparationStemKind::Vocal => crate::vocal_separation::extract_vocals_cancellable(
                    &source_path,
                    &output_dir,
                    Arc::clone(&cancel),
                ),
                SeparationStemKind::Music => {
                    crate::vocal_separation::extract_instrumental_cancellable(
                        &source_path,
                        &output_dir,
                        Arc::clone(&cancel),
                    )
                }
            };
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(VocalSeparationMessage::Cancelled);
                return;
            }
            let _ = tx.send(VocalSeparationMessage::Finished {
                target,
                kind,
                result,
            });
        });
    }

    fn stop_vocal_separation(&mut self) {
        if let Some(cancel) = self.vocal_separation_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
            let status_key = match self.vocal_separation_kind {
                Some(SeparationStemKind::Music) => "editor.music_stopping",
                _ => "editor.vocal_stopping",
            };
            self.status = Some(self.t(status_key));
        }
        self.vocal_separation_running = false;
        self.vocal_separation_cancel = None;
        self.vocal_separation_target = None;
        self.vocal_separation_kind = None;
        self.vocal_separation_started_at = None;
        self.vocal_separation_last_result = None;
    }

    fn start_vocal_separation_if_needed(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        if !draft.keep_vocal
            || draft.vocal_separated_path.is_some()
            || self.vocal_separation_running
        {
            return;
        }

        let source_path = draft.source_path.clone();
        let output_dir = self
            .storage
            .root_dir()
            .join("temp_vocals")
            .join(draft.sound.id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Vocal,
            VocalSeparationTarget::RecordingReview {
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    fn start_library_vocal_separation(&mut self, sound_id: Uuid) {
        if self.vocal_separation_running {
            return;
        }

        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };

        let source_path = self.sounds[index].asset_path(self.storage.root_dir());
        if !source_path.exists() {
            self.set_error_status(self.t("editor.vocal_source_missing"));
            return;
        }

        let output_dir = self
            .storage
            .root_dir()
            .join("temp_vocals")
            .join(sound_id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Vocal,
            VocalSeparationTarget::LibrarySound {
                sound_id,
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    fn start_music_separation_if_needed(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        if !draft.keep_music
            || draft.music_separated_path.is_some()
            || self.vocal_separation_running
        {
            return;
        }

        let source_path = draft.source_path.clone();
        let output_dir = self
            .storage
            .root_dir()
            .join("temp_music")
            .join(draft.sound.id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Music,
            VocalSeparationTarget::RecordingReview {
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    fn start_library_music_separation(&mut self, sound_id: Uuid) {
        if self.vocal_separation_running {
            return;
        }

        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };

        let source_path = self.sounds[index].asset_path(self.storage.root_dir());
        if !source_path.exists() {
            self.set_error_status(self.t("editor.vocal_source_missing"));
            return;
        }

        let output_dir = self
            .storage
            .root_dir()
            .join("temp_music")
            .join(sound_id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Music,
            VocalSeparationTarget::LibrarySound {
                sound_id,
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    fn pointer_primary_pressed_within(ctx: &Context, rect: Rect) -> bool {
        ctx.input(|input| {
            input.pointer.button_pressed(egui::PointerButton::Primary)
                && input
                    .pointer
                    .press_origin()
                    .is_some_and(|pos| rect.contains(pos))
        })
    }

    fn pointer_primary_drag_ready(ctx: &Context) -> bool {
        ctx.input(|input| {
            input.pointer.primary_down()
                && input
                    .pointer
                    .press_origin()
                    .zip(input.pointer.interact_pos().or(input.pointer.latest_pos()))
                    .is_some_and(|(origin, pos)| origin.distance_sq(pos) >= 36.0)
        })
    }

    fn pointer_within_rect(ctx: &Context, rect: Rect) -> bool {
        ctx.input(|input| {
            input
                .pointer
                .hover_pos()
                .or(input.pointer.interact_pos())
                .or(input.pointer.press_origin())
                .is_some_and(|pos| rect.contains(pos))
        })
    }

    fn response_pointer_within(ctx: &Context, response: &egui::Response) -> bool {
        response.hovered() || Self::pointer_within_rect(ctx, response.rect)
    }

    fn titlebar_drag_active(&self, ctx: &Context) -> bool {
        self.titlebar_drag_rect.is_some_and(|rect| {
            ctx.input(|input| {
                input.pointer.primary_down()
                    && input
                        .pointer
                        .press_origin()
                        .is_some_and(|pos| rect.contains(pos))
            })
        })
    }

    fn reveal_window(ctx: &Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn centered_outer_position(ctx: &Context, size: Vec2) -> Pos2 {
        let anchor_rect = ctx
            .input(|input| input.viewport().outer_rect.or(input.viewport().inner_rect))
            .unwrap_or_else(|| ctx.screen_rect());
        let center = anchor_rect.center();
        Pos2::new(
            (center.x - size.x * 0.5).round(),
            (center.y - size.y * 0.5).round(),
        )
    }

    fn apply_overlay_only_viewport(ctx: &Context, size: Vec2) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(
            Self::centered_outer_position(ctx, size),
        ));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn restore_main_viewport(ctx: &Context) {
        let size = Self::desired_window_size();
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(
            Self::centered_outer_position(ctx, size),
        ));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn mark_dirty(&mut self, ctx: &Context) {
        self.pending_save = true;
        self.last_edit_at = ctx.input(|input| input.time);
        self.library_filtered_sound_indices_cache
            .borrow_mut()
            .clear();
        self.library_waveform_preview_cache.borrow_mut().clear();
    }

    fn flush_pending_save(&mut self, ctx: &Context) {
        if !self.pending_save {
            return;
        }

        let now = ctx.input(|input| input.time);
        if now - self.last_edit_at >= 0.18 {
            self.save_now();
        } else {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
    }

    fn save_now(&mut self) -> bool {
        match self
            .storage
            .save_library_with_folders(&self.sounds, &self.folders)
        {
            Ok(()) => {
                self.pending_save = false;
                self.clear_status();
                true
            }
            Err(error) => {
                self.set_error_status(error);
                false
            }
        }
    }

    fn repair_sound_preview_asset(
        &mut self,
        ctx: &Context,
        sound_id: Uuid,
        failing_path: &Path,
    ) -> Result<bool> {
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return Ok(false);
        };
        let ffmpeg_path = self.downloader.ensure_ffmpeg_available()?;
        let updated = Storage::repair_sound_preview_asset_with_ffmpeg_at(
            self.storage.root_dir(),
            &self.sounds[index],
            failing_path,
            &ffmpeg_path,
        )?;
        self.sounds[index] = updated;
        self.audio_preload_failures.remove(failing_path);
        self.mark_dirty(ctx);
        if !self.save_now() {
            return Ok(false);
        }
        Ok(true)
    }

    fn schedule_processed_export(&mut self, sound_id: Uuid) {
        self.pending_processed_export_sound = Some(sound_id);
    }

    fn spawn_processed_export_job(&mut self, sound_id: Uuid) {
        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };
        if !sound.needs_processed_export() {
            return;
        }

        let root_dir = self.storage.root_dir().to_path_buf();
        let export_path = Storage::processed_export_path(&root_dir, &sound);
        if self.processed_export_inflight.contains(&export_path) || export_path.exists() {
            return;
        }
        self.processed_export_inflight.insert(export_path.clone());
        let tx = self.processed_export_tx.clone();

        thread::spawn(move || {
            let result = Storage::export_processed_sound_at(&root_dir, &sound)
                .map_err(|error| error.to_string());
            let _ = tx.send(ProcessedExportMessage::Finished {
                export_path,
                result,
            });
        });
    }

    fn start_normalize_job(&mut self, sound_id: Uuid) {
        if self.normalize_inflight.contains(&sound_id) {
            return;
        }

        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };

        let asset_path = sound.asset_path(self.storage.root_dir());
        self.normalize_inflight.insert(sound_id);
        let tx = self.normalize_tx.clone();

        thread::spawn(move || {
            let result =
                calculate_normalization_gain(&asset_path).map_err(|error| error.to_string());
            let _ = tx.send(NormalizeMessage::Finished { sound_id, result });
        });
    }

    fn schedule_audio_preload(&mut self, asset_path: PathBuf) {
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.has_cached_audio(&asset_path))
            || self.audio_preload_inflight.contains(&asset_path)
            || self.audio_preload_failures.contains_key(&asset_path)
        {
            return;
        }

        self.audio_preload_inflight.insert(asset_path.clone());
        let tx = self.audio_preload_tx.clone();
        thread::spawn(move || {
            let result = crate::audio::AudioEngine::decode_audio_for_cache(&asset_path)
                .map_err(|error| error.to_string());
            let _ = tx.send(AudioPreloadMessage::Finished { asset_path, result });
        });
    }

    fn icon(codepoint: u32, size: f32, color: Color32) -> RichText {
        RichText::new(char::from_u32(codepoint).unwrap_or(' '))
            .family(FontFamily::Name(MATERIAL_ICONS_FONT.into()))
            .size(size)
            .color(color)
    }

    fn dark_theme_enabled() -> bool {
        DARK_THEME_ENABLED.load(Ordering::Relaxed)
    }

    fn set_theme_enabled(enabled: bool) {
        DARK_THEME_ENABLED.store(enabled, Ordering::Relaxed);
    }

    fn overlay_panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 251, 254)
        }
    }

    fn input_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(31, 26, 37)
        } else {
            Color32::from_rgb(250, 246, 249)
        }
    }

    fn with_slider_visuals<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let visuals = &mut ui.style_mut().visuals;
                let widgets = &mut visuals.widgets;
                widgets.inactive.bg_fill = Color32::from_rgb(244, 240, 247);
                widgets.inactive.bg_stroke.color = Color32::from_rgb(244, 240, 247);
                widgets.hovered.bg_fill = Color32::from_rgb(255, 248, 252);
                widgets.hovered.bg_stroke.color = Color32::from_rgb(255, 248, 252);
                widgets.active.bg_fill = Color32::from_rgb(255, 253, 254);
                widgets.active.bg_stroke.color = Color32::from_rgb(255, 253, 254);
                visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
                visuals.selection.stroke.color = Color32::WHITE;
                visuals.override_text_color = Some(Color32::from_rgb(28, 22, 31));
                visuals.extreme_bg_color = Color32::from_rgb(244, 240, 247);
            }
            add_contents(ui)
        })
        .inner
    }

    fn click_slider(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
    ) -> (egui::Response, bool) {
        Self::click_slider_impl(ui, value, range, step, size, false)
    }

    fn click_slider_deferred(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
    ) -> (egui::Response, bool) {
        Self::click_slider_impl(ui, value, range, step, size, true)
    }

    fn click_slider_impl(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
        apply_on_release: bool,
    ) -> (egui::Response, bool) {
        let desired_size = vec2(size.x.max(48.0), size.y.max(20.0));
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
        let dark_theme = Self::dark_theme_enabled();
        let knob_radius = 8.0;
        let track_rect = Rect::from_center_size(
            rect.center(),
            vec2((rect.width() - knob_radius * 2.0).max(12.0), 8.0),
        );
        let min = *range.start();
        let max = *range.end();
        let span = (max - min).max(f32::EPSILON);
        let normalized = ((*value - min) / span).clamp(0.0, 1.0);
        let knob_x = egui::lerp(track_rect.left()..=track_rect.right(), normalized);
        let knob_center = Pos2::new(knob_x, track_rect.center().y);
        let track_fill = if dark_theme {
            Color32::from_rgb(244, 240, 247)
        } else {
            Color32::from_rgb(237, 231, 238)
        };
        let active_fill = Color32::from_rgb(227, 82, 149);
        let knob_fill = if response.hovered() {
            Color32::from_rgb(255, 248, 252)
        } else {
            Color32::WHITE
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(track_rect, 4.0, track_fill);
        painter.rect_filled(
            Rect::from_min_max(track_rect.min, Pos2::new(knob_x, track_rect.max.y)),
            4.0,
            active_fill,
        );
        painter.circle_filled(knob_center, knob_radius, knob_fill);
        painter.circle_stroke(knob_center, knob_radius, Stroke::new(1.5, active_fill));

        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let mut value_changed = false;
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let ratio = ((pointer.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
            let raw = min + span * ratio;
            let stepped = if step > f32::EPSILON {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            let next = stepped.clamp(min, max);
            *value = next;
            if !apply_on_release {
                value_changed = true;
            }
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let ratio = ((pointer.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
            let raw = min + span * ratio;
            let stepped = if step > f32::EPSILON {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            let next = stepped.clamp(min, max);
            *value = next;
            value_changed = true;
        }
        if apply_on_release && response.drag_stopped() {
            value_changed = true;
        }

        (response, value_changed)
    }

    fn deferred_drag_value_commit(ctx: &Context, response: &egui::Response) -> bool {
        let active_id = response.id.with("deferred-drag-active");
        if response.drag_started() || response.dragged() {
            ctx.data_mut(|data| data.insert_temp(active_id, true));
        }

        let mut commit = false;
        if response.drag_stopped() {
            commit = true;
        } else if ctx
            .data(|data| data.get_temp::<bool>(active_id))
            .unwrap_or(false)
            && !ctx.input(|input| input.pointer.primary_down())
        {
            commit = true;
        }

        let enter_commit =
            response.has_focus() && ctx.input(|input| input.key_pressed(egui::Key::Enter));
        if commit || response.lost_focus() || enter_commit {
            ctx.data_mut(|data| data.remove::<bool>(active_id));
        }

        commit || response.lost_focus() || enter_commit
    }

    fn paint_dashed_border(painter: &egui::Painter, rect: Rect, color: Color32) {
        let dash = 12.0;
        let gap = 8.0;
        let stroke = Stroke::new(1.5, color);
        let mut x = rect.left() + 12.0;
        while x < rect.right() - 12.0 {
            let end = (x + dash).min(rect.right() - 12.0);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(end, rect.top())],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(x, rect.bottom()), Pos2::new(end, rect.bottom())],
                stroke,
            );
            x += dash + gap;
        }
        let mut y = rect.top() + 12.0;
        while y < rect.bottom() - 12.0 {
            let end = (y + dash).min(rect.bottom() - 12.0);
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.left(), end)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(rect.right(), y), Pos2::new(rect.right(), end)],
                stroke,
            );
            y += dash + gap;
        }
    }

    fn reveal_in_file_explorer(path: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("explorer.exe");
            cmd.arg("/select,").arg(path);
            cmd.creation_flags(0x08000000);
            cmd.spawn()
                .with_context(|| format!("unable to reveal {}", path.display()))?;
            return Ok(());
        }
        #[allow(unreachable_code)]
        if let Some(parent) = path.parent() {
            open::that(parent).with_context(|| format!("unable to open {}", parent.display()))?;
        }
        Ok(())
    }

    fn with_dark_combo_visuals<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let visuals = &mut ui.style_mut().visuals;
                visuals.extreme_bg_color = Color32::from_rgb(28, 24, 33);
                visuals.faint_bg_color = Color32::from_rgb(33, 28, 39);
                visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.noninteractive.weak_bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(88, 70, 96);
                visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.inactive.bg_stroke.color = Color32::from_rgb(88, 70, 96);
                visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 35, 48);
                visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(43, 35, 48);
                visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
                visuals.widgets.active.bg_fill = Color32::from_rgb(53, 41, 58);
                visuals.widgets.active.weak_bg_fill = Color32::from_rgb(53, 41, 58);
                visuals.widgets.active.bg_stroke.color = Color32::from_rgb(230, 94, 150);
                visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
                visuals.selection.stroke.color = Color32::WHITE;
                visuals.override_text_color = Some(Color32::from_rgb(246, 233, 241));
            }
            add_contents(ui)
        })
        .inner
    }

    fn shadow_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgba_premultiplied(0, 0, 0, 60)
        } else {
            Color32::from_rgba_premultiplied(86, 43, 67, 22)
        }
    }

    fn render_record_panel(&mut self, ctx: &Context) {
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

    fn render_stream_panel(&mut self, ctx: &Context) {
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

    fn draw_settings_sound_row(
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

    fn render_video_viewer_panel(&mut self, ctx: &Context) {
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

    fn has_modal_panel(&self) -> bool {
        self.show_pitch_panel
            || self.show_stream_panel
            || self.show_myinstants_panel
            || self.show_import_panel
            || self.show_download_panel
            || self.show_record_panel
            || self.show_record_review_panel
            || self.show_settings_panel
            || self.video_viewer.is_some()
            || self.show_trim_commit_panel
            || self.show_delete_folder_confirm.is_some()
    }
    fn render_pitch_monitor(&mut self, ctx: &Context) {
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
            self.centered_modal_placement(ctx, vec2(332.0, 312.0), vec2(292.0, 268.0), 0.0);
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
                ui.set_width(292.0);
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
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Hotkey")
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
                            );
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                                let keyboard_response = Self::icon_action(
                                    ui,
                                    [42.0, 34.0],
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
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Input")
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
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
                                                                    name, 28,
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
                });

                ui.add_space(10.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Display")
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
                            );
                            ui.add_space(8.0);
                            Self::with_slider_visuals(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(Self::icon(0xe8b5, 16.0, Self::muted_text_color()));
                                    let (speed_response, _) = Self::click_slider(
                                        ui,
                                        &mut self.pitch_update_hz,
                                        1.0..=12.0,
                                        0.5,
                                        vec2(164.0, 24.0),
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
                                .fill(Color32::from_rgb(255, 248, 252))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(236, 224, 232)))
                                .corner_radius(16.0)
                                .inner_margin(Margin::symmetric(12, 10))
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = vec2(12.0, 8.0);
                                        let animation_label = self.t("settings.animation");
                                        let sharp_label = self.t("pitch.sharp");
                                        let animation_changed = ui
                                            .checkbox(
                                                &mut self.pitch_overlay_animation,
                                                RichText::new(animation_label)
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(58, 48, 58)),
                                            )
                                            .changed();
                                        let sharp_changed = ui
                                            .checkbox(
                                                &mut self.pitch_show_sharps,
                                                RichText::new(sharp_label)
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(58, 48, 58)),
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

    fn render_pitch_overlay_viewport(&mut self, ctx: &Context) {
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

    fn render_pitch_pill_overlay(
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

    fn render_pitch_blob_overlay(
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

    fn pitch_overlay_meta(&self, snapshot: &PitchSnapshot) -> String {
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

    fn display_pitch_note(&self, note: &str) -> String {
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

    fn draw_pitch_overlay_wave_strip(ui: &mut Ui, snapshot: &PitchSnapshot) {
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

    fn draw_library_grid(&mut self, ui: &mut Ui) {
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
                        RichText::new("Library").size(12.5),
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
                                RichText::new("Grid").size(11.5),
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
                                RichText::new("Rows").size(11.5),
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
                                RichText::new(format!("Row {}", self.library_row_thickness))
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

    fn draw_library_sound_grid_content(
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
                    drag_sound = Some(sound.id);
                    self.pending_sound_drag = None;
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
                                        if Self::icon_action(
                                            ui,
                                            action_button_size,
                                            if is_loading { 0xe5d5 } else { 0xe037 },
                                            is_loading,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            if is_loading {
                                                self.stop_preview();
                                            } else {
                                                preview_sound = Some(sound.id);
                                            }
                                        }
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
                                            RichText::new("Copied").size(11.0).color(meta_color),
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
                    self.status = Some("Copied to clipboard".to_owned());
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

    fn draw_video_library_grid(&mut self, ui: &mut Ui) {
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
                                            RichText::new("Copied").size(11.0).color(meta_color),
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
                self.status = Some("Copied to clipboard".to_owned());
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

    fn render_record_overlay_viewport(&mut self, ctx: &Context) {
        let snapshot = self.recorder.snapshot();
        if !snapshot.running {
            self.record_overlay_open = false;
            self.record_overlay_native_visuals_applied = false;
            return;
        }
        self.record_overlay_open = true;
        let mut should_stop = false;

        let overlay_size = vec2(430.0, 118.0);
        let overlay_pos =
            if self.center_record_overlay_next_frame || self.record_overlay_pos.is_none() {
                let centered = self.centered_overlay_pos(ctx, overlay_size);
                self.record_overlay_pos = Some(centered);
                centered
            } else {
                self.record_overlay_pos.unwrap_or_default()
            };
        ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        let area_id = egui::Id::new("record-overlay-panel");
        egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .current_pos(overlay_pos)
            .constrain_to(self.popup_safe_rect(ctx))
            .interactable(true)
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    overlay_size,
                    egui::Layout::top_down(Align::Min),
                    |ui| self.render_record_blob_overlay(ui, ctx, &snapshot, &mut should_stop),
                );
            });
        if let Some(state) = egui::AreaState::load(ctx, area_id) {
            self.record_overlay_pos =
                Some(self.clamp_overlay_pos(ctx, overlay_size, state.left_top_pos()));
        }
        self.center_record_overlay_next_frame = false;
        self.record_overlay_native_visuals_applied = false;
        if should_stop {
            self.stop_recording(Some(ctx));
        }
    }

    fn render_record_blob_overlay(
        &mut self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &crate::recorder::RecorderSnapshot,
        should_stop: &mut bool,
    ) {
        let rect = ui.max_rect().shrink2(vec2(8.0, 8.0));
        let response = ui.interact(
            rect,
            ui.id().with("record-blob-overlay-drag"),
            Sense::click_and_drag(),
        );
        if self.overlay_only_mode {
            if response.drag_started() {
                overlay_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        } else {
            let overlay_rect = self.popup_safe_rect(overlay_ctx);
            Self::update_overlay_drag_position(
                overlay_ctx,
                overlay_rect,
                &response,
                vec2(430.0, 118.0),
                &mut self.record_overlay_pos,
            );
        }
        let painter = ui.painter_at(rect);
        let center = rect.center();
        let time = overlay_ctx.input(|input| input.time) as f32;
        let pulse = (time * 4.4).sin() * 0.5 + 0.5;
        let aura = snapshot.level.clamp(0.06, 1.0);

        for (scale, alpha) in [(1.08, 20), (1.04, 34)] {
            let points = Self::squircle_points(
                center,
                rect.width() * 0.5 * scale,
                rect.height() * 0.38 * scale,
                4.8,
                0.03 + aura * 0.02,
                time * 0.8,
            );
            painter.add(egui::Shape::convex_polygon(
                points,
                Color32::from_rgba_premultiplied(214, 51, 132, alpha),
                Stroke::NONE,
            ));
        }

        let blob = Self::squircle_points(
            center,
            rect.width() * 0.48,
            rect.height() * 0.34,
            4.8,
            0.04 + aura * 0.025,
            time,
        );
        painter.add(egui::Shape::convex_polygon(
            blob,
            Color32::from_rgba_premultiplied(17, 14, 20, 238),
            Stroke::new(1.4, Color32::from_rgba_premultiplied(236, 116, 179, 220)),
        ));

        let dot_center = Pos2::new(rect.left() + 40.0, center.y - 6.0);
        painter.circle_filled(
            dot_center,
            8.0 + pulse * 2.0,
            Color32::from_rgba_premultiplied(214, 51, 132, 230),
        );
        painter.text(
            Pos2::new(rect.left() + 58.0, center.y - 18.0),
            egui::Align2::LEFT_TOP,
            "Recording",
            egui::FontId::new(16.0, FontFamily::Proportional),
            Color32::from_rgb(255, 234, 244),
        );
        painter.text(
            Pos2::new(rect.left() + 58.0, center.y + 2.0),
            egui::Align2::LEFT_TOP,
            format_time(snapshot.elapsed_secs),
            egui::FontId::new(12.0, FontFamily::Proportional),
            Color32::from_rgb(219, 185, 206),
        );

        let stop_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 22.0, rect.top() + 22.0),
            vec2(28.0, 28.0),
        );
        let stop_response = ui.interact(
            stop_rect,
            ui.id().with("record-blob-overlay-stop"),
            Sense::click(),
        );
        if stop_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        painter.rect_filled(
            stop_rect,
            14.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 16),
        );
        painter.text(
            stop_rect.center(),
            egui::Align2::CENTER_CENTER,
            char::from_u32(0xe047).unwrap_or(' '),
            egui::FontId::new(18.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
            Color32::from_rgb(255, 234, 244),
        );
        if stop_response.clicked() {
            *should_stop = true;
        }

        let wave_rect =
            Rect::from_center_size(Pos2::new(rect.right() - 132.0, center.y), vec2(210.0, 44.0));
        painter.rect_filled(
            wave_rect,
            18.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 10),
        );
        let bars = if snapshot.waveform.is_empty() {
            vec![0.04; 40]
        } else {
            snapshot.waveform.clone()
        };
        let inner = wave_rect.shrink2(vec2(12.0, 8.0));
        let bar_width = inner.width() / bars.len().max(1) as f32;
        for (index, level) in bars.iter().enumerate() {
            let x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = level.clamp(0.04, 1.0) * inner.height() * 0.42;
            let bar = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.18, inner.center().y - half),
                Pos2::new(x + bar_width * 0.18, inner.center().y + half),
            );
            painter.rect_filled(bar, 4.0, Color32::from_rgb(231, 92, 162));
        }
    }

    fn draw_library(&mut self, ui: &mut Ui) {
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
                            let row_height = self.library_list_row_height().max(108.0);
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
                                drag_request = Some(sound.id);
                                self.pending_sound_drag = None;
                            }
                            if !modal_open && response.clicked() {
                                self.selected = Some(sound.id);
                                if is_loading || playing {
                                    self.stop_preview();
                                } else {
                                    preview_request = Some(sound.id);
                                }
                            }
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

    fn draw_editor(&mut self, ui: &mut Ui, ctx: &Context) {
        let Some(index) = self.selected_sound_index() else {
            self.draw_empty_editor(ui);
            return;
        };

        let (sound_id, vocal_job_running, vocal_ready, music_job_running, music_ready) = {
            let sound = &self.sounds[index];
            let vocal_asset_path = sound.vocal_asset_path(self.storage.root_dir());
            let vocal_ready = vocal_asset_path.as_ref().is_some_and(|path| path.exists());
            let music_asset_path = sound.music_asset_path(self.storage.root_dir());
            let music_ready = music_asset_path.as_ref().is_some_and(|path| path.exists());
            let vocal_job_running = self.vocal_separation_running
                && self.vocal_separation_kind == Some(SeparationStemKind::Vocal)
                && matches!(
                    self.vocal_separation_target.as_ref(),
                    Some(VocalSeparationTarget::LibrarySound {
                        sound_id: target_sound_id,
                        ..
                    }) if *target_sound_id == sound.id
                );
            let music_job_running = self.vocal_separation_running
                && self.vocal_separation_kind == Some(SeparationStemKind::Music)
                && matches!(
                    self.vocal_separation_target.as_ref(),
                    Some(VocalSeparationTarget::LibrarySound {
                        sound_id: target_sound_id,
                        ..
                    }) if *target_sound_id == sound.id
                );
            (
                sound.id,
                vocal_job_running,
                vocal_ready,
                music_job_running,
                music_ready,
            )
        };
        let preview_asset_path = self.preview_asset_path_for_sound(&self.sounds[index]);
        let editor_audio_loading = self.audio_preload_inflight.contains(&preview_asset_path)
            || self
                .pending_preview_after_preload
                .is_some_and(|(pending_sound_id, _)| pending_sound_id == sound_id);
        self.schedule_audio_preload(preview_asset_path);
        self.sync_editor_tags_input();
        let waveform_samples = self.sound_waveform_samples(&self.sounds[index]);
        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound_id));
        let playhead_drag_active = ctx
            .data(|data| data.get_temp::<bool>(Self::trim_playhead_drag_id(sound_id)))
            .unwrap_or(false);
        let playback_position_secs = self
            .audio
            .as_ref()
            .and_then(|audio| audio.playback_position_secs(sound_id));
        let mut preview_cursor_secs = {
            let sound = &self.sounds[index];
            let mut cursor = self.preview_cursor_secs_for(sound);
            if let Some(position_secs) = playback_position_secs
                && !playhead_drag_active
            {
                cursor = position_secs.clamp(0.0, sound.safe_duration());
            }
            cursor
        };

        let mut delete_request = false;
        let mut copy_request = false;
        let mut open_location_request = false;
        let mut commit_trim_request = false;
        let mut seek_request = false;
        let mut playback_reapply_request = false;
        let mut normalize_request = false;
        let mut start_vocal_job = false;
        let mut start_music_job = false;
        let mut stop_vocal_job = false;
        let mut stop_music_job = false;
        let mut changed = false;
        let mut processed_export_dirty = false;
        let mut tags_changed = false;
        let mut vocal_reapply_request = false;
        let mut trim_timeline_zoom = self.trim_timeline_zoom;
        let editor_timeline_interactive = !self.has_modal_panel();
        let normalize_loading = self.normalize_inflight.contains(&sound_id);
        let vocal_only_label = self.t("editor.vocal_only");
        let vocal_separate_label = self.t("editor.vocal_separate");
        let vocal_stop_label = self.t("editor.vocal_stop");
        let vocal_ready_label = self.t("editor.vocal_ready");
        let vocal_loading_label = self.t("editor.vocal_loading");
        let music_only_label = self.t("editor.music_only");
        let music_separate_label = self.t("editor.music_separate");
        let music_ready_label = self.t("editor.music_ready");
        let music_loading_label = self.t("editor.music_loading");
        let music_hint_label = self.t("editor.music_hint");
        let vocal_elapsed_label = self.t("editor.vocal_elapsed");
        let vocal_hint_label = self.t("editor.vocal_hint");
        let effects_label = self.t("editor.effects");
        let effect_reverb_label = self.t("editor.effect_reverb");
        let effect_telephone_label = self.t("editor.effect_telephone");
        let effect_distortion_label = self.t("editor.effect_distortion");
        let effect_echo_label = self.t("editor.effect_echo");
        let effect_underwater_label = self.t("editor.effect_underwater");
        let effect_robot_label = self.t("editor.effect_robot");
        let effect_pitch_shift_label = self.t("editor.effect_pitch_shift");
        let effect_reverb_hint = self.t("editor.effect_reverb_hint");
        let effect_telephone_hint = self.t("editor.effect_telephone_hint");
        let effect_distortion_hint = self.t("editor.effect_distortion_hint");
        let effect_echo_hint = self.t("editor.effect_echo_hint");
        let effect_underwater_hint = self.t("editor.effect_underwater_hint");
        let effect_robot_hint = self.t("editor.effect_robot_hint");
        let effect_pitch_shift_hint = self.t("editor.effect_pitch_shift_hint");
        let vocal_elapsed_text = if vocal_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let vocal_last_elapsed_text = self
            .vocal_separation_last_elapsed_for_sound(sound_id, SeparationStemKind::Vocal)
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let music_elapsed_text = if music_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let music_last_elapsed_text = self
            .vocal_separation_last_elapsed_for_sound(sound_id, SeparationStemKind::Music)
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let tags_label = self.t("editor.tags");
        let tags_hint = self.t("editor.tags_hint");
        let tags_available_label = self.t("editor.tags_available");
        let available_tags = self.distinct_sound_tags();

        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Self::shadow_color(),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                let sound = &mut self.sounds[index];
                let controls_width = 52.0 + 52.0 + 52.0 + 64.0 + 36.0;
                let row_gap = 8.0;
                let back_button_width = if self.editing_from_folder.is_some() {
                    42.0 + 8.0
                } else {
                    0.0
                };
                let name_width =
                    (ui.available_width() - controls_width - row_gap - back_button_width)
                        .max(120.0);

                ui.horizontal(|ui| {
                    if let Some(folder_id) = self.editing_from_folder {
                        if Self::icon_action(ui, [42.0, 34.0], 0xe5c4, false, false).clicked() {
                            self.app_view = AppView::Library;
                            self.library_tab = LibraryTab::Sounds;
                            self.library_current_folder = Some(folder_id);
                            self.editing_from_folder = None;
                        }
                        ui.add_space(8.0);
                    }
                    let response = Frame::new()
                        .fill(Self::input_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(16.0)
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.add_sized(
                                [name_width - 24.0, 26.0],
                                TextEdit::singleline(&mut sound.name)
                                    .frame(false)
                                    .font(egui::TextStyle::Heading)
                                    .desired_width(name_width)
                                    .margin(Vec2::new(0.0, 5.0)),
                            )
                        })
                        .inner;
                    if response.changed() {
                        changed = true;
                    }

                    ui.add_space(row_gap);
                    ui.allocate_ui_with_layout(
                        vec2(controls_width, 34.0),
                        egui::Layout::right_to_left(Align::Center),
                        |ui| {
                            if Self::icon_action(ui, [52.0, 34.0], 0xe872, false, false).clicked() {
                                delete_request = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe14e, false, false).clicked() {
                                commit_trim_request = true;
                            }
                            if Self::icon_action(ui, [64.0, 34.0], 0xe14d, false, false).clicked() {
                                copy_request = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe2c8, false, false).clicked() {
                                open_location_request = true;
                            }
                        },
                    );
                });

                ui.add_space(2.0);

                egui::CollapsingHeader::new(
                    RichText::new(&tags_label)
                        .size(11.5)
                        .color(Self::muted_text_color())
                        .strong(),
                )
                .default_open(false)
                .show(ui, |ui| {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(26.0)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    let hint_color = Color32::from_rgba_premultiplied(
                                        Self::muted_text_color().r(),
                                        Self::muted_text_color().g(),
                                        Self::muted_text_color().b(),
                                        128,
                                    );
                                    let response = Frame::new()
                                        .fill(Self::input_fill())
                                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                                        .corner_radius(14.0)
                                        .inner_margin(Margin::symmetric(10, 4))
                                        .show(ui, |ui| {
                                            ui.add_sized(
                                                [ui.available_width(), 24.0],
                                                TextEdit::singleline(&mut self.editor_tags_input)
                                                    .frame(false)
                                                    .hint_text(
                                                        RichText::new(tags_hint.as_str())
                                                            .color(hint_color),
                                                    )
                                                    .desired_width(f32::INFINITY),
                                            )
                                        })
                                        .inner;
                                    if response.changed() {
                                        tags_changed = true;
                                    }
                                });
                                if !available_tags.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new(&tags_available_label)
                                            .size(11.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    if Self::draw_sound_tag_picker(
                                        ui,
                                        &available_tags,
                                        &mut self.editor_tags_input,
                                    ) {
                                        tags_changed = true;
                                    }
                                }
                            });
                        });
                });

                ui.add_space(12.0);

                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        let (timeline_changed, timeline_seek_request, timeline_preview_commit) =
                            Self::draw_trim_timeline(
                                ui,
                                sound,
                                &waveform_samples,
                                &mut preview_cursor_secs,
                                &mut trim_timeline_zoom,
                                !is_playing,
                                editor_timeline_interactive,
                                editor_audio_loading,
                            );
                        changed |= timeline_changed;
                        seek_request |= timeline_seek_request;
                        if timeline_preview_commit {
                            seek_request = true;
                            processed_export_dirty = true;
                        }
                    });

                ui.add_space(18.0);

                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        Self::with_slider_visuals(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Self::icon(0xe050, 16.0, Self::muted_text_color()));
                                let (volume_response, volume_slider_changed) =
                                    Self::click_slider_deferred(
                                        ui,
                                        &mut sound.volume,
                                        0.0..=5.0,
                                        0.0,
                                        vec2(128.0, 24.0),
                                    );
                                let volume_input = ui.add(
                                    DragValue::new(&mut sound.volume)
                                        .range(0.0..=5.0)
                                        .speed(0.01)
                                        .max_decimals(2)
                                        .suffix("x"),
                                );
                                sound.volume = sound.volume.clamp(0.0, 5.0);
                                ui.add_space(8.0);
                                let normalize_response = ui.add_enabled(
                                    !normalize_loading,
                                    Button::new(
                                        RichText::new("Normalize")
                                            .size(11.0)
                                            .color(Color32::from_rgb(214, 51, 132)),
                                    )
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(12.0),
                                );
                                if normalize_response
                                    .on_hover_text(
                                        "Automatically adjust volume to a standard listening level",
                                    )
                                    .clicked()
                                {
                                    normalize_request = true;
                                }
                                if normalize_loading {
                                    ui.add_space(6.0);
                                    ui.add(egui::Spinner::new().size(16.0));
                                }
                                ui.add_space(10.0);
                                ui.label(Self::icon(0xe9e4, 16.0, Self::muted_text_color()));
                                let (speed_response, speed_slider_changed) =
                                    Self::click_slider_deferred(
                                        ui,
                                        &mut sound.speed,
                                        0.25..=2.0,
                                        0.0,
                                        vec2(128.0, 24.0),
                                    );
                                let speed_input = ui.add(
                                    DragValue::new(&mut sound.speed)
                                        .range(0.25..=2.0)
                                        .speed(0.01)
                                        .max_decimals(2)
                                        .suffix("x"),
                                );
                                sound.speed = sound.speed.clamp(0.25, 2.0);
                                let volume_input_commit =
                                    Self::deferred_drag_value_commit(ui.ctx(), &volume_input);
                                let speed_input_commit =
                                    Self::deferred_drag_value_commit(ui.ctx(), &speed_input);
                                if volume_response.changed()
                                    || speed_response.changed()
                                    || volume_input.changed()
                                    || speed_input.changed()
                                    || volume_slider_changed
                                    || speed_slider_changed
                                {
                                    changed = true;
                                    processed_export_dirty = true;
                                }
                                if volume_slider_changed
                                    || speed_slider_changed
                                    || volume_input_commit
                                    || speed_input_commit
                                {
                                    playback_reapply_request = true;
                                }
                            });
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&effects_label)
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                                let reverb = ui
                                    .add_sized(
                                        [88.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_reverb_label).size(11.5),
                                            sound.reverb_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_reverb_hint);
                                Self::decorate_button_response(ui, &reverb);
                                if reverb.clicked() {
                                    sound.reverb_enabled = !sound.reverb_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }

                                let telephone = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_telephone_label).size(11.5),
                                            sound.telephone_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_telephone_hint);
                                Self::decorate_button_response(ui, &telephone);
                                if telephone.clicked() {
                                    sound.telephone_enabled = !sound.telephone_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }

                                let distortion = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_distortion_label).size(11.5),
                                            sound.distortion_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_distortion_hint);
                                Self::decorate_button_response(ui, &distortion);
                                if distortion.clicked() {
                                    sound.distortion_enabled = !sound.distortion_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }

                                let echo = ui
                                    .add_sized(
                                        [78.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_echo_label).size(11.5),
                                            sound.echo_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_echo_hint);
                                Self::decorate_button_response(ui, &echo);
                                if echo.clicked() {
                                    sound.echo_enabled = !sound.echo_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(54.0);
                                let underwater = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_underwater_label).size(11.5),
                                            sound.underwater_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_underwater_hint);
                                Self::decorate_button_response(ui, &underwater);
                                if underwater.clicked() {
                                    sound.underwater_enabled = !sound.underwater_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }

                                let robot = ui
                                    .add_sized(
                                        [78.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_robot_label).size(11.5),
                                            sound.robot_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_robot_hint);
                                Self::decorate_button_response(ui, &robot);
                                if robot.clicked() {
                                    sound.robot_enabled = !sound.robot_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }

                                let pitch_shift = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_pitch_shift_label).size(11.5),
                                            sound.pitch_shift_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_pitch_shift_hint);
                                Self::decorate_button_response(ui, &pitch_shift);
                                if pitch_shift.clicked() {
                                    sound.pitch_shift_enabled = !sound.pitch_shift_enabled;
                                    changed = true;
                                    processed_export_dirty = true;
                                    playback_reapply_request = true;
                                }
                                if sound.pitch_shift_enabled {
                                    let semitone_input = ui.add(
                                        DragValue::new(&mut sound.pitch_shift_semitones)
                                            .range(-24.0..=24.0)
                                            .speed(0.1)
                                            .max_decimals(1)
                                            .suffix(" st"),
                                    );
                                    if Self::deferred_drag_value_commit(ctx, &semitone_input) {
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }
                                }
                            });
                        });
                    });

                ui.add_space(16.0);

                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&vocal_only_label)
                                        .size(11.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                let vocal_toggle = ui
                                    .add(Checkbox::new(&mut sound.vocal_only, ""))
                                    .on_hover_text(&vocal_hint_label);
                                if vocal_toggle.changed() {
                                    changed = true;
                                    vocal_reapply_request = true;
                                    if sound.vocal_only {
                                        sound.music_only = false;
                                        if vocal_ready {
                                            vocal_reapply_request = true;
                                        } else if !vocal_job_running {
                                            start_vocal_job = true;
                                        }
                                    } else if vocal_job_running {
                                        stop_vocal_job = true;
                                    }
                                }

                                ui.add_space(8.0);

                                if vocal_job_running {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new(&vocal_loading_label)
                                            .size(11.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    if let Some(vocal_elapsed_text) = &vocal_elapsed_text {
                                        ui.label(
                                            RichText::new(vocal_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                    let stop = ui.add(
                                        Button::new(
                                            RichText::new(&vocal_stop_label)
                                                .size(10.5)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        )
                                        .fill(Self::surface_fill())
                                        .stroke(Stroke::new(1.0, Self::border_color()))
                                        .corner_radius(10.0),
                                    );
                                    if stop.clicked() {
                                        stop_vocal_job = true;
                                    }
                                } else if vocal_ready {
                                    ui.label(
                                        RichText::new(&vocal_ready_label)
                                            .size(11.0)
                                            .color(Color32::from_rgb(100, 200, 100)),
                                    );
                                    if let Some(vocal_last_elapsed_text) = &vocal_last_elapsed_text
                                    {
                                        ui.label(
                                            RichText::new(vocal_last_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                } else {
                                    let separate = ui.add(
                                        Button::new(
                                            RichText::new(&vocal_separate_label)
                                                .size(10.5)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        )
                                        .fill(Self::surface_fill())
                                        .stroke(Stroke::new(1.0, Self::border_color()))
                                        .corner_radius(10.0),
                                    );
                                    if separate.clicked() {
                                        if !vocal_job_running {
                                            sound.vocal_only = true;
                                            sound.music_only = false;
                                            changed = true;
                                            start_vocal_job = true;
                                        }
                                    }
                                }
                            });
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&music_only_label)
                                        .size(11.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                let music_toggle = ui
                                    .add(Checkbox::new(&mut sound.music_only, ""))
                                    .on_hover_text(&music_hint_label);
                                if music_toggle.changed() {
                                    changed = true;
                                    vocal_reapply_request = true;
                                    if sound.music_only {
                                        sound.vocal_only = false;
                                        if music_ready {
                                            vocal_reapply_request = true;
                                        } else if !music_job_running {
                                            start_music_job = true;
                                        }
                                    } else if music_job_running {
                                        stop_music_job = true;
                                    }
                                }

                                ui.add_space(8.0);

                                if music_job_running {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new(&music_loading_label)
                                            .size(11.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    if let Some(music_elapsed_text) = &music_elapsed_text {
                                        ui.label(
                                            RichText::new(music_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                    let stop = ui.add(
                                        Button::new(
                                            RichText::new(&vocal_stop_label)
                                                .size(10.5)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        )
                                        .fill(Self::surface_fill())
                                        .stroke(Stroke::new(1.0, Self::border_color()))
                                        .corner_radius(10.0),
                                    );
                                    if stop.clicked() {
                                        stop_music_job = true;
                                    }
                                } else if music_ready {
                                    ui.label(
                                        RichText::new(&music_ready_label)
                                            .size(11.0)
                                            .color(Color32::from_rgb(100, 200, 100)),
                                    );
                                    if let Some(music_last_elapsed_text) = &music_last_elapsed_text
                                    {
                                        ui.label(
                                            RichText::new(music_last_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                } else {
                                    let separate = ui.add(
                                        Button::new(
                                            RichText::new(&music_separate_label)
                                                .size(10.5)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        )
                                        .fill(Self::surface_fill())
                                        .stroke(Stroke::new(1.0, Self::border_color()))
                                        .corner_radius(10.0),
                                    );
                                    if separate.clicked() {
                                        if !music_job_running {
                                            sound.music_only = true;
                                            sound.vocal_only = false;
                                            changed = true;
                                            start_music_job = true;
                                        }
                                    }
                                }
                            });
                        });
                    });

                ui.add_space(16.0);

                let drop_response = Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::NONE)
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(76.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Drop sound here")
                                    .size(14.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new("or click to open import browser")
                                    .size(12.5)
                                    .color(Self::muted_text_color()),
                            );
                        });
                    })
                    .response
                    .interact(Sense::click());
                Self::paint_dashed_border(
                    ui.painter(),
                    drop_response.rect.shrink(8.0),
                    Self::subtle_border_color(),
                );
                self.editor_drop_rect = Some(drop_response.rect.expand(8.0));
                if drop_response.hovered()
                    && ui.ctx().input(|input| input.raw.hovered_files.is_empty())
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if drop_response.clicked() {
                    self.add_sound();
                }
            });

        let sound_id = self.sounds[index].id;
        let sound_duration = self.sounds[index].safe_duration();
        self.trim_timeline_zoom = trim_timeline_zoom;
        if tags_changed {
            let tags = Self::parse_tags(&self.editor_tags_input);
            self.sounds[index].tags = tags;
            changed = true;
        }
        self.set_preview_cursor_secs(sound_id, preview_cursor_secs, sound_duration);

        if start_vocal_job {
            self.start_library_vocal_separation(sound_id);
        }
        if start_music_job {
            self.start_library_music_separation(sound_id);
        }
        if stop_vocal_job {
            self.stop_vocal_separation();
        }
        if stop_music_job {
            self.stop_vocal_separation();
        }
        if vocal_reapply_request {
            let vocal_preview_path = self.preview_asset_path_for_sound(&self.sounds[index]);
            self.schedule_audio_preload(vocal_preview_path);
        }

        if (seek_request || playback_reapply_request || vocal_reapply_request) && is_playing {
            self.preview_sound_from_position(sound_id, Some(preview_cursor_secs));
        }

        if changed {
            self.mark_dirty(ctx);
        }

        if delete_request {
            self.delete_selected();
            return;
        }

        if copy_request {
            self.copy_selected_processed_sound();
        }

        if normalize_request {
            self.start_normalize_job(sound_id);
        }

        if processed_export_dirty {
            self.schedule_processed_export(sound_id);
        }

        if open_location_request {
            let path = self.sounds[index].asset_path(self.storage.root_dir());
            if let Err(error) = Self::reveal_in_file_explorer(&path) {
                self.set_error_status(error);
            }
        }

        if commit_trim_request {
            self.show_trim_commit_panel = true;
        }
    }

    fn draw_empty_editor(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(Color32::from_rgba_premultiplied(255, 255, 255, 210))
            .stroke(Stroke::new(1.0, Color32::from_rgb(236, 223, 232)))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Color32::from_rgba_premultiplied(84, 48, 69, 16),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(28))
            .show(ui, |ui| {
                let height = ui.available_height().clamp(280.0, 420.0);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    self.add_sound();
                }

                let painter = ui.painter_at(rect);
                let center = rect.center();
                painter.text(
                    center,
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::new(52.0, FontFamily::Proportional),
                    Color32::from_rgb(214, 51, 132),
                );
                painter.text(
                    Pos2::new(center.x, center.y + 42.0),
                    Align2::CENTER_CENTER,
                    "Click to import sound",
                    FontId::new(13.5, FontFamily::Proportional),
                    Color32::from_rgb(122, 96, 111),
                );
            });
    }

    fn draw_trim_timeline(
        ui: &mut Ui,
        sound: &mut SoundEffect,
        waveform_samples: &[f32],
        preview_cursor_secs: &mut f32,
        zoom: &mut f32,
        clamp_cursor_to_trim: bool,
        interactive: bool,
        show_loading_indicator: bool,
    ) -> (bool, bool, bool) {
        sound.clamp_trim();
        let duration = sound.safe_duration();
        *preview_cursor_secs = if clamp_cursor_to_trim {
            (*preview_cursor_secs).clamp(sound.trim_start_secs, sound.trim_end_secs)
        } else {
            (*preview_cursor_secs).clamp(0.0, duration)
        };
        *zoom = (*zoom).clamp(1.0, 8.0);
        let playhead_drag_id = Self::trim_playhead_drag_id(sound.id);

        ui.horizontal(|ui| {
            ui.label(Self::icon(0xe14e, 14.0, Color32::from_rgb(118, 106, 116)));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Trim")
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.add_space(6.0);
            let help = ui.add_sized(
                [24.0, 24.0],
                Button::new(Self::icon(0xe887, 16.0, Color32::from_rgb(214, 51, 132)))
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(12.0),
            );
            if help.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
            }
            help.on_hover_ui_at_pointer(|ui| {
                ui.set_max_width(250.0);
                ui.label(
                    RichText::new("Trim shortcuts")
                        .size(13.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label("Space: preview or stop");
                ui.label("S: preview from the left trim");
                ui.label("Q: move the left trim to the mouse");
                ui.label("W: move the right trim to the mouse");
                ui.label("A / D: pan timeline left or right");
                ui.label("Ctrl + mouse wheel: zoom around the hover playhead");
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{:.1}x", *zoom))
                        .size(12.0)
                        .color(Self::muted_text_color()),
                );
            });
        });
        ui.add_space(8.0);

        let viewport_width = ui.available_width().max(320.0);
        let zoom_scroll_offset_id = egui::Id::new((sound.id, "trim-zoom-offset"));
        let trim_adjusting_id = egui::Id::new((sound.id, "trim-adjusting"));
        let trim_hotkey_adjusting_id = egui::Id::new((sound.id, "trim-hotkey-adjusting"));
        let stored_zoom_scroll_offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(zoom_scroll_offset_id));
        let mut requested_scroll_offset: Option<f32> = None;
        let timeline_size = vec2((viewport_width * *zoom).max(viewport_width), 160.0);
        let dark_theme = Self::dark_theme_enabled();
        let mut changed = false;
        let mut seek_requested = false;
        let mut preview_commit_requested = false;

        ui.allocate_ui_with_layout(
            vec2(viewport_width, timeline_size.y + 10.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                let mut scroll_area = ScrollArea::horizontal()
                    .id_salt((sound.id, "trim-timeline-scroll"))
                    .drag_to_scroll(false)
                    .auto_shrink([false, false]);
                if let Some(offset) = stored_zoom_scroll_offset {
                    scroll_area = scroll_area.horizontal_scroll_offset(offset);
                }
                let scroll_output = scroll_area.show(ui, |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(timeline_size, Sense::click_and_drag());
                    let viewport_rect = rect.intersect(ui.clip_rect());
                    let painter = ui.painter_at(rect);
                    let timeline_fill = if dark_theme {
                        Color32::from_rgb(11, 10, 14)
                    } else {
                        Color32::from_rgb(255, 255, 255)
                    };
                    let timeline_stroke = if dark_theme {
                        Color32::from_rgb(74, 61, 82)
                    } else {
                        Color32::from_rgb(235, 223, 232)
                    };
                    painter.rect_filled(rect, 18.0, timeline_fill);
                    painter.rect_stroke(
                        rect,
                        18.0,
                        Stroke::new(1.0, timeline_stroke),
                        StrokeKind::Outside,
                    );

                    let start_t = sound.trim_start_secs / duration;
                    let end_t = sound.trim_end_secs / duration;
                    let start_x = rect.left() + rect.width() * start_t.clamp(0.0, 1.0);
                    let end_x = rect.left() + rect.width() * end_t.clamp(0.0, 1.0);

                    Self::paint_waveform_bars(
                        &painter,
                        rect.shrink2(vec2(12.0, 14.0)),
                        waveform_samples,
                        start_x,
                        end_x,
                        None,
                    );

                    let selection = Rect::from_min_max(
                        Pos2::new(start_x, rect.top() + 10.0),
                        Pos2::new(end_x.max(start_x + 2.0), rect.bottom() - 10.0),
                    );
                    painter.rect_filled(
                        selection,
                        16.0,
                        if dark_theme {
                            Color32::from_rgba_premultiplied(227, 82, 149, 36)
                        } else {
                            Color32::from_rgba_premultiplied(227, 82, 149, 24)
                        },
                    );

                    let handle_stroke = Stroke::new(2.0, Color32::from_rgb(214, 51, 132));
                    painter.line_segment(
                        [
                            Pos2::new(start_x, rect.top() + 10.0),
                            Pos2::new(start_x, rect.bottom() - 10.0),
                        ],
                        handle_stroke,
                    );
                    painter.line_segment(
                        [
                            Pos2::new(end_x, rect.top() + 10.0),
                            Pos2::new(end_x, rect.bottom() - 10.0),
                        ],
                        handle_stroke,
                    );
                    painter.circle_filled(
                        Pos2::new(start_x, rect.center().y),
                        7.0,
                        Color32::from_rgb(214, 51, 132),
                    );
                    painter.circle_filled(
                        Pos2::new(end_x, rect.center().y),
                        7.0,
                        Color32::from_rgb(214, 51, 132),
                    );

                    let start_handle_rect = Rect::from_center_size(
                        Pos2::new(start_x, rect.center().y),
                        vec2(24.0, rect.height()),
                    );
                    let end_handle_rect = Rect::from_center_size(
                        Pos2::new(end_x, rect.center().y),
                        vec2(24.0, rect.height()),
                    );
                    let start_response = ui.interact(
                        start_handle_rect,
                        ui.make_persistent_id((sound.id, "trim-start")),
                        Sense::click_and_drag(),
                    );
                    let end_response = ui.interact(
                        end_handle_rect,
                        ui.make_persistent_id((sound.id, "trim-end")),
                        Sense::click_and_drag(),
                    );

                    let pointer_pos = interactive
                        .then(|| ui.ctx().input(|input| input.pointer.hover_pos()))
                        .flatten()
                        .filter(|pos| viewport_rect.contains(*pos));
                    let pointer_time = pointer_pos.map(|pointer| {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        ratio * duration
                    });
                    let playhead_outline = if dark_theme {
                        Color32::from_rgba_premultiplied(8, 13, 19, 224)
                    } else {
                        Color32::from_rgba_premultiplied(255, 255, 255, 232)
                    };
                    let playhead_color = if dark_theme {
                        Color32::from_rgb(108, 231, 255)
                    } else {
                        Color32::from_rgb(42, 39, 44)
                    };
                    let hover_playhead_color = if dark_theme {
                        Color32::from_rgba_premultiplied(108, 231, 255, 150)
                    } else {
                        Color32::from_rgba_premultiplied(42, 39, 44, 110)
                    };
                    let pan_left = interactive && ui.input(|input| input.key_down(egui::Key::A));
                    let pan_right = interactive && ui.input(|input| input.key_down(egui::Key::D));
                    let keyboard_panning = pan_left ^ pan_right;
                    let timeline_hovered =
                        interactive && (response.hovered() || pointer_pos.is_some());
                    let showing_hover_preview = pointer_pos.is_some()
                        && !keyboard_panning
                        && !start_response.is_pointer_button_down_on()
                        && !end_response.is_pointer_button_down_on()
                        && !response.dragged();

                    if showing_hover_preview && let Some(pointer) = pointer_pos {
                        painter.line_segment(
                            [
                                Pos2::new(pointer.x, rect.top() + 12.0),
                                Pos2::new(pointer.x, rect.bottom() - 12.0),
                            ],
                            Stroke::new(1.0, hover_playhead_color),
                        );
                        painter.circle_filled(
                            Pos2::new(pointer.x, rect.top() + 12.0),
                            4.0,
                            hover_playhead_color,
                        );
                        if let Some(pointer_time) = pointer_time {
                            let text_pos = Pos2::new(
                                (pointer.x + 8.0).clamp(rect.left() + 6.0, rect.right() - 56.0),
                                rect.top() + 12.0,
                            );
                            painter.text(
                                text_pos,
                                egui::Align2::LEFT_TOP,
                                format_time(pointer_time),
                                egui::FontId::proportional(11.5),
                                if dark_theme {
                                    Color32::from_rgb(208, 244, 255)
                                } else {
                                    Color32::from_rgb(42, 39, 44)
                                },
                            );
                        }
                    }

                    let cursor_ratio = (*preview_cursor_secs / duration).clamp(0.0, 1.0);
                    let cursor_x = rect.left() + rect.width() * cursor_ratio;
                    painter.line_segment(
                        [
                            Pos2::new(cursor_x, rect.top() + 8.0),
                            Pos2::new(cursor_x, rect.bottom() - 8.0),
                        ],
                        Stroke::new(4.0, playhead_outline),
                    );
                    painter.line_segment(
                        [
                            Pos2::new(cursor_x, rect.top() + 8.0),
                            Pos2::new(cursor_x, rect.bottom() - 8.0),
                        ],
                        Stroke::new(2.0, playhead_color),
                    );
                    painter.circle_filled(
                        Pos2::new(cursor_x, rect.top() + 10.0),
                        4.5,
                        playhead_color,
                    );

                    if timeline_hovered && keyboard_panning {
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                        let pan_speed = (viewport_rect.width() * 2.4).max(420.0);
                        let pan_step =
                            pan_speed * ui.input(|input| input.stable_dt).max(1.0 / 240.0);
                        let max_offset = (rect.width() - viewport_rect.width()).max(0.0);
                        let delta = match (pan_left, pan_right) {
                            (true, false) => -pan_step,
                            (false, true) => pan_step,
                            _ => 0.0,
                        };
                        let current_offset = requested_scroll_offset
                            .unwrap_or_else(|| (viewport_rect.left() - rect.left()).max(0.0));
                        requested_scroll_offset =
                            Some((current_offset + delta).clamp(0.0, max_offset));
                        ui.ctx().request_repaint();
                    }

                    if interactive && pointer_pos.is_some() && !ui.ctx().wants_keyboard_input() {
                        let zoom_delta = ui.input(|input| {
                            if input.modifiers.ctrl {
                                input.raw_scroll_delta.y
                            } else {
                                0.0
                            }
                        });
                        if zoom_delta.abs() > 0.0 {
                            let anchor_viewport_x = pointer_pos
                                .map(|pointer| {
                                    (pointer.x - viewport_rect.left())
                                        .clamp(0.0, viewport_rect.width())
                                })
                                .unwrap_or(viewport_rect.width() * cursor_ratio.clamp(0.0, 1.0));
                            let anchor_content_x = pointer_pos
                                .map(|pointer| (pointer.x - rect.left()).clamp(0.0, rect.width()))
                                .unwrap_or((cursor_ratio * rect.width()).clamp(0.0, rect.width()));
                            let factor = if zoom_delta > 0.0 { 1.12 } else { 1.0 / 1.12 };
                            *zoom = (*zoom * factor).clamp(1.0, 8.0);
                            let next_timeline_width = (viewport_width * *zoom).max(viewport_width);
                            let next_anchor_content_x =
                                (anchor_content_x / rect.width().max(1.0)) * next_timeline_width;
                            let max_offset = (next_timeline_width - viewport_width).max(0.0);
                            requested_scroll_offset = Some(
                                (next_anchor_content_x - anchor_viewport_x).clamp(0.0, max_offset),
                            );
                            ui.ctx().request_repaint();
                        }

                        let move_left = ui.input(|input| input.key_down(egui::Key::Q));
                        let move_right = ui.input(|input| input.key_down(egui::Key::W));

                        if let Some(pointer_time) = pointer_time {
                            if move_left {
                                sound.trim_start_secs =
                                    pointer_time.min(sound.trim_end_secs - 0.05);
                                sound.clamp_trim();
                                changed = true;
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(trim_hotkey_adjusting_id, true)
                                });
                            }
                            if move_right {
                                sound.trim_end_secs =
                                    pointer_time.max(sound.trim_start_secs + 0.05);
                                sound.clamp_trim();
                                changed = true;
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(trim_hotkey_adjusting_id, true)
                                });
                            }
                        }

                        if !move_left
                            && !move_right
                            && ui
                                .ctx()
                                .data(|data| data.get_temp::<bool>(trim_hotkey_adjusting_id))
                                .unwrap_or(false)
                        {
                            preview_commit_requested = true;
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_hotkey_adjusting_id));
                        }
                    }

                    if interactive
                        && duration > 0.0
                        && let Some(pointer) = start_response.interact_pointer_pos()
                        && (start_response.clicked() || start_response.dragged())
                    {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        let next = ratio * duration;
                        sound.trim_start_secs = next.min(sound.trim_end_secs - 0.05);
                        sound.clamp_trim();
                        changed = true;
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(trim_adjusting_id, true));
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if start_response.clicked() {
                            preview_commit_requested = true;
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    } else if interactive
                        && duration > 0.0
                        && let Some(pointer) = end_response.interact_pointer_pos()
                        && (end_response.clicked() || end_response.dragged())
                    {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        let next = ratio * duration;
                        sound.trim_end_secs = next.max(sound.trim_start_secs + 0.05);
                        sound.clamp_trim();
                        changed = true;
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(trim_adjusting_id, true));
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if end_response.clicked() {
                            preview_commit_requested = true;
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    } else if interactive
                        && !start_response.is_pointer_button_down_on()
                        && !end_response.is_pointer_button_down_on()
                        && duration > 0.0
                        && let Some(pointer) = response.interact_pointer_pos()
                        && (response.clicked() || response.dragged())
                    {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        *preview_cursor_secs =
                            (ratio * duration).clamp(sound.trim_start_secs, sound.trim_end_secs);
                        if response.clicked() {
                            seek_requested = true;
                        }
                        if response.dragged() {
                            ui.ctx()
                                .data_mut(|data| data.insert_temp(playhead_drag_id, true));
                        }
                    }

                    if interactive
                        && response.drag_stopped()
                        && ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(playhead_drag_id))
                            .unwrap_or(false)
                    {
                        seek_requested = true;
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                    }

                    if interactive
                        && (start_response.drag_stopped() || end_response.drag_stopped())
                        && ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                            .unwrap_or(false)
                    {
                        preview_commit_requested = true;
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                    }

                    if !interactive || !ui.input(|input| input.pointer.primary_down()) {
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                            .unwrap_or(false)
                        {
                            preview_commit_requested = true;
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    }

                    let trim_adjusting_active = ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                        .unwrap_or(false)
                        || ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_hotkey_adjusting_id))
                            .unwrap_or(false);

                    if clamp_cursor_to_trim {
                        let clamped_cursor = (*preview_cursor_secs)
                            .clamp(sound.trim_start_secs, sound.trim_end_secs);
                        if (clamped_cursor - *preview_cursor_secs).abs() > f32::EPSILON {
                            *preview_cursor_secs = clamped_cursor;
                            if trim_adjusting_active {
                                preview_commit_requested = true;
                            } else {
                                seek_requested = true;
                            }
                        }
                    }
                });
                let scroll_offset = requested_scroll_offset
                    .unwrap_or_else(|| scroll_output.state.offset.x.max(0.0));
                ui.ctx().data_mut(|data| {
                    data.insert_temp(zoom_scroll_offset_id, scroll_offset);
                });
            },
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if show_loading_indicator {
                ui.add(egui::Spinner::new().size(16.0));
                ui.add_space(8.0);
            }
            ui.label(
                RichText::new(format_time(sound.trim_start_secs))
                    .size(13.0)
                    .color(if dark_theme {
                        Color32::from_rgb(208, 196, 207)
                    } else {
                        Color32::from_rgb(118, 106, 116)
                    }),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.trimmed_length()))
                    .size(13.0)
                    .color(Color32::from_rgb(214, 51, 132)),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.trim_end_secs))
                    .size(13.0)
                    .color(if dark_theme {
                        Color32::from_rgb(208, 196, 207)
                    } else {
                        Color32::from_rgb(118, 106, 116)
                    }),
            );
        });

        (changed, seek_requested, preview_commit_requested)
    }

    fn render_tts_download_tab(&mut self, ui: &mut Ui, ctx: &Context) {
        let mut generate_request = false;
        let mut preview_request = false;
        let mut add_to_library = false;
        let mut clear_result = false;
        let mut save_gemini = false;
        let mut save_preset = false;
        let mut delete_preset = false;
        let mut draft_changed = false;
        let selected_voice_label = Self::gemini_voice_label(&self.tts_voice_name).to_owned();
        let selected_preset_name = self
            .selected_tts_preset_name()
            .map(str::to_owned)
            .unwrap_or_else(|| self.t("download.custom"));

        if self.tts_running {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }

        ui.label(
            RichText::new(self.t("download.gemini_tts"))
                .size(14.0)
                .color(Self::strong_text_color())
                .strong(),
        );
        ui.add_space(10.0);

        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(18.0)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                let column_gap = 14.0;
                let action_button_width = 30.0;
                let action_gap = 8.0;
                let column_width = ((ui.available_width() - column_gap) / 2.0).max(180.0);
                let label_size = 12.0;

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = column_gap;

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.voice"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        Self::with_dark_combo_visuals(ui, |ui| {
                            ComboBox::from_id_salt("gemini-tts-voice")
                                .width(column_width)
                                .selected_text(
                                    RichText::new(&selected_voice_label)
                                        .color(Self::strong_text_color()),
                                )
                                .show_ui(ui, |ui| {
                                    for voice in GEMINI_VOICE_OPTIONS {
                                        if ui
                                            .selectable_label(
                                                self.tts_voice_name == voice.name,
                                                Self::gemini_voice_label(voice.name),
                                            )
                                            .clicked()
                                        {
                                            self.tts_voice_name = voice.name.to_owned();
                                            draft_changed = true;
                                        }
                                    }
                                });
                        });
                    });

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.name"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        let name_response = ui.add_sized(
                            [column_width, 30.0],
                            TextEdit::singleline(&mut self.tts_output_name)
                                .hint_text("gemini tts")
                                .desired_width(f32::INFINITY),
                        );
                        draft_changed |= name_response.changed();
                    });
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = column_gap;

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.prompt_preset"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        Self::with_dark_combo_visuals(ui, |ui| {
                            ComboBox::from_id_salt("gemini-tts-preset")
                                .width(column_width)
                                .selected_text(
                                    RichText::new(selected_preset_name.clone())
                                        .color(Self::strong_text_color()),
                                )
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(
                                            self.selected_tts_preset_name().is_none(),
                                            self.t("download.custom"),
                                        )
                                        .clicked()
                                    {
                                        self.tts_preset_name.clear();
                                        draft_changed = true;
                                    }

                                    let preset_names = self
                                        .tts_prompt_presets
                                        .iter()
                                        .map(|preset| preset.name.clone())
                                        .collect::<Vec<_>>();
                                    for preset_name in preset_names {
                                        if ui
                                            .selectable_label(
                                                self.selected_tts_preset_name()
                                                    == Some(preset_name.as_str()),
                                                &preset_name,
                                            )
                                            .clicked()
                                        {
                                            self.apply_tts_preset_by_name(&preset_name);
                                            draft_changed = true;
                                        }
                                    }
                                });
                        });
                    });

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.preset_name"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = action_gap;
                            let preset_name_hint = self.t("download.preset_name");
                            let preset_response = ui.add_sized(
                                [
                                    (column_width - action_button_width * 2.0 - action_gap * 2.0)
                                        .max(84.0),
                                    30.0,
                                ],
                                TextEdit::singleline(&mut self.tts_preset_name)
                                    .hint_text(preset_name_hint),
                            );
                            draft_changed |= preset_response.changed();

                            let save = ui.add_sized(
                                [action_button_width, 30.0],
                                Self::action_button(RichText::new("+").size(16.0), false, false),
                            );
                            Self::decorate_button_response(ui, &save);
                            if save.clicked() {
                                save_preset = true;
                            }

                            let delete = ui.add_enabled(
                                self.selected_tts_preset_name().is_some(),
                                Self::action_button(RichText::new("x").size(15.0), false, false),
                            );
                            Self::decorate_button_response(ui, &delete);
                            if delete.clicked() {
                                delete_preset = true;
                            }
                        });
                    });
                });

                ui.add_space(10.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        if Self::gemini_api_key_field(
                            ui,
                            &self.t("download.gemini_api_key"),
                            &mut self.gemini_api_key,
                            &mut self.gemini_api_key_visible,
                        ) {
                            save_gemini = true;
                        }
                    });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.t("download.direction_prompt"))
                        .size(12.0)
                        .color(Self::muted_text_color()),
                );
                ui.add_space(6.0);
                let direction_hint = self.t("download.direction_hint");
                let direction_response = ui.add_sized(
                    [ui.available_width(), 96.0],
                    TextEdit::multiline(&mut self.tts_direction_prompt)
                        .desired_width(f32::INFINITY)
                        .hint_text(direction_hint),
                );
                draft_changed |= direction_response.changed();

                ui.add_space(10.0);
                let enter_text_hint = self.t("download.enter_text");
                let text_response = ui.add_sized(
                    [ui.available_width(), 130.0],
                    TextEdit::multiline(&mut self.tts_text)
                        .desired_width(f32::INFINITY)
                        .hint_text(enter_text_hint),
                );
                draft_changed |= text_response.changed();
            });

        if draft_changed {
            self.save_tts_draft_preferences();
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let generate = ui.add_enabled(
                !self.tts_running
                    && !self.tts_text.trim().is_empty()
                    && !self.gemini_api_key.trim().is_empty(),
                Self::action_button(
                    RichText::new(self.t("download.generate")).size(13.0),
                    false,
                    true,
                ),
            );
            Self::decorate_button_response(ui, &generate);
            if generate.clicked() {
                generate_request = true;
            }

            let preview = ui.add_enabled(
                self.tts_last_file.is_some() && !self.tts_running,
                Self::action_button(
                    RichText::new(self.t("download.preview")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &preview);
            if preview.clicked() {
                preview_request = true;
            }

            let add = ui.add_enabled(
                self.tts_can_add_to_library,
                Self::action_button(
                    RichText::new(self.t("download.add_to_library")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &add);
            if add.clicked() {
                add_to_library = true;
            }

            let clear = ui.add_enabled(
                self.tts_last_file.is_some() && !self.tts_running,
                Self::action_button(
                    RichText::new(self.t("download.clear")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &clear);
            if clear.clicked() {
                clear_result = true;
            }
        });

        if self.tts_running {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(18.0));
                ui.label(
                    RichText::new(self.t("download.generating_speech"))
                        .size(12.5)
                        .color(Self::muted_text_color()),
                );
            });
        } else if !self.gemini_api_key.trim().is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.t("download.gemini_tts_help"))
                    .size(12.0)
                    .color(Self::muted_text_color()),
            );
        }

        if self.gemini_api_key.trim().is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.t("download.gemini_api_key_missing"))
                    .size(12.5)
                    .color(Color32::from_rgb(171, 54, 91)),
            );
        } else if let Some(path) = &self.tts_last_file {
            ui.add_space(12.0);
            ui.label(
                RichText::new(
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("audio"),
                )
                .size(13.5)
                .color(Self::strong_text_color())
                .strong(),
            );
        }

        if let Some(error) = &self.tts_error {
            ui.add_space(10.0);
            ui.label(
                RichText::new(error)
                    .size(12.5)
                    .color(Color32::from_rgb(171, 54, 91)),
            );
        }

        if save_gemini {
            let _ = self.storage.save_gemini_api_key(&self.gemini_api_key);
        }
        if delete_preset {
            self.delete_selected_tts_preset();
            self.save_tts_draft_preferences();
        }
        if save_preset {
            self.save_current_tts_preset();
            self.save_tts_draft_preferences();
        }
        if generate_request {
            self.start_tts_generation();
        }
        if preview_request
            && let Some(path) = self.tts_last_file.clone()
            && let Some(audio) = self.audio.as_mut()
        {
            if let Err(error) = audio.play_file(&path) {
                self.set_error_status(error);
            }
        }
        if add_to_library && let Some(path) = self.tts_last_file.clone() {
            self.add_tts_result_to_library(&path);
        }
        if clear_result {
            if let Some(path) = self.tts_last_file.take() {
                let _ = fs::remove_file(path);
            }
            self.tts_status.clear();
            self.tts_error = None;
            self.tts_can_add_to_library = false;
            self.tts_added_to_library = false;
        }
    }

    fn render_download_panel(&mut self, ctx: &Context) {
        if !self.show_download_panel {
            return;
        }

        let snapshot = self.downloader.snapshot();
        let mut open_panel = self.show_download_panel;
        let mut should_start_download = false;
        let mut should_stop_download = false;
        let mut add_to_library = false;
        let mut open_file = false;
        let mut open_folder = false;
        let mut clear_result = false;
        let mut minimize_request = false;
        let mut close_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(520.0, 420.0), vec2(320.0, 260.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("youtube-audio-download"))
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
                    .inner_margin(Margin {
                        left: 20,
                        right: 12,
                        top: 12,
                        bottom: 20,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(34.0);
                    ui.label(Self::icon(0xe2c4, 20.0, Self::strong_text_color()).strong());
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if Self::icon_titlebar(ui, [34.0, 34.0], 0xe5cd, false, true).clicked() {
                            clear_result = !snapshot.running;
                            close_request = true;
                        }
                        if Self::icon_titlebar(ui, [34.0, 34.0], 0xe15b, false, false).clicked() {
                            minimize_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let download_tab = ui.add_sized(
                        [120.0, 32.0],
                        Self::action_button(
                            RichText::new(self.t("download.download")).size(12.5),
                            self.download_panel_tab == DownloadPanelTab::Download,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &download_tab);
                    if download_tab.clicked() {
                        self.download_panel_tab = DownloadPanelTab::Download;
                    }
                    let tts_tab = ui.add_sized(
                        [120.0, 32.0],
                        Self::action_button(
                            RichText::new(self.t("download.gemini_tts")).size(12.5),
                            self.download_panel_tab == DownloadPanelTab::Tts,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &tts_tab);
                    if tts_tab.clicked() {
                        self.download_panel_tab = DownloadPanelTab::Tts;
                    }
                });

                ui.add_space(12.0);
                if self.download_panel_tab == DownloadPanelTab::Tts {
                    self.render_tts_download_tab(ui, ctx);
                } else {
                    if snapshot.running {
                        ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
                    }

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Supporting web:")
                                .size(12.5)
                                .color(Self::muted_text_color()),
                        );
                        let help = ui.add_sized(
                            [22.0, 22.0],
                            Button::new(Self::icon(0xe887, 15.0, Color32::from_rgb(214, 51, 132)))
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(11.0),
                        );
                        if help.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
                        }
                        help.on_hover_ui_at_pointer(|ui| {
                            ui.set_max_width(300.0);
                            ui.label(
                                RichText::new("Supported websites")
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.add_space(4.0);
                            ui.label("Works through yt-dlp, so it supports many sites.");
                            ui.label("Common examples: YouTube, SoundCloud, Bandcamp, TikTok, Facebook, Instagram, X/Twitter, Vimeo, Dailymotion, Bilibili, Twitch, Google Drive, direct media links.");
                            ui.add_space(4.0);
                            ui.label("Some sites can still fail because of login, region lock, cookies, or DRM.");
                            ui.label("Spotify album / track links are usually DRM-protected and will not download.");
                        });
                    });
                    ui.add_space(8.0);
                    Self::render_download_site_badges(ui);

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        let response = Frame::new()
                            .fill(Self::input_fill())
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(16.0)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.add_sized(
                                    [ui.available_width() - 4.0, 22.0],
                                    TextEdit::singleline(&mut self.download_url)
                                        .frame(false)
                                        .hint_text("https://youtube.com/watch?v=... or soundcloud / tiktok / facebook")
                                        .desired_width(f32::INFINITY)
                                        .margin(Vec2::new(0.0, 4.0)),
                                )
                            })
                            .inner;
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && !snapshot.running
                        {
                            should_start_download = true;
                        }

                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        let start_button = ui.add_enabled(
                            !snapshot.running && !self.download_url.trim().is_empty(),
                            Self::action_button(
                                RichText::new(self.t("download.download_sound")).size(13.0),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &start_button);
                        if start_button.clicked() {
                            should_start_download = true;
                        }

                        if snapshot.running {
                            if Self::icon_action(ui, [42.0, 32.0], 0xe047, false, true).clicked() {
                                should_stop_download = true;
                            }
                            ui.label(
                                RichText::new(snapshot.stage.clone())
                                    .size(13.0)
                                    .color(Self::muted_text_color()),
                            );
                        }
                    });

                    if let Some(progress) = snapshot.progress {
                        ui.add_space(8.0);
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(ui.available_width())
                                .fill(Color32::from_rgb(227, 82, 149)),
                        );
                    } else if snapshot.running {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(18.0));
                            ui.label(
                                RichText::new(self.t("download.working"))
                                    .size(12.5)
                                    .color(Self::muted_text_color()),
                            );
                        });
                    }

                    if let Some(error) = &snapshot.error {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(error)
                                .size(13.0)
                                .color(Color32::from_rgb(171, 54, 91)),
                        );
                    }

                    if let Some(path) = &snapshot.last_file {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(
                                path.file_name()
                                    .and_then(|value| value.to_str())
                                    .unwrap_or("audio"),
                            )
                            .size(14.0)
                            .color(Self::strong_text_color())
                            .strong(),
                        );

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let add_response = ui.add_enabled(
                                snapshot.can_add_to_library,
                                Self::action_button(
                                    Self::icon(0xe02e, 18.0, Color32::WHITE),
                                    false,
                                    true,
                                ),
                            );
                            Self::decorate_button_response(ui, &add_response);
                            if add_response.clicked() {
                                add_to_library = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe89e, false, false).clicked() {
                                open_file = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe2c8, false, false).clicked() {
                                open_folder = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe14c, false, false).clicked() {
                                clear_result = true;
                            }
                        });
                    }
                }
            });

        if close_request {
            open_panel = false;
        }
        if minimize_request {
            open_panel = false;
        }
        self.show_download_panel = open_panel;

        if should_start_download {
            match self
                .downloader
                .start_audio_download(self.download_url.trim().to_owned())
            {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if should_stop_download {
            self.downloader.stop_audio_download();
        }

        if let Some(path) = snapshot.last_file.clone() {
            if add_to_library {
                self.import_paths(vec![path.clone()]);
                self.downloader.mark_added_to_library();
            }
            if open_file {
                if let Err(error) = self.downloader.open_file(&path) {
                    self.set_error_status(error);
                }
            }
            if open_folder {
                if let Err(error) = self.downloader.open_folder(&path) {
                    self.set_error_status(error);
                }
            }
        }

        if clear_result {
            self.downloader.clear_result();
        }
    }

    fn render_myinstants_panel(&mut self, ctx: &Context) {
        if !self.show_myinstants_panel {
            return;
        }

        let snapshot = self.myinstants.snapshot();
        let youtube_snapshot = self.downloader.snapshot();
        let youtube_results = youtube_snapshot.youtube_results.clone();
        let was_open = self.show_myinstants_panel;
        let mut open_panel = self.show_myinstants_panel;
        let mut close_request = false;
        let mut search_request = false;
        let mut youtube_search_request = false;
        let mut add_request: Option<MyinstantsResult> = None;
        let mut download_request: Option<MyinstantsResult> = None;
        let mut preview_request: Option<MyinstantsResult> = None;
        let mut folder_request: Option<MyinstantsResult> = None;
        let mut copy_request: Option<MyinstantsResult> = None;
        let mut youtube_download_request: Option<String> = None;
        let mut stop_youtube_download = false;
        let mut clear_youtube_results = false;
        let mut more_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(720.0, 620.0), vec2(360.0, 300.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("myinstants-search-panel"))
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
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe8b6, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let button_group_width = 236.0;
                    let search_placeholder = self.t("download.search_placeholder");
                    let response = ui.add_sized(
                        [(ui.available_width() - button_group_width).max(180.0), 42.0],
                        TextEdit::singleline(&mut self.myinstants_query)
                            .hint_text(search_placeholder)
                            .margin(Vec2::new(14.0, 12.0)),
                    );
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        search_request = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let youtube_button = Self::youtube_search_button(
                            ui,
                            &self.t("download.search_youtube"),
                            !snapshot.searching
                                && !snapshot.downloading
                                && !youtube_snapshot.running
                                && !youtube_snapshot.searching
                                && !self.myinstants_query.trim().is_empty(),
                        );
                        if youtube_button.clicked() {
                            youtube_search_request = true;
                        }
                        ui.label(
                            RichText::new(self.t("download.or"))
                                .size(12.5)
                                .color(Self::muted_text_color())
                                .strong(),
                        );
                        if Self::search_sound_button(
                            ui,
                            !snapshot.searching
                                && !snapshot.downloading
                                && !self.myinstants_query.trim().is_empty(),
                        )
                        .clicked()
                        {
                            search_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                if snapshot.searching
                    || snapshot.downloading
                    || youtube_snapshot.searching
                    || youtube_snapshot.running
                {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(
                            RichText::new(
                                if youtube_snapshot.searching || youtube_snapshot.running {
                                    youtube_snapshot.stage.as_str()
                                } else {
                                    "..."
                                },
                            )
                            .size(14.0)
                            .color(Color32::from_rgb(214, 51, 132)),
                        );
                        if youtube_snapshot.running
                            && Self::icon_action(ui, [42.0, 30.0], 0xe047, false, true).clicked()
                        {
                            stop_youtube_download = true;
                        }
                    });
                    ui.add_space(8.0);
                }
                if let Some(error) = snapshot
                    .error
                    .as_deref()
                    .or(youtube_snapshot.error.as_deref())
                {
                    ui.label(
                        RichText::new(Self::truncate_middle_ascii(error, 80))
                            .size(12.0)
                            .color(Color32::from_rgb(189, 62, 117)),
                    );
                    ui.add_space(8.0);
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !youtube_results.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("YouTube")
                                        .size(14.0)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                    if Self::icon_action(ui, [42.0, 32.0], 0xe14c, false, false)
                                        .clicked()
                                    {
                                        clear_youtube_results = true;
                                    }
                                });
                            });
                            ui.add_space(8.0);
                            for result in youtube_results
                                .iter()
                                .take(self.youtube_search_visible_count)
                            {
                                if Self::render_youtube_result_row(
                                    ui,
                                    result,
                                    &self.t("download.download"),
                                ) {
                                    youtube_download_request = Some(result.webpage_url.clone());
                                }
                                ui.add_space(8.0);
                            }
                            ui.add_space(12.0);
                        }

                        for result in snapshot.results.iter().take(self.myinstants_visible_count) {
                            self.queue_myinstants_waveform_prefetch(result);
                            let downloaded_path =
                                self.existing_myinstants_download_path(&result.audio_url);
                            let preview_path = downloaded_path.clone().or_else(|| {
                                self.existing_myinstants_preview_path(&result.audio_url)
                            });
                            let is_downloaded = downloaded_path.is_some();
                            let is_previewing = preview_path.as_ref().is_some_and(|path| {
                                self.audio
                                    .as_ref()
                                    .is_some_and(|audio| audio.is_playing_file(path))
                            });
                            let preview_progress = preview_path.as_ref().and_then(|path| {
                                self.audio
                                    .as_ref()
                                    .and_then(|audio| audio.playback_progress_for_file(path))
                            });
                            Frame::new()
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(22.0)
                                .inner_margin(Margin::same(14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            [ui.available_width() - 176.0, 20.0],
                                            egui::Label::new(
                                                RichText::new(&result.title)
                                                    .size(13.5)
                                                    .color(Self::strong_text_color())
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );
                                        if Self::icon_action(
                                            ui,
                                            [44.0, 32.0],
                                            if is_previewing { 0xe047 } else { 0xe037 },
                                            is_previewing,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            preview_request = Some(result.clone());
                                        }
                                        if is_downloaded {
                                            if Self::icon_action(
                                                ui,
                                                [44.0, 32.0],
                                                0xe2c7,
                                                false,
                                                false,
                                            )
                                            .clicked()
                                            {
                                                folder_request = Some(result.clone());
                                            }
                                            if Self::icon_action(
                                                ui,
                                                [44.0, 32.0],
                                                0xe14d,
                                                false,
                                                false,
                                            )
                                            .clicked()
                                            {
                                                copy_request = Some(result.clone());
                                            }
                                        } else if Self::icon_action(
                                            ui,
                                            [44.0, 32.0],
                                            0xe2c4,
                                            false,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            download_request = Some(result.clone());
                                        }
                                        if Self::icon_action(ui, [44.0, 32.0], 0xe145, false, true)
                                            .clicked()
                                        {
                                            add_request = Some(result.clone());
                                        }
                                    });

                                    ui.add_space(10.0);
                                    let waveform = self
                                        .myinstants_waveforms
                                        .get(&result.audio_url)
                                        .map(Vec::as_slice)
                                        .unwrap_or(&[]);
                                    Self::draw_wave_strip(
                                        ui,
                                        waveform,
                                        if is_previewing {
                                            preview_progress
                                        } else {
                                            None
                                        },
                                        Color32::from_rgb(214, 51, 132),
                                        if self.dark_theme {
                                            Color32::from_rgb(102, 74, 102)
                                        } else {
                                            Color32::from_rgb(238, 213, 227)
                                        },
                                        Self::panel_fill(),
                                        46.0,
                                    );
                                });
                            ui.add_space(10.0);
                        }

                        if snapshot.results.len() > self.myinstants_visible_count {
                            ui.add_space(2.0);
                            ui.horizontal_centered(|ui| {
                                let more_response = ui.add_sized(
                                    [76.0, 34.0],
                                    Self::action_button(
                                        RichText::new("+10")
                                            .size(13.0)
                                            .color(Self::strong_text_color()),
                                        false,
                                        false,
                                    ),
                                );
                                Self::decorate_button_response(ui, &more_response);
                                if more_response.clicked() {
                                    more_request = true;
                                }
                            });
                        }
                    });
            });

        if close_request {
            open_panel = false;
        }
        self.show_myinstants_panel = open_panel;
        if was_open && !open_panel && self.myinstants_preview_audio_url.is_some() {
            self.stop_preview();
        }

        if search_request {
            match self.myinstants.start_search(self.myinstants_query.clone()) {
                Ok(()) => {
                    self.myinstants_visible_count = 10;
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
        if youtube_search_request {
            match self
                .downloader
                .start_youtube_search(self.myinstants_query.clone())
            {
                Ok(()) => {
                    self.youtube_search_visible_count = 8;
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
        if more_request {
            self.myinstants_visible_count =
                (self.myinstants_visible_count + 10).min(snapshot.results.len());
        }
        if let Some(url) = youtube_download_request {
            self.download_url = url.clone();
            match self.downloader.start_audio_download(url) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if stop_youtube_download {
            self.downloader.stop_audio_download();
        }
        if clear_youtube_results {
            self.downloader.clear_youtube_results();
        }
        if let Some(result) = preview_request {
            match self.toggle_myinstants_preview(&result) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = download_request {
            match self.myinstants.start_download(result, false) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = folder_request
            && let Some(path) = self.existing_myinstants_download_path(&result.audio_url)
            && let Some(parent) = path.parent()
            && let Err(error) = open::that(parent)
        {
            self.set_error_status(error);
        }
        if let Some(result) = copy_request
            && let Some(path) = self.existing_myinstants_download_path(&result.audio_url)
        {
            match self.copy_file_path_to_clipboard(&path) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = add_request {
            if let Some(path) = self.existing_myinstants_download_path(&result.audio_url) {
                self.import_downloaded_sound(&path, false);
                self.clear_status();
            } else {
                match self.myinstants.start_download(result, true) {
                    Ok(()) => self.clear_status(),
                    Err(error) => self.set_error_status(error),
                }
            }
        }
    }

    fn render_trim_commit_panel(&mut self, ctx: &Context) {
        if !self.show_trim_commit_panel {
            return;
        }

        let mut open_panel = self.show_trim_commit_panel;
        let mut close_request = false;
        let mut keep_old = false;
        let mut replace_current = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(360.0, 180.0), vec2(280.0, 160.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("trim-commit-panel"))
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
                    ui.label(Self::icon(0xe14e, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(
                    RichText::new("Keep old file?")
                        .size(15.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );

                ui.add_space(12.0);
                ui.horizontal_centered(|ui| {
                    let keep_response = ui.add_sized(
                        [120.0, 38.0],
                        Self::action_button(RichText::new("Yes").size(13.0), false, true),
                    );
                    Self::decorate_button_response(ui, &keep_response);
                    if keep_response.clicked() {
                        keep_old = true;
                    }

                    let replace_response = ui.add_sized(
                        [120.0, 38.0],
                        Self::action_button(RichText::new("No").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &replace_response);
                    if replace_response.clicked() {
                        replace_current = true;
                    }
                });
            });

        if close_request {
            open_panel = false;
        }
        self.show_trim_commit_panel = open_panel;

        if keep_old {
            self.show_trim_commit_panel = false;
            self.duplicate_selected_trimmed_sound(ctx);
        }
        if replace_current {
            self.show_trim_commit_panel = false;
            self.commit_selected_trimmed_sound(ctx);
        }
    }

    fn render_delete_folder_confirm_panel(&mut self, ctx: &Context) {
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

    fn download_site_badges() -> [DownloadSiteBadge; 10] {
        [
            DownloadSiteBadge {
                name: "YouTube",
                kind: DownloadSiteKind::Youtube,
                color: Color32::from_rgb(255, 77, 141),
            },
            DownloadSiteBadge {
                name: "SoundCloud",
                kind: DownloadSiteKind::SoundCloud,
                color: Color32::from_rgb(255, 124, 72),
            },
            DownloadSiteBadge {
                name: "Bandcamp",
                kind: DownloadSiteKind::Bandcamp,
                color: Color32::from_rgb(45, 156, 219),
            },
            DownloadSiteBadge {
                name: "TikTok",
                kind: DownloadSiteKind::TikTok,
                color: Color32::from_rgb(24, 19, 30),
            },
            DownloadSiteBadge {
                name: "Facebook",
                kind: DownloadSiteKind::Facebook,
                color: Color32::from_rgb(24, 119, 242),
            },
            DownloadSiteBadge {
                name: "Instagram",
                kind: DownloadSiteKind::Instagram,
                color: Color32::from_rgb(193, 53, 132),
            },
            DownloadSiteBadge {
                name: "X / Twitter",
                kind: DownloadSiteKind::X,
                color: Color32::from_rgb(16, 16, 16),
            },
            DownloadSiteBadge {
                name: "Vimeo",
                kind: DownloadSiteKind::Vimeo,
                color: Color32::from_rgb(25, 183, 234),
            },
            DownloadSiteBadge {
                name: "Twitch",
                kind: DownloadSiteKind::Twitch,
                color: Color32::from_rgb(145, 70, 255),
            },
            DownloadSiteBadge {
                name: "Google Drive",
                kind: DownloadSiteKind::GoogleDrive,
                color: Color32::from_rgb(52, 168, 83),
            },
        ]
    }
}

mod controls;
mod downloader;
mod editor;
mod layout;
mod library;
mod message;
mod pitch_monitor;
mod settings;
mod state;
mod transition_state;
mod transition_view;
mod update;
mod view;
mod waveform;

fn load_ppm_color_image(path: &Path) -> Result<(egui::ColorImage, Vec2)> {
    let bytes = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut index = 0usize;
    let magic = next_ppm_token(&bytes, &mut index).context("invalid ppm header")?;
    if magic != "P6" {
        anyhow::bail!("unsupported ppm format");
    }
    let width = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm width")?
        .parse::<usize>()
        .context("invalid ppm width")?;
    let height = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm height")?
        .parse::<usize>()
        .context("invalid ppm height")?;
    let max_value = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm max value")?
        .parse::<usize>()
        .context("invalid ppm max value")?;
    if max_value != 255 {
        anyhow::bail!("unsupported ppm color depth");
    }

    if index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("ppm frame too large")?;
    let data = bytes
        .get(index..index + expected)
        .context("ppm frame payload is incomplete")?;
    let image = egui::ColorImage::from_rgb([width, height], data);
    Ok((image, vec2(width as f32, height as f32)))
}

fn next_ppm_token<'a>(bytes: &'a [u8], index: &mut usize) -> Option<&'a str> {
    while *index < bytes.len() {
        let byte = bytes[*index];
        if byte == b'#' {
            while *index < bytes.len() && bytes[*index] != b'\n' {
                *index += 1;
            }
        } else if byte.is_ascii_whitespace() {
            *index += 1;
        } else {
            break;
        }
    }

    let start = *index;
    while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
    std::str::from_utf8(bytes.get(start..*index)?).ok()
}

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            AUDIO_FILTERS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}
