use crate::audio::AudioEngine;
use crate::downloader::{YoutubeAudioDownloader, YoutubeSearchResult};
use crate::hotkey::GlobalHotkeyManager;
use crate::myinstants::{MyinstantsClient, MyinstantsResult};
use crate::pitch::{
    PitchInputSource, PitchMonitor, PitchMonitorConfig, PitchSnapshot, analyze_pitch_file,
    list_capture_devices,
};
use crate::platform;
use crate::record_video;
use crate::recorder::{Recorder, RecorderConfig};
use crate::storage::{SoundEffect, Storage, VideoAsset, format_time};
use anyhow::{Context as _, Result};
#[cfg(windows)]
use clipboard_win::{Clipboard, Setter, formats::FileList};
use eframe::egui::{
    self, Align, Button, CentralPanel, Color32, ComboBox, Context, CornerRadius, FontFamily, Frame,
    Margin, Pos2, ProgressBar, Rect, RichText, ScrollArea, Sense, Slider, Stroke, StrokeKind,
    TextEdit, TextureHandle, Ui, Vec2, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId,
    vec2,
};
use eframe::epaint::Shadow;
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

const AUDIO_FILTERS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];
const APP_FRAME_RADIUS: f32 = 30.0;
const APP_OUTER_MARGIN: f32 = 0.0;
const LIVE_UI_FADE_SEC: f32 = 0.32;
const TRANSITION_POINT_COUNT: usize = 240;
const MATERIAL_ICONS_FONT: &str = "material_icons";
const PITCH_OVERLAY_ID: &str = "pitch-monitor-overlay";
const PITCH_OVERLAY_TITLE: &str = "Sound FX Pitch Overlay";
const RECORD_OVERLAY_ID: &str = "record-monitor-overlay";
const RECORD_OVERLAY_TITLE: &str = "Sound FX Record Overlay";
const ACTIVE_UI_REPAINT_MS: u64 = 33;
const JOB_POLL_REPAINT_MS: u64 = 90;
const DEFAULT_INTRO_DURATION_SEC: f32 = 1.35;
const DEFAULT_OUTRO_DURATION_SEC: f32 = 0.72;
const TRANSITION_WAVE_BUCKETS: usize = 160;
const RECORD_EXPORT_VIDEO_FPS_OPTIONS: [u32; 3] = [
    record_video::LOW_VIDEO_FPS,
    record_video::STANDARD_VIDEO_FPS,
    record_video::HIGH_VIDEO_FPS,
];
static DARK_THEME_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppView {
    Editor,
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryTab {
    Sounds,
    Videos,
}

struct RecordingDraft {
    sound: SoundEffect,
    source_path: PathBuf,
}

struct VideoViewerState {
    video: VideoAsset,
    frame_paths: Vec<PathBuf>,
    audio_path: PathBuf,
    progress: f32,
    current_frame: Option<(usize, TextureHandle, Vec2)>,
}

struct RecordVideoExportState {
    progress: f32,
    stage: String,
    receiver: Receiver<RecordVideoExportMessage>,
}

struct RecordVideoExportResult {
    processed_audio_path: PathBuf,
    video_path: PathBuf,
    duration_secs: f32,
    video_fps: u32,
    video_name: String,
}

enum RecordVideoExportMessage {
    Progress { progress: f32, stage: String },
    Finished(Result<RecordVideoExportResult, String>),
}

enum MyinstantsWaveformMessage {
    Ready {
        audio_url: String,
        path: PathBuf,
        waveform: Vec<f32>,
    },
    Failed {
        audio_url: String,
        error: String,
    },
}

#[derive(Clone, Copy)]
struct DownloadSiteBadge {
    name: &'static str,
    kind: DownloadSiteKind,
    color: Color32,
}

#[derive(Clone, Copy)]
enum DownloadSiteKind {
    Youtube,
    SoundCloud,
    Bandcamp,
    TikTok,
    Facebook,
    Instagram,
    X,
    Vimeo,
    Twitch,
    GoogleDrive,
}

pub struct SoundFxApp {
    storage: Storage,
    audio: Option<AudioEngine>,
    sounds: Vec<SoundEffect>,
    selected: Option<Uuid>,
    status: Option<String>,
    downloader: YoutubeAudioDownloader,
    myinstants: MyinstantsClient,
    download_url: String,
    myinstants_query: String,
    youtube_search_visible_count: usize,
    myinstants_visible_count: usize,
    myinstants_cached_files: HashMap<String, PathBuf>,
    myinstants_preview_files: HashMap<String, PathBuf>,
    myinstants_waveforms: HashMap<String, Vec<f32>>,
    myinstants_waveform_jobs: HashSet<String>,
    myinstants_waveform_tx: Sender<MyinstantsWaveformMessage>,
    myinstants_waveform_rx: Receiver<MyinstantsWaveformMessage>,
    myinstants_preview_audio_url: Option<String>,
    show_download_panel: bool,
    download_was_running: bool,
    show_myinstants_panel: bool,
    show_import_panel: bool,
    show_record_panel: bool,
    show_record_review_panel: bool,
    show_pitch_panel: bool,
    show_settings_panel: bool,
    show_trim_commit_panel: bool,
    import_dir: PathBuf,
    import_audio_entries: Vec<PathBuf>,
    app_view: AppView,
    library_tab: LibraryTab,
    recorder: Recorder,
    record_name: String,
    record_input_source: PitchInputSource,
    record_capture_devices: Vec<String>,
    selected_record_input_device: Option<String>,
    record_hotkey: Option<egui::Key>,
    capture_record_hotkey: bool,
    record_hotkey_manager: GlobalHotkeyManager,
    record_export_video_sharps: bool,
    record_export_video_animation: bool,
    record_export_video_fps: u32,
    center_record_overlay_next_frame: bool,
    record_overlay_native_visuals_applied: bool,
    library_grid_scale: f32,
    video_assets: Vec<VideoAsset>,
    recording_draft: Option<RecordingDraft>,
    active_record_video_export: Option<RecordVideoExportState>,
    video_viewer: Option<VideoViewerState>,
    pitch_monitor: PitchMonitor,
    pitch_update_hz: f32,
    pitch_overlay_animation: bool,
    pitch_show_sharps: bool,
    pitch_input_source: PitchInputSource,
    pitch_capture_devices: Vec<String>,
    selected_pitch_input_device: Option<String>,
    center_pitch_overlay_next_frame: bool,
    pitch_overlay_native_visuals_applied: bool,
    startup: StartupSplashState,
    center_window_next_frame: bool,
    titlebar_drag_rect: Option<Rect>,
    native_shadow_applied: bool,
    transition_window_topmost_applied: bool,
    record_overlay_open: bool,
    trim_timeline_zoom: f32,
    preview_cursor: Option<(Uuid, f32)>,
    dark_theme: bool,
    startup_sound_name: Option<String>,
    exit_sound_name: Option<String>,
    settings_startup_candidate: Option<Uuid>,
    settings_exit_candidate: Option<Uuid>,
    library_audio_query: String,
    library_video_query: String,
    pending_sound_drag: Option<Uuid>,
    suppress_sound_drag_until_release: bool,
    ignored_drop_path: Option<PathBuf>,
    reveal_record_review_on_open: bool,
    startup_sound_played: bool,
    pending_save: bool,
    last_edit_at: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransitionPhase {
    Intro,
    Live,
    Outro,
}

struct StartupSplashState {
    phase: TransitionPhase,
    started_at: Option<f64>,
    live_started_at: Option<f64>,
    duration_sec: f32,
    close_sent: bool,
    sound_waveform: Vec<f32>,
    sound_duration_sec: f32,
}

impl SoundFxApp {
    pub fn new() -> Self {
        let storage = Storage::new().unwrap_or_else(|error| panic!("Storage init failed: {error}"));
        let mut status = None;
        let mut sounds = storage.load_library().unwrap_or_else(|error| {
            status = Some(error.to_string());
            Vec::new()
        });

        let mut hydrated = false;
        for sound in &mut sounds {
            if let Ok(changed) = storage.hydrate_sound(sound) {
                hydrated |= changed;
            }
        }
        if hydrated {
            let _ = storage.save_library(&sounds);
        }

        let audio = AudioEngine::new().ok();
        if audio.is_none() && status.is_none() {
            status = Some("Audio unavailable".to_owned());
        }

        let downloader = YoutubeAudioDownloader::new(storage.root_dir()).unwrap_or_else(|error| {
            panic!("Downloader init failed: {error}");
        });
        let myinstants = MyinstantsClient::new(storage.root_dir()).unwrap_or_else(|error| {
            panic!("Myinstants init failed: {error}");
        });
        let (myinstants_waveform_tx, myinstants_waveform_rx) = mpsc::channel();
        let video_assets = storage.load_video_library().unwrap_or_else(|error| {
            status = Some(error.to_string());
            Vec::new()
        });
        let import_dir = storage.load_import_dir().ok().flatten().unwrap_or_default();
        let pitch_update_hz = storage.load_pitch_update_hz().ok().flatten().unwrap_or(4.0);
        let pitch_overlay_animation = storage
            .load_overlay_animation()
            .ok()
            .flatten()
            .unwrap_or(true);
        let library_grid_scale = storage
            .load_library_grid_scale()
            .ok()
            .flatten()
            .unwrap_or(0.86);
        let dark_theme = storage.load_dark_theme().ok().flatten().unwrap_or(false);
        let pitch_show_sharps = storage
            .load_pitch_show_sharps()
            .ok()
            .flatten()
            .unwrap_or(false);
        let pitch_capture_devices = list_capture_devices().unwrap_or_default();
        let selected_pitch_input_device = pitch_capture_devices.first().cloned();
        let record_capture_devices = pitch_capture_devices.clone();
        let selected_record_input_device = record_capture_devices.first().cloned();
        let record_hotkey = storage
            .load_record_hotkey()
            .ok()
            .flatten()
            .and_then(|value| Self::parse_key_name(&value));
        let startup_sound_name = storage.load_startup_sound_name().ok().flatten();
        let exit_sound_name = storage.load_exit_sound_name().ok().flatten();
        let resolved_startup_sound = storage.resolved_startup_sound_path().ok().flatten();
        let startup_transition_duration_sec = Self::custom_transition_duration_secs_opt(
            &storage,
            resolved_startup_sound.as_deref(),
            DEFAULT_INTRO_DURATION_SEC,
        );
        let startup_transition_sound =
            Self::load_transition_sound_visual(&storage, resolved_startup_sound);

        Self {
            storage,
            audio,
            sounds,
            selected: None,
            status,
            downloader,
            myinstants,
            download_url: String::new(),
            myinstants_query: String::new(),
            youtube_search_visible_count: 8,
            myinstants_visible_count: 10,
            myinstants_cached_files: HashMap::new(),
            myinstants_preview_files: HashMap::new(),
            myinstants_waveforms: HashMap::new(),
            myinstants_waveform_jobs: HashSet::new(),
            myinstants_waveform_tx,
            myinstants_waveform_rx,
            myinstants_preview_audio_url: None,
            show_download_panel: false,
            download_was_running: false,
            show_myinstants_panel: false,
            show_import_panel: false,
            show_record_panel: false,
            show_record_review_panel: false,
            show_pitch_panel: false,
            show_settings_panel: false,
            show_trim_commit_panel: false,
            import_dir,
            import_audio_entries: Vec::new(),
            app_view: AppView::Editor,
            library_tab: LibraryTab::Sounds,
            recorder: Recorder::new(),
            record_name: "recording".to_owned(),
            record_input_source: PitchInputSource::System,
            record_capture_devices,
            selected_record_input_device,
            record_hotkey,
            capture_record_hotkey: false,
            record_hotkey_manager: GlobalHotkeyManager::new(),
            record_export_video_sharps: pitch_show_sharps,
            record_export_video_animation: pitch_overlay_animation,
            record_export_video_fps: record_video::STANDARD_VIDEO_FPS,
            center_record_overlay_next_frame: false,
            record_overlay_native_visuals_applied: false,
            library_grid_scale,
            video_assets,
            recording_draft: None,
            active_record_video_export: None,
            video_viewer: None,
            pitch_monitor: PitchMonitor::new(),
            pitch_update_hz,
            pitch_overlay_animation,
            pitch_show_sharps,
            pitch_input_source: PitchInputSource::System,
            pitch_capture_devices,
            selected_pitch_input_device,
            center_pitch_overlay_next_frame: false,
            pitch_overlay_native_visuals_applied: false,
            startup: StartupSplashState {
                phase: TransitionPhase::Intro,
                started_at: None,
                live_started_at: None,
                duration_sec: startup_transition_duration_sec,
                close_sent: false,
                sound_waveform: startup_transition_sound.0,
                sound_duration_sec: startup_transition_sound.1,
            },
            center_window_next_frame: true,
            titlebar_drag_rect: None,
            native_shadow_applied: false,
            transition_window_topmost_applied: false,
            record_overlay_open: false,
            trim_timeline_zoom: 1.0,
            preview_cursor: None,
            dark_theme,
            startup_sound_name,
            exit_sound_name,
            settings_startup_candidate: None,
            settings_exit_candidate: None,
            library_audio_query: String::new(),
            library_video_query: String::new(),
            pending_sound_drag: None,
            suppress_sound_drag_until_release: false,
            ignored_drop_path: None,
            reveal_record_review_on_open: false,
            startup_sound_played: false,
            pending_save: false,
            last_edit_at: 0.0,
        }
        .with_initial_selection()
    }

    fn with_initial_selection(mut self) -> Self {
        self.selected = self.sounds.first().map(|sound| sound.id);
        let _ = self.record_hotkey_manager.set_hotkey(self.record_hotkey);
        self
    }

    fn clear_status(&mut self) {
        self.status = None;
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
        vec2(900.0, 900.0)
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

    fn request_close(&mut self, ctx: &Context) {
        if self.startup.phase == TransitionPhase::Outro {
            return;
        }

        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        if self.recorder.snapshot().running {
            self.stop_recording_for_close(ctx);
        }
        let exit_sound_path = self.storage.resolved_exit_sound_path().ok().flatten();
        if let Some(path) = exit_sound_path.as_ref() {
            let _ = self.play_file_detached_if_exists(path);
        }
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }

        let exit_transition_sound =
            Self::load_transition_sound_visual(&self.storage, exit_sound_path.clone());
        self.startup.phase = TransitionPhase::Outro;
        self.startup.started_at = None;
        self.startup.live_started_at = None;
        self.startup.duration_sec = Self::custom_transition_duration_secs_opt(
            &self.storage,
            exit_sound_path.as_deref(),
            DEFAULT_OUTRO_DURATION_SEC,
        );
        self.startup.sound_waveform = exit_transition_sound.0;
        self.startup.sound_duration_sec = exit_transition_sound.1;
        ctx.request_repaint();
    }

    fn stop_recording_for_close(&mut self, ctx: &Context) {
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        ctx.send_viewport_cmd_to(Self::record_overlay_viewport_id(), ViewportCommand::Close);
        if let Some(path) = self.recorder.take_completed_path() {
            let _ = fs::remove_file(path);
        }
    }

    fn finalize_close_cleanup(&mut self, ctx: &Context) {
        self.show_download_panel = false;
        self.close_recording_review(true);
        self.video_viewer = None;
        self.pitch_monitor.stop();
        self.pitch_overlay_native_visuals_applied = false;
        ctx.send_viewport_cmd_to(Self::pitch_overlay_viewport_id(), ViewportCommand::Close);
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        ctx.send_viewport_cmd_to(Self::record_overlay_viewport_id(), ViewportCommand::Close);
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

    fn play_file_detached_if_exists(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let exe_path = std::env::current_exe().context("unable to resolve current executable")?;
        let mut cmd = Command::new(exe_path);
        cmd.arg("--play-file-detached").arg(path);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        cmd.spawn()
            .context("unable to launch detached audio player")?;
        Ok(())
    }

    fn playback_needs_live_repaint(&self) -> bool {
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        if self.myinstants_preview_audio_url.is_some() {
            return true;
        }
        if let Some(viewer) = self.video_viewer.as_ref()
            && audio.is_playing_file(&viewer.audio_path)
        {
            return true;
        }
        if self.show_record_review_panel
            && let Some(draft) = self.recording_draft.as_ref()
            && audio.is_playing(draft.sound.id)
        {
            return true;
        }
        if let Some(selected) = self.selected
            && audio.is_playing(selected)
        {
            return true;
        }
        false
    }

    fn play_startup_sound_if_needed(&mut self, ctx: &Context) {
        if self.startup_sound_played || self.startup.phase != TransitionPhase::Intro {
            return;
        }

        if let Ok(Some(path)) = self.storage.resolved_startup_sound_path() {
            let _ = self.play_file_if_exists(&path);
        }
        self.startup_sound_played = true;
        self.startup.started_at = Some(ctx.input(|input| input.time));
    }

    fn handle_space_preview(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            return;
        }

        if self.video_viewer.is_some() {
            self.toggle_video_viewer_playback();
            return;
        }

        if self.show_record_review_panel {
            let Some(sound) = self
                .recording_draft
                .as_ref()
                .map(|draft| draft.sound.clone())
            else {
                return;
            };
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(sound.id))
            {
                self.stop_preview();
                return;
            }
            let mut cursor_secs = self.preview_cursor_secs_for(&sound);
            if cursor_secs >= sound.trim_end_secs - 0.02 {
                cursor_secs = sound.trim_start_secs;
                self.set_preview_cursor_secs(sound.id, cursor_secs, sound.safe_duration());
            }
            self.preview_recording_draft_from_position(Some(cursor_secs));
            return;
        }

        if self.has_modal_panel() {
            return;
        }

        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound = self.sounds[index].clone();
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound.id))
        {
            self.stop_preview();
            return;
        }
        let mut cursor_secs = self.preview_cursor_secs_for(&sound);
        if cursor_secs >= sound.trim_end_secs - 0.02 {
            cursor_secs = sound.trim_start_secs;
            self.set_preview_cursor_secs(sound.id, cursor_secs, sound.safe_duration());
        }
        self.preview_sound_from_position(sound.id, Some(cursor_secs));
    }

    fn intercept_close_request(&mut self, ctx: &Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.startup.close_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(ctx);
        }
    }

    fn add_sound(&mut self) {
        self.refresh_import_audio_entries();
        self.show_import_panel = true;
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>) {
        let mut imported = Vec::new();
        let mut ignored = 0usize;

        for path in paths {
            if !is_supported_audio(&path) {
                ignored += 1;
                continue;
            }

            match self.storage.import_sound(&path) {
                Ok(sound) => imported.push(sound),
                Err(error) => self.set_error_status(error),
            }
        }

        if imported.is_empty() {
            if ignored > 0 && self.status.is_none() {
                self.set_error_status("Unsupported file");
            }
            return;
        }

        imported.reverse();
        for sound in imported {
            self.selected = Some(sound.id);
            self.sounds.insert(0, sound);
        }

        let _ = ignored;
        self.save_now();
    }

    fn import_downloaded_sound(&mut self, path: &Path, remove_source: bool) {
        self.import_paths(vec![path.to_path_buf()]);
        if remove_source {
            let _ = fs::remove_file(path);
        }
    }

    fn open_sound_from_library(&mut self, sound_id: Uuid) {
        self.selected = Some(sound_id);
        self.app_view = AppView::Editor;
    }

    fn available_import_roots() -> Vec<PathBuf> {
        ["C:\\", "D:\\"]
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .collect()
    }

    fn set_import_dir(&mut self, path: Option<PathBuf>) {
        self.import_dir = path.unwrap_or_default();
        let _ = self.storage.save_import_dir(
            (!self.import_dir.as_os_str().is_empty()).then_some(self.import_dir.as_path()),
        );
        self.refresh_import_audio_entries();
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

    fn pitch_overlay_viewport_id() -> ViewportId {
        ViewportId::from_hash_of(PITCH_OVERLAY_ID)
    }

    fn refresh_pitch_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.pitch_capture_devices = devices;
                if !self
                    .selected_pitch_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.pitch_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_pitch_input_device = self.pitch_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn refresh_record_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.record_capture_devices = devices;
                if !self
                    .selected_record_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.record_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_record_input_device =
                        self.record_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
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

    fn record_overlay_viewport_id() -> ViewportId {
        ViewportId::from_hash_of(RECORD_OVERLAY_ID)
    }

    fn open_recording_review(&mut self, path: &Path) {
        self.close_recording_review(true);
        let name = if self.record_name.trim().is_empty() {
            "recording".to_owned()
        } else {
            self.record_name.trim().to_owned()
        };

        match self.storage.analyze_sound_as_effect(path, &name) {
            Ok(sound) => {
                self.stop_preview();
                let preview_id = sound.id;
                let preview_start = sound.trim_start_secs;
                let preview_duration = sound.safe_duration();
                self.recording_draft = Some(RecordingDraft {
                    sound,
                    source_path: path.to_path_buf(),
                });
                self.show_record_panel = false;
                self.show_record_review_panel = true;
                self.app_view = AppView::Editor;
                self.set_preview_cursor_secs(preview_id, preview_start, preview_duration);
                self.preview_recording_draft_from_position(Some(preview_start));
                self.clear_status();
            }
            Err(error) => {
                let _ = fs::remove_file(path);
                self.set_error_status(error);
            }
        }
    }

    fn format_key_name(key: egui::Key) -> &'static str {
        match key {
            egui::Key::ArrowDown => "Down",
            egui::Key::ArrowLeft => "Left",
            egui::Key::ArrowRight => "Right",
            egui::Key::ArrowUp => "Up",
            egui::Key::Escape => "Esc",
            egui::Key::Tab => "Tab",
            egui::Key::Backspace => "Backspace",
            egui::Key::Enter => "Enter",
            egui::Key::Space => "Space",
            egui::Key::Insert => "Insert",
            egui::Key::Delete => "Delete",
            egui::Key::Home => "Home",
            egui::Key::End => "End",
            egui::Key::PageUp => "PageUp",
            egui::Key::PageDown => "PageDown",
            egui::Key::Num0 => "0",
            egui::Key::Num1 => "1",
            egui::Key::Num2 => "2",
            egui::Key::Num3 => "3",
            egui::Key::Num4 => "4",
            egui::Key::Num5 => "5",
            egui::Key::Num6 => "6",
            egui::Key::Num7 => "7",
            egui::Key::Num8 => "8",
            egui::Key::Num9 => "9",
            egui::Key::A => "A",
            egui::Key::B => "B",
            egui::Key::C => "C",
            egui::Key::D => "D",
            egui::Key::E => "E",
            egui::Key::F => "F",
            egui::Key::G => "G",
            egui::Key::H => "H",
            egui::Key::I => "I",
            egui::Key::J => "J",
            egui::Key::K => "K",
            egui::Key::L => "L",
            egui::Key::M => "M",
            egui::Key::N => "N",
            egui::Key::O => "O",
            egui::Key::P => "P",
            egui::Key::Q => "Q",
            egui::Key::R => "R",
            egui::Key::S => "S",
            egui::Key::T => "T",
            egui::Key::U => "U",
            egui::Key::V => "V",
            egui::Key::W => "W",
            egui::Key::X => "X",
            egui::Key::Y => "Y",
            egui::Key::Z => "Z",
            egui::Key::F1 => "F1",
            egui::Key::F2 => "F2",
            egui::Key::F3 => "F3",
            egui::Key::F4 => "F4",
            egui::Key::F5 => "F5",
            egui::Key::F6 => "F6",
            egui::Key::F7 => "F7",
            egui::Key::F8 => "F8",
            egui::Key::F9 => "F9",
            egui::Key::F10 => "F10",
            egui::Key::F11 => "F11",
            egui::Key::F12 => "F12",
            _ => "Key",
        }
    }

    fn parse_key_name(name: &str) -> Option<egui::Key> {
        Some(match name {
            "Down" => egui::Key::ArrowDown,
            "Left" => egui::Key::ArrowLeft,
            "Right" => egui::Key::ArrowRight,
            "Up" => egui::Key::ArrowUp,
            "Esc" => egui::Key::Escape,
            "Tab" => egui::Key::Tab,
            "Backspace" => egui::Key::Backspace,
            "Enter" => egui::Key::Enter,
            "Space" => egui::Key::Space,
            "Insert" => egui::Key::Insert,
            "Delete" => egui::Key::Delete,
            "Home" => egui::Key::Home,
            "End" => egui::Key::End,
            "PageUp" => egui::Key::PageUp,
            "PageDown" => egui::Key::PageDown,
            "0" => egui::Key::Num0,
            "1" => egui::Key::Num1,
            "2" => egui::Key::Num2,
            "3" => egui::Key::Num3,
            "4" => egui::Key::Num4,
            "5" => egui::Key::Num5,
            "6" => egui::Key::Num6,
            "7" => egui::Key::Num7,
            "8" => egui::Key::Num8,
            "9" => egui::Key::Num9,
            "A" => egui::Key::A,
            "B" => egui::Key::B,
            "C" => egui::Key::C,
            "D" => egui::Key::D,
            "E" => egui::Key::E,
            "F" => egui::Key::F,
            "G" => egui::Key::G,
            "H" => egui::Key::H,
            "I" => egui::Key::I,
            "J" => egui::Key::J,
            "K" => egui::Key::K,
            "L" => egui::Key::L,
            "M" => egui::Key::M,
            "N" => egui::Key::N,
            "O" => egui::Key::O,
            "P" => egui::Key::P,
            "Q" => egui::Key::Q,
            "R" => egui::Key::R,
            "S" => egui::Key::S,
            "T" => egui::Key::T,
            "U" => egui::Key::U,
            "V" => egui::Key::V,
            "W" => egui::Key::W,
            "X" => egui::Key::X,
            "Y" => egui::Key::Y,
            "Z" => egui::Key::Z,
            "F1" => egui::Key::F1,
            "F2" => egui::Key::F2,
            "F3" => egui::Key::F3,
            "F4" => egui::Key::F4,
            "F5" => egui::Key::F5,
            "F6" => egui::Key::F6,
            "F7" => egui::Key::F7,
            "F8" => egui::Key::F8,
            "F9" => egui::Key::F9,
            "F10" => egui::Key::F10,
            "F11" => egui::Key::F11,
            "F12" => egui::Key::F12,
            _ => return None,
        })
    }

    fn normalize_record_export_video_fps(fps: u32) -> u32 {
        match fps {
            record_video::LOW_VIDEO_FPS => record_video::LOW_VIDEO_FPS,
            record_video::HIGH_VIDEO_FPS => record_video::HIGH_VIDEO_FPS,
            _ => record_video::STANDARD_VIDEO_FPS,
        }
    }

    fn stop_recording(&mut self) {
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        if let Some(path) = self.recorder.take_completed_path() {
            self.open_recording_review(&path);
        }
    }

    fn start_recording(&mut self) {
        if self.record_input_source == PitchInputSource::Microphone
            && self.selected_record_input_device.is_none()
        {
            self.set_error_status("No microphone input found");
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
                self.record_overlay_native_visuals_applied = false;
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn toggle_recording(&mut self) {
        if self.recorder.snapshot().running {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    fn trigger_record_hotkey_action(&mut self, ctx: &Context) {
        if self.recorder.snapshot().running {
            self.reveal_record_review_on_open = true;
            self.stop_recording();
            if self.show_record_review_panel {
                Self::reveal_window(ctx);
                self.reveal_record_review_on_open = false;
            }
        } else {
            self.start_recording();
        }
    }

    fn preview_recording_draft_from_position(&mut self, start_position_secs: Option<f32>) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };

        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };

        self.myinstants_preview_audio_url = None;
        let playback = match start_position_secs {
            Some(start_position_secs) => {
                audio.play_from(&draft.sound, &draft.source_path, start_position_secs)
            }
            None => audio.play(&draft.sound, &draft.source_path),
        };

        match playback {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    fn close_recording_review(&mut self, discard_audio: bool) {
        if let Some(draft) = self.recording_draft.take() {
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(draft.sound.id))
            {
                self.stop_preview();
            }
            if discard_audio {
                let _ = fs::remove_file(draft.source_path);
            }
        }
        self.show_record_review_panel = false;
    }

    fn save_recording_review_to_library(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        let keep_review_open = self.active_record_video_export.is_some();

        let export_path = match self
            .storage
            .export_processed_sound_from_path(&draft.source_path, &draft.sound)
        {
            Ok(path) => path,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };

        match self.storage.import_sound(&export_path) {
            Ok(mut sound) => {
                sound.name = draft.sound.name.clone();
                self.selected = Some(sound.id);
                self.sounds.insert(0, sound);
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
        let source_path = draft.source_path.clone();
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
            stage: "Preparing".to_owned(),
            receiver: rx,
        });
        self.clear_status();

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
                send_progress(0.08, "Preparing audio");
                let storage = Storage::new()?;
                let processed_audio =
                    storage.export_processed_sound_from_path(&source_path, &sound)?;
                processed_audio_to_clean = Some(processed_audio.clone());

                send_progress(0.18, "Checking ffmpeg");
                let downloader = YoutubeAudioDownloader::new(&root_dir)?;
                let ffmpeg_path = downloader.ensure_ffmpeg_available()?;

                send_progress(0.28, "Analyzing pitch");
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

    fn poll_record_video_export(&mut self, ctx: &Context) {
        let mut finished: Option<Result<RecordVideoExportResult, String>> = None;

        if let Some(export) = self.active_record_video_export.as_mut() {
            while let Ok(message) = export.receiver.try_recv() {
                match message {
                    RecordVideoExportMessage::Progress { progress, stage } => {
                        export.progress = progress.clamp(0.0, 1.0);
                        export.stage = stage;
                        ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
                    }
                    RecordVideoExportMessage::Finished(result) => {
                        finished = Some(result);
                        break;
                    }
                }
            }
        }

        let Some(result) = finished else {
            return;
        };

        self.active_record_video_export = None;

        match result {
            Ok(exported) => {
                match self.storage.import_video(
                    &exported.video_path,
                    &exported.video_name,
                    exported.duration_secs,
                    exported.video_fps,
                ) {
                    Ok(mut video) => {
                        if let Ok(waveform) = self
                            .storage
                            .analyze_waveform_preview(&exported.processed_audio_path, 96)
                        {
                            video.waveform = waveform;
                        }
                        self.video_assets.insert(0, video.clone());
                        let _ = self.storage.save_video_library(&self.video_assets);
                        self.app_view = AppView::Library;
                        self.library_tab = LibraryTab::Videos;
                        let mut opened_viewer = false;
                        match self.prepare_video_viewer(ctx, &video) {
                            Ok(()) => {
                                opened_viewer = true;
                                if let Err(error) = self.play_video_viewer_from_current_playhead() {
                                    self.set_error_status(error);
                                }
                            }
                            Err(error) => self.set_error_status(error),
                        }
                        if opened_viewer {
                            self.clear_status();
                        }
                    }
                    Err(error) => self.set_error_status(error),
                }
                let _ = fs::remove_file(&exported.processed_audio_path);
                let _ = fs::remove_file(&exported.video_path);
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn handle_record_hotkey(&mut self, ctx: &Context) {
        self.record_hotkey_manager.set_repaint_context(ctx.clone());

        if self.capture_record_hotkey {
            let mut captured_key = None;
            let mut cancel = false;
            ctx.input(|input| {
                for event in &input.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } = event
                    {
                        if *key == egui::Key::Escape {
                            cancel = true;
                        } else {
                            captured_key = Some(*key);
                        }
                        break;
                    }
                }
            });

            if cancel {
                self.capture_record_hotkey = false;
                return;
            }

            if let Some(key) = captured_key {
                self.record_hotkey = Some(key);
                self.capture_record_hotkey = false;
                if let Err(error) = self.record_hotkey_manager.set_hotkey(self.record_hotkey) {
                    self.set_error_status(error);
                }
                let _ = self
                    .storage
                    .save_record_hotkey(Some(Self::format_key_name(key)));
            }
            return;
        }

        if self.is_transition_active() {
            return;
        }
        if let Some(error) = self.record_hotkey_manager.take_error() {
            self.set_error_status(error);
        }
        if self.record_hotkey_manager.take_triggered() {
            self.trigger_record_hotkey_action(ctx);
        }
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

    fn handle_dropped_files(&mut self, ctx: &Context) {
        if self.app_view != AppView::Editor || self.has_modal_panel() {
            return;
        }

        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let mut paths = Vec::new();
        for path in dropped.into_iter().filter_map(|file| file.path) {
            let normalized = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if self.ignored_drop_path.as_ref() == Some(&normalized) {
                self.ignored_drop_path = None;
                continue;
            }
            paths.push(path);
        }
        if !paths.is_empty() {
            self.import_paths(paths);
        }
    }

    fn preview_sound(&mut self, sound_id: Uuid) {
        self.preview_sound_from_position(sound_id, None);
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

        let asset_path = sound.asset_path(self.storage.root_dir());
        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };
        self.myinstants_preview_audio_url = None;

        let playback = match start_position_secs {
            Some(start_position_secs) => audio.play_from(&sound, &asset_path, start_position_secs),
            None => audio.play(&sound, &asset_path),
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

    fn trim_playhead_drag_id(sound_id: Uuid) -> egui::Id {
        egui::Id::new((sound_id, "trim-playhead-drag"))
    }

    fn set_preview_cursor_secs(&mut self, sound_id: Uuid, secs: f32, duration_secs: f32) {
        self.preview_cursor = Some((sound_id, secs.clamp(0.0, duration_secs)));
    }

    fn stop_preview(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.myinstants_preview_audio_url = None;
    }

    fn pointer_drag_active(ctx: &Context, rect: Rect) -> bool {
        ctx.input(|input| {
            if !input.pointer.primary_down() || input.pointer.delta().length_sq() <= 4.0 {
                return false;
            }
            input
                .pointer
                .interact_pos()
                .or_else(|| input.pointer.latest_pos())
                .is_some_and(|pos| rect.contains(pos))
        })
    }

    fn pointer_press_origin_within(ctx: &Context, rect: Rect) -> bool {
        ctx.input(|input| {
            input
                .pointer
                .press_origin()
                .is_some_and(|pos| rect.contains(pos))
        })
    }

    fn pointer_primary_pressed_in_app(ctx: &Context) -> bool {
        let app_rect = ctx.screen_rect().expand(4.0);
        ctx.input(|input| {
            input.pointer.button_pressed(egui::PointerButton::Primary)
                && input
                    .pointer
                    .press_origin()
                    .is_some_and(|pos| app_rect.contains(pos))
        })
    }

    fn library_query_matches(name: &str, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        name.to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
    }

    fn reveal_window(ctx: &Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn delete_selected(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound = self.sounds.remove(index);
        if let Some(audio) = self.audio.as_mut() {
            if audio.is_playing(sound.id) {
                audio.stop();
            }
        }

        if let Err(error) = self.storage.remove_sound(&sound) {
            self.set_error_status(error);
            return;
        }

        self.selected = self
            .sounds
            .get(index)
            .or_else(|| self.sounds.get(index.saturating_sub(1)))
            .map(|next| next.id);
        self.save_now();
    }

    fn copy_selected_processed_sound(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        match self.copy_sound_file_to_clipboard(&self.sounds[index]) {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    fn commit_selected_trimmed_sound(&mut self, ctx: &Context) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound_id = self.sounds[index].id;
        if let Some(audio) = self.audio.as_mut()
            && audio.is_playing(sound_id)
        {
            audio.stop();
        }

        match self
            .storage
            .replace_sound_with_processed(&mut self.sounds[index])
        {
            Ok(()) => {
                self.save_now();
                ctx.request_repaint();
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn duplicate_selected_trimmed_sound(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound_id = self.sounds[index].id;
        if let Some(audio) = self.audio.as_mut()
            && audio.is_playing(sound_id)
        {
            audio.stop();
        }

        let source_name = self.sounds[index].name.clone();
        let export_path = match self.storage.export_processed_sound(&self.sounds[index]) {
            Ok(path) => path,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };

        match self.storage.import_sound(&export_path) {
            Ok(mut sound) => {
                sound.name = format!("{source_name} trim");
                self.selected = Some(sound.id);
                self.sounds.insert(0, sound);
                self.save_now();
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }

        let _ = fs::remove_file(export_path);
    }

    fn copy_sound_file_to_clipboard(&self, sound: &SoundEffect) -> Result<()> {
        let export_path = self.storage.export_processed_sound(sound)?;
        self.copy_file_path_to_clipboard(&export_path)
    }

    fn drag_sound_file_out(&mut self, ctx: &Context, sound: &SoundEffect) -> Result<()> {
        let drag_path = self.storage.drag_sound_source_path(sound)?;
        self.ignored_drop_path =
            Some(fs::canonicalize(&drag_path).unwrap_or_else(|_| drag_path.clone()));
        self.pending_sound_drag = None;
        self.suppress_sound_drag_until_release = true;
        let drag_ghost = platform::DragGhostSpec {
            waveform: Self::trimmed_waveform_preview(sound),
            dark_theme: self.dark_theme,
        };
        let result = platform::drag_file_out(&drag_path, Some(&drag_ghost));
        ctx.request_repaint();
        result
    }

    fn copy_video_file_to_clipboard(&self, video: &VideoAsset) -> Result<()> {
        let video_path = video.asset_path(self.storage.root_dir());
        self.copy_file_path_to_clipboard(&video_path)
    }

    fn play_video_viewer_from_current_playhead(&mut self) -> Result<()> {
        let Some((audio_path, progress, duration_secs)) =
            self.video_viewer.as_ref().map(|viewer| {
                (
                    viewer.audio_path.clone(),
                    viewer.progress.clamp(0.0, 1.0),
                    viewer.video.duration_secs.max(0.05),
                )
            })
        else {
            return Ok(());
        };

        let start_progress = if progress >= 0.995 { 0.0 } else { progress };
        let start_secs = start_progress * duration_secs;
        let Some(audio) = self.audio.as_mut() else {
            return Err(anyhow::anyhow!("Audio unavailable"));
        };

        audio.play_file_from(&audio_path, start_secs)?;
        if let Some(viewer) = self.video_viewer.as_mut() {
            viewer.progress = start_progress;
        }
        Ok(())
    }

    fn toggle_video_viewer_playback(&mut self) {
        let Some((audio_path, stored_progress)) = self
            .video_viewer
            .as_ref()
            .map(|viewer| (viewer.audio_path.clone(), viewer.progress))
        else {
            return;
        };

        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing_file(&audio_path));
        if is_playing {
            if let Some(progress) = self
                .audio
                .as_ref()
                .and_then(|audio| audio.playback_progress_for_file(&audio_path))
                .or(Some(stored_progress))
                && let Some(viewer) = self.video_viewer.as_mut()
            {
                viewer.progress = progress.clamp(0.0, 1.0);
            }
            self.stop_preview();
            return;
        }

        if let Err(error) = self.play_video_viewer_from_current_playhead() {
            self.set_error_status(error);
        } else {
            self.clear_status();
        }
    }

    fn copy_file_path_to_clipboard(&self, file_path: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            let _clipboard =
                Clipboard::new_attempts(10).context("unable to open system clipboard")?;
            let paths = [file_path.display().to_string()];
            FileList
                .write_clipboard(&paths)
                .context("unable to place file on clipboard")?;
            return Ok(());
        }

        #[cfg(not(windows))]
        {
            let _ = file_path;
            anyhow::bail!("Clipboard file copy is only available on Windows");
        }
    }

    fn prepare_video_viewer(&mut self, ctx: &Context, video: &VideoAsset) -> Result<()> {
        let ffmpeg_path = self.downloader.ensure_ffmpeg_available()?;
        let source_path = video.asset_path(self.storage.root_dir());
        let preview_root = self
            .storage
            .root_dir()
            .join("video-preview")
            .join(video.id.to_string());
        let frames_dir = preview_root.join("frames");
        let audio_path = preview_root.join("audio.wav");
        let ready_marker = preview_root.join("ready.txt");

        if !ready_marker.exists() {
            if preview_root.exists() {
                let _ = fs::remove_dir_all(&preview_root);
            }
            fs::create_dir_all(&frames_dir)
                .with_context(|| format!("unable to create {}", frames_dir.display()))?;

            let frame_pattern = frames_dir.join("frame_%05d.ppm");
            Self::run_ffmpeg_command(
                &ffmpeg_path,
                [
                    "-y",
                    "-i",
                    &source_path.to_string_lossy(),
                    "-vf",
                    &format!("fps={}", video.normalized_fps()),
                    "-pix_fmt",
                    "rgb24",
                    &frame_pattern.to_string_lossy(),
                ],
            )?;
            Self::run_ffmpeg_command(
                &ffmpeg_path,
                [
                    "-y",
                    "-i",
                    &source_path.to_string_lossy(),
                    "-vn",
                    "-acodec",
                    "pcm_s16le",
                    &audio_path.to_string_lossy(),
                ],
            )?;
            fs::write(&ready_marker, b"ok")
                .with_context(|| format!("unable to write {}", ready_marker.display()))?;
        }

        let mut frame_paths = fs::read_dir(&frames_dir)
            .with_context(|| format!("unable to read {}", frames_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("ppm"))
            .collect::<Vec<_>>();
        frame_paths.sort();
        if frame_paths.is_empty() {
            anyhow::bail!("video preview is empty");
        }

        self.stop_preview();
        self.video_viewer = Some(VideoViewerState {
            video: video.clone(),
            frame_paths,
            audio_path,
            progress: 0.0,
            current_frame: None,
        });
        self.load_video_frame_texture(ctx, 0)?;
        ctx.request_repaint();
        self.clear_status();
        Ok(())
    }

    fn run_ffmpeg_command<I, S>(ffmpeg_path: &Path, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cmd = Command::new(ffmpeg_path);
        for arg in args {
            cmd.arg(arg.as_ref());
        }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let output = cmd
            .output()
            .with_context(|| format!("failed to launch {}", ffmpeg_path.display()))?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    fn load_video_frame_texture(&mut self, ctx: &Context, frame_index: usize) -> Result<()> {
        let Some(viewer) = self.video_viewer.as_mut() else {
            return Ok(());
        };
        if viewer
            .current_frame
            .as_ref()
            .is_some_and(|(current, _, _)| *current == frame_index)
        {
            return Ok(());
        }

        let path = viewer
            .frame_paths
            .get(frame_index)
            .context("video frame not found")?;
        let (image, size) = load_ppm_color_image(path)?;
        let texture = ctx.load_texture(
            format!("video-frame-{}-{frame_index}", viewer.video.id),
            image,
            egui::TextureOptions::LINEAR,
        );
        viewer.current_frame = Some((frame_index, texture, size));
        Ok(())
    }

    fn preview_file_path(&mut self, path: &Path) -> Result<()> {
        if let Some(audio) = self.audio.as_mut() {
            audio.play_file(path)?;
        }
        Ok(())
    }

    fn existing_myinstants_path(
        files: &mut HashMap<String, PathBuf>,
        audio_url: &str,
    ) -> Option<PathBuf> {
        let path = files.get(audio_url).cloned()?;
        if path.exists() {
            Some(path)
        } else {
            files.remove(audio_url);
            None
        }
    }

    fn existing_myinstants_download_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_cached_files, audio_url)
    }

    fn existing_myinstants_preview_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_preview_files, audio_url)
    }

    fn existing_myinstants_playback_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        self.existing_myinstants_download_path(audio_url)
            .or_else(|| self.existing_myinstants_preview_path(audio_url))
    }

    fn toggle_myinstants_preview(&mut self, result: &MyinstantsResult) -> Result<()> {
        if let Some(path) = self.existing_myinstants_playback_path(&result.audio_url)
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing_file(&path))
        {
            self.stop_preview();
            return Ok(());
        }

        let path = if let Some(path) = self.existing_myinstants_download_path(&result.audio_url) {
            path
        } else if let Some(path) = self.existing_myinstants_preview_path(&result.audio_url) {
            path
        } else {
            let path = self.myinstants.ensure_preview_file(result)?;
            self.myinstants_preview_files
                .insert(result.audio_url.clone(), path.clone());
            path
        };
        self.myinstants_preview_audio_url = Some(result.audio_url.clone());
        if !self.myinstants_waveforms.contains_key(&result.audio_url) {
            let waveform = self.storage.analyze_waveform_preview(&path, 96)?;
            self.myinstants_waveforms
                .insert(result.audio_url.clone(), waveform);
        }
        self.preview_file_path(&path)
    }

    fn queue_myinstants_waveform_prefetch(&mut self, result: &MyinstantsResult) {
        if self.myinstants_waveforms.contains_key(&result.audio_url)
            || self.myinstants_waveform_jobs.contains(&result.audio_url)
        {
            return;
        }

        self.myinstants_waveform_jobs
            .insert(result.audio_url.clone());
        let result = result.clone();
        let client = self.myinstants.clone();
        let tx = self.myinstants_waveform_tx.clone();
        thread::spawn(move || {
            let message = match client.ensure_preview_file(&result) {
                Ok(path) => match Storage::new()
                    .and_then(|storage| storage.analyze_waveform_preview(&path, 96))
                {
                    Ok(waveform) => MyinstantsWaveformMessage::Ready {
                        audio_url: result.audio_url.clone(),
                        path,
                        waveform,
                    },
                    Err(error) => MyinstantsWaveformMessage::Failed {
                        audio_url: result.audio_url.clone(),
                        error: error.to_string(),
                    },
                },
                Err(error) => MyinstantsWaveformMessage::Failed {
                    audio_url: result.audio_url.clone(),
                    error: error.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn poll_myinstants_waveform_jobs(&mut self) {
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

    fn mark_dirty(&mut self, ctx: &Context) {
        self.pending_save = true;
        self.last_edit_at = ctx.input(|input| input.time);
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

    fn save_now(&mut self) {
        match self.storage.save_library(&self.sounds) {
            Ok(()) => {
                self.pending_save = false;
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
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

    fn apply_theme(ctx: &Context, dark_theme: bool) {
        Self::set_theme_enabled(dark_theme);

        let mut style = (*ctx.style()).clone();
        style.visuals = if dark_theme {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        style.visuals.panel_fill = if dark_theme {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.window_fill = if dark_theme {
            Color32::from_rgb(20, 17, 24)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.extreme_bg_color = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.faint_bg_color = if dark_theme {
            Color32::from_rgb(33, 28, 39)
        } else {
            Color32::from_rgb(244, 239, 246)
        };
        style.visuals.widgets.noninteractive.bg_fill = if dark_theme {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.noninteractive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(71, 57, 78)
        } else {
            Color32::from_rgb(229, 220, 228)
        };
        style.visuals.widgets.noninteractive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(236, 233, 240)
        } else {
            Color32::from_rgb(233, 226, 234)
        };
        style.visuals.widgets.inactive.bg_fill = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.inactive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(82, 67, 90)
        } else {
            Color32::from_rgb(226, 216, 225)
        };
        style.visuals.widgets.inactive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(245, 242, 248)
        } else {
            Color32::from_rgb(226, 218, 228)
        };
        style.visuals.widgets.hovered.bg_fill = if dark_theme {
            Color32::from_rgb(53, 36, 52)
        } else {
            Color32::from_rgb(255, 236, 246)
        };
        style.visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
        style.visuals.widgets.hovered.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 248, 252)
        } else {
            Color32::from_rgb(240, 229, 239)
        };
        style.visuals.widgets.active.bg_fill = if dark_theme {
            Color32::from_rgb(78, 30, 68)
        } else {
            Color32::from_rgb(255, 223, 239)
        };
        style.visuals.widgets.active.bg_stroke.color = Color32::from_rgb(214, 51, 132);
        style.visuals.widgets.active.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 251, 253)
        } else {
            Color32::from_rgb(245, 233, 243)
        };
        style.visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.visuals.hyperlink_color = Color32::from_rgb(214, 51, 132);
        style.visuals.window_shadow.color = if dark_theme {
            Color32::from_rgba_premultiplied(0, 0, 0, 96)
        } else {
            Color32::from_rgba_premultiplied(68, 27, 56, 48)
        };
        style.interaction.show_tooltips_only_when_still = false;
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.8;
        ctx.set_style(style);
    }

    fn page_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 247, 251)
        }
    }

    fn surface_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(24, 20, 29)
        } else {
            Color32::WHITE
        }
    }

    fn panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(29, 25, 35)
        } else {
            Color32::from_rgb(252, 248, 251)
        }
    }

    fn overlay_panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 251, 254)
        }
    }

    fn border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(78, 64, 87)
        } else {
            Color32::from_rgb(229, 220, 228)
        }
    }

    fn subtle_border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(88, 72, 96)
        } else {
            Color32::from_rgb(237, 226, 234)
        }
    }

    fn strong_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(246, 233, 241)
        } else {
            Color32::from_rgb(40, 35, 41)
        }
    }

    fn muted_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(191, 174, 189)
        } else {
            Color32::from_rgb(118, 106, 116)
        }
    }

    fn with_slider_visuals<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let widgets = &mut ui.style_mut().visuals.widgets;
                widgets.inactive.bg_fill = Color32::from_rgb(244, 240, 247);
                widgets.inactive.bg_stroke.color = Color32::from_rgb(244, 240, 247);
                widgets.hovered.bg_fill = Color32::from_rgb(255, 248, 252);
                widgets.hovered.bg_stroke.color = Color32::from_rgb(255, 248, 252);
                widgets.active.bg_fill = Color32::from_rgb(255, 253, 254);
                widgets.active.bg_stroke.color = Color32::from_rgb(255, 253, 254);
            }
            add_contents(ui)
        })
        .inner
    }

    fn with_dark_combo_visuals<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let visuals = &mut ui.style_mut().visuals;
                visuals.extreme_bg_color = Color32::from_rgb(28, 24, 33);
                visuals.faint_bg_color = Color32::from_rgb(33, 28, 39);
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

    fn titlebar_button(label: RichText, active: bool, danger: bool) -> Button<'static> {
        let (fill, stroke) = if danger {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(32, 26, 38)
                } else {
                    Color32::WHITE
                },
                Color32::from_rgb(230, 94, 150),
            )
        } else if active {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgba_premultiplied(118, 31, 82, 210)
                } else {
                    Color32::from_rgba_premultiplied(229, 85, 149, 118)
                },
                Color32::from_rgb(214, 51, 132),
            )
        } else {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgba_premultiplied(41, 34, 47, 224)
                } else {
                    Color32::from_rgba_premultiplied(237, 231, 238, 198)
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(84, 69, 92)
                } else {
                    Color32::from_rgb(221, 212, 222)
                },
            )
        };

        Button::new(label.strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(9.0)
    }

    fn action_button(label: RichText, active: bool, accent: bool) -> Button<'static> {
        let (fill, stroke, text) = if accent {
            (
                Color32::from_rgb(214, 51, 132),
                Color32::from_rgb(214, 51, 132),
                Color32::WHITE,
            )
        } else if active {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(66, 30, 60)
                } else {
                    Color32::from_rgb(255, 231, 243)
                },
                Color32::from_rgb(230, 94, 150),
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(255, 222, 240)
                } else {
                    Color32::from_rgb(120, 22, 72)
                },
            )
        } else {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(29, 25, 35)
                } else {
                    Color32::WHITE
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(83, 69, 92)
                } else {
                    Color32::from_rgb(227, 217, 226)
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(243, 230, 239)
                } else {
                    Color32::from_rgb(60, 54, 61)
                },
            )
        };

        Button::new(label.color(text))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(18.0)
    }

    fn icon_action(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        accent: bool,
    ) -> egui::Response {
        let icon_color = if accent || active {
            Color32::WHITE
        } else {
            Self::strong_text_color()
        };
        let response = ui.add_sized(
            size,
            Self::action_button(Self::icon(codepoint, 18.0, icon_color), active, accent),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn icon_titlebar(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        danger: bool,
    ) -> egui::Response {
        let response = ui.add_sized(
            size,
            Self::titlebar_button(
                Self::icon(codepoint, 18.0, Self::strong_text_color()),
                active,
                danger,
            ),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn paint_theme_titlebar_icon(painter: &egui::Painter, rect: Rect, active: bool) {
        let center = rect.center();
        let icon_color = Self::strong_text_color();

        if active {
            let moon_fill = if Self::dark_theme_enabled() {
                Color32::from_rgb(246, 233, 241)
            } else {
                Color32::from_rgb(245, 240, 246)
            };
            let cutout = if Self::dark_theme_enabled() {
                Color32::from_rgba_premultiplied(118, 31, 82, 210)
            } else {
                Color32::from_rgba_premultiplied(229, 85, 149, 118)
            };
            painter.circle_filled(center, 6.0, moon_fill);
            painter.circle_filled(Pos2::new(center.x + 3.0, center.y - 2.0), 6.0, cutout);
        } else {
            painter.circle_stroke(center, 5.0, Stroke::new(1.5, icon_color));
            for (dx, dy) in [
                (0.0, -8.0),
                (5.8, -5.8),
                (8.0, 0.0),
                (5.8, 5.8),
                (0.0, 8.0),
                (-5.8, 5.8),
                (-8.0, 0.0),
                (-5.8, -5.8),
            ] {
                let start = Pos2::new(center.x + dx * 0.62, center.y + dy * 0.62);
                let end = Pos2::new(center.x + dx, center.y + dy);
                painter.line_segment([start, end], Stroke::new(1.3, icon_color));
            }
        }
    }

    fn decorate_button_response(ui: &Ui, response: &egui::Response) {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            Self::paint_hover_button_notes(
                ui.painter(),
                response.rect,
                ui.input(|input| input.time) as f32,
            );
        }
    }

    fn paint_hover_button_notes(painter: &egui::Painter, rect: Rect, time: f32) {
        let anchor = Pos2::new(rect.right() - 10.0, rect.top() - 4.0);
        for (index, (dx, dy, scale, phase)) in [
            (-2.0, 2.0, 0.34, 0.0),
            (10.0, -6.0, 0.28, 0.8),
            (18.0, 6.0, 0.24, 1.4),
        ]
        .into_iter()
        .enumerate()
        {
            let drift = (time * 2.8 + phase).sin() * 3.0;
            let rise = (time * 2.0 + phase).cos() * 2.0 - index as f32 * 2.5;
            Self::paint_music_note(
                painter,
                Pos2::new(anchor.x + dx + drift, anchor.y + dy + rise),
                scale,
                (time * 1.3 + phase).sin() * 0.16,
                Color32::from_rgba_premultiplied(229, 85, 149, 188),
            );
        }
    }

    fn draw_titlebar(&mut self, ui: &mut Ui, ctx: &Context) {
        self.titlebar_drag_rect = None;
        ui.horizontal(|ui| {
            let drag_width = (ui.available_width() - 506.0).max(180.0);
            let drag_response = ui
                .allocate_ui_with_layout(
                    vec2(drag_width, 44.0),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        let frame = Frame::new()
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
                        ui.interact(
                            frame.response.rect,
                            ui.id().with("titlebar-drag"),
                            Sense::click_and_drag(),
                        )
                    },
                )
                .inner;

            self.titlebar_drag_rect = Some(drag_response.rect);
            if drag_response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

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

                if Self::icon_titlebar(
                    ui,
                    [42.0, 30.0],
                    0xe9b0,
                    self.app_view == AppView::Library,
                    false,
                )
                .clicked()
                {
                    self.app_view = if self.app_view == AppView::Library {
                        AppView::Editor
                    } else {
                        AppView::Library
                    };
                }

                let spn_response = ui.add_sized(
                    [52.0, 30.0],
                    Self::titlebar_button(
                        RichText::new("SPN")
                            .size(11.5)
                            .color(Self::strong_text_color()),
                        self.show_pitch_panel,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &spn_response);
                if spn_response.clicked() {
                    self.show_pitch_panel = !self.show_pitch_panel;
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe8b8, self.show_settings_panel, false)
                    .clicked()
                {
                    self.show_settings_panel = true;
                    if self.settings_startup_candidate.is_none() {
                        self.settings_startup_candidate = self.selected;
                    }
                    if self.settings_exit_candidate.is_none() {
                        self.settings_exit_candidate = self.selected;
                    }
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe145, false, false).clicked() {
                    self.add_sound();
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe061, false, false).clicked() {
                    self.show_record_panel = true;
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe8b6, self.show_myinstants_panel, false)
                    .clicked()
                {
                    self.show_myinstants_panel = true;
                }

                if Self::icon_titlebar(ui, [48.0, 30.0], 0xe2c4, false, false).clicked() {
                    self.show_download_panel = true;
                }
            });
        });
    }

    fn render_titlebar_drag_zone(&self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        let Some(handle_rect) = self.titlebar_drag_rect else {
            return;
        };

        egui::Area::new(egui::Id::new("titlebar-drag-overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(handle_rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (_, response) =
                    ui.allocate_exact_size(handle_rect.size(), Sense::click_and_drag());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });
    }

    fn render_custom_window_resize_handles(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
            return;
        }

        let rect = ctx.screen_rect();
        let edge = 8.0;
        let corner = 22.0;
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

    fn paint_titlebar_blob(ui: &mut Ui) {
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

    fn paint_titlebar_wave(ui: &mut Ui) {
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

    fn refresh_import_audio_entries(&mut self) {
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

    fn should_skip_import_path(path: &Path) -> bool {
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

    fn render_import_panel(&mut self, ctx: &Context) {
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

        egui::Window::new("")
            .id(egui::Id::new("sound-import-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(640.0, 560.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
        let mut refresh_inputs = false;
        let mut clear_hotkey = false;

        egui::Window::new("")
            .id(egui::Id::new("sound-record-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(520.0, 420.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                        let keyboard_response = ui.add_sized(
                            [42.0, 36.0],
                            Self::action_button(
                                Self::icon(0xe312, 18.0, Self::strong_text_color()),
                                self.capture_record_hotkey,
                                self.capture_record_hotkey,
                            ),
                        );
                        Self::decorate_button_response(ui, &keyboard_response);
                        if keyboard_response.clicked() {
                            self.capture_record_hotkey = true;
                        }

                        let hotkey_label = if self.capture_record_hotkey {
                            "Press key"
                        } else {
                            self.record_hotkey
                                .map(Self::format_key_name)
                                .unwrap_or("Key")
                        };
                        let hotkey_response = ui.add_sized(
                            [118.0, 36.0],
                            Self::action_button(
                                RichText::new(hotkey_label).size(13.0),
                                self.capture_record_hotkey,
                                self.capture_record_hotkey,
                            ),
                        );
                        Self::decorate_button_response(ui, &hotkey_response);
                        if hotkey_response.clicked() {
                            self.capture_record_hotkey = true;
                        }
                        if Self::icon_action(ui, [42.0, 36.0], 0xe14c, false, false).clicked() {
                            clear_hotkey = true;
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
                            if self.record_capture_devices.is_empty() {
                                refresh_inputs = true;
                            }
                        }

                        if Self::icon_action(ui, [42.0, 36.0], 0xe5d5, false, false).clicked() {
                            refresh_inputs = true;
                        }
                    });
                });

                if self.record_input_source == PitchInputSource::Microphone {
                    ui.add_space(10.0);
                    ui.add_enabled_ui(!snapshot.running, |ui| {
                        Frame::new()
                            .fill(Self::surface_fill())
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(18.0)
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
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
                            });
                    });
                }

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
                });
            });

        if close_request {
            open_panel = false;
            if snapshot.running {
                self.stop_recording();
            }
        }
        self.show_record_panel = open_panel;

        if clear_hotkey {
            self.record_hotkey = None;
            self.capture_record_hotkey = false;
            let _ = self.record_hotkey_manager.set_hotkey(None);
            let _ = self.storage.save_record_hotkey(None);
        }

        if refresh_inputs {
            self.refresh_record_capture_devices();
        }

        if toggle_record {
            self.toggle_recording();
        }
    }

    fn render_record_review_panel(&mut self, ctx: &Context) {
        if !self.show_record_review_panel {
            return;
        }

        let Some(sound_snapshot) = self
            .recording_draft
            .as_ref()
            .map(|draft| draft.sound.clone())
        else {
            self.show_record_review_panel = false;
            return;
        };

        let sound_id = sound_snapshot.id;
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
        if is_playing {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        let mut preview_cursor_secs = self.preview_cursor_secs_for(&sound_snapshot);
        if let Some(position_secs) = playback_position_secs
            && !playhead_drag_active
        {
            preview_cursor_secs = position_secs.clamp(0.0, sound_snapshot.safe_duration());
        }

        let mut open_panel = self.show_record_review_panel;
        let mut close_request = false;
        let mut save_audio = false;
        let mut export_video = false;
        let mut preview_toggle = false;
        let mut seek_request = false;
        let mut changed = false;
        let mut trim_timeline_zoom = self.trim_timeline_zoom;
        let export_progress = self
            .active_record_video_export
            .as_ref()
            .map(|export| (export.progress, export.stage.clone()));
        let exporting_video = export_progress.is_some();

        egui::Window::new("")
            .id(egui::Id::new("record-review-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(680.0, 560.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                    .corner_radius(32.0)
                    .inner_margin(Margin::same(22)),
            )
            .show(ctx, |ui| {
                let draft = self.recording_draft.as_mut().expect("record draft missing");
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe061, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                let response = ui.add_sized(
                    [ui.available_width(), 48.0],
                    TextEdit::singleline(&mut draft.sound.name)
                        .font(egui::TextStyle::Heading)
                        .desired_width(f32::INFINITY)
                        .margin(Vec2::new(14.0, 14.0)),
                );
                if response.changed() {
                    changed = true;
                }

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        let (timeline_changed, timeline_seek_request) = Self::draw_trim_timeline(
                            ui,
                            &mut draft.sound,
                            &mut preview_cursor_secs,
                            &mut trim_timeline_zoom,
                            !is_playing,
                            true,
                        );
                        changed |= timeline_changed;
                        seek_request |= timeline_seek_request;
                    });

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        Self::with_slider_visuals(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Self::icon(0xe050, 16.0, Self::muted_text_color()));
                                let volume_response = ui.add(
                                    Slider::new(&mut draft.sound.volume, 0.0..=5.0)
                                        .show_value(false)
                                        .step_by(0.01),
                                );
                                ui.label(
                                    RichText::new(format!("{:.0}%", draft.sound.volume * 100.0))
                                        .size(14.0)
                                        .color(Self::strong_text_color()),
                                );
                                ui.add_space(10.0);
                                ui.label(Self::icon(0xe9e4, 16.0, Self::muted_text_color()));
                                let speed_response = ui.add(
                                    Slider::new(&mut draft.sound.speed, 0.25..=2.0)
                                        .show_value(false)
                                        .step_by(0.01),
                                );
                                ui.label(
                                    RichText::new(format!("{:.2}x", draft.sound.speed))
                                        .size(14.0)
                                        .color(Self::strong_text_color()),
                                );
                                if volume_response.changed() || speed_response.changed() {
                                    changed = true;
                                }
                            });
                        });
                    });

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                vec2(44.0, 32.0),
                                egui::Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("Export")
                                            .size(12.5)
                                            .color(Self::muted_text_color()),
                                    );
                                },
                            );
                            ui.add_space(8.0);

                            let animation = ui.add_sized(
                                [108.0, 32.0],
                                Self::action_button(
                                    RichText::new("Animation").size(12.5),
                                    self.record_export_video_animation,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &animation);
                            if animation.clicked() {
                                self.record_export_video_animation =
                                    !self.record_export_video_animation;
                                ctx.request_repaint();
                            }

                            let sharp = ui.add_sized(
                                [88.0, 32.0],
                                Self::action_button(
                                    RichText::new("Sharp").size(12.5),
                                    self.record_export_video_sharps,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &sharp);
                            if sharp.clicked() {
                                self.record_export_video_sharps = !self.record_export_video_sharps;
                                ctx.request_repaint();
                            }

                            ui.add_space(10.0);
                            ui.allocate_ui_with_layout(
                                vec2(24.0, 32.0),
                                egui::Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("FPS")
                                            .size(12.5)
                                            .color(Self::muted_text_color()),
                                    );
                                },
                            );
                            for fps in RECORD_EXPORT_VIDEO_FPS_OPTIONS {
                                let active = self.record_export_video_fps == fps;
                                let response = ui.add_sized(
                                    [74.0, 32.0],
                                    Self::action_button(
                                        RichText::new(format!("{fps}fps")).size(12.5),
                                        active,
                                        false,
                                    ),
                                );
                                Self::decorate_button_response(ui, &response);
                                if response.clicked() {
                                    self.record_export_video_fps = fps;
                                    ctx.request_repaint();
                                }
                            }
                        });
                    });

                ui.add_space(12.0);
                if let Some((progress, stage)) = export_progress.as_ref() {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(22.0)
                        .inner_margin(Margin::same(16))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(stage.as_str())
                                    .size(13.0)
                                    .color(Self::strong_text_color()),
                            );
                            ui.add_space(8.0);
                            ui.add(
                                ProgressBar::new(*progress)
                                    .desired_width(ui.available_width())
                                    .fill(Color32::from_rgb(227, 82, 149))
                                    .text(format!("{:.0}%", *progress * 100.0)),
                            );
                        });
                    ui.add_space(12.0);
                }

                ui.horizontal_centered(|ui| {
                    let preview = ui.add_sized(
                        [118.0, 38.0],
                        Self::action_button(
                            RichText::new(if is_playing { "Stop" } else { "Preview" }).size(13.0),
                            is_playing,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &preview);
                    if preview.clicked() {
                        preview_toggle = true;
                    }

                    let save = ui.add_sized(
                        [132.0, 38.0],
                        Self::action_button(RichText::new("Save audio").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &save);
                    if save.clicked() {
                        save_audio = true;
                    }

                    let video = ui
                        .add_enabled_ui(!exporting_video, |ui| {
                            ui.add_sized(
                                [132.0, 38.0],
                                Self::action_button(
                                    RichText::new("Export SPN").size(13.0),
                                    false,
                                    false,
                                ),
                            )
                        })
                        .inner;
                    Self::decorate_button_response(ui, &video);
                    if video.clicked() {
                        export_video = true;
                    }

                    let discard = ui
                        .add_enabled_ui(!exporting_video, |ui| {
                            ui.add_sized(
                                [118.0, 38.0],
                                Self::action_button(
                                    RichText::new("Discard").size(13.0),
                                    false,
                                    false,
                                ),
                            )
                        })
                        .inner;
                    Self::decorate_button_response(ui, &discard);
                    if discard.clicked() {
                        close_request = true;
                    }
                });
            });

        let Some(updated_sound) = self
            .recording_draft
            .as_ref()
            .map(|draft| draft.sound.clone())
        else {
            self.show_record_review_panel = false;
            return;
        };
        let sound_duration = updated_sound.safe_duration();
        self.trim_timeline_zoom = trim_timeline_zoom;
        self.set_preview_cursor_secs(sound_id, preview_cursor_secs, sound_duration);
        self.show_record_review_panel = open_panel;

        if seek_request && is_playing {
            self.preview_recording_draft_from_position(Some(preview_cursor_secs));
        }
        if preview_toggle {
            if is_playing {
                self.stop_preview();
            } else {
                let mut cursor_secs = self.preview_cursor_secs_for(&updated_sound);
                if cursor_secs >= updated_sound.trim_end_secs - 0.02 {
                    cursor_secs = updated_sound.trim_start_secs;
                    self.set_preview_cursor_secs(sound_id, cursor_secs, sound_duration);
                }
                self.preview_recording_draft_from_position(Some(cursor_secs));
            }
        }
        if changed {
            ctx.request_repaint();
        }
        if export_video {
            self.export_recording_review_video();
        }
        if save_audio {
            self.save_recording_review_to_library();
        } else if close_request || !open_panel {
            self.close_recording_review(true);
        }
    }

    fn render_settings_panel(&mut self, ctx: &Context) {
        if !self.show_settings_panel {
            return;
        }

        let mut open_panel = self.show_settings_panel;
        let mut close_request = false;
        let mut save_startup = false;
        let mut save_exit = false;
        let mut clear_startup = false;
        let mut clear_exit = false;
        let mut reset_startup = false;
        let mut reset_exit = false;

        egui::Window::new("")
            .id(egui::Id::new("settings-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(520.0, 360.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                    ui.label(Self::icon(0xe8b8, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                Self::draw_settings_sound_row(
                    ui,
                    &self.sounds,
                    "Startup",
                    &mut self.settings_startup_candidate,
                    &self.startup_sound_name,
                    "settings-startup-combo",
                    &mut save_startup,
                    &mut clear_startup,
                    &mut reset_startup,
                );

                ui.add_space(14.0);
                Self::draw_settings_sound_row(
                    ui,
                    &self.sounds,
                    "Exit",
                    &mut self.settings_exit_candidate,
                    &self.exit_sound_name,
                    "settings-exit-combo",
                    &mut save_exit,
                    &mut clear_exit,
                    &mut reset_exit,
                );
            });

        self.show_settings_panel = open_panel;
        if close_request {
            self.show_settings_panel = false;
        }

        if save_startup
            && let Some(sound_id) = self.settings_startup_candidate
            && let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            match self.storage.save_startup_sound(sound) {
                Ok(()) => {
                    self.startup_sound_name = Some(sound.name.clone());
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if save_exit
            && let Some(sound_id) = self.settings_exit_candidate
            && let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            match self.storage.save_exit_sound(sound) {
                Ok(()) => {
                    self.exit_sound_name = Some(sound.name.clone());
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if clear_startup {
            match self.storage.clear_startup_sound() {
                Ok(()) => self.startup_sound_name = None,
                Err(error) => self.set_error_status(error),
            }
        }

        if clear_exit {
            match self.storage.clear_exit_sound() {
                Ok(()) => self.exit_sound_name = None,
                Err(error) => self.set_error_status(error),
            }
        }

        if reset_startup {
            match self.storage.reset_startup_sound() {
                Ok(()) => {
                    let resolved = self.storage.resolved_startup_sound_path().ok().flatten();
                    self.startup_sound_name = self.storage.load_startup_sound_name().ok().flatten();
                    let visual =
                        Self::load_transition_sound_visual(&self.storage, resolved.clone());
                    self.startup.sound_waveform = visual.0;
                    self.startup.sound_duration_sec = visual.1;
                    self.startup.duration_sec = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_INTRO_DURATION_SEC,
                    );
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if reset_exit {
            match self.storage.reset_exit_sound() {
                Ok(()) => {
                    let resolved = self.storage.resolved_exit_sound_path().ok().flatten();
                    self.exit_sound_name = self.storage.load_exit_sound_name().ok().flatten();
                    let duration = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_OUTRO_DURATION_SEC,
                    );
                    if self.startup.phase == TransitionPhase::Outro {
                        self.startup.duration_sec = duration;
                    }
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
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

        egui::Window::new("")
            .id(egui::Id::new("video-viewer-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(760.0, 620.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
            || self.show_myinstants_panel
            || self.show_import_panel
            || self.show_download_panel
            || self.show_record_panel
            || self.show_record_review_panel
            || self.show_settings_panel
            || self.video_viewer.is_some()
            || self.show_trim_commit_panel
    }

    fn render_modal_backdrop(&self, ctx: &Context) {
        if !self.has_modal_panel() {
            return;
        }

        let rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("modal-backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (backdrop_rect, _) = ui.allocate_exact_size(rect.size(), Sense::click());
                ui.painter().rect_filled(
                    backdrop_rect,
                    0.0,
                    Color32::from_rgba_premultiplied(22, 16, 22, 132),
                );
            });
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
        let mut refresh_inputs = false;
        let mut close_request = false;
        egui::Window::new("")
            .id(egui::Id::new("pitch-monitor-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(296.0, 250.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                    .inner_margin(Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.set_width(260.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("SPN")
                            .size(13.0)
                            .color(Color32::from_rgb(58, 48, 58))
                            .strong(),
                    );
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

                ui.add_space(8.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
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
                            if self.pitch_capture_devices.is_empty() {
                                refresh_inputs = true;
                            }
                        }
                    });
                });

                if self.pitch_input_source == PitchInputSource::Microphone {
                    ui.add_space(8.0);
                    ui.add_enabled_ui(!snapshot.running, |ui| {
                        Frame::new()
                            .fill(Self::surface_fill())
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(18.0)
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                Self::with_dark_combo_visuals(ui, |ui| {
                                    ComboBox::from_id_salt("pitch-input-device")
                                        .width(ui.available_width() - 4.0)
                                        .selected_text(
                                            RichText::new(
                                                self.selected_pitch_input_device
                                                    .as_deref()
                                                    .map(|name| {
                                                        Self::truncate_middle_ascii(name, 28)
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
                            });
                    });
                }

                ui.add_space(8.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Self::with_slider_visuals(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(Self::icon(0xe8b5, 16.0, Self::muted_text_color()));
                            let speed_response = ui.add(
                                Slider::new(&mut self.pitch_update_hz, 1.0..=12.0)
                                    .show_value(false)
                                    .step_by(0.5),
                            );
                            ui.label(
                                RichText::new(format!("{:.1}/s", self.pitch_update_hz))
                                    .size(13.0)
                                    .color(Self::strong_text_color()),
                            );
                            if speed_response.changed() {
                                let _ = self.storage.save_pitch_update_hz(self.pitch_update_hz);
                            }
                        });
                    });
                });

                ui.add_space(8.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Color32::from_rgb(255, 248, 252))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(236, 224, 232)))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let animation_changed = ui
                                    .checkbox(
                                        &mut self.pitch_overlay_animation,
                                        RichText::new("Animation")
                                            .size(13.0)
                                            .color(Color32::from_rgb(58, 48, 58)),
                                    )
                                    .changed();
                                ui.add_space(10.0);
                                let sharp_changed = ui
                                    .checkbox(
                                        &mut self.pitch_show_sharps,
                                        RichText::new("Sharp")
                                            .size(13.0)
                                            .color(Color32::from_rgb(58, 48, 58)),
                                    )
                                    .changed();
                                if animation_changed {
                                    let _ = self
                                        .storage
                                        .save_overlay_animation(self.pitch_overlay_animation);
                                    self.center_pitch_overlay_next_frame = snapshot.running;
                                    self.pitch_overlay_native_visuals_applied = false;
                                }
                                if sharp_changed {
                                    let _ =
                                        self.storage.save_pitch_show_sharps(self.pitch_show_sharps);
                                }
                            });
                        });
                });

                if let Some(error) = snapshot.error.as_deref() {
                    ui.add_space(6.0);
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
                    self.set_error_status("No microphone input found");
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
                            self.pitch_overlay_native_visuals_applied = false;
                            self.clear_status();
                        }
                        Err(error) => self.set_error_status(error),
                    }
                }
            } else {
                self.pitch_monitor.stop();
                self.pitch_overlay_native_visuals_applied = false;
                ctx.send_viewport_cmd_to(Self::pitch_overlay_viewport_id(), ViewportCommand::Close);
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

        let viewport_id = Self::pitch_overlay_viewport_id();
        let mut should_stop = false;
        let overlay_size = if self.pitch_overlay_animation {
            vec2(276.0, 276.0)
        } else {
            vec2(430.0, 104.0)
        };
        let mut builder = ViewportBuilder::default()
            .with_title(PITCH_OVERLAY_TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_inner_size(overlay_size)
            .with_min_inner_size(overlay_size)
            .with_max_inner_size(overlay_size)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_visible(true)
            .with_active(false)
            .with_taskbar(false);
        if self.center_pitch_overlay_next_frame
            && let Some(monitor) = ctx.input(|input| input.viewport().monitor_size)
        {
            builder = builder.with_position(Pos2::new(
                ((monitor.x - overlay_size.x) * 0.5).max(0.0),
                ((monitor.y - overlay_size.y) * 0.5).max(0.0),
            ));
        }

        ctx.show_viewport_immediate(viewport_id, builder, |overlay_ctx, class| {
            if class == ViewportClass::Embedded {
                return;
            }

            if self.pitch_overlay_animation {
                let mut style = (*overlay_ctx.style()).clone();
                style.visuals.window_fill = Color32::TRANSPARENT;
                style.visuals.panel_fill = Color32::TRANSPARENT;
                style.visuals.extreme_bg_color = Color32::TRANSPARENT;
                style.visuals.faint_bg_color = Color32::TRANSPARENT;
                style.visuals.code_bg_color = Color32::TRANSPARENT;
                overlay_ctx.set_style(style);
            }

            if self.center_pitch_overlay_next_frame {
                overlay_ctx.send_viewport_cmd(ViewportCommand::InnerSize(overlay_size));
                if let Some(center_cmd) = ViewportCommand::center_on_screen(overlay_ctx) {
                    overlay_ctx.send_viewport_cmd(center_cmd);
                }
            }
            if overlay_ctx.input(|input| input.viewport().close_requested()) {
                should_stop = true;
            }

            overlay_ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
            CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(Color32::from_rgba_premultiplied(0, 0, 0, 0))
                        .inner_margin(0.0),
                )
                .show(overlay_ctx, |ui| {
                    if self.pitch_overlay_animation {
                        self.render_pitch_blob_overlay(
                            ui,
                            overlay_ctx,
                            &snapshot,
                            &mut should_stop,
                        );
                    } else {
                        self.render_pitch_pill_overlay(
                            ui,
                            overlay_ctx,
                            &snapshot,
                            &mut should_stop,
                        );
                    }
                });
        });
        self.center_pitch_overlay_next_frame = false;
        if !self.pitch_overlay_native_visuals_applied
            && platform::set_overlay_window_native_visuals(
                PITCH_OVERLAY_TITLE,
                !self.pitch_overlay_animation,
                self.pitch_overlay_animation,
            )
        {
            self.pitch_overlay_native_visuals_applied = true;
        }

        if should_stop {
            self.pitch_monitor.stop();
            self.pitch_overlay_native_visuals_applied = false;
            ctx.send_viewport_cmd_to(viewport_id, ViewportCommand::Close);
            self.clear_status();
        }
    }

    fn render_pitch_pill_overlay(
        &self,
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
                let drag_response = ui.interact(
                    drag_rect,
                    ui.id().with("pitch-overlay-drag"),
                    Sense::click_and_drag(),
                );
                if drag_response.drag_started() || drag_response.is_pointer_button_down_on() {
                    overlay_ctx.send_viewport_cmd(ViewportCommand::StartDrag);
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
        if drag_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_response.dragged() || drag_response.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if drag_response.drag_started() || drag_response.is_pointer_button_down_on() {
            overlay_ctx.send_viewport_cmd(ViewportCommand::StartDrag);
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
        let Some(first_digit) = flat_with_octave
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_digit() || *ch == '-')
            .map(|(index, _)| index)
        else {
            return note.to_owned();
        };

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
        let modal_open = self.has_modal_panel();
        let mut scale_changed = false;
        ui.horizontal(|ui| {
            let sounds_tab = ui.add_sized(
                [84.0, 30.0],
                Self::action_button(
                    RichText::new("Audio").size(12.5),
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
                    RichText::new("Video").size(12.5),
                    self.library_tab == LibraryTab::Videos,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &videos_tab);
            if videos_tab.clicked() {
                self.library_tab = LibraryTab::Videos;
            }

            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                Self::with_slider_visuals(ui, |ui| {
                    let scale_response = ui.add_sized(
                        [112.0, 28.0],
                        Slider::new(&mut self.library_grid_scale, 0.72..=1.1)
                            .show_value(false)
                            .step_by(0.01),
                    );
                    scale_changed = scale_response.changed();
                    ui.add_space(8.0);
                    ui.label(Self::icon(0xe8b8, 15.0, Self::muted_text_color()));
                });
            });
        });
        ui.add_space(10.0);
        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(18.0)
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe8b6, 16.0, Self::muted_text_color()));
                    let query = if self.library_tab == LibraryTab::Videos {
                        &mut self.library_video_query
                    } else {
                        &mut self.library_audio_query
                    };
                    ui.add_sized(
                        [ui.available_width(), 24.0],
                        TextEdit::singleline(query)
                            .hint_text("Search")
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        if scale_changed {
            let _ = self
                .storage
                .save_library_grid_scale(self.library_grid_scale);
        }
        ui.add_space(12.0);

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.library_tab == LibraryTab::Videos {
                    self.draw_video_library_grid(ui);
                    return;
                }

                if self.sounds.is_empty() {
                    Self::draw_empty_editor(ui);
                    return;
                }

                let spacing = 16.0;
                let available_width = ui.available_width().max(180.0);
                let target_card = (204.0 * self.library_grid_scale).clamp(150.0, 220.0);
                let sounds = self
                    .sounds
                    .iter()
                    .filter(|sound| {
                        Self::library_query_matches(&sound.name, &self.library_audio_query)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if sounds.is_empty() {
                    Self::draw_empty_editor(ui);
                    return;
                }
                let mut open_sound = None;
                let mut preview_sound = None;
                let mut copy_sound = None;
                let mut drag_sound = None;

                let columns = (((available_width + spacing) / (target_card + spacing)).floor()
                    as usize)
                    .max(1);
                let card_size = ((available_width - spacing * (columns.saturating_sub(1)) as f32)
                    / columns as f32)
                    .clamp(150.0, 220.0);
                let row_count = sounds.len().div_ceil(columns);

                for (row_index, row) in sounds.chunks(columns).enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = vec2(spacing, spacing);
                        let row_width = row.len() as f32 * card_size
                            + row.len().saturating_sub(1) as f32 * spacing;
                        let left_pad = ((available_width - row_width) * 0.5).max(0.0);
                        if left_pad > 0.0 {
                            ui.add_space(left_pad);
                        }

                        for sound in row {
                            let (tile_rect, _tile_response) =
                                ui.allocate_exact_size(vec2(card_size, card_size), Sense::hover());
                            let body_rect = Rect::from_min_max(
                                tile_rect.min,
                                Pos2::new(tile_rect.max.x, tile_rect.max.y - 46.0),
                            );
                            let body_response = ui.interact(
                                body_rect,
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
                            let pointer_drag_active =
                                !modal_open && Self::pointer_drag_active(ui.ctx(), body_rect);
                            let hovered = !modal_open && pointer_hover;

                            if hovered {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                            }
                            if !modal_open
                                && !self.suppress_sound_drag_until_release
                                && ui.ctx().input(|input| input.pointer.primary_down())
                                && Self::pointer_press_origin_within(ui.ctx(), body_rect)
                            {
                                self.pending_sound_drag = Some(sound.id);
                            }
                            if !modal_open
                                && !self.suppress_sound_drag_until_release
                                && (body_response.dragged()
                                    || body_response.is_pointer_button_down_on()
                                    || pointer_drag_active)
                            {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            }
                            if !modal_open
                                && self.pending_sound_drag == Some(sound.id)
                                && !self.suppress_sound_drag_until_release
                                && (body_response.dragged() || pointer_drag_active)
                            {
                                drag_sound = Some(sound.id);
                                self.pending_sound_drag = None;
                            }
                            if !modal_open && body_response.clicked() {
                                open_sound = Some(sound.id);
                            }

                            ui.scope_builder(egui::UiBuilder::new().max_rect(tile_rect), |ui| {
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

                                Frame::new()
                                    .fill(fill)
                                    .stroke(Stroke::new(1.0, stroke))
                                    .shadow(Shadow {
                                        offset: [0, 12],
                                        blur: 28,
                                        spread: 0,
                                        color: Color32::from_rgba_premultiplied(86, 43, 67, 18),
                                    })
                                    .corner_radius(30.0)
                                    .inner_margin(Margin::same(16))
                                    .show(ui, |ui| {
                                        let inner_size = card_size - 32.0;
                                        ui.set_min_size(vec2(inner_size, inner_size));
                                        ui.set_width(inner_size);
                                        ui.vertical(|ui| {
                                            ui.add_sized(
                                                [inner_size, 18.0],
                                                egui::Label::new(
                                                    RichText::new(&sound.name)
                                                        .size(12.5)
                                                        .color(title_color)
                                                        .strong(),
                                                )
                                                .truncate(),
                                            );
                                            ui.add_space(7.0);
                                            let waveform_preview =
                                                Self::trimmed_waveform_preview(sound);
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
                                                (card_size * 0.38).clamp(58.0, 78.0),
                                            );
                                            ui.add_space(9.0);
                                            ui.label(
                                                RichText::new(format_time(sound.trimmed_length()))
                                                    .size(11.5)
                                                    .color(meta_color),
                                            );
                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                if Self::icon_action(
                                                    ui,
                                                    [46.0, 31.0],
                                                    0xe037,
                                                    false,
                                                    false,
                                                )
                                                .clicked()
                                                {
                                                    preview_sound = Some(sound.id);
                                                }
                                                if Self::icon_action(
                                                    ui,
                                                    [46.0, 31.0],
                                                    0xe14d,
                                                    false,
                                                    true,
                                                )
                                                .clicked()
                                                {
                                                    copy_sound = Some(sound.id);
                                                }
                                            });
                                        });
                                    });
                            });
                        }
                    });

                    if row_index + 1 < row_count {
                        ui.add_space(spacing);
                    }
                }
                ui.add_space(208.0);

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
                        }
                    }
                }
                if let Some(sound_id) = open_sound {
                    self.open_sound_from_library(sound_id);
                }
            });
    }

    fn draw_video_library_grid(&mut self, ui: &mut Ui) {
        let modal_open = self.has_modal_panel();
        let videos = self
            .video_assets
            .iter()
            .filter(|video| Self::library_query_matches(&video.name, &self.library_video_query))
            .cloned()
            .collect::<Vec<_>>();
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

        let spacing = 16.0;
        let side_padding = 10.0;
        let available_width = (ui.available_width() - side_padding * 2.0).max(180.0);
        let target_card = (204.0 * self.library_grid_scale).clamp(150.0, 220.0);
        let columns =
            (((available_width + spacing) / (target_card + spacing)).floor() as usize).max(1);
        let card_size = ((available_width - spacing * (columns.saturating_sub(1)) as f32)
            / columns as f32)
            .clamp(150.0, 220.0);
        let row_count = videos.len().div_ceil(columns);
        let mut open_video: Option<VideoAsset> = None;
        let mut copy_video: Option<VideoAsset> = None;
        let mut delete_video: Option<Uuid> = None;

        for (row_index, row) in videos.chunks(columns).enumerate() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(spacing, spacing);
                if side_padding > 0.0 {
                    ui.add_space(side_padding);
                }
                let row_width =
                    row.len() as f32 * card_size + row.len().saturating_sub(1) as f32 * spacing;
                let left_pad = ((available_width - row_width) * 0.5).max(0.0);
                if left_pad > 0.0 {
                    ui.add_space(left_pad);
                }

                for video in row {
                    let (tile_rect, tile_response) =
                        ui.allocate_exact_size(vec2(card_size, card_size), Sense::hover());
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
                    let hovered =
                        !modal_open && (tile_response.hovered() || body_response.hovered());
                    if hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if !modal_open && body_response.clicked() {
                        open_video = Some(video.clone());
                    }

                    ui.scope_builder(egui::UiBuilder::new().max_rect(tile_rect), |ui| {
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
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                let inner_size = card_size - 32.0;
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
                                    Frame::new()
                                        .fill(if hovered {
                                            Color32::from_rgba_premultiplied(255, 255, 255, 22)
                                        } else {
                                            Self::panel_fill()
                                        })
                                        .corner_radius(20.0)
                                        .inner_margin(Margin::same(12))
                                        .show(ui, |ui| {
                                            ui.set_min_height((card_size * 0.38).clamp(58.0, 78.0));
                                            ui.vertical_centered(|ui| {
                                                Self::draw_wave_strip(
                                                    ui,
                                                    &video.waveform,
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
                                                    (card_size * 0.38).clamp(58.0, 78.0),
                                                );
                                            });
                                        });
                                    ui.add_space(9.0);
                                    ui.label(
                                        RichText::new(format_time(video.duration_secs))
                                            .size(11.5)
                                            .color(meta_color),
                                    );
                                    ui.add_space(10.0);
                                    ui.horizontal(|ui| {
                                        if Self::icon_action(ui, [46.0, 31.0], 0xe89e, false, false)
                                            .clicked()
                                        {
                                            open_video = Some(video.clone());
                                        }
                                        if Self::icon_action(ui, [46.0, 31.0], 0xe14d, false, false)
                                            .clicked()
                                        {
                                            copy_video = Some(video.clone());
                                        }
                                        if Self::icon_action(ui, [46.0, 31.0], 0xe872, false, false)
                                            .clicked()
                                        {
                                            delete_video = Some(video.id);
                                        }
                                    });
                                });
                            });
                    });
                }
            });

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
            }
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
        let viewport_id = Self::record_overlay_viewport_id();
        if !snapshot.running {
            if self.record_overlay_open {
                self.record_overlay_open = false;
                self.record_overlay_native_visuals_applied = false;
                ctx.send_viewport_cmd_to(viewport_id, ViewportCommand::Close);
            }
            return;
        }
        self.record_overlay_open = true;

        let overlay_size = vec2(430.0, 118.0);
        let mut builder = ViewportBuilder::default()
            .with_title(RECORD_OVERLAY_TITLE)
            .with_inner_size(overlay_size)
            .with_min_inner_size(overlay_size)
            .with_max_inner_size(overlay_size)
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_top();

        if self.center_record_overlay_next_frame
            && let Some(monitor) = ctx.input(|input| input.viewport().monitor_size)
        {
            builder = builder.with_position(Pos2::new(
                ((monitor.x - overlay_size.x) * 0.5).max(0.0),
                ((monitor.y - overlay_size.y) * 0.5).max(0.0),
            ));
        }

        ctx.show_viewport_immediate(viewport_id, builder, |overlay_ctx, class| {
            if class != ViewportClass::Immediate {
                return;
            }

            let mut style = (*overlay_ctx.style()).clone();
            style.visuals.window_fill = Color32::TRANSPARENT;
            style.visuals.panel_fill = Color32::TRANSPARENT;
            style.visuals.extreme_bg_color = Color32::TRANSPARENT;
            style.visuals.faint_bg_color = Color32::TRANSPARENT;
            style.visuals.code_bg_color = Color32::TRANSPARENT;
            overlay_ctx.set_style(style);

            if self.center_record_overlay_next_frame {
                overlay_ctx.send_viewport_cmd(ViewportCommand::InnerSize(overlay_size));
                if let Some(center_cmd) = ViewportCommand::center_on_screen(overlay_ctx) {
                    overlay_ctx.send_viewport_cmd(center_cmd);
                }
            }
            if overlay_ctx.input(|input| input.viewport().close_requested()) {
                overlay_ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            overlay_ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));

            CentralPanel::default()
                .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(0.0))
                .show(overlay_ctx, |ui| {
                    self.render_record_blob_overlay(ui, overlay_ctx, &snapshot);
                });
        });

        self.center_record_overlay_next_frame = false;
        if !self.record_overlay_native_visuals_applied
            && platform::set_overlay_window_native_visuals(RECORD_OVERLAY_TITLE, false, true)
        {
            self.record_overlay_native_visuals_applied = true;
        }
    }

    fn render_record_blob_overlay(
        &self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &crate::recorder::RecorderSnapshot,
    ) {
        let rect = ui.max_rect().shrink2(vec2(8.0, 8.0));
        let response = ui.interact(
            rect,
            ui.id().with("record-blob-overlay-drag"),
            Sense::click_and_drag(),
        );
        if response.drag_started() {
            overlay_ctx.send_viewport_cmd(ViewportCommand::StartDrag);
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
            Frame::new()
                .fill(Self::surface_fill())
                .stroke(Stroke::new(1.0, Self::border_color()))
                .corner_radius(18.0)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Self::icon(0xe8b6, 16.0, Self::muted_text_color()));
                        ui.add_sized(
                            [ui.available_width(), 24.0],
                            TextEdit::singleline(&mut self.library_audio_query)
                                .hint_text("Search")
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
            ui.add_space(12.0);
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let visible_sounds = self
                        .sounds
                        .iter()
                        .filter(|sound| {
                            Self::library_query_matches(&sound.name, &self.library_audio_query)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if visible_sounds.is_empty() {
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

                    let mut preview_request = None;
                    let mut drag_request = None;

                    for sound in &visible_sounds {
                        let selected = self.selected == Some(sound.id);
                        let playing = self
                            .audio
                            .as_ref()
                            .is_some_and(|audio| audio.is_playing(sound.id));
                        let progress = self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.playback_progress(sound.id));
                        let frame = Frame::new()
                            .fill(if selected {
                                if self.dark_theme {
                                    Color32::from_rgb(60, 25, 52)
                                } else {
                                    Color32::from_rgb(255, 239, 247)
                                }
                            } else {
                                Self::surface_fill()
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if selected {
                                    Color32::from_rgb(235, 118, 171)
                                } else {
                                    Self::border_color()
                                },
                            ))
                            .shadow(Shadow {
                                offset: [0, 10],
                                blur: 24,
                                spread: 0,
                                color: if selected {
                                    Color32::from_rgba_premultiplied(138, 45, 93, 26)
                                } else {
                                    Color32::from_rgba_premultiplied(82, 48, 70, 14)
                                },
                            })
                            .corner_radius(28.0)
                            .inner_margin(Margin::same(18))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&sound.name)
                                        .size(16.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                ui.add_space(8.0);
                                let waveform_preview = Self::trimmed_waveform_preview(sound);
                                Self::draw_wave_strip(
                                    ui,
                                    &waveform_preview,
                                    progress,
                                    Color32::from_rgb(214, 51, 132),
                                    Color32::from_rgb(238, 213, 227),
                                    Self::panel_fill(),
                                    52.0,
                                );
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format_time(sound.trimmed_length()))
                                            .size(12.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    ui.separator();
                                    ui.label(
                                        RichText::new(format!("{:.0}%", sound.volume * 100.0))
                                            .size(12.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    if playing {
                                        ui.separator();
                                        ui.label(Self::icon(
                                            0xe050,
                                            14.0,
                                            Color32::from_rgb(214, 51, 132),
                                        ));
                                    }
                                });
                            });

                        let response = ui.interact(
                            frame.response.rect,
                            ui.id().with(sound.id),
                            Sense::click_and_drag(),
                        );
                        let pointer_hover = ui
                            .ctx()
                            .input(|input| input.pointer.hover_pos())
                            .is_some_and(|pos| frame.response.rect.contains(pos));
                        let pointer_drag_active =
                            Self::pointer_drag_active(ui.ctx(), frame.response.rect);
                        if !self.suppress_sound_drag_until_release
                            && ui.ctx().input(|input| input.pointer.primary_down())
                            && Self::pointer_press_origin_within(ui.ctx(), frame.response.rect)
                        {
                            self.pending_sound_drag = Some(sound.id);
                        }
                        if pointer_hover {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                        if !self.suppress_sound_drag_until_release
                            && (response.dragged() || pointer_drag_active)
                        {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        }
                        if self.pending_sound_drag == Some(sound.id)
                            && !self.suppress_sound_drag_until_release
                            && (response.dragged() || pointer_drag_active)
                        {
                            drag_request = Some(sound.id);
                            self.pending_sound_drag = None;
                        }
                        if response.clicked() {
                            self.selected = Some(sound.id);
                            preview_request = Some(sound.id);
                        }
                        ui.add_space(12.0);
                    }

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
        });
    }

    fn draw_editor(&mut self, ui: &mut Ui, ctx: &Context) {
        let Some(index) = self.selected_sound_index() else {
            Self::draw_empty_editor(ui);
            return;
        };

        let sound_id = self.sounds[index].id;
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

        let mut preview_toggle = false;
        let mut delete_request = false;
        let mut copy_request = false;
        let mut commit_trim_request = false;
        let mut seek_request = false;
        let mut changed = false;
        let mut trim_timeline_zoom = self.trim_timeline_zoom;
        let editor_timeline_interactive = !self.has_modal_panel();

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
            .inner_margin(Margin::same(28))
            .show(ui, |ui| {
                let sound = &mut self.sounds[index];
                let controls_width = 52.0 + 52.0 + 64.0 + 64.0 + 36.0;
                let name_width = (ui.available_width() - controls_width).max(180.0);

                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        [name_width, 50.0],
                        TextEdit::singleline(&mut sound.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(name_width)
                            .margin(Vec2::new(14.0, 14.0)),
                    );
                    if response.changed() {
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_action(ui, [52.0, 34.0], 0xe872, false, false).clicked() {
                            delete_request = true;
                        }
                        if Self::icon_action(ui, [52.0, 34.0], 0xe14e, false, false).clicked() {
                            commit_trim_request = true;
                        }
                        if Self::icon_action(ui, [64.0, 34.0], 0xe14d, false, false).clicked() {
                            copy_request = true;
                        }
                        if Self::icon_action(
                            ui,
                            [64.0, 34.0],
                            if is_playing { 0xe047 } else { 0xe037 },
                            is_playing,
                            false,
                        )
                        .clicked()
                        {
                            preview_toggle = true;
                        }
                    });
                });

                ui.add_space(24.0);

                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        let (timeline_changed, timeline_seek_request) = Self::draw_trim_timeline(
                            ui,
                            sound,
                            &mut preview_cursor_secs,
                            &mut trim_timeline_zoom,
                            !is_playing,
                            editor_timeline_interactive,
                        );
                        changed |= timeline_changed;
                        seek_request |= timeline_seek_request;
                    });

                ui.add_space(18.0);

                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        Self::with_slider_visuals(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Self::icon(0xe050, 16.0, Self::muted_text_color()));
                                let volume_response = ui.add(
                                    Slider::new(&mut sound.volume, 0.0..=5.0)
                                        .show_value(false)
                                        .step_by(0.01),
                                );
                                ui.label(
                                    RichText::new(format!("{:.0}%", sound.volume * 100.0))
                                        .size(14.0)
                                        .color(Self::strong_text_color()),
                                );
                                ui.add_space(10.0);
                                ui.label(Self::icon(0xe9e4, 16.0, Self::muted_text_color()));
                                let speed_response = ui.add(
                                    Slider::new(&mut sound.speed, 0.25..=2.0)
                                        .show_value(false)
                                        .step_by(0.01),
                                );
                                ui.label(
                                    RichText::new(format!("{:.2}x", sound.speed))
                                        .size(14.0)
                                        .color(Self::strong_text_color()),
                                );
                                if volume_response.changed() || speed_response.changed() {
                                    changed = true;
                                }
                            });
                        });
                    });
            });

        let sound_id = self.sounds[index].id;
        let sound_duration = self.sounds[index].safe_duration();
        self.trim_timeline_zoom = trim_timeline_zoom;
        self.set_preview_cursor_secs(sound_id, preview_cursor_secs, sound_duration);

        if seek_request && is_playing {
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

        if commit_trim_request {
            self.show_trim_commit_panel = true;
        }

        if preview_toggle {
            if is_playing {
                self.stop_preview();
            } else {
                let sound = self.sounds[index].clone();
                let mut cursor_secs = self.preview_cursor_secs_for(&sound);
                if cursor_secs >= sound.trim_end_secs - 0.02 {
                    cursor_secs = sound.trim_start_secs;
                    self.set_preview_cursor_secs(sound_id, cursor_secs, sound_duration);
                }
                self.preview_sound_from_position(sound_id, Some(cursor_secs));
            }
        }
    }

    fn draw_empty_editor(ui: &mut Ui) {
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
                ui.vertical_centered(|ui| {
                    ui.add_space(160.0);
                    ui.label(
                        RichText::new("+")
                            .size(48.0)
                            .color(Color32::from_rgb(214, 51, 132)),
                    );
                });
            });
    }

    fn draw_trim_timeline(
        ui: &mut Ui,
        sound: &mut SoundEffect,
        preview_cursor_secs: &mut f32,
        zoom: &mut f32,
        clamp_cursor_to_trim: bool,
        interactive: bool,
    ) -> (bool, bool) {
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
        let stored_zoom_scroll_offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(zoom_scroll_offset_id));
        let mut next_scroll_offset = stored_zoom_scroll_offset;
        let timeline_size = vec2((viewport_width * *zoom).max(viewport_width), 160.0);
        let dark_theme = Self::dark_theme_enabled();
        let mut changed = false;
        let mut seek_requested = false;

        ui.allocate_ui_with_layout(
            vec2(viewport_width, timeline_size.y + 10.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                let mut scroll_area = ScrollArea::horizontal()
                    .id_salt((sound.id, "trim-timeline-scroll"))
                    .auto_shrink([false, false]);
                if let Some(offset) = stored_zoom_scroll_offset {
                    scroll_area = scroll_area.horizontal_scroll_offset(offset);
                }
                scroll_area.show(ui, |ui| {
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
                        &sound.waveform,
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
                        let current_offset = next_scroll_offset
                            .unwrap_or_else(|| (viewport_rect.left() - rect.left()).max(0.0));
                        next_scroll_offset = Some((current_offset + delta).clamp(0.0, max_offset));
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
                            next_scroll_offset = Some(
                                (next_anchor_content_x - anchor_viewport_x).clamp(0.0, max_offset),
                            );
                            ui.ctx().request_repaint();
                        }

                        let move_left = ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Q)
                        });
                        let move_right = ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::W)
                        });

                        if let Some(pointer_time) = pointer_time {
                            if move_left {
                                sound.trim_start_secs =
                                    pointer_time.min(sound.trim_end_secs - 0.05);
                                sound.clamp_trim();
                                changed = true;
                            }
                            if move_right {
                                sound.trim_end_secs =
                                    pointer_time.max(sound.trim_start_secs + 0.05);
                                sound.clamp_trim();
                                changed = true;
                            }
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
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
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
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
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

                    if !interactive || !ui.input(|input| input.pointer.primary_down()) {
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                    }

                    if clamp_cursor_to_trim {
                        let clamped_cursor = (*preview_cursor_secs)
                            .clamp(sound.trim_start_secs, sound.trim_end_secs);
                        if (clamped_cursor - *preview_cursor_secs).abs() > f32::EPSILON {
                            *preview_cursor_secs = clamped_cursor;
                            seek_requested = true;
                        }
                    }

                    if next_scroll_offset.is_none() {
                        next_scroll_offset = Some((viewport_rect.left() - rect.left()).max(0.0));
                    }
                });
                if let Some(offset) = next_scroll_offset {
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(zoom_scroll_offset_id, offset);
                    });
                }
            },
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
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

        (changed, seek_requested)
    }

    fn trimmed_waveform_preview(sound: &SoundEffect) -> Vec<f32> {
        if sound.waveform.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration();
        let trim_start = sound.trim_start_secs.clamp(0.0, total_duration);
        let trim_end = sound
            .trim_end_secs
            .clamp(trim_start + 0.001, total_duration);
        let source_len = sound.waveform.len();
        if source_len <= 1 || (trim_start <= 0.001 && trim_end >= total_duration - 0.001) {
            return sound.waveform.clone();
        }

        let start_index = ((trim_start / total_duration) * source_len as f32).floor() as usize;
        let mut end_index = ((trim_end / total_duration) * source_len as f32).ceil() as usize;
        end_index = end_index.clamp(start_index.saturating_add(1), source_len);
        let segment = &sound.waveform[start_index.min(source_len - 1)..end_index];
        if segment.len() >= source_len {
            return segment.to_vec();
        }

        let mut preview = Vec::with_capacity(source_len);
        for target_index in 0..source_len {
            let start =
                ((target_index as f32 / source_len as f32) * segment.len() as f32).floor() as usize;
            let mut end = ((((target_index + 1) as f32) / source_len as f32) * segment.len() as f32)
                .ceil() as usize;
            let start = start.min(segment.len().saturating_sub(1));
            end = end.clamp(start + 1, segment.len());

            let mut peak = 0.0_f32;
            for sample in &segment[start..end] {
                peak = peak.max(*sample);
            }
            preview.push(peak.max(0.02));
        }

        preview
    }

    fn paint_waveform_bars(
        painter: &egui::Painter,
        rect: Rect,
        waveform: &[f32],
        start_x: f32,
        end_x: f32,
        progress: Option<f32>,
    ) {
        let dark_theme = Self::dark_theme_enabled();
        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.center().y),
                    Pos2::new(rect.right(), rect.center().y),
                ],
                Stroke::new(
                    2.0,
                    if dark_theme {
                        Color32::from_rgba_premultiplied(255, 255, 255, 140)
                    } else {
                        Color32::from_rgb(221, 214, 220)
                    },
                ),
            );
            if let Some(progress) = progress {
                let play_x = egui::lerp(rect.left()..=rect.right(), progress.clamp(0.0, 1.0));
                painter.line_segment(
                    [
                        Pos2::new(play_x, rect.top()),
                        Pos2::new(play_x, rect.bottom()),
                    ],
                    Stroke::new(
                        2.0,
                        if dark_theme {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(42, 39, 44)
                        },
                    ),
                );
            }
            return;
        }

        let bar_width = rect.width() / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.05, 1.0);
            let center_x = rect.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * rect.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.3).max(1.0),
                    rect.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.3).max(1.0),
                    rect.center().y + half,
                ),
            );
            let selected = center_x >= start_x && center_x <= end_x;
            let color = if selected {
                if dark_theme {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(227, 82, 149)
                }
            } else {
                if dark_theme {
                    Color32::from_rgba_premultiplied(255, 255, 255, 188)
                } else {
                    Color32::from_rgb(234, 214, 226)
                }
            };
            painter.rect_filled(wave_rect, 2.0, color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(start_x..=end_x.max(start_x + 1.0), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, rect.top()),
                    Pos2::new(play_x, rect.bottom()),
                ],
                Stroke::new(
                    2.0,
                    if dark_theme {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(34, 31, 36)
                    },
                ),
            );
        }
    }

    fn draw_record_wave_strip(ui: &mut Ui, waveform: &[f32]) {
        let desired = vec2(ui.available_width().max(240.0), 56.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        let dark_theme = Self::dark_theme_enabled();
        painter.rect_filled(
            rect,
            24.0,
            if dark_theme {
                Color32::from_rgba_premultiplied(255, 255, 255, 18)
            } else {
                Color32::from_rgba_premultiplied(255, 255, 255, 120)
            },
        );
        painter.rect_stroke(
            rect,
            24.0,
            Stroke::new(
                1.0,
                if dark_theme {
                    Color32::from_rgba_premultiplied(106, 86, 118, 180)
                } else {
                    Color32::from_rgba_premultiplied(235, 219, 229, 180)
                },
            ),
            StrokeKind::Outside,
        );

        let data = if waveform.is_empty() {
            vec![0.05; 40]
        } else {
            waveform.to_vec()
        };
        let inner = rect.shrink2(vec2(14.0, 10.0));
        let bar_width = inner.width() / data.len().max(1) as f32;
        for (index, value) in data.iter().enumerate() {
            let x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = value.clamp(0.04, 1.0) * inner.height() * 0.46;
            let bar = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.18, inner.center().y - half),
                Pos2::new(x + bar_width * 0.18, inner.center().y + half),
            );
            painter.rect_filled(bar, 4.0, Color32::from_rgb(227, 82, 149));
        }
    }

    fn draw_wave_strip(
        ui: &mut Ui,
        waveform: &[f32],
        progress: Option<f32>,
        active_color: Color32,
        idle_color: Color32,
        background_color: Color32,
        height: f32,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 14.0, background_color);

        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 12.0, rect.center().y),
                    Pos2::new(rect.right() - 12.0, rect.center().y),
                ],
                Stroke::new(2.0, idle_color),
            );
            return;
        }

        let inner = rect.shrink2(vec2(10.0, 8.0));
        let bar_width = inner.width() / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.06, 1.0);
            let center_x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * inner.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.28).max(1.0),
                    inner.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.28).max(1.0),
                    inner.center().y + half,
                ),
            );
            painter.rect_filled(wave_rect, 2.0, idle_color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(inner.left()..=inner.right(), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, inner.top()),
                    Pos2::new(play_x, inner.bottom()),
                ],
                Stroke::new(2.0, active_color),
            );
        }
    }

    fn render_download_panel(&mut self, ctx: &Context) {
        if !self.show_download_panel {
            return;
        }

        let snapshot = self.downloader.snapshot();
        let mut open_panel = self.show_download_panel;
        let mut should_start_download = false;
        let mut add_to_library = false;
        let mut open_file = false;
        let mut open_folder = false;
        let mut clear_result = false;
        let mut minimize_request = false;
        let mut close_request = false;

        egui::Window::new("")
            .id(egui::Id::new("youtube-audio-download"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(520.0, 260.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                    ui.label(Self::icon(0xe2c4, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            clear_result = !snapshot.running;
                            close_request = true;
                        }
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe15b, false, false).clicked() {
                            minimize_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                Self::render_download_site_badges(ui);

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        [ui.available_width() - 32.0, 42.0],
                        TextEdit::singleline(&mut self.download_url)
                            .hint_text("https://youtube.com/watch?v=... or soundcloud / tiktok / facebook")
                            .desired_width(f32::INFINITY)
                            .margin(Vec2::new(14.0, 12.0)),
                    );
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && !snapshot.running
                    {
                        should_start_download = true;
                    }

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

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let start_button = ui.add_enabled(
                        !snapshot.running && !self.download_url.trim().is_empty(),
                        Button::new(Self::icon(0xe2c4, 16.0, Color32::WHITE))
                            .fill(Color32::from_rgb(214, 51, 132))
                            .stroke(Stroke::NONE)
                            .corner_radius(18.0),
                    );
                    Self::decorate_button_response(ui, &start_button);
                    if start_button.clicked() {
                        should_start_download = true;
                    }

                    if snapshot.running {
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
        let mut clear_youtube_results = false;
        let mut more_request = false;

        egui::Window::new("")
            .id(egui::Id::new("myinstants-search-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(720.0, 620.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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
                    let response = ui.add_sized(
                        [(ui.available_width() - button_group_width).max(180.0), 42.0],
                        TextEdit::singleline(&mut self.myinstants_query)
                            .hint_text("sound effect")
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
                            RichText::new("OR")
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
                    ui.label(
                        RichText::new(if youtube_snapshot.searching || youtube_snapshot.running {
                            youtube_snapshot.stage.as_str()
                        } else {
                            "..."
                        })
                        .size(14.0)
                        .color(Color32::from_rgb(214, 51, 132)),
                    );
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
                                if Self::render_youtube_result_row(ui, result) {
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

        egui::Window::new("")
            .id(egui::Id::new("trim-commit-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(360.0, 180.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
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

                ui.add_space(18.0);
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
            self.duplicate_selected_trimmed_sound();
        }
        if replace_current {
            self.show_trim_commit_panel = false;
            self.commit_selected_trimmed_sound(ctx);
        }
    }

    fn transition_progress(&mut self, ctx: &Context) -> Option<(TransitionPhase, f32)> {
        let phase = self.startup.phase;
        if phase == TransitionPhase::Live {
            return None;
        }

        let now = ctx.input(|input| input.time);
        let started_at = self.startup.started_at.get_or_insert(now);
        let progress =
            ((now - *started_at) / self.startup.duration_sec as f64).clamp(0.0, 1.0) as f32;

        if progress >= 1.0 {
            match phase {
                TransitionPhase::Intro => {
                    self.startup.phase = TransitionPhase::Live;
                    self.startup.started_at = None;
                    self.startup.live_started_at = Some(now);
                    self.startup.duration_sec = 0.0;
                    return None;
                }
                TransitionPhase::Outro => {
                    if !self.startup.close_sent {
                        self.finalize_close_cleanup(ctx);
                        if let Some(audio) = self.audio.as_mut() {
                            audio.stop();
                        }
                        self.startup.close_sent = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
                    return Some((phase, 1.0));
                }
                TransitionPhase::Live => {}
            }
        }

        ctx.request_repaint();
        Some((phase, progress))
    }

    fn live_ui_reveal_progress(&mut self, ctx: &Context) -> f32 {
        let Some(started_at) = self.startup.live_started_at else {
            return 1.0;
        };

        let now = ctx.input(|input| input.time);
        let progress = ((now - started_at) / LIVE_UI_FADE_SEC as f64).clamp(0.0, 1.0) as f32;
        if progress >= 1.0 {
            self.startup.live_started_at = None;
            return 1.0;
        }

        Self::ease_in_out_cubic(progress)
    }

    fn render_transition_layer(&self, ctx: &Context, progress: f32, phase: TransitionPhase) {
        let screen_rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("transition-layer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen_rect.size(), Sense::click_and_drag());
                let _ = ui.interact(
                    rect,
                    ui.id().with("transition-layer"),
                    Sense::click_and_drag(),
                );
                let painter = ui.painter_at(rect);
                let time = ctx.input(|input| input.time) as f32;
                let audio_progress = self.transition_audio_progress(ctx).unwrap_or(progress);
                let audio_level =
                    Self::sample_waveform_level(&self.startup.sound_waveform, audio_progress);
                let wave_bars =
                    Self::transition_wave_bars(&self.startup.sound_waveform, audio_progress, 11);
                let center = rect.center();
                let intro_monochrome = self.dark_theme && phase == TransitionPhase::Intro;
                let intro_light_fade = !self.dark_theme && phase == TransitionPhase::Intro;
                let light_outro = !self.dark_theme && phase == TransitionPhase::Outro;
                let light_transition = intro_light_fade || light_outro;
                let light_intro_base = Color32::from_rgb(206, 198, 211);
                let light_intro_berry = Color32::from_rgb(188, 152, 174);
                let light_intro_magenta = Color32::from_rgb(171, 120, 149);
                let light_intro_plum = Color32::from_rgb(145, 94, 126);
                let light_intro_deep = Color32::from_rgb(117, 70, 104);
                let light_wave_primary = Color32::from_rgb(214, 51, 132);
                let light_wave_secondary = Color32::from_rgb(229, 85, 149);
                let light_wave_glow = (246, 124, 181);
                let (rose_ice, berry, magenta, plum, deep_plum, star_rgb, star_alpha_scale) =
                    if intro_monochrome {
                        (
                            Color32::from_rgb(26, 22, 31),
                            Color32::from_rgb(42, 37, 48),
                            Color32::from_rgb(56, 51, 64),
                            Color32::from_rgb(23, 19, 28),
                            Color32::from_rgb(7, 4, 10),
                            (255, 255, 255),
                            0.16,
                        )
                    } else if self.dark_theme {
                        (
                            Color32::from_rgb(15, 7, 13),
                            Color32::from_rgb(190, 63, 129),
                            Color32::from_rgb(132, 37, 89),
                            Color32::from_rgb(29, 11, 23),
                            Color32::from_rgb(7, 4, 10),
                            (255, 214, 234),
                            0.30,
                        )
                    } else if intro_light_fade {
                        (
                            light_intro_base,
                            light_intro_berry,
                            light_intro_magenta,
                            light_intro_plum,
                            light_intro_deep,
                            (255, 226, 238),
                            0.72,
                        )
                    } else {
                        (
                            Color32::from_rgb(255, 233, 242),
                            Color32::from_rgb(205, 58, 126),
                            Color32::from_rgb(168, 36, 104),
                            Color32::from_rgb(76, 24, 58),
                            Color32::from_rgb(30, 10, 24),
                            (255, 246, 252),
                            1.0,
                        )
                    };
                let t = match phase {
                    TransitionPhase::Intro => Self::ease_in_out_cubic(progress),
                    TransitionPhase::Outro => 1.0 - Self::ease_in_out_cubic(progress),
                    TransitionPhase::Live => 1.0,
                };
                let layer_alpha = match phase {
                    TransitionPhase::Intro => 1.0,
                    TransitionPhase::Outro => 1.0,
                    TransitionPhase::Live => 1.0,
                };
                let ornament_alpha = match phase {
                    TransitionPhase::Intro => {
                        1.0 - Self::ease_in_out_cubic(((progress - 0.18) / 0.18).clamp(0.0, 1.0))
                    }
                    TransitionPhase::Outro => 1.0,
                    TransitionPhase::Live => 1.0,
                };
                let ui_match: f32 = match phase {
                    TransitionPhase::Intro => 0.0,
                    TransitionPhase::Outro => 0.0,
                    TransitionPhase::Live => 1.0,
                };
                let aura = ((1.0 - t) * (0.75 + audio_level * 0.5)).clamp(0.0, 1.0);
                let mut overlay = match phase {
                    TransitionPhase::Intro => (1.0 - t * 0.7).clamp(0.0, 1.0),
                    TransitionPhase::Outro => Self::ease_in_out_cubic(progress),
                    TransitionPhase::Live => 0.0,
                };
                if intro_light_fade {
                    overlay = 0.0;
                }
                let intro_black_fade = if intro_monochrome {
                    Self::ease_in_out_cubic(((progress - 0.58) / 0.28).clamp(0.0, 1.0))
                } else {
                    0.0
                };
                let target_rect = Self::transition_target_rect(rect);
                let base = rect.width().min(rect.height()).clamp(260.0, 440.0);
                let half_w = egui::lerp((base * 0.17)..=(target_rect.width() * 0.5), t);
                let half_h = egui::lerp((base * 0.13)..=(target_rect.height() * 0.5), t);
                let exponent = egui::lerp(2.2..=6.4, t);
                let wobble = (1.0 - t).powf(1.4) * 0.24;
                let square_seed = ((t - 0.08) / 0.66).clamp(0.0, 1.0);
                let square_morph = square_seed * square_seed * (3.0 - 2.0 * square_seed);
                let ornament_alpha = if phase == TransitionPhase::Intro && progress >= 0.50 {
                    0.0
                } else {
                    ornament_alpha * (1.0 - square_morph).powf(1.7)
                };
                let content_alpha = if intro_light_fade {
                    1.0
                } else {
                    ornament_alpha
                };
                let mut card_fill = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Self::lerp_color(
                                Color32::from_rgba_premultiplied(
                                    16,
                                    13,
                                    20,
                                    (236.0 + (1.0 - intro_black_fade) * 12.0) as u8,
                                ),
                                Self::page_fill(),
                                intro_black_fade,
                            )
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(12, 9, 15, (232.0 + t * 18.0) as u8)
                        } else {
                            Color32::from_rgba_premultiplied(
                                255,
                                247,
                                251,
                                (208.0 + t * 28.0) as u8,
                            )
                        },
                        Self::page_fill(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha,
                );
                let mut card_stroke = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Self::lerp_color(
                                Color32::from_rgba_premultiplied(
                                    98,
                                    92,
                                    108,
                                    (92.0 + (1.0 - intro_black_fade) * 28.0) as u8,
                                ),
                                Self::border_color(),
                                intro_black_fade,
                            )
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(232, 162, 202, (84.0 + t * 56.0) as u8)
                        } else {
                            Color32::from_rgba_premultiplied(
                                229,
                                168,
                                199,
                                (116.0 + t * 68.0) as u8,
                            )
                        },
                        Self::border_color(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha,
                );
                let mut glaze_fill = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Color32::TRANSPARENT
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                214,
                                234,
                                (20.0 + (1.0 - aura) * 18.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                rose_ice.r(),
                                rose_ice.g(),
                                rose_ice.b(),
                                (40.0 + (1.0 - aura) * 28.0) as u8,
                            )
                        },
                        Self::page_fill(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha * ornament_alpha,
                );
                if intro_light_fade {
                    card_fill = Color32::from_rgb(206, 198, 211);
                    glaze_fill = Color32::TRANSPARENT;
                    card_stroke = Color32::from_rgb(156, 108, 136);
                }
                let wave_color = if light_transition {
                    Color32::from_rgb(214, 51, 132)
                } else {
                    Self::with_alpha(
                        if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                248,
                                252,
                                (118.0 + t * 120.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                magenta.r(),
                                magenta.g(),
                                magenta.b(),
                                (92.0 + t * 132.0) as u8,
                            )
                        },
                        layer_alpha * content_alpha,
                    )
                };
                let ribbon_color = if light_transition {
                    Color32::from_rgb(229, 85, 149)
                } else {
                    Self::with_alpha(
                        if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                250,
                                252,
                                (136.0 + t * 108.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                berry.r(),
                                berry.g(),
                                berry.b(),
                                (118.0 + t * 124.0) as u8,
                            )
                        },
                        layer_alpha * content_alpha,
                    )
                };
                let note_base = if intro_monochrome {
                    Color32::from_rgb(246, 243, 248)
                } else if self.dark_theme {
                    Color32::from_rgb(246, 124, 181)
                } else if light_transition {
                    light_wave_primary
                } else {
                    Color32::from_rgb(214, 51, 132)
                };
                let note_alt = if intro_monochrome {
                    Color32::from_rgb(223, 216, 228)
                } else if self.dark_theme {
                    Color32::from_rgb(255, 188, 219)
                } else if light_transition {
                    light_wave_secondary
                } else {
                    Color32::from_rgb(236, 116, 179)
                };
                let note_glow_rgb = if intro_monochrome {
                    (255, 255, 255)
                } else if self.dark_theme {
                    (227, 82, 149)
                } else if light_transition {
                    light_wave_glow
                } else {
                    (16, 10, 14)
                };

                if self.dark_theme {
                    painter.circle_filled(
                        center,
                        base * 0.56,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(9, 6, 12, (34.0 + aura * 54.0) as u8),
                            layer_alpha,
                        ),
                    );
                    painter.circle_filled(
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.42,
                        Self::with_alpha(
                            if intro_monochrome {
                                Color32::from_rgba_premultiplied(
                                    52,
                                    47,
                                    61,
                                    (14.0 + aura * 18.0) as u8,
                                )
                            } else {
                                Color32::from_rgba_premultiplied(
                                    120,
                                    25,
                                    72,
                                    (16.0 + aura * 34.0) as u8,
                                )
                            },
                            layer_alpha * ornament_alpha,
                        ),
                    );
                } else if phase != TransitionPhase::Live {
                    painter.circle_filled(
                        center,
                        base * 0.52,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_intro_base.r(),
                                light_intro_base.g(),
                                light_intro_base.b(),
                                (86.0 + aura * 64.0).round().clamp(0.0, 255.0) as u8,
                            ),
                            layer_alpha * ornament_alpha.max(0.45),
                        ),
                    );
                    painter.circle_filled(
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.40,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_wave_secondary.r(),
                                light_wave_secondary.g(),
                                light_wave_secondary.b(),
                                (72.0 + aura * 56.0).round().clamp(0.0, 255.0) as u8,
                            ),
                            layer_alpha * ornament_alpha,
                        ),
                    );
                }

                for star_index in 0..16 {
                    let seed = star_index as f32 * 11.713;
                    let px =
                        rect.left() + rect.width() * (0.18 + ((seed.sin() * 0.5 + 0.5) * 0.64));
                    let py = rect.top()
                        + rect.height() * (0.16 + (((seed * 1.7).cos() * 0.5 + 0.5) * 0.54));
                    let twinkle = 0.48 + ((time * 1.7 + seed).sin() * 0.52).abs();
                    painter.circle_filled(
                        Pos2::new(px, py),
                        0.8 + (star_index % 3) as f32 * 0.35,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                star_rgb.0,
                                star_rgb.1,
                                star_rgb.2,
                                (26.0 * star_alpha_scale * twinkle * (0.35 + aura * 0.65)) as u8,
                            ),
                            layer_alpha * ornament_alpha,
                        ),
                    );
                }

                let aura_layers = [
                    (
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.23,
                        base * 0.18,
                        Color32::from_rgba_premultiplied(
                            deep_plum.r(),
                            deep_plum.g(),
                            deep_plum.b(),
                            (74.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            plum.r(),
                            plum.g(),
                            plum.b(),
                            (52.0 + aura * 72.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x, center.y + base * 0.018),
                        base * 0.31,
                        base * 0.24,
                        Color32::from_rgba_premultiplied(
                            plum.r(),
                            plum.g(),
                            plum.b(),
                            (58.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            magenta.r(),
                            magenta.g(),
                            magenta.b(),
                            (58.0 + aura * 82.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x, center.y + base * 0.016),
                        base * 0.39,
                        base * 0.3,
                        Color32::from_rgba_premultiplied(
                            magenta.r(),
                            magenta.g(),
                            magenta.b(),
                            (42.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            berry.r(),
                            berry.g(),
                            berry.b(),
                            (50.0 + aura * 96.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x, center.y + base * 0.022),
                        base * 0.47,
                        base * 0.35,
                        Color32::from_rgba_premultiplied(
                            berry.r(),
                            berry.g(),
                            berry.b(),
                            (28.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            rose_ice.r(),
                            rose_ice.g(),
                            rose_ice.b(),
                            (46.0 + aura * 88.0) as u8,
                        ),
                    ),
                ];
                for (layer_index, (layer_center, radius_x, radius_y, fill, stroke)) in
                    aura_layers.into_iter().enumerate()
                {
                    let stage = target_rect.shrink(layer_index as f32 * 12.0);
                    let points = Self::morph_squircle_to_rect(
                        layer_center,
                        radius_x,
                        radius_y,
                        2.6 + layer_index as f32 * 0.18,
                        (0.18 - layer_index as f32 * 0.02).max(0.08),
                        time + layer_index as f32 * 0.16,
                        stage,
                        square_morph,
                    );
                    painter.add(egui::Shape::convex_polygon(
                        points,
                        Self::with_alpha(fill, layer_alpha * ornament_alpha),
                        Stroke::new(
                            (1.8 - layer_index as f32 * 0.24) * (1.0 - square_morph * 0.3),
                            Self::with_alpha(stroke, layer_alpha * ornament_alpha),
                        ),
                    ));
                }

                for (radius, alpha) in [
                    (base * 0.48, 28.0),
                    (base * 0.38, 42.0),
                    (base * 0.28, 68.0),
                    (base * 0.2, 96.0),
                ] {
                    painter.circle_filled(
                        center,
                        egui::lerp((radius * 0.75)..=radius, 1.0 - aura * 0.22),
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                berry.r(),
                                berry.g(),
                                berry.b(),
                                (alpha * (0.2 + aura * 0.8)) as u8,
                            ),
                            layer_alpha * ornament_alpha,
                        ),
                    );
                }

                let shadow_points = Self::morph_squircle_to_rect(
                    Pos2::new(center.x, center.y + 14.0 + aura * 12.0),
                    half_w * 1.02,
                    half_h * 1.02,
                    exponent,
                    wobble * 0.55,
                    time - 0.35,
                    target_rect.expand(8.0),
                    square_morph,
                );
                painter.add(egui::Shape::convex_polygon(
                    shadow_points,
                    Self::with_alpha(
                        Color32::from_rgba_premultiplied(
                            deep_plum.r(),
                            deep_plum.g(),
                            deep_plum.b(),
                            ((36.0 + t * 42.0) * (1.0 - square_morph * 0.82)) as u8,
                        ),
                        layer_alpha,
                    ),
                    Stroke::NONE,
                ));

                let card_points = Self::morph_squircle_to_rect(
                    center,
                    half_w,
                    half_h,
                    exponent,
                    wobble,
                    time,
                    target_rect,
                    square_morph,
                );
                painter.add(egui::Shape::convex_polygon(
                    card_points.clone(),
                    card_fill,
                    Stroke::new((1.2 - square_morph * 0.55).max(0.35), card_stroke),
                ));
                let rounded_card_lock_seed = ((square_morph - 0.9) / 0.1).clamp(0.0, 1.0);
                let rounded_card_lock = rounded_card_lock_seed
                    * rounded_card_lock_seed
                    * (3.0 - 2.0 * rounded_card_lock_seed);
                if rounded_card_lock > 0.0 {
                    painter.rect(
                        target_rect,
                        CornerRadius::same(APP_FRAME_RADIUS as u8),
                        Self::with_alpha(card_fill, rounded_card_lock),
                        Stroke::new(
                            egui::lerp(
                                (1.2 - square_morph * 0.55).max(0.35)..=1.0,
                                rounded_card_lock,
                            ),
                            Self::with_alpha(card_stroke, rounded_card_lock),
                        ),
                        StrokeKind::Outside,
                    );
                }
                let glaze_center = Pos2::new(
                    center.x,
                    egui::lerp(
                        (center.y - half_h * 0.06)
                            ..=(target_rect.top() + target_rect.height() * 0.22),
                        square_morph,
                    ),
                );
                let glaze_size = vec2(
                    egui::lerp((half_w * 1.84)..=(target_rect.width() * 0.84), square_morph),
                    egui::lerp((half_h * 0.96)..=(target_rect.height() * 0.4), square_morph),
                );
                let glaze_rect = Rect::from_center_size(glaze_center, glaze_size);
                let glaze_points = Self::morph_squircle_to_rect(
                    glaze_center,
                    half_w * 0.92,
                    half_h * 0.54,
                    exponent,
                    wobble * 0.4,
                    time + 0.8,
                    glaze_rect,
                    square_morph * 0.94,
                );
                painter.add(egui::Shape::convex_polygon(
                    glaze_points,
                    glaze_fill,
                    Stroke::NONE,
                ));
                if rounded_card_lock > 0.0 {
                    let top_glaze_rect = Rect::from_min_max(
                        Pos2::new(target_rect.left() + 18.0, target_rect.top() + 14.0),
                        Pos2::new(
                            target_rect.right() - 18.0,
                            target_rect.top() + target_rect.height() * 0.34,
                        ),
                    );
                    painter.rect(
                        top_glaze_rect,
                        CornerRadius::same(((APP_FRAME_RADIUS * 0.85).round() as u8).max(8)),
                        Self::with_alpha(glaze_fill, rounded_card_lock * 0.92),
                        Stroke::NONE,
                        StrokeKind::Outside,
                    );
                }

                let inner_rect = Rect::from_center_size(
                    center,
                    vec2(half_w * 1.18, half_h * 1.02).min(target_rect.size() * 0.86),
                );
                let panel_fill = if intro_monochrome {
                    Color32::from_rgba_premultiplied(34, 30, 40, 210)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(27, 22, 33, 224)
                } else {
                    Color32::from_rgba_premultiplied(248, 244, 248, 244)
                };
                let panel_stroke = if intro_monochrome {
                    Color32::from_rgba_premultiplied(92, 86, 102, 132)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(109, 84, 116, 148)
                } else if light_transition {
                    Color32::from_rgba_premultiplied(198, 166, 184, 184)
                } else {
                    Color32::from_rgba_premultiplied(223, 198, 213, 176)
                };
                let panel_shadow = if intro_monochrome {
                    Color32::from_rgba_premultiplied(8, 7, 10, 48)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(9, 7, 12, 64)
                } else {
                    Color32::from_rgba_premultiplied(132, 90, 113, 26)
                };

                let hero_rect = Rect::from_center_size(
                    Pos2::new(center.x, center.y - half_h * 0.23),
                    vec2(inner_rect.width() * 0.68, 44.0 + t * 16.0),
                );
                let hero_fill = if intro_monochrome {
                    Color32::from_rgba_premultiplied(43, 38, 50, 190)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(39, 30, 45, 204)
                } else {
                    Color32::from_rgba_premultiplied(255, 250, 253, 238)
                };
                let detail_fill = if intro_monochrome {
                    Color32::from_rgba_premultiplied(80, 73, 88, 150)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(232, 162, 202, 108)
                } else if light_transition {
                    Color32::from_rgba_premultiplied(214, 51, 132, 114)
                } else {
                    Color32::from_rgba_premultiplied(229, 85, 149, 96)
                };
                let detail_secondary = if intro_monochrome {
                    Color32::from_rgba_premultiplied(72, 66, 80, 118)
                } else if self.dark_theme {
                    Color32::from_rgba_premultiplied(248, 226, 238, 84)
                } else {
                    Color32::from_rgba_premultiplied(218, 196, 210, 160)
                };
                painter.rect(
                    hero_rect,
                    CornerRadius::same(20),
                    Self::with_alpha(hero_fill, layer_alpha * ornament_alpha.max(0.56)),
                    Stroke::new(
                        1.0,
                        Self::with_alpha(panel_stroke, layer_alpha * ornament_alpha.max(0.5)),
                    ),
                    StrokeKind::Outside,
                );
                let hero_pill_rect = Rect::from_center_size(
                    Pos2::new(hero_rect.center().x, hero_rect.center().y - 4.0),
                    vec2(hero_rect.width() * 0.42, 10.0),
                );
                painter.rect_filled(
                    hero_pill_rect,
                    6.0,
                    Self::with_alpha(detail_fill, layer_alpha * ornament_alpha.max(0.66)),
                );
                let hero_meta_rect = Rect::from_center_size(
                    Pos2::new(hero_rect.center().x, hero_rect.center().y + 14.0),
                    vec2(hero_rect.width() * 0.28, 6.0),
                );
                painter.rect_filled(
                    hero_meta_rect,
                    4.0,
                    Self::with_alpha(detail_secondary, layer_alpha * ornament_alpha.max(0.58)),
                );

                let module_rect = Rect::from_center_size(
                    Pos2::new(center.x, center.y + half_h * 0.22),
                    vec2(inner_rect.width() * 0.62, inner_rect.height() * 0.54),
                );
                painter.rect(
                    module_rect.translate(vec2(0.0, 10.0 + aura * 5.0)),
                    CornerRadius::same(26),
                    Self::with_alpha(panel_shadow, layer_alpha * ornament_alpha.max(0.54)),
                    Stroke::NONE,
                    StrokeKind::Outside,
                );
                painter.rect(
                    module_rect,
                    CornerRadius::same(26),
                    Self::with_alpha(panel_fill, layer_alpha * ornament_alpha.max(0.72)),
                    Stroke::new(
                        1.0,
                        Self::with_alpha(panel_stroke, layer_alpha * ornament_alpha.max(0.64)),
                    ),
                    StrokeKind::Outside,
                );

                let module_inner = module_rect.shrink2(vec2(24.0, 20.0));
                let clip = painter.with_clip_rect(module_inner.expand2(vec2(8.0, 8.0)));
                let accent_rect = Rect::from_center_size(
                    Pos2::new(module_rect.center().x, module_rect.top() + 26.0),
                    vec2(module_rect.width() * 0.56, 14.0 + t * 3.0),
                );
                let band_rect = Rect::from_min_max(
                    Pos2::new(module_inner.left(), module_rect.top() + 56.0),
                    Pos2::new(module_inner.right(), module_rect.top() + 140.0),
                );
                if light_transition {
                    clip.rect_filled(
                        band_rect.expand2(vec2(18.0, 14.0)),
                        18.0,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_wave_secondary.r(),
                                light_wave_secondary.g(),
                                light_wave_secondary.b(),
                                52,
                            ),
                            layer_alpha * content_alpha,
                        ),
                    );
                }
                let bar_width = band_rect.width() / wave_bars.len().max(1) as f32;
                for (index, bar) in wave_bars.iter().enumerate() {
                    let phase_shift = time * 3.2 + index as f32 * 0.44;
                    let animated = (*bar + 0.06_f32 * phase_shift.sin()).clamp(0.16, 1.0);
                    let x = band_rect.left() + (index as f32 + 0.5) * bar_width;
                    let half = animated * band_rect.height() * (0.2 + t * 0.3);
                    let wave_rect = Rect::from_min_max(
                        Pos2::new(x - bar_width * 0.22, band_rect.center().y - half),
                        Pos2::new(x + bar_width * 0.22, band_rect.center().y + half),
                    );
                    clip.rect_filled(wave_rect, 4.0, wave_color);
                }

                let ribbon_rect = Rect::from_min_max(
                    Pos2::new(module_inner.left(), module_rect.bottom() - 84.0),
                    Pos2::new(module_inner.right(), module_rect.bottom() - 26.0),
                );
                if light_transition {
                    clip.rect_filled(
                        ribbon_rect.expand2(vec2(22.0, 16.0)),
                        20.0,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_wave_primary.r(),
                                light_wave_primary.g(),
                                light_wave_primary.b(),
                                44,
                            ),
                            layer_alpha * content_alpha,
                        ),
                    );
                }
                let mut line = Vec::with_capacity(120);
                for step in 0..120 {
                    let sample_t = step as f32 / 119.0;
                    let x = egui::lerp(ribbon_rect.left()..=ribbon_rect.right(), sample_t);
                    let ribbon_energy = Self::sample_waveform_level(
                        &self.startup.sound_waveform,
                        (audio_progress + (sample_t - 0.5) * 0.18).clamp(0.0, 1.0),
                    );
                    let y = ribbon_rect.center().y
                        + (sample_t * std::f32::consts::TAU * 2.2 + time * 2.9).sin()
                            * ribbon_rect.height()
                            * (0.18 + ribbon_energy * 0.34)
                        + (sample_t * std::f32::consts::TAU * 5.8 - time * 1.5).cos()
                            * ribbon_rect.height()
                            * (0.08 + ribbon_energy * 0.14);
                    line.push(Pos2::new(x, y));
                }
                clip.add(egui::Shape::line(line, Stroke::new(4.0, ribbon_color)));
                clip.rect_filled(
                    accent_rect,
                    9.0,
                    if light_transition {
                        Color32::from_rgb(229, 85, 149)
                    } else {
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                rose_ice.r(),
                                rose_ice.g(),
                                rose_ice.b(),
                                (34.0 + t * 38.0) as u8,
                            ),
                            layer_alpha * content_alpha,
                        )
                    },
                );

                for index in 0..7 {
                    let angle = time * 0.72 + index as f32 * 0.9;
                    let orbit = egui::lerp(
                        (module_rect.width() * 0.28)..=(module_rect.width() * 0.14),
                        t,
                    ) + (index % 3) as f32 * 10.0
                        + audio_level * 8.0;
                    let note_anchor = Pos2::new(
                        module_rect.center().x,
                        module_rect.top() + module_rect.height() * 0.4,
                    );
                    let note_pos = Pos2::new(
                        note_anchor.x + angle.cos() * orbit,
                        note_anchor.y + angle.sin() * orbit * 0.62,
                    );
                    let note_scale = 0.64 + (index % 3) as f32 * 0.12 + audio_level * 0.12;
                    let note_alpha = if light_transition {
                        (196.0 + aura * 52.0).clamp(0.0, 248.0) as u8
                    } else {
                        (160.0 * aura).clamp(0.0, 160.0) as u8
                    };
                    let note_color = if index % 2 == 0 {
                        if light_transition {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_base.r(),
                                    note_base.g(),
                                    note_base.b(),
                                    note_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        } else {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_base.r(),
                                    note_base.g(),
                                    note_base.b(),
                                    note_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        }
                    } else {
                        if light_transition {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_alt.r(),
                                    note_alt.g(),
                                    note_alt.b(),
                                    note_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        } else {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_alt.r(),
                                    note_alt.g(),
                                    note_alt.b(),
                                    note_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        }
                    };
                    let glow_alpha = if light_transition {
                        (92.0 + aura * 54.0).clamp(0.0, 168.0) as u8
                    } else if self.dark_theme {
                        (110.0 * aura).clamp(0.0, 148.0) as u8
                    } else {
                        (88.0 * aura).clamp(0.0, 128.0) as u8
                    };
                    Self::paint_glowing_music_note(
                        &painter,
                        note_pos,
                        note_scale,
                        angle.sin() * 0.18,
                        note_color,
                        if light_transition {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_glow_rgb.0,
                                    note_glow_rgb.1,
                                    note_glow_rgb.2,
                                    glow_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        } else {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_glow_rgb.0,
                                    note_glow_rgb.1,
                                    note_glow_rgb.2,
                                    glow_alpha,
                                ),
                                layer_alpha * content_alpha,
                            )
                        },
                    );
                }
            });
    }

    fn transition_audio_progress(&self, ctx: &Context) -> Option<f32> {
        let started_at = self.startup.started_at?;
        let duration_sec = self.startup.sound_duration_sec.max(0.0);
        if duration_sec <= 0.0 {
            return None;
        }

        let now = ctx.input(|input| input.time);
        Some(((now - started_at) as f32 / duration_sec).clamp(0.0, 1.0))
    }

    fn with_alpha(color: Color32, factor: f32) -> Color32 {
        let factor = factor.clamp(0.0, 1.0);
        Color32::from_rgba_premultiplied(
            color.r(),
            color.g(),
            color.b(),
            ((color.a() as f32) * factor).round().clamp(0.0, 255.0) as u8,
        )
    }

    fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        Color32::from_rgba_premultiplied(
            egui::lerp(from.r() as f32..=to.r() as f32, t).round() as u8,
            egui::lerp(from.g() as f32..=to.g() as f32, t).round() as u8,
            egui::lerp(from.b() as f32..=to.b() as f32, t).round() as u8,
            egui::lerp(from.a() as f32..=to.a() as f32, t).round() as u8,
        )
    }

    fn sample_waveform_level(waveform: &[f32], progress: f32) -> f32 {
        if waveform.is_empty() {
            return 0.45;
        }
        if waveform.len() == 1 {
            return waveform[0].clamp(0.05, 1.0);
        }

        let position = progress.clamp(0.0, 1.0) * (waveform.len() - 1) as f32;
        let left_index = position.floor() as usize;
        let right_index = position.ceil() as usize;
        let mix = (position - left_index as f32).clamp(0.0, 1.0);
        let left = waveform[left_index].clamp(0.05, 1.0);
        let right = waveform[right_index.min(waveform.len() - 1)].clamp(0.05, 1.0);
        egui::lerp(left..=right, mix)
    }

    fn transition_wave_bars(waveform: &[f32], progress: f32, count: usize) -> Vec<f32> {
        if count == 0 {
            return Vec::new();
        }
        if waveform.is_empty() {
            return vec![
                0.24, 0.42, 0.76, 0.94, 0.56, 0.28, 0.68, 0.88, 0.5, 0.22, 0.62,
            ];
        }

        let mut bars = Vec::with_capacity(count);
        let window = 0.26;
        let middle = (count.saturating_sub(1)) as f32 * 0.5;
        for index in 0..count {
            let offset = if count <= 1 {
                0.0
            } else {
                (index as f32 - middle) / middle.max(1.0)
            };
            let sample_progress = (progress + offset * window).clamp(0.0, 1.0);
            bars.push(Self::sample_waveform_level(waveform, sample_progress));
        }
        bars
    }

    fn transition_target_rect(rect: Rect) -> Rect {
        Rect::from_min_max(
            Pos2::new(
                rect.left() + APP_OUTER_MARGIN,
                rect.top() + APP_OUTER_MARGIN,
            ),
            Pos2::new(
                rect.right() - APP_OUTER_MARGIN,
                rect.bottom() - APP_OUTER_MARGIN,
            ),
        )
    }

    fn squircle_points(
        center: Pos2,
        half_w: f32,
        half_h: f32,
        exponent: f32,
        wobble: f32,
        time: f32,
    ) -> Vec<Pos2> {
        let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);
        for step in 0..TRANSITION_POINT_COUNT {
            let angle = step as f32 / TRANSITION_POINT_COUNT as f32 * std::f32::consts::TAU;
            let cos = angle.cos();
            let sin = angle.sin();
            let power = 2.0 / exponent.max(2.0);
            let x = cos.signum() * cos.abs().powf(power) * half_w;
            let y = sin.signum() * sin.abs().powf(power) * half_h;
            let drift = 1.0
                + wobble * (angle * 3.0 + time * 1.4).sin()
                + wobble * 0.45 * (angle * 5.0 - time * 0.9).cos();
            points.push(Pos2::new(center.x + x * drift, center.y + y * drift));
        }
        points
    }

    fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<Pos2> {
        let half_w = rect.width().max(1.0) * 0.5;
        let half_h = rect.height().max(1.0) * 0.5;
        let radius = radius.min(half_w).min(half_h).max(0.0);
        let inner_half_w = (half_w - radius).max(0.0);
        let inner_half_h = (half_h - radius).max(0.0);
        let right = rect.right();
        let left = rect.left();
        let top = rect.top();
        let bottom = rect.bottom();
        let center_y = rect.center().y;

        if radius <= 0.0 {
            let segments = [
                ((right, center_y), (right, bottom)),
                ((right, bottom), (left, bottom)),
                ((left, bottom), (left, top)),
                ((left, top), (right, top)),
                ((right, top), (right, center_y)),
            ];
            let total = (bottom - center_y)
                + rect.width()
                + rect.height()
                + rect.width()
                + (center_y - top);
            let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);
            for step in 0..TRANSITION_POINT_COUNT {
                let mut distance = step as f32 / TRANSITION_POINT_COUNT as f32 * total;
                for &((x1, y1), (x2, y2)) in &segments {
                    let length = (x2 - x1).abs() + (y2 - y1).abs();
                    if distance <= length || length <= f32::EPSILON {
                        let t = if length <= f32::EPSILON {
                            0.0
                        } else {
                            distance / length
                        };
                        points.push(Pos2::new(egui::lerp(x1..=x2, t), egui::lerp(y1..=y2, t)));
                        break;
                    }
                    distance -= length;
                }
            }
            return points;
        }

        let right_half = inner_half_h;
        let vertical = inner_half_h * 2.0;
        let horizontal = inner_half_w * 2.0;
        let arc = std::f32::consts::FRAC_PI_2 * radius;
        let total = right_half * 2.0 + vertical + horizontal * 2.0 + arc * 4.0;
        let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);

        for step in 0..TRANSITION_POINT_COUNT {
            let mut distance = step as f32 / TRANSITION_POINT_COUNT as f32 * total;

            if distance <= right_half {
                points.push(Pos2::new(right, center_y + distance));
                continue;
            }
            distance -= right_half;

            if distance <= arc {
                let angle = egui::lerp(
                    0.0..=std::f32::consts::FRAC_PI_2,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    right - radius + angle.cos() * radius,
                    bottom - radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= horizontal {
                points.push(Pos2::new(right - radius - distance, bottom));
                continue;
            }
            distance -= horizontal;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::FRAC_PI_2..=std::f32::consts::PI,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    left + radius + angle.cos() * radius,
                    bottom - radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= vertical {
                points.push(Pos2::new(left, bottom - radius - distance));
                continue;
            }
            distance -= vertical;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::PI..=std::f32::consts::PI * 1.5,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    left + radius + angle.cos() * radius,
                    top + radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= horizontal {
                points.push(Pos2::new(left + radius + distance, top));
                continue;
            }
            distance -= horizontal;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::PI * 1.5..=std::f32::consts::TAU,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    right - radius + angle.cos() * radius,
                    top + radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            points.push(Pos2::new(right, top + radius + distance));
        }

        points
    }

    fn morph_squircle_to_rect(
        center: Pos2,
        half_w: f32,
        half_h: f32,
        exponent: f32,
        wobble: f32,
        time: f32,
        target_rect: Rect,
        morph: f32,
    ) -> Vec<Pos2> {
        let blob = Self::squircle_points(center, half_w, half_h, exponent, wobble, time);
        if morph <= 0.0 {
            return blob;
        }

        let settled_blob = Self::squircle_points(
            target_rect.center(),
            target_rect.width() * 0.5,
            target_rect.height() * 0.5,
            egui::lerp(exponent.max(2.0)..=8.8, morph),
            wobble * (1.0 - morph * 0.82).max(0.0),
            time,
        );
        let rounded_card = Self::rounded_rect_points(target_rect, APP_FRAME_RADIUS);
        let corner_lock = morph.clamp(0.0, 1.0).powf(2.4);
        let target_shape = settled_blob
            .into_iter()
            .zip(rounded_card)
            .map(|(blob_point, rounded_point)| {
                Pos2::new(
                    egui::lerp(blob_point.x..=rounded_point.x, corner_lock),
                    egui::lerp(blob_point.y..=rounded_point.y, corner_lock),
                )
            })
            .collect::<Vec<_>>();
        let eased_morph = morph * morph * (3.0 - 2.0 * morph);

        blob.into_iter()
            .zip(target_shape)
            .map(|(blob_point, target_point)| {
                Pos2::new(
                    egui::lerp(blob_point.x..=target_point.x, eased_morph),
                    egui::lerp(blob_point.y..=target_point.y, eased_morph),
                )
            })
            .collect()
    }

    fn ease_in_out_cubic(t: f32) -> f32 {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        }
    }

    fn paint_music_note(
        painter: &egui::Painter,
        center: Pos2,
        scale: f32,
        tilt: f32,
        color: Color32,
    ) {
        let head = Vec2::new(12.0 * scale, 8.0 * scale);
        let stem = 18.0 * scale;
        let head_center = Pos2::new(center.x, center.y + 6.0 * scale);
        painter.circle_filled(head_center, head.y * 0.8, color);
        let stem_top = Pos2::new(head_center.x + head.x * 0.5, head_center.y - stem);
        painter.line_segment(
            [
                Pos2::new(head_center.x + head.x * 0.4, head_center.y),
                stem_top,
            ],
            Stroke::new((2.8 * scale).max(1.0), color),
        );
        let flag_end = Pos2::new(
            stem_top.x + (12.0 * scale) * (1.0 + tilt),
            stem_top.y + 8.0 * scale,
        );
        painter.line_segment(
            [stem_top, flag_end],
            Stroke::new((2.4 * scale).max(1.0), color),
        );
    }

    fn paint_glowing_music_note(
        painter: &egui::Painter,
        center: Pos2,
        scale: f32,
        tilt: f32,
        color: Color32,
        glow_color: Color32,
    ) {
        for (glow_scale, alpha_scale) in [(1.34, 0.22), (1.2, 0.38), (1.08, 0.62)] {
            let alpha = ((glow_color.a() as f32) * alpha_scale).clamp(0.0, 255.0) as u8;
            let glow = Color32::from_rgba_unmultiplied(
                glow_color.r(),
                glow_color.g(),
                glow_color.b(),
                alpha,
            );
            Self::paint_music_note(painter, center, scale * glow_scale, tilt, glow);
        }
        Self::paint_music_note(painter, center, scale, tilt, color);
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

    fn render_download_site_badges(ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for badge in Self::download_site_badges() {
                let fill = if Self::dark_theme_enabled() {
                    Color32::from_rgb(29, 25, 35)
                } else {
                    Color32::from_rgb(255, 251, 254)
                };
                let stroke = if Self::dark_theme_enabled() {
                    Color32::from_rgb(82, 67, 90)
                } else {
                    Color32::from_rgb(230, 221, 229)
                };
                let response = Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, stroke))
                    .corner_radius(14.0)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                        let painter = ui.painter_at(icon_rect);
                        Self::paint_download_site_icon(&painter, icon_rect, badge);
                    })
                    .response
                    .on_hover_text(badge.name);
                Self::decorate_button_response(ui, &response);
            }
        });
    }

    fn youtube_search_button(ui: &mut Ui, enabled: bool) -> egui::Response {
        let desired = vec2(152.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::TRANSPARENT
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        let badge = DownloadSiteBadge {
            name: "YouTube",
            kind: DownloadSiteKind::Youtube,
            color: Color32::from_rgb(255, 77, 141),
        };
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            vec2(18.0, 18.0),
        );
        Self::paint_download_site_icon(ui.painter(), icon_rect, badge);
        ui.painter().text(
            Pos2::new(rect.left() + 34.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Search YouTube",
            egui::FontId::proportional(13.0),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn search_sound_button(ui: &mut Ui, enabled: bool) -> egui::Response {
        let desired = vec2(36.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            char::from_u32(0xe8b6).unwrap_or(' '),
            egui::FontId::new(16.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn render_youtube_result_row(ui: &mut Ui, result: &YoutubeSearchResult) -> bool {
        let mut download_clicked = false;
        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(22.0)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let badge = DownloadSiteBadge {
                        name: "YouTube",
                        kind: DownloadSiteKind::Youtube,
                        color: Color32::from_rgb(255, 77, 141),
                    };
                    let (icon_rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    Self::paint_download_site_icon(ui.painter(), icon_rect, badge);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add_sized(
                            [ui.available_width().min(380.0), 18.0],
                            egui::Label::new(
                                RichText::new(&result.title)
                                    .size(13.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .truncate(),
                        );
                        let mut parts = Vec::new();
                        if let Some(duration) = result.duration {
                            parts.push(format_time(duration as f32));
                        }
                        if let Some(uploader) = &result.uploader
                            && !uploader.trim().is_empty()
                        {
                            parts.push(Self::truncate_middle_ascii(uploader, 28));
                        }
                        if let Some(view_count) = result.view_count {
                            parts.push(Self::format_compact_count(view_count));
                        }
                        if !result.id.trim().is_empty() {
                            parts.push(format!("ID {}", result.id));
                        }
                        if parts.is_empty() {
                            parts.push("YouTube".to_owned());
                        }
                        ui.label(
                            RichText::new(parts.join("  •  "))
                                .size(12.0)
                                .color(Self::muted_text_color()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let response = ui.add_sized(
                            [104.0, 34.0],
                            Self::action_button(
                                RichText::new("Download").size(13.0).color(Color32::WHITE),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            download_clicked = true;
                        }
                    });
                });
            });
        download_clicked
    }

    fn format_compact_count(value: u64) -> String {
        if value >= 1_000_000_000 {
            format!("{:.1}B views", value as f64 / 1_000_000_000.0)
        } else if value >= 1_000_000 {
            format!("{:.1}M views", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.1}K views", value as f64 / 1_000.0)
        } else {
            format!("{value} views")
        }
    }

    fn paint_download_site_icon(painter: &egui::Painter, rect: Rect, badge: DownloadSiteBadge) {
        let center = rect.center();
        let white = Color32::WHITE;
        let radius = rect.width().min(rect.height()) * 0.5;
        painter.circle_filled(center, radius, badge.color);

        match badge.kind {
            DownloadSiteKind::Youtube => {
                let body = Rect::from_center_size(center, vec2(15.0, 10.4));
                painter.rect_filled(body, 4.0, white);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(center.x - 2.6, center.y - 3.4),
                        Pos2::new(center.x - 2.6, center.y + 3.4),
                        Pos2::new(center.x + 4.2, center.y),
                    ],
                    badge.color,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::SoundCloud => {
                let base_y = center.y + 4.2;
                for (index, height) in [5.0, 6.6, 8.2, 9.2, 9.2].into_iter().enumerate() {
                    let x = center.x - 7.0 + index as f32 * 2.1;
                    let bar = Rect::from_min_max(
                        Pos2::new(x, base_y - height),
                        Pos2::new(x + 1.4, base_y + 0.8),
                    );
                    painter.rect_filled(bar, 1.0, white);
                }
                painter.circle_filled(Pos2::new(center.x + 2.4, center.y + 0.2), 4.4, white);
                painter.circle_filled(Pos2::new(center.x + 5.6, center.y + 1.4), 3.2, white);
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(center.x - 0.2, center.y + 1.0),
                        Pos2::new(center.x + 8.0, center.y + 4.8),
                    ),
                    2.0,
                    white,
                );
            }
            DownloadSiteKind::Bandcamp => {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(center.x - 6.6, center.y + 5.4),
                        Pos2::new(center.x - 0.6, center.y - 5.4),
                        Pos2::new(center.x + 7.0, center.y - 5.4),
                        Pos2::new(center.x + 1.0, center.y + 5.4),
                    ],
                    white,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::TikTok => {
                painter.line_segment(
                    [
                        Pos2::new(center.x + 2.0, center.y - 6.6),
                        Pos2::new(center.x + 2.0, center.y + 1.0),
                    ],
                    Stroke::new(2.4, white),
                );
                painter.line_segment(
                    [
                        Pos2::new(center.x + 2.0, center.y - 6.4),
                        Pos2::new(center.x + 6.2, center.y - 4.3),
                    ],
                    Stroke::new(2.4, white),
                );
                painter.circle_stroke(
                    Pos2::new(center.x - 1.8, center.y + 3.2),
                    3.3,
                    Stroke::new(2.0, white),
                );
            }
            DownloadSiteKind::Facebook => {
                painter.text(
                    Pos2::new(center.x, center.y + 0.3),
                    egui::Align2::CENTER_CENTER,
                    "f",
                    egui::FontId::proportional(18.0),
                    white,
                );
            }
            DownloadSiteKind::Instagram => {
                let body = Rect::from_center_size(center, vec2(12.8, 12.8));
                painter.rect_stroke(body, 4.0, Stroke::new(1.8, white), StrokeKind::Outside);
                painter.circle_stroke(center, 3.3, Stroke::new(1.8, white));
                painter.circle_filled(Pos2::new(center.x + 3.8, center.y - 3.8), 1.1, white);
            }
            DownloadSiteKind::X => {
                painter.line_segment(
                    [
                        Pos2::new(center.x - 5.2, center.y - 5.4),
                        Pos2::new(center.x + 5.4, center.y + 5.4),
                    ],
                    Stroke::new(2.2, white),
                );
                painter.line_segment(
                    [
                        Pos2::new(center.x + 5.2, center.y - 5.4),
                        Pos2::new(center.x - 1.0, center.y + 0.4),
                    ],
                    Stroke::new(2.2, white),
                );
            }
            DownloadSiteKind::Vimeo => {
                painter.add(egui::Shape::line(
                    vec![
                        Pos2::new(center.x - 5.8, center.y - 2.2),
                        Pos2::new(center.x - 2.0, center.y + 5.2),
                        Pos2::new(center.x + 1.6, center.y - 0.8),
                        Pos2::new(center.x + 5.2, center.y - 4.8),
                    ],
                    Stroke::new(2.4, white),
                ));
            }
            DownloadSiteKind::Twitch => {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(center.x - 6.2, center.y - 5.6),
                        Pos2::new(center.x + 5.4, center.y - 5.6),
                        Pos2::new(center.x + 5.4, center.y + 2.0),
                        Pos2::new(center.x + 1.4, center.y + 5.8),
                        Pos2::new(center.x + 1.2, center.y + 2.0),
                        Pos2::new(center.x - 6.2, center.y + 2.0),
                    ],
                    white,
                    Stroke::NONE,
                ));
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(center.x - 2.4, center.y - 2.4),
                        Pos2::new(center.x - 0.9, center.y + 1.2),
                    ),
                    0.5,
                    badge.color,
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(center.x + 0.8, center.y - 2.4),
                        Pos2::new(center.x + 2.3, center.y + 1.2),
                    ),
                    0.5,
                    badge.color,
                );
            }
            DownloadSiteKind::GoogleDrive => {
                let top = Pos2::new(center.x, center.y - 6.0);
                let left = Pos2::new(center.x - 5.8, center.y + 4.8);
                let right = Pos2::new(center.x + 5.8, center.y + 4.8);
                let upper_left = Pos2::new(center.x - 2.0, center.y - 0.3);
                let upper_right = Pos2::new(center.x + 2.0, center.y - 0.3);
                painter.line_segment(
                    [top, upper_left],
                    Stroke::new(2.3, Color32::from_rgb(255, 205, 86)),
                );
                painter.line_segment(
                    [top, upper_right],
                    Stroke::new(2.3, Color32::from_rgb(66, 133, 244)),
                );
                painter.line_segment([left, upper_left], Stroke::new(2.3, white));
                painter.line_segment([right, upper_right], Stroke::new(2.3, white));
                painter.line_segment(
                    [left, right],
                    Stroke::new(2.3, Color32::from_rgb(15, 157, 88)),
                );
            }
        }
    }
}

impl eframe::App for SoundFxApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let _ = self;
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        Self::apply_theme(ctx, self.dark_theme);
        ctx.set_cursor_icon(egui::CursorIcon::Default);
        self.center_window_if_needed(ctx);
        self.intercept_close_request(ctx);
        self.poll_myinstants_waveform_jobs();
        if !ctx.input(|input| input.pointer.primary_down()) {
            self.pending_sound_drag = None;
            self.suppress_sound_drag_until_release = false;
        } else if self.suppress_sound_drag_until_release
            && Self::pointer_primary_pressed_in_app(ctx)
        {
            self.suppress_sound_drag_until_release = false;
        }

        self.play_startup_sound_if_needed(ctx);

        let transition = self.transition_progress(ctx);
        let download_snapshot = self.downloader.snapshot();
        let wants_shadow = false;
        if self.native_shadow_applied != wants_shadow {
            platform::set_native_window_shadow(frame, wants_shadow);
            self.native_shadow_applied = wants_shadow;
        }
        let wants_transition_topmost = self.is_transition_active();
        if self.transition_window_topmost_applied != wants_transition_topmost {
            platform::set_native_window_topmost(frame, wants_transition_topmost);
            self.transition_window_topmost_applied = wants_transition_topmost;
        }

        self.enforce_square_window_if_needed(ctx);
        self.handle_space_preview(ctx);
        self.handle_record_hotkey(ctx);

        if !self.is_transition_active() {
            self.handle_dropped_files(ctx);
        }

        if let Some(audio) = self.audio.as_mut() {
            audio.tick();
            if self.myinstants_preview_audio_url.is_some() && !audio.has_active_playback() {
                self.myinstants_preview_audio_url = None;
            }
        }
        if self.playback_needs_live_repaint() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        if self.download_was_running
            && !download_snapshot.running
            && download_snapshot.last_file.is_some()
        {
            self.show_download_panel = true;
            self.stop_preview();
            self.clear_status();
        }
        self.download_was_running = download_snapshot.running;

        if download_snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }

        let recorder_snapshot = self.recorder.snapshot();
        if recorder_snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if let Some(error) = recorder_snapshot.error.clone() {
            self.set_error_status(error);
        }
        if let Some(path) = self.recorder.take_completed_path() {
            self.open_recording_review(&path);
            if self.reveal_record_review_on_open {
                Self::reveal_window(ctx);
                self.reveal_record_review_on_open = false;
            }
        }
        self.poll_record_video_export(ctx);
        if self.active_record_video_export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }

        let myinstants_snapshot = self.myinstants.snapshot();
        if myinstants_snapshot.searching || myinstants_snapshot.downloading {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        if let Some(error) = myinstants_snapshot.error.clone() {
            self.set_error_status(error);
        }
        if let Some((result, path, add_to_library)) = self.myinstants.take_completed_download() {
            self.myinstants_cached_files
                .insert(result.audio_url.clone(), path.clone());
            if add_to_library {
                self.import_downloaded_sound(&path, true);
            }
        }

        self.flush_pending_save(ctx);

        if let Some((TransitionPhase::Intro, progress)) = transition {
            self.render_transition_layer(ctx, progress, TransitionPhase::Intro);
            return;
        }

        if let Some((TransitionPhase::Outro, progress)) = transition {
            self.render_transition_layer(ctx, progress, TransitionPhase::Outro);
            return;
        }

        let live_ui_reveal = self.live_ui_reveal_progress(ctx);
        let live_ui_overlay_alpha = if self.dark_theme {
            1.0 - live_ui_reveal
        } else {
            0.0
        };
        if live_ui_overlay_alpha > 0.0 {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        let root_fill = Color32::TRANSPARENT;

        CentralPanel::default()
            .frame(Frame::new().fill(root_fill).inner_margin(0.0))
            .show(ctx, |ui| {
                let frame_response = Frame::new()
                    .fill(Self::page_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 30,
                        spread: 0,
                        color: Self::with_alpha(
                            if self.dark_theme {
                                Color32::from_rgba_premultiplied(0, 0, 0, 72)
                            } else {
                                Color32::from_rgba_premultiplied(78, 40, 63, 20)
                            },
                            live_ui_reveal,
                        ),
                    })
                    .corner_radius(CornerRadius::same(APP_FRAME_RADIUS as u8))
                    .outer_margin(Margin::same(APP_OUTER_MARGIN as i8))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        self.draw_titlebar(ui, ctx);
                        ui.add_space(14.0);

                        let content_height = ui.available_height();
                        if self.app_view == AppView::Library {
                            self.draw_library_grid(ui);
                        } else {
                            ui.horizontal_top(|ui| {
                                let library_width =
                                    (ui.available_width() * 0.34).clamp(258.0, 292.0);
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

                if live_ui_overlay_alpha > 0.0 {
                    ui.painter().rect(
                        frame_response.response.rect,
                        CornerRadius::same(APP_FRAME_RADIUS as u8),
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
                        StrokeKind::Outside,
                    );
                }
            });

        self.render_modal_backdrop(ctx);
        self.render_download_panel(ctx);
        self.render_myinstants_panel(ctx);
        self.render_import_panel(ctx);
        self.render_record_panel(ctx);
        self.render_record_review_panel(ctx);
        self.render_settings_panel(ctx);
        self.render_video_viewer_panel(ctx);
        self.render_pitch_monitor(ctx);
        self.render_trim_commit_panel(ctx);
        self.render_pitch_overlay_viewport(ctx);
        self.render_record_overlay_viewport(ctx);
        self.render_titlebar_drag_zone(ctx);
        self.render_custom_window_resize_handles(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.recorder.stop();
        self.pitch_monitor.stop();
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }
    }
}

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
