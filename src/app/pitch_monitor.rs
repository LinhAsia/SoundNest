use super::*;
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
use crate::storage::{GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset, format_time};
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

impl SoundFxApp {
pub(super) fn trigger_record_hotkey_action(&mut self, ctx: &Context) {
        if self.recorder.snapshot().running {
            self.reveal_record_review_on_open = true;
            self.stop_recording(Some(ctx));
            if self.show_record_review_panel {
                Self::reveal_window(ctx);
                self.reveal_record_review_on_open = false;
            }
        } else {
            self.start_recording(ctx, false);
        }
    }

pub(super) fn trigger_pitch_hotkey_action(&mut self, ctx: &Context) {
        let snapshot = self.pitch_monitor.snapshot();
        if snapshot.running {
            self.pitch_monitor.stop();
            self.pitch_overlay_native_visuals_applied = false;
            self.overlay_only_mode = false;
            self.center_window_next_frame = true;
            Self::restore_main_viewport(ctx);
            self.clear_status();
            return;
        }

        if self.pitch_input_source == PitchInputSource::Microphone
            && self.selected_pitch_input_device.is_none()
        {
            self.set_error_status(self.t("pitch.no_microphone_input"));
            return;
        }

        match self.pitch_monitor.start(PitchMonitorConfig {
            source: self.pitch_input_source,
            input_device_name: if self.pitch_input_source == PitchInputSource::Microphone {
                self.selected_pitch_input_device.clone()
            } else {
                None
            },
            updates_per_second: self.pitch_update_hz,
        }) {
            Ok(()) => {
                self.center_pitch_overlay_next_frame = true;
                self.pitch_overlay_pos = None;
                self.pitch_overlay_native_visuals_applied = false;
                self.show_pitch_panel = false;
                let overlay_size = if self.pitch_overlay_animation {
                    vec2(276.0, 276.0)
                } else {
                    vec2(430.0, 104.0)
                };
                Self::apply_overlay_only_viewport(ctx, overlay_size);
                self.overlay_only_mode = true;
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

pub(super) fn handle_space_preview(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if self.video_viewer.is_some() {
            if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
                return;
            }
            self.toggle_video_viewer_playback();
            return;
        }

        if self.show_record_review_panel {
            if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
                return;
            }
            let Some(sound) = self
                .recording_draft
                .as_ref()
                .map(|draft| draft.sound.clone())
            else {
                return;
            };
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(sound.id))
            {
                self.stop_preview();
                return;
            }
            let mut cursor_secs = self.preview_cursor_secs_for(&sound);
            if cursor_secs >= sound.trim_end_secs - 0.02 {
                cursor_secs = sound.trim_start_secs;
                self.set_preview_cursor_secs(sound.id, cursor_secs, sound.safe_duration());
            }
            self.preview_recording_draft_from_position(Some(cursor_secs));
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            return;
        }

        if self.has_modal_panel() {
            return;
        }

        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound = self.sounds[index].clone();
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound.id))
        {
            self.stop_preview();
            return;
        }
        let mut cursor_secs = self.preview_cursor_secs_for(&sound);
        if cursor_secs >= sound.trim_end_secs - 0.02 {
            cursor_secs = sound.trim_start_secs;
            self.set_preview_cursor_secs(sound.id, cursor_secs, sound.safe_duration());
        }
        self.preview_sound_from_position(sound.id, Some(cursor_secs));
    }

pub(super) fn handle_record_hotkey(&mut self, ctx: &Context) {
        self.record_hotkey_manager.set_repaint_context(ctx.clone());

        if self.capture_record_hotkey || self.capture_pitch_hotkey {
            let mut add_hotkey: Option<Hotkey> = None;
            let mut cancel = false;
            
            fn is_modifier_key(key: egui::Key) -> bool {
                let name = format!("{:?}", key);
                matches!(
                    name.to_lowercase().as_str(),
                    "control" | "ctrl" | "alt" | "shift" | "command" | "maccmd" | "meta" | "win"
                )
            }

            ctx.input(|input| {
                for event in &input.events {
                    match event {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            modifiers,
                            ..
                        } => {
                            if *key == egui::Key::Escape {
                                cancel = true;
                            } else if !is_modifier_key(*key) {
                                let hotkey = Hotkey {
                                    ctrl: modifiers.ctrl || modifiers.command,
                                    alt: modifiers.alt,
                                    shift: modifiers.shift,
                                    win: modifiers.mac_cmd,
                                    key: *key,
                                };
                                if self.capture_record_hotkey {
                                    self.preview_record_hotkey = Some(hotkey);
                                }
                                if self.capture_pitch_hotkey {
                                    self.preview_pitch_hotkey = Some(hotkey);
                                }
                            }
                        }
                        egui::Event::Key {
                            key,
                            pressed: false,
                            ..
                        } => {
                            if *key == egui::Key::Escape {
                                cancel = true;
                            } else {
                                if self.capture_record_hotkey && self.preview_record_hotkey.map(|hk| hk.key) == Some(*key) {
                                    add_hotkey = self.preview_record_hotkey;
                                }
                                if self.capture_pitch_hotkey && self.preview_pitch_hotkey.map(|hk| hk.key) == Some(*key) {
                                    add_hotkey = self.preview_pitch_hotkey;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });

            if cancel {
                self.capture_record_hotkey = false;
                self.capture_pitch_hotkey = false;
                self.preview_record_hotkey = None;
                self.preview_pitch_hotkey = None;
                return;
            }

            if let Some(hotkey) = add_hotkey {
                if self.capture_record_hotkey {
                    if !self.record_hotkeys.contains(&hotkey) {
                        self.record_hotkeys.push(hotkey);
                        let names: Vec<String> = self.record_hotkeys.iter().map(|k| k.to_string()).collect();
                        let _ = self.storage.save_record_hotkeys(&names);
                        if let Err(error) = self.record_hotkey_manager.set_hotkeys(&self.record_hotkeys) {
                            self.set_error_status(error);
                        }
                    }
                    self.capture_record_hotkey = false;
                    self.preview_record_hotkey = None;
                }
                if self.capture_pitch_hotkey {
                    if !self.pitch_hotkeys.contains(&hotkey) {
                        self.pitch_hotkeys.push(hotkey);
                        let names: Vec<String> = self.pitch_hotkeys.iter().map(|k| k.to_string()).collect();
                        let _ = self.storage.save_pitch_hotkeys(&names);
                        if let Err(error) = self.record_hotkey_manager.set_secondary_hotkeys(&self.pitch_hotkeys) {
                            self.set_error_status(error);
                        }
                    }
                    self.capture_pitch_hotkey = false;
                    self.preview_pitch_hotkey = None;
                }
            }
            return;
        }

        if self.is_transition_active() {
            return;
        }
        let app_focused = ctx.input(|input| input.focused);
        let local_record_trigger = app_focused
            && self.record_hotkeys.iter().any(|hotkey| {
                ctx.input(|input| {
                    input.events.iter().any(|event| {
                        if let egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            modifiers,
                            ..
                        } = event {
                            *key == hotkey.key
                                && (modifiers.ctrl || modifiers.command) == hotkey.ctrl
                                && modifiers.alt == hotkey.alt
                                && modifiers.shift == hotkey.shift
                                && modifiers.mac_cmd == hotkey.win
                        } else {
                            false
                        }
                    })
                })
            });
        let local_pitch_trigger = app_focused
            && self.pitch_hotkeys.iter().any(|hotkey| {
                ctx.input(|input| {
                    input.events.iter().any(|event| {
                        if let egui::Event::Key {
                            key,
                            pressed: true,
                            repeat: false,
                            modifiers,
                            ..
                        } = event {
                            *key == hotkey.key
                                && (modifiers.ctrl || modifiers.command) == hotkey.ctrl
                                && modifiers.alt == hotkey.alt
                                && modifiers.shift == hotkey.shift
                                && modifiers.mac_cmd == hotkey.win
                        } else {
                            false
                        }
                    })
                })
            });
        if let Some(error) = self.record_hotkey_manager.take_error() {
            self.set_error_status(error);
        }
        let global_record_trigger = self.record_hotkey_manager.take_triggered();
        let global_pitch_trigger = self.record_hotkey_manager.take_secondary_triggered();
        if local_record_trigger || global_record_trigger {
            self.trigger_record_hotkey_action(ctx);
        }
        if local_pitch_trigger || global_pitch_trigger {
            self.trigger_pitch_hotkey_action(ctx);
        }
    }
}
