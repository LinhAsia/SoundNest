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
    pub(super) fn available_import_roots() -> Vec<PathBuf> {
        ["C:\\", "D:\\"]
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .collect()
    }

    pub(super) fn set_import_dir(&mut self, path: Option<PathBuf>) {
        self.import_dir = path.unwrap_or_default();
        let _ = self.storage.save_import_dir(
            (!self.import_dir.as_os_str().is_empty()).then_some(self.import_dir.as_path()),
        );
        self.refresh_import_audio_entries();
    }

    pub(super) fn refresh_pitch_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.pitch_capture_devices = devices;
                if !self
                    .selected_pitch_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.pitch_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_pitch_input_device = self.pitch_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn refresh_record_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.record_capture_devices = devices;
                if !self
                    .selected_record_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.record_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_record_input_device =
                        self.record_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn refresh_stream_input_capture_devices(&mut self) {
        match list_capture_devices() {
            Ok(devices) => {
                self.stream_input_capture_devices = devices;
                if !self
                    .selected_stream_input_device
                    .as_ref()
                    .is_some_and(|selected| {
                        self.stream_input_capture_devices
                            .iter()
                            .any(|name| name == selected)
                    })
                {
                    self.selected_stream_input_device =
                        self.stream_input_capture_devices.first().cloned();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn start_demucs_install(&mut self, ctx: &Context) {
        if self.demucs_installing {
            return;
        }
        self.demucs_installing = true;
        self.demucs_install_error = None;
        let tx = self.demucs_install_tx.clone();
        thread::spawn(move || {
            let result = crate::vocal_separation::install_demucs();
            let _ = tx.send(result);
        });
        ctx.request_repaint();
    }

    pub(super) fn start_demucs_model_preload(&mut self, ctx: &Context) {
        if self.demucs_model_loading || !crate::vocal_separation::is_demucs_available() {
            return;
        }
        self.demucs_model_loading = true;
        self.demucs_model_error = None;
        self.demucs_model_ready = false;
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.demucs_model_cancel = Some(Arc::clone(&cancel_flag));
        let tx = self.demucs_model_tx.clone();
        let root_dir = self.storage.root_dir().to_path_buf();
        thread::spawn(move || {
            let message = match crate::vocal_separation::preload_demucs_model_cancellable(
                &root_dir,
                cancel_flag,
            ) {
                Ok(true) => DemucsModelMessage::Finished(Ok(())),
                Ok(false) => DemucsModelMessage::Cancelled,
                Err(error) => DemucsModelMessage::Finished(Err(error)),
            };
            let _ = tx.send(message);
        });
        ctx.request_repaint();
    }

    pub(super) fn stop_demucs_model_work(&mut self) {
        if self.demucs_model_loading {
            if let Some(cancel_flag) = self.demucs_model_cancel.as_ref() {
                cancel_flag.store(true, Ordering::Relaxed);
            }
            self.status = Some("Stopping demucs model preparation".to_owned());
            return;
        }

        if self.demucs_model_ready {
            match crate::vocal_separation::clear_demucs_model_ready() {
                Ok(()) => {
                    self.demucs_model_ready = false;
                    self.demucs_model_error = None;
                    self.status = Some("demucs model stopped".to_owned());
                }
                Err(error) => self.set_error_status(error),
            }
        }
    }

    pub(super) fn uninstall_demucs_from_settings(&mut self) {
        match crate::vocal_separation::uninstall_demucs() {
            Ok(()) => {
                self.demucs_install_error = None;
                self.demucs_model_error = None;
                self.demucs_model_loading = false;
                self.demucs_model_ready = false;
                self.demucs_model_cancel = None;
                if let Some(draft) = self.recording_draft.as_mut() {
                    draft.keep_vocal = false;
                    draft.vocal_separated_path = None;
                    draft.keep_music = false;
                    draft.music_separated_path = None;
                }
                self.status = Some("demucs-rs uninstalled".to_owned());
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn begin_async_stream_driver_probe(&mut self) {
        self.stream_driver_checked = false;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = Ok(crate::stream_driver::is_stream_driver_installed());
            let _ = tx.send(StreamDriverMessage::ProbeFinished(result));
        });
    }

    pub(super) fn start_stream_driver_install(&mut self, ctx: &Context) {
        if self.stream_driver_busy || self.stream_driver_installed {
            return;
        }
        self.stream_driver_busy = true;
        self.stream_driver_error = None;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = crate::stream_driver::install_stream_driver()
                .map(|_| crate::stream_driver::is_stream_driver_installed());
            let _ = tx.send(StreamDriverMessage::Finished(result));
        });
        ctx.request_repaint();
    }

    pub(super) fn start_stream_driver_uninstall(&mut self, ctx: &Context) {
        if self.stream_driver_busy {
            return;
        }
        let _ = self.stream_input_router.configure(None);
        self.stream_driver_busy = true;
        self.stream_driver_error = None;
        let tx = self.stream_driver_tx.clone();
        thread::spawn(move || {
            let result = crate::stream_driver::uninstall_stream_driver().map(|_| false);
            let _ = tx.send(StreamDriverMessage::Finished(result));
        });
        ctx.request_repaint();
    }

    pub(super) fn apply_stream_input_routing(&mut self) {
        let mic_available = self.selected_stream_input_device.is_some()
            || !self.stream_input_capture_devices.is_empty();
        let monitor_microphone = self.stream_input_monitor_microphone && mic_available;
        let route_system_audio = self.stream_input_system_audio && !monitor_microphone;
        let route_microphone = self.stream_input_microphone && mic_available;
        let can_route_virtual_mic =
            self.stream_driver_installed && (route_system_audio || route_microphone);
        let wants_local_mic_monitor = monitor_microphone;
        let config =
            if !self.stream_driver_busy && (can_route_virtual_mic || wants_local_mic_monitor) {
                Some(StreamInputConfig {
                    route_system_audio: self.stream_driver_installed && route_system_audio,
                    route_microphone: self.stream_driver_installed && route_microphone,
                    monitor_microphone,
                    microphone_device_name: self.selected_stream_input_device.clone(),
                })
            } else {
                None
            };

        if let Err(error) = self.stream_input_router.configure(config) {
            self.set_error_status(error);
        }
    }

    pub(super) fn gemini_api_key_field(
        ui: &mut Ui,
        label: &str,
        api_key: &mut String,
        visible: &mut bool,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label)
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let toggle = Self::icon_titlebar(
                    ui,
                    [34.0, 28.0],
                    if *visible { 0xe8f5 } else { 0xe8f4 },
                    *visible,
                    false,
                )
                .on_hover_text(if *visible {
                    "Hide API key"
                } else {
                    "Show API key"
                });
                if toggle.clicked() {
                    *visible = !*visible;
                }
            });
        });
        ui.add_space(8.0);
        let response = ui.add(
            TextEdit::singleline(api_key)
                .desired_width(f32::INFINITY)
                .hint_text("AIza...")
                .password(!*visible),
        );
        if response.changed() {
            changed = true;
        }
        changed
    }

    pub(super) fn poll_demucs_install_result(&mut self, ctx: &Context) {
        while let Ok(result) = self.demucs_install_rx.try_recv() {
            self.demucs_installing = false;
            match result {
                Ok(()) => {
                    self.demucs_install_error = None;
                    self.demucs_model_ready = crate::vocal_separation::is_demucs_model_ready();
                    self.status = Some("demucs-rs installed".to_owned());
                }
                Err(e) => {
                    self.demucs_install_error = Some(e);
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_demucs_model_result(&mut self, ctx: &Context) {
        while let Ok(message) = self.demucs_model_rx.try_recv() {
            self.demucs_model_loading = false;
            self.demucs_model_cancel = None;
            match message {
                DemucsModelMessage::Finished(Ok(())) => {
                    self.demucs_model_error = None;
                    self.demucs_model_ready = true;
                    self.status = Some("demucs model ready".to_owned());
                }
                DemucsModelMessage::Finished(Err(error)) => {
                    self.demucs_model_error = Some(error);
                    self.demucs_model_ready = crate::vocal_separation::is_demucs_model_ready();
                }
                DemucsModelMessage::Cancelled => {
                    self.demucs_model_error = None;
                    self.demucs_model_ready = crate::vocal_separation::is_demucs_model_ready();
                    self.status = Some("demucs model preparation stopped".to_owned());
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_stream_driver_result(&mut self, ctx: &Context) {
        while let Ok(message) = self.stream_driver_rx.try_recv() {
            match message {
                StreamDriverMessage::ProbeFinished(Ok(installed)) => {
                    self.stream_driver_checked = true;
                    self.stream_driver_installed = installed;
                    self.stream_driver_error = None;
                }
                StreamDriverMessage::ProbeFinished(Err(error)) => {
                    self.stream_driver_checked = true;
                    self.stream_driver_error = Some(error);
                }
                StreamDriverMessage::Finished(Ok(installed)) => {
                    self.stream_driver_busy = false;
                    self.stream_driver_checked = true;
                    self.stream_driver_installed = installed;
                    self.stream_driver_error = None;
                    self.status = Some(if installed {
                        "stream driver installed".to_owned()
                    } else {
                        "stream driver removed".to_owned()
                    });
                }
                StreamDriverMessage::Finished(Err(error)) => {
                    self.stream_driver_busy = false;
                    self.stream_driver_checked = true;
                    self.stream_driver_error = Some(error);
                    self.begin_async_stream_driver_probe();
                }
            }
            self.apply_stream_input_routing();
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_stream_input_router(&mut self, ctx: &Context) {
        let snapshot = self.stream_input_router.snapshot();
        if snapshot.running || snapshot.error.is_some() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if let Some(error) = snapshot.error {
            self.set_error_status(error);
        }
    }

    pub(super) fn apply_theme(ctx: &Context, dark_theme: bool) {
        Self::set_theme_enabled(dark_theme);

        let mut style = (*ctx.style()).clone();
        style.visuals = if dark_theme {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        style.visuals.panel_fill = if dark_theme {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.window_fill = if dark_theme {
            Color32::from_rgb(20, 17, 24)
        } else {
            Color32::from_rgb(248, 247, 251)
        };
        style.visuals.extreme_bg_color = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.faint_bg_color = if dark_theme {
            Color32::from_rgb(33, 28, 39)
        } else {
            Color32::from_rgb(244, 239, 246)
        };
        style.visuals.widgets.noninteractive.bg_fill = if dark_theme {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.noninteractive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(71, 57, 78)
        } else {
            Color32::from_rgb(229, 220, 228)
        };
        style.visuals.widgets.noninteractive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(236, 233, 240)
        } else {
            Color32::from_rgb(233, 226, 234)
        };
        style.visuals.widgets.inactive.bg_fill = if dark_theme {
            Color32::from_rgb(28, 24, 33)
        } else {
            Color32::from_rgb(255, 255, 255)
        };
        style.visuals.widgets.inactive.bg_stroke.color = if dark_theme {
            Color32::from_rgb(82, 67, 90)
        } else {
            Color32::from_rgb(226, 216, 225)
        };
        style.visuals.widgets.inactive.bg_stroke.width = 1.2;
        style.visuals.widgets.inactive.fg_stroke.color = if dark_theme {
            Color32::from_rgb(236, 230, 239)
        } else {
            Color32::from_rgb(76, 58, 85)
        };
        style.visuals.widgets.inactive.weak_bg_fill = if dark_theme {
            Color32::from_rgb(245, 242, 248)
        } else {
            Color32::from_rgb(226, 218, 228)
        };
        style.visuals.widgets.hovered.bg_fill = if dark_theme {
            Color32::from_rgb(53, 36, 52)
        } else {
            Color32::from_rgb(255, 236, 246)
        };
        style.visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
        style.visuals.widgets.hovered.bg_stroke.width = 1.2;
        style.visuals.widgets.hovered.fg_stroke.color = Color32::from_rgb(255, 252, 255);
        style.visuals.widgets.hovered.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 248, 252)
        } else {
            Color32::from_rgb(240, 229, 239)
        };
        style.visuals.widgets.active.bg_fill = if dark_theme {
            Color32::from_rgb(78, 30, 68)
        } else {
            Color32::from_rgb(255, 223, 239)
        };
        style.visuals.widgets.active.bg_stroke.color = Color32::from_rgb(214, 51, 132);
        style.visuals.widgets.active.bg_stroke.width = 1.2;
        style.visuals.widgets.active.fg_stroke.color = Color32::from_rgb(255, 252, 255);
        style.visuals.widgets.active.weak_bg_fill = if dark_theme {
            Color32::from_rgb(255, 251, 253)
        } else {
            Color32::from_rgb(245, 233, 243)
        };
        style.visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.visuals.hyperlink_color = Color32::from_rgb(214, 51, 132);
        style.visuals.window_shadow.color = if dark_theme {
            Color32::from_rgba_premultiplied(0, 0, 0, 96)
        } else {
            Color32::from_rgba_premultiplied(68, 27, 56, 48)
        };
        style.interaction.selectable_labels = false;
        style.interaction.show_tooltips_only_when_still = false;
        style.interaction.tooltip_delay = 0.0;
        style.interaction.tooltip_grace_time = 0.8;
        ctx.set_style(style);
    }

    pub(super) fn page_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(17, 14, 20)
        } else {
            Color32::from_rgb(248, 248, 248)
        }
    }

    pub(super) fn surface_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(24, 20, 29)
        } else {
            Color32::WHITE
        }
    }

    pub(super) fn panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(29, 25, 35)
        } else {
            Color32::from_rgb(252, 248, 251)
        }
    }

    pub(super) fn border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(78, 64, 87)
        } else {
            Color32::from_rgb(229, 220, 228)
        }
    }

    pub(super) fn subtle_border_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(88, 72, 96)
        } else {
            Color32::from_rgb(237, 226, 234)
        }
    }

    pub(super) fn strong_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(246, 233, 241)
        } else {
            Color32::from_rgb(40, 35, 41)
        }
    }

    pub(super) fn muted_text_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(191, 174, 189)
        } else {
            Color32::from_rgb(118, 106, 116)
        }
    }

    pub(super) fn t(&self, key: &str) -> String {
        self.localization.text(key)
    }

    pub(super) fn render_settings_panel(&mut self, ctx: &Context) {
        if !self.show_settings_panel {
            return;
        }

        let mut open_panel = self.show_settings_panel;
        let mut close_request = false;
        let mut save_startup = false;
        let mut save_exit = false;
        let mut clear_startup = false;
        let mut clear_exit = false;
        let mut reset_startup = false;
        let mut reset_exit = false;
        let mut animation_changed = false;
        let mut install_demucs = false;
        let mut preload_demucs = false;
        let mut stop_demucs = false;
        let mut uninstall_demucs = false;
        let mut install_stream_driver = false;
        let mut uninstall_stream_driver = false;
        let available_languages = self.localization.available_languages();
        let mut selected_language = self.localization.current_code().to_owned();
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(520.0, 700.0), vec2(320.0, 360.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("settings-panel"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_size(panel_size)
            .fixed_pos(panel_pos)
            .open(&mut open_panel)
            .frame(
                Frame::new()
                    .fill(Self::overlay_panel_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 32,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe8b8, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.t("settings.language"))
                            .size(12.5)
                            .color(Self::muted_text_color()),
                    );
                    Self::with_dark_combo_visuals(ui, |ui| {
                        ComboBox::from_id_salt("settings-language")
                            .width(140.0)
                            .selected_text(
                                RichText::new(self.localization.current_name())
                                    .color(Self::strong_text_color()),
                            )
                            .show_ui(ui, |ui| {
                                for (code, name) in &available_languages {
                                    ui.selectable_value(&mut selected_language, code.clone(), name);
                                }
                            });
                    });
                });
                ui.add_space(12.0);
                let startup_toggle = ui.add_sized(
                    [ui.available_width(), 34.0],
                    Self::action_button(
                        RichText::new(self.t("settings.startup_sound")).size(13.0),
                        self.settings_show_startup_sound,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &startup_toggle);
                if startup_toggle.clicked() {
                    self.settings_show_startup_sound = !self.settings_show_startup_sound;
                }
                if self.settings_show_startup_sound {
                    ui.add_space(10.0);
                    Self::draw_settings_sound_row(
                        ui,
                        &self.sounds,
                        &self.t("settings.startup_sound"),
                        &mut self.settings_startup_candidate,
                        &self.startup_sound_name,
                        "settings-startup-combo",
                        &mut save_startup,
                        &mut clear_startup,
                        &mut reset_startup,
                    );
                }

                ui.add_space(12.0);
                let exit_toggle = ui.add_sized(
                    [ui.available_width(), 34.0],
                    Self::action_button(
                        RichText::new(self.t("settings.exit_sound")).size(13.0),
                        self.settings_show_exit_sound,
                        false,
                    ),
                );
                Self::decorate_button_response(ui, &exit_toggle);
                if exit_toggle.clicked() {
                    self.settings_show_exit_sound = !self.settings_show_exit_sound;
                }
                if self.settings_show_exit_sound {
                    ui.add_space(10.0);
                    Self::draw_settings_sound_row(
                        ui,
                        &self.sounds,
                        &self.t("settings.exit_sound"),
                        &mut self.settings_exit_candidate,
                        &self.exit_sound_name,
                        "settings-exit-combo",
                        &mut save_exit,
                        &mut clear_exit,
                        &mut reset_exit,
                    );
                }

                ui.add_space(14.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        let animation_label = self.t("settings.animation");
                        let response = ui.checkbox(
                            &mut self.app_transition_animation,
                            RichText::new(animation_label)
                                .size(13.0)
                                .color(Self::strong_text_color()),
                        );
                        if response.changed() {
                            animation_changed = true;
                        }
                    });

                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.t("settings.keep_vocal_model"))
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                let status = if self.demucs_model_loading {
                                    self.t("settings.preparing")
                                } else if self.demucs_model_ready {
                                    self.t("settings.ready")
                                } else if crate::vocal_separation::is_demucs_available() {
                                    self.t("settings.installed")
                                } else {
                                    self.t("settings.not_installed")
                                };
                                ui.label(
                                    RichText::new(status)
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(self.t("settings.keep_vocal_model_description"))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let install = ui.add_enabled(
                                !self.demucs_installing
                                    && !crate::vocal_separation::is_demucs_available(),
                                Self::action_button(
                                    RichText::new(self.t("settings.install_demucs")).size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &install);
                            if install.clicked() {
                                install_demucs = true;
                            }

                            let preload = ui.add_enabled(
                                !self.demucs_model_loading
                                    && crate::vocal_separation::is_demucs_available(),
                                Self::action_button(
                                    RichText::new(if self.demucs_model_ready {
                                        self.t("settings.reload_model")
                                    } else {
                                        self.t("settings.prepare_model")
                                    })
                                    .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &preload);
                            if preload.clicked() {
                                preload_demucs = true;
                            }

                            let stop = ui.add_enabled(
                                self.demucs_model_loading || self.demucs_model_ready,
                                Self::action_button(
                                    RichText::new(if self.demucs_model_loading {
                                        self.t("settings.stop_preparing")
                                    } else {
                                        self.t("settings.stop_model")
                                    })
                                    .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &stop);
                            if stop.clicked() {
                                stop_demucs = true;
                            }

                            let uninstall = ui.add_enabled(
                                !self.demucs_installing
                                    && !self.demucs_model_loading
                                    && crate::vocal_separation::is_demucs_available(),
                                Self::action_button(
                                    RichText::new(self.t("settings.uninstall_demucs")).size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &uninstall);
                            if uninstall.clicked() {
                                uninstall_demucs = true;
                            }
                        });
                        if self.demucs_installing || self.demucs_model_loading {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    RichText::new(if self.demucs_installing {
                                        "Installing demucs-rs..."
                                    } else {
                                        "Preparing vocal model..."
                                    })
                                    .size(11.5)
                                    .color(Self::muted_text_color()),
                                );
                            });
                        }
                        if let Some(error) = self
                            .demucs_model_error
                            .as_ref()
                            .or(self.demucs_install_error.as_ref())
                        {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(error)
                                    .size(11.5)
                                    .color(Color32::from_rgb(171, 54, 91)),
                            );
                        }
                    });

                ui.add_space(12.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(self.t("settings.stream_mic_driver"))
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(if self.stream_driver_busy {
                                        self.t("settings.preparing")
                                    } else if !self.stream_driver_checked {
                                        self.t("stream.checking")
                                    } else if self.stream_driver_installed {
                                        self.t("settings.installed")
                                    } else {
                                        self.t("settings.not_installed")
                                    })
                                    .size(12.0)
                                    .color(Self::muted_text_color()),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(self.t("settings.stream_driver_description"))
                                .size(11.5)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let install = ui.add_enabled(
                                !self.stream_driver_busy && !self.stream_driver_installed,
                                Self::action_button(
                                    RichText::new(self.t("settings.install_stream_driver"))
                                        .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &install);
                            if install.clicked() {
                                install_stream_driver = true;
                            }

                            let uninstall = ui.add_enabled(
                                !self.stream_driver_busy,
                                Self::action_button(
                                    RichText::new(self.t("settings.remove_stream_driver"))
                                        .size(12.0),
                                    false,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &uninstall);
                            if uninstall.clicked() {
                                uninstall_stream_driver = true;
                            }
                        });
                        if self.stream_driver_busy {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    RichText::new(self.t("settings.updating_stream_driver"))
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        }
                        if let Some(error) = self.stream_driver_error.as_ref() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(error)
                                    .size(11.5)
                                    .color(Color32::from_rgb(171, 54, 91)),
                            );
                        }
                    });
            });

        self.show_settings_panel = open_panel;
        if close_request {
            self.show_settings_panel = false;
        }
        if animation_changed {
            let _ = self
                .storage
                .save_app_transition_animation(self.app_transition_animation);
        }
        if selected_language != self.localization.current_code() {
            self.localization.set_current_code(&selected_language);
            let _ = self
                .storage
                .save_language_code(self.localization.current_code());
        }
        if install_demucs {
            self.start_demucs_install(ctx);
        }
        if preload_demucs {
            self.start_demucs_model_preload(ctx);
        }
        if stop_demucs {
            self.stop_demucs_model_work();
        }
        if uninstall_demucs {
            self.uninstall_demucs_from_settings();
        }
        if install_stream_driver {
            self.start_stream_driver_install(ctx);
        }
        if uninstall_stream_driver {
            self.start_stream_driver_uninstall(ctx);
        }

        if save_startup
            && let Some(sound_id) = self.settings_startup_candidate
            && let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            match self.storage.save_startup_sound(sound) {
                Ok(()) => {
                    self.startup_sound_name = Some(sound.name.clone());
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if save_exit
            && let Some(sound_id) = self.settings_exit_candidate
            && let Some(sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            match self.storage.save_exit_sound(sound) {
                Ok(()) => {
                    self.exit_sound_name = Some(sound.name.clone());
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if clear_startup {
            match self.storage.clear_startup_sound() {
                Ok(()) => self.startup_sound_name = None,
                Err(error) => self.set_error_status(error),
            }
        }

        if clear_exit {
            match self.storage.clear_exit_sound() {
                Ok(()) => self.exit_sound_name = None,
                Err(error) => self.set_error_status(error),
            }
        }

        if reset_startup {
            match self.storage.reset_startup_sound() {
                Ok(()) => {
                    let resolved = self.storage.resolved_startup_sound_path().ok().flatten();
                    self.startup_sound_name = self.storage.load_startup_sound_name().ok().flatten();
                    let visual =
                        Self::load_transition_sound_visual(&self.storage, resolved.clone());
                    self.startup.sound_waveform = visual.0;
                    self.startup.sound_duration_sec = visual.1;
                    self.startup.duration_sec = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_INTRO_DURATION_SEC,
                    );
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }

        if reset_exit {
            match self.storage.reset_exit_sound() {
                Ok(()) => {
                    let resolved = self.storage.resolved_exit_sound_path().ok().flatten();
                    self.exit_sound_name = self.storage.load_exit_sound_name().ok().flatten();
                    let duration = Self::custom_transition_duration_secs_opt(
                        &self.storage,
                        resolved.as_deref(),
                        DEFAULT_OUTRO_DURATION_SEC,
                    );
                    if self.startup.phase == TransitionPhase::Outro {
                        self.startup.duration_sec = duration;
                    }
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
    }
}
