use super::*;

impl SoundFxApp {
    pub(crate) fn with_initial_selection(mut self) -> Self {
        self.selected = self.sounds.first().map(|sound| sound.id);
        self.preload_selected_sound_audio();
        let _ = self.record_hotkey_manager.set_hotkeys(&self.record_hotkeys);
        let _ = self
            .record_hotkey_manager
            .set_secondary_hotkeys(&self.pitch_hotkeys);
        self
    }

    pub(crate) fn begin_async_transition_analysis(&mut self, startup_sound_path: Option<PathBuf>) {
        let Some(path) = startup_sound_path else {
            return;
        };
        let tx = self.transition_analysis_tx.clone();
        thread::spawn(move || {
            let result = Storage::new()
                .and_then(|storage| storage.analyze_audio_preview(&path, TRANSITION_WAVE_BUCKETS))
                .map(
                    |(waveform, duration_sec)| TransitionAnalysisMessage::StartupReady {
                        waveform,
                        duration_sec,
                    },
                );
            if let Ok(message) = result {
                let _ = tx.send(message);
            }
        });
    }

    pub(crate) fn begin_async_library_hydration(&mut self) {
        if self
            .sounds
            .iter()
            .all(|sound| !sound.waveform.is_empty() && sound.duration_secs > 0.0)
        {
            return;
        }

        let tx = self.library_hydration_tx.clone();
        let mut sounds = self.sounds.clone();
        thread::spawn(move || {
            let result = Storage::new().and_then(|storage| {
                let mut changed = false;
                for sound in &mut sounds {
                    if storage.hydrate_sound(sound)? {
                        changed = true;
                    }
                }
                if changed {
                    let _ = storage.save_library(&sounds);
                }
                Ok::<_, anyhow::Error>(sounds)
            });
            if let Ok(sounds) = result {
                let _ = tx.send(LibraryHydrationMessage::Ready(sounds));
            }
        });
    }

    pub(crate) fn set_error_status(&mut self, error: impl ToString) {
        self.status = Some(error.to_string());
    }

    pub(crate) fn selected_sound_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.sounds.iter().position(|sound| sound.id == selected)
    }

    pub(crate) fn is_transition_active(&self) -> bool {
        self.startup.phase != TransitionPhase::Live
    }

    pub(crate) fn center_window_if_needed(&mut self, ctx: &Context) {
        if !self.center_window_next_frame {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Self::desired_window_size()));
        if let Some(center_cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(center_cmd);
        }
        self.center_window_next_frame = false;
    }

    pub(crate) fn desired_window_size() -> Vec2 {
        vec2(1500.0, 920.0)
    }

    pub(crate) fn popup_safe_rect(&self, ctx: &Context) -> Rect {
        if self.overlay_only_mode {
            ctx.screen_rect().shrink(4.0)
        } else {
            self.modal_safe_rect(ctx)
        }
    }

    pub(crate) fn centered_overlay_pos(&self, ctx: &Context, size: Vec2) -> Pos2 {
        let rect = self.popup_safe_rect(ctx);
        let center = rect.center();
        Pos2::new(
            (center.x - size.x * 0.5).round(),
            (center.y - size.y * 0.5).round(),
        )
    }

    pub(crate) fn clamp_overlay_pos(&self, ctx: &Context, size: Vec2, pos: Pos2) -> Pos2 {
        let rect = self.popup_safe_rect(ctx);
        let max_x = (rect.right() - size.x).max(rect.left());
        let max_y = (rect.bottom() - size.y).max(rect.top());
        Pos2::new(
            pos.x.round().clamp(rect.left(), max_x),
            pos.y.round().clamp(rect.top(), max_y),
        )
    }

    pub(crate) fn update_overlay_drag_position(
        ctx: &Context,
        rect: Rect,
        response: &egui::Response,
        size: Vec2,
        position: &mut Option<Pos2>,
    ) {
        let drag_origin_id = response.id.with("overlay-drag-origin");
        if response.drag_started() || response.clicked() {
            ctx.data_mut(|data| {
                data.insert_temp(drag_origin_id, position.unwrap_or_default());
            });
        }
        if (response.dragged()
            || (response.is_pointer_button_down_on() && response.drag_delta().length_sq() > 0.0))
            && let Some(origin) = ctx.data(|data| data.get_temp::<Pos2>(drag_origin_id))
        {
            let max_x = (rect.right() - size.x).max(rect.left());
            let max_y = (rect.bottom() - size.y).max(rect.top());
            let next = origin + response.drag_delta();
            *position = Some(Pos2::new(
                next.x.round().clamp(rect.left(), max_x),
                next.y.round().clamp(rect.top(), max_y),
            ));
        }
        if !ctx.input(|input| input.pointer.primary_down()) {
            ctx.data_mut(|data| {
                data.remove::<Pos2>(drag_origin_id);
            });
        }
    }

    pub(crate) fn enforce_square_window_if_needed(&mut self, _ctx: &Context) {}

    pub(crate) fn custom_transition_duration_secs(
        storage: &Storage,
        sound_path: &Path,
        fallback_secs: f32,
    ) -> f32 {
        if !sound_path.exists() {
            return fallback_secs;
        }

        storage
            .audio_duration_secs(sound_path)
            .unwrap_or(fallback_secs)
            .max(0.05)
    }

    pub(crate) fn custom_transition_duration_secs_opt(
        storage: &Storage,
        sound_path: Option<&Path>,
        fallback_secs: f32,
    ) -> f32 {
        match sound_path {
            Some(path) => Self::custom_transition_duration_secs(storage, path, fallback_secs),
            None => fallback_secs,
        }
    }

    pub(crate) fn load_transition_sound_visual(
        storage: &Storage,
        sound_path: Option<PathBuf>,
    ) -> (Vec<f32>, f32) {
        let Some(sound_path) = sound_path else {
            return (Vec::new(), 0.0);
        };

        storage
            .analyze_audio_preview(&sound_path, TRANSITION_WAVE_BUCKETS)
            .unwrap_or_else(|_| (Vec::new(), 0.0))
    }

    pub(crate) fn request_close(&mut self, ctx: &Context) {
        self.startup.close_sent = true;
        if self.recorder.snapshot().running {
            self.stop_recording_for_close(ctx);
        }
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.finalize_close_cleanup(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    pub(crate) fn stop_recording_for_close(&mut self, _ctx: &Context) {
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        if let Some(path) = self.recorder.take_completed_path() {
            let _ = fs::remove_file(path);
        }
    }

    pub(crate) fn finalize_close_cleanup(&mut self, _ctx: &Context) {
        self.show_download_panel = false;
        self.close_recording_review(true);
        self.video_viewer = None;
        self.pitch_monitor.stop();
        self.pitch_overlay_native_visuals_applied = false;
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
    }

    pub(crate) fn play_file_if_exists(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.play_file(path)?;
        }
        Ok(())
    }

    pub(crate) fn open_sound_from_library(&mut self, sound_id: Uuid) {
        self.selected = Some(sound_id);
        self.editing_from_folder = self.library_current_folder;
        self.app_view = AppView::Editor;
    }

    pub(crate) fn open_selected_sound_for_record_export(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            self.set_error_status("Pick a sound first");
            return;
        };
        let selected_sound = self.sounds[index].clone();
        let source_path = match self.storage.drag_sound_source_path(&selected_sound) {
            Ok(path) => path,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };
        if !source_path.exists() {
            self.set_error_status(format!("unable to open {}", source_path.display()));
            return;
        }
        let mut review_sound = match self
            .storage
            .analyze_sound_as_effect(&source_path, &selected_sound.name)
        {
            Ok(sound) => sound,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };
        review_sound.waveform = Self::center_waveform_visual(&review_sound.waveform);
        let preview_id = review_sound.id;
        let preview_start = review_sound.trim_start_secs;
        let preview_duration = review_sound.safe_duration();
        let source_is_temporary = source_path != selected_sound.asset_path(self.storage.root_dir());
        self.trim_timeline_zoom = 1.0;
        self.recording_draft = Some(RecordingDraft {
            sound: review_sound,
            source_path,
            source_is_temporary,
            keep_vocal: false,
            vocal_separated_path: None,
            keep_music: false,
            music_separated_path: None,
        });
        self.show_record_panel = false;
        self.show_record_review_panel = true;
        self.app_view = AppView::Editor;
        self.set_preview_cursor_secs(preview_id, preview_start, preview_duration);
        self.preview_recording_draft_from_position(Some(preview_start));
        self.clear_status();
    }

    pub(crate) fn truncate_middle_ascii(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(6) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(3) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push_str("...");
        compact.push_str("...");
        compact.push_str("...");
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    pub(crate) fn center_waveform_visual(waveform: &[f32]) -> Vec<f32> {
        if waveform.len() < 8 {
            return waveform.to_vec();
        }

        let peak = waveform.iter().copied().fold(0.0f32, f32::max);
        if peak <= f32::EPSILON {
            return waveform.to_vec();
        }

        let threshold = (peak * 0.035).clamp(0.006, 0.04);
        let Some(first) = waveform.iter().position(|level| *level >= threshold) else {
            return waveform.to_vec();
        };
        let Some(last) = waveform.iter().rposition(|level| *level >= threshold) else {
            return waveform.to_vec();
        };
        let active = &waveform[first..=last];
        if active.len() >= waveform.len() {
            return waveform.to_vec();
        }

        let mut centered = vec![0.0; waveform.len()];
        let offset = (waveform.len() - active.len()) / 2;
        centered[offset..offset + active.len()].copy_from_slice(active);
        centered
    }
}
