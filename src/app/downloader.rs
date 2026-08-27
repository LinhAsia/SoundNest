use super::*;

impl SoundFxApp {
    pub(super) fn save_tts_draft_preferences(&mut self) {
        let draft = crate::storage::GeminiTtsDraftPreferences {
            text: self.tts_text.clone(),
            voice_name: self.tts_voice_name.clone(),
            direction_prompt: self.tts_direction_prompt.clone(),
            output_name: self.tts_output_name.clone(),
        };
        let _ = self.storage.save_tts_draft(&draft);
    }

    pub(super) fn start_tts_generation(&mut self) {
        if self.tts_running {
            return;
        }
        let api_key = self.gemini_api_key.trim().to_owned();
        if api_key.is_empty() {
            self.tts_error = Some(self.t("download.gemini_api_key_missing"));
            return;
        }
        let text = self.tts_text.trim().to_owned();
        if text.is_empty() {
            self.tts_error = Some(self.t("download.text_empty"));
            return;
        }
        let voice = self.tts_voice_name.trim().to_owned();
        let direction_prompt = self.tts_direction_prompt.trim().to_owned();
        let output_name = self.tts_output_name.trim().to_owned();
        let out_dir = self.storage.root_dir().join("gemini-tts");
        let tx = self.tts_tx.clone();
        self.tts_running = true;
        self.tts_status = "Generating".to_owned();
        self.tts_error = None;
        self.tts_last_file = None;
        self.tts_can_add_to_library = false;
        self.tts_added_to_library = false;
        self.save_tts_draft_preferences();
        thread::spawn(move || {
            let result = gemini_tts::generate_speech_to_file(
                &api_key,
                &text,
                &voice,
                &direction_prompt,
                &out_dir,
                if output_name.is_empty() {
                    "gemini tts"
                } else {
                    &output_name
                },
            )
            .map(|path| GeminiTtsResult {
                path,
                display_name: if output_name.is_empty() {
                    "gemini tts".to_owned()
                } else {
                    output_name
                },
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(GeminiTtsMessage::Finished(result));
        });
    }

    pub(super) fn selected_tts_preset_name(&self) -> Option<&str> {
        self.tts_prompt_presets
            .iter()
            .find(|preset| preset.prompt == self.tts_direction_prompt)
            .map(|preset| preset.name.as_str())
    }

    pub(super) fn gemini_voice_label(name: &str) -> &str {
        GEMINI_VOICE_OPTIONS
            .iter()
            .find(|voice| voice.name == name)
            .map(|voice| voice.label)
            .unwrap_or("Custom voice")
    }

    pub(super) fn save_current_tts_preset(&mut self) {
        let name = self.tts_preset_name.trim();
        let prompt = self.tts_direction_prompt.trim();
        if name.is_empty() || prompt.is_empty() {
            self.tts_error = Some("Preset name and prompt are required".to_owned());
            return;
        }

        if let Some(existing) = self
            .tts_prompt_presets
            .iter_mut()
            .find(|preset| preset.name.eq_ignore_ascii_case(name))
        {
            existing.name = name.to_owned();
            existing.prompt = prompt.to_owned();
        } else {
            self.tts_prompt_presets.push(GeminiTtsPromptPreset {
                name: name.to_owned(),
                prompt: prompt.to_owned(),
            });
            self.tts_prompt_presets.sort_by(|left, right| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            });
        }
        self.tts_error = None;
        let _ = self
            .storage
            .save_tts_prompt_presets(&self.tts_prompt_presets);
        self.tts_status = "Preset saved".to_owned();
    }

    pub(super) fn apply_tts_preset_by_name(&mut self, name: &str) {
        if let Some(preset) = self
            .tts_prompt_presets
            .iter()
            .find(|preset| preset.name == name)
            .cloned()
        {
            self.tts_preset_name = preset.name;
            self.tts_direction_prompt = preset.prompt;
            self.tts_error = None;
        }
    }

    pub(super) fn delete_selected_tts_preset(&mut self) {
        let Some(selected_name) = self.selected_tts_preset_name().map(str::to_owned) else {
            return;
        };
        self.tts_prompt_presets
            .retain(|preset| !preset.name.eq_ignore_ascii_case(&selected_name));
        let _ = self
            .storage
            .save_tts_prompt_presets(&self.tts_prompt_presets);
        self.tts_preset_name.clear();
        self.tts_status = "Preset removed".to_owned();
    }

    pub(super) fn add_tts_result_to_library(&mut self, path: &Path) {
        match self.storage.import_sound(path) {
            Ok(mut sound) => {
                let preferred_name = self.tts_output_name.trim();
                if !preferred_name.is_empty() {
                    sound.name = preferred_name.to_owned();
                }
                sound.folder_id = None;
                self.app_view = AppView::Library;
                self.library_tab = LibraryTab::Sounds;
                self.library_current_folder = None;
                self.folder_import_select_mode = None;
                self.selected = Some(sound.id);
                self.library_audio_query.clear();
                self.library_audio_tag_filters.clear();
                self.library_favorites_only_audio = false;
                self.sounds.insert(0, sound);
                self.save_now();
                self.tts_can_add_to_library = false;
                self.tts_added_to_library = true;
                self.status = Some("Added to library".to_owned());
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn existing_myinstants_path(
        files: &mut HashMap<String, PathBuf>,
        audio_url: &str,
    ) -> Option<PathBuf> {
        let path = files.get(audio_url).cloned()?;
        if path.exists() {
            Some(path)
        } else {
            files.remove(audio_url);
            None
        }
    }

    pub(super) fn existing_myinstants_download_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_cached_files, audio_url)
    }

    pub(super) fn existing_myinstants_preview_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        Self::existing_myinstants_path(&mut self.myinstants_preview_files, audio_url)
    }

    pub(super) fn existing_myinstants_playback_path(&mut self, audio_url: &str) -> Option<PathBuf> {
        self.existing_myinstants_download_path(audio_url)
            .or_else(|| self.existing_myinstants_preview_path(audio_url))
    }

    pub(super) fn toggle_myinstants_preview(&mut self, result: &MyinstantsResult) -> Result<()> {
        if let Some(path) = self.existing_myinstants_playback_path(&result.audio_url)
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing_file(&path))
        {
            self.stop_preview();
            return Ok(());
        }

        let path = if let Some(path) = self.existing_myinstants_download_path(&result.audio_url) {
            path
        } else if let Some(path) = self.existing_myinstants_preview_path(&result.audio_url) {
            path
        } else {
            let path = self.myinstants.ensure_preview_file(result)?;
            self.myinstants_preview_files
                .insert(result.audio_url.clone(), path.clone());
            path
        };
        self.myinstants_preview_audio_url = Some(result.audio_url.clone());
        if !self.myinstants_waveforms.contains_key(&result.audio_url) {
            let waveform = self.storage.analyze_waveform_preview(&path, 96)?;
            self.myinstants_waveforms
                .insert(result.audio_url.clone(), waveform);
        }
        self.preview_file_path(&path)
    }

    pub(super) fn queue_myinstants_waveform_prefetch(&mut self, result: &MyinstantsResult) {
        if self.myinstants_waveforms.contains_key(&result.audio_url)
            || self.myinstants_waveform_jobs.contains(&result.audio_url)
        {
            return;
        }

        self.myinstants_waveform_jobs
            .insert(result.audio_url.clone());
        let result = result.clone();
        let client = self.myinstants.clone();
        let tx = self.myinstants_waveform_tx.clone();
        thread::spawn(move || {
            let message = match client.ensure_preview_file(&result) {
                Ok(path) => match Storage::new()
                    .and_then(|storage| storage.analyze_waveform_preview(&path, 96))
                {
                    Ok(waveform) => MyinstantsWaveformMessage::Ready {
                        audio_url: result.audio_url.clone(),
                        path,
                        waveform,
                    },
                    Err(error) => MyinstantsWaveformMessage::Failed {
                        audio_url: result.audio_url.clone(),
                        error: error.to_string(),
                    },
                },
                Err(error) => MyinstantsWaveformMessage::Failed {
                    audio_url: result.audio_url.clone(),
                    error: error.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    pub(super) fn download_site_url(kind: DownloadSiteKind) -> &'static str {
        match kind {
            DownloadSiteKind::Youtube => "https://www.youtube.com/",
            DownloadSiteKind::SoundCloud => "https://soundcloud.com/",
            DownloadSiteKind::Bandcamp => "https://bandcamp.com/",
            DownloadSiteKind::TikTok => "https://www.tiktok.com/",
            DownloadSiteKind::Facebook => "https://www.facebook.com/",
            DownloadSiteKind::Instagram => "https://www.instagram.com/",
            DownloadSiteKind::X => "https://x.com/",
            DownloadSiteKind::Vimeo => "https://vimeo.com/",
            DownloadSiteKind::Twitch => "https://www.twitch.tv/",
            DownloadSiteKind::GoogleDrive => "https://drive.google.com/",
        }
    }

    fn download_site_icon_png_bytes(kind: DownloadSiteKind) -> &'static [u8] {
        match kind {
            DownloadSiteKind::Youtube => include_bytes!("../../assets/site-badges/youtube.png"),
            DownloadSiteKind::SoundCloud => include_bytes!("../../assets/site-badges/soundcloud.png"),
            DownloadSiteKind::Bandcamp => include_bytes!("../../assets/site-badges/bandcamp.png"),
            DownloadSiteKind::TikTok => include_bytes!("../../assets/site-badges/tiktok.png"),
            DownloadSiteKind::Facebook => include_bytes!("../../assets/site-badges/facebook.png"),
            DownloadSiteKind::Instagram => include_bytes!("../../assets/site-badges/instagram.png"),
            DownloadSiteKind::X => include_bytes!("../../assets/site-badges/x.png"),
            DownloadSiteKind::Vimeo => include_bytes!("../../assets/site-badges/vimeo.png"),
            DownloadSiteKind::Twitch => include_bytes!("../../assets/site-badges/twitch.png"),
            DownloadSiteKind::GoogleDrive => {
                include_bytes!("../../assets/site-badges/google-drive.png")
            }
        }
    }

    fn cached_download_site_icon_texture(
        &self,
        ctx: &Context,
        kind: DownloadSiteKind,
    ) -> Option<TextureHandle> {
        if let Some(texture) = self.download_site_icon_cache.borrow().get(&kind).cloned() {
            return Some(texture);
        }

        let icon = eframe::icon_data::from_png_bytes(Self::download_site_icon_png_bytes(kind)).ok()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let texture = ctx.load_texture(
            format!("download-site-icon-{kind:?}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.download_site_icon_cache
            .borrow_mut()
            .insert(kind, texture.clone());
        Some(texture)
    }

    pub(super) fn youtube_search_button(
        &mut self,
        ui: &mut Ui,
        label: &str,
        enabled: bool,
    ) -> egui::Response {
        let desired = vec2(152.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::TRANSPARENT
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        let badge = DownloadSiteBadge {
            name: "YouTube",
            kind: DownloadSiteKind::Youtube,
            color: Color32::from_rgb(255, 77, 141),
        };
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            vec2(18.0, 18.0),
        );
        self.paint_download_site_icon(ui, icon_rect, badge);
        ui.painter().text(
            Pos2::new(rect.left() + 34.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn search_sound_button(ui: &mut Ui, enabled: bool) -> egui::Response {
        let desired = vec2(36.0, 36.0);
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(desired, sense);
        let fill = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if enabled {
            Color32::from_rgb(214, 51, 132)
        } else if Self::dark_theme_enabled() {
            Color32::from_rgb(76, 63, 83)
        } else {
            Color32::from_rgb(224, 211, 220)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(18),
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            char::from_u32(0xe8b6).unwrap_or(' '),
            egui::FontId::new(16.0, FontFamily::Name(MATERIAL_ICONS_FONT.into())),
            if enabled {
                Color32::WHITE
            } else {
                Self::muted_text_color()
            },
        );
        Self::decorate_button_response(ui, &response);
        response
    }

    pub(super) fn format_compact_count(value: u64) -> String {
        if value >= 1_000_000_000 {
            format!("{:.1}B views", value as f64 / 1_000_000_000.0)
        } else if value >= 1_000_000 {
            format!("{:.1}M views", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.1}K views", value as f64 / 1_000.0)
        } else {
            format!("{value} views")
        }
    }

    pub(super) fn paint_download_site_icon(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        badge: DownloadSiteBadge,
    ) {
        if let Some(texture) = self.cached_download_site_icon_texture(ui.ctx(), badge.kind) {
            ui.painter().image(
                texture.id(),
                rect.shrink2(vec2(1.0, 1.0)),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            return;
        }

        let painter = ui.painter();
        let center = rect.center();
        let white = Color32::WHITE;
        let radius = rect.width().min(rect.height()) * 0.5;
        let s = radius / 12.0;

        // Except for Google Drive which has a white background circle drawn inside the match,
        // all other badges draw their background circle using badge.color.
        if badge.kind != DownloadSiteKind::GoogleDrive {
            painter.circle_filled(center, radius, badge.color);
        }

        match badge.kind {
            DownloadSiteKind::Youtube => {
                // YouTube: Red/pink backing circle, white rounded rect, inner red/pink play triangle.
                let body = Rect::from_center_size(center, vec2(14.0 * s, 9.8 * s));
                painter.rect_filled(body, 3.0 * s, white);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-2.0 * s, -2.8 * s),
                        center + vec2(-2.0 * s, 2.8 * s),
                        center + vec2(3.0 * s, 0.0),
                    ],
                    badge.color,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::SoundCloud => {
                // SoundCloud: Orange backing circle, 6 vertical soundwave bars, overlapping cloud circles & base.
                // Draw left-side bars
                let bar_heights = [3.5, 5.0, 6.5, 8.0, 9.0, 9.5];
                for (index, height) in bar_heights.into_iter().enumerate() {
                    let dx = -8.0 + (index as f32) * 1.6;
                    let bar = Rect::from_min_max(
                        center + vec2((dx - 0.5) * s, -height * s + 4.5 * s),
                        center + vec2((dx + 0.5) * s, 4.5 * s),
                    );
                    painter.rect_filled(bar, 0.5 * s, white);
                }
                // Draw cloud body circles
                painter.circle_filled(center + vec2(2.5 * s, 0.5 * s), 4.0 * s, white);
                painter.circle_filled(center + vec2(6.0 * s, 1.5 * s), 3.0 * s, white);
                // Draw flat connector base
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(0.0 * s, 0.5 * s),
                        center + vec2(9.0 * s, 4.5 * s),
                    ),
                    0.0,
                    white,
                );
            }
            DownloadSiteKind::Bandcamp => {
                // Bandcamp: Blue backing circle, white slanted parallelogram.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-1.5 * s, -4.5 * s),
                        center + vec2(7.5 * s, -4.5 * s),
                        center + vec2(1.5 * s, 4.5 * s),
                        center + vec2(-7.5 * s, 4.5 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));
            }
            DownloadSiteKind::TikTok => {
                // TikTok: Dark backing circle, music note with cyan & red/magenta offset fringes.
                let paint_note = |painter: &egui::Painter, offset: Vec2, color: Color32| {
                    let n_center = center + offset;
                    // Note head (filled circle at bottom-left)
                    painter.circle_filled(n_center + vec2(-2.0 * s, 3.0 * s), 2.8 * s, color);

                    // Stem (vertical line)
                    painter.line_segment(
                        [
                            n_center + vec2(0.8 * s, 3.0 * s),
                            n_center + vec2(0.8 * s, -4.0 * s),
                        ],
                        Stroke::new(2.0 * s, color),
                    );

                    // Hook (quarter circle arc from PI to 1.5 PI, centered at 4.8, -4.0)
                    let mut hook_pts = Vec::new();
                    for i in 0..=8 {
                        let theta = std::f32::consts::PI * (1.0 + (i as f32) / 16.0);
                        let pt = n_center
                            + vec2(
                                (4.8 + 4.0 * theta.cos()) * s,
                                (-4.0 + 4.0 * theta.sin()) * s,
                            );
                        hook_pts.push(pt);
                    }
                    painter.add(egui::Shape::line(hook_pts, Stroke::new(2.0 * s, color)));
                };

                // Offset passes for chromatic aberration
                paint_note(
                    painter,
                    vec2(-0.8 * s, -0.5 * s),
                    Color32::from_rgb(0, 242, 234),
                ); // Cyan
                paint_note(
                    painter,
                    vec2(0.8 * s, 0.5 * s),
                    Color32::from_rgb(254, 44, 85),
                ); // Red/Magenta
                paint_note(painter, vec2(0.0, 0.0), white); // White
            }
            DownloadSiteKind::Facebook => {
                // Facebook: Blue backing circle, custom vector Facebook "f" logo.
                let mut stem_pts = vec![
                    center + vec2(1.5 * s, 8.0 * s),
                    center + vec2(1.5 * s, -3.0 * s),
                ];
                for i in 1..=6 {
                    let theta = std::f32::consts::PI * (1.0 + (i as f32) / 12.0);
                    stem_pts.push(
                        center
                            + vec2(
                                (4.5 + 3.0 * theta.cos()) * s,
                                (-3.0 + 3.0 * theta.sin()) * s,
                            ),
                    );
                }
                // Draw stem and hook
                painter.add(egui::Shape::line(stem_pts, Stroke::new(3.2 * s, white)));
                // Draw crossbar
                painter.line_segment(
                    [
                        center + vec2(-2.0 * s, -1.0 * s),
                        center + vec2(4.5 * s, -1.0 * s),
                    ],
                    Stroke::new(3.2 * s, white),
                );
            }
            DownloadSiteKind::Instagram => {
                // Instagram: Purple/pink backing circle, camera body outline, inner lens, and flash dot.
                let body = Rect::from_center_size(center, vec2(13.0 * s, 13.0 * s));
                painter.rect_stroke(
                    body,
                    4.0 * s,
                    Stroke::new(1.8 * s, white),
                    StrokeKind::Outside,
                );
                painter.circle_stroke(center, 3.3 * s, Stroke::new(1.8 * s, white));
                painter.circle_filled(center + vec2(3.8 * s, -3.8 * s), 1.0 * s, white);
            }
            DownloadSiteKind::X => {
                // X: Black backing circle, custom double-struck X layout using solid polygon and parallel line strokes.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(3.0 * s, -5.0 * s),
                        center + vec2(5.5 * s, -5.0 * s),
                        center + vec2(-3.0 * s, 5.0 * s),
                        center + vec2(-5.5 * s, 5.0 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));
                painter.line_segment(
                    [
                        center + vec2(-5.5 * s, -5.0 * s),
                        center + vec2(4.5 * s, 5.0 * s),
                    ],
                    Stroke::new(1.2 * s, white),
                );
                painter.line_segment(
                    [
                        center + vec2(-3.0 * s, -5.0 * s),
                        center + vec2(7.0 * s, 5.0 * s),
                    ],
                    Stroke::new(1.2 * s, white),
                );
            }
            DownloadSiteKind::Vimeo => {
                // Vimeo: Blue backing circle, custom curved "v".
                painter.add(egui::Shape::line(
                    vec![
                        center + vec2(-5.5 * s, -2.5 * s),
                        center + vec2(-3.0 * s, 3.5 * s),
                        center + vec2(-1.0 * s, 4.0 * s),
                        center + vec2(1.0 * s, 0.5 * s),
                        center + vec2(5.0 * s, -4.5 * s),
                    ],
                    Stroke::new(2.6 * s, white),
                ));
            }
            DownloadSiteKind::Twitch => {
                // Twitch: Purple backing circle, chat bubble (rounded rect, left bottom block, and beak) with eye slots.
                let body = Rect::from_min_max(
                    center + vec2(-6.0 * s, -6.0 * s),
                    center + vec2(6.0 * s, 2.0 * s),
                );
                painter.rect_filled(body, 1.0 * s, white);

                let left_ext = Rect::from_min_max(
                    center + vec2(-6.0 * s, 2.0 * s),
                    center + vec2(-2.0 * s, 4.0 * s),
                );
                painter.rect_filled(left_ext, 0.0, white);

                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + vec2(-2.0 * s, 2.0 * s),
                        center + vec2(-2.0 * s, 5.5 * s),
                        center + vec2(1.5 * s, 2.0 * s),
                    ],
                    white,
                    Stroke::NONE,
                ));

                // Draw purple eye slots
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(-2.5 * s, -2.5 * s),
                        center + vec2(-1.0 * s, 1.0 * s),
                    ),
                    0.5 * s,
                    badge.color,
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        center + vec2(1.0 * s, -2.5 * s),
                        center + vec2(2.5 * s, 1.0 * s),
                    ),
                    0.5 * s,
                    badge.color,
                );
            }
            DownloadSiteKind::GoogleDrive => {
                // Google Drive: White backing circle, interlocking trapezoid bands (green, blue, yellow).
                painter.circle_filled(center, radius, Color32::from_rgb(252, 252, 252));

                let outer_top = center + vec2(0.0 * s, -6.7 * s);
                let outer_bottom_left = center + vec2(-6.0 * s, 3.3 * s);
                let outer_bottom_right = center + vec2(6.0 * s, 3.3 * s);

                let inner_top = center + vec2(-1.0 * s, -4.7 * s);
                let inner_bottom_left = center + vec2(-1.8 * s, 1.3 * s);
                let inner_bottom_right = center + vec2(2.8 * s, 1.3 * s);

                // Green band (bottom)
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        outer_bottom_left,
                        outer_bottom_right,
                        inner_bottom_right,
                        inner_bottom_left,
                    ],
                    Color32::from_rgb(15, 157, 88),
                    Stroke::NONE,
                ));

                // Blue band (right)
                painter.add(egui::Shape::convex_polygon(
                    vec![outer_bottom_right, outer_top, inner_top, inner_bottom_right],
                    Color32::from_rgb(66, 133, 244),
                    Stroke::NONE,
                ));

                // Yellow band (left)
                painter.add(egui::Shape::convex_polygon(
                    vec![outer_top, outer_bottom_left, inner_bottom_left, inner_top],
                    Color32::from_rgb(251, 188, 5),
                    Stroke::NONE,
                ));
            }
        }
    }

    pub(super) fn poll_tts_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.tts_rx.try_recv() {
            self.tts_running = false;
            match message {
                GeminiTtsMessage::Finished(Ok(result)) => {
                    self.tts_status = "Done".to_owned();
                    self.tts_last_file = Some(result.path);
                    if self.tts_output_name.trim().is_empty() {
                        self.tts_output_name = result.display_name;
                    }
                    self.tts_error = None;
                    self.tts_can_add_to_library = true;
                }
                GeminiTtsMessage::Finished(Err(error)) => {
                    self.tts_status = "Error".to_owned();
                    self.tts_error = Some(error);
                    self.tts_last_file = None;
                    self.tts_can_add_to_library = false;
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn render_download_site_badges(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for badge in Self::download_site_badges() {
                let fill = if Self::dark_theme_enabled() {
                    Color32::from_rgb(29, 25, 35)
                } else {
                    Color32::from_rgb(255, 251, 254)
                };
                let stroke = if Self::dark_theme_enabled() {
                    Color32::from_rgb(82, 67, 90)
                } else {
                    Color32::from_rgb(230, 221, 229)
                };
                let response = Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, stroke))
                    .corner_radius(14.0)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        let (icon_rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                        self.paint_download_site_icon(ui, icon_rect, badge);
                    })
                    .response
                    .on_hover_text(badge.name);
                Self::decorate_button_response(ui, &response);
                if response.clicked() {
                    let _ = open::that(Self::download_site_url(badge.kind));
                }
            }
        });
    }

    pub(super) fn render_youtube_result_row(
        &mut self,
        ui: &mut Ui,
        result: &YoutubeSearchResult,
        download_label: &str,
    ) -> bool {
        let mut download_clicked = false;
        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(22.0)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let badge = DownloadSiteBadge {
                        name: "YouTube",
                        kind: DownloadSiteKind::Youtube,
                        color: Color32::from_rgb(255, 77, 141),
                    };
                    let (icon_rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    self.paint_download_site_icon(ui, icon_rect, badge);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add_sized(
                            [ui.available_width().min(380.0), 18.0],
                            egui::Label::new(
                                RichText::new(&result.title)
                                    .size(13.5)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            )
                            .truncate(),
                        );
                        let mut parts = Vec::new();
                        if let Some(duration) = result.duration {
                            parts.push(format_time(duration as f32));
                        }
                        if let Some(uploader) = &result.uploader
                            && !uploader.trim().is_empty()
                        {
                            parts.push(Self::truncate_middle_ascii(uploader, 28));
                        }
                        if let Some(view_count) = result.view_count {
                            parts.push(Self::format_compact_count(view_count));
                        }
                        if !result.id.trim().is_empty() {
                            parts.push(format!("ID {}", result.id));
                        }
                        if parts.is_empty() {
                            parts.push("YouTube".to_owned());
                        }
                        ui.label(
                            RichText::new(parts.join("  -  "))
                                .size(12.0)
                                .color(Self::muted_text_color()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let response = ui.add_sized(
                            [104.0, 34.0],
                            Self::action_button(
                                RichText::new(download_label)
                                    .size(13.0)
                                    .color(Color32::WHITE),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &response);
                        if response.clicked() {
                            download_clicked = true;
                        }
                    });
                });
            });
        download_clicked
    }

    pub(super) fn render_tts_download_tab(&mut self, ui: &mut Ui, ctx: &Context) {
        let mut generate_request = false;
        let mut preview_request = false;
        let mut add_to_library = false;
        let mut clear_result = false;
        let mut save_gemini = false;
        let mut save_preset = false;
        let mut delete_preset = false;
        let mut draft_changed = false;
        let selected_voice_label = Self::gemini_voice_label(&self.tts_voice_name).to_owned();
        let selected_preset_name = self
            .selected_tts_preset_name()
            .map(str::to_owned)
            .unwrap_or_else(|| self.t("download.custom"));

        if self.tts_running {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }

        ui.label(
            RichText::new(self.t("download.gemini_tts"))
                .size(14.0)
                .color(Self::strong_text_color())
                .strong(),
        );
        ui.add_space(10.0);

        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .corner_radius(18.0)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                let column_gap = 14.0;
                let action_button_width = 30.0;
                let action_gap = 8.0;
                let column_width = ((ui.available_width() - column_gap) / 2.0).max(180.0);
                let label_size = 12.0;

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = column_gap;

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.voice"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        Self::with_dark_combo_visuals(ui, |ui| {
                            ComboBox::from_id_salt("gemini-tts-voice")
                                .width(column_width)
                                .selected_text(
                                    RichText::new(&selected_voice_label)
                                        .color(Self::strong_text_color()),
                                )
                                .show_ui(ui, |ui| {
                                    for voice in GEMINI_VOICE_OPTIONS {
                                        if ui
                                            .selectable_label(
                                                self.tts_voice_name == voice.name,
                                                Self::gemini_voice_label(voice.name),
                                            )
                                            .clicked()
                                        {
                                            self.tts_voice_name = voice.name.to_owned();
                                            draft_changed = true;
                                        }
                                    }
                                });
                        });
                    });

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.name"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        let name_response = Self::with_input_widget_visuals(ui, |ui| {
                            ui.add_sized(
                                [column_width, 30.0],
                                TextEdit::singleline(&mut self.tts_output_name)
                                    .hint_text("gemini tts")
                                    .desired_width(f32::INFINITY),
                            )
                        });
                        draft_changed |= name_response.changed();
                    });
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = column_gap;

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.prompt_preset"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        Self::with_dark_combo_visuals(ui, |ui| {
                            ComboBox::from_id_salt("gemini-tts-preset")
                                .width(column_width)
                                .selected_text(
                                    RichText::new(selected_preset_name.clone())
                                        .color(Self::strong_text_color()),
                                )
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(
                                            self.selected_tts_preset_name().is_none(),
                                            self.t("download.custom"),
                                        )
                                        .clicked()
                                    {
                                        self.tts_preset_name.clear();
                                        draft_changed = true;
                                    }

                                    let preset_names = self
                                        .tts_prompt_presets
                                        .iter()
                                        .map(|preset| preset.name.clone())
                                        .collect::<Vec<_>>();
                                    for preset_name in preset_names {
                                        if ui
                                            .selectable_label(
                                                self.selected_tts_preset_name()
                                                    == Some(preset_name.as_str()),
                                                &preset_name,
                                            )
                                            .clicked()
                                        {
                                            self.apply_tts_preset_by_name(&preset_name);
                                            draft_changed = true;
                                        }
                                    }
                                });
                        });
                    });

                    ui.vertical(|ui| {
                        ui.set_width(column_width);
                        ui.label(
                            RichText::new(self.t("download.preset_name"))
                                .size(label_size)
                                .color(Self::muted_text_color()),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = action_gap;
                            let preset_name_hint = self.t("download.preset_name");
                            let preset_response = Self::with_input_widget_visuals(ui, |ui| {
                                ui.add_sized(
                                    [
                                        (column_width
                                            - action_button_width * 2.0
                                            - action_gap * 2.0)
                                            .max(84.0),
                                        30.0,
                                    ],
                                    TextEdit::singleline(&mut self.tts_preset_name)
                                        .hint_text(preset_name_hint),
                                )
                            });
                            draft_changed |= preset_response.changed();

                            let save = ui.add_sized(
                                [action_button_width, 30.0],
                                Self::action_button(RichText::new("+").size(16.0), false, false),
                            );
                            Self::decorate_button_response(ui, &save);
                            if save.clicked() {
                                save_preset = true;
                            }

                            let delete = ui.add_enabled(
                                self.selected_tts_preset_name().is_some(),
                                Self::action_button(RichText::new("x").size(15.0), false, false),
                            );
                            Self::decorate_button_response(ui, &delete);
                            if delete.clicked() {
                                delete_preset = true;
                            }
                        });
                    });
                });

                ui.add_space(10.0);
                Frame::new()
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        if Self::gemini_api_key_field(
                            ui,
                            &self.t("download.gemini_api_key"),
                            &mut self.gemini_api_key,
                            &mut self.gemini_api_key_visible,
                        ) {
                            save_gemini = true;
                        }
                    });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.t("download.direction_prompt"))
                        .size(12.0)
                        .color(Self::muted_text_color()),
                );
                ui.add_space(6.0);
                let direction_hint = self.t("download.direction_hint");
                let direction_response = Self::with_input_widget_visuals(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 96.0],
                        TextEdit::multiline(&mut self.tts_direction_prompt)
                            .desired_width(f32::INFINITY)
                            .hint_text(direction_hint),
                    )
                });
                draft_changed |= direction_response.changed();

                ui.add_space(10.0);
                let enter_text_hint = self.t("download.enter_text");
                let text_response = Self::with_input_widget_visuals(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 130.0],
                        TextEdit::multiline(&mut self.tts_text)
                            .desired_width(f32::INFINITY)
                            .hint_text(enter_text_hint),
                    )
                });
                draft_changed |= text_response.changed();
            });

        if draft_changed {
            self.save_tts_draft_preferences();
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let generate = ui.add_enabled(
                !self.tts_running
                    && !self.tts_text.trim().is_empty()
                    && !self.gemini_api_key.trim().is_empty(),
                Self::action_button(
                    RichText::new(self.t("download.generate")).size(13.0),
                    false,
                    true,
                ),
            );
            Self::decorate_button_response(ui, &generate);
            if generate.clicked() {
                generate_request = true;
            }

            let preview = ui.add_enabled(
                self.tts_last_file.is_some() && !self.tts_running,
                Self::action_button(
                    RichText::new(self.t("download.preview")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &preview);
            if preview.clicked() {
                preview_request = true;
            }

            let add = ui.add_enabled(
                self.tts_can_add_to_library,
                Self::action_button(
                    RichText::new(self.t("download.add_to_library")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &add);
            if add.clicked() {
                add_to_library = true;
            }

            let clear = ui.add_enabled(
                self.tts_last_file.is_some() && !self.tts_running,
                Self::action_button(
                    RichText::new(self.t("download.clear")).size(13.0),
                    false,
                    false,
                ),
            );
            Self::decorate_button_response(ui, &clear);
            if clear.clicked() {
                clear_result = true;
            }
        });

        if self.tts_running {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(18.0));
                ui.label(
                    RichText::new(self.t("download.generating_speech"))
                        .size(12.5)
                        .color(Self::muted_text_color()),
                );
            });
        } else if !self.gemini_api_key.trim().is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.t("download.gemini_tts_help"))
                    .size(12.0)
                    .color(Self::muted_text_color()),
            );
        }

        if self.gemini_api_key.trim().is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.t("download.gemini_api_key_missing"))
                    .size(12.5)
                    .color(Color32::from_rgb(171, 54, 91)),
            );
        } else if let Some(path) = &self.tts_last_file {
            ui.add_space(12.0);
            ui.label(
                RichText::new(
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("audio"),
                )
                .size(13.5)
                .color(Self::strong_text_color())
                .strong(),
            );
        }

        if let Some(error) = &self.tts_error {
            ui.add_space(10.0);
            ui.label(
                RichText::new(error)
                    .size(12.5)
                    .color(Color32::from_rgb(171, 54, 91)),
            );
        }

        if save_gemini {
            let _ = self.storage.save_gemini_api_key(&self.gemini_api_key);
        }
        if delete_preset {
            self.delete_selected_tts_preset();
            self.save_tts_draft_preferences();
        }
        if save_preset {
            self.save_current_tts_preset();
            self.save_tts_draft_preferences();
        }
        if generate_request {
            self.start_tts_generation();
        }
        if preview_request
            && let Some(path) = self.tts_last_file.clone()
            && let Some(audio) = self.audio.as_mut()
        {
            if let Err(error) = audio.play_file(&path) {
                self.set_error_status(error);
            }
        }
        if add_to_library && let Some(path) = self.tts_last_file.clone() {
            self.add_tts_result_to_library(&path);
        }
        if clear_result {
            if let Some(path) = self.tts_last_file.take() {
                let _ = fs::remove_file(path);
            }
            self.tts_status.clear();
            self.tts_error = None;
            self.tts_can_add_to_library = false;
            self.tts_added_to_library = false;
        }
    }

    pub(super) fn render_download_panel(&mut self, ctx: &Context) {
        if !self.show_download_panel {
            return;
        }

        let snapshot = self.downloader.snapshot();
        let mut open_panel = self.show_download_panel;
        let mut should_start_download = false;
        let mut should_stop_download = false;
        let mut add_to_library = false;
        let mut open_file = false;
        let mut open_folder = false;
        let mut clear_result = false;
        let mut minimize_request = false;
        let mut close_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(520.0, 420.0), vec2(320.0, 260.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("youtube-audio-download"))
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
                        blur: 32,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(28.0)
                    .inner_margin(Margin {
                        left: 20,
                        right: 12,
                        top: 12,
                        bottom: 20,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(34.0);
                    ui.label(Self::icon(0xe2c4, 20.0, Self::strong_text_color()).strong());
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if Self::icon_titlebar(ui, [34.0, 34.0], 0xe5cd, false, true).clicked() {
                            clear_result = !snapshot.running;
                            close_request = true;
                        }
                        if Self::icon_titlebar(ui, [34.0, 34.0], 0xe15b, false, false).clicked() {
                            minimize_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let download_tab = ui.add_sized(
                        [120.0, 32.0],
                        Self::action_button(
                            RichText::new(self.t("download.download")).size(12.5),
                            self.download_panel_tab == DownloadPanelTab::Download,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &download_tab);
                    if download_tab.clicked() {
                        self.download_panel_tab = DownloadPanelTab::Download;
                    }
                    let tts_tab = ui.add_sized(
                        [120.0, 32.0],
                        Self::action_button(
                            RichText::new(self.t("download.gemini_tts")).size(12.5),
                            self.download_panel_tab == DownloadPanelTab::Tts,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &tts_tab);
                    if tts_tab.clicked() {
                        self.download_panel_tab = DownloadPanelTab::Tts;
                    }
                });

                ui.add_space(12.0);
                if self.download_panel_tab == DownloadPanelTab::Tts {
                    self.render_tts_download_tab(ui, ctx);
                } else {
                    if snapshot.running {
                        ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
                    }

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(self.t("download.supporting_web"))
                                .size(12.5)
                                .color(Self::muted_text_color()),
                        );
                        let help = ui.add_sized(
                            [22.0, 22.0],
                            Button::new(Self::icon(0xe887, 15.0, Color32::from_rgb(214, 51, 132)))
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(11.0),
                        );
                        if help.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
                        }
                        help.on_hover_ui_at_pointer(|ui| {
                            ui.set_max_width(300.0);
                            ui.label(
                                RichText::new(self.t("download.supported_websites"))
                                    .size(13.0)
                                    .color(Self::strong_text_color())
                                    .strong(),
                            );
                            ui.add_space(4.0);
                            ui.label(self.t("download.supported_websites_help_1"));
                            ui.label(self.t("download.supported_websites_help_2"));
                            ui.add_space(4.0);
                            ui.label(self.t("download.supported_websites_help_3"));
                            ui.label(self.t("download.supported_websites_help_4"));
                        });
                    });
                    ui.add_space(8.0);
                    self.render_download_site_badges(ui);

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        let response = Frame::new()
                            .fill(Self::input_fill())
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(16.0)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.add_sized(
                                    [ui.available_width() - 4.0, 22.0],
                                    TextEdit::singleline(&mut self.download_url)
                                        .frame(false)
                                        .hint_text("https://youtube.com/watch?v=... or soundcloud / tiktok / facebook")
                                        .desired_width(f32::INFINITY)
                                        .margin(Vec2::new(0.0, 4.0)),
                                )
                            })
                            .inner;
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && !snapshot.running
                        {
                            should_start_download = true;
                        }

                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        let start_button = ui.add_enabled(
                            !snapshot.running && !self.download_url.trim().is_empty(),
                            Self::action_button(
                                RichText::new(self.t("download.download_sound")).size(13.0),
                                false,
                                true,
                            ),
                        );
                        Self::decorate_button_response(ui, &start_button);
                        if start_button.clicked() {
                            should_start_download = true;
                        }

                        if snapshot.running {
                            if Self::icon_action(ui, [42.0, 32.0], 0xe047, false, true).clicked() {
                                should_stop_download = true;
                            }
                            ui.label(
                                RichText::new(snapshot.stage.clone())
                                    .size(13.0)
                                    .color(Self::muted_text_color()),
                            );
                        }
                    });

                    if let Some(progress) = snapshot.progress {
                        ui.add_space(8.0);
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(ui.available_width())
                                .fill(Color32::from_rgb(227, 82, 149)),
                        );
                    } else if snapshot.running {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(18.0));
                            ui.label(
                                RichText::new(self.t("download.working"))
                                    .size(12.5)
                                    .color(Self::muted_text_color()),
                            );
                        });
                    }

                    if let Some(error) = &snapshot.error {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(error)
                                .size(13.0)
                                .color(Color32::from_rgb(171, 54, 91)),
                        );
                    }

                    if let Some(path) = &snapshot.last_file {
                        if self.download_preview_file.as_ref() != Some(path) {
                            self.download_preview_file = Some(path.clone());
                            if let Ok((waveform, duration)) =
                                self.storage.analyze_audio_preview(path, 64)
                            {
                                self.download_preview_waveform = waveform;
                                self.download_preview_duration = duration;
                            } else {
                                self.download_preview_waveform.clear();
                                self.download_preview_duration = 0.0;
                            }
                            self.download_preview_cursor = Some(0.0);
                        }

                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(
                                path.file_name()
                                    .and_then(|value| value.to_str())
                                    .unwrap_or("audio"),
                            )
                            .size(13.5)
                            .color(Self::strong_text_color())
                            .strong(),
                        );

                        let is_playing = self
                            .audio
                            .as_ref()
                            .is_some_and(|audio| audio.is_playing_file(path));
                        let playback_pos = self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.playback_position_secs_for_file(path));
                        if is_playing {
                            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
                        }
                        let duration = self.download_preview_duration.max(0.05);
                        let current_cursor = if is_playing {
                            playback_pos.unwrap_or(0.0)
                        } else {
                            self.download_preview_cursor.unwrap_or(0.0)
                        };

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if Self::icon_action(
                                ui,
                                [36.0, 28.0],
                                if is_playing { 0xe047 } else { 0xe037 },
                                is_playing,
                                false,
                            )
                            .clicked()
                            {
                                if is_playing {
                                    self.stop_preview();
                                } else {
                                    let start = self.download_preview_cursor.unwrap_or(0.0);
                                    let start = if start >= duration - 0.02 { 0.0 } else { start };
                                    self.download_preview_cursor = Some(start);
                                    if let Some(audio) = self.audio.as_mut() {
                                        let _ = audio.play_file_from(path, start);
                                    }
                                }
                            }

                            let time_text =
                                format!("{}/{}", format_time(current_cursor), format_time(duration));
                            let time_width = 86.0;
                            let wave_width = (ui.available_width() - time_width - 8.0).max(60.0);
                            let (wave_rect, wave_response) =
                                ui.allocate_exact_size(vec2(wave_width, 28.0), Sense::click_and_drag());

                            if wave_response.clicked() || wave_response.dragged() {
                                if let Some(pointer) = wave_response.interact_pointer_pos() {
                                    let ratio =
                                        ((pointer.x - wave_rect.left()) / wave_rect.width()).clamp(0.0, 1.0);
                                    let seek_secs = ratio * duration;
                                    self.download_preview_cursor = Some(seek_secs);
                                    if let Some(audio) = self.audio.as_mut() {
                                        let _ = audio.play_file_from(path, seek_secs);
                                    }
                                }
                            }

                            let painter = ui.painter_at(wave_rect);
                            painter.rect_filled(wave_rect, 8.0, Self::panel_fill());
                            let inner = wave_rect.shrink2(vec2(6.0, 3.0));

                            if self.download_preview_waveform.is_empty() {
                                painter.line_segment(
                                    [
                                        Pos2::new(inner.left(), inner.center().y),
                                        Pos2::new(inner.right(), inner.center().y),
                                    ],
                                    Stroke::new(1.0, Self::muted_text_color().linear_multiply(0.4)),
                                );
                            } else {
                                let bar_width =
                                    inner.width() / self.download_preview_waveform.len().max(1) as f32;
                                let active_color = Color32::from_rgb(214, 51, 132);
                                let idle_color = if self.dark_theme {
                                    Color32::from_rgb(120, 80, 110)
                                } else {
                                    Color32::from_rgb(238, 200, 220)
                                };
                                for (index, level) in self.download_preview_waveform.iter().enumerate() {
                                    let amplitude = Self::wave_strip_level(*level).clamp(0.08, 1.0);
                                    let center_x = inner.left() + (index as f32 + 0.5) * bar_width;
                                    let half = amplitude * inner.height() * 0.42;
                                    let bar = Rect::from_min_max(
                                        Pos2::new(
                                            center_x - (bar_width * 0.22).max(0.8),
                                            inner.center().y - half,
                                        ),
                                        Pos2::new(
                                            center_x + (bar_width * 0.22).max(0.8),
                                            inner.center().y + half,
                                        ),
                                    );
                                    let color = if amplitude > 0.32 { active_color } else { idle_color };
                                    painter.rect_filled(bar, 1.5, color);
                                }
                            }

                            let play_progress = (current_cursor / duration).clamp(0.0, 1.0);
                            let play_x = egui::lerp(inner.left()..=inner.right(), play_progress);
                            painter.line_segment(
                                [
                                    Pos2::new(play_x, inner.top()),
                                    Pos2::new(play_x, inner.bottom()),
                                ],
                                Stroke::new(2.0, Color32::from_rgb(255, 77, 141)),
                            );

                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(time_text)
                                    .size(11.0)
                                    .color(Self::muted_text_color()),
                            );
                        });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(snapshot.can_add_to_library, |ui| {
                                if Self::icon_action(
                                    ui,
                                    [52.0, 34.0],
                                    0xe02e,
                                    false,
                                    snapshot.can_add_to_library,
                                )
                                .clicked()
                                {
                                    add_to_library = true;
                                }
                            });
                            if Self::icon_action(ui, [52.0, 34.0], 0xe89e, false, false).clicked() {
                                open_file = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe2c8, false, false).clicked() {
                                open_folder = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe14c, false, false).clicked() {
                                clear_result = true;
                            }
                        });
                    }
                }
            });

        if close_request {
            self.stop_preview();
            open_panel = false;
        }
        if minimize_request {
            self.stop_preview();
            open_panel = false;
        }
        self.show_download_panel = open_panel;

        if should_start_download {
            match self
                .downloader
                .start_audio_download(self.download_url.trim().to_owned())
            {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if should_stop_download {
            self.downloader.stop_audio_download();
        }

        if let Some(path) = snapshot.last_file.clone() {
            if add_to_library {
                self.import_paths(vec![path.clone()]);
                self.downloader.mark_added_to_library();
            }
            if open_file {
                if let Err(error) = self.downloader.open_file(&path) {
                    self.set_error_status(error);
                }
            }
            if open_folder {
                if let Err(error) = self.downloader.open_folder(&path) {
                    self.set_error_status(error);
                }
            }
        }

        if clear_result {
            self.stop_preview();
            self.download_preview_file = None;
            self.download_preview_waveform.clear();
            self.download_preview_duration = 0.0;
            self.download_preview_cursor = None;
            self.downloader.clear_result();
        }
    }

    pub(super) fn render_myinstants_panel(&mut self, ctx: &Context) {
        if !self.show_myinstants_panel {
            return;
        }

        let snapshot = self.myinstants.snapshot();
        let youtube_snapshot = self.downloader.snapshot();
        let youtube_results = youtube_snapshot.youtube_results.clone();
        let was_open = self.show_myinstants_panel;
        let mut open_panel = self.show_myinstants_panel;
        let mut close_request = false;
        let mut search_request = false;
        let mut youtube_search_request = false;
        let mut add_request: Option<MyinstantsResult> = None;
        let mut download_request: Option<MyinstantsResult> = None;
        let mut preview_request: Option<MyinstantsResult> = None;
        let mut folder_request: Option<MyinstantsResult> = None;
        let mut copy_request: Option<MyinstantsResult> = None;
        let mut youtube_download_request: Option<String> = None;
        let mut stop_youtube_download = false;
        let mut clear_youtube_results = false;
        let mut more_request = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(720.0, 620.0), vec2(360.0, 300.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("myinstants-search-panel"))
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
                        blur: 32,
                        spread: 0,
                        color: Color32::from_rgba_premultiplied(78, 40, 63, 24),
                    })
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe8b6, 20.0, Self::strong_text_color()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let button_group_width = 236.0;
                    let search_placeholder = self.t("download.search_placeholder");
                    let response = ui.add_sized(
                        [(ui.available_width() - button_group_width).max(180.0), 42.0],
                        TextEdit::singleline(&mut self.myinstants_query)
                            .hint_text(search_placeholder)
                            .margin(Vec2::new(14.0, 12.0)),
                    );
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        search_request = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let youtube_button = self.youtube_search_button(
                            ui,
                            &self.t("download.search_youtube"),
                            !snapshot.searching
                                && !snapshot.downloading
                                && !youtube_snapshot.running
                                && !youtube_snapshot.searching
                                && !self.myinstants_query.trim().is_empty(),
                        );
                        if youtube_button.clicked() {
                            youtube_search_request = true;
                        }
                        ui.label(
                            RichText::new(self.t("download.or"))
                                .size(12.5)
                                .color(Self::muted_text_color())
                                .strong(),
                        );
                        if Self::search_sound_button(
                            ui,
                            !snapshot.searching
                                && !snapshot.downloading
                                && !self.myinstants_query.trim().is_empty(),
                        )
                        .clicked()
                        {
                            search_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                if snapshot.searching
                    || snapshot.downloading
                    || youtube_snapshot.searching
                    || youtube_snapshot.running
                {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(
                            RichText::new(
                                if youtube_snapshot.searching || youtube_snapshot.running {
                                    youtube_snapshot.stage.as_str()
                                } else {
                                    "..."
                                },
                            )
                            .size(14.0)
                            .color(Color32::from_rgb(214, 51, 132)),
                        );
                        if youtube_snapshot.running
                            && Self::icon_action(ui, [42.0, 30.0], 0xe047, false, true).clicked()
                        {
                            stop_youtube_download = true;
                        }
                    });
                    ui.add_space(8.0);
                }
                if let Some(error) = snapshot
                    .error
                    .as_deref()
                    .or(youtube_snapshot.error.as_deref())
                {
                    ui.label(
                        RichText::new(Self::truncate_middle_ascii(error, 80))
                            .size(12.0)
                            .color(Color32::from_rgb(189, 62, 117)),
                    );
                    ui.add_space(8.0);
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !youtube_results.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("YouTube")
                                        .size(14.0)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                    if Self::icon_action(ui, [42.0, 32.0], 0xe14c, false, false)
                                        .clicked()
                                    {
                                        clear_youtube_results = true;
                                    }
                                });
                            });
                            ui.add_space(8.0);
                            for result in youtube_results
                                .iter()
                                .take(self.youtube_search_visible_count)
                            {
                                if self.render_youtube_result_row(
                                    ui,
                                    result,
                                    &self.t("download.download"),
                                ) {
                                    youtube_download_request = Some(result.webpage_url.clone());
                                }
                                ui.add_space(8.0);
                            }
                            ui.add_space(12.0);
                        }

                        for result in snapshot.results.iter().take(self.myinstants_visible_count) {
                            self.queue_myinstants_waveform_prefetch(result);
                            let downloaded_path =
                                self.existing_myinstants_download_path(&result.audio_url);
                            let preview_path = downloaded_path.clone().or_else(|| {
                                self.existing_myinstants_preview_path(&result.audio_url)
                            });
                            let is_downloaded = downloaded_path.is_some();
                            let is_previewing = preview_path.as_ref().is_some_and(|path| {
                                self.audio
                                    .as_ref()
                                    .is_some_and(|audio| audio.is_playing_file(path))
                            });
                            let preview_progress = preview_path.as_ref().and_then(|path| {
                                self.audio
                                    .as_ref()
                                    .and_then(|audio| audio.playback_progress_for_file(path))
                            });
                            Frame::new()
                                .fill(Self::surface_fill())
                                .stroke(Stroke::new(1.0, Self::border_color()))
                                .corner_radius(22.0)
                                .inner_margin(Margin::same(14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            [ui.available_width() - 176.0, 20.0],
                                            egui::Label::new(
                                                RichText::new(&result.title)
                                                    .size(13.5)
                                                    .color(Self::strong_text_color())
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );
                                        if Self::icon_action(
                                            ui,
                                            [44.0, 32.0],
                                            if is_previewing { 0xe047 } else { 0xe037 },
                                            is_previewing,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            preview_request = Some(result.clone());
                                        }
                                        if is_downloaded {
                                            if Self::icon_action(
                                                ui,
                                                [44.0, 32.0],
                                                0xe2c7,
                                                false,
                                                false,
                                            )
                                            .clicked()
                                            {
                                                folder_request = Some(result.clone());
                                            }
                                            if Self::icon_action(
                                                ui,
                                                [44.0, 32.0],
                                                0xe14d,
                                                false,
                                                false,
                                            )
                                            .clicked()
                                            {
                                                copy_request = Some(result.clone());
                                            }
                                        } else if Self::icon_action(
                                            ui,
                                            [44.0, 32.0],
                                            0xe2c4,
                                            false,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            download_request = Some(result.clone());
                                        }
                                        if Self::icon_action(ui, [44.0, 32.0], 0xe145, false, true)
                                            .clicked()
                                        {
                                            add_request = Some(result.clone());
                                        }
                                    });

                                    ui.add_space(10.0);
                                    let waveform = self
                                        .myinstants_waveforms
                                        .get(&result.audio_url)
                                        .map(Vec::as_slice)
                                        .unwrap_or(&[]);
                                    Self::draw_wave_strip(
                                        ui,
                                        waveform,
                                        if is_previewing {
                                            preview_progress
                                        } else {
                                            None
                                        },
                                        Color32::from_rgb(214, 51, 132),
                                        if self.dark_theme {
                                            Color32::from_rgb(102, 74, 102)
                                        } else {
                                            Color32::from_rgb(238, 213, 227)
                                        },
                                        Self::panel_fill(),
                                        46.0,
                                    );
                                });
                            ui.add_space(10.0);
                        }

                        if snapshot.results.len() > self.myinstants_visible_count {
                            ui.add_space(2.0);
                            ui.horizontal_centered(|ui| {
                                let more_response = ui.add_sized(
                                    [76.0, 34.0],
                                    Self::action_button(
                                        RichText::new("+10")
                                            .size(13.0)
                                            .color(Self::strong_text_color()),
                                        false,
                                        false,
                                    ),
                                );
                                Self::decorate_button_response(ui, &more_response);
                                if more_response.clicked() {
                                    more_request = true;
                                }
                            });
                        }
                    });
            });

        if close_request {
            open_panel = false;
        }
        self.show_myinstants_panel = open_panel;
        if was_open && !open_panel && self.myinstants_preview_audio_url.is_some() {
            self.stop_preview();
        }

        if search_request {
            match self.myinstants.start_search(self.myinstants_query.clone()) {
                Ok(()) => {
                    self.myinstants_visible_count = 10;
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
        if youtube_search_request {
            match self
                .downloader
                .start_youtube_search(self.myinstants_query.clone())
            {
                Ok(()) => {
                    self.youtube_search_visible_count = 8;
                    self.clear_status();
                }
                Err(error) => self.set_error_status(error),
            }
        }
        if more_request {
            self.myinstants_visible_count =
                (self.myinstants_visible_count + 10).min(snapshot.results.len());
        }
        if let Some(url) = youtube_download_request {
            self.download_url = url.clone();
            match self.downloader.start_audio_download(url) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if stop_youtube_download {
            self.downloader.stop_audio_download();
        }
        if clear_youtube_results {
            self.downloader.clear_youtube_results();
        }
        if let Some(result) = preview_request {
            match self.toggle_myinstants_preview(&result) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = download_request {
            match self.myinstants.start_download(result, false) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = folder_request
            && let Some(path) = self.existing_myinstants_download_path(&result.audio_url)
            && let Some(parent) = path.parent()
            && let Err(error) = open::that(parent)
        {
            self.set_error_status(error);
        }
        if let Some(result) = copy_request
            && let Some(path) = self.existing_myinstants_download_path(&result.audio_url)
        {
            match self.copy_file_path_to_clipboard(&path) {
                Ok(()) => self.clear_status(),
                Err(error) => self.set_error_status(error),
            }
        }
        if let Some(result) = add_request {
            if let Some(path) = self.existing_myinstants_download_path(&result.audio_url) {
                self.import_downloaded_sound(&path, false);
                self.clear_status();
            } else {
                match self.myinstants.start_download(result, true) {
                    Ok(()) => self.clear_status(),
                    Err(error) => self.set_error_status(error),
                }
            }
        }
    }

    pub(super) fn is_valid_download_url(text: &str) -> bool {
        let s = text.trim();
        if s.len() < 8 || s.len() > 2048 {
            return false;
        }
        let lower = s.to_ascii_lowercase();
        if !lower.starts_with("https://") && !lower.starts_with("http://") {
            return false;
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        let rest = if let Some(after) = lower.strip_prefix("https://") {
            after
        } else if let Some(after) = lower.strip_prefix("http://") {
            after
        } else {
            return false;
        };
        let host = rest.split(['/', '?', '#', ':']).next().unwrap_or("");
        if host.is_empty() || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
            return false;
        }
        true
    }

    pub(super) fn check_clipboard_for_download_url(&mut self) {
        if let Ok(text) = self.clipboard_text() {
            let trimmed = text.trim();
            if Self::is_valid_download_url(trimmed) {
                let url = trimmed.to_owned();
                if self.clipboard_download_url.as_ref() != Some(&url) {
                    self.clipboard_download_url = Some(url);
                }
                return;
            }
        }
        self.clipboard_download_url = None;
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_preview_state_defaults_are_empty() {
        let app = SoundFxApp::new();
        assert!(app.download_preview_file.is_none());
        assert!(app.download_preview_waveform.is_empty());
        assert_eq!(app.download_preview_duration, 0.0);
        assert!(app.download_preview_cursor.is_none());
    }

    #[test]
    fn validate_download_urls_correctly() {
        assert!(SoundFxApp::is_valid_download_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(SoundFxApp::is_valid_download_url("https://youtu.be/dQw4w9WgXcQ"));
        assert!(SoundFxApp::is_valid_download_url("https://soundcloud.com/artist/track"));
        assert!(SoundFxApp::is_valid_download_url("https://www.tiktok.com/@user/video/123456789"));
        assert!(SoundFxApp::is_valid_download_url("http://example.com/audio.mp3"));

        assert!(!SoundFxApp::is_valid_download_url(""));
        assert!(!SoundFxApp::is_valid_download_url("hello world"));
        assert!(!SoundFxApp::is_valid_download_url("ftp://example.com/file.mp3"));
        assert!(!SoundFxApp::is_valid_download_url("https://localhost"));
        assert!(!SoundFxApp::is_valid_download_url("https://"));
    }
}


