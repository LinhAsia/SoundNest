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
mod media_panels;
mod message;
mod pitch_monitor;
mod pitch_overlay_view;
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
