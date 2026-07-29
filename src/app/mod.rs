use crate::audio::{AudioEngine, calculate_normalization_gain};
use crate::downloader::{YoutubeAudioDownloader, YoutubeSearchResult};
use crate::gemini_tts;
use crate::hotkey::{GlobalHotkeyManager, Hotkey};
use crate::localization::Localization;
use crate::myinstants::{MyinstantsClient, MyinstantsResult};
use crate::pitch::{
    PitchInputSource, PitchMonitor, PitchMonitorConfig, PitchSnapshot, analyze_pitch_file,
    list_capture_devices,
};
use crate::platform;
use crate::record_video;
use crate::recorder::{Recorder, RecorderConfig};
use crate::storage::{
    Folder, GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset, format_time,
};
use crate::stream_input::{StreamInputConfig, StreamInputRouter};
use anyhow::{Context as _, Result};
#[cfg(windows)]
use clipboard_win::{Clipboard, Setter, formats::FileList};
use eframe::egui::{
    self, Align, Align2, Button, CentralPanel, Checkbox, Color32, ComboBox, Context, CornerRadius,
    DragValue, FontFamily, FontId, Frame, Margin, Pos2, ProgressBar, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextEdit, TextureHandle, Ui, Vec2, ViewportCommand, vec2,
};
use eframe::epaint::Shadow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;
const AUDIO_FILTERS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];
const APP_FRAME_RADIUS: f32 = 30.0;
const APP_OUTER_MARGIN: f32 = 0.0;
const LIVE_UI_FADE_SEC: f32 = 0.12;
const TRANSITION_POINT_COUNT: usize = 240;
const MATERIAL_ICONS_FONT: &str = "material_icons";
const ACTIVE_UI_REPAINT_MS: u64 = 8;
const JOB_POLL_REPAINT_MS: u64 = 90;
const DEFAULT_INTRO_DURATION_SEC: f32 = 0.10;
const TRANSITION_WAVE_BUCKETS: usize = 160;
const LIBRARY_GRID_MIN_COLUMNS: usize = 3;
const LIBRARY_GRID_MAX_COLUMNS: usize = 8;
const LIBRARY_ROW_MIN_THICKNESS: usize = 1;
const LIBRARY_ROW_MAX_THICKNESS: usize = 5;
const RECORD_EXPORT_VIDEO_FPS_OPTIONS: [u32; 3] = [
    record_video::LOW_VIDEO_FPS,
    record_video::STANDARD_VIDEO_FPS,
    record_video::HIGH_VIDEO_FPS,
];
static DARK_THEME_ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) use message::*;
pub(crate) use state::*;
pub(crate) use util::*;

impl SoundFxApp {
    fn library_list_row_height(&self) -> f32 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 108.0,
            2 => 116.0,
            3 => 124.0,
            4 => 134.0,
            _ => 144.0,
        }
    }

    fn library_row_wave_height(&self) -> f32 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 22.0,
            2 => 26.0,
            3 => 30.0,
            4 => 36.0,
            _ => 42.0,
        }
    }

    fn library_row_vertical_padding(&self) -> i8 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 4,
            2 => 6,
            3 => 8,
            4 => 10,
            _ => 12,
        }
    }
}

impl SoundFxApp {}

mod audio_workflow;
mod background_jobs;
mod controls;
mod core_helpers;
mod downloader;
mod editor;
mod general_helpers;
mod importing;
mod layout;
mod library;
mod library_filtering;
mod library_folder_views;
mod library_helpers;
mod library_import_panel;
mod library_jobs;
mod library_row_helpers;
mod media_panels;
mod message;
mod pitch_monitor;
mod pitch_overlay_view;
mod recording;
mod settings;
mod state;
mod transition_state;
mod transition_view;
mod ui_helpers;
mod update;
mod util;
mod view;
mod waveform;
mod window_helpers;
