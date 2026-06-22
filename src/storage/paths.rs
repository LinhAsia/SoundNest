use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::SoundEffect;

pub(super) const WAVEFORM_BUCKETS: usize = 320;
pub(super) const DEFAULT_STARTUP_SOUND_NAME: &str = "spectrum start";
pub(super) const DEFAULT_EXIT_SOUND_NAME: &str = "spectrum end";
#[cfg(windows)]
pub(super) const WINDOWS_STORAGE_ROOT: &str = r"D:\Data\soundfx manager";
pub(super) const DEFAULT_STARTUP_SOUND_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/default-startup.wav"
));
pub(super) const DEFAULT_EXIT_SOUND_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/default-exit.wav"
));

pub(super) fn preferred_storage_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(WINDOWS_STORAGE_ROOT))
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

pub(super) fn migrate_storage_root_if_needed(target_root: &Path) -> Result<()> {
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

pub fn format_time(seconds: f32) -> String {
    let total = seconds.max(0.0);
    let mins = (total / 60.0).floor() as u32;
    let secs = (total % 60.0).floor() as u32;
    let millis = ((total.fract()) * 100.0).round() as u32;
    format!("{mins:02}:{secs:02}.{millis:02}")
}

pub(super) fn committed_trimmed_sound_name(sound: &SoundEffect) -> String {
    let base_name = sound.name.trim();
    let base_name = if base_name.is_empty() {
        "sound"
    } else {
        base_name
    };
    format!(
        "{} [trim {}-{}]",
        base_name,
        format_time(sound.trim_start_secs),
        format_time(sound.trim_end_secs)
    )
}

pub(super) fn sound_asset_file_name(name: &str, sound_id: Uuid, extension: &str) -> String {
    let base_name = sanitize_file_system_name(name, "sound").replace(' ', "_");
    let extension = extension.trim_start_matches('.');
    format!("{base_name}-{sound_id}.{extension}")
}

pub(super) fn sanitize_stem(name: &str) -> String {
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

pub(super) fn sanitize_file_system_name(name: &str, fallback: &str) -> String {
    let cleaned = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            _ => ch,
        })
        .collect::<String>();
    let trimmed = cleaned
        .trim()
        .trim_end_matches(['.', ' '])
        .trim_matches('_');
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub(super) fn unique_directory_path(parent: &Path, base_name: &str) -> PathBuf {
    let mut candidate = parent.join(base_name);
    let mut index = 2usize;
    while candidate.exists() {
        candidate = parent.join(format!("{base_name} ({index})"));
        index += 1;
    }
    candidate
}

pub(super) fn unique_file_path(parent: &Path, base_name: &str, extension: &str) -> PathBuf {
    let extension = extension.trim_start_matches('.');
    let mut candidate = parent.join(format!("{base_name}.{extension}"));
    let mut index = 2usize;
    while candidate.exists() {
        candidate = parent.join(format!("{base_name} ({index}).{extension}"));
        index += 1;
    }
    candidate
}

pub(super) fn default_speed() -> f32 {
    1.0
}

pub(super) fn default_video_fps() -> u32 {
    20
}

pub(super) fn default_pitch_semitones() -> f32 {
    0.0
}

pub(super) fn normalize_video_fps(fps: u32) -> u32 {
    match fps {
        60 => 60,
        144 => 144,
        _ => default_video_fps(),
    }
}
