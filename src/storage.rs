use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use hound::{SampleFormat, WavSpec, WavWriter};
use rodio::{Decoder, Source};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const WAVEFORM_BUCKETS: usize = 320;
const DEFAULT_STARTUP_SOUND_NAME: &str = "spectrum start";
const DEFAULT_EXIT_SOUND_NAME: &str = "spectrum end";
#[cfg(windows)]
const WINDOWS_STORAGE_ROOT: &str = r"D:\Data\soundfx manager";
const DEFAULT_STARTUP_SOUND_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/default-startup.wav"
));
const DEFAULT_EXIT_SOUND_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/default-exit.wav"
));

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoundEffect {
    pub id: Uuid,
    pub name: String,
    pub asset_file: String,
    #[serde(default)]
    pub favorite: bool,
    pub duration_secs: f32,
    pub volume: f32,
    #[serde(default = "default_speed")]
    pub speed: f32,
    pub trim_start_secs: f32,
    pub trim_end_secs: f32,
    #[serde(default)]
    pub waveform: Vec<f32>,
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
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct LibraryFile {
    sounds: Vec<SoundEffect>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct VideoLibraryFile {
    videos: Vec<VideoAsset>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PreferencesFile {
    import_dir: Option<PathBuf>,
    pitch_update_hz: Option<f32>,
    overlay_animation: Option<bool>,
    app_transition_animation: Option<bool>,
    pitch_show_sharps: Option<bool>,
    library_grid_scale: Option<f32>,
    dark_theme: Option<bool>,
    record_hotkey: Option<String>,
    pitch_hotkey: Option<String>,
    startup_sound_name: Option<String>,
    exit_sound_name: Option<String>,
    startup_sound_cleared: Option<bool>,
    exit_sound_cleared: Option<bool>,
    gemini_api_key: Option<String>,
}

pub struct Storage {
    root_dir: PathBuf,
    sounds_dir: PathBuf,
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
        let videos_dir = root_dir.join("videos");
        let settings_sounds_dir = root_dir.join("settings-sounds");
        let bundled_sounds_dir = root_dir.join("bundled-sounds");
        let exports_dir = root_dir.join("exports");
        fs::create_dir_all(&sounds_dir).context("unable to create sounds directory")?;
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

        library
            .sounds
            .retain(|sound| sound.asset_path(&self.root_dir).exists());

        for sound in &mut library.sounds {
            sound.clamp_trim();
        }

        Ok(library.sounds)
    }

    pub fn save_library(&self, sounds: &[SoundEffect]) -> Result<()> {
        let payload = LibraryFile {
            sounds: sounds.to_vec(),
        };
        let json = serde_json::to_string_pretty(&payload).context("unable to serialize library")?;
        fs::write(&self.library_path, json).context("unable to write library file")?;
        Ok(())
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
            if video.waveform.is_empty()
                && let Ok(analysis) = analyze_audio_file(&video.asset_path(&self.root_dir), 96)
            {
                video.waveform = analysis.waveform;
                changed = true;
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

    pub fn load_library_grid_scale(&self) -> Result<Option<f32>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .library_grid_scale
            .map(|value| value.clamp(0.72, 1.1)))
    }

    pub fn save_library_grid_scale(&self, scale: f32) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.library_grid_scale = Some(scale.clamp(0.72, 1.1));
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

    pub fn load_record_hotkey(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.record_hotkey)
    }

    pub fn save_record_hotkey(&self, hotkey: Option<&str>) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.record_hotkey = hotkey.map(str::to_owned);
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_hotkey(&self) -> Result<Option<String>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.pitch_hotkey)
    }

    pub fn save_pitch_hotkey(&self, hotkey: Option<&str>) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_hotkey = hotkey.map(str::to_owned);
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
        if !source_path.exists() {
            bail!("file not found");
        }

        let extension = source_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let id = Uuid::new_v4();
        let asset_file = format!("{id}.{extension}");
        let target_path = self.sounds_dir.join(&asset_file);

        fs::copy(source_path, &target_path).context("unable to copy imported sound")?;

        let analysis = match analyze_audio_file(&target_path, WAVEFORM_BUCKETS) {
            Ok(analysis) => analysis,
            Err(error) => {
                let _ = fs::remove_file(&target_path);
                return Err(error);
            }
        };

        let name = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Sound")
            .to_owned();

        Ok(SoundEffect {
            id,
            name,
            asset_file,
            favorite: false,
            duration_secs: analysis.duration_secs,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 0.0,
            trim_end_secs: analysis.duration_secs,
            waveform: analysis.waveform,
        })
    }

    pub fn hydrate_sound(&self, sound: &mut SoundEffect) -> Result<bool> {
        if !sound.waveform.is_empty() && sound.duration_secs > 0.0 {
            sound.clamp_trim();
            return Ok(false);
        }

        let analysis = analyze_audio_file(&sound.asset_path(&self.root_dir), WAVEFORM_BUCKETS)?;
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
        self.export_processed_sound_from_path(&sound.asset_path(&self.root_dir), sound)
    }

    pub fn drag_sound_source_path(&self, sound: &SoundEffect) -> Result<PathBuf> {
        if sound.needs_processed_export() {
            self.export_processed_sound(sound)
        } else {
            Ok(sound.asset_path(&self.root_dir))
        }
    }

    pub fn export_processed_sound_from_path(
        &self,
        source_path: &Path,
        sound: &SoundEffect,
    ) -> Result<PathBuf> {
        let extension = if sound.needs_processed_export() {
            "wav".to_owned()
        } else {
            source_path
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("wav")
                .to_owned()
        };
        let export_name = format!(
            "{}-{}.{}",
            sanitize_stem(&sound.name),
            &sound.id.to_string()[..8],
            extension
        );
        let export_path = self.exports_dir.join(export_name);
        if !sound.needs_processed_export() {
            if export_path.exists() {
                return Ok(export_path);
            }
            match fs::hard_link(source_path, &export_path) {
                Ok(()) => return Ok(export_path),
                Err(_) => {
                    fs::copy(source_path, &export_path)
                        .with_context(|| format!("unable to export {}", export_path.display()))?;
                    return Ok(export_path);
                }
            }
        }
        write_processed_wav(source_path, &export_path, sound)?;
        Ok(export_path)
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
            duration_secs: analysis.duration_secs,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 0.0,
            trim_end_secs: analysis.duration_secs,
            waveform: analysis.waveform,
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

    pub fn replace_sound_with_processed(&self, sound: &mut SoundEffect) -> Result<()> {
        let source_path = sound.asset_path(&self.root_dir);
        if !source_path.exists() {
            bail!("sound source file is missing");
        }

        let target_file = format!("{}.wav", sound.id);
        let target_path = self.sounds_dir.join(&target_file);
        let temp_path = self
            .sounds_dir
            .join(format!("{}.trimmed.tmp.wav", sound.id));

        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }

        write_processed_wav(&source_path, &temp_path, sound)?;

        if target_path != source_path && target_path.exists() {
            fs::remove_file(&target_path).context("unable to replace existing trimmed sound")?;
        }

        fs::rename(&temp_path, &target_path).context("unable to finalize trimmed sound")?;

        if source_path != target_path && source_path.exists() {
            fs::remove_file(&source_path).context("unable to remove previous sound source")?;
        }

        let analysis = analyze_audio_file(&target_path, WAVEFORM_BUCKETS)?;
        sound.asset_file = target_file;
        sound.duration_secs = analysis.duration_secs;
        sound.waveform = analysis.waveform;
        sound.volume = 1.0;
        sound.speed = 1.0;
        sound.trim_start_secs = 0.0;
        sound.trim_end_secs = sound.duration_secs;
        sound.clamp_trim();
        Ok(())
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

fn preferred_storage_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        return Ok(PathBuf::from(WINDOWS_STORAGE_ROOT));
    }

    #[cfg(not(windows))]
    {
        legacy_storage_root()?.context("unable to resolve app data directory")
    }
}

fn legacy_storage_root() -> Result<Option<PathBuf>> {
    Ok(ProjectDirs::from("dev", "codex", "soundfx_manager")
        .map(|dirs| dirs.data_local_dir().to_path_buf()))
}

fn migrate_storage_root_if_needed(target_root: &Path) -> Result<()> {
    let Some(legacy_root) = legacy_storage_root()? else {
        return Ok(());
    };

    if legacy_root == target_root || !legacy_root.exists() {
        return Ok(());
    }

    fs::create_dir_all(target_root)
        .with_context(|| format!("unable to create {}", target_root.display()))?;
    merge_directory_contents(&legacy_root, target_root)?;
    remove_dir_if_empty(&legacy_root)?;
    Ok(())
}

fn merge_directory_contents(source_dir: &Path, target_dir: &Path) -> Result<()> {
    if !source_dir.exists() {
        return Ok(());
    }

    fs::create_dir_all(target_dir)
        .with_context(|| format!("unable to create {}", target_dir.display()))?;

    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("unable to read {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| format!("unable to read {}", source_dir.display()))?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        merge_path(&source_path, &target_path)?;
    }

    Ok(())
}

fn merge_path(source_path: &Path, target_path: &Path) -> Result<()> {
    if !target_path.exists() {
        move_path(source_path, target_path)?;
        return Ok(());
    }

    let source_meta = fs::metadata(source_path)
        .with_context(|| format!("unable to read {}", source_path.display()))?;
    let target_meta = fs::metadata(target_path)
        .with_context(|| format!("unable to read {}", target_path.display()))?;

    if source_meta.is_dir() && target_meta.is_dir() {
        merge_directory_contents(source_path, target_path)?;
        remove_dir_if_empty(source_path)?;
        return Ok(());
    }

    if source_meta.is_file() && target_meta.is_file() {
        return Ok(());
    }

    bail!(
        "unable to merge {} into {}",
        source_path.display(),
        target_path.display()
    )
}

fn move_path(source_path: &Path, target_path: &Path) -> Result<()> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }

    match fs::rename(source_path, target_path) {
        Ok(()) => Ok(()),
        Err(_) => copy_then_remove(source_path, target_path),
    }
}

fn copy_then_remove(source_path: &Path, target_path: &Path) -> Result<()> {
    let metadata = fs::metadata(source_path)
        .with_context(|| format!("unable to read {}", source_path.display()))?;

    if metadata.is_dir() {
        copy_directory_recursive(source_path, target_path)?;
        fs::remove_dir_all(source_path)
            .with_context(|| format!("unable to remove {}", source_path.display()))?;
    } else {
        fs::copy(source_path, target_path).with_context(|| {
            format!(
                "unable to copy {} to {}",
                source_path.display(),
                target_path.display()
            )
        })?;
        fs::remove_file(source_path)
            .with_context(|| format!("unable to remove {}", source_path.display()))?;
    }

    Ok(())
}

fn copy_directory_recursive(source_dir: &Path, target_dir: &Path) -> Result<()> {
    fs::create_dir_all(target_dir)
        .with_context(|| format!("unable to create {}", target_dir.display()))?;

    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("unable to read {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| format!("unable to read {}", source_dir.display()))?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        let metadata = fs::metadata(&source_path)
            .with_context(|| format!("unable to read {}", source_path.display()))?;
        if metadata.is_dir() {
            copy_directory_recursive(&source_path, &target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("unable to create {}", parent.display()))?;
            }
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "unable to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn remove_dir_if_empty(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }

    let mut entries =
        fs::read_dir(path).with_context(|| format!("unable to read {}", path.display()))?;
    if entries.next().is_none() {
        fs::remove_dir(path).with_context(|| format!("unable to remove {}", path.display()))?;
    }
    Ok(())
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

fn analyze_audio_file(path: &Path, buckets: usize) -> Result<AudioAnalysis> {
    let decoded = decode_audio_file(path)?;
    let bucket_count = buckets.max(64);
    let samples_per_bucket = (decoded.samples.len() / bucket_count).max(1);
    let mut peaks = vec![0.0f32; bucket_count];
    for (sample_index, sample) in decoded.samples.iter().enumerate() {
        let bucket = (sample_index / samples_per_bucket).min(bucket_count - 1);
        peaks[bucket] = peaks[bucket].max(sample.abs());
    }

    let decoded_duration_secs = decoded.samples.len() as f32
        / decoded.channels.max(1) as f32
        / decoded.sample_rate.max(1) as f32;

    let peak_max = peaks.iter().copied().fold(0.0f32, f32::max);
    if peak_max > 0.0 {
        for peak in &mut peaks {
            *peak /= peak_max;
        }
    }

    Ok(AudioAnalysis {
        duration_secs: decoded_duration_secs,
        waveform: peaks,
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

    for sample in &decoded.samples[start_sample..end_sample] {
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

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    Decoder::new(BufReader::new(file)).context("unsupported audio file")
}

pub fn format_time(seconds: f32) -> String {
    let total = seconds.max(0.0);
    let mins = (total / 60.0).floor() as u32;
    let secs = (total % 60.0).floor() as u32;
    let millis = ((total.fract()) * 100.0).round() as u32;
    format!("{mins:02}:{secs:02}.{millis:02}")
}

fn sanitize_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '_',
        })
        .collect();

    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "sound".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn default_speed() -> f32 {
    1.0
}

fn default_video_fps() -> u32 {
    20
}

fn normalize_video_fps(fps: u32) -> u32 {
    match fps {
        60 => 60,
        144 => 144,
        _ => default_video_fps(),
    }
}
