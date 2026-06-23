use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingDraftMode {
    Recording,
    VideoExport,
}

pub(crate) struct RecordingDraft {
    pub(crate) mode: RecordingDraftMode,
    pub(crate) sound: SoundEffect,
    pub(crate) source_path: PathBuf,
    pub(crate) source_is_temporary: bool,
    pub(crate) keep_vocal: bool,
    pub(crate) vocal_separated_path: Option<PathBuf>,
    pub(crate) keep_music: bool,
    pub(crate) music_separated_path: Option<PathBuf>,
}

pub(crate) struct VideoViewerState {
    pub(crate) video: VideoAsset,
    pub(crate) frame_paths: Vec<PathBuf>,
    pub(crate) audio_path: PathBuf,
    pub(crate) progress: f32,
    pub(crate) current_frame: Option<(usize, TextureHandle, Vec2)>,
}

pub(crate) struct RecordVideoExportState {
    pub(crate) progress: f32,
    pub(crate) stage: String,
    pub(crate) receiver: Receiver<RecordVideoExportMessage>,
}

pub(crate) struct RecordVideoExportResult {
    pub(crate) processed_audio_path: PathBuf,
    pub(crate) video_path: PathBuf,
    pub(crate) duration_secs: f32,
    pub(crate) video_fps: u32,
    pub(crate) video_name: String,
}

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

pub(crate) enum RecordingReviewMessage {
    Finished {
        path: PathBuf,
        result: Result<SoundEffect, String>,
    },
}

pub(crate) enum LibraryImportMessage {
    Progress {
        job_id: Uuid,
        completed: usize,
        total: usize,
        current_label: String,
    },
    Finished {
        job_id: Uuid,
        result: Result<LibraryImportResult, String>,
    },
}

pub(crate) struct LibraryImportResult {
    pub(crate) target_folder_id: Option<Uuid>,
    pub(crate) imported_sounds: Vec<SoundEffect>,
    pub(crate) imported_folders: Vec<Folder>,
    pub(crate) ignored_count: usize,
}

pub(crate) struct ActiveLibraryImport {
    pub(crate) job_id: Uuid,
    pub(crate) target_folder_id: Option<Uuid>,
    pub(crate) completed: usize,
    pub(crate) total: usize,
    pub(crate) current_label: String,
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
    pub(crate) display_name: String,
}

pub(crate) enum GeminiTtsMessage {
    Finished(Result<GeminiTtsResult, String>),
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
