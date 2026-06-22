use super::*;

impl SoundFxApp {
    pub(super) fn active_audio_tag_filter(&self) -> Option<&str> {
        if self.app_view == AppView::Library {
            self.library_audio_tag_filter.as_deref()
        } else {
            None
        }
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

    pub(super) fn sound_tag_matches_filter(tags: &[String], filter: Option<&str>) -> bool {
        match filter {
            Some(filter) => tags.iter().any(|tag| tag.eq_ignore_ascii_case(filter)),
            None => true,
        }
    }

    pub(super) fn has_sound_tag(&self, tag: &str) -> bool {
        self.sounds.iter().any(|sound| {
            sound
                .tags
                .iter()
                .any(|sound_tag| sound_tag.eq_ignore_ascii_case(tag))
        })
    }

    pub(super) fn reconcile_library_audio_tag_filter(&mut self) {
        let Some(active_filter) = self.library_audio_tag_filter.as_deref() else {
            return;
        };
        if !self.has_sound_tag(active_filter) {
            self.library_audio_tag_filter = None;
        }
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

    pub(super) fn tag_chip_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
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
        let response = ui.add(
            Button::new(RichText::new(label).size(11.5).color(text_color))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(999.0),
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    fn library_tag_toggle_label(&self) -> String {
        let active_tag_count = usize::from(self.library_audio_tag_filter.is_some());
        if self.library_audio_tags_expanded {
            format!(
                "{} Hide Tags",
                Self::icon(0xe5ce, 13.0, Self::strong_text_color()).text()
            )
        } else if active_tag_count > 0 {
            format!(
                "{} Tags ({active_tag_count})",
                Self::icon(0xe5cf, 13.0, Self::strong_text_color()).text()
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
        if Self::tag_chip_button(ui, &toggle_label, self.library_audio_tags_expanded).clicked() {
            self.library_audio_tags_expanded = !self.library_audio_tags_expanded;
        }
    }

    pub(super) fn draw_library_tag_filter_row(&mut self, ui: &mut Ui) {
        self.reconcile_library_audio_tag_filter();
        let tags = self.distinct_sound_tags();
        let active_filter = self.library_audio_tag_filter.clone();
        if !self.library_audio_tags_expanded || tags.is_empty() {
            return;
        }

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let all_active = active_filter.is_none();
            if Self::tag_chip_button(ui, &self.t("library.tag_all"), all_active).clicked() {
                self.library_audio_tag_filter = None;
            }

            for tag in tags {
                let active = active_filter.as_deref().is_some_and(|value| value == tag);
                if Self::tag_chip_button(ui, &tag, active).clicked() {
                    self.library_audio_tag_filter = if active { None } else { Some(tag) };
                }
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
