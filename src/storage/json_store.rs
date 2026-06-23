use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::analyze_audio_file;
use super::models::{LibraryFile, PreferencesFile, VideoLibraryFile};
use super::paths::{DEFAULT_STARTUP_SOUND_BYTES, DEFAULT_STARTUP_SOUND_NAME};
use super::{
    Folder, GeminiTtsDraftPreferences, GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset,
};

impl Storage {
    pub fn load_library(&self) -> Result<Vec<SoundEffect>> {
        if !self.library_path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.library_path).context("unable to read library file")?;
        let mut library: LibraryFile =
            serde_json::from_str(&raw).context("invalid library file format")?;

        library.sounds.retain(|sound| {
            sound.asset_path(&self.root_dir).exists()
                || sound
                    .vocal_asset_path(&self.root_dir)
                    .is_some_and(|path| path.exists())
                || sound
                    .music_asset_path(&self.root_dir)
                    .is_some_and(|path| path.exists())
        });

        for sound in &mut library.sounds {
            sound.clamp_trim();
        }

        Ok(library.sounds)
    }

    pub fn save_library(&self, sounds: &[SoundEffect]) -> Result<()> {
        let folders = self.load_folders().unwrap_or_default();
        self.save_library_with_folders(sounds, &folders)
    }

    pub fn save_library_with_folders(
        &self,
        sounds: &[SoundEffect],
        folders: &[Folder],
    ) -> Result<()> {
        let payload = LibraryFile {
            sounds: sounds.to_vec(),
            folders: folders.to_vec(),
        };
        let json = serde_json::to_string_pretty(&payload).context("unable to serialize library")?;
        fs::write(&self.library_path, json).context("unable to write library file")?;
        Ok(())
    }

    pub fn load_folders(&self) -> Result<Vec<Folder>> {
        if !self.library_path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&self.library_path).context("unable to read library file")?;
        let library: LibraryFile =
            serde_json::from_str(&raw).context("invalid library file format")?;
        Ok(library.folders)
    }

    pub fn save_folders(&self, folders: &[Folder]) -> Result<()> {
        let sounds = self.load_library().unwrap_or_default();
        self.save_library_with_folders(&sounds, folders)
    }

    pub fn load_video_library(&self) -> Result<Vec<VideoAsset>> {
        if !self.video_library_path.exists() {
            return Ok(Vec::new());
        }

        let raw =
            fs::read_to_string(&self.video_library_path).context("unable to read video library")?;
        let mut library: VideoLibraryFile =
            serde_json::from_str(&raw).context("invalid video library format")?;
        library
            .videos
            .retain(|video| video.asset_path(&self.root_dir).exists());
        let mut changed = false;
        for video in &mut library.videos {
            let normalized_fps = video.normalized_fps();
            if video.fps != normalized_fps {
                video.fps = normalized_fps;
                changed = true;
            }
            if video.waveform.is_empty() {
                if let Ok(analysis) = analyze_audio_file(&video.asset_path(&self.root_dir), 96) {
                    video.waveform = analysis.waveform;
                    changed = true;
                } else {
                    video.waveform = vec![0.0];
                    changed = true;
                }
            }
        }
        if changed {
            let json = serde_json::to_string_pretty(&library)
                .context("unable to serialize video library")?;
            fs::write(&self.video_library_path, json)
                .context("unable to write video library file")?;
        }
        Ok(library.videos)
    }

    pub fn save_video_library(&self, videos: &[VideoAsset]) -> Result<()> {
        let payload = VideoLibraryFile {
            videos: videos.to_vec(),
        };
        let json =
            serde_json::to_string_pretty(&payload).context("unable to serialize video library")?;
        fs::write(&self.video_library_path, json).context("unable to write video library file")?;
        Ok(())
    }

    pub fn load_import_dir(&self) -> Result<Option<PathBuf>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.import_dir.filter(|path| path.exists()))
    }

    pub fn save_import_dir(&self, import_dir: Option<&Path>) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.import_dir = import_dir.map(Path::to_path_buf);
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_update_hz(&self) -> Result<Option<f32>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .pitch_update_hz
            .map(|value| value.clamp(1.0, 12.0)))
    }

    pub fn save_pitch_update_hz(&self, pitch_update_hz: f32) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_update_hz = Some(pitch_update_hz.clamp(1.0, 12.0));
        self.save_preferences(&preferences)
    }

    pub fn load_overlay_animation(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.overlay_animation)
    }

    pub fn save_overlay_animation(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.overlay_animation = Some(enabled);
        self.save_preferences(&preferences)
    }

    pub fn load_app_transition_animation(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.app_transition_animation)
    }

    pub fn save_app_transition_animation(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.app_transition_animation = Some(enabled);
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_show_sharps(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.pitch_show_sharps)
    }

    pub fn save_pitch_show_sharps(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_show_sharps = Some(enabled);
        self.save_preferences(&preferences)
    }

    pub fn load_language_code(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .language_code
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()))
    }

    pub fn save_language_code(&self, language_code: &str) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        let trimmed = language_code.trim();
        preferences.language_code = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        self.save_preferences(&preferences)
    }

    pub fn load_library_grid_columns(&self) -> Result<Option<usize>> {
        let preferences = self.load_preferences()?;
        if let Some(columns) = preferences.library_grid_columns {
            return Ok(Some(columns.clamp(3, 8)));
        }
        Ok(preferences.library_grid_scale.map(|scale| {
            let normalized = ((1.1 - scale.clamp(0.72, 1.1)) / (1.1 - 0.72)).clamp(0.0, 1.0);
            let mapped = 3.0 + normalized * 5.0;
            mapped.round().clamp(3.0, 8.0) as usize
        }))
    }

    pub fn save_library_grid_columns(&self, columns: usize) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.library_grid_columns = Some(columns.clamp(3, 8));
        preferences.library_grid_scale = None;
        self.save_preferences(&preferences)
    }

    pub fn load_library_sound_view(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.library_sound_view)
    }

    pub fn save_library_sound_view(&self, view: &str) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        let normalized = match view.trim().to_ascii_lowercase().as_str() {
            "grid" => Some("grid".to_owned()),
            "rows" => Some("rows".to_owned()),
            _ => None,
        };
        preferences.library_sound_view = normalized;
        self.save_preferences(&preferences)
    }

    pub fn load_library_folder_view(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.library_folder_view)
    }

    pub fn save_library_folder_view(&self, view: &str) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        let normalized = match view.trim().to_ascii_lowercase().as_str() {
            "grid" => Some("grid".to_owned()),
            "rows" => Some("rows".to_owned()),
            _ => None,
        };
        preferences.library_folder_view = normalized;
        self.save_preferences(&preferences)
    }

    pub fn load_library_row_thickness(&self) -> Result<Option<usize>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .library_row_thickness
            .map(|value| value.clamp(1, 5)))
    }

    pub fn save_library_row_thickness(&self, thickness: usize) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.library_row_thickness = Some(thickness.clamp(1, 5));
        self.save_preferences(&preferences)
    }

    pub fn load_dark_theme(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.dark_theme)
    }

    pub fn save_dark_theme(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.dark_theme = Some(enabled);
        self.save_preferences(&preferences)
    }

    pub fn load_record_hotkeys(&self) -> Result<Vec<String>> {
        let preferences = self.load_preferences()?;
        if let Some(list) = preferences.record_hotkeys {
            Ok(list)
        } else if let Some(single) = preferences.record_hotkey {
            Ok(vec![single])
        } else {
            Ok(Vec::new())
        }
    }

    pub fn save_record_hotkeys(&self, hotkeys: &[String]) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.record_hotkeys = Some(hotkeys.to_vec());
        preferences.record_hotkey = hotkeys.first().cloned();
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_hotkeys(&self) -> Result<Vec<String>> {
        let preferences = self.load_preferences()?;
        if let Some(list) = preferences.pitch_hotkeys {
            Ok(list)
        } else if let Some(single) = preferences.pitch_hotkey {
            Ok(vec![single])
        } else {
            Ok(Vec::new())
        }
    }

    pub fn save_pitch_hotkeys(&self, hotkeys: &[String]) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_hotkeys = Some(hotkeys.to_vec());
        preferences.pitch_hotkey = hotkeys.first().cloned();
        self.save_preferences(&preferences)
    }

    pub fn load_gemini_api_key(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.gemini_api_key)
    }

    pub fn save_gemini_api_key(&self, api_key: &str) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        let trimmed = api_key.trim();
        preferences.gemini_api_key = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        self.save_preferences(&preferences)
    }

    pub fn load_tts_prompt_presets(&self) -> Result<Vec<GeminiTtsPromptPreset>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .tts_prompt_presets
            .into_iter()
            .filter_map(|preset| {
                let name = preset.name.trim().to_owned();
                let prompt = preset.prompt.trim().to_owned();
                (!name.is_empty() && !prompt.is_empty())
                    .then_some(GeminiTtsPromptPreset { name, prompt })
            })
            .collect())
    }

    pub fn save_tts_prompt_presets(&self, presets: &[GeminiTtsPromptPreset]) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.tts_prompt_presets = presets
            .iter()
            .filter_map(|preset| {
                let name = preset.name.trim().to_owned();
                let prompt = preset.prompt.trim().to_owned();
                (!name.is_empty() && !prompt.is_empty())
                    .then_some(GeminiTtsPromptPreset { name, prompt })
            })
            .collect();
        self.save_preferences(&preferences)
    }

    pub fn load_tts_draft(&self) -> Result<GeminiTtsDraftPreferences> {
        let preferences = self.load_preferences()?;
        Ok(preferences.tts_draft)
    }

    pub fn save_tts_draft(&self, draft: &GeminiTtsDraftPreferences) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.tts_draft = GeminiTtsDraftPreferences {
            text: draft.text.trim().to_owned(),
            voice_name: draft.voice_name.trim().to_owned(),
            direction_prompt: draft.direction_prompt.trim().to_owned(),
            output_name: draft.output_name.trim().to_owned(),
        };
        self.save_preferences(&preferences)
    }

    pub fn load_startup_sound_name(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        if let Some(name) = preferences
            .startup_sound_name
            .filter(|_| self.startup_sound_path().exists())
        {
            return Ok(Some(name));
        }
        if preferences.startup_sound_cleared.unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(DEFAULT_STARTUP_SOUND_NAME.to_owned()))
    }

    pub fn startup_sound_path(&self) -> PathBuf {
        self.settings_sounds_dir.join("startup.wav")
    }

    pub fn resolved_startup_sound_path(&self) -> Result<Option<PathBuf>> {
        let preferences = self.load_preferences()?;
        if preferences.startup_sound_cleared.unwrap_or(false) {
            return Ok(None);
        }
        let custom_path = self.startup_sound_path();
        if custom_path.exists() {
            return Ok(Some(custom_path));
        }
        let bundled_path = self.bundled_sounds_dir.join("default-startup.wav");
        self.ensure_bundled_sound(&bundled_path, DEFAULT_STARTUP_SOUND_BYTES)?;
        Ok(Some(bundled_path))
    }

    pub fn save_startup_sound(&self, sound: &SoundEffect) -> Result<()> {
        self.save_special_sound(sound, &self.startup_sound_path(), |preferences, name| {
            preferences.startup_sound_name = Some(name);
            preferences.startup_sound_cleared = Some(false);
        })
    }

    pub fn clear_startup_sound(&self) -> Result<()> {
        let path = self.startup_sound_path();
        if path.exists() {
            fs::remove_file(path).context("unable to remove startup sound")?;
        }
        let mut preferences = self.load_preferences()?;
        preferences.startup_sound_name = None;
        preferences.startup_sound_cleared = Some(true);
        self.save_preferences(&preferences)
    }

    pub fn reset_startup_sound(&self) -> Result<()> {
        let path = self.startup_sound_path();
        if path.exists() {
            fs::remove_file(path).context("unable to remove startup sound override")?;
        }
        let mut preferences = self.load_preferences()?;
        preferences.startup_sound_name = None;
        preferences.startup_sound_cleared = Some(false);
        self.save_preferences(&preferences)
    }

    pub(super) fn load_preferences(&self) -> Result<PreferencesFile> {
        if !self.preferences_path.exists() {
            return Ok(PreferencesFile::default());
        }

        let raw = fs::read_to_string(&self.preferences_path)
            .context("unable to read preferences file")?;
        serde_json::from_str(&raw).context("invalid preferences file format")
    }

    pub(super) fn save_preferences(&self, preferences: &PreferencesFile) -> Result<()> {
        let json =
            serde_json::to_string_pretty(preferences).context("unable to serialize preferences")?;
        fs::write(&self.preferences_path, json).context("unable to write preferences file")?;
        Ok(())
    }
}
