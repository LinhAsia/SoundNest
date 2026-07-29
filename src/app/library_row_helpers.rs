use super::*;

impl SoundFxApp {
    pub(super) fn visible_folder_sound_count(
        &mut self,
        folder_id: Option<Uuid>,
        total: usize,
        ctx: &Context,
    ) -> usize {
        if total == 0 {
            self.library_folder_visible_sound_counts.remove(&folder_id);
            return 0;
        }

        let batch = if self.library_sound_view == LibrarySoundView::Grid {
            8usize
        } else {
            6usize
        };
        let entry = self
            .library_folder_visible_sound_counts
            .entry(folder_id)
            .or_insert_with(|| total.min(batch));
        *entry = (*entry).min(total);
        let visible = *entry;
        if visible < total {
            *entry = (visible + batch).min(total);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        visible
    }

    pub(super) fn draw_folder_loading_hint(&self, ui: &mut Ui, visible: usize, total: usize) {
        if visible >= total {
            return;
        }
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(
                RichText::new(format!("Loading sounds... {visible}/{total}"))
                    .size(11.5)
                    .color(Self::muted_text_color()),
            );
        });
    }

    pub(super) fn library_row_gap(&self) -> f32 {
        if self.library_row_thickness <= 2 {
            8.0
        } else {
            10.0
        }
    }

    pub(super) fn inline_folder_sound_row_height(&self) -> f32 {
        match self
            .library_row_thickness
            .clamp(LIBRARY_ROW_MIN_THICKNESS, LIBRARY_ROW_MAX_THICKNESS)
        {
            1 => 60.0,
            2 => 64.0,
            3 => 68.0,
            4 => 74.0,
            _ => 80.0,
        }
    }

    pub(super) fn inline_folder_sound_row_outer_height(&self) -> f32 {
        self.inline_folder_sound_row_height() + self.library_row_gap()
    }

    pub(super) fn draw_inline_folder_sound_rows_content(
        &mut self,
        ui: &mut Ui,
        sounds: &[SoundEffect],
    ) {
        if sounds.is_empty() {
            return;
        }

        ui.set_width(ui.available_width());
        ui.set_min_width(ui.available_width());
        for (index, sound) in sounds.iter().enumerate() {
            self.draw_inline_folder_sound_row(ui, sound);
            if index + 1 == sounds.len() {
                ui.add_space(self.library_row_gap());
            }
        }
    }

    pub(super) fn draw_inline_folder_sound_rows(&mut self, ui: &mut Ui, sounds: &[SoundEffect]) {
        if sounds.is_empty() {
            return;
        }

        let row_height = self.inline_folder_sound_row_outer_height();
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let first_row = (viewport.top() / row_height).floor().max(0.0) as usize;
                let last_row = ((viewport.bottom() / row_height).ceil() as usize)
                    .saturating_add(1)
                    .min(sounds.len());
                ui.set_min_height(row_height * sounds.len() as f32);
                ui.add_space(first_row as f32 * row_height);
                for sound in &sounds[first_row.min(sounds.len())..last_row] {
                    self.draw_inline_folder_sound_row(ui, sound);
                }
            });
    }

    fn external_drop_preview_details(&self, ctx: &Context) -> Option<(String, bool, usize)> {
        let hovered_files = ctx.input(|input| input.raw.hovered_files.clone());
        if hovered_files.is_empty() {
            return None;
        }

        let first_path = hovered_files.first().and_then(|file| file.path.as_ref())?;
        let label = first_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "Imported item".to_owned());
        let is_folder = first_path.is_dir();
        Some((label, is_folder, hovered_files.len()))
    }

    pub(super) fn draw_external_drop_preview_row(
        &mut self,
        ui: &mut egui::Ui,
        depth: usize,
        parent_folder_id: Option<Uuid>,
    ) {
        let Some((label, is_folder, item_count)) = self.external_drop_preview_details(ui.ctx())
        else {
            return;
        };

        let indent = 18.0 * depth as f32;
        let accent = Color32::from_rgb(242, 140, 56);
        let accent_soft = Color32::from_rgb(255, 202, 145);
        let detail = if is_folder {
            if item_count > 1 {
                format!("Will be added here with {} dragged items", item_count)
            } else {
                "Will be added here as a child folder".to_owned()
            }
        } else if item_count > 1 {
            format!("{} audio items will be imported here", item_count)
        } else {
            "Will be imported here".to_owned()
        };
        let prefix = parent_folder_id
            .map(|folder_id| self.folder_path_label(folder_id))
            .unwrap_or_else(|| "Root".to_owned());

        ui.horizontal(|ui| {
            ui.add_space(indent);
            Frame::new()
                .fill(Color32::from_rgba_premultiplied(242, 140, 56, 28))
                .stroke(Stroke::new(1.5, accent))
                .corner_radius(16.0)
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Self::icon(0xe5c8, 18.0, accent));
                        ui.add_space(8.0);
                        ui.label(Self::icon(
                            if is_folder { 0xe2c8 } else { 0xe061 },
                            18.0,
                            accent,
                        ));
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(label)
                                        .size(13.2)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                )
                                .truncate(),
                            );
                            ui.label(
                                RichText::new(format!("{detail} in {prefix}"))
                                    .size(11.0)
                                    .color(accent_soft),
                            );
                        });
                    });
                });
        });
    }
}
