use super::*;

impl SoundFxApp {
    pub(super) fn render_transition_layer(
        &self,
        ctx: &Context,
        progress: f32,
        phase: TransitionPhase,
    ) {
        let screen_rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("transition-layer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen_rect.size(), Sense::click_and_drag());
                let _ = ui.interact(
                    rect,
                    ui.id().with("transition-layer"),
                    Sense::click_and_drag(),
                );
                let painter = ui.painter_at(rect);
                let time = ctx.input(|input| input.time) as f32;
                let audio_progress = self.transition_audio_progress(ctx).unwrap_or(progress);
                let audio_level =
                    Self::sample_waveform_level(&self.startup.sound_waveform, audio_progress);
                let wave_bars =
                    Self::transition_wave_bars(&self.startup.sound_waveform, audio_progress, 11);
                let center = rect.center();
                let intro_monochrome = self.dark_theme && phase == TransitionPhase::Intro;
                let intro_light_fade = !self.dark_theme && phase == TransitionPhase::Intro;
                let light_transition = intro_light_fade;
                let light_intro_base = Color32::from_rgb(206, 198, 211);
                let light_intro_berry = Color32::from_rgb(188, 152, 174);
                let light_intro_magenta = Color32::from_rgb(171, 120, 149);
                let light_intro_plum = Color32::from_rgb(145, 94, 126);
                let light_intro_deep = Color32::from_rgb(117, 70, 104);
                let light_wave_primary = Color32::from_rgb(214, 51, 132);
                let light_wave_secondary = Color32::from_rgb(229, 85, 149);
                let light_wave_glow = (246, 124, 181);
                let blob_only_transition = false;
                let transparent_transition_backdrop = true;
                let (rose_ice, berry, magenta, plum, deep_plum, star_rgb, star_alpha_scale) =
                    if intro_monochrome {
                        (
                            Color32::from_rgb(26, 22, 31),
                            Color32::from_rgb(42, 37, 48),
                            Color32::from_rgb(56, 51, 64),
                            Color32::from_rgb(23, 19, 28),
                            Color32::from_rgb(7, 4, 10),
                            (255, 255, 255),
                            0.16,
                        )
                    } else if self.dark_theme {
                        (
                            Color32::from_rgb(15, 7, 13),
                            Color32::from_rgb(190, 63, 129),
                            Color32::from_rgb(132, 37, 89),
                            Color32::from_rgb(29, 11, 23),
                            Color32::from_rgb(7, 4, 10),
                            (255, 214, 234),
                            0.30,
                        )
                    } else if intro_light_fade {
                        (
                            light_intro_base,
                            light_intro_berry,
                            light_intro_magenta,
                            light_intro_plum,
                            light_intro_deep,
                            (255, 226, 238),
                            0.72,
                        )
                    } else {
                        (
                            Color32::from_rgb(255, 233, 242),
                            Color32::from_rgb(205, 58, 126),
                            Color32::from_rgb(168, 36, 104),
                            Color32::from_rgb(76, 24, 58),
                            Color32::from_rgb(30, 10, 24),
                            (255, 246, 252),
                            1.0,
                        )
                    };
                let t = match phase {
                    TransitionPhase::Intro => Self::ease_in_out_cubic(progress),
                    TransitionPhase::Live => 1.0,
                };
                let layer_alpha = 1.0;
                let ornament_alpha = match phase {
                    TransitionPhase::Intro => {
                        1.0 - Self::ease_in_out_cubic(((progress - 0.18) / 0.18).clamp(0.0, 1.0))
                    }
                    TransitionPhase::Live => 1.0,
                };
                let ui_match: f32 = match phase {
                    TransitionPhase::Intro => 0.0,
                    TransitionPhase::Live => 1.0,
                };
                let aura = ((1.0 - t) * (0.75 + audio_level * 0.5)).clamp(0.0, 1.0);
                let mut overlay = match phase {
                    TransitionPhase::Intro => (1.0 - t * 0.7).clamp(0.0, 1.0),
                    TransitionPhase::Live => 0.0,
                };
                if intro_light_fade {
                    overlay = 0.0;
                }
                let intro_black_fade = if intro_monochrome {
                    Self::ease_in_out_cubic(((progress - 0.58) / 0.28).clamp(0.0, 1.0))
                } else {
                    0.0
                };
                let target_rect = Self::transition_target_rect(rect);
                let base = rect.width().min(rect.height()).clamp(260.0, 440.0);
                let half_w = egui::lerp((base * 0.17)..=(target_rect.width() * 0.5), t);
                let half_h = egui::lerp((base * 0.13)..=(target_rect.height() * 0.5), t);
                let exponent = egui::lerp(2.2..=6.4, t);
                let wobble = (1.0 - t).powf(1.4) * 0.24;
                let square_morph = match phase {
                    TransitionPhase::Intro => {
                        let square_seed = ((t - 0.08) / 0.66).clamp(0.0, 1.0);
                        square_seed * square_seed * (3.0 - 2.0 * square_seed)
                    }
                    TransitionPhase::Live => 1.0,
                };
                let ornament_alpha = if phase == TransitionPhase::Intro && progress >= 0.50 {
                    0.0
                } else {
                    ornament_alpha * (1.0 - square_morph).powf(1.7)
                };
                let preview_content_alpha =
                    Self::ease_in_out_cubic(((square_morph - 0.16) / 0.72).clamp(0.0, 1.0));
                let content_alpha = if blob_only_transition {
                    if intro_light_fade {
                        1.0
                    } else {
                        ornament_alpha
                    }
                } else {
                    preview_content_alpha
                };
                let mut card_fill = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Self::lerp_color(
                                Color32::from_rgba_premultiplied(
                                    16,
                                    13,
                                    20,
                                    (236.0 + (1.0 - intro_black_fade) * 12.0) as u8,
                                ),
                                Self::page_fill(),
                                intro_black_fade,
                            )
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(12, 9, 15, (232.0 + t * 18.0) as u8)
                        } else {
                            Color32::from_rgba_premultiplied(
                                255,
                                247,
                                251,
                                (208.0 + t * 28.0) as u8,
                            )
                        },
                        Self::page_fill(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha,
                );
                let mut card_stroke = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Self::lerp_color(
                                Color32::from_rgba_premultiplied(
                                    98,
                                    92,
                                    108,
                                    (92.0 + (1.0 - intro_black_fade) * 28.0) as u8,
                                ),
                                Self::border_color(),
                                intro_black_fade,
                            )
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(232, 162, 202, (84.0 + t * 56.0) as u8)
                        } else {
                            Color32::from_rgba_premultiplied(
                                229,
                                168,
                                199,
                                (116.0 + t * 68.0) as u8,
                            )
                        },
                        Self::border_color(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha,
                );
                let mut glaze_fill = Self::with_alpha(
                    Self::lerp_color(
                        if intro_monochrome {
                            Color32::TRANSPARENT
                        } else if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                214,
                                234,
                                (20.0 + (1.0 - aura) * 18.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                rose_ice.r(),
                                rose_ice.g(),
                                rose_ice.b(),
                                (40.0 + (1.0 - aura) * 28.0) as u8,
                            )
                        },
                        Self::page_fill(),
                        ui_match.max(1.0 - ornament_alpha),
                    ),
                    layer_alpha * ornament_alpha,
                );
                if intro_light_fade {
                    card_fill = Color32::from_rgb(206, 198, 211);
                    glaze_fill = Color32::TRANSPARENT;
                    card_stroke = Color32::from_rgb(156, 108, 136);
                }
                let wave_color = if light_transition {
                    Color32::from_rgb(214, 51, 132)
                } else {
                    Self::with_alpha(
                        if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                248,
                                252,
                                (118.0 + t * 120.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                magenta.r(),
                                magenta.g(),
                                magenta.b(),
                                (92.0 + t * 132.0) as u8,
                            )
                        },
                        layer_alpha * content_alpha,
                    )
                };
                let ribbon_color = if light_transition {
                    Color32::from_rgb(229, 85, 149)
                } else {
                    Self::with_alpha(
                        if self.dark_theme {
                            Color32::from_rgba_premultiplied(
                                255,
                                250,
                                252,
                                (136.0 + t * 108.0) as u8,
                            )
                        } else {
                            Color32::from_rgba_premultiplied(
                                berry.r(),
                                berry.g(),
                                berry.b(),
                                (118.0 + t * 124.0) as u8,
                            )
                        },
                        layer_alpha * content_alpha,
                    )
                };
                let note_base = if intro_monochrome {
                    Color32::from_rgb(246, 243, 248)
                } else if self.dark_theme {
                    Color32::from_rgb(246, 124, 181)
                } else if light_transition {
                    light_wave_primary
                } else {
                    Color32::from_rgb(214, 51, 132)
                };
                let note_alt = if intro_monochrome {
                    Color32::from_rgb(223, 216, 228)
                } else if self.dark_theme {
                    Color32::from_rgb(255, 188, 219)
                } else if light_transition {
                    light_wave_secondary
                } else {
                    Color32::from_rgb(236, 116, 179)
                };
                let note_glow_rgb = if intro_monochrome {
                    (255, 255, 255)
                } else if self.dark_theme {
                    (227, 82, 149)
                } else if light_transition {
                    light_wave_glow
                } else {
                    (16, 10, 14)
                };

                if blob_only_transition {
                    let pulse = 1.0 + audio_level * 0.14 + (time * 2.2).sin() * 0.03;
                    let phase_scale = match phase {
                        TransitionPhase::Intro => egui::lerp(0.84..=1.02, t),
                        TransitionPhase::Live => 1.0,
                    };
                    let blob_alpha = match phase {
                        TransitionPhase::Intro => egui::lerp(1.0..=0.9, t),
                        TransitionPhase::Live => 1.0,
                    };
                    let outer_points = Self::squircle_points(
                        center,
                        base * 0.34 * phase_scale * pulse,
                        base * 0.28 * phase_scale * pulse,
                        2.9,
                        0.12 + audio_level * 0.08,
                        time,
                    );
                    let middle_points = Self::squircle_points(
                        Pos2::new(center.x, center.y + 4.0),
                        base * 0.27 * phase_scale,
                        base * 0.22 * phase_scale,
                        3.2,
                        0.08 + audio_level * 0.06,
                        time + 0.45,
                    );
                    let core_points = Self::squircle_points(
                        center,
                        base * 0.16 * phase_scale,
                        base * 0.13 * phase_scale,
                        3.8,
                        0.05,
                        time + 0.2,
                    );
                    let outer_fill = if self.dark_theme {
                        Color32::from_rgba_premultiplied(242, 170, 207, 226)
                    } else {
                        Color32::from_rgba_premultiplied(246, 208, 230, 236)
                    };
                    let middle_fill = if self.dark_theme {
                        Color32::from_rgba_premultiplied(255, 236, 246, 176)
                    } else {
                        Color32::from_rgba_premultiplied(255, 245, 250, 200)
                    };
                    let core_fill = if self.dark_theme {
                        Color32::from_rgb(28, 18, 30)
                    } else {
                        Color32::from_rgb(50, 29, 45)
                    };
                    painter.add(egui::Shape::convex_polygon(
                        outer_points,
                        Self::with_alpha(outer_fill, blob_alpha),
                        Stroke::new(
                            1.3,
                            Self::with_alpha(Color32::from_rgb(239, 124, 190), blob_alpha),
                        ),
                    ));
                    painter.add(egui::Shape::convex_polygon(
                        middle_points,
                        Self::with_alpha(middle_fill, blob_alpha * 0.92),
                        Stroke::NONE,
                    ));
                    painter.add(egui::Shape::convex_polygon(
                        core_points,
                        Self::with_alpha(core_fill, blob_alpha),
                        Stroke::NONE,
                    ));

                    let bar_rect = Rect::from_center_size(
                        center,
                        vec2(base * 0.18 * phase_scale, base * 0.075 * phase_scale),
                    );
                    let clip = painter.with_clip_rect(bar_rect.expand2(vec2(8.0, 8.0)));
                    let bar_width = bar_rect.width() / wave_bars.len().max(1) as f32;
                    for (index, bar) in wave_bars.iter().enumerate() {
                        let x = bar_rect.left() + (index as f32 + 0.5) * bar_width;
                        let half = bar_rect.height() * (0.12 + *bar * 0.42);
                        let wave_rect = Rect::from_min_max(
                            Pos2::new(x - bar_width * 0.18, bar_rect.center().y - half),
                            Pos2::new(x + bar_width * 0.18, bar_rect.center().y + half),
                        );
                        clip.rect_filled(
                            wave_rect,
                            3.0,
                            Self::with_alpha(Color32::from_rgb(255, 231, 242), blob_alpha * 0.98),
                        );
                    }

                    for index in 0..6 {
                        let angle = time * 0.7 + index as f32 * 1.05;
                        let orbit = base * 0.18 + (index % 3) as f32 * 10.0;
                        let note_pos = Pos2::new(
                            center.x + angle.cos() * orbit,
                            center.y + angle.sin() * orbit * 0.8,
                        );
                        Self::paint_glowing_music_note(
                            &painter,
                            note_pos,
                            0.8 + ((index % 2) as f32 * 0.12),
                            (index as f32 * 0.17).sin() * 0.18,
                            if index % 2 == 0 {
                                Self::with_alpha(note_base, blob_alpha)
                            } else {
                                Self::with_alpha(note_alt, blob_alpha)
                            },
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    note_glow_rgb.0,
                                    note_glow_rgb.1,
                                    note_glow_rgb.2,
                                    72,
                                ),
                                blob_alpha,
                            ),
                        );
                    }
                    return;
                }

                if !blob_only_transition && !transparent_transition_backdrop && self.dark_theme {
                    painter.circle_filled(
                        center,
                        base * 0.56,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(9, 6, 12, (34.0 + aura * 54.0) as u8),
                            layer_alpha,
                        ),
                    );
                    painter.circle_filled(
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.42,
                        Self::with_alpha(
                            if intro_monochrome {
                                Color32::from_rgba_premultiplied(
                                    52,
                                    47,
                                    61,
                                    (14.0 + aura * 18.0) as u8,
                                )
                            } else {
                                Color32::from_rgba_premultiplied(
                                    120,
                                    25,
                                    72,
                                    (16.0 + aura * 34.0) as u8,
                                )
                            },
                            layer_alpha * ornament_alpha,
                        ),
                    );
                } else if !blob_only_transition
                    && !transparent_transition_backdrop
                    && phase != TransitionPhase::Live
                {
                    painter.circle_filled(
                        center,
                        base * 0.52,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_intro_base.r(),
                                light_intro_base.g(),
                                light_intro_base.b(),
                                (86.0 + aura * 64.0).round().clamp(0.0, 255.0) as u8,
                            ),
                            layer_alpha * ornament_alpha.max(0.45),
                        ),
                    );
                    painter.circle_filled(
                        Pos2::new(center.x, center.y + base * 0.02),
                        base * 0.40,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                light_wave_secondary.r(),
                                light_wave_secondary.g(),
                                light_wave_secondary.b(),
                                (72.0 + aura * 56.0).round().clamp(0.0, 255.0) as u8,
                            ),
                            layer_alpha * ornament_alpha,
                        ),
                    );
                }

                if !blob_only_transition && !transparent_transition_backdrop {
                    for star_index in 0..16 {
                        let seed = star_index as f32 * 11.713;
                        let px =
                            rect.left() + rect.width() * (0.18 + ((seed.sin() * 0.5 + 0.5) * 0.64));
                        let py = rect.top()
                            + rect.height() * (0.16 + (((seed * 1.7).cos() * 0.5 + 0.5) * 0.54));
                        let twinkle = 0.48 + ((time * 1.7 + seed).sin() * 0.52).abs();
                        painter.circle_filled(
                            Pos2::new(px, py),
                            0.8 + (star_index % 3) as f32 * 0.35,
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    star_rgb.0,
                                    star_rgb.1,
                                    star_rgb.2,
                                    (26.0 * star_alpha_scale * twinkle * (0.35 + aura * 0.65))
                                        as u8,
                                ),
                                layer_alpha * ornament_alpha,
                            ),
                        );
                    }
                }

                if !blob_only_transition {
                    let aura_layers = [
                        (
                            Pos2::new(center.x, center.y + base * 0.02),
                            base * 0.23,
                            base * 0.18,
                            Color32::from_rgba_premultiplied(
                                deep_plum.r(),
                                deep_plum.g(),
                                deep_plum.b(),
                                (74.0 * overlay) as u8,
                            ),
                            Color32::from_rgba_premultiplied(
                                plum.r(),
                                plum.g(),
                                plum.b(),
                                (52.0 + aura * 72.0) as u8,
                            ),
                        ),
                        (
                            Pos2::new(center.x, center.y + base * 0.018),
                            base * 0.31,
                            base * 0.24,
                            Color32::from_rgba_premultiplied(
                                plum.r(),
                                plum.g(),
                                plum.b(),
                                (58.0 * overlay) as u8,
                            ),
                            Color32::from_rgba_premultiplied(
                                magenta.r(),
                                magenta.g(),
                                magenta.b(),
                                (58.0 + aura * 82.0) as u8,
                            ),
                        ),
                        (
                            Pos2::new(center.x, center.y + base * 0.016),
                            base * 0.39,
                            base * 0.3,
                            Color32::from_rgba_premultiplied(
                                magenta.r(),
                                magenta.g(),
                                magenta.b(),
                                (42.0 * overlay) as u8,
                            ),
                            Color32::from_rgba_premultiplied(
                                berry.r(),
                                berry.g(),
                                berry.b(),
                                (50.0 + aura * 96.0) as u8,
                            ),
                        ),
                        (
                            Pos2::new(center.x, center.y + base * 0.022),
                            base * 0.47,
                            base * 0.35,
                            Color32::from_rgba_premultiplied(
                                berry.r(),
                                berry.g(),
                                berry.b(),
                                (28.0 * overlay) as u8,
                            ),
                            Color32::from_rgba_premultiplied(
                                rose_ice.r(),
                                rose_ice.g(),
                                rose_ice.b(),
                                (46.0 + aura * 88.0) as u8,
                            ),
                        ),
                    ];
                    for (layer_index, (layer_center, radius_x, radius_y, fill, stroke)) in
                        aura_layers.into_iter().enumerate()
                    {
                        let stage = target_rect.shrink(layer_index as f32 * 12.0);
                        let points = Self::morph_squircle_to_rect(
                            layer_center,
                            radius_x,
                            radius_y,
                            2.6 + layer_index as f32 * 0.18,
                            (0.18 - layer_index as f32 * 0.02).max(0.08),
                            time + layer_index as f32 * 0.16,
                            stage,
                            square_morph,
                        );
                        painter.add(egui::Shape::convex_polygon(
                            points,
                            Self::with_alpha(fill, layer_alpha * ornament_alpha),
                            Stroke::new(
                                (1.8 - layer_index as f32 * 0.24) * (1.0 - square_morph * 0.3),
                                Self::with_alpha(stroke, layer_alpha * ornament_alpha),
                            ),
                        ));
                    }

                    for (radius, alpha) in [
                        (base * 0.48, 28.0),
                        (base * 0.38, 42.0),
                        (base * 0.28, 68.0),
                        (base * 0.2, 96.0),
                    ] {
                        painter.circle_filled(
                            center,
                            egui::lerp((radius * 0.75)..=radius, 1.0 - aura * 0.22),
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    berry.r(),
                                    berry.g(),
                                    berry.b(),
                                    (alpha * (0.2 + aura * 0.8)) as u8,
                                ),
                                layer_alpha * ornament_alpha,
                            ),
                        );
                    }

                    let shadow_points = Self::morph_squircle_to_rect(
                        Pos2::new(center.x, center.y + 14.0 + aura * 12.0),
                        half_w * 1.02,
                        half_h * 1.02,
                        exponent,
                        wobble * 0.55,
                        time - 0.35,
                        target_rect.expand(8.0),
                        square_morph,
                    );
                    painter.add(egui::Shape::convex_polygon(
                        shadow_points,
                        Self::with_alpha(
                            Color32::from_rgba_premultiplied(
                                deep_plum.r(),
                                deep_plum.g(),
                                deep_plum.b(),
                                ((36.0 + t * 42.0) * (1.0 - square_morph * 0.82)) as u8,
                            ),
                            layer_alpha,
                        ),
                        Stroke::NONE,
                    ));
                }

                let card_points = Self::morph_squircle_to_rect(
                    center,
                    half_w,
                    half_h,
                    exponent,
                    wobble,
                    time,
                    target_rect,
                    square_morph,
                );
                painter.add(egui::Shape::convex_polygon(
                    card_points.clone(),
                    card_fill,
                    Stroke::new((1.2 - square_morph * 0.55).max(0.35), card_stroke),
                ));
                let rounded_card_lock_seed = ((square_morph - 0.9) / 0.1).clamp(0.0, 1.0);
                let rounded_card_lock = rounded_card_lock_seed
                    * rounded_card_lock_seed
                    * (3.0 - 2.0 * rounded_card_lock_seed);
                if rounded_card_lock > 0.0 {
                    painter.rect(
                        target_rect,
                        CornerRadius::same(APP_FRAME_RADIUS as u8),
                        Self::with_alpha(card_fill, rounded_card_lock),
                        Stroke::new(
                            egui::lerp(
                                (1.2 - square_morph * 0.55).max(0.35)..=1.0,
                                rounded_card_lock,
                            ),
                            Self::with_alpha(card_stroke, rounded_card_lock),
                        ),
                        StrokeKind::Outside,
                    );
                }
                let glaze_center = Pos2::new(
                    center.x,
                    egui::lerp(
                        (center.y - half_h * 0.06)
                            ..=(target_rect.top() + target_rect.height() * 0.22),
                        square_morph,
                    ),
                );
                let glaze_size = vec2(
                    egui::lerp((half_w * 1.84)..=(target_rect.width() * 0.84), square_morph),
                    egui::lerp((half_h * 0.96)..=(target_rect.height() * 0.4), square_morph),
                );
                let glaze_rect = Rect::from_center_size(glaze_center, glaze_size);
                let glaze_points = Self::morph_squircle_to_rect(
                    glaze_center,
                    half_w * 0.92,
                    half_h * 0.54,
                    exponent,
                    wobble * 0.4,
                    time + 0.8,
                    glaze_rect,
                    square_morph * 0.94,
                );
                painter.add(egui::Shape::convex_polygon(
                    glaze_points,
                    glaze_fill,
                    Stroke::NONE,
                ));
                if rounded_card_lock > 0.0 {
                    let top_glaze_rect = Rect::from_min_max(
                        Pos2::new(target_rect.left() + 18.0, target_rect.top() + 14.0),
                        Pos2::new(
                            target_rect.right() - 18.0,
                            target_rect.top() + target_rect.height() * 0.34,
                        ),
                    );
                    painter.rect(
                        top_glaze_rect,
                        CornerRadius::same(((APP_FRAME_RADIUS * 0.85).round() as u8).max(8)),
                        Self::with_alpha(glaze_fill, rounded_card_lock * 0.92),
                        Stroke::NONE,
                        StrokeKind::Outside,
                    );
                }

                if !blob_only_transition {
                    let inner_rect = Rect::from_center_size(
                        center,
                        vec2(half_w * 1.08, half_h * 0.9).min(target_rect.size() * 0.78),
                    );
                    let module_rect = Rect::from_center_size(
                        Pos2::new(center.x, center.y + half_h * 0.1),
                        vec2(inner_rect.width() * 0.82, inner_rect.height() * 0.7),
                    );
                    let module_inner = module_rect.shrink2(vec2(24.0, 18.0));
                    let clip = painter.with_clip_rect(module_inner.expand2(vec2(8.0, 8.0)));
                    let accent_rect = Rect::from_center_size(
                        Pos2::new(module_rect.center().x, module_rect.top() + 26.0),
                        vec2(module_rect.width() * 0.56, 14.0 + t * 3.0),
                    );
                    let band_rect = Rect::from_min_max(
                        Pos2::new(module_inner.left(), module_rect.top() + 56.0),
                        Pos2::new(module_inner.right(), module_rect.top() + 140.0),
                    );
                    if light_transition {
                        clip.rect_filled(
                            band_rect.expand2(vec2(18.0, 14.0)),
                            18.0,
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    light_wave_secondary.r(),
                                    light_wave_secondary.g(),
                                    light_wave_secondary.b(),
                                    52,
                                ),
                                layer_alpha * content_alpha,
                            ),
                        );
                    }
                    let bar_width = band_rect.width() / wave_bars.len().max(1) as f32;
                    for (index, bar) in wave_bars.iter().enumerate() {
                        let phase_shift = time * 3.2 + index as f32 * 0.44;
                        let animated = (*bar + 0.06_f32 * phase_shift.sin()).clamp(0.16, 1.0);
                        let x = band_rect.left() + (index as f32 + 0.5) * bar_width;
                        let half = animated * band_rect.height() * (0.2 + t * 0.3);
                        let wave_rect = Rect::from_min_max(
                            Pos2::new(x - bar_width * 0.22, band_rect.center().y - half),
                            Pos2::new(x + bar_width * 0.22, band_rect.center().y + half),
                        );
                        clip.rect_filled(wave_rect, 4.0, wave_color);
                    }

                    let ribbon_rect = Rect::from_min_max(
                        Pos2::new(module_inner.left(), module_rect.bottom() - 84.0),
                        Pos2::new(module_inner.right(), module_rect.bottom() - 26.0),
                    );
                    if light_transition {
                        clip.rect_filled(
                            ribbon_rect.expand2(vec2(22.0, 16.0)),
                            20.0,
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    light_wave_primary.r(),
                                    light_wave_primary.g(),
                                    light_wave_primary.b(),
                                    44,
                                ),
                                layer_alpha * content_alpha,
                            ),
                        );
                    }
                    let mut line = Vec::with_capacity(120);
                    for step in 0..120 {
                        let sample_t = step as f32 / 119.0;
                        let x = egui::lerp(ribbon_rect.left()..=ribbon_rect.right(), sample_t);
                        let ribbon_energy = Self::sample_waveform_level(
                            &self.startup.sound_waveform,
                            (audio_progress + (sample_t - 0.5) * 0.18).clamp(0.0, 1.0),
                        );
                        let y = ribbon_rect.center().y
                            + (sample_t * std::f32::consts::TAU * 2.2 + time * 2.9).sin()
                                * ribbon_rect.height()
                                * (0.18 + ribbon_energy * 0.34)
                            + (sample_t * std::f32::consts::TAU * 5.8 - time * 1.5).cos()
                                * ribbon_rect.height()
                                * (0.08 + ribbon_energy * 0.14);
                        line.push(Pos2::new(x, y));
                    }
                    clip.add(egui::Shape::line(line, Stroke::new(4.0, ribbon_color)));
                    clip.rect_filled(
                        accent_rect,
                        9.0,
                        if light_transition {
                            Color32::from_rgb(229, 85, 149)
                        } else {
                            Self::with_alpha(
                                Color32::from_rgba_premultiplied(
                                    rose_ice.r(),
                                    rose_ice.g(),
                                    rose_ice.b(),
                                    (34.0 + t * 38.0) as u8,
                                ),
                                layer_alpha * content_alpha,
                            )
                        },
                    );

                    for index in 0..7 {
                        let angle = time * 0.72 + index as f32 * 0.9;
                        let orbit = egui::lerp(
                            (module_rect.width() * 0.28)..=(module_rect.width() * 0.14),
                            t,
                        ) + (index % 3) as f32 * 10.0
                            + audio_level * 8.0;
                        let note_anchor = Pos2::new(
                            module_rect.center().x,
                            module_rect.top() + module_rect.height() * 0.4,
                        );
                        let note_pos = Pos2::new(
                            note_anchor.x + angle.cos() * orbit,
                            note_anchor.y + angle.sin() * orbit * 0.62,
                        );
                        let note_scale = 0.64 + (index % 3) as f32 * 0.12 + audio_level * 0.12;
                        let note_alpha = if light_transition {
                            (196.0 + aura * 52.0).clamp(0.0, 248.0) as u8
                        } else {
                            (160.0 * aura).clamp(0.0, 160.0) as u8
                        };
                        let note_color = if index % 2 == 0 {
                            if light_transition {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_base.r(),
                                        note_base.g(),
                                        note_base.b(),
                                        note_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            } else {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_base.r(),
                                        note_base.g(),
                                        note_base.b(),
                                        note_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            }
                        } else {
                            if light_transition {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_alt.r(),
                                        note_alt.g(),
                                        note_alt.b(),
                                        note_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            } else {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_alt.r(),
                                        note_alt.g(),
                                        note_alt.b(),
                                        note_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            }
                        };
                        let glow_alpha = if light_transition {
                            (92.0 + aura * 54.0).clamp(0.0, 168.0) as u8
                        } else if self.dark_theme {
                            (110.0 * aura).clamp(0.0, 148.0) as u8
                        } else {
                            (88.0 * aura).clamp(0.0, 128.0) as u8
                        };
                        Self::paint_glowing_music_note(
                            &painter,
                            note_pos,
                            note_scale,
                            angle.sin() * 0.18,
                            note_color,
                            if light_transition {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_glow_rgb.0,
                                        note_glow_rgb.1,
                                        note_glow_rgb.2,
                                        glow_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            } else {
                                Self::with_alpha(
                                    Color32::from_rgba_premultiplied(
                                        note_glow_rgb.0,
                                        note_glow_rgb.1,
                                        note_glow_rgb.2,
                                        glow_alpha,
                                    ),
                                    layer_alpha * content_alpha,
                                )
                            },
                        );
                    }
                }
            });
    }

    pub(super) fn transition_audio_progress(&self, ctx: &Context) -> Option<f32> {
        let started_at = self.startup.started_at?;
        let duration_sec = self.startup.sound_duration_sec.max(0.0);
        if duration_sec <= 0.0 {
            return None;
        }

        let now = ctx.input(|input| input.time);
        Some(((now - started_at) as f32 / duration_sec).clamp(0.0, 1.0))
    }

    pub(super) fn with_alpha(color: Color32, factor: f32) -> Color32 {
        let factor = factor.clamp(0.0, 1.0);
        Color32::from_rgba_premultiplied(
            color.r(),
            color.g(),
            color.b(),
            ((color.a() as f32) * factor).round().clamp(0.0, 255.0) as u8,
        )
    }

    pub(super) fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        Color32::from_rgba_premultiplied(
            egui::lerp(from.r() as f32..=to.r() as f32, t).round() as u8,
            egui::lerp(from.g() as f32..=to.g() as f32, t).round() as u8,
            egui::lerp(from.b() as f32..=to.b() as f32, t).round() as u8,
            egui::lerp(from.a() as f32..=to.a() as f32, t).round() as u8,
        )
    }

    pub(super) fn sample_waveform_level(waveform: &[f32], progress: f32) -> f32 {
        if waveform.is_empty() {
            return 0.45;
        }
        if waveform.len() == 1 {
            return waveform[0].clamp(0.05, 1.0);
        }

        let position = progress.clamp(0.0, 1.0) * (waveform.len() - 1) as f32;
        let left_index = position.floor() as usize;
        let right_index = position.ceil() as usize;
        let mix = (position - left_index as f32).clamp(0.0, 1.0);
        let left = waveform[left_index].clamp(0.05, 1.0);
        let right = waveform[right_index.min(waveform.len() - 1)].clamp(0.05, 1.0);
        egui::lerp(left..=right, mix)
    }

    pub(super) fn transition_wave_bars(waveform: &[f32], progress: f32, count: usize) -> Vec<f32> {
        if count == 0 {
            return Vec::new();
        }
        if waveform.is_empty() {
            return vec![
                0.24, 0.42, 0.76, 0.94, 0.56, 0.28, 0.68, 0.88, 0.5, 0.22, 0.62,
            ];
        }

        let mut bars = Vec::with_capacity(count);
        let window = 0.26;
        let middle = (count.saturating_sub(1)) as f32 * 0.5;
        for index in 0..count {
            let offset = if count <= 1 {
                0.0
            } else {
                (index as f32 - middle) / middle.max(1.0)
            };
            let sample_progress = (progress + offset * window).clamp(0.0, 1.0);
            bars.push(Self::sample_waveform_level(waveform, sample_progress));
        }
        bars
    }

    pub(super) fn transition_target_rect(rect: Rect) -> Rect {
        Rect::from_min_max(
            Pos2::new(
                rect.left() + APP_OUTER_MARGIN,
                rect.top() + APP_OUTER_MARGIN,
            ),
            Pos2::new(
                rect.right() - APP_OUTER_MARGIN,
                rect.bottom() - APP_OUTER_MARGIN,
            ),
        )
    }

    pub(super) fn squircle_points(
        center: Pos2,
        half_w: f32,
        half_h: f32,
        exponent: f32,
        wobble: f32,
        time: f32,
    ) -> Vec<Pos2> {
        let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);
        for step in 0..TRANSITION_POINT_COUNT {
            let angle = step as f32 / TRANSITION_POINT_COUNT as f32 * std::f32::consts::TAU;
            let cos = angle.cos();
            let sin = angle.sin();
            let power = 2.0 / exponent.max(2.0);
            let x = cos.signum() * cos.abs().powf(power) * half_w;
            let y = sin.signum() * sin.abs().powf(power) * half_h;
            let drift = 1.0
                + wobble * (angle * 3.0 + time * 1.4).sin()
                + wobble * 0.45 * (angle * 5.0 - time * 0.9).cos();
            points.push(Pos2::new(center.x + x * drift, center.y + y * drift));
        }
        points
    }

    pub(super) fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<Pos2> {
        let half_w = rect.width().max(1.0) * 0.5;
        let half_h = rect.height().max(1.0) * 0.5;
        let radius = radius.min(half_w).min(half_h).max(0.0);
        let inner_half_w = (half_w - radius).max(0.0);
        let inner_half_h = (half_h - radius).max(0.0);
        let right = rect.right();
        let left = rect.left();
        let top = rect.top();
        let bottom = rect.bottom();
        let center_y = rect.center().y;

        if radius <= 0.0 {
            let segments = [
                ((right, center_y), (right, bottom)),
                ((right, bottom), (left, bottom)),
                ((left, bottom), (left, top)),
                ((left, top), (right, top)),
                ((right, top), (right, center_y)),
            ];
            let total = (bottom - center_y)
                + rect.width()
                + rect.height()
                + rect.width()
                + (center_y - top);
            let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);
            for step in 0..TRANSITION_POINT_COUNT {
                let mut distance = step as f32 / TRANSITION_POINT_COUNT as f32 * total;
                for &((x1, y1), (x2, y2)) in &segments {
                    let length = (x2 - x1).abs() + (y2 - y1).abs();
                    if distance <= length || length <= f32::EPSILON {
                        let t = if length <= f32::EPSILON {
                            0.0
                        } else {
                            distance / length
                        };
                        points.push(Pos2::new(egui::lerp(x1..=x2, t), egui::lerp(y1..=y2, t)));
                        break;
                    }
                    distance -= length;
                }
            }
            return points;
        }

        let right_half = inner_half_h;
        let vertical = inner_half_h * 2.0;
        let horizontal = inner_half_w * 2.0;
        let arc = std::f32::consts::FRAC_PI_2 * radius;
        let total = right_half * 2.0 + vertical + horizontal * 2.0 + arc * 4.0;
        let mut points = Vec::with_capacity(TRANSITION_POINT_COUNT);

        for step in 0..TRANSITION_POINT_COUNT {
            let mut distance = step as f32 / TRANSITION_POINT_COUNT as f32 * total;

            if distance <= right_half {
                points.push(Pos2::new(right, center_y + distance));
                continue;
            }
            distance -= right_half;

            if distance <= arc {
                let angle = egui::lerp(
                    0.0..=std::f32::consts::FRAC_PI_2,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    right - radius + angle.cos() * radius,
                    bottom - radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= horizontal {
                points.push(Pos2::new(right - radius - distance, bottom));
                continue;
            }
            distance -= horizontal;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::FRAC_PI_2..=std::f32::consts::PI,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    left + radius + angle.cos() * radius,
                    bottom - radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= vertical {
                points.push(Pos2::new(left, bottom - radius - distance));
                continue;
            }
            distance -= vertical;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::PI..=std::f32::consts::PI * 1.5,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    left + radius + angle.cos() * radius,
                    top + radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            if distance <= horizontal {
                points.push(Pos2::new(left + radius + distance, top));
                continue;
            }
            distance -= horizontal;

            if distance <= arc {
                let angle = egui::lerp(
                    std::f32::consts::PI * 1.5..=std::f32::consts::TAU,
                    distance / arc.max(f32::EPSILON),
                );
                points.push(Pos2::new(
                    right - radius + angle.cos() * radius,
                    top + radius + angle.sin() * radius,
                ));
                continue;
            }
            distance -= arc;

            points.push(Pos2::new(right, top + radius + distance));
        }

        points
    }

    pub(super) fn morph_squircle_to_rect(
        center: Pos2,
        half_w: f32,
        half_h: f32,
        exponent: f32,
        wobble: f32,
        time: f32,
        target_rect: Rect,
        morph: f32,
    ) -> Vec<Pos2> {
        let blob = Self::squircle_points(center, half_w, half_h, exponent, wobble, time);
        if morph <= 0.0 {
            return blob;
        }

        let settled_blob = Self::squircle_points(
            target_rect.center(),
            target_rect.width() * 0.5,
            target_rect.height() * 0.5,
            egui::lerp(exponent.max(2.0)..=8.8, morph),
            wobble * (1.0 - morph * 0.82).max(0.0),
            time,
        );
        let rounded_card = Self::rounded_rect_points(target_rect, APP_FRAME_RADIUS);
        let corner_lock = morph.clamp(0.0, 1.0).powf(2.4);
        let target_shape = settled_blob
            .into_iter()
            .zip(rounded_card)
            .map(|(blob_point, rounded_point)| {
                Pos2::new(
                    egui::lerp(blob_point.x..=rounded_point.x, corner_lock),
                    egui::lerp(blob_point.y..=rounded_point.y, corner_lock),
                )
            })
            .collect::<Vec<_>>();
        let eased_morph = morph * morph * (3.0 - 2.0 * morph);

        blob.into_iter()
            .zip(target_shape)
            .map(|(blob_point, target_point)| {
                Pos2::new(
                    egui::lerp(blob_point.x..=target_point.x, eased_morph),
                    egui::lerp(blob_point.y..=target_point.y, eased_morph),
                )
            })
            .collect()
    }

    pub(super) fn ease_in_out_cubic(t: f32) -> f32 {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        }
    }

    pub(super) fn paint_music_note(
        painter: &egui::Painter,
        center: Pos2,
        scale: f32,
        tilt: f32,
        color: Color32,
    ) {
        let head = Vec2::new(12.0 * scale, 8.0 * scale);
        let stem = 18.0 * scale;
        let head_center = Pos2::new(center.x, center.y + 6.0 * scale);
        painter.circle_filled(head_center, head.y * 0.8, color);
        let stem_top = Pos2::new(head_center.x + head.x * 0.5, head_center.y - stem);
        painter.line_segment(
            [
                Pos2::new(head_center.x + head.x * 0.4, head_center.y),
                stem_top,
            ],
            Stroke::new((2.8 * scale).max(1.0), color),
        );
        let flag_end = Pos2::new(
            stem_top.x + (12.0 * scale) * (1.0 + tilt),
            stem_top.y + 8.0 * scale,
        );
        painter.line_segment(
            [stem_top, flag_end],
            Stroke::new((2.4 * scale).max(1.0), color),
        );
    }

    pub(super) fn paint_glowing_music_note(
        painter: &egui::Painter,
        center: Pos2,
        scale: f32,
        tilt: f32,
        color: Color32,
        glow_color: Color32,
    ) {
        for (glow_scale, alpha_scale) in [(1.34, 0.22), (1.2, 0.38), (1.08, 0.62)] {
            let alpha = ((glow_color.a() as f32) * alpha_scale).clamp(0.0, 255.0) as u8;
            let glow = Color32::from_rgba_unmultiplied(
                glow_color.r(),
                glow_color.g(),
                glow_color.b(),
                alpha,
            );
            Self::paint_music_note(painter, center, scale * glow_scale, tilt, glow);
        }
        Self::paint_music_note(painter, center, scale, tilt, color);
    }
}
