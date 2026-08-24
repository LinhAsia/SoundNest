use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppView {
    Editor,
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibraryTab {
    Sounds,
    Videos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibrarySoundView {
    Rows,
    Grid,
}

impl LibrarySoundView {
    pub(crate) fn from_preference(value: Option<&str>) -> Self {
        match value.map(|value| value.trim().to_ascii_lowercase()) {
            Some(value) if value == "grid" => Self::Grid,
            _ => Self::Rows,
        }
    }

    pub(crate) fn preference_value(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Grid => "grid",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibraryFolderView {
    Rows,
    Grid,
}

impl LibraryFolderView {
    pub(crate) fn from_preference(value: Option<&str>) -> Self {
        match value.map(|value| value.trim().to_ascii_lowercase()) {
            Some(value) if value == "grid" => Self::Grid,
            _ => Self::Rows,
        }
    }

    pub(crate) fn preference_value(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Grid => "grid",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadPanelTab {
    Download,
    Tts,
}

pub(super) struct GeminiVoiceOption {
    pub(super) name: &'static str,
    pub(super) label: &'static str,
}

pub(super) const GEMINI_VOICE_OPTIONS: &[GeminiVoiceOption] = &[
    GeminiVoiceOption {
        name: "Kore",
        label: "Kore - Female",
    },
    GeminiVoiceOption {
        name: "Puck",
        label: "Puck - Male",
    },
    GeminiVoiceOption {
        name: "Charon",
        label: "Charon - Male",
    },
    GeminiVoiceOption {
        name: "Aoede",
        label: "Aoede - Female",
    },
    GeminiVoiceOption {
        name: "Fenrir",
        label: "Fenrir - Male",
    },
    GeminiVoiceOption {
        name: "Leda",
        label: "Leda - Female",
    },
    GeminiVoiceOption {
        name: "Orus",
        label: "Orus - Male",
    },
    GeminiVoiceOption {
        name: "Zephyr",
        label: "Zephyr - Female",
    },
];

#[derive(Clone, Copy)]
pub(super) struct DownloadSiteBadge {
    pub(super) name: &'static str,
    pub(super) kind: DownloadSiteKind,
    pub(super) color: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DownloadSiteKind {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrimSnapshot {
    pub(crate) sound_id: Uuid,
    pub(crate) trim_start_secs: f32,
    pub(crate) trim_end_secs: f32,
    pub(crate) cut_start_secs: Option<f32>,
    pub(crate) cut_end_secs: Option<f32>,
    pub(crate) display_trim_start_secs: Option<f32>,
    pub(crate) display_trim_end_secs: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineClipAudioSettings {
    pub(crate) volume: f32,
    pub(crate) speed: f32,
    pub(crate) reverb_enabled: bool,
    pub(crate) telephone_enabled: bool,
    pub(crate) distortion_enabled: bool,
    pub(crate) echo_enabled: bool,
    pub(crate) underwater_enabled: bool,
    pub(crate) robot_enabled: bool,
    pub(crate) eight_d_enabled: bool,
    pub(crate) pitch_shift_enabled: bool,
    pub(crate) pitch_shift_semitones: f32,
    pub(crate) vocal_only: bool,
    pub(crate) music_only: bool,
}

impl Default for TimelineClipAudioSettings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            speed: 1.0,
            reverb_enabled: false,
            telephone_enabled: false,
            distortion_enabled: false,
            echo_enabled: false,
            underwater_enabled: false,
            robot_enabled: false,
            eight_d_enabled: false,
            pitch_shift_enabled: false,
            pitch_shift_semitones: 0.0,
            vocal_only: false,
            music_only: false,
        }
    }
}

impl TimelineClipAudioSettings {
    pub(crate) fn from_sound(sound: &SoundEffect) -> Self {
        Self {
            volume: sound.volume,
            speed: sound.speed,
            reverb_enabled: sound.reverb_enabled,
            telephone_enabled: sound.telephone_enabled,
            distortion_enabled: sound.distortion_enabled,
            echo_enabled: sound.echo_enabled,
            underwater_enabled: sound.underwater_enabled,
            robot_enabled: sound.robot_enabled,
            eight_d_enabled: sound.eight_d_enabled,
            pitch_shift_enabled: sound.pitch_shift_enabled,
            pitch_shift_semitones: sound.pitch_shift_semitones,
            vocal_only: sound.vocal_only,
            music_only: sound.music_only,
        }
    }

    pub(crate) fn apply_changed_from(&mut self, before: &Self, after: &Self) {
        if after.volume != before.volume { self.volume = after.volume; }
        if after.speed != before.speed { self.speed = after.speed; }
        if after.reverb_enabled != before.reverb_enabled { self.reverb_enabled = after.reverb_enabled; }
        if after.telephone_enabled != before.telephone_enabled { self.telephone_enabled = after.telephone_enabled; }
        if after.distortion_enabled != before.distortion_enabled { self.distortion_enabled = after.distortion_enabled; }
        if after.echo_enabled != before.echo_enabled { self.echo_enabled = after.echo_enabled; }
        if after.underwater_enabled != before.underwater_enabled { self.underwater_enabled = after.underwater_enabled; }
        if after.robot_enabled != before.robot_enabled { self.robot_enabled = after.robot_enabled; }
        if after.eight_d_enabled != before.eight_d_enabled { self.eight_d_enabled = after.eight_d_enabled; }
        if after.pitch_shift_enabled != before.pitch_shift_enabled { self.pitch_shift_enabled = after.pitch_shift_enabled; }
        if after.vocal_only != before.vocal_only { self.vocal_only = after.vocal_only; }
        if after.music_only != before.music_only { self.music_only = after.music_only; }
    }
}

#[cfg(test)]
mod timeline_audio_tests {
    use super::*;

    #[test]
    fn batch_edit_preserves_unmodified_clip_settings() {
        let before = TimelineClipAudioSettings::default();
        let mut after = before.clone();
        after.reverb_enabled = true;
        let mut target = TimelineClipAudioSettings { volume: 0.35, speed: 1.5, ..Default::default() };
        target.apply_changed_from(&before, &after);
        assert!(target.reverb_enabled);
        assert_eq!(target.volume, 0.35);
        assert_eq!(target.speed, 1.5);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrimTimelineClip {
    pub(crate) id: Uuid,
    pub(crate) source_sound_id: Uuid,
    pub(crate) start_secs: f32,
    pub(crate) clip_start_secs: f32,
    pub(crate) clip_end_secs: f32,
    pub(crate) audio: TimelineClipAudioSettings,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrimTimelineRow {
    pub(crate) clips: Vec<TrimTimelineClip>,
    pub(crate) muted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrimTimelineState {
    pub(crate) sound_id: Uuid,
    pub(crate) enabled: bool,
    pub(crate) playhead_secs: f32,
    pub(crate) snap_enabled: bool,
    pub(crate) selected_clip_id: Option<Uuid>,
    pub(crate) selected_clip_ids: HashSet<Uuid>,
    pub(crate) rows: Vec<TrimTimelineRow>,
    pub(crate) timeline_is_playing: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TrimTimelineDropTarget {
    pub(crate) row_index: usize,
    pub(crate) start_secs: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct TrimTimelineClipboardClip {
    pub(crate) source_sound_id: Uuid,
    pub(crate) clip_start_secs: f32,
    pub(crate) clip_end_secs: f32,
    pub(crate) audio: TimelineClipAudioSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct TrimTimelineSegmentDeleteAnimation {
    pub(crate) owner_sound_id: Uuid,
    pub(crate) row_index: usize,
    pub(crate) source_sound_id: Uuid,
    pub(crate) start_secs: f32,
    pub(crate) clip_start_secs: f32,
    pub(crate) clip_end_secs: f32,
    pub(crate) started_at: Instant,
}

impl TrimSnapshot {
    pub(crate) fn from_sound(sound: &SoundEffect) -> Self {
        Self {
            sound_id: sound.id,
            trim_start_secs: sound.trim_start_secs,
            trim_end_secs: sound.trim_end_secs,
            cut_start_secs: sound.cut_start_secs,
            cut_end_secs: sound.cut_end_secs,
            display_trim_start_secs: sound.display_trim_start_secs,
            display_trim_end_secs: sound.display_trim_end_secs,
        }
    }

    pub(crate) fn matches_sound(self, sound: &SoundEffect) -> bool {
        const EPSILON: f32 = 0.000_5;
        self.sound_id == sound.id
            && (self.trim_start_secs - sound.trim_start_secs).abs() <= EPSILON
            && (self.trim_end_secs - sound.trim_end_secs).abs() <= EPSILON
            && match (self.cut_start_secs, sound.cut_start_secs) {
                (Some(left), Some(right)) => (left - right).abs() <= EPSILON,
                (None, None) => true,
                _ => false,
            }
            && match (self.cut_end_secs, sound.cut_end_secs) {
                (Some(left), Some(right)) => (left - right).abs() <= EPSILON,
                (None, None) => true,
                _ => false,
            }
            && match (self.display_trim_start_secs, sound.display_trim_start_secs) {
                (Some(left), Some(right)) => (left - right).abs() <= EPSILON,
                (None, None) => true,
                _ => false,
            }
            && match (self.display_trim_end_secs, sound.display_trim_end_secs) {
                (Some(left), Some(right)) => (left - right).abs() <= EPSILON,
                (None, None) => true,
                _ => false,
            }
    }

    pub(crate) fn apply_to(self, sound: &mut SoundEffect) {
        if self.sound_id != sound.id {
            return;
        }
        sound.trim_start_secs = self.trim_start_secs;
        sound.trim_end_secs = self.trim_end_secs;
        sound.cut_start_secs = self.cut_start_secs;
        sound.cut_end_secs = self.cut_end_secs;
        sound.display_trim_start_secs = self.display_trim_start_secs;
        sound.display_trim_end_secs = self.display_trim_end_secs;
        sound.clamp_trim();
    }
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
    pub(super) download_site_icon_cache: RefCell<HashMap<DownloadSiteKind, TextureHandle>>,
    pub(super) vocal_waveform_cache: RefCell<HashMap<Uuid, Vec<f32>>>,
    pub(super) music_waveform_cache: RefCell<HashMap<Uuid, Vec<f32>>>,
    pub(super) library_waveform_preview_cache: RefCell<HashMap<String, Vec<f32>>>,
    pub(super) library_filtered_sound_indices_cache: RefCell<HashMap<String, Vec<usize>>>,
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
    pub(super) trim_commit_output_name: String,
    pub(super) show_delete_folder_confirm: Option<Uuid>,
    pub(super) trim_timeline_state: Option<TrimTimelineState>,
    pub(super) trim_timeline_drop_target: Option<TrimTimelineDropTarget>,
    pub(super) trim_timeline_preview_path: Option<PathBuf>,
    pub(super) trim_timeline_preview_start_secs: f32,
    pub(super) trim_timeline_preview_dirty: bool,
    pub(super) trim_timeline_clip_delete_animating: HashMap<Uuid, Instant>,
    pub(super) trim_timeline_segment_delete_animations: Vec<TrimTimelineSegmentDeleteAnimation>,
    pub(super) trim_timeline_copied_clip: Option<TrimTimelineClipboardClip>,
    pub(super) trim_timeline_scrub_resume_pending: bool,
    pub(super) import_dir: PathBuf,
    pub(super) import_audio_entries: Vec<PathBuf>,
    pub(super) app_view: AppView,
    pub(super) library_tab: LibraryTab,
    pub(super) folders: Vec<crate::storage::Folder>,
    pub(super) library_current_folder: Option<Uuid>,
    pub(super) library_collapsed_folders: HashSet<Uuid>,
    pub(super) editing_folder_id: Option<Uuid>,
    pub(super) folder_rename_name: String,
    pub(super) new_folder_name: String,
    pub(super) library_folder_create_open: bool,
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
    pub(super) library_row_thickness: usize,
    pub(super) library_sound_view: LibrarySoundView,
    pub(super) library_folder_view: LibraryFolderView,
    pub(super) video_assets: Vec<VideoAsset>,
    pub(super) recording_draft: Option<RecordingDraft>,
    pub(super) active_record_video_export: Option<RecordVideoExportState>,
    pub(super) video_viewer: Option<VideoViewerState>,
    pub(super) recording_review_pending_path: Option<PathBuf>,
    pub(super) recording_review_tx: Sender<RecordingReviewMessage>,
    pub(super) recording_review_rx: Receiver<RecordingReviewMessage>,
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
    pub(super) trim_timeline_view_start_secs: f32,
    pub(super) preview_cursor: Option<(Uuid, f32)>,
    pub(super) dark_theme: bool,
    pub(super) app_transition_animation: bool,
    pub(super) localization: Localization,
    pub(super) startup_sound_name: Option<String>,
    pub(super) gemini_api_key: String,
    pub(super) gemini_api_key_visible: bool,
    pub(super) settings_startup_candidate: Option<Uuid>,
    pub(super) library_audio_query: String,
    pub(super) library_audio_tag_filters: Vec<String>,
    pub(super) library_audio_tags_expanded: bool,
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
    pub(super) pending_folder_drag: Option<Uuid>,
    pub(super) library_drop_target_folder: Option<Uuid>,
    pub(super) library_drop_target_root: bool,
    pub(super) library_drop_target_root_rect: Option<Rect>,
    pub(super) library_drop_target_folder_rects: Vec<(Uuid, Rect)>,
    pub(super) ignored_drop_path: Option<PathBuf>,
    pub(super) download_panel_tab: DownloadPanelTab,
    pub(super) download_preview_file: Option<PathBuf>,
    pub(super) download_preview_waveform: Vec<f32>,
    pub(super) download_preview_duration: f32,
    pub(super) download_preview_cursor: Option<f32>,
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
    pub(super) vocal_separation_last_result:
        Option<(VocalSeparationTarget, SeparationStemKind, f32)>,
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
    pub(super) trim_undo_stack: Vec<TrimSnapshot>,
    pub(super) trim_redo_stack: Vec<TrimSnapshot>,
    pub(super) trim_timeline_undo_stack: Vec<TrimTimelineState>,
    pub(super) trim_timeline_redo_stack: Vec<TrimTimelineState>,
    pub(super) processed_export_inflight: HashSet<PathBuf>,
    pub(super) processed_export_tx: Sender<ProcessedExportMessage>,
    pub(super) processed_export_rx: Receiver<ProcessedExportMessage>,
    pub(super) trim_commit_tx: Sender<TrimCommitMessage>,
    pub(super) trim_commit_rx: Receiver<TrimCommitMessage>,
    pub(super) audio_preload_inflight: HashSet<PathBuf>,
    pub(super) audio_preload_queued: HashSet<PathBuf>,
    pub(super) audio_preload_failures: HashMap<PathBuf, String>,
    pub(super) audio_preload_tx: Sender<AudioPreloadMessage>,
    pub(super) audio_preload_rx: Receiver<AudioPreloadMessage>,
    pub(super) timeline_mix_tx: Sender<TimelineMixMessage>,
    pub(super) timeline_mix_rx: Receiver<TimelineMixMessage>,
    pub(super) pending_timeline_mix_restart: Option<(Uuid, f32)>,
    pub(super) library_import_job: Option<ActiveLibraryImport>,
    pub(super) library_import_tx: Sender<LibraryImportMessage>,
    pub(super) library_import_rx: Receiver<LibraryImportMessage>,
    pub(super) normalize_tx: Sender<NormalizeMessage>,
    pub(super) normalize_rx: Receiver<NormalizeMessage>,
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
    pub(super) folder_import_animating: HashMap<Uuid, Instant>,
    pub(super) library_folder_visible_sound_counts: HashMap<Option<Uuid>, usize>,
    pub(super) editing_from_folder: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionPhase {
    Intro,
    Live,
}

pub(crate) struct StartupSplashState {
    pub(crate) phase: TransitionPhase,
    pub(crate) started_at: Option<f64>,
    pub(crate) live_started_at: Option<f64>,
    pub(crate) duration_sec: f32,
    pub(crate) close_sent: bool,
    pub(crate) sound_waveform: Vec<f32>,
    pub(crate) sound_duration_sec: f32,
}

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
        let (tts_tx, tts_rx) = mpsc::channel();
        let (library_hydration_tx, library_hydration_rx) = mpsc::channel();
        let (transition_analysis_tx, transition_analysis_rx) = mpsc::channel();
        let (stream_driver_tx, stream_driver_rx) = mpsc::channel();
        let (vocal_separation_tx, vocal_separation_rx) = mpsc::channel();
        let (processed_export_tx, processed_export_rx) = mpsc::channel();
        let (trim_commit_tx, trim_commit_rx) = mpsc::channel();
        let (audio_preload_tx, audio_preload_rx) = mpsc::channel();
        let (timeline_mix_tx, timeline_mix_rx) = mpsc::channel();
        let (recording_review_tx, recording_review_rx) = mpsc::channel();
        let (library_import_tx, library_import_rx) = mpsc::channel();
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
        let library_row_thickness = storage
            .load_library_row_thickness()
            .ok()
            .flatten()
            .unwrap_or(3)
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS);
        let dark_theme = storage.load_dark_theme().ok().flatten().unwrap_or(false);
        let pitch_show_sharps = storage
            .load_pitch_show_sharps()
            .ok()
            .flatten()
            .unwrap_or(false);
        let pitch_capture_devices = Vec::new();
        let selected_pitch_input_device = None;
        let record_capture_devices = Vec::new();
        let selected_record_input_device = None;
        let stream_input_capture_devices = Vec::new();
        let selected_stream_input_device = None;
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
        let library_sound_view = LibrarySoundView::from_preference(
            storage.load_library_sound_view().ok().flatten().as_deref(),
        );
        let library_folder_view = LibraryFolderView::from_preference(
            storage.load_library_folder_view().ok().flatten().as_deref(),
        );
        let mut localization = Localization::load();
        if let Ok(Some(language_code)) = storage.load_language_code() {
            localization.set_current_code(&language_code);
        }
        let startup_sound_name = storage.load_startup_sound_name().ok().flatten();
        let gemini_api_key = storage
            .load_gemini_api_key()
            .ok()
            .flatten()
            .unwrap_or_default();
        let tts_prompt_presets = storage.load_tts_prompt_presets().unwrap_or_default();
        let tts_draft = storage.load_tts_draft().unwrap_or_default();
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
            download_site_icon_cache: RefCell::new(HashMap::new()),
            vocal_waveform_cache: RefCell::new(HashMap::new()),
            music_waveform_cache: RefCell::new(HashMap::new()),
            library_waveform_preview_cache: RefCell::new(HashMap::new()),
            library_filtered_sound_indices_cache: RefCell::new(HashMap::new()),
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
            trim_commit_output_name: String::new(),
            show_delete_folder_confirm: None,
            trim_timeline_state: None,
            trim_timeline_drop_target: None,
            trim_timeline_preview_path: None,
            trim_timeline_preview_start_secs: 0.0,
            trim_timeline_preview_dirty: false,
            trim_timeline_clip_delete_animating: HashMap::new(),
            trim_timeline_segment_delete_animations: Vec::new(),
            trim_timeline_copied_clip: None,
            trim_timeline_scrub_resume_pending: false,
            import_dir,
            import_audio_entries: Vec::new(),
            app_view: AppView::Editor,
            library_tab: LibraryTab::Sounds,
            folders,
            library_current_folder: None,
            library_collapsed_folders: HashSet::new(),
            editing_folder_id: None,
            folder_rename_name: String::new(),
            new_folder_name: String::new(),
            library_folder_create_open: false,
            recorder: Recorder::new(),
            record_name: "recording".to_owned(),
            record_input_source: PitchInputSource::Microphone,
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
            record_export_video_sharps: false,
            record_export_video_animation: true,
            record_export_video_fps: record_video::STANDARD_VIDEO_FPS,
            center_record_overlay_next_frame: false,
            record_overlay_native_visuals_applied: false,
            record_overlay_pos: None,
            library_grid_columns,
            library_row_thickness,
            library_sound_view,
            library_folder_view,
            video_assets,
            recording_draft: None,
            active_record_video_export: None,
            video_viewer: None,
            recording_review_pending_path: None,
            recording_review_tx,
            recording_review_rx,
            pitch_monitor: PitchMonitor::new(),
            pitch_update_hz,
            pitch_overlay_animation,
            pitch_show_sharps,
            pitch_input_source: PitchInputSource::Microphone,
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
            trim_timeline_view_start_secs: 0.0,
            preview_cursor: None,
            dark_theme,
            app_transition_animation,
            localization,
            startup_sound_name,
            gemini_api_key,
            gemini_api_key_visible: false,
            settings_startup_candidate: None,
            library_audio_query: String::new(),
            library_audio_tag_filters: Vec::new(),
            library_audio_tags_expanded: false,
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
            pending_folder_drag: None,
            library_drop_target_folder: None,
            library_drop_target_root: false,
            library_drop_target_root_rect: None,
            library_drop_target_folder_rects: Vec::new(),
            ignored_drop_path: None,
            download_panel_tab: DownloadPanelTab::Download,
            download_preview_file: None,
            download_preview_waveform: Vec::new(),
            download_preview_duration: 0.0,
            download_preview_cursor: None,
            tts_text: tts_draft.text,
            tts_voice_name: if tts_draft.voice_name.trim().is_empty() {
                "Kore".to_owned()
            } else {
                tts_draft.voice_name
            },
            tts_direction_prompt: tts_draft.direction_prompt,
            tts_prompt_presets,
            tts_preset_name: String::new(),
            tts_output_name: if tts_draft.output_name.trim().is_empty() {
                "gemini tts".to_owned()
            } else {
                tts_draft.output_name
            },
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
            trim_undo_stack: Vec::new(),
            trim_redo_stack: Vec::new(),
            trim_timeline_undo_stack: Vec::new(),
            trim_timeline_redo_stack: Vec::new(),
            processed_export_inflight: HashSet::new(),
            processed_export_tx,
            processed_export_rx,
            trim_commit_tx,
            trim_commit_rx,
            audio_preload_inflight: HashSet::new(),
            audio_preload_queued: HashSet::new(),
            audio_preload_failures: HashMap::new(),
            audio_preload_tx,
            audio_preload_rx,
            timeline_mix_tx,
            timeline_mix_rx,
            pending_timeline_mix_restart: None,
            library_import_job: None,
            library_import_tx,
            library_import_rx,
            normalize_tx,
            normalize_rx,
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
            folder_import_animating: HashMap::new(),
            library_folder_visible_sound_counts: HashMap::new(),
            editing_from_folder: None,
        };

        app.reset_library_tree_state();
        app.begin_async_library_hydration();
        app.begin_async_transition_analysis(resolved_startup_sound);
        app.begin_async_stream_driver_probe();
        app.with_initial_selection()
    }
}
