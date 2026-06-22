#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragGhostKind {
    Sound,
    Folder,
}

#[derive(Clone, Debug)]
pub struct DragGhostSpec {
    pub kind: DragGhostKind,
    pub waveform: Vec<f32>,
    pub dark_theme: bool,
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
pub fn set_native_window_shadow(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn set_native_window_topmost(_frame: &eframe::Frame, _enabled: bool) {}

#[cfg(not(windows))]
pub fn hide_native_window_by_title(_title: &str) {}

#[cfg(not(windows))]
pub fn show_native_window(_frame: &eframe::Frame) {}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn set_overlay_window_native_visuals(
    _window_title: &str,
    _enabled: bool,
    _popup_only: bool,
) -> bool {
    false
}

#[cfg(not(windows))]
pub fn drag_file_out(
    _path: &std::path::Path,
    _ghost: Option<&DragGhostSpec>,
) -> anyhow::Result<()> {
    anyhow::bail!("Drag out is only available on Windows")
}

#[cfg(not(windows))]
pub fn cursor_screen_position() -> Option<eframe::egui::Pos2> {
    None
}

#[cfg(not(windows))]
pub fn cursor_window_position(_window_title: &str) -> Option<eframe::egui::Pos2> {
    None
}
