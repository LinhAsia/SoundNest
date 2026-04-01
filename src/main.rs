#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod audio;
mod downloader;
mod pitch;
mod platform;
mod storage;

use app::SoundFxApp;
use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Style, TextStyle, Visuals,
};
use std::sync::Arc;

const MATERIAL_ICONS_FONT: &str = "material_icons";

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sound FX")
            .with_inner_size([900.0, 900.0])
            .with_min_inner_size([720.0, 720.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_transparent(true),
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

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
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
