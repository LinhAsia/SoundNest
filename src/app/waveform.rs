use super::*;

impl SoundFxApp {
    pub(super) fn cached_stem_waveform(
        &self,
        cache: &RefCell<HashMap<Uuid, Vec<f32>>>,
        sound_id: Uuid,
        path: &Path,
    ) -> Vec<f32> {
        if let Some(existing) = cache.borrow().get(&sound_id).cloned() {
            return existing;
        }

        let Ok(waveform) = self.storage.analyze_waveform_preview(path, 96) else {
            return Vec::new();
        };

        cache.borrow_mut().insert(sound_id, waveform.clone());
        waveform
    }

    pub(super) fn sound_waveform_samples(&self, sound: &SoundEffect) -> Vec<f32> {
        Self::scale_waveform_for_volume(self.raw_sound_waveform_samples(sound), sound.volume)
    }

    pub(super) fn raw_sound_waveform_samples(&self, sound: &SoundEffect) -> Vec<f32> {
        let samples = if sound.music_only
            && let Some(path) = sound.music_asset_path(self.storage.root_dir())
            && path.exists()
        {
            self.cached_stem_waveform(&self.music_waveform_cache, sound.id, &path)
        } else if sound.vocal_only
            && let Some(path) = sound.vocal_asset_path(self.storage.root_dir())
            && path.exists()
        {
            self.cached_stem_waveform(&self.vocal_waveform_cache, sound.id, &path)
        } else {
            sound.waveform.clone()
        };

        samples
    }

    pub(super) fn scale_waveform_for_volume(mut samples: Vec<f32>, volume: f32) -> Vec<f32> {
        let volume = volume.clamp(0.0, 5.0);
        samples
            .iter_mut()
            .for_each(|sample| *sample = (*sample * volume).clamp(0.0, 1.0));
        samples
    }

    pub(super) fn recording_waveform_samples(&self, draft: &RecordingDraft) -> Vec<f32> {
        let samples = if draft.keep_music
            && let Some(path) = draft.music_separated_path.as_ref()
            && path.exists()
        {
            self.cached_stem_waveform(&self.music_waveform_cache, draft.sound.id, path)
        } else if draft.keep_vocal
            && let Some(path) = draft.vocal_separated_path.as_ref()
            && path.exists()
        {
            self.cached_stem_waveform(&self.vocal_waveform_cache, draft.sound.id, path)
        } else {
            draft.sound.waveform.clone()
        };

        Self::scale_waveform_for_volume(samples, draft.sound.volume)
    }

    pub(super) fn cached_library_waveform_preview(
        &self,
        sound: &SoundEffect,
        buckets: usize,
    ) -> Vec<f32> {
        let cache_key = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            sound.id,
            sound.asset_file,
            sound.vocal_asset_file.as_deref().unwrap_or(""),
            sound.music_asset_file.as_deref().unwrap_or(""),
            sound.music_only,
            sound.vocal_only,
            (sound.volume * 1000.0).round() as i32,
            (sound.trim_start_secs * 1000.0).round() as i32,
            (sound.trim_end_secs * 1000.0).round() as i32,
            sound
                .cut_start_secs
                .map(|value| (value * 1000.0).round() as i32)
                .unwrap_or(-1),
            sound
                .cut_end_secs
                .map(|value| (value * 1000.0).round() as i32)
                .unwrap_or(-1),
        );
        let cache_key = format!("{cache_key}:{buckets}");
        if let Some(existing) = self
            .library_waveform_preview_cache
            .borrow()
            .get(&cache_key)
            .cloned()
        {
            return existing;
        }

        let waveform_samples = self.raw_sound_waveform_samples(sound);
        let preview = Self::scale_waveform_for_volume(
            Self::library_sound_waveform_preview_from_samples(sound, &waveform_samples, buckets),
            sound.volume,
        );
        self.library_waveform_preview_cache
            .borrow_mut()
            .insert(cache_key, preview.clone());
        preview
    }

    pub(super) fn trimmed_waveform_preview_from_samples(
        sound: &SoundEffect,
        samples: &[f32],
    ) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration();
        let source_len = samples.len();
        let ranges = sound.trim_ranges();
        if source_len <= 1
            || (ranges.len() == 1 && ranges[0].0 <= 0.001 && ranges[0].1 >= total_duration - 0.001)
        {
            return samples.to_vec();
        }
        let mut stitched = Vec::new();
        for (trim_start, trim_end) in ranges {
            let start_index = ((trim_start / total_duration) * source_len as f32).floor() as usize;
            let mut end_index = ((trim_end / total_duration) * source_len as f32).ceil() as usize;
            end_index = end_index.clamp(start_index.saturating_add(1), source_len);
            stitched.extend_from_slice(&samples[start_index.min(source_len - 1)..end_index]);
        }
        let segment = if stitched.is_empty() {
            samples
        } else {
            &stitched
        };
        if segment.len() >= source_len {
            return segment.to_vec();
        }

        let mut preview = Vec::with_capacity(source_len);
        for target_index in 0..source_len {
            let start =
                ((target_index as f32 / source_len as f32) * segment.len() as f32).floor() as usize;
            let mut end = ((((target_index + 1) as f32) / source_len as f32) * segment.len() as f32)
                .ceil() as usize;
            let start = start.min(segment.len().saturating_sub(1));
            end = end.clamp(start + 1, segment.len());

            let mut peak = 0.0_f32;
            for sample in &segment[start..end] {
                peak = peak.max(*sample);
            }
            preview.push(peak.max(0.02));
        }

        preview
    }

    pub(super) fn library_sound_waveform_preview_from_samples(
        sound: &SoundEffect,
        samples: &[f32],
        buckets: usize,
    ) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration();
        let source_len = samples.len();
        let mut stitched = Vec::new();
        for (trim_start, trim_end) in sound.trim_ranges() {
            let start_index = ((trim_start / total_duration) * source_len as f32).floor() as usize;
            let mut end_index = ((trim_end / total_duration) * source_len as f32).ceil() as usize;
            end_index = end_index.clamp(start_index.saturating_add(1), source_len);
            stitched.extend_from_slice(
                &samples[start_index.min(source_len.saturating_sub(1))..end_index],
            );
        }
        let segment = if stitched.is_empty() {
            samples
        } else {
            &stitched
        };
        Self::compact_library_waveform(segment, buckets)
    }

    pub(super) fn timeline_clip_waveform_preview_from_samples(
        sound: &SoundEffect,
        samples: &[f32],
        clip_start_secs: f32,
        clip_end_secs: f32,
        buckets: usize,
    ) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let total_duration = sound.safe_duration().max(0.05);
        let start_ratio = (clip_start_secs / total_duration).clamp(0.0, 1.0);
        let end_ratio = (clip_end_secs / total_duration).clamp(start_ratio, 1.0);
        Self::resample_timeline_waveform(samples, start_ratio, end_ratio, buckets)
    }

    pub(super) fn compact_timeline_waveform(samples: &[f32], buckets: usize) -> Vec<f32> {
        Self::resample_timeline_waveform(samples, 0.0, 1.0, buckets)
    }

    fn resample_timeline_waveform(
        samples: &[f32],
        start_ratio: f32,
        end_ratio: f32,
        buckets: usize,
    ) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let bucket_count = buckets.clamp(1, 256).max(1);
        let mut preview = Vec::with_capacity(bucket_count);
        let range_start = start_ratio.clamp(0.0, 1.0);
        let range_end = end_ratio.clamp(range_start, 1.0);
        let start_index = ((range_start * samples.len() as f32).floor() as usize)
            .min(samples.len().saturating_sub(1));
        let end_index = ((range_end * samples.len() as f32).ceil() as usize)
            .clamp(start_index + 1, samples.len());
        let range_len = (end_index - start_index).max(1);

        for bucket_index in 0..bucket_count {
            let bucket_start = start_index
                + (((bucket_index as f32 / bucket_count as f32) * range_len as f32).floor()
                    as usize)
                    .min(range_len.saturating_sub(1));
            let mut bucket_end = start_index
                + (((((bucket_index + 1) as f32) / bucket_count as f32) * range_len as f32).ceil()
                    as usize);
            bucket_end = bucket_end.clamp(bucket_start + 1, end_index);

            let slice = &samples[bucket_start..bucket_end];
            let mut peak = 0.0_f32;
            let mut energy = 0.0_f32;
            for sample in slice {
                peak = peak.max(*sample);
                energy += sample * sample;
            }
            let rms = (energy / slice.len() as f32).sqrt();
            preview.push((peak * 0.76 + rms * 0.24).clamp(0.0, 1.0));
        }

        for value in &mut preview {
            *value = Self::wave_strip_level(*value).clamp(0.02, 1.0);
        }

        preview
    }

    pub(super) fn compact_library_waveform(samples: &[f32], buckets: usize) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let bucket_count = buckets.max(12);
        let mut preview = Vec::with_capacity(bucket_count);

        for bucket_index in 0..bucket_count {
            let start = ((bucket_index as f32 / bucket_count as f32) * samples.len() as f32).floor()
                as usize;
            let mut end = ((((bucket_index + 1) as f32) / bucket_count as f32)
                * samples.len() as f32)
                .ceil() as usize;
            let start = start.min(samples.len().saturating_sub(1));
            end = end.clamp(start + 1, samples.len());

            let slice = &samples[start..end];
            let mut peak = 0.0_f32;
            let mut energy = 0.0_f32;
            for sample in slice {
                peak = peak.max(*sample);
                energy += sample * sample;
            }
            let rms = (energy / slice.len() as f32).sqrt();
            preview.push((peak * 0.62 + rms * 0.38).powf(1.12));
        }

        let max_level = preview
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(f32::EPSILON);
        for value in &mut preview {
            *value = (*value / max_level).clamp(0.0, 1.0);
            if *value < 0.06 {
                *value *= 0.5;
            }
        }

        preview
    }

    pub(super) fn paint_waveform_bars(
        painter: &egui::Painter,
        rect: Rect,
        waveform: &[f32],
        start_x: f32,
        end_x: f32,
        progress: Option<f32>,
    ) {
        let dark_theme = Self::dark_theme_enabled();
        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.center().y),
                    Pos2::new(rect.right(), rect.center().y),
                ],
                Stroke::new(
                    2.0,
                    if dark_theme {
                        Color32::from_rgba_premultiplied(255, 255, 255, 140)
                    } else {
                        Color32::from_rgb(221, 214, 220)
                    },
                ),
            );
            if let Some(progress) = progress {
                let play_x = egui::lerp(rect.left()..=rect.right(), progress.clamp(0.0, 1.0));
                painter.line_segment(
                    [
                        Pos2::new(play_x, rect.top()),
                        Pos2::new(play_x, rect.bottom()),
                    ],
                    Stroke::new(
                        2.0,
                        if dark_theme {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(42, 39, 44)
                        },
                    ),
                );
            }
            return;
        }

        let bar_width = rect.width() / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.05, 1.0);
            let center_x = rect.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * rect.height() * 0.42;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.3).max(1.0),
                    rect.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.3).max(1.0),
                    rect.center().y + half,
                ),
            );
            let selected = center_x >= start_x && center_x <= end_x;
            let color = if selected {
                if dark_theme {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(227, 82, 149)
                }
            } else {
                if dark_theme {
                    Color32::from_rgba_premultiplied(255, 255, 255, 188)
                } else {
                    Color32::from_rgb(234, 214, 226)
                }
            };
            painter.rect_filled(wave_rect, 2.0, color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(start_x..=end_x.max(start_x + 1.0), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, rect.top()),
                    Pos2::new(play_x, rect.bottom()),
                ],
                Stroke::new(
                    2.0,
                    if dark_theme {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(34, 31, 36)
                    },
                ),
            );
        }
    }

    pub(super) fn paint_timeline_waveform_columns(
        painter: &egui::Painter,
        rect: Rect,
        waveform: &[f32],
        color: Color32,
        highlight: bool,
    ) {
        painter.line_segment(
            [
                Pos2::new(rect.left(), rect.bottom() - 4.0),
                Pos2::new(rect.right(), rect.bottom() - 4.0),
            ],
            Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 88)),
        );

        if waveform.is_empty() {
            return;
        }

        let column_count = waveform.len().max(1);
        let step = rect.width() / column_count as f32;
        let column_width = (step * 0.42).clamp(1.0, 2.2).min(rect.width().max(1.0));
        let stride = if column_count <= 1 {
            0.0
        } else {
            ((rect.width() - column_width).max(0.0)) / (column_count - 1) as f32
        };
        let baseline = (rect.bottom() - 2.0).round();
        let min_height = 2.5;
        let max_height = ((rect.height() - 4.0).max(min_height) - min_height).max(2.0);
        let wave_color = if highlight {
            Color32::from_rgb(255, 230, 244)
        } else {
            color
        };

        for (index, level) in waveform.iter().enumerate() {
            let amplitude = level.clamp(0.02, 1.0);
            let left = if column_count <= 1 {
                rect.left()
            } else {
                rect.left() + index as f32 * stride
            }
            .round();
            let right = if index + 1 >= column_count {
                rect.right()
            } else {
                (left + column_width).min(rect.right())
            }
            .round()
            .max(left + 1.0);
            let height = min_height + amplitude * max_height;
            let wave_rect = Rect::from_min_max(
                Pos2::new(left, (baseline - height).round()),
                Pos2::new(right, baseline),
            );
            painter.rect_filled(wave_rect, 1.5, wave_color);
        }
    }

    pub(super) fn draw_record_wave_strip(ui: &mut Ui, waveform: &[f32]) {
        let desired = vec2(ui.available_width().max(240.0), 60.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        let dark_theme = Self::dark_theme_enabled();
        painter.rect_filled(
            rect,
            20.0,
            if dark_theme {
                Color32::from_rgba_premultiplied(14, 10, 18, 200)
            } else {
                Color32::from_rgba_premultiplied(242, 236, 244, 220)
            },
        );
        painter.rect_stroke(
            rect,
            20.0,
            Stroke::new(
                1.0,
                if dark_theme {
                    Color32::from_rgba_premultiplied(214, 51, 132, 70)
                } else {
                    Color32::from_rgba_premultiplied(214, 51, 132, 90)
                },
            ),
            StrokeKind::Inside,
        );

        let time = ui.input(|i| i.time) as f32;
        let data: Vec<f32> = if waveform.is_empty() {
            (0..36)
                .map(|i| {
                    let phase = i as f32 * 0.25 + time * 2.0;
                    0.08 + phase.sin().abs() * 0.12
                })
                .collect()
        } else {
            waveform.to_vec()
        };
        let inner = rect.shrink2(vec2(16.0, 10.0));
        let bar_width = inner.width() / data.len().max(1) as f32;
        for (index, value) in data.iter().enumerate() {
            let x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = value.clamp(0.04, 1.0) * inner.height() * 0.46;
            let bar = Rect::from_min_max(
                Pos2::new(x - (bar_width * 0.24).max(1.0), inner.center().y - half),
                Pos2::new(x + (bar_width * 0.24).max(1.0), inner.center().y + half),
            );
            painter.rect_filled(bar, 3.0, Color32::from_rgb(227, 82, 149));
        }
    }

    pub(super) fn draw_stream_wave_strip(ui: &mut Ui, waveform: &[f32], active: bool) {
        let desired = vec2(ui.available_width().max(220.0), 46.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        let dark_theme = Self::dark_theme_enabled();
        painter.rect_filled(
            rect,
            18.0,
            if dark_theme {
                Color32::from_rgba_premultiplied(255, 255, 255, 14)
            } else {
                Color32::from_rgba_premultiplied(255, 255, 255, 96)
            },
        );
        painter.rect_stroke(
            rect,
            18.0,
            Stroke::new(
                1.0,
                if dark_theme {
                    Color32::from_rgba_premultiplied(111, 86, 120, 150)
                } else {
                    Color32::from_rgba_premultiplied(231, 214, 224, 180)
                },
            ),
            StrokeKind::Outside,
        );

        let data = if waveform.is_empty() {
            vec![0.05; 40]
        } else {
            waveform.to_vec()
        };
        let inner = rect.shrink2(vec2(12.0, 8.0));
        let bar_width = inner.width() / data.len().max(1) as f32;
        let active_color = if active {
            Color32::from_rgb(227, 82, 149)
        } else {
            Color32::from_rgba_premultiplied(184, 132, 164, 120)
        };
        for (index, value) in data.iter().enumerate() {
            let x = inner.left() + (index as f32 + 0.5) * bar_width;
            let amplitude = Self::boost_stream_meter_level(*value);
            let half = amplitude * inner.height() * 0.44;
            let bar = Rect::from_min_max(
                Pos2::new(x - bar_width * 0.22, inner.center().y - half),
                Pos2::new(x + bar_width * 0.22, inner.center().y + half),
            );
            painter.rect_filled(bar, 3.0, active_color);
        }
    }

    pub(super) fn draw_wave_strip(
        ui: &mut Ui,
        waveform: &[f32],
        progress: Option<f32>,
        active_color: Color32,
        idle_color: Color32,
        background_color: Color32,
        height: f32,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 14.0, background_color);

        if waveform.is_empty() {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 12.0, rect.center().y),
                    Pos2::new(rect.right() - 12.0, rect.center().y),
                ],
                Stroke::new(2.0, idle_color),
            );
            return;
        }

        let inner = rect.shrink2(vec2(10.0, 8.0));
        let wave_width = (inner.width() * 0.82).clamp(inner.width().min(56.0), inner.width());
        let wave_left = inner.center().x - wave_width * 0.5;
        let bar_width = wave_width / waveform.len() as f32;
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = Self::wave_strip_level(*level);
            let center_x = wave_left + (index as f32 + 0.5) * bar_width;
            let half = amplitude * inner.height() * 0.34;
            let wave_rect = Rect::from_min_max(
                Pos2::new(
                    center_x - (bar_width * 0.2).max(0.8),
                    inner.center().y - half,
                ),
                Pos2::new(
                    center_x + (bar_width * 0.2).max(0.8),
                    inner.center().y + half,
                ),
            );
            painter.rect_filled(wave_rect, 2.0, idle_color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(wave_left..=wave_left + wave_width, progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, inner.top()),
                    Pos2::new(play_x, inner.bottom()),
                ],
                Stroke::new(2.0, active_color),
            );
        }
    }

    pub(super) fn draw_full_width_wave_strip(
        ui: &mut Ui,
        waveform: &[f32],
        progress: Option<f32>,
        active_color: Color32,
        idle_color: Color32,
        background_color: Color32,
        height: f32,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 14.0, background_color);

        let inner = rect.shrink2(vec2(10.0, 7.0));
        painter.line_segment(
            [
                Pos2::new(inner.left(), inner.center().y),
                Pos2::new(inner.right(), inner.center().y),
            ],
            Stroke::new(1.0, idle_color.linear_multiply(0.2)),
        );

        if waveform.is_empty() {
            return;
        }

        let bar_width = inner.width() / waveform.len().max(1) as f32;
        let column_width = (bar_width * 0.44).clamp(1.5, 3.2);
        for (index, level) in waveform.iter().enumerate() {
            let amplitude = Self::wave_strip_level(*level).clamp(0.08, 1.0);
            let center_x = inner.left() + (index as f32 + 0.5) * bar_width;
            let half = amplitude * inner.height() * 0.42;
            let bar = Rect::from_min_max(
                Pos2::new(
                    center_x - column_width * 0.5,
                    inner.center().y - half,
                ),
                Pos2::new(
                    center_x + column_width * 0.5,
                    inner.center().y + half,
                ),
            );
            let color = if amplitude > 0.32 {
                active_color
            } else {
                idle_color
            };
            painter.rect_filled(bar, 2.0, color);
        }

        if let Some(progress) = progress {
            let play_x = egui::lerp(inner.left()..=inner.right(), progress.clamp(0.0, 1.0));
            painter.line_segment(
                [
                    Pos2::new(play_x, inner.top()),
                    Pos2::new(play_x, inner.bottom()),
                ],
                Stroke::new(2.0, active_color.linear_multiply(0.95)),
            );
        }
    }

    pub(super) fn boost_stream_meter_level(level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        (level * 6.0).sqrt().clamp(0.12, 1.0)
    }

    pub(super) fn wave_strip_level(level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        let shaped = (level * 1.28).powf(1.16).clamp(0.0, 1.0);
        if shaped < 0.05 { shaped * 0.55 } else { shaped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_amplitude_tracks_sound_volume() {
        assert_eq!(
            SoundFxApp::scale_waveform_for_volume(vec![0.2, 0.8], 0.5),
            vec![0.1, 0.4]
        );
        assert_eq!(
            SoundFxApp::scale_waveform_for_volume(vec![0.2, 0.8], 2.0),
            vec![0.4, 1.0]
        );
    }
}
