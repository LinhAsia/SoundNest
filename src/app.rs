use crate::audio::AudioEngine;
use crate::downloader::YoutubeAudioDownloader;
use crate::pitch::{
    PitchInputSource, PitchMonitor, PitchMonitorConfig, PitchSnapshot, list_capture_devices,
};
use crate::platform;
use crate::storage::{SoundEffect, Storage, format_time};
use anyhow::{Context as _, Result};
#[cfg(windows)]
use clipboard_win::{Clipboard, Setter, formats::FileList};
use eframe::egui::{
    self, Align, Button, CentralPanel, Color32, ComboBox, Context, CornerRadius, FontFamily, Frame,
    Margin, Pos2, Rect, RichText, ScrollArea, Sense, Slider, Stroke, StrokeKind, TextEdit, Ui,
    Vec2, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId, vec2,
};
use eframe::epaint::Shadow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const AUDIO_FILTERS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];
const APP_FRAME_RADIUS: f32 = 30.0;
const APP_OUTER_MARGIN: f32 = 0.0;
const MATERIAL_ICONS_FONT: &str = "material_icons";
const PITCH_OVERLAY_ID: &str = "pitch-monitor-overlay";
const PITCH_OVERLAY_TITLE: &str = "Sound FX Pitch Overlay";

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppView {
    Editor,
    Library,
}

pub struct SoundFxApp {
    storage: Storage,
    audio: Option<AudioEngine>,
    sounds: Vec<SoundEffect>,
    selected: Option<Uuid>,
    status: Option<String>,
    downloader: YoutubeAudioDownloader,
    download_url: String,
    show_download_panel: bool,
    download_was_running: bool,
    show_import_panel: bool,
    import_dir: PathBuf,
    app_view: AppView,
    pitch_monitor: PitchMonitor,
    pitch_update_hz: f32,
    pitch_overlay_animation: bool,
    pitch_show_sharps: bool,
    pitch_input_source: PitchInputSource,
    pitch_capture_devices: Vec<String>,
    selected_pitch_input_device: Option<String>,
    center_pitch_overlay_next_frame: bool,
    pitch_overlay_native_visuals_applied: bool,
    startup: StartupSplashState,
    center_window_next_frame: bool,
    titlebar_drag_rect: Option<Rect>,
    native_shadow_applied: bool,
    pending_save: bool,
    last_edit_at: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransitionPhase {
    Intro,
    Live,
    Outro,
}

struct StartupSplashState {
    phase: TransitionPhase,
    started_at: Option<f64>,
    duration_sec: f32,
    close_sent: bool,
}

impl SoundFxApp {
    pub fn new() -> Self {
        let storage = Storage::new().unwrap_or_else(|error| panic!("Storage init failed: {error}"));
        let mut status = None;
        let mut sounds = storage.load_library().unwrap_or_else(|error| {
            status = Some(error.to_string());
            Vec::new()
        });

        let mut hydrated = false;
        for sound in &mut sounds {
            if let Ok(changed) = storage.hydrate_sound(sound) {
                hydrated |= changed;
            }
        }
        if hydrated {
            let _ = storage.save_library(&sounds);
        }

        let audio = AudioEngine::new().ok();
        if audio.is_none() && status.is_none() {
            status = Some("Audio unavailable".to_owned());
        }

        let downloader = YoutubeAudioDownloader::new(storage.root_dir()).unwrap_or_else(|error| {
            panic!("Downloader init failed: {error}");
        });
        let import_dir = storage.load_import_dir().ok().flatten().unwrap_or_default();
        let pitch_update_hz = storage.load_pitch_update_hz().ok().flatten().unwrap_or(4.0);
        let pitch_overlay_animation = storage
            .load_overlay_animation()
            .ok()
            .flatten()
            .unwrap_or(true);
        let pitch_show_sharps = storage
            .load_pitch_show_sharps()
            .ok()
            .flatten()
            .unwrap_or(false);
        let pitch_capture_devices = list_capture_devices().unwrap_or_default();
        let selected_pitch_input_device = pitch_capture_devices.first().cloned();

        Self {
            storage,
            audio,
            sounds,
            selected: None,
            status,
            downloader,
            download_url: String::new(),
            show_download_panel: false,
            download_was_running: false,
            show_import_panel: false,
            import_dir,
            app_view: AppView::Editor,
            pitch_monitor: PitchMonitor::new(),
            pitch_update_hz,
            pitch_overlay_animation,
            pitch_show_sharps,
            pitch_input_source: PitchInputSource::System,
            pitch_capture_devices,
            selected_pitch_input_device,
            center_pitch_overlay_next_frame: false,
            pitch_overlay_native_visuals_applied: false,
            startup: StartupSplashState {
                phase: TransitionPhase::Intro,
                started_at: None,
                duration_sec: 1.35,
                close_sent: false,
            },
            center_window_next_frame: true,
            titlebar_drag_rect: None,
            native_shadow_applied: false,
            pending_save: false,
            last_edit_at: 0.0,
        }
        .with_initial_selection()
    }

    fn with_initial_selection(mut self) -> Self {
        self.selected = self.sounds.first().map(|sound| sound.id);
        self
    }

    fn clear_status(&mut self) {
        self.status = None;
    }

    fn set_error_status(&mut self, error: impl ToString) {
        self.status = Some(error.to_string());
    }

    fn selected_sound_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.sounds.iter().position(|sound| sound.id == selected)
    }

    fn is_transition_active(&self) -> bool {
        self.startup.phase != TransitionPhase::Live
    }

    fn center_window_if_needed(&mut self, ctx: &Context) {
        if !self.center_window_next_frame {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Self::desired_window_size()));
        if let Some(center_cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(center_cmd);
            self.center_window_next_frame = false;
        }
    }

    fn desired_window_size() -> Vec2 {
        vec2(900.0, 900.0)
    }

    fn enforce_square_window_if_needed(&mut self, _ctx: &Context) {}

    fn request_close(&mut self, ctx: &Context) {
        if self.startup.phase == TransitionPhase::Outro {
            return;
        }

        self.show_download_panel = false;
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.pitch_monitor.stop();
        self.pitch_overlay_native_visuals_applied = false;
        ctx.send_viewport_cmd_to(Self::pitch_overlay_viewport_id(), ViewportCommand::Close);
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }

        self.startup.phase = TransitionPhase::Outro;
        self.startup.started_at = None;
        self.startup.duration_sec = 0.72;
        ctx.request_repaint();
    }

    fn handle_space_preview(&mut self, ctx: &Context) {
        if self.is_transition_active() || self.show_download_panel {
            return;
        }

        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            return;
        }

        let Some(sound_id) = self.selected else {
            return;
        };
        self.preview_sound(sound_id);
    }

    fn intercept_close_request(&mut self, ctx: &Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.startup.close_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(ctx);
        }
    }

    fn add_sound(&mut self) {
        self.show_import_panel = true;
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>) {
        let mut imported = Vec::new();
        let mut ignored = 0usize;

        for path in paths {
            if !is_supported_audio(&path) {
                ignored += 1;
                continue;
            }

            match self.storage.import_sound(&path) {
                Ok(sound) => imported.push(sound),
                Err(error) => self.set_error_status(error),
            }
        }

        if imported.is_empty() {
            if ignored > 0 && self.status.is_none() {
                self.set_error_status("Unsupported file");
            }
            return;
        }

        imported.reverse();
        for sound in imported {
            self.selected = Some(sound.id);
            self.sounds.insert(0, sound);
        }

        let _ = ignored;
        self.save_now();
    }

    fn open_sound_from_library(&mut self, sound_id: Uuid) {
        self.selected = Some(sound_id);
        self.app_view = AppView::Editor;
    }

    fn available_import_roots() -> Vec<PathBuf> {
        ["C:\\", "D:\\"]
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .collect()
    }

    fn set_import_dir(&mut self, path: Option<PathBuf>) {
        self.import_dir = path.unwrap_or_default();
        let _ = self.storage.save_import_dir(
            (!self.import_dir.as_os_str().is_empty()).then_some(self.import_dir.as_path()),
        );
    }

    fn truncate_middle_ascii(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(6) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(3) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push('.');
        compact.push('.');
        compact.push('.');
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    fn pitch_overlay_viewport_id() -> ViewportId {
        ViewportId::from_hash_of(PITCH_OVERLAY_ID)
    }

    fn refresh_pitch_capture_devices(&mut self) {
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

    #[allow(dead_code)]
    fn truncate_middle(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(3) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(1) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push('…');
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    fn handle_dropped_files(&mut self, ctx: &Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let paths = dropped
            .into_iter()
            .filter_map(|file| file.path)
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            self.import_paths(paths);
        }
    }

    fn preview_sound(&mut self, sound_id: Uuid) {
        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };

        let asset_path = sound.asset_path(self.storage.root_dir());
        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };

        match audio.play(&sound, &asset_path) {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    fn stop_preview(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
    }

    fn delete_selected(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound = self.sounds.remove(index);
        if let Some(audio) = self.audio.as_mut() {
            if audio.is_playing(sound.id) {
                audio.stop();
            }
        }

        if let Err(error) = self.storage.remove_sound(&sound) {
            self.set_error_status(error);
            return;
        }

        self.selected = self
            .sounds
            .get(index)
            .or_else(|| self.sounds.get(index.saturating_sub(1)))
            .map(|next| next.id);
        self.save_now();
    }

    fn copy_selected_processed_sound(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        match self.copy_sound_file_to_clipboard(&self.sounds[index]) {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    fn copy_sound_file_to_clipboard(&self, sound: &SoundEffect) -> Result<()> {
        let export_path = self.storage.export_processed_sound(sound)?;
        #[cfg(windows)]
        {
            let _clipboard =
                Clipboard::new_attempts(10).context("unable to open system clipboard")?;
            let paths = [export_path.display().to_string()];
            FileList
                .write_clipboard(&paths)
                .context("unable to place sound file on clipboard")?;
            return Ok(());
        }

        #[cfg(not(windows))]
        {
            let _ = export_path;
            anyhow::bail!("Clipboard file copy is only available on Windows");
        }
    }

    fn mark_dirty(&mut self, ctx: &Context) {
        self.pending_save = true;
        self.last_edit_at = ctx.input(|input| input.time);
    }

    fn flush_pending_save(&mut self, ctx: &Context) {
        if !self.pending_save {
            return;
        }

        let now = ctx.input(|input| input.time);
        if now - self.last_edit_at >= 0.18 {
            self.save_now();
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn save_now(&mut self) {
        match self.storage.save_library(&self.sounds) {
            Ok(()) => {
                self.pending_save = false;
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    fn icon(codepoint: u32, size: f32, color: Color32) -> RichText {
        RichText::new(char::from_u32(codepoint).unwrap_or(' '))
            .family(FontFamily::Name(MATERIAL_ICONS_FONT.into()))
            .size(size)
            .color(color)
    }

    fn titlebar_button(label: RichText, active: bool, danger: bool) -> Button<'static> {
        let (fill, stroke) = if danger {
            (
                Color32::from_rgba_premultiplied(176, 62, 104, if active { 160 } else { 88 }),
                Color32::from_rgb(214, 92, 136),
            )
        } else if active {
            (
                Color32::from_rgba_premultiplied(229, 85, 149, 118),
                Color32::from_rgb(214, 51, 132),
            )
        } else {
            (
                Color32::from_rgba_premultiplied(237, 231, 238, 198),
                Color32::from_rgb(221, 212, 222),
            )
        };

        Button::new(label.strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(9.0)
    }

    fn action_button(label: RichText, active: bool, accent: bool) -> Button<'static> {
        let (fill, stroke, text) = if accent {
            (
                Color32::from_rgb(214, 51, 132),
                Color32::from_rgb(214, 51, 132),
                Color32::WHITE,
            )
        } else if active {
            (
                Color32::from_rgb(255, 231, 243),
                Color32::from_rgb(230, 94, 150),
                Color32::from_rgb(120, 22, 72),
            )
        } else {
            (
                Color32::WHITE,
                Color32::from_rgb(227, 217, 226),
                Color32::from_rgb(60, 54, 61),
            )
        };

        Button::new(label.color(text))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(18.0)
    }

    fn icon_action(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        accent: bool,
    ) -> egui::Response {
        let icon_color = if accent || active {
            Color32::WHITE
        } else {
            Color32::from_rgb(60, 54, 61)
        };
        let response = ui.add_sized(
            size,
            Self::action_button(Self::icon(codepoint, 18.0, icon_color), active, accent),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn icon_titlebar(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        danger: bool,
    ) -> egui::Response {
        let response = ui.add_sized(
            size,
            Self::titlebar_button(
                Self::icon(codepoint, 18.0, Color32::from_rgb(60, 54, 61)),
                active,
                danger,
            ),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn decorate_button_response(ui: &Ui, response: &egui::Response) {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            Self::paint_hover_button_notes(
                ui.painter(),
                response.rect,
                ui.input(|input| input.time) as f32,
            );
        }
    }

    fn paint_hover_button_notes(painter: &egui::Painter, rect: Rect, time: f32) {
        let anchor = Pos2::new(rect.right() - 10.0, rect.top() - 4.0);
        for (index, (dx, dy, scale, phase)) in [
            (-2.0, 2.0, 0.34, 0.0),
            (10.0, -6.0, 0.28, 0.8),
            (18.0, 6.0, 0.24, 1.4),
        ]
        .into_iter()
        .enumerate()
        {
            let drift = (time * 2.8 + phase).sin() * 3.0;
            let rise = (time * 2.0 + phase).cos() * 2.0 - index as f32 * 2.5;
            Self::paint_music_note(
                painter,
                Pos2::new(anchor.x + dx + drift, anchor.y + dy + rise),
                scale,
                (time * 1.3 + phase).sin() * 0.16,
                Color32::from_rgba_premultiplied(229, 85, 149, 188),
            );
        }
    }

    fn draw_titlebar(&mut self, ui: &mut Ui, ctx: &Context) {
        self.titlebar_drag_rect = None;
        ui.horizontal(|ui| {
            let drag_width = (ui.available_width() - 246.0).max(180.0);
            let drag_response = ui
                .allocate_ui_with_layout(
                    vec2(drag_width, 44.0),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        let frame = Frame::new()
                            .fill(Color32::from_rgba_premultiplied(255, 246, 250, 238))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(233, 217, 226)))
                            .corner_radius(12.0)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    Self::paint_titlebar_blob(ui);
                                    ui.add_space(10.0);
                                    Self::paint_titlebar_wave(ui);
                                });
                            });
                        ui.interact(
                            frame.response.rect,
                            ui.id().with("titlebar-drag"),
                            Sense::click_and_drag(),
                        )
                    },
                )
                .inner;

            self.titlebar_drag_rect = Some(drag_response.rect);
            if drag_response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            if let Some(status) = &self.status {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(status)
                        .size(12.0)
                        .color(Color32::from_rgb(171, 54, 91)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if Self::icon_titlebar(ui, [38.0, 30.0], 0xe5cd, false, true).clicked() {
                    self.request_close(ctx);
                }

                if Self::icon_titlebar(ui, [38.0, 30.0], 0xe15b, false, false).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }

                if Self::icon_titlebar(
                    ui,
                    [42.0, 30.0],
                    0xe9b0,
                    self.app_view == AppView::Library,
                    false,
                )
                .clicked()
                {
                    self.app_view = if self.app_view == AppView::Library {
                        AppView::Editor
                    } else {
                        AppView::Library
                    };
                }

                if Self::icon_titlebar(ui, [42.0, 30.0], 0xe145, true, false).clicked() {
                    self.add_sound();
                }

                if Self::icon_titlebar(ui, [48.0, 30.0], 0xe2c4, false, false).clicked() {
                    self.show_download_panel = true;
                }
            });
        });
    }

    fn render_titlebar_drag_zone(&self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        let Some(handle_rect) = self.titlebar_drag_rect else {
            return;
        };

        egui::Area::new(egui::Id::new("titlebar-drag-overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(handle_rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (_, response) =
                    ui.allocate_exact_size(handle_rect.size(), Sense::click_and_drag());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });
    }

    fn render_custom_window_resize_handles(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
            return;
        }

        let rect = ctx.screen_rect();
        let edge = 8.0;
        let corner = 22.0;
        let handles = [
            (
                "resize-n",
                Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + edge)),
                egui::viewport::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                "resize-s",
                Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - edge), rect.max),
                egui::viewport::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                "resize-w",
                Rect::from_min_max(rect.min, egui::pos2(rect.min.x + edge, rect.max.y)),
                egui::viewport::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                "resize-e",
                Rect::from_min_max(egui::pos2(rect.max.x - edge, rect.min.y), rect.max),
                egui::viewport::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                "resize-nw",
                Rect::from_min_size(rect.min, vec2(corner, corner)),
                egui::viewport::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                "resize-ne",
                Rect::from_min_max(
                    egui::pos2(rect.max.x - corner, rect.min.y),
                    egui::pos2(rect.max.x, rect.min.y + corner),
                ),
                egui::viewport::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "resize-sw",
                Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.max.y - corner),
                    egui::pos2(rect.min.x + corner, rect.max.y),
                ),
                egui::viewport::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                "resize-se",
                Rect::from_min_max(
                    egui::pos2(rect.max.x - corner, rect.max.y - corner),
                    rect.max,
                ),
                egui::viewport::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (id, handle_rect, direction, cursor) in handles {
            egui::Area::new(egui::Id::new(id))
                .order(egui::Order::Foreground)
                .fixed_pos(handle_rect.min)
                .interactable(true)
                .show(ctx, |ui| {
                    let (_, response) =
                        ui.allocate_exact_size(handle_rect.size(), Sense::click_and_drag());
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(cursor);
                    }
                    if response.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                    }
                });
        }
    }

    fn paint_titlebar_blob(ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 24.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let center = rect.center();
        for (radius_x, radius_y, tint, phase) in [
            (
                15.5,
                8.8,
                Color32::from_rgba_premultiplied(255, 173, 216, 160),
                0.0,
            ),
            (
                12.0,
                7.0,
                Color32::from_rgba_premultiplied(246, 124, 181, 148),
                0.9,
            ),
            (
                8.6,
                5.8,
                Color32::from_rgba_premultiplied(227, 82, 149, 164),
                1.8,
            ),
        ] {
            let mut points = Vec::with_capacity(36);
            for step in 0..36 {
                let angle = step as f32 / 36.0 * std::f32::consts::TAU;
                let wobble = 1.0 + 0.18 * (angle * 3.0 + phase).sin() + 0.08 * (angle * 5.0).cos();
                points.push(egui::pos2(
                    center.x + angle.cos() * radius_x * wobble,
                    center.y + angle.sin() * radius_y * wobble,
                ));
            }
            painter.add(egui::Shape::convex_polygon(points, tint, Stroke::NONE));
        }
    }

    fn paint_titlebar_wave(ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(130.0, 18.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let bars: [f32; 12] = [
            0.22, 0.48, 0.84, 0.52, 0.28, 0.74, 0.94, 0.62, 0.34, 0.56, 0.3, 0.78,
        ];
        let bar_width = rect.width() / bars.len() as f32;
        for (index, bar) in bars.into_iter().enumerate() {
            let x = rect.left() + (index as f32 + 0.5) * bar_width;
            let half = bar * rect.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                egui::pos2(x - bar_width * 0.22, rect.center().y - half),
                egui::pos2(x + bar_width * 0.22, rect.center().y + half),
            );
            painter.rect_filled(wave_rect, 2.0, Color32::from_rgb(227, 82, 149));
        }
    }

    fn import_browser_entries(&self) -> Vec<(PathBuf, bool)> {
        if self.import_dir.as_os_str().is_empty() || !self.import_dir.exists() {
            return Vec::new();
        }

        let mut entries = fs::read_dir(&self.import_dir)
            .ok()
            .into_iter()
            .flat_map(|iter| iter.filter_map(|entry| entry.ok()))
            .filter_map(|entry| {
                let path = entry.path();
                let is_dir = path.is_dir();
                if is_dir || is_supported_audio(&path) {
                    Some((path, is_dir))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        entries.sort_by(|(left_path, left_dir), (right_path, right_dir)| {
            right_dir
                .cmp(left_dir)
                .then_with(|| left_path.file_name().cmp(&right_path.file_name()))
        });
        entries
    }

    fn render_import_panel(&mut self, ctx: &Context) {
        if !self.show_import_panel {
            return;
        }

        let mut open_panel = self.show_import_panel;
        let entries = self.import_browser_entries();
        let mut go_up = false;
        let mut next_dir = None;
        let mut import_file = None;
        let mut close_request = false;
        let mut switch_root = None;
        let roots = Self::available_import_roots();
        let browsing_root = self.import_dir.as_os_str().is_empty() || !self.import_dir.exists();
        let path_label = if browsing_root {
            "Drive".to_owned()
        } else {
            Self::truncate_middle_ascii(self.import_dir.to_string_lossy().as_ref(), 42)
        };

        egui::Window::new("")
            .id(egui::Id::new("sound-import-panel"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(640.0, 560.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open_panel)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(255, 249, 252))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(236, 211, 226)))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 30,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe2c8, 20.0, Color32::from_rgb(41, 36, 42)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if Self::icon_action(ui, [42.0, 34.0], 0xe2c8, browsing_root, false).clicked() {
                        self.set_import_dir(None);
                    }
                    if !browsing_root
                        && Self::icon_action(ui, [42.0, 34.0], 0xe5d8, false, false).clicked()
                    {
                        go_up = true;
                    }
                    for root in &roots {
                        let label = root.to_string_lossy();
                        let active = !browsing_root && self.import_dir.starts_with(root);
                        let response = ui.add_sized(
                            [74.0, 38.0],
                            Self::action_button(
                                RichText::new(label.as_ref()).size(14.0),
                                active,
                                active,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            switch_root = Some(root.clone());
                        }
                    }
                    ui.add_space(6.0);
                    Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, Color32::from_rgb(239, 224, 233)))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.add_sized(
                                [ui.available_width().max(120.0), 18.0],
                                egui::Label::new(
                                    RichText::new(path_label.as_str())
                                        .size(13.0)
                                        .color(Color32::from_rgb(96, 84, 94)),
                                )
                                .truncate(),
                            );
                        });
                });

                ui.add_space(14.0);

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if browsing_root {
                            for root in &roots {
                                let label = root.to_string_lossy();
                                let row = Frame::new()
                                    .fill(Color32::WHITE)
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(239, 224, 233)))
                                    .corner_radius(22.0)
                                    .inner_margin(Margin::symmetric(16, 14))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(Self::icon(
                                                0xe2c8,
                                                18.0,
                                                Color32::from_rgb(214, 51, 132),
                                            ));
                                            ui.add_space(10.0);
                                            ui.add_sized(
                                                [ui.available_width(), 18.0],
                                                egui::Label::new(
                                                    RichText::new(label.as_ref())
                                                        .size(14.0)
                                                        .color(Color32::from_rgb(52, 46, 53)),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                                let response = ui.interact(
                                    row.response.rect,
                                    ui.id().with(root),
                                    Sense::click(),
                                );
                                if response.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if response.clicked() {
                                    switch_root = Some(root.clone());
                                }
                                ui.add_space(10.0);
                            }
                            return;
                        }

                        for (path, is_dir) in entries {
                            let label = path
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("sound");
                            let icon = if is_dir { 0xe2c8 } else { 0xeb82 };
                            let row = Frame::new()
                                .fill(Color32::WHITE)
                                .stroke(Stroke::new(1.0, Color32::from_rgb(239, 224, 233)))
                                .corner_radius(22.0)
                                .inner_margin(Margin::symmetric(16, 14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(Self::icon(
                                            icon,
                                            18.0,
                                            Color32::from_rgb(214, 51, 132),
                                        ));
                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [ui.available_width(), 18.0],
                                            egui::Label::new(
                                                RichText::new(label)
                                                    .size(14.0)
                                                    .color(Color32::from_rgb(52, 46, 53)),
                                            )
                                            .truncate(),
                                        );
                                    });
                                });
                            let response =
                                ui.interact(row.response.rect, ui.id().with(&path), Sense::click());
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if response.clicked() {
                                if is_dir {
                                    next_dir = Some(path.clone());
                                } else {
                                    import_file = Some(path.clone());
                                }
                            }
                            ui.add_space(10.0);
                        }
                    });
            });

        if close_request {
            open_panel = false;
        }
        self.show_import_panel = open_panel;
        if go_up {
            if let Some(parent) = self.import_dir.parent() {
                self.set_import_dir(Some(parent.to_path_buf()));
            }
        }
        if let Some(root) = switch_root {
            self.set_import_dir(Some(root));
        }
        if let Some(dir) = next_dir {
            self.set_import_dir(Some(dir));
        }
        if let Some(path) = import_file {
            self.import_paths(vec![path]);
            self.show_import_panel = false;
        }
    }

    fn render_pitch_monitor(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        let snapshot = self.pitch_monitor.snapshot();
        if snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if let Some(error) = snapshot.error.clone() {
            self.set_error_status(error);
        }

        let mut toggle = None;
        let mut refresh_inputs = false;
        egui::Area::new(egui::Id::new("pitch-monitor-panel"))
            .anchor(egui::Align2::LEFT_BOTTOM, vec2(26.0, -24.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgb(255, 252, 254))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(233, 220, 228)))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(260.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("SPN")
                                    .size(13.0)
                                    .color(Color32::from_rgb(58, 48, 58))
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                let icon = if snapshot.running { 0xe047 } else { 0xe037 };
                                if Self::icon_action(
                                    ui,
                                    [56.0, 34.0],
                                    icon,
                                    snapshot.running,
                                    snapshot.running,
                                )
                                .clicked()
                                {
                                    toggle = Some(!snapshot.running);
                                }
                            });
                        });

                        ui.add_space(8.0);
                        ui.add_enabled_ui(!snapshot.running, |ui| {
                            ui.horizontal(|ui| {
                                let system = ui.add_sized(
                                    [72.0, 34.0],
                                    Self::action_button(
                                        RichText::new("SYS").size(13.0),
                                        self.pitch_input_source == PitchInputSource::System,
                                        self.pitch_input_source == PitchInputSource::System,
                                    ),
                                );
                                Self::decorate_button_response(ui, &system);
                                if system.clicked() {
                                    self.pitch_input_source = PitchInputSource::System;
                                }

                                let mic = ui.add_sized(
                                    [72.0, 34.0],
                                    Self::action_button(
                                        RichText::new("MIC").size(13.0),
                                        self.pitch_input_source == PitchInputSource::Microphone,
                                        self.pitch_input_source == PitchInputSource::Microphone,
                                    ),
                                );
                                Self::decorate_button_response(ui, &mic);
                                if mic.clicked() {
                                    self.pitch_input_source = PitchInputSource::Microphone;
                                    if self.pitch_capture_devices.is_empty() {
                                        refresh_inputs = true;
                                    }
                                }
                            });
                        });

                        if self.pitch_input_source == PitchInputSource::Microphone {
                            ui.add_space(8.0);
                            ui.add_enabled_ui(!snapshot.running, |ui| {
                                Frame::new()
                                    .fill(Color32::WHITE)
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(234, 223, 230)))
                                    .corner_radius(18.0)
                                    .inner_margin(Margin::symmetric(12, 8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ComboBox::from_id_salt("pitch-input-device")
                                            .width(ui.available_width() - 4.0)
                                            .selected_text(
                                                self.selected_pitch_input_device
                                                    .as_deref()
                                                    .map(|name| {
                                                        Self::truncate_middle_ascii(name, 28)
                                                    })
                                                    .unwrap_or_else(|| "No mic".to_owned()),
                                            )
                                            .show_ui(ui, |ui| {
                                                for name in &self.pitch_capture_devices {
                                                    ui.selectable_value(
                                                        &mut self.selected_pitch_input_device,
                                                        Some(name.clone()),
                                                        Self::truncate_middle_ascii(name, 38),
                                                    );
                                                }
                                            });
                                    });
                            });
                        }

                        ui.add_space(8.0);
                        ui.add_enabled_ui(!snapshot.running, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Self::icon(
                                    0xe8b5,
                                    16.0,
                                    Color32::from_rgb(118, 106, 116),
                                ));
                                let speed_response = ui.add(
                                    Slider::new(&mut self.pitch_update_hz, 1.0..=12.0)
                                        .show_value(false)
                                        .step_by(0.5),
                                );
                                ui.label(
                                    RichText::new(format!("{:.1}/s", self.pitch_update_hz))
                                        .size(13.0)
                                        .color(Color32::from_rgb(58, 48, 58)),
                                );
                                if speed_response.changed() {
                                    let _ = self.storage.save_pitch_update_hz(self.pitch_update_hz);
                                }
                            });
                        });

                        ui.add_space(8.0);
                        ui.add_enabled_ui(!snapshot.running, |ui| {
                            Frame::new()
                                .fill(Color32::from_rgb(255, 248, 252))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(236, 224, 232)))
                                .corner_radius(18.0)
                                .inner_margin(Margin::symmetric(12, 10))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let animation_changed = ui
                                            .checkbox(
                                                &mut self.pitch_overlay_animation,
                                                RichText::new("Animation")
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(58, 48, 58)),
                                            )
                                            .changed();
                                        ui.add_space(10.0);
                                        let sharp_changed = ui
                                            .checkbox(
                                                &mut self.pitch_show_sharps,
                                                RichText::new("Sharp")
                                                    .size(13.0)
                                                    .color(Color32::from_rgb(58, 48, 58)),
                                            )
                                            .changed();
                                        if animation_changed {
                                            let _ = self.storage.save_overlay_animation(
                                                self.pitch_overlay_animation,
                                            );
                                            self.center_pitch_overlay_next_frame = snapshot.running;
                                            self.pitch_overlay_native_visuals_applied = false;
                                        }
                                        if sharp_changed {
                                            let _ = self
                                                .storage
                                                .save_pitch_show_sharps(self.pitch_show_sharps);
                                        }
                                    });
                                });
                        });

                        if let Some(error) = snapshot.error.as_deref() {
                            ui.add_space(6.0);
                            ui.add_sized(
                                [ui.available_width(), 16.0],
                                egui::Label::new(
                                    RichText::new(Self::truncate_middle_ascii(error, 34))
                                        .size(11.0)
                                        .color(Color32::from_rgb(189, 62, 117)),
                                )
                                .truncate(),
                            );
                        }
                    });
            });

        if let Some(should_start) = toggle {
            if should_start {
                if self.pitch_input_source == PitchInputSource::Microphone
                    && self.selected_pitch_input_device.is_none()
                {
                    self.set_error_status("No microphone input found");
                } else {
                    match self.pitch_monitor.start(PitchMonitorConfig {
                        source: self.pitch_input_source,
                        input_device_name: if self.pitch_input_source
                            == PitchInputSource::Microphone
                        {
                            self.selected_pitch_input_device.clone()
                        } else {
                            None
                        },
                        updates_per_second: self.pitch_update_hz,
                    }) {
                        Ok(()) => {
                            self.center_pitch_overlay_next_frame = true;
                            self.pitch_overlay_native_visuals_applied = false;
                            self.clear_status();
                        }
                        Err(error) => self.set_error_status(error),
                    }
                }
            } else {
                self.pitch_monitor.stop();
                self.pitch_overlay_native_visuals_applied = false;
                ctx.send_viewport_cmd_to(Self::pitch_overlay_viewport_id(), ViewportCommand::Close);
                self.clear_status();
            }
        }
        if refresh_inputs {
            self.refresh_pitch_capture_devices();
        }
    }

    fn render_pitch_overlay_viewport(&mut self, ctx: &Context) {
        let snapshot = self.pitch_monitor.snapshot();
        if !snapshot.running {
            return;
        }

        let viewport_id = Self::pitch_overlay_viewport_id();
        let mut should_stop = false;
        let overlay_size = if self.pitch_overlay_animation {
            vec2(276.0, 276.0)
        } else {
            vec2(430.0, 104.0)
        };
        let mut builder = ViewportBuilder::default()
            .with_title(PITCH_OVERLAY_TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_inner_size(overlay_size)
            .with_min_inner_size(overlay_size)
            .with_max_inner_size(overlay_size)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_visible(true)
            .with_active(false)
            .with_taskbar(false);
        if self.center_pitch_overlay_next_frame
            && let Some(monitor) = ctx.input(|input| input.viewport().monitor_size)
        {
            builder = builder.with_position(Pos2::new(
                ((monitor.x - overlay_size.x) * 0.5).max(0.0),
                ((monitor.y - overlay_size.y) * 0.5).max(0.0),
            ));
        }

        ctx.show_viewport_immediate(viewport_id, builder, |overlay_ctx, class| {
            if class == ViewportClass::Embedded {
                return;
            }

            if self.pitch_overlay_animation {
                let mut style = (*overlay_ctx.style()).clone();
                style.visuals.window_fill = Color32::TRANSPARENT;
                style.visuals.panel_fill = Color32::TRANSPARENT;
                style.visuals.extreme_bg_color = Color32::TRANSPARENT;
                style.visuals.faint_bg_color = Color32::TRANSPARENT;
                style.visuals.code_bg_color = Color32::TRANSPARENT;
                overlay_ctx.set_style(style);
            }

            if self.center_pitch_overlay_next_frame {
                overlay_ctx.send_viewport_cmd(ViewportCommand::InnerSize(overlay_size));
                if let Some(center_cmd) = ViewportCommand::center_on_screen(overlay_ctx) {
                    overlay_ctx.send_viewport_cmd(center_cmd);
                }
            }
            if overlay_ctx.input(|input| input.viewport().close_requested()) {
                should_stop = true;
            }

            overlay_ctx.request_repaint_after(Duration::from_millis(16));
            CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(Color32::from_rgba_premultiplied(0, 0, 0, 0))
                        .inner_margin(0.0),
                )
                .show(overlay_ctx, |ui| {
                    if self.pitch_overlay_animation {
                        self.render_pitch_blob_overlay(
                            ui,
                            overlay_ctx,
                            &snapshot,
                            &mut should_stop,
                        );
                    } else {
                        self.render_pitch_pill_overlay(
                            ui,
                            overlay_ctx,
                            &snapshot,
                            &mut should_stop,
                        );
                    }
                });
        });
        self.center_pitch_overlay_next_frame = false;
        if !self.pitch_overlay_native_visuals_applied
            && platform::set_overlay_window_native_visuals(
                PITCH_OVERLAY_TITLE,
                !self.pitch_overlay_animation,
                self.pitch_overlay_animation,
            )
        {
            self.pitch_overlay_native_visuals_applied = true;
        }

        if should_stop {
            self.pitch_monitor.stop();
            self.pitch_overlay_native_visuals_applied = false;
            ctx.send_viewport_cmd_to(viewport_id, ViewportCommand::Close);
            self.clear_status();
        }
    }

    fn render_pitch_pill_overlay(
        &self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &PitchSnapshot,
        should_stop: &mut bool,
    ) {
        let rect = ui.max_rect().shrink2(vec2(10.0, 14.0));
        let painter = ui.painter_at(rect);
        let glow_color = Color32::from_rgba_premultiplied(227, 82, 149, 26);
        painter.circle_filled(
            Pos2::new(rect.left() + 74.0, rect.center().y),
            44.0,
            glow_color,
        );
        painter.circle_filled(
            Pos2::new(rect.right() - 108.0, rect.center().y),
            58.0,
            Color32::from_rgba_premultiplied(236, 126, 183, 18),
        );

        let capsule = Frame::new()
            .fill(Color32::from_rgba_premultiplied(255, 252, 254, 168))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_premultiplied(255, 255, 255, 112),
            ))
            .shadow(Shadow {
                offset: [0, 18],
                blur: 36,
                spread: 0,
                color: Color32::from_rgba_premultiplied(78, 40, 63, 30),
            })
            .corner_radius(44.0)
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                let content_size = vec2(rect.width(), rect.height());
                ui.set_min_size(content_size);
                let drag_rect = Rect::from_min_max(
                    ui.min_rect().min,
                    Pos2::new(ui.min_rect().max.x - 46.0, ui.min_rect().max.y),
                );
                let drag_response = ui.interact(
                    drag_rect,
                    ui.id().with("pitch-overlay-drag"),
                    Sense::click_and_drag(),
                );
                if drag_response.drag_started() || drag_response.is_pointer_button_down_on() {
                    overlay_ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }

                let top_highlight = Rect::from_min_max(
                    Pos2::new(ui.min_rect().left() + 18.0, ui.min_rect().top() + 1.0),
                    Pos2::new(ui.min_rect().right() - 58.0, ui.min_rect().top() + 14.0),
                );
                ui.painter().rect_filled(
                    top_highlight,
                    14.0,
                    Color32::from_rgba_premultiplied(255, 255, 255, 34),
                );

                let body_height = (content_size.y - 10.0).max(62.0);
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    vec2(content_size.x, body_height),
                    egui::Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(52.0, 52.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let center = ui.max_rect().center();
                                ui.painter().circle_filled(
                                    center,
                                    18.0,
                                    Color32::from_rgba_premultiplied(227, 82, 149, 40),
                                );
                                ui.painter().circle_filled(
                                    center,
                                    8.0 + snapshot.level * 6.0,
                                    Color32::from_rgb(227, 82, 149),
                                );
                            },
                        );
                        ui.add_space(6.0);
                        ui.allocate_ui_with_layout(
                            vec2(110.0, 52.0),
                            egui::Layout::top_down_justified(Align::Min),
                            |ui| {
                                ui.add_space(1.0);
                                ui.label(
                                    RichText::new(self.display_pitch_note(&snapshot.note))
                                        .size(33.0)
                                        .color(Color32::from_rgb(44, 38, 46))
                                        .strong(),
                                );
                                ui.add_space(-2.0);
                                ui.label(
                                    RichText::new(self.pitch_overlay_meta(snapshot))
                                        .size(11.5)
                                        .color(Color32::from_rgb(117, 101, 113)),
                                );
                            },
                        );
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            vec2(176.0, 38.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                Self::draw_pitch_overlay_wave_strip(ui, snapshot);
                            },
                        );
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            vec2(34.0, 34.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                let close =
                                    Self::icon_titlebar(ui, [34.0, 30.0], 0xe5cd, false, true);
                                if close.clicked() {
                                    *should_stop = true;
                                }
                            },
                        );
                    },
                );
            });
        let _ = capsule.response;
    }

    fn render_pitch_blob_overlay(
        &mut self,
        ui: &mut Ui,
        overlay_ctx: &Context,
        snapshot: &PitchSnapshot,
        should_stop: &mut bool,
    ) {
        let rect = ui.max_rect().shrink2(vec2(4.0, 4.0));
        let drag_response = ui.interact(
            rect,
            ui.id().with("pitch-blob-overlay-drag"),
            Sense::click_and_drag(),
        );
        if drag_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag_response.dragged() || drag_response.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if drag_response.drag_started() || drag_response.is_pointer_button_down_on() {
            overlay_ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let painter = ui.painter_at(rect);
        let center = rect.center();
        let time = overlay_ctx.input(|input| input.time) as f32;
        let aura = snapshot.level.clamp(0.04, 1.0);
        let waveform_energy = if snapshot.waveform.is_empty() {
            aura
        } else {
            snapshot.waveform.iter().copied().sum::<f32>() / snapshot.waveform.len() as f32
        };
        let glow = aura.max(waveform_energy).clamp(0.05, 1.0);
        let base = rect.width().min(rect.height()) * 0.34;
        let half_w = base * (1.0 + glow * 0.16);
        let half_h = base * (0.92 + glow * 0.24);
        let wobble = 0.052 + glow * 0.16;
        let exponent = 4.0 - glow * 1.35;

        let deep_shadow = Color32::from_rgb(4, 4, 8);
        let ink = Color32::from_rgb(10, 10, 14);
        let charcoal = Color32::from_rgb(18, 18, 24);
        let graphite = Color32::from_rgb(28, 28, 36);
        let berry = Color32::from_rgb(232, 73, 154);
        let magenta = Color32::from_rgb(255, 118, 186);
        let petal = Color32::from_rgb(255, 238, 247);

        for (spread, alpha) in [(1.42, 22), (1.2, 34), (1.04, 56)] {
            let shadow_points = Self::squircle_points(
                Pos2::new(center.x, center.y + 14.0 + glow * 8.0),
                half_w * spread,
                half_h * spread,
                exponent,
                wobble * 0.72,
                time - spread,
            );
            painter.add(egui::Shape::convex_polygon(
                shadow_points,
                Color32::from_rgba_premultiplied(
                    deep_shadow.r(),
                    deep_shadow.g(),
                    deep_shadow.b(),
                    alpha,
                ),
                Stroke::NONE,
            ));
        }

        let outer_points = Self::squircle_points(
            center,
            half_w * 1.02,
            half_h * 1.02,
            exponent,
            wobble * 1.12,
            time,
        );
        painter.add(egui::Shape::convex_polygon(
            outer_points,
            Color32::from_rgba_premultiplied(charcoal.r(), charcoal.g(), charcoal.b(), 124),
            Stroke::new(
                1.4,
                Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 94),
            ),
        ));

        let blob_points =
            Self::squircle_points(center, half_w, half_h, exponent, wobble, time + 0.4);
        painter.add(egui::Shape::convex_polygon(
            blob_points.clone(),
            Color32::from_rgba_premultiplied(ink.r(), ink.g(), ink.b(), 238),
            Stroke::new(
                1.2,
                Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 112),
            ),
        ));

        let highlight_points = Self::squircle_points(
            Pos2::new(center.x, center.y - half_h * 0.2),
            half_w * 0.76,
            half_h * 0.38,
            exponent,
            wobble * 0.5,
            time + 1.1,
        );
        painter.add(egui::Shape::convex_polygon(
            highlight_points,
            Color32::from_rgba_premultiplied(graphite.r(), graphite.g(), graphite.b(), 112),
            Stroke::NONE,
        ));

        let clip_rect = Rect::from_center_size(center, vec2(half_w * 1.52, half_h * 1.46));
        let clip = painter.with_clip_rect(clip_rect);
        let wave = if snapshot.waveform.is_empty() {
            vec![0.08; 40]
        } else {
            snapshot.waveform.clone()
        };
        let band_rect = Rect::from_center_size(
            Pos2::new(center.x, center.y + half_h * 0.54),
            vec2(half_w * 1.08, half_h * 0.22),
        );
        let bar_width = band_rect.width() / wave.len().max(1) as f32;
        for (index, value) in wave.iter().enumerate() {
            let pulse = (time * 5.2 + index as f32 * 0.46).sin() * 0.06;
            let amplitude = (value + pulse).clamp(0.06, 1.0);
            let x = band_rect.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * band_rect.height() * 0.52;
            let bar_rect = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.18, band_rect.center().y - half),
                Pos2::new(x + bar_width * 0.18, band_rect.center().y + half),
            );
            clip.rect_filled(
                bar_rect,
                4.0,
                Color32::from_rgba_premultiplied(berry.r(), berry.g(), berry.b(), 176),
            );
        }

        for index in 0..8 {
            let angle = time * 0.84 + index as f32 * 0.82;
            let orbit = base * (0.64 + (index % 3) as f32 * 0.12) + glow * 18.0;
            let note_pos = Pos2::new(
                center.x + angle.cos() * orbit,
                center.y + angle.sin() * orbit * 0.72,
            );
            let note_alpha = (60.0 + glow * 92.0).clamp(0.0, 160.0) as u8;
            let note_color = if index % 2 == 0 {
                Color32::from_rgba_premultiplied(255, 205, 230, note_alpha)
            } else {
                Color32::from_rgba_premultiplied(235, 112, 176, note_alpha)
            };
            Self::paint_music_note(
                &painter,
                note_pos,
                0.5 + (index % 3) as f32 * 0.1,
                angle.sin() * 0.22,
                note_color,
            );
        }

        let note_pos = Pos2::new(center.x, center.y - 12.0);
        painter.text(
            note_pos,
            egui::Align2::CENTER_CENTER,
            self.display_pitch_note(&snapshot.note),
            egui::FontId::proportional(42.0),
            petal,
        );

        let sharp_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 50.0, rect.top() + 18.0),
            vec2(34.0, 26.0),
        );
        let sharp_response = ui.interact(
            sharp_rect,
            ui.id().with("pitch-blob-overlay-sharp"),
            Sense::click(),
        );
        let close_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 18.0, rect.top() + 18.0),
            vec2(26.0, 26.0),
        );
        let close_response = ui.interact(
            close_rect,
            ui.id().with("pitch-blob-overlay-close"),
            Sense::click(),
        );
        let show_controls =
            drag_response.hovered() || close_response.hovered() || sharp_response.hovered();
        if sharp_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if close_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if show_controls {
            let sharp_active = self.pitch_show_sharps;
            painter.rect_filled(
                sharp_rect,
                13.0,
                if sharp_active {
                    Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 228)
                } else if sharp_response.hovered() {
                    Color32::from_rgba_premultiplied(36, 36, 44, 232)
                } else {
                    Color32::from_rgba_premultiplied(18, 18, 24, 188)
                },
            );
            painter.rect_stroke(
                sharp_rect,
                13.0,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(
                        magenta.r(),
                        magenta.g(),
                        magenta.b(),
                        if sharp_active { 230 } else { 112 },
                    ),
                ),
                StrokeKind::Outside,
            );
            painter.text(
                sharp_rect.center(),
                egui::Align2::CENTER_CENTER,
                "S#",
                egui::FontId::proportional(12.5),
                if sharp_active {
                    Color32::from_rgb(20, 12, 18)
                } else {
                    petal
                },
            );
            painter.circle_filled(
                Pos2::new(sharp_rect.right() - 6.0, sharp_rect.top() + 6.0),
                2.6,
                if sharp_active {
                    Color32::from_rgb(255, 245, 250)
                } else {
                    Color32::from_rgba_premultiplied(255, 255, 255, 70)
                },
            );

            painter.circle_filled(
                close_rect.center(),
                13.0,
                if close_response.hovered() {
                    Color32::from_rgba_premultiplied(36, 36, 44, 232)
                } else {
                    Color32::from_rgba_premultiplied(18, 18, 24, 188)
                },
            );
            painter.circle_stroke(
                close_rect.center(),
                13.0,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(magenta.r(), magenta.g(), magenta.b(), 132),
                ),
            );
            painter.text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                Self::icon(0xe5cd, 16.0, petal).text(),
                egui::FontId::new(16.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
                petal,
            );
        }
        if sharp_response.clicked() {
            self.pitch_show_sharps = !self.pitch_show_sharps;
            let _ = self.storage.save_pitch_show_sharps(self.pitch_show_sharps);
        }
        if close_response.clicked() {
            *should_stop = true;
        }
    }

    fn pitch_overlay_meta(&self, snapshot: &PitchSnapshot) -> String {
        format!(
            "{}  {:.0}%",
            if self.pitch_input_source == PitchInputSource::Microphone {
                Self::truncate_middle_ascii(
                    self.selected_pitch_input_device.as_deref().unwrap_or("MIC"),
                    12,
                )
            } else {
                "SYS".to_owned()
            },
            (snapshot.confidence * 100.0).round()
        )
    }

    fn display_pitch_note(&self, note: &str) -> String {
        if !note.contains('/') {
            return note.to_owned();
        }

        let Some((sharp_name, flat_with_octave)) = note.split_once('/') else {
            return note.to_owned();
        };
        let Some(first_digit) = flat_with_octave
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_digit() || *ch == '-')
            .map(|(index, _)| index)
        else {
            return note.to_owned();
        };

        let flat_name = &flat_with_octave[..first_digit];
        let octave = &flat_with_octave[first_digit..];
        if self.pitch_show_sharps {
            format!("{sharp_name}{octave}")
        } else {
            format!("{flat_name}{octave}")
        }
    }

    fn draw_pitch_overlay_wave_strip(ui: &mut Ui, snapshot: &PitchSnapshot) {
        let desired = vec2(ui.available_width().max(150.0), 38.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            19.0,
            Color32::from_rgba_premultiplied(255, 255, 255, 84),
        );
        painter.rect_stroke(
            rect,
            19.0,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 76)),
            StrokeKind::Outside,
        );

        let waveform = if snapshot.waveform.is_empty() {
            vec![0.04; 40]
        } else {
            snapshot.waveform.clone()
        };
        let inner = rect.shrink2(vec2(10.0, 8.0));
        let bar_width = inner.width() / waveform.len().max(1) as f32;
        for (index, level) in waveform.iter().enumerate() {
            let x = inner.left() + index as f32 * bar_width;
            let height = inner.height() * level.clamp(0.06, 1.0);
            let bar = Rect::from_min_max(
                Pos2::new(x + bar_width * 0.22, inner.center().y - height * 0.5),
                Pos2::new(x + bar_width * 0.78, inner.center().y + height * 0.5),
            );
            painter.rect_filled(bar, 3.0, Color32::from_rgb(227, 82, 149));
        }
    }

    fn draw_library_grid(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.sounds.is_empty() {
                    Self::draw_empty_editor(ui);
                    return;
                }

                let card_size = 208.0;
                let spacing = 16.0;
                let sounds = self.sounds.clone();
                let mut open_sound = None;
                let mut preview_sound = None;
                let mut copy_sound = None;

                let columns = (((ui.available_width() + spacing) / (card_size + spacing)).floor()
                    as usize)
                    .max(1);
                let row_count = sounds.len().div_ceil(columns);

                for (row_index, row) in sounds.chunks(columns).enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = vec2(spacing, spacing);

                        for sound in row {
                            let (tile_rect, tile_response) =
                                ui.allocate_exact_size(vec2(card_size, card_size), Sense::hover());
                            let body_rect = Rect::from_min_max(
                                tile_rect.min,
                                Pos2::new(tile_rect.max.x, tile_rect.max.y - 48.0),
                            );
                            let body_response = ui.interact(
                                body_rect,
                                ui.id().with(("library-grid", sound.id)),
                                Sense::click(),
                            );
                            let hovered = tile_response.hovered() || body_response.hovered();

                            if hovered {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if body_response.clicked() {
                                open_sound = Some(sound.id);
                            }

                            ui.scope_builder(egui::UiBuilder::new().max_rect(tile_rect), |ui| {
                                let fill = if hovered {
                                    Color32::from_rgb(227, 82, 149)
                                } else {
                                    Color32::WHITE
                                };
                                let stroke = if hovered {
                                    Color32::from_rgb(227, 82, 149)
                                } else {
                                    Color32::from_rgb(232, 220, 228)
                                };
                                let title_color = if hovered {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(40, 35, 41)
                                };
                                let meta_color = if hovered {
                                    Color32::from_rgba_premultiplied(255, 255, 255, 196)
                                } else {
                                    Color32::from_rgb(116, 106, 115)
                                };

                                Frame::new()
                                    .fill(fill)
                                    .stroke(Stroke::new(1.0, stroke))
                                    .shadow(Shadow {
                                        offset: [0, 12],
                                        blur: 28,
                                        spread: 0,
                                        color: Color32::from_rgba_premultiplied(86, 43, 67, 18),
                                    })
                                    .corner_radius(30.0)
                                    .inner_margin(Margin::same(18))
                                    .show(ui, |ui| {
                                        let inner_size = card_size - 36.0;
                                        ui.set_min_size(vec2(inner_size, inner_size));
                                        ui.set_width(inner_size);
                                        ui.vertical(|ui| {
                                            ui.add_sized(
                                                [inner_size, 20.0],
                                                egui::Label::new(
                                                    RichText::new(&sound.name)
                                                        .size(13.0)
                                                        .color(title_color)
                                                        .strong(),
                                                )
                                                .truncate(),
                                            );
                                            ui.add_space(8.0);
                                            Self::draw_wave_strip(
                                                ui,
                                                &sound.waveform,
                                                None,
                                                if hovered {
                                                    Color32::from_rgb(255, 214, 232)
                                                } else {
                                                    Color32::from_rgb(214, 51, 132)
                                                },
                                                if hovered {
                                                    Color32::from_rgb(255, 214, 232)
                                                } else {
                                                    Color32::from_rgb(238, 213, 227)
                                                },
                                                if hovered {
                                                    Color32::from_rgba_premultiplied(
                                                        255, 255, 255, 22,
                                                    )
                                                } else {
                                                    Color32::from_rgb(252, 248, 251)
                                                },
                                                78.0,
                                            );
                                            ui.add_space(10.0);
                                            ui.label(
                                                RichText::new(format_time(sound.trimmed_length()))
                                                    .size(12.0)
                                                    .color(meta_color),
                                            );
                                            ui.add_space(10.0);
                                            ui.horizontal(|ui| {
                                                if Self::icon_action(
                                                    ui,
                                                    [48.0, 32.0],
                                                    0xe037,
                                                    false,
                                                    false,
                                                )
                                                .clicked()
                                                {
                                                    preview_sound = Some(sound.id);
                                                }
                                                if Self::icon_action(
                                                    ui,
                                                    [48.0, 32.0],
                                                    0xe14d,
                                                    false,
                                                    true,
                                                )
                                                .clicked()
                                                {
                                                    copy_sound = Some(sound.id);
                                                }
                                            });
                                        });
                                    });
                            });
                        }
                    });

                    if row_index + 1 < row_count {
                        ui.add_space(spacing);
                    }
                }

                if let Some(sound_id) = preview_sound {
                    self.preview_sound(sound_id);
                }
                if let Some(sound_id) = copy_sound {
                    if let Some(sound) = self
                        .sounds
                        .iter()
                        .find(|sound| sound.id == sound_id)
                        .cloned()
                    {
                        if let Err(error) = self.copy_sound_file_to_clipboard(&sound) {
                            self.set_error_status(error);
                        }
                    }
                }
                if let Some(sound_id) = open_sound {
                    self.open_sound_from_library(sound_id);
                }
            });
    }

    fn draw_library(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.sounds.is_empty() {
                        Frame::new()
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(1.0, Color32::from_rgb(230, 219, 227)))
                            .shadow(Shadow {
                                offset: [0, 10],
                                blur: 22,
                                spread: 0,
                                color: Color32::from_rgba_premultiplied(98, 46, 74, 18),
                            })
                            .corner_radius(28.0)
                            .inner_margin(Margin::same(22))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("+")
                                        .size(28.0)
                                        .color(Color32::from_rgb(214, 51, 132)),
                                );
                            });
                        return;
                    }

                    let mut preview_request = None;

                    for sound in &self.sounds {
                        let selected = self.selected == Some(sound.id);
                        let playing = self
                            .audio
                            .as_ref()
                            .is_some_and(|audio| audio.is_playing(sound.id));
                        let progress = self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.playback_progress(sound.id));
                        let frame = Frame::new()
                            .fill(if selected {
                                Color32::from_rgb(255, 239, 247)
                            } else {
                                Color32::WHITE
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if selected {
                                    Color32::from_rgb(235, 118, 171)
                                } else {
                                    Color32::from_rgb(231, 220, 228)
                                },
                            ))
                            .shadow(Shadow {
                                offset: [0, 10],
                                blur: 24,
                                spread: 0,
                                color: if selected {
                                    Color32::from_rgba_premultiplied(138, 45, 93, 26)
                                } else {
                                    Color32::from_rgba_premultiplied(82, 48, 70, 14)
                                },
                            })
                            .corner_radius(28.0)
                            .inner_margin(Margin::same(18))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&sound.name)
                                        .size(16.5)
                                        .color(Color32::from_rgb(40, 35, 41))
                                        .strong(),
                                );
                                ui.add_space(8.0);
                                Self::draw_wave_strip(
                                    ui,
                                    &sound.waveform,
                                    progress,
                                    Color32::from_rgb(214, 51, 132),
                                    Color32::from_rgb(238, 213, 227),
                                    Color32::from_rgb(252, 248, 251),
                                    52.0,
                                );
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format_time(sound.trimmed_length()))
                                            .size(12.0)
                                            .color(Color32::from_rgb(116, 106, 115)),
                                    );
                                    ui.separator();
                                    ui.label(
                                        RichText::new(format!("{:.0}%", sound.volume * 100.0))
                                            .size(12.0)
                                            .color(Color32::from_rgb(116, 106, 115)),
                                    );
                                    if playing {
                                        ui.separator();
                                        ui.label(
                                            RichText::new("•")
                                                .size(12.0)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        );
                                    }
                                });
                            });

                        let response = ui.interact(
                            frame.response.rect,
                            ui.id().with(sound.id),
                            Sense::click(),
                        );
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            self.selected = Some(sound.id);
                            preview_request = Some(sound.id);
                        }
                        ui.add_space(12.0);
                    }

                    if let Some(sound_id) = preview_request {
                        self.preview_sound(sound_id);
                    }
                });
        });
    }

    fn draw_editor(&mut self, ui: &mut Ui, ctx: &Context) {
        let Some(index) = self.selected_sound_index() else {
            Self::draw_empty_editor(ui);
            return;
        };

        let sound_id = self.sounds[index].id;
        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound_id));
        let progress = self
            .audio
            .as_ref()
            .and_then(|audio| audio.playback_progress(sound_id));

        let mut preview_toggle = false;
        let mut delete_request = false;
        let mut copy_request = false;
        let mut changed = false;

        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, Color32::from_rgb(232, 220, 228)))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Color32::from_rgba_premultiplied(86, 43, 67, 22),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(28))
            .show(ui, |ui| {
                let sound = &mut self.sounds[index];
                let controls_width = 52.0 + 64.0 + 64.0 + 28.0;
                let name_width = (ui.available_width() - controls_width).max(180.0);

                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        [name_width, 50.0],
                        TextEdit::singleline(&mut sound.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(name_width)
                            .margin(Vec2::new(14.0, 14.0)),
                    );
                    if response.changed() {
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_action(ui, [52.0, 34.0], 0xe872, false, false).clicked() {
                            delete_request = true;
                        }
                        if Self::icon_action(ui, [64.0, 34.0], 0xe14d, false, true).clicked() {
                            copy_request = true;
                        }
                        if Self::icon_action(
                            ui,
                            [64.0, 34.0],
                            if is_playing { 0xe047 } else { 0xe037 },
                            is_playing,
                            false,
                        )
                        .clicked()
                        {
                            preview_toggle = true;
                        }
                    });
                });

                ui.add_space(24.0);

                Frame::new()
                    .fill(Color32::from_rgb(252, 248, 251))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(237, 226, 234)))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        changed |= Self::draw_trim_timeline(ui, sound, progress);
                    });

                ui.add_space(18.0);

                Frame::new()
                    .fill(Color32::from_rgb(252, 248, 251))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(237, 226, 234)))
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(Self::icon(0xe050, 16.0, Color32::from_rgb(118, 106, 116)));
                            let volume_response = ui.add(
                                Slider::new(&mut sound.volume, 0.0..=2.0)
                                    .show_value(false)
                                    .step_by(0.01),
                            );
                            ui.label(
                                RichText::new(format!("{:.0}%", sound.volume * 100.0))
                                    .size(14.0)
                                    .color(Color32::from_rgb(40, 35, 41)),
                            );
                            ui.add_space(10.0);
                            ui.label(Self::icon(0xe9e4, 16.0, Color32::from_rgb(118, 106, 116)));
                            let speed_response = ui.add(
                                Slider::new(&mut sound.speed, 0.25..=2.0)
                                    .show_value(false)
                                    .step_by(0.01),
                            );
                            ui.label(
                                RichText::new(format!("{:.2}x", sound.speed))
                                    .size(14.0)
                                    .color(Color32::from_rgb(40, 35, 41)),
                            );
                            if volume_response.changed() || speed_response.changed() {
                                changed = true;
                            }
                        });
                    });
            });

        if changed {
            self.mark_dirty(ctx);
        }

        if delete_request {
            self.delete_selected();
            return;
        }

        if copy_request {
            self.copy_selected_processed_sound();
        }

        if preview_toggle {
            if is_playing {
                self.stop_preview();
            } else {
                self.preview_sound(sound_id);
            }
        }
    }

    fn draw_empty_editor(ui: &mut Ui) {
        Frame::new()
            .fill(Color32::from_rgba_premultiplied(255, 255, 255, 210))
            .stroke(Stroke::new(1.0, Color32::from_rgb(236, 223, 232)))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Color32::from_rgba_premultiplied(84, 48, 69, 16),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(28))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(160.0);
                    ui.label(
                        RichText::new("+")
                            .size(48.0)
                            .color(Color32::from_rgb(214, 51, 132)),
                    );
                });
            });
    }

    fn draw_trim_timeline(ui: &mut Ui, sound: &mut SoundEffect, progress: Option<f32>) -> bool {
        sound.clamp_trim();

        ui.label(Self::icon(0xe14e, 14.0, Color32::from_rgb(118, 106, 116)));
        ui.add_space(8.0);

        let desired_size = vec2(ui.available_width().max(320.0), 196.0);
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 18.0, Color32::from_rgb(255, 255, 255));
        painter.rect_stroke(
            rect,
            18.0,
            Stroke::new(1.0, Color32::from_rgb(235, 223, 232)),
            StrokeKind::Outside,
        );

        let duration = sound.safe_duration();
        let start_t = sound.trim_start_secs / duration;
        let end_t = sound.trim_end_secs / duration;
        let start_x = rect.left() + rect.width() * start_t.clamp(0.0, 1.0);
        let end_x = rect.left() + rect.width() * end_t.clamp(0.0, 1.0);

        Self::paint_waveform_bars(
            &painter,
            rect.shrink2(vec2(12.0, 18.0)),
            &sound.waveform,
            start_x,
            end_x,
            progress,
        );

        let selection = Rect::from_min_max(
            Pos2::new(start_x, rect.top() + 12.0),
            Pos2::new(end_x.max(start_x + 2.0), rect.bottom() - 12.0),
        );
        painter.rect_filled(
            selection,
            16.0,
            Color32::from_rgba_premultiplied(227, 82, 149, 24),
        );

        let handle_stroke = Stroke::new(2.0, Color32::from_rgb(214, 51, 132));
        painter.line_segment(
            [
                Pos2::new(start_x, rect.top() + 10.0),
                Pos2::new(start_x, rect.bottom() - 10.0),
            ],
            handle_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(end_x, rect.top() + 10.0),
                Pos2::new(end_x, rect.bottom() - 10.0),
            ],
            handle_stroke,
        );
        painter.circle_filled(
            Pos2::new(start_x, rect.center().y),
            7.0,
            Color32::from_rgb(214, 51, 132),
        );
        painter.circle_filled(
            Pos2::new(end_x, rect.center().y),
            7.0,
            Color32::from_rgb(214, 51, 132),
        );

        if let Some(progress) = progress {
            let play_x = egui::lerp(start_x..=end_x.max(start_x + 1.0), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, rect.top() + 8.0),
                    Pos2::new(play_x, rect.bottom() - 8.0),
                ],
                Stroke::new(2.0, Color32::from_rgb(42, 39, 44)),
            );
        }

        let start_handle_rect = Rect::from_center_size(
            Pos2::new(start_x, rect.center().y),
            vec2(24.0, rect.height()),
        );
        let end_handle_rect =
            Rect::from_center_size(Pos2::new(end_x, rect.center().y), vec2(24.0, rect.height()));
        let start_response = ui.interact(
            start_handle_rect,
            ui.make_persistent_id((sound.id, "trim-start")),
            Sense::click_and_drag(),
        );
        let end_response = ui.interact(
            end_handle_rect,
            ui.make_persistent_id((sound.id, "trim-end")),
            Sense::click_and_drag(),
        );

        let mut changed = false;
        if duration > 0.0
            && let Some(pointer) = start_response.interact_pointer_pos()
            && (start_response.clicked() || start_response.dragged())
        {
            let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let next = ratio * duration;
            sound.trim_start_secs = next.min(sound.trim_end_secs - 0.05);
            sound.clamp_trim();
            changed = true;
        } else if duration > 0.0
            && let Some(pointer) = end_response.interact_pointer_pos()
            && (end_response.clicked() || end_response.dragged())
        {
            let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let next = ratio * duration;
            sound.trim_end_secs = next.max(sound.trim_start_secs + 0.05);
            sound.clamp_trim();
            changed = true;
        } else if response.clicked()
            && duration > 0.0
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let next = ratio * duration;
            if (pointer.x - start_x).abs() <= (pointer.x - end_x).abs() {
                sound.trim_start_secs = next.min(sound.trim_end_secs - 0.05);
            } else {
                sound.trim_end_secs = next.max(sound.trim_start_secs + 0.05);
            }
            sound.clamp_trim();
            changed = true;
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_time(sound.trim_start_secs))
                    .size(13.0)
                    .color(Color32::from_rgb(118, 106, 116)),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.trimmed_length()))
                    .size(13.0)
                    .color(Color32::from_rgb(214, 51, 132)),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.trim_end_secs))
                    .size(13.0)
                    .color(Color32::from_rgb(118, 106, 116)),
            );
        });

        changed
    }

    fn paint_waveform_bars(
        painter: &egui::Painter,
        rect: Rect,
        waveform: &[f32],
        start_x: f32,
        end_x: f32,
        progress: Option<f32>,
    ) {
        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.center().y),
                    Pos2::new(rect.right(), rect.center().y),
                ],
                Stroke::new(2.0, Color32::from_rgb(221, 214, 220)),
            );
            if let Some(progress) = progress {
                let play_x = egui::lerp(rect.left()..=rect.right(), progress.clamp(0.0, 1.0));
                painter.line_segment(
                    [
                        Pos2::new(play_x, rect.top()),
                        Pos2::new(play_x, rect.bottom()),
                    ],
                    Stroke::new(2.0, Color32::from_rgb(42, 39, 44)),
                );
            }
            return;
        }

        let bar_width = rect.width() / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.05, 1.0);
            let center_x = rect.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * rect.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.3).max(1.0),
                    rect.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.3).max(1.0),
                    rect.center().y + half,
                ),
            );
            let selected = center_x >= start_x && center_x <= end_x;
            let color = if selected {
                Color32::from_rgb(227, 82, 149)
            } else {
                Color32::from_rgb(234, 214, 226)
            };
            painter.rect_filled(wave_rect, 2.0, color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(start_x..=end_x.max(start_x + 1.0), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, rect.top()),
                    Pos2::new(play_x, rect.bottom()),
                ],
                Stroke::new(2.0, Color32::from_rgb(34, 31, 36)),
            );
        }
    }

    fn draw_wave_strip(
        ui: &mut Ui,
        waveform: &[f32],
        progress: Option<f32>,
        active_color: Color32,
        idle_color: Color32,
        background_color: Color32,
        height: f32,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 14.0, background_color);

        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 12.0, rect.center().y),
                    Pos2::new(rect.right() - 12.0, rect.center().y),
                ],
                Stroke::new(2.0, idle_color),
            );
            return;
        }

        let inner = rect.shrink2(vec2(10.0, 8.0));
        let bar_width = inner.width() / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.06, 1.0);
            let center_x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * inner.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.28).max(1.0),
                    inner.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.28).max(1.0),
                    inner.center().y + half,
                ),
            );
            painter.rect_filled(wave_rect, 2.0, idle_color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(inner.left()..=inner.right(), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, inner.top()),
                    Pos2::new(play_x, inner.bottom()),
                ],
                Stroke::new(2.0, active_color),
            );
        }
    }

    fn render_download_panel(&mut self, ctx: &Context) {
        if !self.show_download_panel {
            return;
        }

        let snapshot = self.downloader.snapshot();
        let mut open_panel = self.show_download_panel;
        let mut should_start_download = false;
        let mut add_to_library = false;
        let mut open_file = false;
        let mut open_folder = false;
        let mut clear_result = false;
        let mut minimize_request = false;
        let mut close_request = false;

        egui::Window::new("")
            .id(egui::Id::new("youtube-audio-download"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_size(vec2(520.0, 260.0))
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open_panel)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(255, 252, 254))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(233, 220, 228)))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 32,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(28.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe2c4, 20.0, Color32::from_rgb(41, 36, 42)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            clear_result = !snapshot.running;
                            close_request = true;
                        }
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe15b, false, false).clicked() {
                            minimize_request = true;
                        }
                    });
                });

                ui.add_space(10.0);

                let response = ui.add(
                    TextEdit::singleline(&mut self.download_url)
                        .hint_text("https://youtube.com/watch?v=...")
                        .desired_width(ui.available_width())
                        .margin(Vec2::new(14.0, 12.0)),
                );
                if response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && !snapshot.running
                {
                    should_start_download = true;
                }

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let start_button = ui.add_enabled(
                        !snapshot.running && !self.download_url.trim().is_empty(),
                        Button::new(Self::icon(0xe2c4, 16.0, Color32::WHITE))
                            .fill(Color32::from_rgb(214, 51, 132))
                            .stroke(Stroke::NONE)
                            .corner_radius(18.0),
                    );
                    Self::decorate_button_response(ui, &start_button);
                    if start_button.clicked() {
                        should_start_download = true;
                    }

                    if snapshot.running {
                        ui.label(
                            RichText::new(snapshot.stage.clone())
                                .size(13.0)
                                .color(Color32::from_rgb(118, 106, 116)),
                        );
                    }
                });

                if let Some(progress) = snapshot.progress {
                    ui.add_space(8.0);
                    ui.add(
                        egui::ProgressBar::new(progress)
                            .desired_width(ui.available_width())
                            .fill(Color32::from_rgb(227, 82, 149)),
                    );
                }

                if let Some(error) = &snapshot.error {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(error)
                            .size(13.0)
                            .color(Color32::from_rgb(171, 54, 91)),
                    );
                }

                if let Some(path) = &snapshot.last_file {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(
                            path.file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("audio"),
                        )
                        .size(14.0)
                        .color(Color32::from_rgb(55, 47, 54))
                        .strong(),
                    );

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let add_response = ui.add_enabled(
                            snapshot.can_add_to_library,
                            Self::action_button(
                                Self::icon(0xe02e, 18.0, Color32::WHITE),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &add_response);
                        if add_response.clicked() {
                            add_to_library = true;
                        }
                        if Self::icon_action(ui, [52.0, 34.0], 0xe89e, false, false).clicked() {
                            open_file = true;
                        }
                        if Self::icon_action(ui, [52.0, 34.0], 0xe2c8, false, false).clicked() {
                            open_folder = true;
                        }
                        if Self::icon_action(ui, [52.0, 34.0], 0xe14c, false, false).clicked() {
                            clear_result = true;
                        }
                    });
                }
            });

        if close_request {
            open_panel = false;
        }
        if minimize_request {
            open_panel = false;
        }
        self.show_download_panel = open_panel;

        if should_start_download {
            match self
                .downloader
                .start_audio_download(self.download_url.trim().to_owned())
            {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }

        if let Some(path) = snapshot.last_file.clone() {
            if add_to_library {
                self.import_paths(vec![path.clone()]);
                self.downloader.mark_added_to_library();
            }
            if open_file {
                if let Err(error) = self.downloader.open_file(&path) {
                    self.set_error_status(error);
                }
            }
            if open_folder {
                if let Err(error) = self.downloader.open_folder(&path) {
                    self.set_error_status(error);
                }
            }
        }

        if clear_result {
            self.downloader.clear_result();
        }
    }

    fn transition_progress(&mut self, ctx: &Context) -> Option<(TransitionPhase, f32)> {
        let phase = self.startup.phase;
        if phase == TransitionPhase::Live {
            return None;
        }

        let now = ctx.input(|input| input.time);
        let started_at = self.startup.started_at.get_or_insert(now);
        let progress =
            ((now - *started_at) / self.startup.duration_sec as f64).clamp(0.0, 1.0) as f32;

        if progress >= 1.0 {
            match phase {
                TransitionPhase::Intro => {
                    self.startup.phase = TransitionPhase::Live;
                    self.startup.started_at = None;
                    self.startup.duration_sec = 0.0;
                    return None;
                }
                TransitionPhase::Outro => {
                    if !self.startup.close_sent {
                        self.startup.close_sent = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    ctx.request_repaint_after(Duration::from_millis(16));
                    return Some((phase, 1.0));
                }
                TransitionPhase::Live => {}
            }
        }

        ctx.request_repaint();
        Some((phase, progress))
    }

    fn render_transition_layer(&self, ctx: &Context, progress: f32, phase: TransitionPhase) {
        CentralPanel::default()
            .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(0.0))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let painter = ui.painter_at(rect);
                let time = ctx.input(|input| input.time) as f32;
                let center = rect.center();
                let rose_ice = Color32::from_rgb(255, 233, 242);
                let berry = Color32::from_rgb(205, 58, 126);
                let magenta = Color32::from_rgb(168, 36, 104);
                let plum = Color32::from_rgb(76, 24, 58);
                let deep_plum = Color32::from_rgb(30, 10, 24);
                let t = match phase {
                    TransitionPhase::Intro => Self::ease_in_out_cubic(progress),
                    TransitionPhase::Outro => 1.0 - Self::ease_in_out_cubic(progress),
                    TransitionPhase::Live => 1.0,
                };
                let aura = (1.0 - t).clamp(0.0, 1.0);
                let overlay = match phase {
                    TransitionPhase::Intro => (1.0 - t * 0.7).clamp(0.0, 1.0),
                    TransitionPhase::Outro => Self::ease_in_out_cubic(progress),
                    TransitionPhase::Live => 0.0,
                };
                let target_rect = Self::transition_target_rect(rect);
                let base = rect.width().min(rect.height()).clamp(260.0, 440.0);
                let half_w = egui::lerp((base * 0.17)..=(target_rect.width() * 0.5), t);
                let half_h = egui::lerp((base * 0.13)..=(target_rect.height() * 0.5), t);
                let exponent = egui::lerp(2.2..=6.4, t);
                let wobble = (1.0 - t).powf(1.4) * 0.24;
                let square_morph = Self::ease_in_out_cubic(((t - 0.56) / 0.44).clamp(0.0, 1.0));

                for star_index in 0..16 {
                    let seed = star_index as f32 * 11.713;
                    let px =
                        rect.left() + rect.width() * (0.18 + ((seed.sin() * 0.5 + 0.5) * 0.64));
                    let py = rect.top()
                        + rect.height() * (0.16 + (((seed * 1.7).cos() * 0.5 + 0.5) * 0.54));
                    let twinkle = 0.48 + ((time * 1.7 + seed).sin() * 0.52).abs();
                    painter.circle_filled(
                        Pos2::new(px, py),
                        0.8 + (star_index % 3) as f32 * 0.35,
                        Color32::from_rgba_premultiplied(
                            255,
                            246,
                            252,
                            (26.0 * twinkle * (0.35 + aura * 0.65)) as u8,
                        ),
                    );
                }

                let aura_layers = [
                    (
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.23,
                        base * 0.18,
                        Color32::from_rgba_premultiplied(
                            deep_plum.r(),
                            deep_plum.g(),
                            deep_plum.b(),
                            (74.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            plum.r(),
                            plum.g(),
                            plum.b(),
                            (52.0 + aura * 72.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x - base * 0.018, center.y + base * 0.018),
                        base * 0.31,
                        base * 0.24,
                        Color32::from_rgba_premultiplied(
                            plum.r(),
                            plum.g(),
                            plum.b(),
                            (58.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            magenta.r(),
                            magenta.g(),
                            magenta.b(),
                            (58.0 + aura * 82.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x + base * 0.024, center.y + base * 0.012),
                        base * 0.39,
                        base * 0.3,
                        Color32::from_rgba_premultiplied(
                            magenta.r(),
                            magenta.g(),
                            magenta.b(),
                            (42.0 * overlay) as u8,
                        ),
                        Color32::from_rgba_premultiplied(
                            berry.r(),
                            berry.g(),
                            berry.b(),
                            (50.0 + aura * 96.0) as u8,
                        ),
                    ),
                    (
                        Pos2::new(center.x - base * 0.012, center.y + base * 0.03),
                        base * 0.47,
                        base * 0.35,
                        Color32::from_rgba_premultiplied(231, 108, 171, (28.0 * overlay) as u8),
                        Color32::from_rgba_premultiplied(246, 141, 192, (46.0 + aura * 88.0) as u8),
                    ),
                ];
                for (layer_index, (layer_center, radius_x, radius_y, fill, stroke)) in
                    aura_layers.into_iter().enumerate()
                {
                    let stage = target_rect.shrink(10.0 + layer_index as f32 * 14.0);
                    let mut points = Vec::with_capacity(96);
                    for step in 0..96 {
                        let angle = step as f32 / 96.0 * std::f32::consts::TAU;
                        let blob_wobble = 1.0
                            + 0.18
                                * (angle * 3.0 + time * (0.82 + layer_index as f32 * 0.18)).sin()
                            + 0.11
                                * (angle * 5.0 - time * (0.56 + layer_index as f32 * 0.12)).cos()
                            + 0.05 * (angle * 9.0 + time * 0.7).sin()
                            + 0.025 * (angle * 13.0 - time * 0.9).cos();
                        let blob_point = Pos2::new(
                            layer_center.x + angle.cos() * radius_x * blob_wobble,
                            layer_center.y + angle.sin() * radius_y * blob_wobble,
                        );
                        let side = step / 24;
                        let side_t = (step % 24) as f32 / 24.0;
                        let square_point = match side {
                            0 => Pos2::new(
                                egui::lerp(stage.left()..=stage.right(), side_t),
                                stage.top(),
                            ),
                            1 => Pos2::new(
                                stage.right(),
                                egui::lerp(stage.top()..=stage.bottom(), side_t),
                            ),
                            2 => Pos2::new(
                                egui::lerp(stage.right()..=stage.left(), side_t),
                                stage.bottom(),
                            ),
                            _ => Pos2::new(
                                stage.left(),
                                egui::lerp(stage.bottom()..=stage.top(), side_t),
                            ),
                        };
                        points.push(Pos2::new(
                            egui::lerp(blob_point.x..=square_point.x, square_morph),
                            egui::lerp(blob_point.y..=square_point.y, square_morph),
                        ));
                    }
                    painter.add(egui::Shape::convex_polygon(
                        points,
                        fill,
                        Stroke::new(
                            (1.8 - layer_index as f32 * 0.24) * (1.0 - square_morph * 0.3),
                            stroke,
                        ),
                    ));
                }

                for (radius, alpha) in [
                    (base * 0.48, 28.0),
                    (base * 0.38, 42.0),
                    (base * 0.28, 68.0),
                    (base * 0.2, 96.0),
                ] {
                    painter.circle_filled(
                        center,
                        egui::lerp((radius * 0.75)..=radius, 1.0 - aura * 0.22),
                        Color32::from_rgba_premultiplied(
                            berry.r(),
                            berry.g(),
                            berry.b(),
                            (alpha * (0.2 + aura * 0.8)) as u8,
                        ),
                    );
                }

                let shadow_points = Self::squircle_points(
                    Pos2::new(center.x, center.y + 14.0 + aura * 12.0),
                    half_w * 1.02,
                    half_h * 1.02,
                    exponent,
                    wobble * 0.55,
                    time - 0.35,
                );
                painter.add(egui::Shape::convex_polygon(
                    shadow_points,
                    Color32::from_rgba_premultiplied(
                        deep_plum.r(),
                        deep_plum.g(),
                        deep_plum.b(),
                        (36.0 + t * 42.0) as u8,
                    ),
                    Stroke::NONE,
                ));

                let card_points =
                    Self::squircle_points(center, half_w, half_h, exponent, wobble, time);
                painter.add(egui::Shape::convex_polygon(
                    card_points.clone(),
                    Color32::from_rgba_premultiplied(255, 247, 251, (208.0 + t * 28.0) as u8),
                    Stroke::new(
                        1.2,
                        Color32::from_rgba_premultiplied(229, 168, 199, (116.0 + t * 68.0) as u8),
                    ),
                ));

                let glaze_points = Self::squircle_points(
                    Pos2::new(center.x, center.y - half_h * 0.14),
                    half_w * 0.92,
                    half_h * 0.54,
                    exponent,
                    wobble * 0.4,
                    time + 0.8,
                );
                painter.add(egui::Shape::convex_polygon(
                    glaze_points,
                    Color32::from_rgba_premultiplied(
                        rose_ice.r(),
                        rose_ice.g(),
                        rose_ice.b(),
                        (40.0 + (1.0 - aura) * 28.0) as u8,
                    ),
                    Stroke::NONE,
                ));

                let inner_rect = Rect::from_center_size(
                    center,
                    vec2(half_w * 1.34, half_h * 1.12).min(target_rect.size() * 0.92),
                );
                let clip = painter.with_clip_rect(inner_rect.expand2(vec2(10.0, 10.0)));
                let bars: [f32; 11] = [
                    0.24, 0.42, 0.76, 0.94, 0.56, 0.28, 0.68, 0.88, 0.5, 0.22, 0.62,
                ];
                let band_rect = Rect::from_center_size(
                    Pos2::new(center.x, center.y - half_h * 0.06),
                    vec2(inner_rect.width() * 0.68, inner_rect.height() * 0.34),
                );
                let bar_width = band_rect.width() / bars.len() as f32;
                for (index, bar) in bars.into_iter().enumerate() {
                    let phase_shift = time * 4.4 + index as f32 * 0.68;
                    let animated = (bar + 0.14_f32 * phase_shift.sin()).clamp(0.16, 1.0);
                    let x = band_rect.left() + (index as f32 + 0.5) * bar_width;
                    let half = animated * band_rect.height() * (0.2 + t * 0.3);
                    let wave_rect = Rect::from_min_max(
                        Pos2::new(x - bar_width * 0.22, band_rect.center().y - half),
                        Pos2::new(x + bar_width * 0.22, band_rect.center().y + half),
                    );
                    clip.rect_filled(
                        wave_rect,
                        4.0,
                        Color32::from_rgba_premultiplied(
                            magenta.r(),
                            magenta.g(),
                            magenta.b(),
                            (92.0 + t * 132.0) as u8,
                        ),
                    );
                }

                let ribbon_rect = Rect::from_center_size(
                    Pos2::new(center.x, center.y + half_h * 0.24),
                    vec2(inner_rect.width() * 0.76, inner_rect.height() * 0.18),
                );
                let mut line = Vec::with_capacity(120);
                for step in 0..120 {
                    let sample_t = step as f32 / 119.0;
                    let x = egui::lerp(ribbon_rect.left()..=ribbon_rect.right(), sample_t);
                    let y = ribbon_rect.center().y
                        + (sample_t * std::f32::consts::TAU * 2.2 + time * 2.9).sin()
                            * ribbon_rect.height()
                            * 0.34
                        + (sample_t * std::f32::consts::TAU * 5.8 - time * 1.5).cos()
                            * ribbon_rect.height()
                            * 0.14;
                    line.push(Pos2::new(x, y));
                }
                clip.add(egui::Shape::line(
                    line,
                    Stroke::new(
                        4.0,
                        Color32::from_rgba_premultiplied(
                            berry.r(),
                            berry.g(),
                            berry.b(),
                            (118.0 + t * 124.0) as u8,
                        ),
                    ),
                ));

                let accent_rect = Rect::from_center_size(
                    Pos2::new(center.x, center.y - half_h * 0.34),
                    vec2(inner_rect.width() * 0.48, 16.0 + t * 6.0),
                );
                clip.rect_filled(
                    accent_rect,
                    9.0,
                    Color32::from_rgba_premultiplied(
                        rose_ice.r(),
                        rose_ice.g(),
                        rose_ice.b(),
                        (34.0 + t * 38.0) as u8,
                    ),
                );

                for index in 0..7 {
                    let angle = time * 0.72 + index as f32 * 0.9;
                    let orbit =
                        egui::lerp((base * 0.32)..=(base * 0.18), t) + (index % 3) as f32 * 10.0;
                    let note_pos = Pos2::new(
                        center.x + angle.cos() * orbit,
                        center.y - half_h * 0.08 + angle.sin() * orbit * 0.72,
                    );
                    let note_alpha = (160.0 * aura).clamp(0.0, 160.0) as u8;
                    let note_color = if index % 2 == 0 {
                        Color32::from_rgba_premultiplied(255, 231, 242, note_alpha)
                    } else {
                        Color32::from_rgba_premultiplied(238, 164, 202, note_alpha)
                    };
                    Self::paint_music_note(
                        &painter,
                        note_pos,
                        0.64 + (index % 3) as f32 * 0.12,
                        angle.sin() * 0.18,
                        note_color,
                    );
                }
            });
    }

    fn transition_target_rect(rect: Rect) -> Rect {
        Rect::from_center_size(
            rect.center(),
            vec2(
                (rect.width() - APP_OUTER_MARGIN * 2.0).max(320.0),
                (rect.height() - APP_OUTER_MARGIN * 2.0).max(320.0),
            ),
        )
    }

    fn squircle_points(
        center: Pos2,
        half_w: f32,
        half_h: f32,
        exponent: f32,
        wobble: f32,
        time: f32,
    ) -> Vec<Pos2> {
        let mut points = Vec::with_capacity(72);
        for step in 0..72 {
            let angle = step as f32 / 72.0 * std::f32::consts::TAU;
            let cos = angle.cos();
            let sin = angle.sin();
            let power = 2.0 / exponent.max(2.0);
            let x = cos.signum() * cos.abs().powf(power) * half_w;
            let y = sin.signum() * sin.abs().powf(power) * half_h;
            let drift = 1.0
                + wobble * (angle * 3.0 + time * 1.4).sin()
                + wobble * 0.45 * (angle * 5.0 - time * 0.9).cos();
            points.push(Pos2::new(center.x + x * drift, center.y + y * drift));
        }
        points
    }

    fn ease_in_out_cubic(t: f32) -> f32 {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        }
    }

    fn paint_music_note(
        painter: &egui::Painter,
        center: Pos2,
        scale: f32,
        tilt: f32,
        color: Color32,
    ) {
        let head = Vec2::new(12.0 * scale, 8.0 * scale);
        let stem = 18.0 * scale;
        let head_center = Pos2::new(center.x, center.y + 6.0 * scale);
        painter.circle_filled(head_center, head.y * 0.8, color);
        let stem_top = Pos2::new(head_center.x + head.x * 0.5, head_center.y - stem);
        painter.line_segment(
            [
                Pos2::new(head_center.x + head.x * 0.4, head_center.y),
                stem_top,
            ],
            Stroke::new((2.8 * scale).max(1.0), color),
        );
        let flag_end = Pos2::new(
            stem_top.x + (12.0 * scale) * (1.0 + tilt),
            stem_top.y + 8.0 * scale,
        );
        painter.line_segment(
            [stem_top, flag_end],
            Stroke::new((2.4 * scale).max(1.0), color),
        );
    }
}

impl eframe::App for SoundFxApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let _ = self;
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        self.center_window_if_needed(ctx);
        self.intercept_close_request(ctx);

        let transition = self.transition_progress(ctx);
        let download_snapshot = self.downloader.snapshot();
        let wants_shadow = self.startup.phase == TransitionPhase::Live;
        if self.native_shadow_applied != wants_shadow {
            platform::set_native_window_shadow(frame, wants_shadow);
            self.native_shadow_applied = wants_shadow;
        }

        self.enforce_square_window_if_needed(ctx);
        self.handle_space_preview(ctx);

        if !self.is_transition_active() {
            self.handle_dropped_files(ctx);
        }

        if let Some(audio) = self.audio.as_mut() {
            audio.tick();
            if audio.has_active_playback() {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        if self.download_was_running
            && !download_snapshot.running
            && download_snapshot.last_file.is_some()
        {
            self.show_download_panel = true;
            self.stop_preview();
            self.clear_status();
        }
        self.download_was_running = download_snapshot.running;

        if download_snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        self.flush_pending_save(ctx);

        if let Some((TransitionPhase::Intro, progress)) = transition {
            self.render_transition_layer(ctx, progress, TransitionPhase::Intro);
            return;
        }

        if let Some((TransitionPhase::Outro, progress)) = transition {
            self.render_transition_layer(ctx, progress, TransitionPhase::Outro);
            return;
        }

        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(248, 247, 251))
                    .inner_margin(0.0),
            )
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgb(248, 247, 251))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(229, 220, 228)))
                    .shadow(Shadow {
                        offset: [0, 14],
                        blur: 30,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 20),
                    })
                    .corner_radius(CornerRadius::same(APP_FRAME_RADIUS as u8))
                    .outer_margin(Margin::same(APP_OUTER_MARGIN as i8))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        self.draw_titlebar(ui, ctx);
                        ui.add_space(14.0);

                        let content_height = ui.available_height();
                        if self.app_view == AppView::Library {
                            self.draw_library_grid(ui);
                        } else {
                            ui.horizontal_top(|ui| {
                                let library_width =
                                    (ui.available_width() * 0.34).clamp(258.0, 292.0);
                                ui.allocate_ui_with_layout(
                                    vec2(library_width, content_height),
                                    egui::Layout::top_down(Align::Min),
                                    |ui| self.draw_library(ui),
                                );
                                ui.add_space(12.0);
                                ui.allocate_ui_with_layout(
                                    vec2(ui.available_width(), content_height),
                                    egui::Layout::top_down(Align::Min),
                                    |ui| self.draw_editor(ui, ctx),
                                );
                            });
                        }
                    });
            });

        self.render_download_panel(ctx);
        self.render_import_panel(ctx);
        self.render_pitch_monitor(ctx);
        self.render_pitch_overlay_viewport(ctx);
        self.render_titlebar_drag_zone(ctx);
        self.render_custom_window_resize_handles(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.pitch_monitor.stop();
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }
    }
}

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            AUDIO_FILTERS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}
