#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod audio;
mod downloader;
mod hotkey;
mod myinstants;
mod pitch;
mod platform;
mod record_video;
mod recorder;
mod storage;

use app::SoundFxApp;
use eframe::egui::{
    self, Align2, CentralPanel, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Margin, Pos2, Rect, RichText, Stroke, Style, TextStyle, ViewportCommand, Visuals, vec2,
};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
};
#[cfg(windows)]
use windows::core::PCWSTR;

const MATERIAL_ICONS_FONT: &str = "material_icons";
const UI_FONT: &str = "ui_font";
#[cfg(windows)]
const SINGLE_INSTANCE_MUTEX: &str = "Local\\SoundFxManagerSingleton";

#[cfg(windows)]
struct SingleInstanceGuard(HANDLE);

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn main() -> eframe::Result<()> {
    let mut args = env::args_os();
    let _ = args.next();
    if let Some(flag) = args.next()
        && flag == "--play-file-detached"
    {
        if let Some(path) = args.next() {
            let _ = audio::play_file_blocking(Path::new(&path));
        }
        return Ok(());
    }

    #[cfg(windows)]
    let _instance_guard = match try_acquire_single_instance() {
        Ok(Some(guard)) => Some(guard),
        Ok(None) => return run_already_running_notice(),
        Err(_) => None,
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sound FX")
            .with_inner_size([900.0, 900.0])
            .with_min_inner_size([720.0, 720.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_transparent(true)
            .with_position(initial_window_position([900.0, 900.0])),
        ..Default::default()
    };

    eframe::run_native(
        "Sound FX",
        native_options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_theme(&cc.egui_ctx);
            Ok(Box::new(SoundFxApp::new()))
        }),
    )
}

#[cfg(windows)]
fn try_acquire_single_instance() -> windows_core::Result<Option<SingleInstanceGuard>> {
    let mutex_name = SINGLE_INSTANCE_MUTEX
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr()))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        Ok(None)
    } else {
        Ok(Some(SingleInstanceGuard(handle)))
    }
}

#[cfg(windows)]
fn run_already_running_notice() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sound FX")
            .with_inner_size([420.0, 320.0])
            .with_min_inner_size([420.0, 320.0])
            .with_max_inner_size([420.0, 320.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_position(initial_window_position([420.0, 320.0]))
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "Sound FX",
        native_options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_theme(&cc.egui_ctx);
            Ok(Box::new(AlreadyRunningNoticeApp::default()))
        }),
    )
}

#[cfg(windows)]
struct AlreadyRunningNoticeApp {
    started_at: Option<f64>,
    centered_on_screen: bool,
}

#[cfg(windows)]
impl Default for AlreadyRunningNoticeApp {
    fn default() -> Self {
        Self {
            started_at: None,
            centered_on_screen: false,
        }
    }
}

#[cfg(windows)]
impl AlreadyRunningNoticeApp {
    fn squircle_points(center: Pos2, half_w: f32, half_h: f32, time: f32) -> Vec<Pos2> {
        let mut points = Vec::with_capacity(96);
        for step in 0..96 {
            let angle = step as f32 / 96.0 * std::f32::consts::TAU;
            let cos = angle.cos();
            let sin = angle.sin();
            let power = 2.0 / 3.1;
            let x = cos.signum() * cos.abs().powf(power) * half_w;
            let y = sin.signum() * sin.abs().powf(power) * half_h;
            let drift = 1.0
                + 0.055 * (angle * 3.0 + time * 1.4).sin()
                + 0.026 * (angle * 5.0 - time * 1.1).cos();
            points.push(Pos2::new(center.x + x * drift, center.y + y * drift));
        }
        points
    }

    fn paint_waving_hand(
        painter: &egui::Painter,
        wrist: Pos2,
        scale: f32,
        time: f32,
        color: Color32,
        stroke: Color32,
    ) {
        let wave = (time * 4.0).sin() * 0.42;
        let arm_end = Pos2::new(wrist.x + 34.0 * scale, wrist.y - 20.0 * scale);
        let palm = Pos2::new(
            arm_end.x + 18.0 * scale * wave.cos(),
            arm_end.y - 8.0 * scale + 12.0 * scale * wave.sin(),
        );

        painter.line_segment([wrist, arm_end], Stroke::new(12.0 * scale, color));
        painter.line_segment([wrist, arm_end], Stroke::new(1.8 * scale, stroke));
        painter.circle_filled(palm, 14.0 * scale, color);
        painter.circle_stroke(palm, 14.0 * scale, Stroke::new(1.8 * scale, stroke));

        for (index, spread) in [-0.88_f32, -0.34, 0.08, 0.48].into_iter().enumerate() {
            let angle = -1.55 + spread + wave * 0.32;
            let finger_len = (18.0 + index as f32 * 2.5) * scale;
            let finger_end = Pos2::new(
                palm.x + angle.cos() * finger_len,
                palm.y + angle.sin() * finger_len,
            );
            painter.line_segment([palm, finger_end], Stroke::new(6.5 * scale, color));
            painter.line_segment([palm, finger_end], Stroke::new(1.2 * scale, stroke));
        }

        let thumb_angle = 2.5 + wave * 0.24;
        let thumb_end = Pos2::new(
            palm.x + thumb_angle.cos() * 14.0 * scale,
            palm.y + thumb_angle.sin() * 14.0 * scale,
        );
        painter.line_segment([palm, thumb_end], Stroke::new(5.4 * scale, color));
        painter.line_segment([palm, thumb_end], Stroke::new(1.0 * scale, stroke));
    }
}

#[cfg(windows)]
impl eframe::App for AlreadyRunningNoticeApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|input| input.time);
        let started_at = self.started_at.get_or_insert(now);
        let elapsed = (now - *started_at) as f32;
        if elapsed >= 2.1 {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if !self.centered_on_screen {
            if let Some(center_cmd) = ViewportCommand::center_on_screen(ctx) {
                ctx.send_viewport_cmd(center_cmd);
            }
            self.centered_on_screen = true;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        CentralPanel::default()
            .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(0.0))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let painter = ui.painter_at(rect);
                let center = rect.center();
                let aura = (1.0 - (elapsed / 2.1)).clamp(0.0, 1.0);
                painter.circle_filled(
                    center,
                    116.0,
                    Color32::from_rgba_premultiplied(227, 82, 149, (26.0 + aura * 44.0) as u8),
                );
                painter.circle_filled(
                    Pos2::new(center.x, center.y + 6.0),
                    88.0,
                    Color32::from_rgba_premultiplied(255, 212, 233, (18.0 + aura * 30.0) as u8),
                );

                let shadow = Self::squircle_points(
                    Pos2::new(center.x, center.y + 10.0),
                    110.0,
                    88.0,
                    elapsed - 0.25,
                );
                painter.add(egui::Shape::convex_polygon(
                    shadow,
                    Color32::from_rgba_premultiplied(47, 18, 38, 52),
                    Stroke::NONE,
                ));

                let blob = Self::squircle_points(center, 102.0, 82.0, elapsed);
                painter.add(egui::Shape::convex_polygon(
                    blob,
                    Color32::from_rgba_premultiplied(241, 134, 186, 246),
                    Stroke::new(1.4, Color32::from_rgb(231, 151, 188)),
                ));

                let glaze = Self::squircle_points(
                    Pos2::new(center.x, center.y - 22.0),
                    82.0,
                    34.0,
                    elapsed + 0.7,
                );
                painter.add(egui::Shape::convex_polygon(
                    glaze,
                    Color32::from_rgba_premultiplied(255, 255, 255, 72),
                    Stroke::NONE,
                ));

                let eye_y = center.y - 14.0;
                for x in [center.x - 22.0, center.x + 22.0] {
                    painter.circle_filled(Pos2::new(x, eye_y), 5.8, Color32::from_rgb(111, 53, 84));
                }

                let smile_rect = Rect::from_center_size(
                    Pos2::new(center.x - 8.0, center.y + 8.0),
                    vec2(42.0, 22.0),
                );
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [
                        Pos2::new(smile_rect.left(), smile_rect.center().y - 1.0),
                        Pos2::new(smile_rect.left() + 10.0, smile_rect.bottom() + 6.0),
                        Pos2::new(smile_rect.right() - 8.0, smile_rect.bottom() + 5.0),
                        Pos2::new(smile_rect.right(), smile_rect.center().y - 5.0),
                    ],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(2.3, Color32::from_rgb(187, 90, 137)),
                ));

                Self::paint_waving_hand(
                    &painter,
                    Pos2::new(center.x + 60.0, center.y + 6.0),
                    1.0,
                    elapsed,
                    Color32::from_rgb(255, 242, 248),
                    Color32::from_rgb(225, 152, 188),
                );

                egui::Area::new(egui::Id::new("already-open-message"))
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 148.0))
                    .show(ctx, |ui| {
                        Frame::new()
                            .fill(Color32::from_rgba_premultiplied(255, 250, 252, 236))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(235, 192, 215)))
                            .corner_radius(28.0)
                            .inner_margin(Margin::same(18))
                            .show(ui, |ui| {
                                ui.set_width(300.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("Sound FX is already open.")
                                            .size(18.0)
                                            .color(Color32::from_rgb(76, 35, 59))
                                            .strong(),
                                    );
                                });
                            });
                    });
            });
    }
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Ok(bytes) = fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts
            .font_data
            .insert(UI_FONT.to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, UI_FONT.to_owned());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, UI_FONT.to_owned());
    }
    fonts.font_data.insert(
        MATERIAL_ICONS_FONT.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/MaterialIcons-Regular.ttf"
        ))),
    );
    let material_family = FontFamily::Name(MATERIAL_ICONS_FONT.into());
    fonts
        .families
        .entry(material_family)
        .or_default()
        .insert(0, MATERIAL_ICONS_FONT.to_owned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push(MATERIAL_ICONS_FONT.to_owned());
    ctx.set_fonts(fonts);
}

#[cfg(windows)]
fn initial_window_position([width, height]: [f32; 2]) -> Pos2 {
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN).max(0) as f32;
        let screen_h = GetSystemMetrics(SM_CYSCREEN).max(0) as f32;
        Pos2::new(
            ((screen_w - width) * 0.5).max(0.0),
            ((screen_h - height) * 0.5).max(0.0),
        )
    }
}

#[cfg(not(windows))]
fn initial_window_position([_width, _height]: [f32; 2]) -> Pos2 {
    Pos2::ZERO
}

fn configure_theme(ctx: &egui::Context) {
    let mut style: Style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 12.0);
    style.spacing.button_padding = egui::vec2(14.0, 10.0);
    style.spacing.indent = 18.0;
    style.spacing.slider_width = 260.0;
    style.visuals = Visuals::light();

    style.visuals.panel_fill = Color32::from_rgb(248, 247, 251);
    style.visuals.window_fill = Color32::from_rgb(248, 247, 251);
    style.visuals.extreme_bg_color = Color32::from_rgb(255, 255, 255);
    style.visuals.faint_bg_color = Color32::from_rgb(244, 239, 246);
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(255, 255, 255);
    style.visuals.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(229, 220, 228);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(255, 255, 255);
    style.visuals.widgets.inactive.bg_stroke.color = Color32::from_rgb(226, 216, 225);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(255, 236, 246);
    style.visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(255, 223, 239);
    style.visuals.widgets.active.bg_stroke.color = Color32::from_rgb(214, 51, 132);
    style.visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
    style.visuals.selection.stroke.color = Color32::WHITE;
    style.visuals.hyperlink_color = Color32::from_rgb(214, 51, 132);
    style.visuals.window_shadow.color = Color32::from_rgba_premultiplied(68, 27, 56, 48);

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(28.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Name("title".into()),
            FontId::new(18.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
    ]
    .into();

    ctx.set_style(style);
}
