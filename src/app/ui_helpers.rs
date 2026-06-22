use super::*;

impl SoundFxApp {
    pub(crate) fn icon(codepoint: u32, size: f32, color: Color32) -> RichText {
        RichText::new(char::from_u32(codepoint).unwrap_or(' '))
            .family(FontFamily::Name(MATERIAL_ICONS_FONT.into()))
            .size(size)
            .color(color)
    }

    pub(crate) fn dark_theme_enabled() -> bool {
        DARK_THEME_ENABLED.load(Ordering::Relaxed)
    }

    pub(crate) fn set_theme_enabled(enabled: bool) {
        DARK_THEME_ENABLED.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn overlay_panel_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(26, 22, 31)
        } else {
            Color32::from_rgb(255, 251, 254)
        }
    }

    pub(crate) fn input_fill() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgb(31, 26, 37)
        } else {
            Color32::from_rgb(250, 246, 249)
        }
    }

    pub(crate) fn with_slider_visuals<R>(
        ui: &mut Ui,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let visuals = &mut ui.style_mut().visuals;
                let widgets = &mut visuals.widgets;
                widgets.inactive.bg_fill = Color32::from_rgb(244, 240, 247);
                widgets.inactive.bg_stroke.color = Color32::from_rgb(244, 240, 247);
                widgets.hovered.bg_fill = Color32::from_rgb(255, 248, 252);
                widgets.hovered.bg_stroke.color = Color32::from_rgb(255, 248, 252);
                widgets.active.bg_fill = Color32::from_rgb(255, 253, 254);
                widgets.active.bg_stroke.color = Color32::from_rgb(255, 253, 254);
                visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
                visuals.selection.stroke.color = Color32::WHITE;
                visuals.override_text_color = Some(Color32::from_rgb(28, 22, 31));
                visuals.extreme_bg_color = Color32::from_rgb(244, 240, 247);
            }
            add_contents(ui)
        })
        .inner
    }

    pub(crate) fn click_slider(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
    ) -> (egui::Response, bool) {
        Self::click_slider_impl(ui, value, range, step, size, false)
    }

    pub(crate) fn click_slider_deferred(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
    ) -> (egui::Response, bool) {
        Self::click_slider_impl(ui, value, range, step, size, true)
    }

    pub(crate) fn click_slider_impl(
        ui: &mut Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        size: Vec2,
        apply_on_release: bool,
    ) -> (egui::Response, bool) {
        let desired_size = vec2(size.x.max(48.0), size.y.max(20.0));
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
        let dark_theme = Self::dark_theme_enabled();
        let knob_radius = 8.0;
        let track_rect = Rect::from_center_size(
            rect.center(),
            vec2((rect.width() - knob_radius * 2.0).max(12.0), 8.0),
        );
        let min = *range.start();
        let max = *range.end();
        let span = (max - min).max(f32::EPSILON);
        let normalized = ((*value - min) / span).clamp(0.0, 1.0);
        let knob_x = egui::lerp(track_rect.left()..=track_rect.right(), normalized);
        let knob_center = Pos2::new(knob_x, track_rect.center().y);
        let track_fill = if dark_theme {
            Color32::from_rgb(244, 240, 247)
        } else {
            Color32::from_rgb(237, 231, 238)
        };
        let active_fill = Color32::from_rgb(227, 82, 149);
        let knob_fill = if response.hovered() {
            Color32::from_rgb(255, 248, 252)
        } else {
            Color32::WHITE
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(track_rect, 4.0, track_fill);
        painter.rect_filled(
            Rect::from_min_max(track_rect.min, Pos2::new(knob_x, track_rect.max.y)),
            4.0,
            active_fill,
        );
        painter.circle_filled(knob_center, knob_radius, knob_fill);
        painter.circle_stroke(knob_center, knob_radius, Stroke::new(1.5, active_fill));

        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let mut value_changed = false;
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let ratio = ((pointer.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
            let raw = min + span * ratio;
            let stepped = if step > f32::EPSILON {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            let next = stepped.clamp(min, max);
            *value = next;
            if !apply_on_release {
                value_changed = true;
            }
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let ratio = ((pointer.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
            let raw = min + span * ratio;
            let stepped = if step > f32::EPSILON {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            let next = stepped.clamp(min, max);
            *value = next;
            value_changed = true;
        }
        if apply_on_release && response.drag_stopped() {
            value_changed = true;
        }

        (response, value_changed)
    }

    pub(crate) fn deferred_drag_value_commit(ctx: &Context, response: &egui::Response) -> bool {
        let active_id = response.id.with("deferred-drag-active");
        if response.drag_started() || response.dragged() {
            ctx.data_mut(|data| data.insert_temp(active_id, true));
        }

        let mut commit = false;
        if response.drag_stopped() {
            commit = true;
        } else if ctx
            .data(|data| data.get_temp::<bool>(active_id))
            .unwrap_or(false)
            && !ctx.input(|input| input.pointer.primary_down())
        {
            commit = true;
        }

        let enter_commit =
            response.has_focus() && ctx.input(|input| input.key_pressed(egui::Key::Enter));
        if commit || response.lost_focus() || enter_commit {
            ctx.data_mut(|data| data.remove::<bool>(active_id));
        }

        commit || response.lost_focus() || enter_commit
    }

    pub(crate) fn paint_dashed_border(painter: &egui::Painter, rect: Rect, color: Color32) {
        let dash = 12.0;
        let gap = 8.0;
        let stroke = Stroke::new(1.5, color);
        let mut x = rect.left() + 12.0;
        while x < rect.right() - 12.0 {
            let end = (x + dash).min(rect.right() - 12.0);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(end, rect.top())],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(x, rect.bottom()), Pos2::new(end, rect.bottom())],
                stroke,
            );
            x += dash + gap;
        }
        let mut y = rect.top() + 12.0;
        while y < rect.bottom() - 12.0 {
            let end = (y + dash).min(rect.bottom() - 12.0);
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.left(), end)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(rect.right(), y), Pos2::new(rect.right(), end)],
                stroke,
            );
            y += dash + gap;
        }
    }

    pub(crate) fn reveal_in_file_explorer(path: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("explorer.exe");
            cmd.arg("/select,").arg(path);
            cmd.creation_flags(0x08000000);
            cmd.spawn()
                .with_context(|| format!("unable to reveal {}", path.display()))?;
            return Ok(());
        }
        #[allow(unreachable_code)]
        if let Some(parent) = path.parent() {
            open::that(parent).with_context(|| format!("unable to open {}", parent.display()))?;
        }
        Ok(())
    }

    pub(crate) fn with_dark_combo_visuals<R>(
        ui: &mut Ui,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        ui.scope(|ui| {
            if Self::dark_theme_enabled() {
                let visuals = &mut ui.style_mut().visuals;
                visuals.extreme_bg_color = Color32::from_rgb(28, 24, 33);
                visuals.faint_bg_color = Color32::from_rgb(33, 28, 39);
                visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.noninteractive.weak_bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(88, 70, 96);
                visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(28, 24, 33);
                visuals.widgets.inactive.bg_stroke.color = Color32::from_rgb(88, 70, 96);
                visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 35, 48);
                visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(43, 35, 48);
                visuals.widgets.hovered.bg_stroke.color = Color32::from_rgb(230, 94, 150);
                visuals.widgets.active.bg_fill = Color32::from_rgb(53, 41, 58);
                visuals.widgets.active.weak_bg_fill = Color32::from_rgb(53, 41, 58);
                visuals.widgets.active.bg_stroke.color = Color32::from_rgb(230, 94, 150);
                visuals.selection.bg_fill = Color32::from_rgb(227, 82, 149);
                visuals.selection.stroke.color = Color32::WHITE;
                visuals.override_text_color = Some(Color32::from_rgb(246, 233, 241));
            }
            add_contents(ui)
        })
        .inner
    }

    pub(crate) fn shadow_color() -> Color32 {
        if Self::dark_theme_enabled() {
            Color32::from_rgba_premultiplied(0, 0, 0, 60)
        } else {
            Color32::from_rgba_premultiplied(86, 43, 67, 22)
        }
    }
}
