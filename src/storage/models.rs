use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::Storage;
use super::paths::{
    default_pitch_semitones, default_speed, default_video_fps, normalize_video_fps,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoundEffect {
    pub id: Uuid,
    pub name: String,
    pub asset_file: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    pub duration_secs: f32,
    pub volume: f32,
    #[serde(default = "default_speed")]
    pub speed: f32,
    pub trim_start_secs: f32,
    pub trim_end_secs: f32,
    #[serde(default)]
    pub vocal_only: bool,
    #[serde(default)]
    pub vocal_asset_file: Option<String>,
    #[serde(default)]
    pub music_only: bool,
    #[serde(default)]
    pub music_asset_file: Option<String>,
    #[serde(default)]
    pub reverb_enabled: bool,
    #[serde(default)]
    pub telephone_enabled: bool,
    #[serde(default)]
    pub distortion_enabled: bool,
    #[serde(default)]
    pub echo_enabled: bool,
    #[serde(default)]
    pub underwater_enabled: bool,
    #[serde(default)]
    pub robot_enabled: bool,
    #[serde(default)]
    pub pitch_shift_enabled: bool,
    #[serde(default = "default_pitch_semitones")]
    pub pitch_shift_semitones: f32,
    #[serde(default)]
    pub waveform: Vec<f32>,
    #[serde(default)]
    pub folder_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoAsset {
    pub id: Uuid,
    pub name: String,
    pub asset_file: String,
    #[serde(default)]
    pub favorite: bool,
    pub duration_secs: f32,
    #[serde(default = "default_video_fps")]
    pub fps: u32,
    #[serde(default)]
    pub waveform: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct GeminiTtsPromptPreset {
    pub name: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct GeminiTtsDraftPreferences {
    pub text: String,
    pub voice_name: String,
    pub direction_prompt: String,
    pub output_name: String,
}

impl VideoAsset {
    pub fn asset_path(&self, storage_dir: &Path) -> PathBuf {
        storage_dir.join("videos").join(&self.asset_file)
    }

    pub fn normalized_fps(&self) -> u32 {
        normalize_video_fps(self.fps)
    }
}

impl SoundEffect {
    pub fn asset_path(&self, storage_dir: &Path) -> PathBuf {
        storage_dir.join("sounds").join(&self.asset_file)
    }

    pub fn vocal_asset_path(&self, storage_dir: &Path) -> Option<PathBuf> {
        self.vocal_asset_file
            .as_ref()
            .map(|asset_file| storage_dir.join("sound-vocals").join(asset_file))
    }

    pub fn music_asset_path(&self, storage_dir: &Path) -> Option<PathBuf> {
        self.music_asset_file
            .as_ref()
            .map(|asset_file| storage_dir.join("sound-music").join(asset_file))
    }

    pub fn playback_asset_path(&self, storage_dir: &Path) -> PathBuf {
        if self.music_only
            && let Some(path) = self.music_asset_path(storage_dir)
            && path.exists()
        {
            return path;
        }
        if self.vocal_only
            && let Some(path) = self.vocal_asset_path(storage_dir)
            && path.exists()
        {
            return path;
        }
        self.asset_path(storage_dir)
    }

    pub fn safe_duration(&self) -> f32 {
        self.duration_secs.max(0.05)
    }

    pub fn trimmed_length(&self) -> f32 {
        (self.trim_end_secs - self.trim_start_secs).max(0.05)
    }

    pub fn clamp_trim(&mut self) {
        let duration = self.safe_duration();
        self.speed = self.speed.clamp(0.25, 2.0);
        self.trim_start_secs = self.trim_start_secs.clamp(0.0, duration);
        self.trim_end_secs = self.trim_end_secs.clamp(0.0, duration);

        if self.trim_end_secs <= self.trim_start_secs {
            self.trim_end_secs = (self.trim_start_secs + 0.05).min(duration);
            self.trim_start_secs = (self.trim_end_secs - 0.05).max(0.0);
        }
    }

    pub fn needs_processed_export(&self) -> bool {
        const EPSILON: f32 = 0.005;
        (self.volume - 1.0).abs() > EPSILON
            || (self.speed - 1.0).abs() > EPSILON
            || self.trim_start_secs.abs() > EPSILON
            || (self.trim_end_secs - self.safe_duration()).abs() > 0.02
            || self.reverb_enabled
            || self.telephone_enabled
            || self.distortion_enabled
            || self.echo_enabled
            || self.underwater_enabled
            || self.robot_enabled
            || self.pitch_shift_enabled
    }
}

impl Storage {
    pub fn vocal_asset_file_name(sound_id: Uuid) -> String {
        format!("{sound_id}-vocals.wav")
    }

    pub fn vocal_asset_path_for(&self, sound: &SoundEffect) -> Option<PathBuf> {
        sound
            .vocal_asset_file
            .as_ref()
            .map(|asset_file| self.vocal_sounds_dir.join(asset_file))
    }

    pub fn music_asset_file_name(sound_id: Uuid) -> String {
        format!("{sound_id}-music.wav")
    }

    pub fn music_asset_path_for(&self, sound: &SoundEffect) -> Option<PathBuf> {
        sound
            .music_asset_file
            .as_ref()
            .map(|asset_file| self.music_sounds_dir.join(asset_file))
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(super) struct LibraryFile {
    pub(super) sounds: Vec<SoundEffect>,
    #[serde(default)]
    pub(super) folders: Vec<Folder>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(super) struct VideoLibraryFile {
    pub(super) videos: Vec<VideoAsset>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(super) struct PreferencesFile {
    pub(super) import_dir: Option<PathBuf>,
    pub(super) pitch_update_hz: Option<f32>,
    pub(super) overlay_animation: Option<bool>,
    pub(super) app_transition_animation: Option<bool>,
    pub(super) pitch_show_sharps: Option<bool>,
    pub(super) language_code: Option<String>,
    pub(super) library_grid_columns: Option<usize>,
    pub(super) library_grid_scale: Option<f32>,
    pub(super) library_sound_view: Option<String>,
    pub(super) library_folder_view: Option<String>,
    pub(super) library_row_thickness: Option<usize>,
    pub(super) dark_theme: Option<bool>,
    pub(super) record_hotkey: Option<String>,
    pub(super) pitch_hotkey: Option<String>,
    #[serde(default)]
    pub(super) record_hotkeys: Option<Vec<String>>,
    #[serde(default)]
    pub(super) pitch_hotkeys: Option<Vec<String>>,
    pub(super) startup_sound_name: Option<String>,
    pub(super) startup_sound_cleared: Option<bool>,
    pub(super) gemini_api_key: Option<String>,
    #[serde(default)]
    pub(super) tts_prompt_presets: Vec<GeminiTtsPromptPreset>,
    #[serde(default)]
    pub(super) tts_draft: GeminiTtsDraftPreferences,
}
