use super::*;

impl SoundFxApp {
    pub(super) fn render_import_panel(&mut self, ctx: &Context) {
        if !self.show_import_panel {
            return;
        }

        let mut open_panel = self.show_import_panel;
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
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(640.0, 560.0), vec2(320.0, 280.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("sound-import-panel"))
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
                        blur: 30,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe2c8, 20.0, Self::strong_text_color()).strong());
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
                        .fill(Self::surface_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(18.0)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.add_sized(
                                [ui.available_width().max(120.0), 18.0],
                                egui::Label::new(
                                    RichText::new(path_label.as_str())
                                        .size(13.0)
                                        .color(Self::muted_text_color()),
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
                                    .fill(Self::surface_fill())
                                    .stroke(Stroke::new(1.0, Self::border_color()))
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
                                                        .color(Self::strong_text_color()),
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

                        for path in &self.import_audio_entries {
                            let is_dir = path.is_dir();
                            let label = path
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or(if is_dir { "folder" } else { "sound" });
                            let row = Frame::new()
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(22.0)
                                .inner_margin(Margin::symmetric(16, 14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(Self::icon(
                                            if is_dir { 0xe2c8 } else { 0xeb82 },
                                            18.0,
                                            Color32::from_rgb(214, 51, 132),
                                        ));
                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [ui.available_width(), 18.0],
                                            egui::Label::new(
                                                RichText::new(label)
                                                    .size(14.0)
                                                    .color(Self::strong_text_color()),
                                            )
                                            .truncate(),
                                        );
                                    });
                                });
                            let response =
                                ui.interact(row.response.rect, ui.id().with(path), Sense::click());
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
        if go_up && let Some(parent) = self.import_dir.parent() {
            self.set_import_dir(Some(parent.to_path_buf()));
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
}
