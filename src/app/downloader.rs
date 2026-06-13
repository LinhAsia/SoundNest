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
    pub(super) fn start_tts_generation(&mut self) {
        if self.tts_running {
            return;
        }
        let api_key = self.gemini_api_key.trim().to_owned();
        if api_key.is_empty() {
            self.tts_error = Some(self.t("download.gemini_api_key_missing"));
            return;
        }
        let text = self.tts_text.trim().to_owned();
        if text.is_empty() {
            self.tts_error = Some(self.t("download.text_empty"));
            return;
        }
        let voice = self.tts_voice_name.trim().to_owned();
        let direction_prompt = self.tts_direction_prompt.trim().to_owned();
        let output_name = self.tts_output_name.trim().to_owned();
        let out_dir = self.storage.root_dir().join("gemini-tts");
        let tx = self.tts_tx.clone();
        self.tts_running = true;
        self.tts_status = "Generating".to_owned();
        self.tts_error = None;
        self.tts_last_file = None;
        self.tts_can_add_to_library = false;
        self.tts_added_to_library = false;
        thread::spawn(move || {
            let result = gemini_tts::generate_speech_to_file(
                &api_key,
                &text,
                &voice,
                &direction_prompt,
                &out_dir,
                if output_name.is_empty() {
                    "gemini tts"
                } else {
                    &output_name
                },
            )
            .map(|path| GeminiTtsResult {
                path,
                display_name: if output_name.is_empty() {
                    "gemini tts".to_owned()
                } else {
                    output_name
                },
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(GeminiTtsMessage::Finished(result));
        });
    }

    pub(super) fn selected_tts_preset_name(&self) -> Option<&str> {
        self.tts_prompt_presets
            .iter()
            .find(|preset| preset.prompt == self.tts_direction_prompt)
            .map(|preset| preset.name.as_str())
    }

    pub(super) fn gemini_voice_label(name: &str) -> &str {
        GEMINI_VOICE_OPTIONS
            .iter()
            .find(|voice| voice.name == name)
            .map(|voice| voice.label)
            .unwrap_or("Custom voice")
    }

    pub(super) fn save_current_tts_preset(&mut self) {
        let name = self.tts_preset_name.trim();
        let prompt = self.tts_direction_prompt.trim();
        if name.is_empty() || prompt.is_empty() {
            self.tts_error = Some("Preset name and prompt are required".to_owned());
            return;
        }

        if let Some(existing) = self
            .tts_prompt_presets
            .iter_mut()
            .find(|preset| preset.name.eq_ignore_ascii_case(name))
        {
            existing.name = name.to_owned();
            existing.prompt = prompt.to_owned();
        } else {
            self.tts_prompt_presets.push(GeminiTtsPromptPreset {
                name: name.to_owned(),
                prompt: prompt.to_owned(),
            });
            self.tts_prompt_presets.sort_by(|left, right| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            });
        }
        self.tts_error = None;
        let _ = self
            .storage
            .save_tts_prompt_presets(&self.tts_prompt_presets);
        self.tts_status = "Preset saved".to_owned();
    }

    pub(super) fn apply_tts_preset_by_name(&mut self, name: &str) {
        if let Some(preset) = self
            .tts_prompt_presets
            .iter()
            .find(|preset| preset.name == name)
            .cloned()
        {
            self.tts_preset_name = preset.name;
            self.tts_direction_prompt = preset.prompt;
            self.tts_error = None;
        }
    }

    pub(super) fn delete_selected_tts_preset(&mut self) {
        let Some(selected_name) = self.selected_tts_preset_name().map(str::to_owned) else {
            return;
        };
        self.tts_prompt_presets
            .retain(|preset| !preset.name.eq_ignore_ascii_case(&selected_name));
        let _ = self
            .storage
            .save_tts_prompt_presets(&self.tts_prompt_presets);
        self.tts_preset_name.clear();
        self.tts_status = "Preset removed".to_owned();
    }

    pub(super) fn add_tts_result_to_library(&mut self, path: &Path) {
        match self.storage.import_sound(path) {
            Ok(mut sound) => {
                let preferred_name = self.tts_output_name.trim();
                if !preferred_name.is_empty() {
                    sound.name = preferred_name.to_owned();
                }
                sound.folder_id = None;
                self.app_view = AppView::Library;
                self.library_tab = LibraryTab::Sounds;
                self.library_current_folder = None;
                self.folder_import_select_mode = None;
                self.selected = Some(sound.id);
                self.library_audio_query.clear();
                self.library_audio_tag_filter = None;
                self.library_favorites_only_audio = false;
                self.sounds.insert(0, sound);
                self.save_now();
                self.tts_can_add_to_library = false;
                self.tts_added_to_library = true;
                self.status = Some("Added to library".to_owned());
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn existing_myinstants_path(
        files: &mut HashMap<String, PathBuf>,
        audio_url: &str,
    ) -> Option<PathBuf> {
        let path = files.get(audio_url).cloned()?;
        if path.exists() {
            Some(path)
        } else {
            files.remove(audio_url);
            None
        }
    }

    pub(super) fn existing_myinstants_download_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_cached_files, audio_url)
    }

    pub(super) fn existing_myinstants_preview_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_preview_files, audio_url)
    }

    pub(super) fn existing_myinstants_playback_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        self.existing_myinstants_download_path(audio_url)
            .or_else(|| self.existing_myinstants_preview_path(audio_url))
    }

    pub(super) fn toggle_myinstants_preview(&mut self, result: &MyinstantsResult) -> Result<()> {
        if let Some(path) = self.existing_myinstants_playback_path(&result.audio_url)
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing_file(&path))
        {
            self.stop_preview();
            return Ok(());
        }

        let path = if let Some(path) = self.existing_myinstants_download_path(&result.audio_url) {
            path
        } else if let Some(path) = self.existing_myinstants_preview_path(&result.audio_url) {
            path
        } else {
            let path = self.myinstants.ensure_preview_file(result)?;
            self.myinstants_preview_files
                .insert(result.audio_url.clone(), path.clone());
            path
        };
        self.myinstants_preview_audio_url = Some(result.audio_url.clone());
        if !self.myinstants_waveforms.contains_key(&result.audio_url) {
            let waveform = self.storage.analyze_waveform_preview(&path, 96)?;
            self.myinstants_waveforms
                .insert(result.audio_url.clone(), waveform);
        }
        self.preview_file_path(&path)
    }

    pub(super) fn queue_myinstants_waveform_prefetch(&mut self, result: &MyinstantsResult) {
        if self.myinstants_waveforms.contains_key(&result.audio_url)
            || self.myinstants_waveform_jobs.contains(&result.audio_url)
        {
            return;
        }

        self.myinstants_waveform_jobs
            .insert(result.audio_url.clone());
        let result = result.clone();
        let client = self.myinstants.clone();
        let tx = self.myinstants_waveform_tx.clone();
        thread::spawn(move || {
            let message = match client.ensure_preview_file(&result) {
                Ok(path) => match Storage::new()
                    .and_then(|storage| storage.analyze_waveform_preview(&path, 96))
                {
                    Ok(waveform) => MyinstantsWaveformMessage::Ready {
                        audio_url: result.audio_url.clone(),
                        path,
                        waveform,
                    },
                    Err(error) => MyinstantsWaveformMessage::Failed {
                        audio_url: result.audio_url.clone(),
                        error: error.to_string(),
                    },
                },
                Err(error) => MyinstantsWaveformMessage::Failed {
                    audio_url: result.audio_url.clone(),
                    error: error.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    pub(super) fn download_site_url(kind: DownloadSiteKind) -> &'static str {
        match kind {
            DownloadSiteKind::Youtube => "https://www.youtube.com/",
            DownloadSiteKind::SoundCloud => "https://soundcloud.com/",
            DownloadSiteKind::Bandcamp => "https://bandcamp.com/",
            DownloadSiteKind::TikTok => "https://www.tiktok.com/",
            DownloadSiteKind::Facebook => "https://www.facebook.com/",
            DownloadSiteKind::Instagram => "https://www.instagram.com/",
            DownloadSiteKind::X => "https://x.com/",
            DownloadSiteKind::Vimeo => "https://vimeo.com/",
            DownloadSiteKind::Twitch => "https://www.twitch.tv/",
            DownloadSiteKind::GoogleDrive => "https://drive.google.com/",
        }
    }

    pub(super) fn youtube_search_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
        let desired = vec2(152.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::TRANSPARENT
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        let badge = DownloadSiteBadge {
            name: "YouTube",
            kind: DownloadSiteKind::Youtube,
            color: Color32::from_rgb(255, 77, 141),
        };
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            vec2(18.0, 18.0),
        );
        Self::paint_download_site_icon(ui.painter(), icon_rect, badge);
        ui.painter().text(
            Pos2::new(rect.left() + 34.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn search_sound_button(ui: &mut Ui, enabled: bool) -> egui::Response {
        let desired = vec2(36.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            char::from_u32(0xe8b6).unwrap_or(' '),
            egui::FontId::new(16.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn format_compact_count(value: u64) -> String {
        if value >= 1_000_000_000 {
            format!("{:.1}B views", value as f64 / 1_000_000_000.0)
        } else if value >= 1_000_000 {
            format!("{:.1}M views", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.1}K views", value as f64 / 1_000.0)
        } else {
            format!("{value} views")
        }
    }

    pub(super) fn paint_download_site_icon(
        painter: &egui::Painter,
        rect: Rect,
        badge: DownloadSiteBadge,
    ) {
        let center = rect.center();
        let white = Color32::WHITE;
        let radius = rect.width().min(rect.height()) * 0.5;
        let s = radius / 12.0;

        // Except for Google Drive which has a white background circle drawn inside the match,
        // all other badges draw their background circle using badge.color.
        if badge.kind != DownloadSiteKind::GoogleDrive {
            painter.circle_filled(center, radius, badge.color);
        }

        match badge.kind {
            DownloadSiteKind::Youtube => {
                // YouTube: Red/pink backing circle, white rounded rect, inner red/pink play triangle.
                let body = Rect::from_center_size(center, vec2(14.0 * s, 9.8 * s));
                painter.rect_filled(body, 3.0 * s, white);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-2.0 * s, -2.8 * s),
                        center + vec2(-2.0 * s, 2.8 * s),
                        center + vec2(3.0 * s, 0.0),
                    ],
                    badge.color,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::SoundCloud => {
                // SoundCloud: Orange backing circle, 6 vertical soundwave bars, overlapping cloud circles & base.
                // Draw left-side bars
                let bar_heights = [3.5, 5.0, 6.5, 8.0, 9.0, 9.5];
                for (index, height) in bar_heights.into_iter().enumerate() {
                    let dx = -8.0 + (index as f32) * 1.6;
                    let bar = Rect::from_min_max(
                        center + vec2((dx - 0.5) * s, -height * s + 4.5 * s),
                        center + vec2((dx + 0.5) * s, 4.5 * s),
                    );
                    painter.rect_filled(bar, 0.5 * s, white);
                }
                // Draw cloud body circles
                painter.circle_filled(center + vec2(2.5 * s, 0.5 * s), 4.0 * s, white);
                painter.circle_filled(center + vec2(6.0 * s, 1.5 * s), 3.0 * s, white);
                // Draw flat connector base
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(0.0 * s, 0.5 * s),
                        center + vec2(9.0 * s, 4.5 * s),
                    ),
                    0.0,
                    white,
                );
            }
            DownloadSiteKind::Bandcamp => {
                // Bandcamp: Blue backing circle, white slanted parallelogram.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-1.5 * s, -4.5 * s),
                        center + vec2(7.5 * s, -4.5 * s),
                        center + vec2(1.5 * s, 4.5 * s),
                        center + vec2(-7.5 * s, 4.5 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::TikTok => {
                // TikTok: Dark backing circle, music note with cyan & red/magenta offset fringes.
                let paint_note = |painter: &egui::Painter, offset: Vec2, color: Color32| {
                    let n_center = center + offset;
                    // Note head (filled circle at bottom-left)
                    painter.circle_filled(n_center + vec2(-2.0 * s, 3.0 * s), 2.8 * s, color);

                    // Stem (vertical line)
                    painter.line_segment(
                        [
                            n_center + vec2(0.8 * s, 3.0 * s),
                            n_center + vec2(0.8 * s, -4.0 * s),
                        ],
                        Stroke::new(2.0 * s, color),
                    );

                    // Hook (quarter circle arc from PI to 1.5 PI, centered at 4.8, -4.0)
                    let mut hook_pts = Vec::new();
                    for i in 0..=8 {
                        let theta = std::f32::consts::PI * (1.0 + (i as f32) / 16.0);
                        let pt = n_center
                            + vec2(
                                (4.8 + 4.0 * theta.cos()) * s,
                                (-4.0 + 4.0 * theta.sin()) * s,
                            );
                        hook_pts.push(pt);
                    }
                    painter.add(egui::Shape::line(hook_pts, Stroke::new(2.0 * s, color)));
                };

                // Offset passes for chromatic aberration
                paint_note(
                    painter,
                    vec2(-0.8 * s, -0.5 * s),
                    Color32::from_rgb(0, 242, 234),
                ); // Cyan
                paint_note(
                    painter,
                    vec2(0.8 * s, 0.5 * s),
                    Color32::from_rgb(254, 44, 85),
                ); // Red/Magenta
                paint_note(painter, vec2(0.0, 0.0), white); // White
            }
            DownloadSiteKind::Facebook => {
                // Facebook: Blue backing circle, custom vector Facebook "f" logo.
                let mut stem_pts = vec![
                    center + vec2(1.5 * s, 8.0 * s),
                    center + vec2(1.5 * s, -3.0 * s),
                ];
                for i in 1..=6 {
                    let theta = std::f32::consts::PI * (1.0 + (i as f32) / 12.0);
                    stem_pts.push(
                        center
                            + vec2(
                                (4.5 + 3.0 * theta.cos()) * s,
                                (-3.0 + 3.0 * theta.sin()) * s,
                            ),
                    );
                }
                // Draw stem and hook
                painter.add(egui::Shape::line(stem_pts, Stroke::new(3.2 * s, white)));
                // Draw crossbar
                painter.line_segment(
                    [
                        center + vec2(-2.0 * s, -1.0 * s),
                        center + vec2(4.5 * s, -1.0 * s),
                    ],
                    Stroke::new(3.2 * s, white),
                );
            }
            DownloadSiteKind::Instagram => {
                // Instagram: Purple/pink backing circle, camera body outline, inner lens, and flash dot.
                let body = Rect::from_center_size(center, vec2(13.0 * s, 13.0 * s));
                painter.rect_stroke(
                    body,
                    4.0 * s,
                    Stroke::new(1.8 * s, white),
                    StrokeKind::Outside,
                );
                painter.circle_stroke(center, 3.3 * s, Stroke::new(1.8 * s, white));
                painter.circle_filled(center + vec2(3.8 * s, -3.8 * s), 1.0 * s, white);
            }
            DownloadSiteKind::X => {
                // X: Black backing circle, custom double-struck X layout using solid polygon and parallel line strokes.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(3.0 * s, -5.0 * s),
                        center + vec2(5.5 * s, -5.0 * s),
                        center + vec2(-3.0 * s, 5.0 * s),
                        center + vec2(-5.5 * s, 5.0 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));
                painter.line_segment(
                    [
                        center + vec2(-5.5 * s, -5.0 * s),
                        center + vec2(4.5 * s, 5.0 * s),
                    ],
                    Stroke::new(1.2 * s, white),
                );
                painter.line_segment(
                    [
                        center + vec2(-3.0 * s, -5.0 * s),
                        center + vec2(7.0 * s, 5.0 * s),
                    ],
                    Stroke::new(1.2 * s, white),
                );
            }
            DownloadSiteKind::Vimeo => {
                // Vimeo: Blue backing circle, custom curved "v".
                painter.add(egui::Shape::line(
                    vec![
                        center + vec2(-5.5 * s, -2.5 * s),
                        center + vec2(-3.0 * s, 3.5 * s),
                        center + vec2(-1.0 * s, 4.0 * s),
                        center + vec2(1.0 * s, 0.5 * s),
                        center + vec2(5.0 * s, -4.5 * s),
                    ],
                    Stroke::new(2.6 * s, white),
                ));
            }
            DownloadSiteKind::Twitch => {
                // Twitch: Purple backing circle, chat bubble (rounded rect, left bottom block, and beak) with eye slots.
                let body = Rect::from_min_max(
                    center + vec2(-6.0 * s, -6.0 * s),
                    center + vec2(6.0 * s, 2.0 * s),
                );
                painter.rect_filled(body, 1.0 * s, white);

                let left_ext = Rect::from_min_max(
                    center + vec2(-6.0 * s, 2.0 * s),
                    center + vec2(-2.0 * s, 4.0 * s),
                );
                painter.rect_filled(left_ext, 0.0, white);

                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-2.0 * s, 2.0 * s),
                        center + vec2(-2.0 * s, 5.5 * s),
                        center + vec2(1.5 * s, 2.0 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));

                // Draw purple eye slots
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(-2.5 * s, -2.5 * s),
                        center + vec2(-1.0 * s, 1.0 * s),
                    ),
                    0.5 * s,
                    badge.color,
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(1.0 * s, -2.5 * s),
                        center + vec2(2.5 * s, 1.0 * s),
                    ),
                    0.5 * s,
                    badge.color,
                );
            }
            DownloadSiteKind::GoogleDrive => {
                // Google Drive: White backing circle, interlocking trapezoid bands (green, blue, yellow).
                painter.circle_filled(center, radius, Color32::from_rgb(252, 252, 252));

                let outer_top = center + vec2(0.0 * s, -6.7 * s);
                let outer_bottom_left = center + vec2(-6.0 * s, 3.3 * s);
                let outer_bottom_right = center + vec2(6.0 * s, 3.3 * s);

                let inner_top = center + vec2(-1.0 * s, -4.7 * s);
                let inner_bottom_left = center + vec2(-1.8 * s, 1.3 * s);
                let inner_bottom_right = center + vec2(2.8 * s, 1.3 * s);

                // Green band (bottom)
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        outer_bottom_left,
                        outer_bottom_right,
                        inner_bottom_right,
                        inner_bottom_left,
                    ],
                    Color32::from_rgb(15, 157, 88),
                    Stroke::NONE,
                ));

                // Blue band (right)
                painter.add(egui::Shape::convex_polygon(
                    vec![outer_bottom_right, outer_top, inner_top, inner_bottom_right],
                    Color32::from_rgb(66, 133, 244),
                    Stroke::NONE,
                ));

                // Yellow band (left)
                painter.add(egui::Shape::convex_polygon(
                    vec![outer_top, outer_bottom_left, inner_bottom_left, inner_top],
                    Color32::from_rgb(251, 188, 5),
                    Stroke::NONE,
                ));
            }
        }
    }

    pub(super) fn poll_tts_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.tts_rx.try_recv() {
            self.tts_running = false;
            match message {
                GeminiTtsMessage::Finished(Ok(result)) => {
                    self.tts_status = "Done".to_owned();
                    self.tts_last_file = Some(result.path);
                    if self.tts_output_name.trim().is_empty() {
                        self.tts_output_name = result.display_name;
                    }
                    self.tts_error = None;
                    self.tts_can_add_to_library = true;
                }
                GeminiTtsMessage::Finished(Err(error)) => {
                    self.tts_status = "Error".to_owned();
                    self.tts_error = Some(error);
                    self.tts_last_file = None;
                    self.tts_can_add_to_library = false;
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn render_download_site_badges(ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.label(
                RichText::new("Supporting web:")
                    .size(12.5)
                    .color(Self::muted_text_color()),
            );
            for badge in Self::download_site_badges() {
                let fill = if Self::dark_theme_enabled() {
                    Color32::from_rgb(29, 25, 35)
                } else {
                    Color32::from_rgb(255, 251, 254)
                };
                let stroke = if Self::dark_theme_enabled() {
                    Color32::from_rgb(82, 67, 90)
                } else {
                    Color32::from_rgb(230, 221, 229)
                };
                let response = Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, stroke))
                    .corner_radius(14.0)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                        let painter = ui.painter_at(icon_rect);
                        Self::paint_download_site_icon(&painter, icon_rect, badge);
                    })
                    .response
                    .on_hover_text(badge.name);
                Self::decorate_button_response(ui, &response);
                if response.clicked() {
                    let _ = open::that(Self::download_site_url(badge.kind));
                }
            }
        });
    }

    pub(super) fn render_youtube_result_row(
        ui: &mut Ui,
        result: &YoutubeSearchResult,
        download_label: &str,
    ) -> bool {
        let mut download_clicked = false;
        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(22.0)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let badge = DownloadSiteBadge {
                        name: "YouTube",
                        kind: DownloadSiteKind::Youtube,
                        color: Color32::from_rgb(255, 77, 141),
                    };
                    let (icon_rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    Self::paint_download_site_icon(ui.painter(), icon_rect, badge);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add_sized(
                            [ui.available_width().min(380.0), 18.0],
                            egui::Label::new(
                                RichText::new(&result.title)
                                    .size(13.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .truncate(),
                        );
                        let mut parts = Vec::new();
                        if let Some(duration) = result.duration {
                            parts.push(format_time(duration as f32));
                        }
                        if let Some(uploader) = &result.uploader
                            && !uploader.trim().is_empty()
                        {
                            parts.push(Self::truncate_middle_ascii(uploader, 28));
                        }
                        if let Some(view_count) = result.view_count {
                            parts.push(Self::format_compact_count(view_count));
                        }
                        if !result.id.trim().is_empty() {
                            parts.push(format!("ID {}", result.id));
                        }
                        if parts.is_empty() {
                            parts.push("YouTube".to_owned());
                        }
                        ui.label(
                            RichText::new(parts.join("  •  "))
                                .size(12.0)
                                .color(Self::muted_text_color()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let response = ui.add_sized(
                            [104.0, 34.0],
                            Self::action_button(
                                RichText::new(download_label)
                                    .size(13.0)
                                    .color(Color32::WHITE),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            download_clicked = true;
                        }
                    });
                });
            });
        download_clicked
    }
}
