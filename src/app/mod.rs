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
