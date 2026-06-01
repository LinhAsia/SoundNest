use eframe::App;
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
const AUDIO_FILTERS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];
const APP_FRAME_RADIUS: f32 = 30.0;
const APP_OUTER_MARGIN: f32 = 0.0;
const LIVE_UI_FADE_SEC: f32 = 0.32;
const TRANSITION_POINT_COUNT: usize = 240;
const MATERIAL_ICONS_FONT: &str = "material_icons";
const ACTIVE_UI_REPAINT_MS: u64 = 33;
const JOB_POLL_REPAINT_MS: u64 = 90;
const DEFAULT_INTRO_DURATION_SEC: f32 = 1.35;
const DEFAULT_OUTRO_DURATION_SEC: f32 = 0.72;
const DEFAULT_OUTRO_PLAYBACK_DURATION_SEC: f32 = 2.0;
const DEFAULT_OUTRO_FADE_START_RATIO: f32 = 0.0;
const TRANSITION_WAVE_BUCKETS: usize = 160;
const LIBRARY_GRID_MIN_COLUMNS: usize = 3;
const LIBRARY_GRID_MAX_COLUMNS: usize = 8;
const RECORD_EXPORT_VIDEO_FPS_OPTIONS: [u32; 3] = [
    record_video::LOW_VIDEO_FPS,
    record_video::STANDARD_VIDEO_FPS,
    record_video::HIGH_VIDEO_FPS,
];
static DARK_THEME_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppView {
    Editor,
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibraryTab {
    Sounds,
    Folders,
    Videos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadPanelTab {
    Download,
    Tts,
}

pub(crate) struct RecordingDraft {
    pub(crate) sound: SoundEffect,
    pub(crate) source_path: PathBuf,
    pub(crate) source_is_temporary: bool,
    pub(crate) keep_vocal: bool,
    pub(crate) vocal_separated_path: Option<PathBuf>,
    pub(crate) keep_music: bool,
    pub(crate) music_separated_path: Option<PathBuf>,}

pub(crate) struct VideoViewerState {
    pub(crate) video: VideoAsset,
    pub(crate) frame_paths: Vec<PathBuf>,
    pub(crate) audio_path: PathBuf,
    pub(crate) progress: f32,
    pub(crate) current_frame: Option<(usize, TextureHandle, Vec2)>,}

pub(crate) struct RecordVideoExportState {
    pub(crate) progress: f32,
    pub(crate) stage: String,
    pub(crate) receiver: Receiver<RecordVideoExportMessage>,}

pub(crate) struct RecordVideoExportResult {
    pub(crate) processed_audio_path: PathBuf,
    pub(crate) video_path: PathBuf,
    pub(crate) duration_secs: f32,
    pub(crate) video_fps: u32,
    pub(crate) video_name: String,}

pub(crate) enum RecordVideoExportMessage {
    Progress { progress: f32, stage: String },
    Finished(Result<RecordVideoExportResult, String>),
}

pub(crate) enum ProcessedExportMessage {
    Finished {
        export_path: PathBuf,
        result: Result<PathBuf, String>,
    },
}

pub(crate) enum TrimCommitMessage {
    Finished {
        sound_id: Uuid,
        keep_old: bool,
        result: Result<SoundEffect, String>,
    },
}

pub(crate) enum AudioPreloadMessage {
    Finished {
        asset_path: PathBuf,
        result: Result<(u16, u32, Vec<f32>), String>,
    },
}

pub(crate) enum NormalizeMessage {
    Finished {
        sound_id: Uuid,
        result: Result<f32, String>,
    },
}

pub(crate) enum MyinstantsWaveformMessage {
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

pub(crate) enum VocalSeparationMessage {
    Finished {
        target: VocalSeparationTarget,
        kind: SeparationStemKind,
        result: Result<PathBuf, String>,
    },
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeparationStemKind {
    Vocal,
    Music,
}

#[derive(Clone)]
pub(crate) enum VocalSeparationTarget {
    RecordingReview {
        source_path: PathBuf,
    },
    LibrarySound {
        sound_id: Uuid,
        source_path: PathBuf,
    },
}

pub(crate) struct GeminiTtsResult {
    pub(crate) path: PathBuf,
    pub(crate) display_name: String,}

pub(crate) enum GeminiTtsMessage {
    Finished(Result<GeminiTtsResult, String>),
}

pub(crate) enum DemucsModelMessage {
    Finished(Result<(), String>),
    Cancelled,
}

pub(crate) enum TransitionAnalysisMessage {
    StartupReady {
        waveform: Vec<f32>,
        duration_sec: f32,
    },
}

pub(crate) enum LibraryHydrationMessage {
    Ready(Vec<SoundEffect>),
}

pub(crate) enum StreamDriverMessage {
    ProbeFinished(Result<bool, String>),
    Finished(Result<bool, String>),
}

struct GeminiVoiceOption {
    name: &'static str,
    label: &'static str,
}

const GEMINI_VOICE_OPTIONS: &[GeminiVoiceOption] = &[
    GeminiVoiceOption {
        name: "Kore",
        label: "Kore · Female",
    },
    GeminiVoiceOption {
        name: "Puck",
        label: "Puck · Male",
    },
    GeminiVoiceOption {
        name: "Charon",
        label: "Charon · Male",
    },
    GeminiVoiceOption {
        name: "Aoede",
        label: "Aoede · Female",
    },
    GeminiVoiceOption {
        name: "Fenrir",
        label: "Fenrir · Male",
    },
    GeminiVoiceOption {
        name: "Leda",
        label: "Leda · Female",
    },
    GeminiVoiceOption {
        name: "Orus",
        label: "Orus · Male",
    },
    GeminiVoiceOption {
        name: "Zephyr",
        label: "Zephyr · Female",
    },
];

#[derive(Clone, Copy)]
struct DownloadSiteBadge {
    name: &'static str,
    kind: DownloadSiteKind,
    color: Color32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
    pub(super) storage: Storage,
    pub(super) audio: Option<AudioEngine>,
    pub(super) sounds: Vec<SoundEffect>,
    pub(super) selected: Option<Uuid>,
    pub(super) status: Option<String>,
    pub(super) downloader: YoutubeAudioDownloader,
    pub(super) myinstants: MyinstantsClient,
    pub(super) download_url: String,
    pub(super) myinstants_query: String,
    pub(super) youtube_search_visible_count: usize,
    pub(super) myinstants_visible_count: usize,
    pub(super) myinstants_cached_files: HashMap<String, PathBuf>,
    pub(super) myinstants_preview_files: HashMap<String, PathBuf>,
    pub(super) myinstants_waveforms: HashMap<String, Vec<f32>>,
    pub(super) myinstants_waveform_jobs: HashSet<String>,
    pub(super) myinstants_waveform_tx: Sender<MyinstantsWaveformMessage>,
    pub(super) myinstants_waveform_rx: Receiver<MyinstantsWaveformMessage>,
    pub(super) vocal_waveform_cache: RefCell<HashMap<Uuid, Vec<f32>>>,
    pub(super) music_waveform_cache: RefCell<HashMap<Uuid, Vec<f32>>>,
    pub(super) myinstants_preview_audio_url: Option<String>,
    pub(super) show_download_panel: bool,
    pub(super) download_was_running: bool,
    pub(super) show_myinstants_panel: bool,
    pub(super) show_import_panel: bool,
    pub(super) show_record_panel: bool,
    pub(super) show_record_review_panel: bool,
    pub(super) show_pitch_panel: bool,
    pub(super) show_stream_panel: bool,
    pub(super) show_settings_panel: bool,
    pub(super) show_trim_commit_panel: bool,
    pub(super) import_dir: PathBuf,
    pub(super) import_audio_entries: Vec<PathBuf>,
    pub(super) app_view: AppView,
    pub(super) library_tab: LibraryTab,
    pub(super) folders: Vec<crate::storage::Folder>,
    pub(super) library_current_folder: Option<Uuid>,
    pub(super) editing_folder_id: Option<Uuid>,
    pub(super) folder_rename_name: String,
    pub(super) new_folder_name: String,
    pub(super) recorder: Recorder,
    pub(super) record_name: String,
    pub(super) record_input_source: PitchInputSource,
    pub(super) record_capture_devices: Vec<String>,
    pub(super) selected_record_input_device: Option<String>,
    pub(super) stream_input_capture_devices: Vec<String>,
    pub(super) selected_stream_input_device: Option<String>,
    pub(super) record_hotkeys: Vec<Hotkey>,
    pub(super) pitch_hotkeys: Vec<Hotkey>,
    pub(super) capture_record_hotkey: bool,
    pub(super) capture_pitch_hotkey: bool,
    pub(super) preview_record_hotkey: Option<Hotkey>,
    pub(super) preview_pitch_hotkey: Option<Hotkey>,
    pub(super) record_hotkey_manager: GlobalHotkeyManager,
    pub(super) record_export_video_sharps: bool,
    pub(super) record_export_video_animation: bool,
    pub(super) record_export_video_fps: u32,
    pub(super) center_record_overlay_next_frame: bool,
    pub(super) record_overlay_native_visuals_applied: bool,
    pub(super) record_overlay_pos: Option<Pos2>,
    pub(super) library_grid_columns: usize,
    pub(super) video_assets: Vec<VideoAsset>,
    pub(super) recording_draft: Option<RecordingDraft>,
    pub(super) active_record_video_export: Option<RecordVideoExportState>,
    pub(super) video_viewer: Option<VideoViewerState>,
    pub(super) pitch_monitor: PitchMonitor,
    pub(super) pitch_update_hz: f32,
    pub(super) pitch_overlay_animation: bool,
    pub(super) pitch_show_sharps: bool,
    pub(super) pitch_input_source: PitchInputSource,
    pub(super) pitch_capture_devices: Vec<String>,
    pub(super) selected_pitch_input_device: Option<String>,
    pub(super) center_pitch_overlay_next_frame: bool,
    pub(super) pitch_overlay_native_visuals_applied: bool,
    pub(super) pitch_overlay_pos: Option<Pos2>,
    pub(super) startup: StartupSplashState,
    pub(super) center_window_next_frame: bool,
    pub(super) titlebar_drag_rect: Option<Rect>,
    pub(super) app_frame_rect: Option<Rect>,
    pub(super) native_shadow_applied: bool,
    pub(super) transition_window_topmost_applied: bool,
    pub(super) record_overlay_open: bool,
    pub(super) record_overlay_pending_visible: bool,
    pub(super) overlay_only_mode: bool,
    pub(super) trim_timeline_zoom: f32,
    pub(super) preview_cursor: Option<(Uuid, f32)>,
    pub(super) dark_theme: bool,
    pub(super) app_transition_animation: bool,
    pub(super) localization: Localization,
    pub(super) startup_sound_name: Option<String>,
    pub(super) exit_sound_name: Option<String>,
    pub(super) gemini_api_key: String,
    pub(super) gemini_api_key_visible: bool,
    pub(super) settings_startup_candidate: Option<Uuid>,
    pub(super) settings_exit_candidate: Option<Uuid>,
    pub(super) settings_show_startup_sound: bool,
    pub(super) settings_show_exit_sound: bool,
    pub(super) library_audio_query: String,
    pub(super) library_audio_tag_filter: Option<String>,
    pub(super) library_video_query: String,
    pub(super) library_favorites_only_audio: bool,
    pub(super) library_favorites_only_video: bool,
    pub(super) editor_tags_input: String,
    pub(super) editor_tags_input_sound_id: Option<Uuid>,
    pub(super) copied_sound_feedback_until: HashMap<Uuid, f64>,
    pub(super) copied_video_feedback_until: HashMap<Uuid, f64>,
    pub(super) editor_drop_armed: bool,
    pub(super) editor_drop_rect: Option<Rect>,
    pub(super) pending_sound_drag: Option<Uuid>,
    pub(super) ignored_drop_path: Option<PathBuf>,
    pub(super) download_panel_tab: DownloadPanelTab,
    pub(super) tts_text: String,
    pub(super) tts_voice_name: String,
    pub(super) tts_direction_prompt: String,
    pub(super) tts_prompt_presets: Vec<GeminiTtsPromptPreset>,
    pub(super) tts_preset_name: String,
    pub(super) tts_output_name: String,
    pub(super) tts_running: bool,
    pub(super) tts_status: String,
    pub(super) tts_error: Option<String>,
    pub(super) tts_last_file: Option<PathBuf>,
    pub(super) tts_can_add_to_library: bool,
    pub(super) tts_added_to_library: bool,
    pub(super) tts_tx: Sender<GeminiTtsMessage>,
    pub(super) tts_rx: Receiver<GeminiTtsMessage>,
    pub(super) vocal_separation_running: bool,
    pub(super) vocal_separation_cancel: Option<Arc<AtomicBool>>,
    pub(super) vocal_separation_target: Option<VocalSeparationTarget>,
    pub(super) vocal_separation_kind: Option<SeparationStemKind>,
    pub(super) vocal_separation_started_at: Option<Instant>,
    pub(super) vocal_separation_last_result: Option<(VocalSeparationTarget, SeparationStemKind, f32)>,
    pub(super) vocal_separation_tx: Sender<VocalSeparationMessage>,
    pub(super) vocal_separation_rx: Receiver<VocalSeparationMessage>,
    pub(super) reveal_record_review_on_open: bool,
    pub(super) startup_sound_played: bool,
    pub(super) pending_save: bool,
    pub(super) last_edit_at: f64,
    pub(super) pending_processed_export_sound: Option<Uuid>,
    pub(super) pending_preview_after_preload: Option<(Uuid, Option<f32>)>,
    pub(super) normalize_inflight: HashSet<Uuid>,
    pub(super) trim_commit_inflight: HashSet<Uuid>,
    pub(super) processed_export_inflight: HashSet<PathBuf>,
    pub(super) processed_export_tx: Sender<ProcessedExportMessage>,
    pub(super) processed_export_rx: Receiver<ProcessedExportMessage>,
    pub(super) trim_commit_tx: Sender<TrimCommitMessage>,
    pub(super) trim_commit_rx: Receiver<TrimCommitMessage>,
    pub(super) audio_preload_inflight: HashSet<PathBuf>,
    pub(super) audio_preload_tx: Sender<AudioPreloadMessage>,
    pub(super) audio_preload_rx: Receiver<AudioPreloadMessage>,
    pub(super) normalize_tx: Sender<NormalizeMessage>,
    pub(super) normalize_rx: Receiver<NormalizeMessage>,
    pub(super) demucs_installing: bool,
    pub(super) demucs_install_error: Option<String>,
    pub(super) demucs_install_tx: Sender<Result<(), String>>,
    pub(super) demucs_install_rx: Receiver<Result<(), String>>,
    pub(super) demucs_model_loading: bool,
    pub(super) demucs_model_error: Option<String>,
    pub(super) demucs_model_ready: bool,
    pub(super) demucs_model_cancel: Option<Arc<AtomicBool>>,
    pub(super) demucs_model_tx: Sender<DemucsModelMessage>,
    pub(super) demucs_model_rx: Receiver<DemucsModelMessage>,
    pub(super) library_hydration_tx: Sender<LibraryHydrationMessage>,
    pub(super) library_hydration_rx: Receiver<LibraryHydrationMessage>,
    pub(super) transition_analysis_tx: Sender<TransitionAnalysisMessage>,
    pub(super) transition_analysis_rx: Receiver<TransitionAnalysisMessage>,
    pub(super) stream_driver_busy: bool,
    pub(super) stream_driver_checked: bool,
    pub(super) stream_driver_error: Option<String>,
    pub(super) stream_driver_installed: bool,
    pub(super) stream_driver_tx: Sender<StreamDriverMessage>,
    pub(super) stream_driver_rx: Receiver<StreamDriverMessage>,
    pub(super) stream_input_router: StreamInputRouter,
    pub(super) stream_input_system_audio: bool,
    pub(super) stream_input_microphone: bool,
    pub(super) stream_input_monitor_microphone: bool,
    pub(super) stream_input_show_waveform: bool,
    pub(super) folder_name_warning: bool,
    pub(super) folder_import_select_mode: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionPhase {
    Intro,
    Live,
    Outro,
}

pub(crate) struct StartupSplashState {
    pub(crate) phase: TransitionPhase,
    pub(crate) started_at: Option<f64>,
    pub(crate) live_started_at: Option<f64>,
    pub(crate) duration_sec: f32,
    pub(crate) close_sent: bool,
    pub(crate) sound_waveform: Vec<f32>,
    pub(crate) sound_duration_sec: f32,}

impl SoundFxApp {
    pub fn new() -> Self {
        let storage = Storage::new().unwrap_or_else(|error| panic!("Storage init failed: {error}"));
        let mut status = None;
        let sounds = storage.load_library().unwrap_or_else(|error| {
            status = Some(error.to_string());
            Vec::new()
        });
        let folders = storage.load_folders().unwrap_or_else(|error| {
            status = Some(error.to_string());
            Vec::new()
        });

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
        let (demucs_install_tx, demucs_install_rx) = mpsc::channel();
        let (demucs_model_tx, demucs_model_rx) = mpsc::channel();
        let (tts_tx, tts_rx) = mpsc::channel();
        let (library_hydration_tx, library_hydration_rx) = mpsc::channel();
        let (transition_analysis_tx, transition_analysis_rx) = mpsc::channel();
        let (stream_driver_tx, stream_driver_rx) = mpsc::channel();
        let (vocal_separation_tx, vocal_separation_rx) = mpsc::channel();
        let (processed_export_tx, processed_export_rx) = mpsc::channel();
        let (trim_commit_tx, trim_commit_rx) = mpsc::channel();
        let (audio_preload_tx, audio_preload_rx) = mpsc::channel();
        let (normalize_tx, normalize_rx) = mpsc::channel();
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
        let library_grid_columns = storage
            .load_library_grid_columns()
            .ok()
            .flatten()
            .unwrap_or(6);
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
        let stream_input_capture_devices = pitch_capture_devices.clone();
        let selected_stream_input_device = stream_input_capture_devices.first().cloned();
        let record_hotkeys = storage
            .load_record_hotkeys()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| Hotkey::from_string(&value))
            .collect::<Vec<_>>();
        let pitch_hotkeys = storage
            .load_pitch_hotkeys()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| Hotkey::from_string(&value))
            .collect::<Vec<_>>();
        let app_transition_animation = storage
            .load_app_transition_animation()
            .ok()
            .flatten()
            .unwrap_or(true);
        let mut localization = Localization::load();
        if let Ok(Some(language_code)) = storage.load_language_code() {
            localization.set_current_code(&language_code);
        }
        let startup_sound_name = storage.load_startup_sound_name().ok().flatten();
        let exit_sound_name = storage.load_exit_sound_name().ok().flatten();
        let gemini_api_key = storage
            .load_gemini_api_key()
            .ok()
            .flatten()
            .unwrap_or_default();
        let tts_prompt_presets = storage.load_tts_prompt_presets().unwrap_or_default();
        let resolved_startup_sound = storage.resolved_startup_sound_path().ok().flatten();

        let mut app = Self {
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
            vocal_waveform_cache: RefCell::new(HashMap::new()),
            music_waveform_cache: RefCell::new(HashMap::new()),
            myinstants_preview_audio_url: None,
            show_download_panel: false,
            download_was_running: false,
            show_myinstants_panel: false,
            show_import_panel: false,
            show_record_panel: false,
            show_record_review_panel: false,
            show_pitch_panel: false,
            show_stream_panel: false,
            show_settings_panel: false,
            show_trim_commit_panel: false,
            import_dir,
            import_audio_entries: Vec::new(),
            app_view: AppView::Editor,
            library_tab: LibraryTab::Sounds,
            folders,
            library_current_folder: None,
            editing_folder_id: None,
            folder_rename_name: String::new(),
            new_folder_name: String::new(),
            recorder: Recorder::new(),
            record_name: "recording".to_owned(),
            record_input_source: PitchInputSource::System,
            record_capture_devices,
            selected_record_input_device,
            stream_input_capture_devices,
            selected_stream_input_device,
            record_hotkeys,
            pitch_hotkeys,
            capture_record_hotkey: false,
            capture_pitch_hotkey: false,
            preview_record_hotkey: None,
            preview_pitch_hotkey: None,
            record_hotkey_manager: GlobalHotkeyManager::new(),
            record_export_video_sharps: pitch_show_sharps,
            record_export_video_animation: pitch_overlay_animation,
            record_export_video_fps: record_video::STANDARD_VIDEO_FPS,
            center_record_overlay_next_frame: false,
            record_overlay_native_visuals_applied: false,
            record_overlay_pos: None,
            library_grid_columns,
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
            pitch_overlay_pos: None,
            startup: StartupSplashState {
                phase: if app_transition_animation {
                    TransitionPhase::Intro
                } else {
                    TransitionPhase::Live
                },
                started_at: None,
                live_started_at: None,
                duration_sec: if app_transition_animation {
                    DEFAULT_INTRO_DURATION_SEC
                } else {
                    0.0
                },
                close_sent: false,
                sound_waveform: Vec::new(),
                sound_duration_sec: 0.0,
            },
            center_window_next_frame: true,
            titlebar_drag_rect: None,
            app_frame_rect: None,
            native_shadow_applied: false,
            transition_window_topmost_applied: false,
            record_overlay_open: false,
            record_overlay_pending_visible: false,
            overlay_only_mode: false,
            trim_timeline_zoom: 1.0,
            preview_cursor: None,
            dark_theme,
            app_transition_animation,
            localization,
            startup_sound_name,
            exit_sound_name,
            gemini_api_key,
            gemini_api_key_visible: false,
            settings_startup_candidate: None,
            settings_exit_candidate: None,
            settings_show_startup_sound: false,
            settings_show_exit_sound: false,
            library_audio_query: String::new(),
            library_audio_tag_filter: None,
            library_video_query: String::new(),
            library_favorites_only_audio: false,
            library_favorites_only_video: false,
            editor_tags_input: String::new(),
            editor_tags_input_sound_id: None,
            copied_sound_feedback_until: HashMap::new(),
            copied_video_feedback_until: HashMap::new(),
            editor_drop_armed: false,
            editor_drop_rect: None,
            pending_sound_drag: None,
            ignored_drop_path: None,
            download_panel_tab: DownloadPanelTab::Download,
            tts_text: String::new(),
            tts_voice_name: "Kore".to_owned(),
            tts_direction_prompt: String::new(),
            tts_prompt_presets,
            tts_preset_name: String::new(),
            tts_output_name: "gemini tts".to_owned(),
            tts_running: false,
            tts_status: String::new(),
            tts_error: None,
            tts_last_file: None,
            tts_can_add_to_library: false,
            tts_added_to_library: false,
            tts_tx,
            tts_rx,
            vocal_separation_running: false,
            vocal_separation_cancel: None,
            vocal_separation_target: None,
            vocal_separation_kind: None,
            vocal_separation_started_at: None,
            vocal_separation_last_result: None,
            vocal_separation_tx,
            vocal_separation_rx,
            reveal_record_review_on_open: false,
            startup_sound_played: false,
            pending_save: false,
            last_edit_at: 0.0,
            pending_processed_export_sound: None,
            pending_preview_after_preload: None,
            normalize_inflight: HashSet::new(),
            trim_commit_inflight: HashSet::new(),
            processed_export_inflight: HashSet::new(),
            processed_export_tx,
            processed_export_rx,
            trim_commit_tx,
            trim_commit_rx,
            audio_preload_inflight: HashSet::new(),
            audio_preload_tx,
            audio_preload_rx,
            normalize_tx,
            normalize_rx,
            demucs_installing: false,
            demucs_install_error: None,
            demucs_install_tx,
            demucs_install_rx,
            demucs_model_loading: false,
            demucs_model_error: None,
            demucs_model_ready: crate::vocal_separation::is_demucs_model_ready(),
            demucs_model_cancel: None,
            demucs_model_tx,
            demucs_model_rx,
            library_hydration_tx,
            library_hydration_rx,
            transition_analysis_tx,
            transition_analysis_rx,
            stream_driver_busy: false,
            stream_driver_checked: false,
            stream_driver_error: None,
            stream_driver_installed: false,
            stream_driver_tx,
            stream_driver_rx,
            stream_input_router: StreamInputRouter::new(),
            stream_input_system_audio: false,
            stream_input_microphone: false,
            stream_input_monitor_microphone: false,
            stream_input_show_waveform: true,
            folder_name_warning: false,
            folder_import_select_mode: None,
        };
        app.begin_async_library_hydration();
        app.begin_async_transition_analysis(resolved_startup_sound);
        app.begin_async_stream_driver_probe();
        app.with_initial_selection()
    }

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
        vec2(980.0, 900.0)
    }

    fn popup_safe_rect(&self, ctx: &Context) -> Rect {
        self.modal_safe_rect(ctx)
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

    fn play_startup_sound_if_needed(&mut self, ctx: &Context) {
        if !self.app_transition_animation {
            self.startup_sound_played = true;
            return;
        }
        if self.startup_sound_played || self.startup.phase != TransitionPhase::Intro {
            return;
        }

        if let Ok(Some(path)) = self.storage.resolved_startup_sound_path() {
            let _ = self.play_file_if_exists(&path);
        }
        self.startup_sound_played = true;
        self.startup.started_at = Some(ctx.input(|input| input.time));
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

        let current_folder_id = if self.library_tab == LibraryTab::Folders {
            self.library_current_folder
        } else {
            None
        };

        imported.reverse();
        for mut sound in imported {
            if let Some(folder_id) = current_folder_id {
                sound.folder_id = Some(folder_id);
            }
            self.selected = Some(sound.id);
            self.sounds.insert(0, sound);
        }

        self.library_audio_tag_filter = None;
        let _ = ignored;
        self.save_now();
    }

    fn open_sound_from_library(&mut self, sound_id: Uuid) {
        self.selected = Some(sound_id);
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
            self.schedule_audio_preload(asset_path.clone());
            self.pending_preview_after_preload = Some((sound.id, start_position_secs));
            self.clear_status();
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
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Self::centered_outer_position(
            ctx, size,
        )));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn restore_main_viewport(ctx: &Context) {
        let size = Self::desired_window_size();
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Self::centered_outer_position(
            ctx, size,
        )));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
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

    fn save_now(&mut self) -> bool {
        match self.storage.save_library(&self.sounds) {
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

    fn schedule_processed_export(&mut self, sound_id: Uuid) {
        self.pending_processed_export_sound = Some(sound_id);
    }

    fn spawn_processed_export_job(&mut self, sound_id: Uuid) {
        let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id).cloned() else {
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
            let result = Storage::export_processed_sound_at(&root_dir, &sound).map_err(|error| error.to_string());
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

        let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id).cloned() else {
            return;
        };

        let asset_path = sound.asset_path(self.storage.root_dir());
        self.normalize_inflight.insert(sound_id);
        let tx = self.normalize_tx.clone();

        thread::spawn(move || {
            let result = calculate_normalization_gain(&asset_path).map_err(|error| error.to_string());
            let _ = tx.send(NormalizeMessage::Finished { sound_id, result });
        });
    }

    fn schedule_audio_preload(&mut self, asset_path: PathBuf) {
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.has_cached_audio(&asset_path))
            || self.audio_preload_inflight.contains(&asset_path)
        {
            return;
        }

        self.audio_preload_inflight.insert(asset_path.clone());
        let tx = self.audio_preload_tx.clone();
        thread::spawn(move || {
            let result = crate::audio::AudioEngine::decode_audio_for_cache(&asset_path)
                .map_err(|error| error.to_string());
            let _ = tx.send(AudioPreloadMessage::Finished {
                asset_path,
                result,
            });
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
        let pointer_over = ui
            .ctx()
            .input(|input| input.pointer.latest_pos().or(input.pointer.hover_pos()))
            .is_some_and(|pos| response.rect.contains(pos));
        if pointer_over {
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
        let downloader_snapshot = self.downloader.snapshot();
        let download_titlebar_active = downloader_snapshot.running && !self.show_download_panel;
        ui.horizontal(|ui| {
            let drag_width = (ui.available_width() - 564.0).max(180.0);
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
            if drag_response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if drag_response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
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
                    0xe03b,
                    self.app_view == AppView::Library,
                    false,
                )
                .clicked()
                {
                    if self.app_view == AppView::Library {
                        self.app_view = AppView::Editor;
                    } else {
                        self.app_view = AppView::Library;
                    }
                    self.library_tab = LibraryTab::Sounds;
                    self.library_current_folder = None;
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

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe061, false, false).clicked() {
                    if self.recording_draft.is_some() && !self.recorder.snapshot().running {
                        self.show_record_panel = false;
                        self.show_record_review_panel = true;
                    } else {
                        self.refresh_record_capture_devices();
                        self.show_record_panel = true;
                    }
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe8b6, self.show_myinstants_panel, false)
                    .clicked()
                {
                    self.show_myinstants_panel = true;
                }

                let download_response = Self::icon_titlebar(ui, [48.0, 30.0], 0xe2c4, false, false);
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
                    self.show_download_panel = true;
                }
            });
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
                                            RichText::new(format!("Pressing: {}", preview_key.to_string()))
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
                                    .strong()
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
                            let names: Vec<String> = self.record_hotkeys.iter().map(|&k| k.to_string()).collect();
                            let _ = self.storage.save_record_hotkeys(&names);
                            if let Err(error) = self.record_hotkey_manager.set_hotkeys(&self.record_hotkeys) {
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
    }

    fn render_modal_backdrop(&self, ctx: &Context) {
        if !self.has_modal_panel() {
            return;
        }

        let rect = self.app_frame_rect.unwrap_or_else(|| ctx.screen_rect());
        let corner_radius = if self.app_frame_rect.is_some() {
            CornerRadius::same(APP_FRAME_RADIUS as u8)
        } else {
            CornerRadius::ZERO
        };
        egui::Area::new(egui::Id::new("modal-backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (backdrop_rect, _) = ui.allocate_exact_size(rect.size(), Sense::click());
                ui.painter().rect_filled(
                    backdrop_rect,
                    corner_radius,
                    Color32::from_rgba_premultiplied(22, 16, 22, 132),
                );
            });
    }

    fn main_frame_corner_radius(rect: Rect) -> CornerRadius {
        let max_radius = ((rect.width().min(rect.height()) * 0.5) - 1.0).max(0.0);
        CornerRadius::same(APP_FRAME_RADIUS.min(max_radius).round().clamp(0.0, 255.0) as u8)
    }

    fn main_frame_inner_margin(rect: Rect) -> i8 {
        let padding = (rect.width().min(rect.height()) * 0.04).clamp(18.0, 30.0);
        padding.round() as i8
    }

    fn modal_safe_rect(&self, ctx: &Context) -> Rect {
        let host_rect = self
            .app_frame_rect
            .filter(|rect| rect.width() > 1.0 && rect.height() > 1.0)
            .unwrap_or_else(|| ctx.screen_rect().shrink(18.0));
        host_rect.shrink2(vec2(APP_FRAME_RADIUS + 8.0, APP_FRAME_RADIUS + 8.0))
    }

    fn fit_modal_dimension(available: f32, desired: f32, min: f32) -> f32 {
        if available <= 1.0 {
            1.0
        } else {
            desired.min(available).max(min.min(available))
        }
    }

    fn centered_modal_placement(
        &self,
        ctx: &Context,
        desired_size: Vec2,
        min_size: Vec2,
        y_offset: f32,
    ) -> (Rect, Vec2, Pos2) {
        let safe_rect = self.modal_safe_rect(ctx);
        let panel_size = vec2(
            Self::fit_modal_dimension(safe_rect.width(), desired_size.x, min_size.x),
            Self::fit_modal_dimension(safe_rect.height(), desired_size.y, min_size.y),
        );
        let center = safe_rect.center();
        let panel_pos = Pos2::new(center.x.round(), (center.y + y_offset).round());
        (safe_rect, panel_size, panel_pos)
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
            self.centered_modal_placement(ctx, vec2(296.0, 250.0), vec2(248.0, 220.0), 0.0);
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
                            if let Some(preview_key) = self.preview_pitch_hotkey {
                                ui.add_space(6.0);
                                Frame::new()
                                    .fill(Color32::from_rgba_premultiplied(80, 70, 30, 255))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(255, 220, 80)))
                                    .corner_radius(12.0)
                                    .inner_margin(Margin::symmetric(10, 5))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(format!("Pressing: {}", preview_key.to_string()))
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
                        for &key in &self.pitch_hotkeys {
                            ui.add_space(4.0);
                            let key_text = key.to_string();
                            let chip_btn = Button::new(
                                RichText::new(key_text)
                                    .size(11.5)
                                    .color(Self::strong_text_color())
                                    .strong()
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
                            let names: Vec<String> = self.pitch_hotkeys.iter().map(|&k| k.to_string()).collect();
                            let _ = self.storage.save_pitch_hotkeys(&names);
                            if let Err(error) = self.record_hotkey_manager.set_secondary_hotkeys(&self.pitch_hotkeys) {
                                self.set_error_status(error);
                            }
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
                        }
                    });
                });

                ui.add_space(8.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
                    Frame::new()
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
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

                ui.add_space(8.0);
                ui.add_enabled_ui(!snapshot.running, |ui| {
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
                                ui.add_space(10.0);
                                let sharp_changed = ui
                                    .checkbox(
                                        &mut self.pitch_show_sharps,
                                        RichText::new(sharp_label)
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
                                    ctx.request_repaint();
                                }
                                if sharp_changed {
                                    let _ =
                                        self.storage.save_pitch_show_sharps(self.pitch_show_sharps);
                                    ctx.request_repaint();
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
        let overlay_pos =
            if self.center_pitch_overlay_next_frame || self.pitch_overlay_pos.is_none() {
                let centered = self.centered_overlay_pos(ctx, overlay_size);
                self.pitch_overlay_pos = Some(centered);
                centered
            } else {
                self.pitch_overlay_pos.unwrap_or_default()
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
            self.pitch_overlay_pos = Some(self.clamp_overlay_pos(
                ctx,
                overlay_size,
                state.left_top_pos(),
            ));
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
        let modal_open = self.has_modal_panel();
        let titlebar_drag_active = self.titlebar_drag_active(ui.ctx());
        let mut columns_changed = false;
        let mut library_slider_active = false;
        ui.horizontal(|ui| {
            if let Some(_import_folder_id) = self.folder_import_select_mode {
                let back_btn = ui.add(
                    Button::new(format!("< {}", self.t("library.exit_import_mode")))
                        .fill(Color32::from_rgb(227, 82, 149))
                        .corner_radius(10.0)
                );
                Self::decorate_button_response(ui, &back_btn);
                if back_btn.clicked() {
                    self.folder_import_select_mode = None;
                }

                ui.add_space(12.0);
                ui.label(
                    RichText::new(self.t("library.import_select_title"))
                        .font(FontId::new(16.0, FontFamily::Proportional))
                        .color(Self::strong_text_color())
                        .strong()
                );
            } else {
                let sounds_tab = ui.add_sized(
                    [84.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.audio")).size(12.5),
                        self.library_tab == LibraryTab::Sounds,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &sounds_tab);
                if sounds_tab.clicked() {
                    self.library_tab = LibraryTab::Sounds;
                }

                ui.add_space(8.0);
                let folders_tab = ui.add_sized(
                    [84.0, 30.0],
                    Self::action_button(
                        RichText::new(self.t("library.folders")).size(12.5),
                        self.library_tab == LibraryTab::Folders,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &folders_tab);
                if folders_tab.clicked() {
                    self.library_tab = LibraryTab::Folders;
                    self.library_current_folder = None;
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
            }

            if self.library_tab == LibraryTab::Folders && self.library_current_folder.is_none() {
                ui.add_space(12.0);
                let hint = self.t("library.folder_placeholder");
                let border_stroke = if self.folder_name_warning {
                    Stroke::new(1.5, Color32::from_rgb(220, 53, 69))
                } else {
                    Stroke::new(1.0, Self::border_color())
                };
                let text_edit_response = Frame::new()
                    .fill(Self::input_fill())
                    .stroke(border_stroke)
                    .corner_radius(12.0)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.add_sized(
                            [180.0, 20.0],
                            egui::TextEdit::singleline(&mut self.new_folder_name)
                                .frame(false)
                                .hint_text(hint)
                        )
                    });

                if text_edit_response.inner.changed() {
                    self.folder_name_warning = false;
                }
                
                ui.add_space(8.0);
                let create_btn = ui.add(
                    Button::new(Self::icon(0xe145, 14.0, Color32::WHITE))
                        .fill(Color32::from_rgb(227, 82, 149))
                        .corner_radius(10.0)
                );
                Self::decorate_button_response(ui, &create_btn);

                let create_clicked = create_btn.clicked() 
                    || (text_edit_response.inner.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));

                if create_clicked {
                    let name = self.new_folder_name.trim().to_owned();
                    if !name.is_empty() {
                        let new_folder = crate::storage::Folder {
                            id: Uuid::new_v4(),
                            name,
                        };
                        self.folders.push(new_folder);
                        self.new_folder_name.clear();
                        self.folder_name_warning = false;
                        let _ = self.storage.save_folders(&self.folders);
                    } else {
                        self.folder_name_warning = true;
                    }
                }
            }

            if self.library_tab == LibraryTab::Sounds || (self.library_tab == LibraryTab::Folders && self.library_current_folder.is_some()) {
                ui.add_space(12.0);
                self.draw_library_tag_filter_row(ui);
            }

            if !(self.library_tab == LibraryTab::Folders && self.library_current_folder.is_none()) {
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
                    Self::with_slider_visuals(ui, |ui| {
                        let mut slider_value = (LIBRARY_GRID_MIN_COLUMNS + LIBRARY_GRID_MAX_COLUMNS)
                            as f32
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
                            self.library_grid_columns =
                                (LIBRARY_GRID_MIN_COLUMNS + LIBRARY_GRID_MAX_COLUMNS) - reversed;
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
                });
            }
        });
        ui.add_space(10.0);
        if !(self.library_tab == LibraryTab::Folders && self.library_current_folder.is_none()) {
            Frame::new()
                .fill(Self::surface_fill())
                .stroke(Stroke::new(1.0, Self::border_color()))
                .corner_radius(18.0)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
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
        if columns_changed {
            let _ = self
                .storage
                .save_library_grid_columns(self.library_grid_columns);
        }
        ui.add_space(12.0);

        ScrollArea::vertical()
            .drag_to_scroll(false)
            .auto_shrink([true, false])
            .show(ui, |ui| {
                let viewport_width = ui.clip_rect().width().min(ui.available_width());
                ui.set_width(viewport_width);
                ui.set_max_width(viewport_width);

                if self.library_tab == LibraryTab::Folders && self.library_current_folder.is_none() {
                    self.draw_folders_list_view(ui);
                    return;
                }

                if self.library_tab == LibraryTab::Folders && self.library_current_folder.is_some() && self.folder_import_select_mode.is_none() {
                    let folder_id = self.library_current_folder.unwrap();
                    let folder_name = self.folders.iter()
                        .find(|f| f.id == folder_id)
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| "Folder".to_owned());
                    
                    ui.horizontal(|ui| {
                        let back_btn = ui.add(
                            Button::new(Self::icon(0xe5c4, 16.0, Self::strong_text_color()))
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(12.0)
                        );
                        if back_btn.clicked() {
                            self.library_current_folder = None;
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(folder_name)
                                .font(FontId::new(16.0, FontFamily::Proportional))
                                .color(Self::strong_text_color())
                                .strong()
                        );
                        ui.add_space(12.0);
                        let import_btn = ui.add(
                            Button::new(format!("+ {}", self.t("library.import_sound_to_folder")))
                                .fill(Color32::from_rgb(227, 82, 149))
                                .corner_radius(10.0)
                        );
                        Self::decorate_button_response(ui, &import_btn);
                        if import_btn.clicked() {
                            self.folder_import_select_mode = Some(folder_id);
                        }
                    });
                    ui.add_space(12.0);
                }
                ui.set_max_width(viewport_width);

                if self.library_tab == LibraryTab::Videos {
                    self.draw_video_library_grid(ui);
                    return;
                }

                if self.sounds.is_empty() {
                    self.draw_empty_editor(ui);
                    return;
                }

                let spacing = 14.0;
                let layout_width = ui.clip_rect().width().min(ui.available_width());
                let columns = self
                    .library_grid_columns
                    .clamp(LIBRARY_GRID_MIN_COLUMNS, LIBRARY_GRID_MAX_COLUMNS);
                let sounds = self.filtered_library_sounds();
                if sounds.is_empty() {
                    self.draw_empty_editor(ui);
                    return;
                }
                let mut open_sound = None;
                let mut preview_sound = None;
                let mut copy_sound = None;
                let mut drag_sound = None;
                let mut favorite_sound = None;

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

                        ui.scope_builder(egui::UiBuilder::new().max_rect(tile_rect), |ui| {
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
                                    let action_button_size =
                                        [action_button_width, action_button_height];
                                    let action_icon_size = if action_button_width < 34.0 {
                                        16.0
                                    } else {
                                        18.0
                                    };
                                    ui.set_min_size(vec2(inner_size, inner_size));
                                    ui.set_width(inner_size);
                                    ui.vertical(|ui| {
                                        if !ultra_compact_card {
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
                                        }
                                        let bucket_count =
                                            (card_size * 0.34).round().clamp(20.0, 52.0) as usize;
                                        let waveform_samples = self.sound_waveform_samples(&sound);
                                        let waveform_preview = Self::library_sound_waveform_preview_from_samples(
                                            &sound,
                                            &waveform_samples,
                                            bucket_count,
                                        );
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
                                                Color32::from_rgba_premultiplied(255, 255, 255, 22)
                                            } else {
                                                Self::panel_fill()
                                            },
                                            (card_size * 0.38).clamp(18.0, 78.0),
                                        );
                                        if !compact_card {
                                            ui.add_space(9.0);
                                            ui.label(
                                                RichText::new(format_time(sound.trimmed_length()))
                                                    .size(11.5)
                                                    .color(meta_color),
                                            );
                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                if self.folder_import_select_mode.is_some() {
                                                    let center_gap = (inner_size - action_button_width) * 0.5;
                                                    if center_gap > 0.0 {
                                                        ui.add_space(center_gap);
                                                    }
                                                    let play_btn = Self::icon_action(
                                                        ui,
                                                        action_button_size,
                                                        0xe037,
                                                        false,
                                                        false,
                                                    );
                                                    Self::decorate_button_response(ui, &play_btn);
                                                    if play_btn.clicked() {
                                                        preview_sound = Some(sound.id);
                                                    }
                                                } else {
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
                                                    if Self::icon_action(
                                                        ui,
                                                        action_button_size,
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
                                                        action_button_size,
                                                        0xe14d,
                                                        self.sound_copy_feedback_active(
                                                            ui.ctx(),
                                                            sound.id,
                                                        ),
                                                        self.sound_copy_feedback_active(
                                                            ui.ctx(),
                                                            sound.id,
                                                        ),
                                                    )
                                                    .clicked()
                                                    {
                                                        copy_sound = Some(sound.id);
                                                    }
                                                }
                                            });
                                            if self.sound_copy_feedback_active(ui.ctx(), sound.id) {
                                                ui.add_space(6.0);
                                                ui.label(
                                                    RichText::new("Copied")
                                                        .size(11.0)
                                                        .color(meta_color),
                                                );
                                            }
                                        }
                                    });
                                });
                        });

                        if body_clicked {
                            let action_clicked = preview_sound == Some(sound.id)
                                || favorite_sound == Some(sound.id)
                                || copy_sound == Some(sound.id);
                            if !action_clicked {
                                if let Some(import_folder_id) = self.folder_import_select_mode {
                                    if let Some(s) = self.sounds.iter_mut().find(|s| s.id == sound.id) {
                                        s.folder_id = Some(import_folder_id);
                                        self.mark_dirty(ui.ctx());
                                    }
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
            });
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
            self.record_overlay_pos = Some(self.clamp_overlay_pos(
                ctx,
                overlay_size,
                state.left_top_pos(),
            ));
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
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let visible_sounds = self.filtered_library_sounds();
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
                                ui.style_mut().interaction.selectable_labels = false;
                                ui.label(
                                    RichText::new(&sound.name)
                                        .size(16.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                ui.add_space(8.0);
                                let waveform_samples = self.sound_waveform_samples(sound);
                                let waveform_preview =
                                    Self::trimmed_waveform_preview_from_samples(
                                        sound,
                                        &waveform_samples,
                                    );
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
                        let scrollbar_gutter = 18.0;
                        let interactive_rect = Rect::from_min_max(
                            frame.response.rect.min,
                            Pos2::new(
                                (frame.response.rect.max.x - scrollbar_gutter)
                                    .max(frame.response.rect.min.x),
                                frame.response.rect.max.y,
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
            self.draw_empty_editor(ui);
            return;
        };

        let (sound_id, vocal_job_running, vocal_ready, music_job_running, music_ready) = {
            let sound = &self.sounds[index];
            let vocal_asset_path = sound.vocal_asset_path(self.storage.root_dir());
            let vocal_ready = vocal_asset_path
                .as_ref()
                .is_some_and(|path| path.exists());
            let music_asset_path = sound.music_asset_path(self.storage.root_dir());
            let music_ready = music_asset_path
                .as_ref()
                .is_some_and(|path| path.exists());
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

        let mut preview_toggle = false;
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
        let folder_label = self.t("editor.folder");
        let no_folder_label = self.t("editor.no_folder");
        let app_folders = self.folders.clone();

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
                let storage_root = self.storage.root_dir().to_path_buf();
                let sound = &mut self.sounds[index];
                let editor_asset_path = if sound.needs_processed_export()
                    && Storage::processed_export_exists(&storage_root, sound)
                {
                    Storage::processed_export_path(&storage_root, sound)
                } else {
                    sound.asset_path(&storage_root)
                };
                let editor_audio_loading = self.audio_preload_inflight.contains(&editor_asset_path)
                    || self
                        .pending_preview_after_preload
                        .is_some_and(|(pending_sound_id, _)| pending_sound_id == sound.id);
                let controls_width = 52.0 + 52.0 + 52.0 + 64.0 + 64.0 + 36.0;
                let row_gap = 8.0;
                let name_width = (ui.available_width() - controls_width - row_gap).max(120.0);

                ui.horizontal(|ui| {
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

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&folder_label)
                            .size(11.5)
                            .color(Self::muted_text_color())
                            .strong(),
                    );
                    ui.add_space(8.0);
                    let mut current_folder_name = no_folder_label.clone();
                    let mut selected_id = sound.folder_id;
                    if let Some(folder_id) = selected_id {
                        if let Some(folder) = app_folders.iter().find(|f| f.id == folder_id) {
                            current_folder_name = folder.name.clone();
                        }
                    }

                    let combo_response = egui::ComboBox::from_id_salt(sound.id)
                        .selected_text(current_folder_name)
                        .show_ui(ui, |ui| {
                            let mut choice_changed = false;
                            if ui.selectable_value(&mut selected_id, None, &no_folder_label).clicked() {
                                choice_changed = true;
                            }
                            for folder in &app_folders {
                                if ui.selectable_value(&mut selected_id, Some(folder.id), &folder.name).clicked() {
                                    choice_changed = true;
                                }
                            }
                            choice_changed
                        });

                    if let Some(choice_changed) = combo_response.inner {
                        if choice_changed {
                            sound.folder_id = selected_id;
                            changed = true;
                        }
                    }
                });

                ui.add_space(16.0);

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
                                    .on_hover_text("Automatically adjust volume to a standard listening level")
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
                        requested_scroll_offset = Some((current_offset + delta).clamp(0.0, max_offset));
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

    fn cached_stem_waveform(
        &self,
        cache: &RefCell<HashMap<Uuid, Vec<f32>>>,
        sound_id: Uuid,
        path: &Path,
    ) -> Vec<f32> {
        if let Some(existing) = cache.borrow().get(&sound_id).cloned() {
            return existing;
        }

        let Ok(waveform) = self.storage.analyze_waveform_preview(path, 96) else {
            return Vec::new();
        };

        cache.borrow_mut().insert(sound_id, waveform.clone());
        waveform
    }

    fn sound_waveform_samples(&self, sound: &SoundEffect) -> Vec<f32> {
        if sound.music_only
            && let Some(path) = sound.music_asset_path(self.storage.root_dir())
            && path.exists()
        {
            return self.cached_stem_waveform(
                &self.music_waveform_cache,
                sound.id,
                &path,
            );
        }

        if sound.vocal_only
            && let Some(path) = sound.vocal_asset_path(self.storage.root_dir())
            && path.exists()
        {
            return self.cached_stem_waveform(
                &self.vocal_waveform_cache,
                sound.id,
                &path,
            );
        }

        sound.waveform.clone()
    }

    fn recording_waveform_samples(&self, draft: &RecordingDraft) -> Vec<f32> {
        if draft.keep_music
            && let Some(path) = draft.music_separated_path.as_ref()
            && path.exists()
        {
            return self.cached_stem_waveform(
                &self.music_waveform_cache,
                draft.sound.id,
                path,
            );
        }

        if draft.keep_vocal
            && let Some(path) = draft.vocal_separated_path.as_ref()
            && path.exists()
        {
            return self.cached_stem_waveform(
                &self.vocal_waveform_cache,
                draft.sound.id,
                path,
            );
        }

        draft.sound.waveform.clone()
    }

    fn trimmed_waveform_preview_from_samples(sound: &SoundEffect, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration();
        let trim_start = sound.trim_start_secs.clamp(0.0, total_duration);
        let trim_end = sound
            .trim_end_secs
            .clamp(trim_start + 0.001, total_duration);
        let source_len = samples.len();
        if source_len <= 1 || (trim_start <= 0.001 && trim_end >= total_duration - 0.001) {
            return samples.to_vec();
        }

        let start_index = ((trim_start / total_duration) * source_len as f32).floor() as usize;
        let mut end_index = ((trim_end / total_duration) * source_len as f32).ceil() as usize;
        end_index = end_index.clamp(start_index.saturating_add(1), source_len);
        let segment = &samples[start_index.min(source_len - 1)..end_index];
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

    fn library_sound_waveform_preview_from_samples(
        sound: &SoundEffect,
        samples: &[f32],
        buckets: usize,
    ) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration();
        let trim_start = sound.trim_start_secs.clamp(0.0, total_duration);
        let trim_end = sound
            .trim_end_secs
            .clamp(trim_start + 0.001, total_duration);
        let source_len = samples.len();
        let start_index = ((trim_start / total_duration) * source_len as f32).floor() as usize;
        let mut end_index = ((trim_end / total_duration) * source_len as f32).ceil() as usize;
        end_index = end_index.clamp(start_index.saturating_add(1), source_len);
        let segment = &samples[start_index.min(source_len.saturating_sub(1))..end_index];
        Self::compact_library_waveform(segment, buckets)
    }

    fn compact_library_waveform(samples: &[f32], buckets: usize) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let bucket_count = buckets.clamp(12, 64).min(samples.len().max(1));
        let mut preview = Vec::with_capacity(bucket_count);

        for bucket_index in 0..bucket_count {
            let start = ((bucket_index as f32 / bucket_count as f32) * samples.len() as f32).floor()
                as usize;
            let mut end = ((((bucket_index + 1) as f32) / bucket_count as f32)
                * samples.len() as f32)
                .ceil() as usize;
            let start = start.min(samples.len().saturating_sub(1));
            end = end.clamp(start + 1, samples.len());

            let slice = &samples[start..end];
            let mut peak = 0.0_f32;
            let mut energy = 0.0_f32;
            for sample in slice {
                peak = peak.max(*sample);
                energy += sample * sample;
            }
            let rms = (energy / slice.len() as f32).sqrt();
            preview.push((peak * 0.62 + rms * 0.38).powf(1.12));
        }

        let max_level = preview
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(f32::EPSILON);
        for value in &mut preview {
            *value = (*value / max_level).clamp(0.0, 1.0);
            if *value < 0.06 {
                *value *= 0.5;
            }
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

    fn draw_stream_wave_strip(ui: &mut Ui, waveform: &[f32], active: bool) {
        let desired = vec2(ui.available_width().max(220.0), 46.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        let dark_theme = Self::dark_theme_enabled();
        painter.rect_filled(
            rect,
            18.0,
            if dark_theme {
                Color32::from_rgba_premultiplied(255, 255, 255, 14)
            } else {
                Color32::from_rgba_premultiplied(255, 255, 255, 96)
            },
        );
        painter.rect_stroke(
            rect,
            18.0,
            Stroke::new(
                1.0,
                if dark_theme {
                    Color32::from_rgba_premultiplied(111, 86, 120, 150)
                } else {
                    Color32::from_rgba_premultiplied(231, 214, 224, 180)
                },
            ),
            StrokeKind::Outside,
        );

        let data = if waveform.is_empty() {
            vec![0.05; 40]
        } else {
            waveform.to_vec()
        };
        let inner = rect.shrink2(vec2(12.0, 8.0));
        let bar_width = inner.width() / data.len().max(1) as f32;
        let active_color = if active {
            Color32::from_rgb(227, 82, 149)
        } else {
            Color32::from_rgba_premultiplied(184, 132, 164, 120)
        };
        for (index, value) in data.iter().enumerate() {
            let x = inner.left() + (index as f32 + 0.5) * bar_width;
            let amplitude = Self::boost_stream_meter_level(*value);
            let half = amplitude * inner.height() * 0.44;
            let bar = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.22, inner.center().y - half),
                Pos2::new(x + bar_width * 0.22, inner.center().y + half),
            );
            painter.rect_filled(bar, 3.0, active_color);
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
        let wave_width = (inner.width() * 0.82).clamp(inner.width().min(56.0), inner.width());
        let wave_left = inner.center().x - wave_width * 0.5;
        let bar_width = wave_width / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = Self::wave_strip_level(*level);
            let center_x = wave_left + (index as f32 + 0.5) * bar_width;
            let half = amplitude * inner.height() * 0.34;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.2).max(0.8),
                    inner.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.2).max(0.8),
                    inner.center().y + half,
                ),
            );
            painter.rect_filled(wave_rect, 2.0, idle_color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(wave_left..=wave_left + wave_width, progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, inner.top()),
                    Pos2::new(play_x, inner.bottom()),
                ],
                Stroke::new(2.0, active_color),
            );
        }
    }

    fn boost_stream_meter_level(level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        (level * 6.0).sqrt().clamp(0.12, 1.0)
    }

    fn wave_strip_level(level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        let shaped = (level * 1.28).powf(1.16).clamp(0.0, 1.0);
        if shaped < 0.05 { shaped * 0.55 } else { shaped }
    }

    fn render_tts_download_tab(&mut self, ui: &mut Ui, ctx: &Context) {
        let mut generate_request = false;
        let mut preview_request = false;
        let mut add_to_library = false;
        let mut clear_result = false;
        let mut save_gemini = false;
        let mut save_preset = false;
        let mut delete_preset = false;
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
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("download.voice"))
                            .size(12.0)
                            .color(Self::muted_text_color()),
                    );
                    Self::with_dark_combo_visuals(ui, |ui| {
                        ComboBox::from_id_salt("gemini-tts-voice")
                            .width(170.0)
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
                                    }
                                }
                            });
                    });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(self.t("download.name"))
                            .size(12.0)
                            .color(Self::muted_text_color()),
                    );
                    ui.add_sized(
                        [ui.available_width().max(120.0), 30.0],
                        TextEdit::singleline(&mut self.tts_output_name)
                            .hint_text("gemini tts")
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("download.prompt_preset"))
                            .size(12.0)
                            .color(Self::muted_text_color()),
                    );
                    Self::with_dark_combo_visuals(ui, |ui| {
                        ComboBox::from_id_salt("gemini-tts-preset")
                            .width(156.0)
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
                                    }
                                }
                            });
                    });
                    ui.add_space(8.0);
                    let preset_name_hint = self.t("download.preset_name");
                    ui.add_sized(
                        [ui.available_width() - 82.0, 30.0],
                        TextEdit::singleline(&mut self.tts_preset_name)
                            .hint_text(preset_name_hint),
                    );
                    let save = ui.add_sized(
                        [30.0, 30.0],
                        Self::action_button(RichText::new("+").size(16.0), false, false),
                    );
                    Self::decorate_button_response(ui, &save);
                    if save.clicked() {
                        save_preset = true;
                    }
                    let delete = ui.add_enabled(
                        self.selected_tts_preset_name().is_some(),
                        Self::action_button(RichText::new("×").size(16.0), false, false),
                    );
                    Self::decorate_button_response(ui, &delete);
                    if delete.clicked() {
                        delete_preset = true;
                    }
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
                if false {
                    ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (label, prompt) in [
                        (
                            "VN Bắc",
                            "Accent: Northern Vietnamese from Hanoi. Style: clear, natural, warm. Pacing: conversational and steady.",
                        ),
                        (
                            "VN Trung",
                            "Accent: Central Vietnamese from Hue. Style: gentle and clear. Pacing: natural and calm.",
                        ),
                        (
                            "VN Nam",
                            "Accent: Southern Vietnamese from Ho Chi Minh City. Style: friendly and relaxed. Pacing: natural conversational pace.",
                        ),
                        (
                            "Indian EN",
                            "Accent: Indian English. Style: confident and clear. Pacing: natural professional delivery.",
                        ),
                        (
                            "US EN",
                            "Accent: American English. Style: natural and energetic. Pacing: medium and clear.",
                        ),
                        (
                            "UK EN",
                            "Accent: British English from London. Style: polished and clear. Pacing: medium.",
                        ),
                    ] {
                        let response = ui.add_sized(
                            [76.0, 28.0],
                            Self::action_button(
                                RichText::new(label).size(11.5),
                                false,
                                false,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            self.tts_direction_prompt = prompt.to_owned();
                        }
                    }
                    });
                }
                ui.add_space(0.0);
                let direction_hint = self.t("download.direction_hint");
                ui.add_sized(
                    [ui.available_width(), 96.0],
                    TextEdit::multiline(&mut self.tts_direction_prompt)
                        .desired_width(f32::INFINITY)
                        .hint_text(direction_hint),
                );
                ui.add_space(10.0);
                let enter_text_hint = self.t("download.enter_text");
                ui.add_sized(
                    [ui.available_width(), 130.0],
                    TextEdit::multiline(&mut self.tts_text)
                        .desired_width(f32::INFINITY)
                        .hint_text(enter_text_hint),
                );
            });

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

        if save_preset {
            self.save_current_tts_preset();
        }
        if save_gemini {
            let _ = self.storage.save_gemini_api_key(&self.gemini_api_key);
        }
        if delete_preset {
            self.delete_selected_tts_preset();
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
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe2c4, 20.0, Self::strong_text_color()).strong());
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 34.0),
                        egui::Layout::right_to_left(Align::Center),
                        |ui| {
                            if Self::icon_titlebar(ui, [34.0, 34.0], 0xe5cd, false, true)
                                .clicked()
                            {
                                clear_result = !snapshot.running;
                                close_request = true;
                            }
                            if Self::icon_titlebar(ui, [34.0, 34.0], 0xe15b, false, false)
                                .clicked()
                            {
                                minimize_request = true;
                            }
                        },
                    );
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

    fn transition_progress(&mut self, ctx: &Context) -> Option<(TransitionPhase, f32)> {
        let phase = self.startup.phase;
        if phase == TransitionPhase::Live {
            return None;
        }

        let now = ctx.input(|input| input.time);
        let started_at = self.startup.started_at.get_or_insert(now);
        let progress =
            ((now - *started_at) / self.startup.duration_sec as f64).clamp(0.0, 1.0) as f32;

        if phase == TransitionPhase::Outro {
            self.update_outro_audio_fade(progress);
        }

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
                        if let Some(audio) = self.audio.as_mut() {
                            audio.set_volume(0.0);
                            audio.stop();
                        }
                        self.finalize_close_cleanup(ctx);
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
                let blob_only_transition = false;
                let transparent_transition_backdrop = true;
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
                let square_morph = match phase {
                    TransitionPhase::Intro => {
                        let square_seed = ((t - 0.08) / 0.66).clamp(0.0, 1.0);
                        square_seed * square_seed * (3.0 - 2.0 * square_seed)
                    }
                    TransitionPhase::Outro => {
                        let release = 1.0 - (1.0 - (progress / 0.82).clamp(0.0, 1.0)).powi(3);
                        (1.0 - release).clamp(0.0, 1.0)
                    }
                    TransitionPhase::Live => 1.0,
                };
                let ornament_alpha = if phase == TransitionPhase::Intro && progress >= 0.50 {
                    0.0
                } else {
                    ornament_alpha * (1.0 - square_morph).powf(1.7)
                };
                let preview_content_alpha =
                    Self::ease_in_out_cubic(((square_morph - 0.16) / 0.72).clamp(0.0, 1.0));
                let content_alpha = if blob_only_transition {
                    if intro_light_fade {
                        1.0
                    } else {
                        ornament_alpha
                    }
                } else {
                    preview_content_alpha
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

                if blob_only_transition {
                    let pulse = 1.0 + audio_level * 0.14 + (time * 2.2).sin() * 0.03;
                    let phase_scale = match phase {
                        TransitionPhase::Intro => egui::lerp(0.84..=1.02, t),
                        TransitionPhase::Outro => egui::lerp(1.0..=0.86, progress),
                        TransitionPhase::Live => 1.0,
                    };
                    let blob_alpha = match phase {
                        TransitionPhase::Intro => egui::lerp(1.0..=0.9, t),
                        TransitionPhase::Outro => egui::lerp(1.0..=0.0, progress),
                        TransitionPhase::Live => 1.0,
                    };
                    let outer_points = Self::squircle_points(
                        center,
                        base * 0.34 * phase_scale * pulse,
                        base * 0.28 * phase_scale * pulse,
                        2.9,
                        0.12 + audio_level * 0.08,
                        time,
                    );
                    let middle_points = Self::squircle_points(
                        Pos2::new(center.x, center.y + 4.0),
                        base * 0.27 * phase_scale,
                        base * 0.22 * phase_scale,
                        3.2,
                        0.08 + audio_level * 0.06,
                        time + 0.45,
                    );
                    let core_points = Self::squircle_points(
                        center,
                        base * 0.16 * phase_scale,
                        base * 0.13 * phase_scale,
                        3.8,
                        0.05,
                        time + 0.2,
                    );
                    let outer_fill = if self.dark_theme {
                        Color32::from_rgba_premultiplied(242, 170, 207, 226)
                    } else {
                        Color32::from_rgba_premultiplied(246, 208, 230, 236)
                    };
                    let middle_fill = if self.dark_theme {
                        Color32::from_rgba_premultiplied(255, 236, 246, 176)
                    } else {
                        Color32::from_rgba_premultiplied(255, 245, 250, 200)
                    };
                    let core_fill = if self.dark_theme {
                        Color32::from_rgb(28, 18, 30)
                    } else {
                        Color32::from_rgb(50, 29, 45)
                    };
                    painter.add(egui::Shape::convex_polygon(
                        outer_points,
                        Self::with_alpha(outer_fill, blob_alpha),
                        Stroke::new(
                            1.3,
                            Self::with_alpha(Color32::from_rgb(239, 124, 190), blob_alpha),
                        ),
                    ));
                    painter.add(egui::Shape::convex_polygon(
                        middle_points,
                        Self::with_alpha(middle_fill, blob_alpha * 0.92),
                        Stroke::NONE,
                    ));
                    painter.add(egui::Shape::convex_polygon(
                        core_points,
                        Self::with_alpha(core_fill, blob_alpha),
                        Stroke::NONE,
                    ));

                    let bar_rect = Rect::from_center_size(
                        center,
                        vec2(base * 0.18 * phase_scale, base * 0.075 * phase_scale),
                    );
                    let clip = painter.with_clip_rect(bar_rect.expand2(vec2(8.0, 8.0)));
                    let bar_width = bar_rect.width() / wave_bars.len().max(1) as f32;
                    for (index, bar) in wave_bars.iter().enumerate() {
                        let x = bar_rect.left() + (index as f32 + 0.5) * bar_width;
                        let half = bar_rect.height() * (0.12 + *bar * 0.42);
                        let wave_rect = Rect::from_min_max(
                            Pos2::new(x - bar_width * 0.18, bar_rect.center().y - half),
                            Pos2::new(x + bar_width * 0.18, bar_rect.center().y + half),
                        );
                        clip.rect_filled(
                            wave_rect,
                            3.0,
                            Self::with_alpha(Color32::from_rgb(255, 231, 242), blob_alpha * 0.98),
                        );
                    }

                    for index in 0..6 {
                        let angle = time * 0.7 + index as f32 * 1.05;
                        let orbit = base * 0.18 + (index % 3) as f32 * 10.0;
                        let note_pos = Pos2::new(
                            center.x + angle.cos() * orbit,
                            center.y + angle.sin() * orbit * 0.8,
                        );
                        Self::paint_glowing_music_note(
                            &painter,
                            note_pos,
                            0.8 + ((index % 2) as f32 * 0.12),
                            (index as f32 * 0.17).sin() * 0.18,
                            if index % 2 == 0 {
                                Self::with_alpha(note_base, blob_alpha)
                            } else {
                                Self::with_alpha(note_alt, blob_alpha)
                            },
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_glow_rgb.0,
                                    note_glow_rgb.1,
                                    note_glow_rgb.2,
                                    72,
                                ),
                                blob_alpha,
                            ),
                        );
                    }
                    return;
                }

                if !blob_only_transition && !transparent_transition_backdrop && self.dark_theme {
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
                } else if !blob_only_transition
                    && !transparent_transition_backdrop
                    && phase != TransitionPhase::Live
                {
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

                if !blob_only_transition && !transparent_transition_backdrop {
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
                                    (26.0 * star_alpha_scale * twinkle * (0.35 + aura * 0.65))
                                        as u8,
                                ),
                                layer_alpha * ornament_alpha,
                            ),
                        );
                    }
                }

                if !blob_only_transition {
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
                }

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

                if !blob_only_transition {
                    let inner_rect = Rect::from_center_size(
                        center,
                        vec2(half_w * 1.08, half_h * 0.9).min(target_rect.size() * 0.78),
                    );
                    let module_rect = Rect::from_center_size(
                        Pos2::new(center.x, center.y + half_h * 0.1),
                        vec2(inner_rect.width() * 0.82, inner_rect.height() * 0.7),
                    );
                    let module_inner = module_rect.shrink2(vec2(24.0, 18.0));
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

    

    

    

    

    

    }

mod editor;
mod library;
mod settings;
mod downloader;
mod pitch_monitor;


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
        self.poll_library_hydration_jobs(ctx);
        self.poll_transition_analysis_jobs(ctx);
        self.poll_myinstants_waveform_jobs();
        self.poll_processed_export_jobs(ctx);
        self.poll_trim_commit_jobs(ctx);
        self.poll_audio_preload_jobs(ctx);
        self.poll_normalize_jobs(ctx);
        self.poll_demucs_install_result(ctx);
        self.poll_demucs_model_result(ctx);
        self.poll_stream_driver_result(ctx);
        self.poll_stream_input_router(ctx);
        self.poll_vocal_separation_jobs(ctx);
        self.poll_tts_jobs(ctx);
        self.prune_copy_feedback(ctx);
        if !ctx.input(|input| input.pointer.primary_down()) {
            self.pending_sound_drag = None;
        }

        self.play_startup_sound_if_needed(ctx);

        let transition = self.transition_progress(ctx);
        let download_snapshot = self.downloader.snapshot();
        let wants_shadow = false;
        if self.native_shadow_applied != wants_shadow {
            platform::set_native_window_shadow(frame, wants_shadow);
            self.native_shadow_applied = wants_shadow;
        }
        let wants_transition_topmost = self.is_transition_active() || self.overlay_only_mode;
        if self.transition_window_topmost_applied != wants_transition_topmost {
            platform::set_native_window_topmost(frame, wants_transition_topmost);
            self.transition_window_topmost_applied = wants_transition_topmost;
        }

        self.enforce_square_window_if_needed(ctx);
        self.preload_selected_sound_audio();
        self.handle_space_preview(ctx);
        self.handle_trim_start_preview(ctx);
        self.handle_record_hotkey(ctx);

        if let Some(audio) = self.audio.as_mut() {
            audio.tick();
            if self.myinstants_preview_audio_url.is_some() && !audio.has_active_playback() {
                self.myinstants_preview_audio_url = None;
            }
        }
        if !self.normalize_inflight.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if !self.trim_commit_inflight.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
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
        if self.tts_running || self.vocal_separation_running {
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
        if self.record_overlay_pending_visible {
            ctx.request_repaint_after(Duration::from_millis(50));
            if self.recorder.snapshot().running {
                self.record_overlay_pending_visible = false;
            }
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

        self.render_modal_backdrop(ctx);
        self.render_download_panel(ctx);
        self.render_myinstants_panel(ctx);
        self.render_import_panel(ctx);
        self.render_record_panel(ctx);
        self.render_record_review_panel(ctx);
        self.render_stream_panel(ctx);
        self.render_settings_panel(ctx);
        self.render_video_viewer_panel(ctx);
        self.render_pitch_monitor(ctx);
        self.render_trim_commit_panel(ctx);
        self.render_pitch_overlay_viewport(ctx);
        self.render_custom_window_resize_handles(ctx);
        self.maybe_start_pending_processed_export();

        let is_folder_open = self.app_view == AppView::Library
            && self.library_tab == LibraryTab::Folders
            && self.library_current_folder.is_some();

        let external_file_hover = (self.app_view == AppView::Editor || is_folder_open)
            && !self.has_modal_panel()
            && ctx.input(|input| !input.raw.hovered_files.is_empty());
        if external_file_hover {
            ctx.request_repaint_after(Duration::from_millis(16));
            let pointer_over_drop = if is_folder_open {
                true
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
