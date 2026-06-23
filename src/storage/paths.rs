use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::SoundEffect;

pub(super) const WAVEFORM_BUCKETS: usize = 320;
pub(super) const DEFAULT_STARTUP_SOUND_NAME: &str = "spectrum start";
#[cfg(windows)]
pub(super) const WINDOWS_STORAGE_ROOT: &str = r"D:\Data\soundfx manager";
pub(super) const DEFAULT_STARTUP_SOUND_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/default-startup.wav"
));

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
    if let (Some(cut_start), Some(cut_end)) = (sound.cut_start_secs, sound.cut_end_secs) {
        format!(
            "{} [cut {}-{}]",
            base_name,
            format_time(cut_start),
            format_time(cut_end)
        )
    } else {
        format!(
            "{} [trim {}-{}]",
            base_name,
            format_time(sound.trim_start_secs),
            format_time(sound.trim_end_secs)
        )
    }
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
