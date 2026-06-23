mod cache;
mod json_store;
mod migration;
mod models;
mod paths;

pub use self::cache::apply_sound_effects;
pub use self::models::{
    Folder, GeminiTtsDraftPreferences, GeminiTtsPromptPreset, SoundEffect, VideoAsset,
};
pub use self::paths::format_time;

use self::migration::{migrate_storage_root_if_needed, preferred_storage_root};
use self::models::PreferencesFile;
use self::paths::{
    WAVEFORM_BUCKETS, committed_trimmed_sound_name, normalize_video_fps, sanitize_file_system_name,
    sanitize_stem, sound_asset_file_name, unique_directory_path, unique_file_path,
};
use anyhow::{Context, Result, bail};
use cache::{analyze_audio_file, write_processed_wav};
use std::collections::HashMap;
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
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
            cut_start_secs: None,
            cut_end_secs: None,
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
        let cut_start = sound
            .cut_start_secs
            .map(|value| (value.max(0.0) * 1000.0).round() as u32)
            .unwrap_or(0);
        let cut_end = sound
            .cut_end_secs
            .map(|value| (value.max(0.0) * 1000.0).round() as u32)
            .unwrap_or(0);
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
            "-proc-v{:04}-s{:04}-a{:06}-b{:06}-c{:06}-d{:06}{}{}{}{}{}{}{}{}",
            volume,
            speed,
            trim_start,
            trim_end,
            cut_start,
            cut_end,
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
            cut_start_secs: None,
            cut_end_secs: None,
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
        updated.cut_start_secs = None;
        updated.cut_end_secs = None;
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

enum SoundAssetRepairTarget {
    Main(String),
    Vocal(String),
    Music(String),
}
