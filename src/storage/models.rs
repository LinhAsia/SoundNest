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
    pub cut_start_secs: Option<f32>,
    #[serde(default)]
    pub cut_end_secs: Option<f32>,
    #[serde(default)]
    pub display_trim_start_secs: Option<f32>,
    #[serde(default)]
    pub display_trim_end_secs: Option<f32>,
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
    pub eight_d_enabled: bool,
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
    pub fn trim_ranges(&self) -> Vec<(f32, f32)> {
        let duration = self.safe_duration();
        let trim_start = self.trim_start_secs.clamp(0.0, duration);
        let trim_end = self.trim_end_secs.clamp(trim_start + 0.001, duration);
        let mut ranges = vec![(trim_start, trim_end)];
        if let (Some(cut_start), Some(cut_end)) = (self.cut_start_secs, self.cut_end_secs) {
            let cut_start = cut_start.clamp(trim_start, trim_end);
            let cut_end = cut_end.clamp(trim_start, trim_end);
            if cut_end - cut_start > 0.001 {
                ranges.clear();
                if cut_start - trim_start > 0.001 {
                    ranges.push((trim_start, cut_start));
                }
                if trim_end - cut_end > 0.001 {
                    ranges.push((cut_end, trim_end));
                }
                if ranges.is_empty() {
                    ranges.push((trim_start, trim_end));
                }
            }
        }
        ranges
    }

    pub fn display_duration_secs(&self) -> f32 {
        if self.has_cutout() {
            self.trimmed_length().max(0.05)
        } else {
            self.safe_duration()
        }
    }

    pub fn display_trim_start(&self) -> f32 {
        if self.has_cutout() {
            self.display_trim_start_secs
                .unwrap_or(0.0)
                .clamp(0.0, self.display_duration_secs())
        } else {
            self.trim_start_secs
        }
    }

    pub fn display_trim_end(&self) -> f32 {
        if self.has_cutout() {
            self.display_trim_end_secs
                .unwrap_or(self.display_duration_secs())
                .clamp(
                    self.display_trim_start() + 0.001,
                    self.display_duration_secs(),
                )
        } else {
            self.trim_end_secs
        }
    }

    pub fn has_cutout(&self) -> bool {
        if let (Some(cut_start), Some(cut_end)) = (self.cut_start_secs, self.cut_end_secs) {
            return cut_end - cut_start > 0.001;
        }
        false
    }

    pub fn clear_cutout(&mut self) {
        self.cut_start_secs = None;
        self.cut_end_secs = None;
        self.display_trim_start_secs = None;
        self.display_trim_end_secs = None;
    }

    pub fn timeline_secs_to_output_secs(&self, timeline_secs: f32) -> f32 {
        let mut played = 0.0;
        for (start, end) in self.trim_ranges() {
            let length = (end - start).max(0.0);
            if timeline_secs <= start {
                return played;
            }
            if timeline_secs < end {
                return played + (timeline_secs - start);
            }
            played += length;
        }
        played
    }

    pub fn output_secs_to_timeline_secs(&self, output_secs: f32) -> f32 {
        let mut remaining = output_secs.max(0.0);
        let ranges = self.trim_ranges();
        for (start, end) in &ranges {
            let length = (*end - *start).max(0.0);
            if remaining <= length {
                return *start + remaining;
            }
            remaining -= length;
        }
        ranges
            .last()
            .map(|(_, end)| *end)
            .unwrap_or(self.trim_end_secs)
    }

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
        self.trim_ranges()
            .into_iter()
            .map(|(start, end)| (end - start).max(0.0))
            .sum::<f32>()
            .max(0.05)
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

        if let (Some(cut_start), Some(cut_end)) = (self.cut_start_secs, self.cut_end_secs) {
            let clamped_start = cut_start.clamp(self.trim_start_secs, self.trim_end_secs);
            let clamped_end = cut_end.clamp(self.trim_start_secs, self.trim_end_secs);
            if clamped_end - clamped_start > 0.001 {
                self.cut_start_secs = Some(clamped_start);
                self.cut_end_secs = Some(clamped_end);
            } else {
                self.clear_cutout();
            }
        }

        if self.has_cutout() {
            let display_duration = self.display_duration_secs();
            let display_start = self
                .display_trim_start_secs
                .unwrap_or(0.0)
                .clamp(0.0, display_duration);
            let display_end = self
                .display_trim_end_secs
                .unwrap_or(display_duration)
                .clamp(display_start + 0.001, display_duration);
            self.display_trim_start_secs = Some(display_start);
            self.display_trim_end_secs = Some(display_end);
        } else {
            self.display_trim_start_secs = None;
            self.display_trim_end_secs = None;
        }
    }

    pub fn needs_processed_export(&self) -> bool {
        const EPSILON: f32 = 0.005;
        self.needs_preprocessed_preview()
            || self.trim_start_secs.abs() > EPSILON
            || (self.trim_end_secs - self.safe_duration()).abs() > 0.02
    }

    pub fn needs_preprocessed_preview(&self) -> bool {
        self.has_cutout()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sound() -> SoundEffect {
        SoundEffect {
            id: Uuid::nil(),
            name: "test".into(),
            asset_file: "test.wav".into(),
            favorite: false,
            tags: Vec::new(),
            duration_secs: 100.0,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 40.0,
            trim_end_secs: 50.0,
            cut_start_secs: None,
            cut_end_secs: None,
            display_trim_start_secs: None,
            display_trim_end_secs: None,
            vocal_only: false,
            vocal_asset_file: None,
            music_only: false,
            music_asset_file: None,
            reverb_enabled: false,
            telephone_enabled: false,
            distortion_enabled: false,
            echo_enabled: false,
            underwater_enabled: false,
            robot_enabled: false,
            eight_d_enabled: false,
            pitch_shift_enabled: false,
            pitch_shift_semitones: 0.0,
            waveform: Vec::new(),
            folder_id: None,
        }
    }

    #[test]
    fn trim_only_preview_streams_without_preprocessing() {
        let mut sound = sound();
        assert!(sound.needs_processed_export());
        assert!(!sound.needs_preprocessed_preview());

        sound.cut_start_secs = Some(44.0);
        sound.cut_end_secs = Some(46.0);
        assert!(sound.needs_preprocessed_preview());
    }
}
