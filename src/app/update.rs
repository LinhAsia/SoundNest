use super::*;

impl eframe::App for SoundFxApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let _ = self;
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        Self::apply_theme(ctx, self.dark_theme);
        ctx.set_cursor_icon(egui::CursorIcon::Default);
        self.center_window_if_needed(ctx);
        self.intercept_close_request(ctx);
        self.poll_library_hydration_jobs(ctx);
        self.poll_transition_analysis_jobs(ctx);
        self.poll_myinstants_waveform_jobs();
        self.poll_processed_export_jobs(ctx);
        self.poll_trim_commit_jobs(ctx);
        self.poll_audio_preload_jobs(ctx);
        self.poll_recording_review_jobs(ctx);
        self.poll_library_import_jobs(ctx);
        self.poll_normalize_jobs(ctx);
        self.poll_stream_driver_result(ctx);
        self.poll_stream_input_router(ctx);
        self.poll_vocal_separation_jobs(ctx);
        self.poll_tts_jobs(ctx);
        self.prune_copy_feedback(ctx);
        if !ctx.input(|input| input.pointer.primary_down()) {
            let accepted_trim_drop = self.finalize_pending_trim_timeline_drop();
            if !accepted_trim_drop {
                self.pending_sound_drag = None;
            }
            self.pending_folder_drag = None;
        }

        self.play_startup_sound_if_needed(ctx);

        let transition = self.transition_progress(ctx);
        let download_snapshot = self.downloader.snapshot();
        let wants_shadow = false;
        if self.native_shadow_applied != wants_shadow {
            platform::set_native_window_shadow(frame, wants_shadow);
            self.native_shadow_applied = wants_shadow;
        }
        let wants_transition_topmost = self.is_transition_active() || self.overlay_only_mode;
        if self.transition_window_topmost_applied != wants_transition_topmost {
            platform::set_native_window_topmost(frame, wants_transition_topmost);
            self.transition_window_topmost_applied = wants_transition_topmost;
        }

        self.enforce_square_window_if_needed(ctx);
        self.preload_selected_sound_audio();
        self.handle_space_preview(ctx);
        self.handle_trim_start_preview(ctx);
        self.handle_record_hotkey(ctx);

        if let Some(audio) = self.audio.as_mut() {
            audio.tick();
            if self.myinstants_preview_audio_url.is_some() && !audio.has_active_playback() {
                self.myinstants_preview_audio_url = None;
            }
        }
        if !self.normalize_inflight.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if !self.trim_commit_inflight.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        if self.playback_needs_live_repaint() {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        if self.download_was_running
            && !download_snapshot.running
            && download_snapshot.last_file.is_some()
        {
            self.show_download_panel = true;
            self.stop_preview();
            self.clear_status();
        }
        self.download_was_running = download_snapshot.running;

        if download_snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        if self.tts_running || self.vocal_separation_running {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }

        let recorder_snapshot = self.recorder.snapshot();
        if recorder_snapshot.running {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
        if let Some(error) = recorder_snapshot.error.clone() {
            self.set_error_status(error);
        }
        if let Some(path) = self.recorder.take_completed_path() {
            self.open_recording_review(&path);
            if self.reveal_record_review_on_open {
                Self::reveal_window(ctx);
                self.reveal_record_review_on_open = false;
            }
        }
        if self.recording_review_pending_path.is_some() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        self.poll_record_video_export(ctx);
        if self.active_record_video_export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        if self.record_overlay_pending_visible {
            ctx.request_repaint_after(Duration::from_millis(50));
            if self.recorder.snapshot().running {
                self.record_overlay_pending_visible = false;
            }
        }

        let myinstants_snapshot = self.myinstants.snapshot();
        if myinstants_snapshot.searching || myinstants_snapshot.downloading {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
        if let Some(error) = myinstants_snapshot.error.clone() {
            self.set_error_status(error);
        }
        if let Some((result, path, add_to_library)) = self.myinstants.take_completed_download() {
            self.myinstants_cached_files
                .insert(result.audio_url.clone(), path.clone());
            if add_to_library {
                self.import_downloaded_sound(&path, true);
            }
        }

        self.flush_pending_save(ctx);

        if let Some((TransitionPhase::Intro, progress)) = transition {
            self.render_transition_layer(ctx, progress, TransitionPhase::Intro);
            return;
        }

        let live_ui_reveal = self.live_ui_reveal_progress(ctx);
        let live_ui_overlay_alpha = if self.dark_theme {
            1.0 - live_ui_reveal
        } else {
            0.0
        };
        if live_ui_overlay_alpha > 0.0 {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        self.render_root_view(ctx, live_ui_overlay_alpha);
        self.handle_external_file_hover(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.recorder.stop();
        self.pitch_monitor.stop();
        self.reset_library_tree_state();
        if self.pending_save {
            let _ = self.storage.save_library(&self.sounds);
            self.pending_save = false;
        }
    }
}
