use super::*;

impl SoundFxApp {
    pub(super) fn titlebar_button(label: RichText, active: bool, danger: bool) -> Button<'static> {
        let (fill, stroke) = if danger {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(32, 26, 38)
                } else {
                    Color32::WHITE
                },
                Color32::from_rgb(230, 94, 150),
            )
        } else if active {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgba_premultiplied(118, 31, 82, 210)
                } else {
                    Color32::from_rgba_premultiplied(229, 85, 149, 118)
                },
                Color32::from_rgb(214, 51, 132),
            )
        } else {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgba_premultiplied(41, 34, 47, 224)
                } else {
                    Color32::from_rgba_premultiplied(237, 231, 238, 198)
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(84, 69, 92)
                } else {
                    Color32::from_rgb(221, 212, 222)
                },
            )
        };

        Button::new(label.strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(9.0)
    }

    pub(super) fn action_button_with_radius(
        label: RichText,
        active: bool,
        accent: bool,
        radius: u8,
    ) -> Button<'static> {
        let (fill, stroke, text) = if accent {
            (
                Color32::from_rgb(214, 51, 132),
                Color32::from_rgb(214, 51, 132),
                Color32::WHITE,
            )
        } else if active {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(66, 30, 60)
                } else {
                    Color32::from_rgb(255, 231, 243)
                },
                Color32::from_rgb(230, 94, 150),
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(255, 222, 240)
                } else {
                    Color32::from_rgb(120, 22, 72)
                },
            )
        } else {
            (
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(29, 25, 35)
                } else {
                    Color32::WHITE
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(83, 69, 92)
                } else {
                    Color32::from_rgb(227, 217, 226)
                },
                if Self::dark_theme_enabled() {
                    Color32::from_rgb(243, 230, 239)
                } else {
                    Color32::from_rgb(60, 54, 61)
                },
            )
        };

        Button::new(label.color(text))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(radius)
    }

    pub(super) fn action_button(label: RichText, active: bool, accent: bool) -> Button<'static> {
        Self::action_button_with_radius(label, active, accent, 18)
    }

    pub(super) fn icon_action_with_radius(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        accent: bool,
        radius: u8,
    ) -> egui::Response {
        let icon_color = if accent || active {
            Color32::WHITE
        } else {
            Self::strong_text_color()
        };
        let response = ui.add_sized(
            size,
            Self::action_button_with_radius(
                Self::icon(codepoint, 18.0, icon_color),
                active,
                accent,
                radius,
            ),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn icon_action(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        accent: bool,
    ) -> egui::Response {
        Self::icon_action_with_radius(ui, size, codepoint, active, accent, 18)
    }

    pub(super) fn icon_titlebar(
        ui: &mut Ui,
        size: [f32; 2],
        codepoint: u32,
        active: bool,
        danger: bool,
    ) -> egui::Response {
        let response = ui.add_sized(
            size,
            Self::titlebar_button(
                Self::icon(codepoint, 18.0, Self::strong_text_color()),
                active,
                danger,
            ),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn paint_theme_titlebar_icon(painter: &egui::Painter, rect: Rect, active: bool) {
        let center = rect.center();
        let icon_color = Self::strong_text_color();

        if active {
            let moon_fill = if Self::dark_theme_enabled() {
                Color32::from_rgb(246, 233, 241)
            } else {
                Color32::from_rgb(245, 240, 246)
            };
            let cutout = if Self::dark_theme_enabled() {
                Color32::from_rgba_premultiplied(118, 31, 82, 210)
            } else {
                Color32::from_rgba_premultiplied(229, 85, 149, 118)
            };
            painter.circle_filled(center, 6.0, moon_fill);
            painter.circle_filled(Pos2::new(center.x + 3.0, center.y - 2.0), 6.0, cutout);
        } else {
            painter.circle_stroke(center, 5.0, Stroke::new(1.5, icon_color));
            for (dx, dy) in [
                (0.0, -8.0),
                (5.8, -5.8),
                (8.0, 0.0),
                (5.8, 5.8),
                (0.0, 8.0),
                (-5.8, 5.8),
                (-8.0, 0.0),
                (-5.8, -5.8),
            ] {
                let start = Pos2::new(center.x + dx * 0.62, center.y + dy * 0.62);
                let end = Pos2::new(center.x + dx, center.y + dy);
                painter.line_segment([start, end], Stroke::new(1.3, icon_color));
            }
        }
    }

    pub(super) fn decorate_button_response(ui: &Ui, response: &egui::Response) {
        let pointer_over = ui
            .ctx()
            .input(|input| input.pointer.latest_pos().or(input.pointer.hover_pos()))
            .is_some_and(|pos| response.rect.contains(pos));
        if pointer_over {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            Self::paint_hover_button_notes(
                ui.painter(),
                response.rect,
                ui.input(|input| input.time) as f32,
            );
        }
    }

    pub(super) fn paint_hover_button_notes(painter: &egui::Painter, rect: Rect, time: f32) {
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
}
