use super::*;

impl SoundFxApp {
    pub(crate) fn repair_sound_preview_asset(
        &mut self,
        ctx: &Context,
        sound_id: Uuid,
        failing_path: &Path,
    ) -> Result<bool> {
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return Ok(false);
        };
        let ffmpeg_path = self.downloader.ensure_ffmpeg_available()?;
        let updated = Storage::repair_sound_preview_asset_with_ffmpeg_at(
            self.storage.root_dir(),
            &self.sounds[index],
            failing_path,
            &ffmpeg_path,
        )?;
        self.sounds[index] = updated;
        self.audio_preload_failures.remove(failing_path);
        self.mark_dirty(ctx);
        if !self.save_now() {
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn schedule_processed_export(&mut self, sound_id: Uuid) {
        self.pending_processed_export_sound = Some(sound_id);
    }

    pub(crate) fn spawn_processed_export_job(&mut self, sound_id: Uuid) {
        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };
        if !sound.needs_processed_export() {
            return;
        }

        let root_dir = self.storage.root_dir().to_path_buf();
        let export_path = Storage::processed_export_path(&root_dir, &sound);
        if self.processed_export_inflight.contains(&export_path) || export_path.exists() {
            return;
        }
        self.processed_export_inflight.insert(export_path.clone());
        let tx = self.processed_export_tx.clone();

        thread::spawn(move || {
            let result = Storage::export_processed_sound_at(&root_dir, &sound)
                .map_err(|error| error.to_string());
            let _ = tx.send(ProcessedExportMessage::Finished {
                export_path,
                result,
            });
        });
    }

    pub(crate) fn start_normalize_job(&mut self, sound_id: Uuid) {
        if self.normalize_inflight.contains(&sound_id) {
            return;
        }

        let Some(sound) = self
            .sounds
            .iter()
            .find(|sound| sound.id == sound_id)
            .cloned()
        else {
            return;
        };

        let asset_path = sound.asset_path(self.storage.root_dir());
        self.normalize_inflight.insert(sound_id);
        let tx = self.normalize_tx.clone();

        thread::spawn(move || {
            let result =
                calculate_normalization_gain(&asset_path).map_err(|error| error.to_string());
            let _ = tx.send(NormalizeMessage::Finished { sound_id, result });
        });
    }

    pub(crate) fn schedule_audio_preload(&mut self, asset_path: PathBuf) {
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.has_cached_audio(&asset_path))
            || self.audio_preload_inflight.contains(&asset_path)
            || self.audio_preload_failures.contains_key(&asset_path)
        {
            return;
        }

        self.audio_preload_inflight.insert(asset_path.clone());
        let tx = self.audio_preload_tx.clone();
        thread::spawn(move || {
            let result = crate::audio::AudioEngine::decode_audio_for_cache(&asset_path)
                .map_err(|error| error.to_string());
            let _ = tx.send(AudioPreloadMessage::Finished { asset_path, result });
        });
    }
}
