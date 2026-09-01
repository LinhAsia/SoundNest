use super::*;

impl SoundFxApp {
    pub(crate) fn recording_output_path(&self) -> PathBuf {
        let stamp = format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0)
        );
        let stem = self
            .record_name
            .trim()
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => '_',
            })
            .collect::<String>()
            .trim_matches('_')
            .to_owned();
        let file_name = if stem.is_empty() {
            format!("recording-{stamp}.wav")
        } else {
            format!("{stem}-{stamp}.wav")
        };
        self.storage.root_dir().join("recordings").join(file_name)
    }

    pub(crate) fn normalize_record_export_video_fps(fps: u32) -> u32 {
        match fps {
            record_video::LOW_VIDEO_FPS => record_video::LOW_VIDEO_FPS,
            record_video::HIGH_VIDEO_FPS => record_video::HIGH_VIDEO_FPS,
            _ => record_video::STANDARD_VIDEO_FPS,
        }
    }

    pub(crate) fn stop_recording(&mut self, ctx: Option<&Context>) {
        let was_overlay_only = self.overlay_only_mode;
        self.recorder.stop();
        self.record_overlay_open = false;
        self.record_overlay_native_visuals_applied = false;
        self.overlay_only_mode = false;
        if was_overlay_only {
            self.center_window_next_frame = true;
            if let Some(ctx) = ctx {
                Self::restore_main_viewport(ctx);
            } else {
                platform::hide_native_window_by_title("Sound FX");
            }
        }
        if let Some(path) = self.recorder.take_completed_path() {
            self.open_recording_review(&path);
        }
    }

    pub(crate) fn start_recording(&mut self, ctx: &Context, reveal_main_window: bool) {
        if self.recording_review_pending_path.is_some() {
            self.status = Some("Preparing recorded audio...".to_owned());
            return;
        }
        if self.record_input_source == PitchInputSource::Microphone
            && self.selected_record_input_device.is_none()
        {
            if self.record_capture_devices.is_empty() {
                self.refresh_record_capture_devices();
            }
        }

        self.close_recording_review(true);
        self.stop_preview();
        match self.recorder.start(RecorderConfig {
            source: self.record_input_source,
            input_device_name: if self.record_input_source == PitchInputSource::Microphone {
                self.selected_record_input_device.clone()
            } else {
                None
            },
            output_path: self.recording_output_path(),
        }) {
            Ok(()) => {
                self.center_record_overlay_next_frame = true;
                self.record_overlay_pos = None;
                self.record_overlay_native_visuals_applied = false;
                self.show_record_panel = false;
                if reveal_main_window {
                    Self::reveal_window(ctx);
                } else {
                    Self::apply_overlay_only_viewport(ctx, vec2(430.0, 118.0));
                    self.overlay_only_mode = true;
                    self.record_overlay_pending_visible = true;
                    ctx.request_repaint();
                }
                self.clear_status();
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(crate) fn toggle_recording(&mut self, ctx: &Context) {
        if self.recorder.snapshot().running {
            self.stop_recording(Some(ctx));
        } else {
            self.start_recording(ctx, false);
        }
    }

    pub(crate) fn preview_recording_draft_from_position(
        &mut self,
        start_position_secs: Option<f32>,
    ) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };

        self.myinstants_preview_audio_url = None;

        let (keep_vocal, keep_music, source_path, vocal_separated_path, music_separated_path) = {
            (
                draft.keep_vocal,
                draft.keep_music,
                draft.source_path.clone(),
                draft.vocal_separated_path.clone(),
                draft.music_separated_path.clone(),
            )
        };

        let source_path = if keep_music {
            if let Some(existing) = music_separated_path {
                existing
            } else {
                self.start_music_separation_if_needed();
                self.status = Some(self.t("editor.music_preview_original"));
                source_path
            }
        } else if keep_vocal {
            if let Some(existing) = vocal_separated_path {
                existing
            } else {
                self.start_vocal_separation_if_needed();
                self.status = Some(self.t("editor.vocal_preview_original"));
                source_path
            }
        } else {
            source_path
        };

        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };

        let draft = self.recording_draft.as_ref().unwrap();
        let playback = match start_position_secs {
            Some(start_position_secs) => {
                audio.play_from(&draft.sound, &source_path, start_position_secs)
            }
            None => audio.play(&draft.sound, &source_path),
        };

        match playback {
            Ok(()) => {
                if !((keep_vocal
                    && self.recording_draft.as_ref().is_some_and(|draft| {
                        draft.keep_vocal && draft.vocal_separated_path.is_none()
                    }))
                    || (keep_music
                        && self.recording_draft.as_ref().is_some_and(|draft| {
                            draft.keep_music && draft.music_separated_path.is_none()
                        })))
                {
                    self.clear_status();
                }
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(crate) fn close_recording_review(&mut self, discard_audio: bool) {
        if let Some(path) = self.recording_review_pending_path.take()
            && discard_audio
        {
            let _ = fs::remove_file(path);
        }
        if let Some(draft) = self.recording_draft.take() {
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(draft.sound.id))
            {
                self.stop_preview();
            }
            if discard_audio && draft.source_is_temporary {
                let _ = fs::remove_file(draft.source_path);
            }
        }
        self.show_record_review_panel = false;
    }

    pub(crate) fn hide_recording_review(&mut self) {
        if let Some(draft) = self.recording_draft.as_ref()
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.is_playing(draft.sound.id))
        {
            self.stop_preview();
        }
        self.show_record_review_panel = false;
    }

    pub(crate) fn save_recording_review_to_library(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        let keep_review_open = self.active_record_video_export.is_some();
        let keep_vocal = draft.keep_vocal;
        let keep_music = draft.keep_music;
        let source_path = draft.source_path.clone();
        let sound = draft.sound.clone();
        let vocal_separated_path = draft.vocal_separated_path.clone();
        let music_separated_path = draft.music_separated_path.clone();

        let export_path = if keep_music {
            let music_path = if let Some(existing) = music_separated_path {
                existing
            } else {
                self.start_music_separation_if_needed();
                self.set_error_status(self.t("editor.music_processing"));
                return;
            };
            match self
                .storage
                .export_processed_sound_from_path(&music_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        } else if keep_vocal {
            let vocal_path = if let Some(existing) = vocal_separated_path {
                existing
            } else {
                self.start_vocal_separation_if_needed();
                self.set_error_status(self.t("editor.vocal_processing"));
                return;
            };
            match self
                .storage
                .export_processed_sound_from_path(&vocal_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        } else {
            match self
                .storage
                .export_processed_sound_from_path(&source_path, &sound)
            {
                Ok(path) => path,
                Err(error) => {
                    self.set_error_status(error);
                    return;
                }
            }
        };

        match self.storage.import_sound(&export_path) {
            Ok(mut imported) => {
                imported.name = sound.name.clone();
                imported.folder_id = None;
                self.app_view = AppView::Library;
                self.library_tab = LibraryTab::Sounds;
                self.library_current_folder = None;
                self.folder_import_select_mode = None;
                self.selected = Some(imported.id);
                self.sounds.insert(0, imported);
                self.save_now();
                let _ = fs::remove_file(&export_path);
                if !keep_review_open {
                    self.close_recording_review(true);
                }
                self.clear_status();
            }
            Err(error) => {
                let _ = fs::remove_file(&export_path);
                self.set_error_status(error);
            }
        }
    }

    pub(crate) fn export_recording_review_video(&mut self) {
        if self.active_record_video_export.is_some() {
            return;
        }

        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        let keep_vocal = draft.keep_vocal;
        let keep_music = draft.keep_music;
        let source_path = draft.source_path.clone();
        let vocal_separated_path = draft.vocal_separated_path.clone();
        let music_separated_path = draft.music_separated_path.clone();
        let sound = draft.sound.clone();
        let video_name = format!("{} SPN", draft.sound.name);

        self.stop_preview();

        let root_dir = self.storage.root_dir().to_path_buf();
        let show_sharps = self.record_export_video_sharps;
        let animated = self.record_export_video_animation;
        let export_fps = Self::normalize_record_export_video_fps(self.record_export_video_fps);
        let (tx, rx) = mpsc::channel();
        self.active_record_video_export = Some(RecordVideoExportState {
            progress: 0.04,
            stage: self.t("record.preparing"),
            receiver: rx,
        });
        self.clear_status();

        let record_preparing_audio = self.t("record.preparing_audio");
        let record_checking_ffmpeg = self.t("record.checking_ffmpeg");
        let record_analyzing_pitch = self.t("record.analyzing_pitch");
        let vocal_processing_error = self.t("editor.vocal_processing");
        let music_processing_error = self.t("editor.music_processing");

        thread::spawn(move || {
            let send_progress = |progress: f32, stage: &str| {
                let _ = tx.send(RecordVideoExportMessage::Progress {
                    progress,
                    stage: stage.to_owned(),
                });
            };

            let mut processed_audio_to_clean: Option<PathBuf> = None;
            let mut video_to_clean: Option<PathBuf> = None;
            let result = (|| -> Result<RecordVideoExportResult> {
                send_progress(0.08, record_preparing_audio.as_str());
                let storage = Storage::new()?;

                let audio_source = if keep_music {
                    if let Some(existing) = music_separated_path {
                        existing
                    } else {
                        anyhow::bail!(music_processing_error.clone())
                    }
                } else if keep_vocal {
                    if let Some(existing) = vocal_separated_path {
                        existing
                    } else {
                        anyhow::bail!(vocal_processing_error.clone())
                    }
                } else {
                    source_path.clone()
                };

                let processed_audio =
                    storage.export_processed_sound_from_path(&audio_source, &sound)?;
                processed_audio_to_clean = Some(processed_audio.clone());

                send_progress(0.18, record_checking_ffmpeg.as_str());
                let downloader = YoutubeAudioDownloader::new(&root_dir)?;
                let ffmpeg_path = downloader.ensure_ffmpeg_available()?;

                send_progress(0.28, record_analyzing_pitch.as_str());
                let (duration_secs, frames) =
                    analyze_pitch_file(&processed_audio, export_fps, show_sharps)?;

                let video_path = record_video::export_record_pitch_video(
                    &root_dir,
                    &ffmpeg_path,
                    &processed_audio,
                    &frames,
                    duration_secs,
                    export_fps,
                    animated,
                    |progress, stage| send_progress(progress, stage),
                )?;
                video_to_clean = Some(video_path.clone());

                Ok(RecordVideoExportResult {
                    processed_audio_path: processed_audio,
                    video_path,
                    duration_secs,
                    video_fps: export_fps,
                    video_name,
                })
            })();

            if result.is_err() {
                if let Some(path) = processed_audio_to_clean {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = video_to_clean {
                    let _ = fs::remove_file(path);
                }
            }

            let _ = tx.send(RecordVideoExportMessage::Finished(
                result.map_err(|error| error.to_string()),
            ));
        });
    }
}
