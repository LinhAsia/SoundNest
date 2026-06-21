use super::*;

impl SoundFxApp {
    pub(super) fn render_modal_backdrop(&self, ctx: &Context) {
        if !self.has_modal_panel() {
            return;
        }

        let rect = self.app_frame_rect.unwrap_or_else(|| ctx.screen_rect());
        let corner_radius = if self.app_frame_rect.is_some() {
            CornerRadius::same(APP_FRAME_RADIUS as u8)
        } else {
            CornerRadius::ZERO
        };
        egui::Area::new(egui::Id::new("modal-backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (backdrop_rect, _) = ui.allocate_exact_size(rect.size(), Sense::click());
                ui.painter().rect_filled(
                    backdrop_rect,
                    corner_radius,
                    Color32::from_rgba_premultiplied(22, 16, 22, 132),
                );
            });
    }

    pub(super) fn modal_safe_rect(&self, ctx: &Context) -> Rect {
        let host_rect = self
            .app_frame_rect
            .filter(|rect| rect.width() > 1.0 && rect.height() > 1.0)
            .unwrap_or_else(|| ctx.screen_rect().shrink(18.0));
        host_rect.shrink2(vec2(APP_FRAME_RADIUS + 8.0, APP_FRAME_RADIUS + 8.0))
    }

    pub(super) fn fit_modal_dimension(available: f32, desired: f32, min: f32) -> f32 {
        if available <= 1.0 {
            1.0
        } else {
            desired.min(available).max(min.min(available))
        }
    }

    pub(super) fn centered_modal_placement(
        &self,
        ctx: &Context,
        desired_size: Vec2,
        min_size: Vec2,
        y_offset: f32,
    ) -> (Rect, Vec2, Pos2) {
        let safe_rect = self.modal_safe_rect(ctx);
        let panel_size = vec2(
            Self::fit_modal_dimension(safe_rect.width(), desired_size.x, min_size.x),
            Self::fit_modal_dimension(safe_rect.height(), desired_size.y, min_size.y),
        );
        let center = safe_rect.center();
        let panel_pos = Pos2::new(center.x.round(), (center.y + y_offset).round());
        (safe_rect, panel_size, panel_pos)
    }
}
