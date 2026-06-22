mod models;
mod paths;

pub use self::models::{
    Folder, GeminiTtsDraftPreferences, GeminiTtsPromptPreset, SoundEffect, VideoAsset,
};
pub use self::paths::format_time;

use self::models::{LibraryFile, PreferencesFile, VideoLibraryFile};
use self::paths::{
    DEFAULT_EXIT_SOUND_BYTES, DEFAULT_EXIT_SOUND_NAME, DEFAULT_STARTUP_SOUND_BYTES,
    DEFAULT_STARTUP_SOUND_NAME, WAVEFORM_BUCKETS, committed_trimmed_sound_name,
    migrate_storage_root_if_needed, normalize_video_fps, preferred_storage_root,
    sanitize_file_system_name, sanitize_stem, sound_asset_file_name, unique_directory_path,
    unique_file_path,
};
use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavSpec, WavWriter};
use rodio::{Decoder, Source};
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::BufReader;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;
pub struct Storage {
    root_dir: PathBuf,
    sounds_dir: PathBuf,
    vocal_sounds_dir: PathBuf,
    music_sounds_dir: PathBuf,
    videos_dir: PathBuf,
    settings_sounds_dir: PathBuf,
    bundled_sounds_dir: PathBuf,
    exports_dir: PathBuf,
    library_path: PathBuf,
    video_library_path: PathBuf,
    preferences_path: PathBuf,
}

impl Storage {
    pub fn new() -> Result<Self> {
        let root_dir = preferred_storage_root()?;
        migrate_storage_root_if_needed(&root_dir)?;
        let sounds_dir = root_dir.join("sounds");
        let vocal_sounds_dir = root_dir.join("sound-vocals");
        let music_sounds_dir = root_dir.join("sound-music");
        let videos_dir = root_dir.join("videos");
        let settings_sounds_dir = root_dir.join("settings-sounds");
        let bundled_sounds_dir = root_dir.join("bundled-sounds");
        let exports_dir = root_dir.join("exports");
        fs::create_dir_all(&sounds_dir).context("unable to create sounds directory")?;
        fs::create_dir_all(&vocal_sounds_dir).context("unable to create vocal sounds directory")?;
        fs::create_dir_all(&music_sounds_dir).context("unable to create music sounds directory")?;
        fs::create_dir_all(&videos_dir).context("unable to create videos directory")?;
        fs::create_dir_all(&settings_sounds_dir)
            .context("unable to create settings sounds directory")?;
        fs::create_dir_all(&bundled_sounds_dir)
            .context("unable to create bundled sounds directory")?;
        fs::create_dir_all(&exports_dir).context("unable to create exports directory")?;
        let library_path = root_dir.join("library.json");
        let video_library_path = root_dir.join("video_library.json");
        let preferences_path = root_dir.join("preferences.json");

        Ok(Self {
            root_dir,
            sounds_dir,
            vocal_sounds_dir,
            music_sounds_dir,
            videos_dir,
            settings_sounds_dir,
            bundled_sounds_dir,
            exports_dir,
            library_path,
            video_library_path,
            preferences_path,
        })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

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

    pub fn load_exit_sound_name(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        if let Some(name) = preferences
            .exit_sound_name
            .filter(|_| self.exit_sound_path().exists())
        {
            return Ok(Some(name));
        }
        if preferences.exit_sound_cleared.unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(DEFAULT_EXIT_SOUND_NAME.to_owned()))
    }

    pub fn startup_sound_path(&self) -> PathBuf {
        self.settings_sounds_dir.join("startup.wav")
    }

    pub fn exit_sound_path(&self) -> PathBuf {
        self.settings_sounds_dir.join("exit.wav")
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

    pub fn resolved_exit_sound_path(&self) -> Result<Option<PathBuf>> {
        let preferences = self.load_preferences()?;
        if preferences.exit_sound_cleared.unwrap_or(false) {
            return Ok(None);
        }
        let custom_path = self.exit_sound_path();
        if custom_path.exists() {
            return Ok(Some(custom_path));
        }
        let bundled_path = self.bundled_sounds_dir.join("default-exit.wav");
        self.ensure_bundled_sound(&bundled_path, DEFAULT_EXIT_SOUND_BYTES)?;
        Ok(Some(bundled_path))
    }

    pub fn save_startup_sound(&self, sound: &SoundEffect) -> Result<()> {
        self.save_special_sound(sound, &self.startup_sound_path(), |preferences, name| {
            preferences.startup_sound_name = Some(name);
            preferences.startup_sound_cleared = Some(false);
        })
    }

    pub fn save_exit_sound(&self, sound: &SoundEffect) -> Result<()> {
        self.save_special_sound(sound, &self.exit_sound_path(), |preferences, name| {
            preferences.exit_sound_name = Some(name);
            preferences.exit_sound_cleared = Some(false);
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

    pub fn clear_exit_sound(&self) -> Result<()> {
        let path = self.exit_sound_path();
        if path.exists() {
            fs::remove_file(path).context("unable to remove exit sound")?;
        }
        let mut preferences = self.load_preferences()?;
        preferences.exit_sound_name = None;
        preferences.exit_sound_cleared = Some(true);
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

    pub fn reset_exit_sound(&self) -> Result<()> {
        let path = self.exit_sound_path();
        if path.exists() {
            fs::remove_file(path).context("unable to remove exit sound override")?;
        }
        let mut preferences = self.load_preferences()?;
        preferences.exit_sound_name = None;
        preferences.exit_sound_cleared = Some(false);
        self.save_preferences(&preferences)
    }

    fn load_preferences(&self) -> Result<PreferencesFile> {
        if !self.preferences_path.exists() {
            return Ok(PreferencesFile::default());
        }

        let raw = fs::read_to_string(&self.preferences_path)
            .context("unable to read preferences file")?;
        serde_json::from_str(&raw).context("invalid preferences file format")
    }

    fn save_preferences(&self, preferences: &PreferencesFile) -> Result<()> {
        let json =
            serde_json::to_string_pretty(preferences).context("unable to serialize preferences")?;
        fs::write(&self.preferences_path, json).context("unable to write preferences file")?;
        Ok(())
    }

    fn ensure_bundled_sound(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        fs::write(path, bytes).with_context(|| format!("unable to write {}", path.display()))?;
        Ok(())
    }

    pub fn import_sound(&self, source_path: &Path) -> Result<SoundEffect> {
        Self::import_sound_into_dir(&self.sounds_dir, source_path)
    }

    pub fn import_sound_at(root_dir: &Path, source_path: &Path) -> Result<SoundEffect> {
        Self::import_sound_into_dir(&root_dir.join("sounds"), source_path)
    }

    fn import_sound_into_dir(sounds_dir: &Path, source_path: &Path) -> Result<SoundEffect> {
        if !source_path.exists() {
            bail!("file not found");
        }

        let name = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Sound")
            .to_owned();
        let extension = source_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let id = Uuid::new_v4();
        let asset_file = sound_asset_file_name(&name, id, extension);
        let target_path = sounds_dir.join(&asset_file);

        fs::copy(source_path, &target_path).context("unable to copy imported sound")?;

        let analysis = match analyze_audio_file(&target_path, WAVEFORM_BUCKETS) {
            Ok(analysis) => analysis,
            Err(error) => {
                let _ = fs::remove_file(&target_path);
                return Err(error);
            }
        };

        Ok(SoundEffect {
            id,
            name,
            asset_file,
            favorite: false,
            tags: Vec::new(),
            duration_secs: analysis.duration_secs,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 0.0,
            trim_end_secs: analysis.duration_secs,
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
            pitch_shift_enabled: false,
            pitch_shift_semitones: 0.0,
            waveform: analysis.waveform,
            folder_id: None,
        })
    }

    pub fn duplicate_trimmed_sound_at(root_dir: &Path, sound: &SoundEffect) -> Result<SoundEffect> {
        let export_path = Self::export_processed_sound_at(root_dir, sound)?;
        if export_path.exists() {
            let import_result =
                Self::import_sound_at(root_dir, &export_path).map(|mut imported_sound| {
                    imported_sound.name = committed_trimmed_sound_name(sound);
                    imported_sound
                });
            let _ = fs::remove_file(&export_path);
            return import_result;
        }
        Self::import_sound_at(root_dir, &export_path).map(|mut imported_sound| {
            imported_sound.name = committed_trimmed_sound_name(sound);
            imported_sound
        })
    }

    pub fn hydrate_sound(&self, sound: &mut SoundEffect) -> Result<bool> {
        if !sound.waveform.is_empty() && sound.duration_secs > 0.0 {
            sound.clamp_trim();
            return Ok(false);
        }

        let analysis =
            analyze_audio_file(&sound.playback_asset_path(&self.root_dir), WAVEFORM_BUCKETS)?;
        sound.duration_secs = analysis.duration_secs;
        if sound.waveform.is_empty() {
            sound.waveform = analysis.waveform;
        }
        if sound.trim_end_secs <= 0.0 {
            sound.trim_end_secs = sound.duration_secs;
        }
        sound.clamp_trim();
        Ok(true)
    }

    pub fn remove_sound(&self, sound: &SoundEffect) -> Result<()> {
        let path = sound.asset_path(&self.root_dir);
        if path.exists() {
            fs::remove_file(path).context("unable to delete audio asset")?;
        }
        if let Some(vocal_path) = sound.vocal_asset_path(&self.root_dir)
            && vocal_path.exists()
        {
            fs::remove_file(vocal_path).context("unable to delete vocal asset")?;
        }
        if let Some(music_path) = sound.music_asset_path(&self.root_dir)
            && music_path.exists()
        {
            fs::remove_file(music_path).context("unable to delete music asset")?;
        }
        Ok(())
    }

    pub fn remove_video(&self, video: &VideoAsset) -> Result<()> {
        let path = video.asset_path(&self.root_dir);
        if path.exists() {
            fs::remove_file(path).context("unable to delete video asset")?;
        }
        Ok(())
    }

    pub fn export_processed_sound(&self, sound: &SoundEffect) -> Result<PathBuf> {
        if sound.needs_processed_export() {
            Self::export_processed_sound_at(&self.root_dir, sound)
        } else {
            self.export_processed_sound_from_path(&sound.playback_asset_path(&self.root_dir), sound)
        }
    }

    pub fn processed_export_path(root_dir: &Path, sound: &SoundEffect) -> PathBuf {
        Self::processed_export_path_in(&root_dir.join("exports"), sound)
    }

    pub fn processed_export_path_in(exports_dir: &Path, sound: &SoundEffect) -> PathBuf {
        let extension = "wav";
        let export_name = format!(
            "{}-{}{}.{}",
            sanitize_stem(&sound.name),
            &sound.id.to_string()[..8],
            Self::processed_export_suffix(sound),
            extension
        );
        exports_dir.join(export_name)
    }

    pub fn processed_export_exists(root_dir: &Path, sound: &SoundEffect) -> bool {
        Self::processed_export_path(root_dir, sound).exists()
    }

    pub fn export_processed_sound_at(root_dir: &Path, sound: &SoundEffect) -> Result<PathBuf> {
        Self::export_processed_sound_from_path_at(
            root_dir,
            &sound.playback_asset_path(root_dir),
            sound,
        )
    }

    pub fn drag_sound_source_path(&self, sound: &SoundEffect) -> Result<PathBuf> {
        if sound.needs_processed_export() {
            if Self::processed_export_path_in(&self.exports_dir, sound).exists() {
                Ok(Self::processed_export_path_in(&self.exports_dir, sound))
            } else {
                self.export_processed_sound(sound)
            }
        } else {
            Ok(sound.playback_asset_path(&self.root_dir))
        }
    }

    pub fn drag_folder_source_path(
        &self,
        root_folder_id: Uuid,
        folders: &[Folder],
        sounds: &[SoundEffect],
    ) -> Result<PathBuf> {
        let root_folder = folders
            .iter()
            .find(|folder| folder.id == root_folder_id)
            .context("folder not found")?;
        let mut folder_ids = vec![root_folder_id];
        let mut index = 0usize;
        while let Some(parent_id) = folder_ids.get(index).copied() {
            index += 1;
            for folder in folders
                .iter()
                .filter(|folder| folder.parent_id == Some(parent_id))
            {
                folder_ids.push(folder.id);
            }
        }

        let branch_ids = folder_ids.iter().copied().collect::<Vec<_>>();
        let branch_lookup = branch_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let staging_root = self.exports_dir.join("folder-drag").join(format!(
            "{}-{}",
            sanitize_file_system_name(&root_folder.name, "folder"),
            root_folder.id
        ));
        if staging_root.exists() {
            let _ = fs::remove_dir_all(&staging_root);
        }

        let root_export_dir =
            staging_root.join(sanitize_file_system_name(&root_folder.name, "folder"));
        fs::create_dir_all(&root_export_dir)
            .with_context(|| format!("unable to create {}", root_export_dir.display()))?;

        let mut export_paths = HashMap::new();
        export_paths.insert(root_folder_id, root_export_dir.clone());
        for folder_id in branch_ids.iter().copied().skip(1) {
            let folder = folders
                .iter()
                .find(|candidate| candidate.id == folder_id)
                .context("folder branch is missing a node")?;
            let parent_id = folder
                .parent_id
                .context("folder branch parent is missing")?;
            let parent_path = export_paths
                .get(&parent_id)
                .cloned()
                .context("folder branch export parent is missing")?;
            let folder_path = unique_directory_path(
                &parent_path,
                &sanitize_file_system_name(&folder.name, "folder"),
            );
            fs::create_dir_all(&folder_path)
                .with_context(|| format!("unable to create {}", folder_path.display()))?;
            export_paths.insert(folder_id, folder_path);
        }

        for sound in sounds.iter().filter(|sound| {
            sound
                .folder_id
                .is_some_and(|folder_id| branch_lookup.contains(&folder_id))
        }) {
            let source_path = self.drag_sound_source_path(sound)?;
            let Some(folder_id) = sound.folder_id else {
                continue;
            };
            let target_dir = export_paths
                .get(&folder_id)
                .cloned()
                .context("folder export target is missing")?;
            let extension = source_path
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("wav");
            let target_path = unique_file_path(
                &target_dir,
                &sanitize_file_system_name(&sound.name, "sound"),
                extension,
            );
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("unable to create {}", parent.display()))?;
            }
            match fs::hard_link(&source_path, &target_path) {
                Ok(()) => {}
                Err(_) => {
                    fs::copy(&source_path, &target_path).with_context(|| {
                        format!(
                            "unable to export {} to {}",
                            source_path.display(),
                            target_path.display()
                        )
                    })?;
                }
            }
        }

        Ok(root_export_dir)
    }

    pub fn export_processed_sound_from_path(
        &self,
        source_path: &Path,
        sound: &SoundEffect,
    ) -> Result<PathBuf> {
        Self::export_processed_sound_from_path_at(&self.root_dir, source_path, sound)
    }

    pub fn export_processed_sound_from_path_at(
        root_dir: &Path,
        source_path: &Path,
        sound: &SoundEffect,
    ) -> Result<PathBuf> {
        if sound.needs_processed_export() {
            let export_path = Self::processed_export_path(root_dir, sound);
            if export_path.exists() {
                return Ok(export_path);
            }
            let temp_path = export_path.with_extension("tmp.wav");
            if temp_path.exists() {
                let _ = fs::remove_file(&temp_path);
            }
            write_processed_wav(source_path, &temp_path, sound)?;
            if export_path.exists() {
                let _ = fs::remove_file(&temp_path);
                return Ok(export_path);
            }
            fs::rename(&temp_path, &export_path)
                .with_context(|| format!("unable to finalize {}", export_path.display()))?;
            return Ok(export_path);
        }

        let extension = source_path
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("wav")
            .to_owned();
        let export_name = format!(
            "{}-{}.{}",
            sanitize_stem(&sound.name),
            &sound.id.to_string()[..8],
            extension
        );
        let export_path = root_dir.join("exports").join(export_name);
        if export_path.exists() {
            return Ok(export_path);
        }
        match fs::hard_link(source_path, &export_path) {
            Ok(()) => Ok(export_path),
            Err(_) => {
                fs::copy(source_path, &export_path)
                    .with_context(|| format!("unable to export {}", export_path.display()))?;
                Ok(export_path)
            }
        }
    }

    fn processed_export_suffix(sound: &SoundEffect) -> String {
        let volume = (sound.volume.clamp(0.0, 5.0) * 1000.0).round() as u32;
        let speed = (sound.speed.clamp(0.25, 2.0) * 1000.0).round() as u32;
        let trim_start = (sound.trim_start_secs.max(0.0) * 1000.0).round() as u32;
        let trim_end = (sound.trim_end_secs.max(0.0) * 1000.0).round() as u32;
        let stem = if sound.music_only {
            "-music"
        } else if sound.vocal_only {
            "-voc"
        } else {
            ""
        };
        let reverb = if sound.reverb_enabled { "-rvb" } else { "" };
        let telephone = if sound.telephone_enabled { "-tel" } else { "" };
        let distortion = if sound.distortion_enabled { "-dst" } else { "" };
        let echo = if sound.echo_enabled { "-ech" } else { "" };
        let underwater = if sound.underwater_enabled { "-und" } else { "" };
        let robot = if sound.robot_enabled { "-rob" } else { "" };
        let pitch_shift = if sound.pitch_shift_enabled {
            format!(
                "-ps{:03}",
                (sound.pitch_shift_semitones * 10.0).round() as i32
            )
        } else {
            "".to_string()
        };
        format!(
            "-proc-v{:04}-s{:04}-a{:06}-b{:06}{}{}{}{}{}{}{}{}",
            volume,
            speed,
            trim_start,
            trim_end,
            stem,
            reverb,
            telephone,
            distortion,
            echo,
            underwater,
            robot,
            pitch_shift
        )
    }

    pub fn analyze_sound_as_effect(&self, path: &Path, name: &str) -> Result<SoundEffect> {
        let analysis = analyze_audio_file(path, WAVEFORM_BUCKETS)?;
        let asset_file = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("recording.wav")
            .to_owned();

        Ok(SoundEffect {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            asset_file,
            favorite: false,
            tags: Vec::new(),
            duration_secs: analysis.duration_secs,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 0.0,
            trim_end_secs: analysis.duration_secs,
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
            pitch_shift_enabled: false,
            pitch_shift_semitones: 0.0,
            waveform: analysis.waveform,
            folder_id: None,
        })
    }

    pub fn import_video(
        &self,
        source_path: &Path,
        name: &str,
        duration_secs: f32,
        fps: u32,
    ) -> Result<VideoAsset> {
        if !source_path.exists() {
            bail!("video file not found");
        }

        let extension = source_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("mp4");
        let id = Uuid::new_v4();
        let asset_file = format!("{id}.{extension}");
        let target_path = self.videos_dir.join(&asset_file);

        fs::copy(source_path, &target_path).context("unable to copy exported video")?;
        let waveform = analyze_audio_file(source_path, 96)
            .map(|analysis| analysis.waveform)
            .unwrap_or_else(|_| vec![0.08; 96]);

        Ok(VideoAsset {
            id,
            name: name.to_owned(),
            asset_file,
            favorite: false,
            duration_secs: duration_secs.max(0.05),
            fps: normalize_video_fps(fps),
            waveform,
        })
    }

    fn save_special_sound<F>(
        &self,
        sound: &SoundEffect,
        target_path: &Path,
        apply_name: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut PreferencesFile, String),
    {
        write_processed_wav(&sound.asset_path(&self.root_dir), target_path, sound)?;
        let mut preferences = self.load_preferences()?;
        apply_name(&mut preferences, sound.name.clone());
        self.save_preferences(&preferences)
    }

    #[allow(dead_code)]
    pub fn replace_sound_with_processed(&self, sound: &mut SoundEffect) -> Result<()> {
        let updated = Self::replace_sound_with_processed_at(&self.root_dir, sound)?;
        *sound = updated;
        Ok(())
    }

    pub fn replace_sound_with_processed_at(
        root_dir: &Path,
        sound: &SoundEffect,
    ) -> Result<SoundEffect> {
        let source_path = sound.playback_asset_path(root_dir);
        if !source_path.exists() {
            bail!("sound source file is missing");
        }

        let mut updated = sound.clone();
        let target_file = sound_asset_file_name(&updated.name, updated.id, "wav");
        let target_path = root_dir.join("sounds").join(&target_file);
        let temp_path = root_dir
            .join("sounds")
            .join(format!("{}.trimmed.tmp.wav", updated.id));

        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }

        write_processed_wav(&source_path, &temp_path, sound)?;

        if target_path.exists() {
            fs::remove_file(&target_path).context("unable to replace existing trimmed sound")?;
        }

        fs::rename(&temp_path, &target_path).context("unable to finalize trimmed sound")?;

        let preserve_source = updated
            .vocal_asset_path(root_dir)
            .is_some_and(|path| path == source_path)
            || updated
                .music_asset_path(root_dir)
                .is_some_and(|path| path == source_path);
        if source_path != target_path && source_path.exists() && !preserve_source {
            fs::remove_file(&source_path).context("unable to remove previous sound source")?;
        }

        let analysis = analyze_audio_file(&target_path, WAVEFORM_BUCKETS)?;
        updated.asset_file = target_file;
        updated.duration_secs = analysis.duration_secs;
        updated.waveform = analysis.waveform;
        updated.volume = 1.0;
        updated.speed = 1.0;
        updated.trim_start_secs = 0.0;
        updated.trim_end_secs = updated.duration_secs;
        updated.vocal_only = false;
        updated.vocal_asset_file = None;
        updated.music_only = false;
        updated.music_asset_file = None;
        updated.reverb_enabled = false;
        updated.telephone_enabled = false;
        updated.distortion_enabled = false;
        updated.echo_enabled = false;
        updated.underwater_enabled = false;
        updated.robot_enabled = false;
        updated.pitch_shift_enabled = false;
        updated.pitch_shift_semitones = 0.0;
        updated.clamp_trim();
        Ok(updated)
    }

    pub fn repair_sound_preview_asset_with_ffmpeg_at(
        root_dir: &Path,
        sound: &SoundEffect,
        failing_path: &Path,
        ffmpeg_path: &Path,
    ) -> Result<SoundEffect> {
        let mut updated = sound.clone();
        let main_path = updated.asset_path(root_dir);
        let vocal_path = updated.vocal_asset_path(root_dir);
        let music_path = updated.music_asset_path(root_dir);

        let (target_path, temp_path, repair_target): (PathBuf, PathBuf, SoundAssetRepairTarget) =
            if failing_path == main_path {
                let target_file = format!("{}.wav", updated.id);
                let target_path = root_dir.join("sounds").join(&target_file);
                let temp_path = root_dir
                    .join("sounds")
                    .join(format!("{}.repair.tmp.wav", updated.id));
                (
                    target_path,
                    temp_path,
                    SoundAssetRepairTarget::Main(target_file),
                )
            } else if vocal_path.as_deref() == Some(failing_path) {
                let target_file = Self::vocal_asset_file_name(updated.id);
                let target_path = root_dir.join("sound-vocals").join(&target_file);
                let temp_path = root_dir
                    .join("sound-vocals")
                    .join(format!("{}.repair.tmp.wav", updated.id));
                (
                    target_path,
                    temp_path,
                    SoundAssetRepairTarget::Vocal(target_file),
                )
            } else if music_path.as_deref() == Some(failing_path) {
                let target_file = Self::music_asset_file_name(updated.id);
                let target_path = root_dir.join("sound-music").join(&target_file);
                let temp_path = root_dir
                    .join("sound-music")
                    .join(format!("{}.repair.tmp.wav", updated.id));
                (
                    target_path,
                    temp_path,
                    SoundAssetRepairTarget::Music(target_file),
                )
            } else {
                bail!("preview asset is not linked to this sound");
            };

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }
        if target_path.exists() && target_path != failing_path {
            let _ = fs::remove_file(&target_path);
        }

        let args = vec![
            "-y".to_owned(),
            "-i".to_owned(),
            failing_path.to_string_lossy().into_owned(),
            "-vn".to_owned(),
            "-acodec".to_owned(),
            "pcm_s16le".to_owned(),
            "-ar".to_owned(),
            "44100".to_owned(),
            "-ac".to_owned(),
            "2".to_owned(),
            temp_path.to_string_lossy().into_owned(),
        ];
        let mut cmd = Command::new(ffmpeg_path);
        for arg in args {
            cmd.arg(arg);
        }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let output = cmd
            .output()
            .with_context(|| format!("failed to launch {}", ffmpeg_path.display()))?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }

        if target_path.exists() {
            let _ = fs::remove_file(&target_path);
        }
        fs::rename(&temp_path, &target_path)
            .with_context(|| format!("unable to finalize {}", target_path.display()))?;
        if failing_path != target_path && failing_path.exists() {
            let _ = fs::remove_file(failing_path);
        }

        let analysis = analyze_audio_file(&target_path, WAVEFORM_BUCKETS)?;
        match repair_target {
            SoundAssetRepairTarget::Main(target_file) => updated.asset_file = target_file,
            SoundAssetRepairTarget::Vocal(target_file) => {
                updated.vocal_asset_file = Some(target_file)
            }
            SoundAssetRepairTarget::Music(target_file) => {
                updated.music_asset_file = Some(target_file)
            }
        }
        updated.duration_secs = analysis.duration_secs;
        updated.waveform = analysis.waveform;
        updated.clamp_trim();
        Ok(updated)
    }

    pub fn analyze_waveform_preview(&self, path: &Path, buckets: usize) -> Result<Vec<f32>> {
        Ok(analyze_audio_file(path, buckets.max(64))?.waveform)
    }

    pub fn analyze_audio_preview(&self, path: &Path, buckets: usize) -> Result<(Vec<f32>, f32)> {
        let analysis = analyze_audio_file(path, buckets.max(64))?;
        Ok((analysis.waveform, analysis.duration_secs.max(0.05)))
    }

    pub fn audio_duration_secs(&self, path: &Path) -> Result<f32> {
        Ok(analyze_audio_file(path, 64)?.duration_secs.max(0.05))
    }
}

struct AudioAnalysis {
    duration_secs: f32,
    waveform: Vec<f32>,
}

struct DecodedAudio {
    channels: u16,
    sample_rate: u32,
    samples: Vec<f32>,
}

enum SoundAssetRepairTarget {
    Main(String),
    Vocal(String),
    Music(String),
}

fn analyze_audio_file(path: &Path, buckets: usize) -> Result<AudioAnalysis> {
    let decoded = decode_audio_file(path)?;
    let bucket_count = buckets.max(64);
    let samples_per_bucket = (decoded.samples.len() / bucket_count).max(1);
    let mut peaks = vec![0.0f32; bucket_count];
    let mut energy = vec![0.0f32; bucket_count];
    let mut counts = vec![0usize; bucket_count];
    for (sample_index, sample) in decoded.samples.iter().enumerate() {
        let bucket = (sample_index / samples_per_bucket).min(bucket_count - 1);
        peaks[bucket] = peaks[bucket].max(sample.abs());
        energy[bucket] += sample * sample;
        counts[bucket] += 1;
    }

    let decoded_duration_secs = decoded.samples.len() as f32
        / decoded.channels.max(1) as f32
        / decoded.sample_rate.max(1) as f32;

    let mut waveform = Vec::with_capacity(bucket_count);
    for bucket_index in 0..bucket_count {
        let rms = if counts[bucket_index] == 0 {
            0.0
        } else {
            (energy[bucket_index] / counts[bucket_index] as f32).sqrt()
        };
        waveform.push((peaks[bucket_index] * 0.62 + rms * 0.38).powf(1.12));
    }

    let peak_max = waveform.iter().copied().fold(0.0f32, f32::max);
    if peak_max > 0.0 {
        for value in &mut waveform {
            *value = (*value / peak_max).clamp(0.0, 1.0);
            if *value < 0.06 {
                *value *= 0.5;
            }
        }
    }

    Ok(AudioAnalysis {
        duration_secs: decoded_duration_secs,
        waveform,
    })
}

fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    let path_buf = path.to_path_buf();
    catch_unwind(AssertUnwindSafe(|| -> Result<DecodedAudio> {
        let decoder = open_decoder(&path_buf)?;
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();

        if samples.is_empty() {
            bail!("audio file is empty");
        }

        Ok(DecodedAudio {
            channels,
            sample_rate,
            samples,
        })
    }))
    .map_err(|_| anyhow::anyhow!("audio decoder crashed while reading {}", path_buf.display()))?
}

fn write_processed_wav(source_path: &Path, target_path: &Path, sound: &SoundEffect) -> Result<()> {
    let decoded = decode_audio_file(source_path)?;
    let channels = decoded.channels.max(1);
    let sample_rate = decoded.sample_rate.max(1);
    let total_frames = decoded.samples.len() / channels as usize;

    let actual_duration = total_frames as f32 / sample_rate as f32;
    let trim_start = sound.trim_start_secs.clamp(0.0, actual_duration);
    let trim_end = if sound.trim_end_secs >= sound.safe_duration() - 0.02 {
        actual_duration
    } else {
        sound.trim_end_secs.clamp(0.0, actual_duration)
    };

    let start_frame = ((trim_start * sample_rate as f32).floor() as usize).min(total_frames);
    let end_frame = if trim_end >= actual_duration - 0.02 {
        total_frames
    } else {
        ((trim_end * sample_rate as f32).ceil() as usize)
            .min(total_frames)
            .max(start_frame)
    };

    let spec = WavSpec {
        channels,
        sample_rate: ((sample_rate as f32 * sound.speed.clamp(0.25, 2.0)).round() as u32).max(1),
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer =
        WavWriter::create(target_path, spec).context("unable to create exported wav file")?;
    let volume = sound.volume.clamp(0.0, 5.0);
    let start_sample = start_frame * channels as usize;
    let end_sample = end_frame * channels as usize;
    let mut processed = decoded.samples[start_sample..end_sample].to_vec();
    apply_sound_effects(&mut processed, channels, sample_rate, sound);

    for sample in &processed {
        let scaled = (*sample * volume).clamp(-1.0, 1.0);
        let pcm = (scaled * i16::MAX as f32).round() as i16;
        writer
            .write_sample(pcm)
            .context("unable to write exported wav sample")?;
    }

    writer
        .finalize()
        .context("unable to finalize exported wav file")?;
    Ok(())
}

pub fn apply_sound_effects(
    samples: &mut [f32],
    channels: u16,
    sample_rate: u32,
    sound: &SoundEffect,
) {
    let channels = channels.max(1) as usize;
    if sound.telephone_enabled {
        apply_telephone_effect(samples, channels, sample_rate.max(1));
    }
    if sound.underwater_enabled {
        apply_underwater_effect(samples, channels, sample_rate.max(1));
    }
    if sound.reverb_enabled {
        apply_reverb_effect(samples, channels, sample_rate.max(1));
    }
    if sound.echo_enabled {
        apply_echo_effect(samples, channels, sample_rate.max(1));
    }
    if sound.distortion_enabled {
        apply_distortion_effect(samples, channels);
    }
    if sound.robot_enabled {
        apply_robot_effect(samples, channels, sample_rate.max(1));
    }
    if sound.pitch_shift_enabled {
        apply_pitch_shift_effect(
            samples,
            channels,
            sample_rate.max(1),
            sound.pitch_shift_semitones,
        );
    }
}

fn apply_distortion_effect(samples: &mut [f32], _channels: usize) {
    let gain = 3.5;
    for sample in samples.iter_mut() {
        let x = *sample * gain;
        *sample = x.tanh();
    }
}

fn apply_echo_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let delay_secs = 0.25;
    let feedback = 0.45;
    let wet = 0.5;
    let delay_samples = ((sample_rate as f32 * delay_secs).round() as usize).max(1) * channels;
    let mut delay_buffer = vec![0.0f32; delay_samples];
    let mut write_pos = 0;

    for sample_index in 0..samples.len() {
        let dry = samples[sample_index];
        let delayed = delay_buffer[write_pos];

        samples[sample_index] = dry + delayed * wet;
        delay_buffer[write_pos] = dry + delayed * feedback;

        write_pos += 1;
        if write_pos >= delay_samples {
            write_pos = 0;
        }
    }
}

fn apply_underwater_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let dt = 1.0 / sample_rate.max(1) as f32;
    let low_pass_cutoff = 350.0;
    let low_pass_rc = 1.0 / (std::f32::consts::TAU * low_pass_cutoff);
    let low_pass_alpha = dt / (low_pass_rc + dt);

    let mut lp_prev_y = vec![0.0f32; channels];

    for frame in samples.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let lp = lp_prev_y[channel] + low_pass_alpha * (x - lp_prev_y[channel]);
            lp_prev_y[channel] = lp;
            *sample = lp;
        }
    }
}

fn apply_robot_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let freq = 50.0;
    let t_step = 1.0 / sample_rate as f32;
    let comb_delay_secs = 0.005;
    let comb_delay_samples =
        ((sample_rate as f32 * comb_delay_secs).round() as usize).max(1) * channels;
    let feedback = 0.6;
    let mut comb_buffer = vec![0.0f32; comb_delay_samples];
    let mut write_pos = 0;

    for sample_index in 0..samples.len() {
        let time = (sample_index / channels) as f32 * t_step;
        let modulator = (std::f32::consts::TAU * freq * time).sin();
        let ring_mod = samples[sample_index] * (0.4 + 0.6 * modulator);

        let delayed = comb_buffer[write_pos];
        let comb_out = ring_mod + delayed * feedback;
        comb_buffer[write_pos] = comb_out;

        samples[sample_index] = comb_out.clamp(-1.0, 1.0);

        write_pos += 1;
        if write_pos >= comb_delay_samples {
            write_pos = 0;
        }
    }
}

fn apply_pitch_shift_effect(
    samples: &mut [f32],
    channels: usize,
    sample_rate: u32,
    semitones: f32,
) {
    if semitones.abs() < 0.05 {
        return;
    }
    let ratio = 2.0f32.powf(semitones / 12.0);
    let size = ((sample_rate as f32 * 0.08).round() as usize).max(256); // 80ms delay buffer
    let mut delay_buf = vec![vec![0.0f32; size]; channels];
    let mut write_pos = 0;

    let mut read_ptr_offset = 0.0f32;
    let original = samples.to_vec();

    for frame_idx in 0..(samples.len() / channels) {
        let delay_a = read_ptr_offset;
        let delay_b = (read_ptr_offset + (size as f32 / 2.0)) % size as f32;

        let w_a = if delay_a < (size as f32 / 2.0) {
            delay_a / (size as f32 / 2.0)
        } else {
            (size as f32 - delay_a) / (size as f32 / 2.0)
        };
        let w_b = 1.0 - w_a;

        for ch in 0..channels {
            let sample_val = original[frame_idx * channels + ch];
            delay_buf[ch][write_pos] = sample_val;

            let idx_a = (write_pos + size - delay_a.floor() as usize) % size;
            let idx_a_next = (idx_a + size - 1) % size;
            let frac_a = delay_a.fract();
            let val_a = delay_buf[ch][idx_a] * (1.0 - frac_a) + delay_buf[ch][idx_a_next] * frac_a;

            let idx_b = (write_pos + size - delay_b.floor() as usize) % size;
            let idx_b_next = (idx_b + size - 1) % size;
            let frac_b = delay_b.fract();
            let val_b = delay_buf[ch][idx_b] * (1.0 - frac_b) + delay_buf[ch][idx_b_next] * frac_b;

            samples[frame_idx * channels + ch] = val_a * w_a + val_b * w_b;
        }

        write_pos = (write_pos + 1) % size;
        read_ptr_offset += 1.0 - ratio;
        if read_ptr_offset < 0.0 {
            read_ptr_offset += size as f32;
        }
        read_ptr_offset %= size as f32;
    }
}

fn apply_telephone_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let dt = 1.0 / sample_rate.max(1) as f32;
    let high_pass_cutoff = 420.0;
    let low_pass_cutoff = 2_350.0;
    let high_pass_rc = 1.0 / (std::f32::consts::TAU * high_pass_cutoff);
    let low_pass_rc = 1.0 / (std::f32::consts::TAU * low_pass_cutoff);
    let high_pass_alpha = high_pass_rc / (high_pass_rc + dt);
    let low_pass_alpha = dt / (low_pass_rc + dt);

    let mut hp_prev_y = vec![0.0f32; channels];
    let mut hp_prev_x = vec![0.0f32; channels];
    let mut lp_prev_y = vec![0.0f32; channels];

    for frame in samples.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let hp = high_pass_alpha * (hp_prev_y[channel] + x - hp_prev_x[channel]);
            hp_prev_y[channel] = hp;
            hp_prev_x[channel] = x;

            let lp = lp_prev_y[channel] + low_pass_alpha * (hp - lp_prev_y[channel]);
            lp_prev_y[channel] = lp;

            *sample = (lp * 1.35).clamp(-1.0, 1.0);
        }
    }
}

fn apply_reverb_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let delay_a = ((sample_rate as f32 * 0.085).round() as usize).max(1) * channels;
    let delay_b = ((sample_rate as f32 * 0.16).round() as usize).max(1) * channels;
    let dry = 0.82f32;
    let wet_a = 0.24f32;
    let wet_b = 0.16f32;
    let feedback = 0.22f32;
    let original = samples.to_vec();

    for index in 0..samples.len() {
        let mut value = original[index] * dry;
        if index >= delay_a {
            value += original[index - delay_a] * wet_a;
        }
        if index >= delay_b {
            value += original[index - delay_b] * wet_b;
        }
        if index >= delay_b + delay_a {
            value += samples[index - delay_a] * feedback * 0.5;
        }
        samples[index] = value.clamp(-1.0, 1.0);
    }
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    Decoder::new(BufReader::new(file)).context("unsupported audio file")
}
