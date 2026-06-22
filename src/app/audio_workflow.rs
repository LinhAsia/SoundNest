use super::*;

impl SoundFxApp {
    #[allow(dead_code)]
    pub(crate) fn truncate_middle(text: &str, max_chars: usize) -> String {
        let chars = text.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars.max(3) {
            return text.to_owned();
        }

        let edge = max_chars.saturating_sub(1) / 2;
        let mut compact = chars[..edge].iter().collect::<String>();
        compact.push_str("...");
        compact.push_str(
            &chars[chars.len().saturating_sub(edge)..]
                .iter()
                .collect::<String>(),
        );
        compact
    }

    pub(crate) fn preview_sound(&mut self, sound_id: Uuid) {
        self.preview_sound_from_position(sound_id, None);
    }

    pub(crate) fn preview_asset_path_for_sound(&self, sound: &SoundEffect) -> PathBuf {
        if sound.needs_processed_export()
            && Storage::processed_export_exists(self.storage.root_dir(), sound)
        {
            Storage::processed_export_path(self.storage.root_dir(), sound)
        } else {
            sound.playback_asset_path(self.storage.root_dir())
        }
    }

    pub(crate) fn preview_sound_from_position(
        &mut self,
        sound_id: Uuid,
        start_position_secs: Option<f32>,
    ) {
        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };

        let asset_path = self.preview_asset_path_for_sound(&sound);
        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };
        self.myinstants_preview_audio_url = None;

        if !audio.has_cached_audio(&asset_path) {
            if let Some(error) = self.audio_preload_failures.get(&asset_path) {
                self.pending_preview_after_preload = None;
                self.set_error_status(format!("Unable to load audio preview: {error}"));
                return;
            }
            self.schedule_audio_preload(asset_path.clone());
            self.pending_preview_after_preload = Some((sound.id, start_position_secs));
            self.status = Some(format!("Loading preview for {}...", sound.name));
            return;
        }

        self.pending_preview_after_preload = None;
        let playback = if sound.needs_processed_export()
            && Storage::processed_export_exists(self.storage.root_dir(), &sound)
        {
            let start_position_secs = start_position_secs.unwrap_or(sound.trim_start_secs);
            audio.play_processed_file(&sound, &asset_path, start_position_secs)
        } else {
            match start_position_secs {
                Some(start_position_secs) => {
                    audio.play_from(&sound, &asset_path, start_position_secs)
                }
                None => audio.play(&sound, &asset_path),
            }
        };

        match playback {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    pub(crate) fn preview_cursor_secs_for(&self, sound: &SoundEffect) -> f32 {
        self.preview_cursor
            .and_then(|(sound_id, secs)| (sound_id == sound.id).then_some(secs))
            .unwrap_or(sound.trim_start_secs)
            .clamp(sound.trim_start_secs, sound.trim_end_secs)
    }

    pub(crate) fn set_preview_cursor_secs(
        &mut self,
        sound_id: Uuid,
        secs: f32,
        duration_secs: f32,
    ) {
        self.preview_cursor = Some((sound_id, secs.clamp(0.0, duration_secs)));
    }

    pub(crate) fn start_vocal_separation_job(
        &mut self,
        kind: SeparationStemKind,
        target: VocalSeparationTarget,
        source_path: PathBuf,
        output_dir: PathBuf,
    ) {
        if self.vocal_separation_running {
            return;
        }

        let tx = self.vocal_separation_tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.vocal_separation_running = true;
        self.vocal_separation_cancel = Some(Arc::clone(&cancel));
        self.vocal_separation_target = Some(target.clone());
        self.vocal_separation_kind = Some(kind);
        match &target {
            VocalSeparationTarget::RecordingReview { source_path } => {
                if let Some(draft) = self.recording_draft.as_ref()
                    && draft.source_path == *source_path
                {
                    self.vocal_waveform_cache
                        .borrow_mut()
                        .remove(&draft.sound.id);
                    self.music_waveform_cache
                        .borrow_mut()
                        .remove(&draft.sound.id);
                }
            }
            VocalSeparationTarget::LibrarySound { sound_id, .. } => {
                self.vocal_waveform_cache.borrow_mut().remove(sound_id);
                self.music_waveform_cache.borrow_mut().remove(sound_id);
            }
        }
        self.vocal_separation_started_at = Some(Instant::now());
        self.vocal_separation_last_result = None;
        self.clear_status();
        thread::spawn(move || {
            let result = match kind {
                SeparationStemKind::Vocal => crate::vocal_separation::extract_vocals_cancellable(
                    &source_path,
                    &output_dir,
                    Arc::clone(&cancel),
                ),
                SeparationStemKind::Music => {
                    crate::vocal_separation::extract_instrumental_cancellable(
                        &source_path,
                        &output_dir,
                        Arc::clone(&cancel),
                    )
                }
            };
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(VocalSeparationMessage::Cancelled);
                return;
            }
            let _ = tx.send(VocalSeparationMessage::Finished {
                target,
                kind,
                result,
            });
        });
    }

    pub(crate) fn stop_vocal_separation(&mut self) {
        if let Some(cancel) = self.vocal_separation_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
            let status_key = match self.vocal_separation_kind {
                Some(SeparationStemKind::Music) => "editor.music_stopping",
                _ => "editor.vocal_stopping",
            };
            self.status = Some(self.t(status_key));
        }
        self.vocal_separation_running = false;
        self.vocal_separation_cancel = None;
        self.vocal_separation_target = None;
        self.vocal_separation_kind = None;
        self.vocal_separation_started_at = None;
        self.vocal_separation_last_result = None;
    }

    pub(crate) fn start_vocal_separation_if_needed(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        if !draft.keep_vocal
            || draft.vocal_separated_path.is_some()
            || self.vocal_separation_running
        {
            return;
        }

        let source_path = draft.source_path.clone();
        let output_dir = self
            .storage
            .root_dir()
            .join("temp_vocals")
            .join(draft.sound.id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Vocal,
            VocalSeparationTarget::RecordingReview {
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    pub(crate) fn start_library_vocal_separation(&mut self, sound_id: Uuid) {
        if self.vocal_separation_running {
            return;
        }

        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };

        let source_path = self.sounds[index].asset_path(self.storage.root_dir());
        if !source_path.exists() {
            self.set_error_status(self.t("editor.vocal_source_missing"));
            return;
        }

        let output_dir = self
            .storage
            .root_dir()
            .join("temp_vocals")
            .join(sound_id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Vocal,
            VocalSeparationTarget::LibrarySound {
                sound_id,
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    pub(crate) fn start_music_separation_if_needed(&mut self) {
        let Some(draft) = self.recording_draft.as_ref() else {
            return;
        };
        if !draft.keep_music
            || draft.music_separated_path.is_some()
            || self.vocal_separation_running
        {
            return;
        }

        let source_path = draft.source_path.clone();
        let output_dir = self
            .storage
            .root_dir()
            .join("temp_music")
            .join(draft.sound.id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Music,
            VocalSeparationTarget::RecordingReview {
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }

    pub(crate) fn start_library_music_separation(&mut self, sound_id: Uuid) {
        if self.vocal_separation_running {
            return;
        }

        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };

        let source_path = self.sounds[index].asset_path(self.storage.root_dir());
        if !source_path.exists() {
            self.set_error_status(self.t("editor.vocal_source_missing"));
            return;
        }

        let output_dir = self
            .storage
            .root_dir()
            .join("temp_music")
            .join(sound_id.to_string());
        self.start_vocal_separation_job(
            SeparationStemKind::Music,
            VocalSeparationTarget::LibrarySound {
                sound_id,
                source_path: source_path.clone(),
            },
            source_path,
            output_dir,
        );
    }
}
