use super::*;

impl SoundFxApp {
    pub(super) fn active_audio_tag_filters(&self) -> &[String] {
        &self.library_audio_tag_filters
    }

    pub(super) fn mark_sound_copied(&mut self, ctx: &Context, sound_id: Uuid) {
        self.copied_sound_feedback_until
            .insert(sound_id, ctx.input(|input| input.time) + 1.15);
    }

    pub(super) fn mark_video_copied(&mut self, ctx: &Context, video_id: Uuid) {
        self.copied_video_feedback_until
            .insert(video_id, ctx.input(|input| input.time) + 1.15);
    }

    pub(super) fn sound_copy_feedback_active(&self, ctx: &Context, sound_id: Uuid) -> bool {
        self.copied_sound_feedback_until
            .get(&sound_id)
            .is_some_and(|until| *until > ctx.input(|input| input.time))
    }

    pub(super) fn video_copy_feedback_active(&self, ctx: &Context, video_id: Uuid) -> bool {
        self.copied_video_feedback_until
            .get(&video_id)
            .is_some_and(|until| *until > ctx.input(|input| input.time))
    }

    pub(super) fn prune_copy_feedback(&mut self, ctx: &Context) {
        let now = ctx.input(|input| input.time);
        self.copied_sound_feedback_until
            .retain(|_, until| *until > now);
        self.copied_video_feedback_until
            .retain(|_, until| *until > now);
    }

    pub(super) fn toggle_sound_favorite(&mut self, sound_id: Uuid, ctx: &Context) {
        if let Some(sound) = self.sounds.iter_mut().find(|sound| sound.id == sound_id) {
            sound.favorite = !sound.favorite;
            self.mark_dirty(ctx);
        }
    }

    pub(super) fn toggle_video_favorite(&mut self, video_id: Uuid) {
        if let Some(video) = self
            .video_assets
            .iter_mut()
            .find(|video| video.id == video_id)
        {
            video.favorite = !video.favorite;
            let _ = self.storage.save_video_library(&self.video_assets);
        }
    }

    pub(super) fn favorite_button_sized(
        ui: &mut Ui,
        active: bool,
        size: [f32; 2],
        icon_size: f32,
    ) -> egui::Response {
        let fill = if active {
            Color32::from_rgb(247, 191, 64)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(29, 25, 35)
        } else {
            Color32::WHITE
        };
        let stroke = if active {
            Color32::from_rgb(247, 191, 64)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(83, 69, 92)
        } else {
            Color32::from_rgb(227, 217, 226)
        };
        let icon = if active { 0xe838 } else { 0xe83a };
        let icon_color = if active {
            Color32::from_rgb(60, 48, 12)
        } else {
            Self::strong_text_color()
        };
        let response = ui.add_sized(
            size,
            Button::new(Self::icon(icon, icon_size, icon_color))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(16.0),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn library_query_matches(name: &str, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        name.to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
    }

    pub(super) fn normalize_tag(tag: &str) -> Option<String> {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() { None } else { Some(tag) }
    }

    pub(super) fn parse_tags(text: &str) -> Vec<String> {
        let mut tags = Vec::new();
        let mut seen = HashSet::new();
        for raw_tag in text.split([',', ';', '\n']) {
            let Some(tag) = Self::normalize_tag(raw_tag) else {
                continue;
            };
            if seen.insert(tag.clone()) {
                tags.push(tag);
            }
        }
        tags
    }

    pub(super) fn join_tags(tags: &[String]) -> String {
        tags.join(", ")
    }

    pub(super) fn sound_tag_matches_filters(tags: &[String], filters: &[String]) -> bool {
        filters.iter().all(|filter| {
            tags.iter()
                .any(|tag| tag.eq_ignore_ascii_case(filter))
        })
    }

    pub(super) fn reconcile_library_audio_tag_filter(&mut self) {
        let available = self.distinct_sound_tags();
        self.library_audio_tag_filters.retain(|active| {
            available
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(active))
        });
    }

    pub(super) fn library_sound_query_matches(sound: &SoundEffect, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        let query = query.to_ascii_lowercase();
        sound.name.to_ascii_lowercase().contains(&query)
            || sound
                .tags
                .iter()
                .any(|tag| tag.to_ascii_lowercase().contains(&query))
    }

    pub(super) fn distinct_sound_tags(&self) -> Vec<String> {
        let mut tags = self
            .sounds
            .iter()
            .flat_map(|sound| sound.tags.iter().cloned())
            .collect::<Vec<_>>();
        tags.sort_unstable_by_key(|tag| tag.to_ascii_lowercase());
        let mut deduped = Vec::new();
        let mut seen = HashSet::new();
        for tag in tags {
            let key = tag.to_ascii_lowercase();
            if seen.insert(key) {
                deduped.push(tag);
            }
        }
        deduped
    }

    fn tag_chip<'a>(label: &'a str, active: bool) -> Button<'a> {
        let fill = if active {
            Color32::from_rgb(227, 82, 149)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgba_premultiplied(41, 34, 47, 224)
        } else {
            Color32::from_rgba_premultiplied(237, 231, 238, 198)
        };
        let stroke = if active {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(84, 69, 92)
        } else {
            Color32::from_rgb(221, 212, 222)
        };
        let text_color = if active {
            Color32::WHITE
        } else {
            Self::strong_text_color()
        };
        Button::new(RichText::new(label).size(11.5).color(text_color))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(999.0)
    }

    pub(super) fn tag_chip_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
        let response = ui.add(Self::tag_chip(label, active));
        Self::decorate_button_response(ui, &response);
        response
    }

    fn tag_chip_button_sized(
        ui: &mut Ui,
        label: &str,
        active: bool,
        size: Vec2,
    ) -> egui::Response {
        let response = ui.add_sized(size, Self::tag_chip(label, active));
        Self::decorate_button_response(ui, &response);
        response
    }

    fn library_tag_toggle_label(&self) -> String {
        if self.library_audio_tags_expanded {
            format!(
                "{} Hide Tags",
                Self::icon(0xe5ce, 13.0, Self::strong_text_color()).text()
            )
        } else {
            format!(
                "{} Tags",
                Self::icon(0xe5cf, 13.0, Self::strong_text_color()).text()
            )
        }
    }

    pub(super) fn draw_library_tag_toggle(&mut self, ui: &mut Ui) {
        let toggle_label = self.library_tag_toggle_label();
        let selected_count = self.library_audio_tag_filters.len();
        let response = Self::tag_chip_button(
            ui,
            &toggle_label,
            self.library_audio_tags_expanded || selected_count > 0,
        );
        if selected_count > 0 {
            let badge_center = Pos2::new(response.rect.right() - 2.0, response.rect.top() + 2.0);
            ui.painter()
                .circle_filled(badge_center, 8.0, Color32::from_rgb(247, 191, 64));
            ui.painter().text(
                badge_center,
                Align2::CENTER_CENTER,
                selected_count,
                FontId::proportional(10.0),
                Color32::from_rgb(55, 39, 8),
            );
        }
        if response.clicked() {
            self.library_audio_tags_expanded = !self.library_audio_tags_expanded;
        }
    }

    pub(super) fn draw_library_tag_filter_row(&mut self, ui: &mut Ui) {
        self.reconcile_library_audio_tag_filter();
        let tags = self.distinct_sound_tags();
        let active_filters = self.library_audio_tag_filters.clone();
        if !self.library_audio_tags_expanded || tags.is_empty() {
            return;
        }

        let panel_width = ui.available_width().max(160.0);
        let columns = if panel_width >= 520.0 {
            4
        } else if panel_width >= 340.0 {
            3
        } else {
            2
        };
        let column_gap = 8.0;
        let cell_width = ((panel_width - 14.0 - column_gap * (columns - 1) as f32)
            / columns as f32)
            .max(64.0);
        let item_count = tags.len() + 1;
        let row_count = item_count.div_ceil(columns);
        let all_label = self.t("library.tag_all");

        ScrollArea::vertical()
            .id_salt("library-tag-filter-scroll")
            .max_height(176.0)
            .auto_shrink([false, true])
            .show_rows(ui, 32.0, row_count, |ui, row_range| {
                ui.set_min_width(panel_width);
                for row in row_range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = column_gap;
                        for column in 0..columns {
                            let item_index = row * columns + column;
                            if item_index >= item_count {
                                break;
                            }
                            if item_index == 0 {
                                if Self::tag_chip_button_sized(
                                    ui,
                                    &all_label,
                                    active_filters.is_empty(),
                                    vec2(cell_width, 24.0),
                                )
                                .clicked()
                                {
                                    self.library_audio_tag_filters.clear();
                                }
                                continue;
                            }

                            let tag = &tags[item_index - 1];
                            let active = active_filters
                                .iter()
                                .any(|value| value.eq_ignore_ascii_case(tag));
                            if Self::tag_chip_button_sized(
                                ui,
                                tag,
                                active,
                                vec2(cell_width, 24.0),
                            )
                            .clicked()
                            {
                                if active {
                                    self.library_audio_tag_filters
                                        .retain(|value| !value.eq_ignore_ascii_case(tag));
                                } else {
                                    self.library_audio_tag_filters.push(tag.clone());
                                }
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
            });
    }

    pub(super) fn draw_sound_tag_picker(
        ui: &mut Ui,
        tags: &[String],
        current_tags: &mut String,
    ) -> bool {
        if tags.is_empty() {
            return false;
        }

        let selected = Self::parse_tags(current_tags);
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for tag in tags {
                let active = selected.iter().any(|value| value.eq_ignore_ascii_case(tag));
                if Self::tag_chip_button(ui, tag, active).clicked() {
                    Self::apply_tag_to_input(current_tags, tag, active);
                    changed = true;
                }
            }
        });
        changed
    }

    pub(super) fn apply_tag_to_input(input: &mut String, tag: &str, active: bool) {
        let mut tags = Self::parse_tags(input);
        let Some(normalized) = Self::normalize_tag(tag) else {
            return;
        };
        if active {
            tags.retain(|value| !value.eq_ignore_ascii_case(&normalized));
        } else if !tags
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&normalized))
        {
            tags.push(normalized);
        }
        *input = Self::join_tags(&tags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_filter_requires_every_selected_tag() {
        let tags = vec!["meme".to_owned(), "short".to_owned(), "funny".to_owned()];

        assert!(SoundFxApp::sound_tag_matches_filters(&tags, &[]));
        assert!(SoundFxApp::sound_tag_matches_filters(
            &tags,
            &["MEME".to_owned(), "short".to_owned()]
        ));
        assert!(!SoundFxApp::sound_tag_matches_filters(
            &tags,
            &["meme".to_owned(), "music".to_owned()]
        ));
    }
}
