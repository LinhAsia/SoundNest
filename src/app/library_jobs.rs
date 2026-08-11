use super::*;

impl SoundFxApp {
    pub(super) fn refresh_import_audio_entries(&mut self) {
        self.import_audio_entries.clear();
        if self.import_dir.as_os_str().is_empty() || !self.import_dir.exists() {
            return;
        }

        let Ok(entries) = fs::read_dir(&self.import_dir) else {
            return;
        };

        self.import_audio_entries = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| !Self::should_skip_import_path(path))
            .filter(|path| path.is_dir() || is_supported_audio(path))
            .collect();

        self.import_audio_entries.sort_by(|left, right| {
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| left.file_name().cmp(&right.file_name()))
        });
    }

    pub(super) fn should_skip_import_path(path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return true;
        };
        if name.starts_with('$') {
            return true;
        }
        if matches!(
            name,
            "System Volume Information" | "Recovery" | "Config.Msi" | "MSOCache"
        ) {
            return true;
        }
        #[cfg(windows)]
        if let Ok(metadata) = fs::metadata(path) {
            let attrs = metadata.file_attributes();
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
            if attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                return true;
            }
        }
        false
    }

    pub(super) fn poll_library_hydration_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.library_hydration_rx.try_recv() {
            match message {
                LibraryHydrationMessage::Ready(hydrated_sounds) => {
                    for hydrated in hydrated_sounds {
                        if let Some(existing) =
                            self.sounds.iter_mut().find(|sound| sound.id == hydrated.id)
                        {
                            if existing.waveform.is_empty() {
                                existing.waveform = hydrated.waveform.clone();
                            }
                            if existing.duration_secs <= 0.0 {
                                existing.duration_secs = hydrated.duration_secs;
                            }
                            if existing.trim_end_secs <= 0.0 {
                                existing.trim_end_secs = hydrated.trim_end_secs;
                            }
                            existing.clamp_trim();
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_transition_analysis_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.transition_analysis_rx.try_recv() {
            match message {
                TransitionAnalysisMessage::StartupReady {
                    waveform,
                    duration_sec,
                } => {
                    self.startup.sound_waveform = waveform;
                    self.startup.sound_duration_sec = duration_sec;
                    if self.startup.phase == TransitionPhase::Intro {
                        self.startup.duration_sec = DEFAULT_INTRO_DURATION_SEC;
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn import_downloaded_sound(&mut self, path: &Path, remove_source: bool) {
        self.library_current_folder = None;
        self.folder_import_select_mode = None;
        self.import_paths(vec![path.to_path_buf()]);
        if remove_source {
            let _ = fs::remove_file(path);
        }
    }

    pub(super) fn poll_myinstants_waveform_jobs(&mut self) {
        while let Ok(message) = self.myinstants_waveform_rx.try_recv() {
            match message {
                MyinstantsWaveformMessage::Ready {
                    audio_url,
                    path,
                    waveform,
                } => {
                    self.myinstants_waveform_jobs.remove(&audio_url);
                    self.myinstants_preview_files
                        .insert(audio_url.clone(), path);
                    self.myinstants_waveforms.insert(audio_url, waveform);
                }
                MyinstantsWaveformMessage::Failed { audio_url, error } => {
                    self.myinstants_waveform_jobs.remove(&audio_url);
                    if self.status.is_none() {
                        self.set_error_status(error);
                    }
                }
            }
        }
    }

    pub(super) fn poll_normalize_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        while let Ok(message) = self.normalize_rx.try_recv() {
            match message {
                NormalizeMessage::Finished { sound_id, result } => {
                    self.normalize_inflight.remove(&sound_id);
                    match result {
                        Ok(gain) => {
                            if let Some(index) =
                                self.sounds.iter().position(|sound| sound.id == sound_id)
                            {
                                self.sounds[index].volume = gain;
                                self.mark_dirty(ctx);
                                self.schedule_processed_export(sound_id);
                                if self
                                    .audio
                                    .as_ref()
                                    .is_some_and(|audio| audio.is_playing(sound_id))
                                {
                                    let cursor_secs =
                                        self.preview_cursor_secs_for(&self.sounds[index]);
                                    self.preview_sound_from_position(sound_id, Some(cursor_secs));
                                }
                                changed = true;
                            }
                        }
                        Err(error) => {
                            self.set_error_status(error);
                        }
                    }
                }
            }
        }

        if changed {
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_audio_preload_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        let mut pending_preview_to_play = None;
        while let Ok(message) = self.audio_preload_rx.try_recv() {
            match message {
                AudioPreloadMessage::Finished { asset_path, result } => {
                    let asset_path_for_match = asset_path.clone();
                    let pending_preview_match = self.pending_preview_after_preload.and_then(
                        |(pending_sound_id, start_position_secs)| {
                            self.sounds
                                .iter()
                                .find(|sound| sound.id == pending_sound_id)
                                .and_then(|sound| {
                                    (self.preview_asset_path_for_sound(sound)
                                        == asset_path_for_match)
                                        .then_some((pending_sound_id, start_position_secs))
                                })
                        },
                    );
                    self.audio_preload_queued.remove(&asset_path);
                    self.audio_preload_inflight.remove(&asset_path);
                    match result {
                        Ok((channels, sample_rate, samples)) => {
                            self.audio_preload_failures.remove(&asset_path);
                            if let Some(audio) = self.audio.as_mut() {
                                audio.insert_cached_audio(
                                    asset_path,
                                    channels,
                                    sample_rate,
                                    samples,
                                );
                                changed = true;
                            }
                        }
                        Err(error) => {
                            if let Some((pending_sound_id, start_position_secs)) =
                                pending_preview_match
                            {
                                match self.repair_sound_preview_asset(
                                    ctx,
                                    pending_sound_id,
                                    &asset_path_for_match,
                                ) {
                                    Ok(true) => {
                                        self.pending_preview_after_preload = None;
                                        pending_preview_to_play =
                                            Some((pending_sound_id, start_position_secs));
                                        continue;
                                    }
                                    Ok(false) => {}
                                    Err(repair_error) => {
                                        self.audio_preload_failures
                                            .insert(asset_path.clone(), repair_error.to_string());
                                        self.pending_preview_after_preload = None;
                                        self.set_error_status(format!(
                                            "Unable to load audio preview: {repair_error}"
                                        ));
                                        continue;
                                    }
                                }
                            }
                            self.audio_preload_failures
                                .insert(asset_path.clone(), error.clone());
                            if pending_preview_match.is_some() {
                                self.pending_preview_after_preload = None;
                                self.set_error_status(format!(
                                    "Unable to load audio preview: {error}"
                                ));
                            }
                        }
                    }
                    if let Some((pending_sound_id, start_position_secs)) = pending_preview_match {
                        self.pending_preview_after_preload = None;
                        pending_preview_to_play = Some((pending_sound_id, start_position_secs));
                    }
                }
            }
        }

        if let Some((sound_id, start_position_secs)) = pending_preview_to_play {
            self.preview_sound_from_position(sound_id, start_position_secs);
            return;
        }

        if changed {
            ctx.request_repaint();
        } else if !self.audio_preload_inflight.is_empty()
            || self.pending_preview_after_preload.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }
    }

    pub(super) fn poll_library_import_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        while let Ok(message) = self.library_import_rx.try_recv() {
            match message {
                LibraryImportMessage::Progress {
                    job_id,
                    completed,
                    total,
                    current_label,
                } => {
                    if let Some(job) = self.library_import_job.as_mut()
                        && job.job_id == job_id
                    {
                        job.completed = completed;
                        job.total = total;
                        job.current_label = current_label;
                        changed = true;
                    }
                }
                LibraryImportMessage::Finished { job_id, result } => {
                    let Some(job) = self.library_import_job.take() else {
                        continue;
                    };
                    if job.job_id != job_id {
                        self.library_import_job = Some(job);
                        continue;
                    }

                    match result {
                        Ok(mut imported) => {
                            let target_folder_id = imported.target_folder_id.filter(|folder_id| {
                                self.folders.iter().any(|folder| folder.id == *folder_id)
                            });
                            for folder in &mut imported.imported_folders {
                                if folder.parent_id == imported.target_folder_id {
                                    folder.parent_id = target_folder_id;
                                }
                            }
                            for sound in &mut imported.imported_sounds {
                                if sound.folder_id == imported.target_folder_id {
                                    sound.folder_id = target_folder_id;
                                }
                            }

                            if !imported.imported_folders.is_empty() {
                                self.folders.extend(imported.imported_folders);
                                let _ = self.storage.save_folders(&self.folders);
                            }
                            if imported.imported_sounds.is_empty() {
                                if imported.ignored_count > 0 {
                                    self.set_error_status("No supported audio files were found");
                                } else {
                                    self.clear_status();
                                }
                                continue;
                            }

                            imported.imported_sounds.reverse();
                            let imported_count = imported.imported_sounds.len();
                            for sound in imported.imported_sounds {
                                self.selected = Some(sound.id);
                                self.sounds.insert(0, sound);
                            }
                            self.library_audio_tag_filters.clear();
                            self.library_current_folder = target_folder_id;
                            self.save_now();
                            self.status = Some(format!("Imported {imported_count} sound(s)"));
                            changed = true;
                        }
                        Err(error) => self.set_error_status(error),
                    }
                }
            }
        }

        if changed {
            ctx.request_repaint();
        } else if self.library_import_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
        }
    }
}
