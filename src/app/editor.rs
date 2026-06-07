use super::*;
use crate::audio::{AudioEngine, calculate_normalization_gain};
use crate::downloader::{YoutubeAudioDownloader, YoutubeSearchResult};
use crate::gemini_tts;
use crate::hotkey::{GlobalHotkeyManager, Hotkey};
use crate::localization::Localization;
use crate::myinstants::{MyinstantsClient, MyinstantsResult};
use crate::pitch::{
    PitchInputSource, PitchMonitor, PitchMonitorConfig, PitchSnapshot, analyze_pitch_file,
    list_capture_devices,
};
use crate::platform;
use crate::record_video;
use crate::recorder::{Recorder, RecorderConfig};
use crate::storage::{GeminiTtsPromptPreset, SoundEffect, Storage, VideoAsset, format_time};
use crate::stream_input::{StreamInputConfig, StreamInputRouter};
use anyhow::{Context as _, Result};
#[cfg(windows)]
use clipboard_win::{Clipboard, Setter, formats::FileList};
use eframe::egui::{
    self, Align, Align2, Button, CentralPanel, Checkbox, Color32, ComboBox, Context, CornerRadius,
    DragValue, FontFamily, FontId, Frame, Margin, Pos2, ProgressBar, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextEdit, TextureHandle, Ui, Vec2, ViewportCommand, vec2,
};
use eframe::epaint::Shadow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

impl SoundFxApp {
    pub(super) fn sync_editor_tags_input(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            self.editor_tags_input.clear();
            self.editor_tags_input_sound_id = None;
            return;
        };
        let sound = &self.sounds[index];
        if self.editor_tags_input_sound_id != Some(sound.id) {
            self.editor_tags_input = Self::join_tags(&sound.tags);
            self.editor_tags_input_sound_id = Some(sound.id);
        }
    }

    pub(super) fn delete_selected(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound = self.sounds.remove(index);
        if let Some(audio) = self.audio.as_mut() {
            if audio.is_playing(sound.id) {
                audio.stop();
            }
        }

        if let Err(error) = self.storage.remove_sound(&sound) {
            self.set_error_status(error);
            return;
        }

        self.selected = self
            .sounds
            .get(index)
            .or_else(|| self.sounds.get(index.saturating_sub(1)))
            .map(|next| next.id);
        self.save_now();
    }

    pub(super) fn copy_selected_processed_sound(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        match self.copy_sound_file_to_clipboard(&self.sounds[index]) {
            Ok(()) => self.clear_status(),
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn commit_selected_trimmed_sound(&mut self, ctx: &Context) {
        self.start_trim_commit_job(ctx, false);
    }

    pub(super) fn duplicate_selected_trimmed_sound(&mut self, ctx: &Context) {
        self.start_trim_commit_job(ctx, true);
    }

    pub(super) fn start_trim_commit_job(&mut self, ctx: &Context, keep_old: bool) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };

        let sound_id = self.sounds[index].id;
        if self.trim_commit_inflight.contains(&sound_id) {
            return;
        }
        if let Some(audio) = self.audio.as_mut()
            && audio.is_playing(sound_id)
        {
            audio.stop();
        }

        let sound = self.sounds[index].clone();
        let root_dir = self.storage.root_dir().to_path_buf();
        let tx = self.trim_commit_tx.clone();
        self.trim_commit_inflight.insert(sound_id);
        thread::spawn(move || {
            let result = if keep_old {
                Storage::duplicate_trimmed_sound_at(&root_dir, &sound)
            } else {
                Storage::replace_sound_with_processed_at(&root_dir, &sound)
            }
            .map_err(|error| error.to_string());
            let _ = tx.send(TrimCommitMessage::Finished {
                sound_id,
                keep_old,
                result,
            });
        });
        ctx.request_repaint();
    }

    pub(super) fn copy_sound_file_to_clipboard(&self, sound: &SoundEffect) -> Result<()> {
        let export_path = self.storage.export_processed_sound(sound)?;
        self.copy_file_path_to_clipboard(&export_path)
    }

    pub(super) fn drag_sound_file_out(&mut self, ctx: &Context, sound: &SoundEffect) -> Result<()> {
        let drag_path = self.storage.drag_sound_source_path(sound)?;
        self.ignored_drop_path =
            Some(fs::canonicalize(&drag_path).unwrap_or_else(|_| drag_path.clone()));
        self.pending_sound_drag = None;
        let drag_waveform = self.sound_waveform_samples(sound);
        let drag_ghost = platform::DragGhostSpec {
            waveform: Self::trimmed_waveform_preview_from_samples(sound, &drag_waveform),
            dark_theme: self.dark_theme,
        };
        let result = platform::drag_file_out(&drag_path, Some(&drag_ghost));
        ctx.request_repaint();
        result
    }

    pub(super) fn copy_video_file_to_clipboard(&self, video: &VideoAsset) -> Result<()> {
        let video_path = video.asset_path(self.storage.root_dir());
        self.copy_file_path_to_clipboard(&video_path)
    }

    pub(super) fn play_video_viewer_from_current_playhead(&mut self) -> Result<()> {
        let Some((audio_path, progress, duration_secs)) =
            self.video_viewer.as_ref().map(|viewer| {
                (
                    viewer.audio_path.clone(),
                    viewer.progress.clamp(0.0, 1.0),
                    viewer.video.duration_secs.max(0.05),
                )
            })
        else {
            return Ok(());
        };

        let start_progress = if progress >= 0.995 { 0.0 } else { progress };
        let start_secs = start_progress * duration_secs;
        let Some(audio) = self.audio.as_mut() else {
            return Err(anyhow::anyhow!("Audio unavailable"));
        };

        audio.play_file_from(&audio_path, start_secs)?;
        if let Some(viewer) = self.video_viewer.as_mut() {
            viewer.progress = start_progress;
        }
        Ok(())
    }

    pub(super) fn toggle_video_viewer_playback(&mut self) {
        let Some((audio_path, stored_progress)) = self
            .video_viewer
            .as_ref()
            .map(|viewer| (viewer.audio_path.clone(), viewer.progress))
        else {
            return;
        };

        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing_file(&audio_path));
        if is_playing {
            if let Some(progress) = self
                .audio
                .as_ref()
                .and_then(|audio| audio.playback_progress_for_file(&audio_path))
                .or(Some(stored_progress))
                && let Some(viewer) = self.video_viewer.as_mut()
            {
                viewer.progress = progress.clamp(0.0, 1.0);
            }
            self.stop_preview();
            return;
        }

        if let Err(error) = self.play_video_viewer_from_current_playhead() {
            self.set_error_status(error);
        } else {
            self.clear_status();
        }
    }

    pub(super) fn copy_file_path_to_clipboard(&self, file_path: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            let _clipboard =
                Clipboard::new_attempts(10).context("unable to open system clipboard")?;
            let paths = [file_path.display().to_string()];
            FileList
                .write_clipboard(&paths)
                .context("unable to place file on clipboard")?;
            return Ok(());
        }

        #[cfg(not(windows))]
        {
            let _ = file_path;
            anyhow::bail!("Clipboard file copy is only available on Windows");
        }
    }

    pub(super) fn prepare_video_viewer(&mut self, ctx: &Context, video: &VideoAsset) -> Result<()> {
        let ffmpeg_path = self.downloader.ensure_ffmpeg_available()?;
        let source_path = video.asset_path(self.storage.root_dir());
        let preview_root = self
            .storage
            .root_dir()
            .join("video-preview")
            .join(video.id.to_string());
        let frames_dir = preview_root.join("frames");
        let audio_path = preview_root.join("audio.wav");
        let ready_marker = preview_root.join("ready.txt");

        if !ready_marker.exists() {
            if preview_root.exists() {
                let _ = fs::remove_dir_all(&preview_root);
            }
            fs::create_dir_all(&frames_dir)
                .with_context(|| format!("unable to create {}", frames_dir.display()))?;

            let frame_pattern = frames_dir.join("frame_%05d.ppm");
            Self::run_ffmpeg_command(
                &ffmpeg_path,
                [
                    "-y",
                    "-i",
                    &source_path.to_string_lossy(),
                    "-vf",
                    &format!("fps={}", video.normalized_fps()),
                    "-pix_fmt",
                    "rgb24",
                    &frame_pattern.to_string_lossy(),
                ],
            )?;
            Self::run_ffmpeg_command(
                &ffmpeg_path,
                [
                    "-y",
                    "-i",
                    &source_path.to_string_lossy(),
                    "-vn",
                    "-acodec",
                    "pcm_s16le",
                    &audio_path.to_string_lossy(),
                ],
            )?;
            fs::write(&ready_marker, b"ok")
                .with_context(|| format!("unable to write {}", ready_marker.display()))?;
        }

        let mut frame_paths = fs::read_dir(&frames_dir)
            .with_context(|| format!("unable to read {}", frames_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("ppm"))
            .collect::<Vec<_>>();
        frame_paths.sort();
        if frame_paths.is_empty() {
            anyhow::bail!("video preview is empty");
        }

        self.stop_preview();
        self.video_viewer = Some(VideoViewerState {
            video: video.clone(),
            frame_paths,
            audio_path,
            progress: 0.0,
            current_frame: None,
        });
        self.load_video_frame_texture(ctx, 0)?;
        ctx.request_repaint();
        self.clear_status();
        Ok(())
    }

    pub(super) fn run_ffmpeg_command<I, S>(ffmpeg_path: &Path, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cmd = Command::new(ffmpeg_path);
        for arg in args {
            cmd.arg(arg.as_ref());
        }
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);
        let output = cmd
            .output()
            .with_context(|| format!("failed to launch {}", ffmpeg_path.display()))?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    pub(super) fn load_video_frame_texture(
        &mut self,
        ctx: &Context,
        frame_index: usize,
    ) -> Result<()> {
        let Some(viewer) = self.video_viewer.as_mut() else {
            return Ok(());
        };
        if viewer
            .current_frame
            .as_ref()
            .is_some_and(|(current, _, _)| *current == frame_index)
        {
            return Ok(());
        }

        let path = viewer
            .frame_paths
            .get(frame_index)
            .context("video frame not found")?;
        let (image, size) = load_ppm_color_image(path)?;
        let texture = ctx.load_texture(
            format!("video-frame-{}-{frame_index}", viewer.video.id),
            image,
            egui::TextureOptions::LINEAR,
        );
        viewer.current_frame = Some((frame_index, texture, size));
        Ok(())
    }

    pub(super) fn preview_file_path(&mut self, path: &Path) -> Result<()> {
        if let Some(audio) = self.audio.as_mut() {
            audio.play_file(path)?;
        }
        Ok(())
    }

    pub(super) fn vocal_separation_elapsed_secs(&self) -> Option<f32> {
        self.vocal_separation_started_at
            .map(|started_at| started_at.elapsed().as_secs_f32())
    }

    pub(super) fn vocal_separation_last_elapsed_for_sound(
        &self,
        sound_id: Uuid,
        kind: SeparationStemKind,
    ) -> Option<f32> {
        self.vocal_separation_last_result.as_ref().and_then(
            |(target, result_kind, elapsed_secs)| match target {
                VocalSeparationTarget::LibrarySound {
                    sound_id: target_sound_id,
                    ..
                } if *target_sound_id == sound_id && *result_kind == kind => Some(*elapsed_secs),
                _ => None,
            },
        )
    }

    pub(super) fn vocal_separation_last_elapsed_for_recording(
        &self,
        source_path: &Path,
        kind: SeparationStemKind,
    ) -> Option<f32> {
        self.vocal_separation_last_result.as_ref().and_then(
            |(target, result_kind, elapsed_secs)| match target {
                VocalSeparationTarget::RecordingReview {
                    source_path: target_source_path,
                } if target_source_path == source_path && *result_kind == kind => {
                    Some(*elapsed_secs)
                }
                _ => None,
            },
        )
    }

    pub(super) fn clear_status(&mut self) {
        self.status = None;
    }

    pub(super) fn external_drop_pointer_pos(&self, ctx: &Context) -> Option<Pos2> {
        ctx.input(|input| input.pointer.hover_pos().or(input.pointer.latest_pos()))
            .or_else(|| {
                #[cfg(windows)]
                {
                    let scale = ctx.pixels_per_point().max(1.0);
                    platform::cursor_window_position("Sound FX")
                        .map(|pos| Pos2::new(pos.x / scale, pos.y / scale))
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
    }

    pub(super) fn playback_needs_live_repaint(&self) -> bool {
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        if self.myinstants_preview_audio_url.is_some() {
            return true;
        }
        if let Some(viewer) = self.video_viewer.as_ref()
            && audio.is_playing_file(&viewer.audio_path)
        {
            return true;
        }
        if self.show_record_review_panel
            && let Some(draft) = self.recording_draft.as_ref()
            && audio.is_playing(draft.sound.id)
        {
            return true;
        }
        if let Some(selected) = self.selected
            && audio.is_playing(selected)
        {
            return true;
        }
        false
    }

    pub(super) fn handle_trim_start_preview(&mut self, ctx: &Context) {
        if self.is_transition_active() {
            return;
        }

        if self.show_record_review_panel {
            if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::S)) {
                return;
            }
            let Some(sound) = self
                .recording_draft
                .as_ref()
                .map(|draft| draft.sound.clone())
            else {
                return;
            };
            self.set_preview_cursor_secs(sound.id, sound.trim_start_secs, sound.safe_duration());
            self.preview_recording_draft_from_position(Some(sound.trim_start_secs));
            return;
        }

        if self.has_modal_panel() || ctx.wants_keyboard_input() {
            return;
        }

        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::S)) {
            return;
        }

        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound = self.sounds[index].clone();
        self.set_preview_cursor_secs(sound.id, sound.trim_start_secs, sound.safe_duration());
        self.preview_sound_from_position(sound.id, Some(sound.trim_start_secs));
    }

    pub(super) fn open_recording_review(&mut self, path: &Path) {
        self.close_recording_review(true);
        let name = if self.record_name.trim().is_empty() {
            "recording".to_owned()
        } else {
            self.record_name.trim().to_owned()
        };

        match self.storage.analyze_sound_as_effect(path, &name) {
            Ok(mut sound) => {
                self.stop_preview();
                sound.waveform = Self::center_waveform_visual(&sound.waveform);
                let preview_id = sound.id;
                let preview_start = sound.trim_start_secs;
                let preview_duration = sound.safe_duration();
                self.trim_timeline_zoom = 1.0;
                self.recording_draft = Some(RecordingDraft {
                    sound,
                    source_path: path.to_path_buf(),
                    source_is_temporary: true,
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
            Err(error) => {
                let _ = fs::remove_file(path);
                self.set_error_status(error);
            }
        }
    }

    pub(super) fn poll_record_video_export(&mut self, ctx: &Context) {
        let mut finished: Option<Result<RecordVideoExportResult, String>> = None;

        if let Some(export) = self.active_record_video_export.as_mut() {
            while let Ok(message) = export.receiver.try_recv() {
                match message {
                    RecordVideoExportMessage::Progress { progress, stage } => {
                        export.progress = progress.clamp(0.0, 1.0);
                        export.stage = stage;
                        ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
                    }
                    RecordVideoExportMessage::Finished(result) => {
                        finished = Some(result);
                        break;
                    }
                }
            }
        }

        let Some(result) = finished else {
            return;
        };

        self.active_record_video_export = None;

        match result {
            Ok(exported) => {
                match self.storage.import_video(
                    &exported.video_path,
                    &exported.video_name,
                    exported.duration_secs,
                    exported.video_fps,
                ) {
                    Ok(mut video) => {
                        if let Ok(waveform) = self
                            .storage
                            .analyze_waveform_preview(&exported.processed_audio_path, 96)
                        {
                            video.waveform = waveform;
                        }
                        self.video_assets.insert(0, video.clone());
                        let _ = self.storage.save_video_library(&self.video_assets);
                        self.app_view = AppView::Library;
                        self.library_tab = LibraryTab::Videos;
                        let mut opened_viewer = false;
                        match self.prepare_video_viewer(ctx, &video) {
                            Ok(()) => {
                                opened_viewer = true;
                                if let Err(error) = self.play_video_viewer_from_current_playhead() {
                                    self.set_error_status(error);
                                }
                            }
                            Err(error) => self.set_error_status(error),
                        }
                        if opened_viewer {
                            self.clear_status();
                        }
                    }
                    Err(error) => self.set_error_status(error),
                }
                let _ = fs::remove_file(&exported.processed_audio_path);
                let _ = fs::remove_file(&exported.video_path);
            }
            Err(error) => self.set_error_status(error),
        }
    }

    pub(super) fn handle_dropped_files(&mut self, ctx: &Context) {
        let is_folder_open = self.app_view == AppView::Library
            && self.library_tab == LibraryTab::Folders
            && self.library_current_folder.is_some();

        if (self.app_view != AppView::Editor && !is_folder_open) || self.has_modal_panel() {
            self.editor_drop_armed = false;
            self.editor_drop_rect = None;
            return;
        }

        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let pointer_pos = self.external_drop_pointer_pos(ctx);
        let dropped_in_rect = is_folder_open
            || self
                .editor_drop_rect
                .is_some_and(|rect| pointer_pos.is_some_and(|pos| rect.contains(pos)));
        if !self.editor_drop_armed && !dropped_in_rect {
            return;
        }
        self.editor_drop_armed = false;

        let mut paths = Vec::new();
        for path in dropped.into_iter().filter_map(|file| file.path) {
            let normalized = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if self.ignored_drop_path.as_ref() == Some(&normalized) {
                self.ignored_drop_path = None;
                continue;
            }
            paths.push(path);
        }
        if !paths.is_empty() {
            self.import_paths(paths);
        }
    }

    pub(super) fn trim_playhead_drag_id(sound_id: Uuid) -> egui::Id {
        egui::Id::new((sound_id, "trim-playhead-drag"))
    }

    pub(super) fn poll_vocal_separation_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.vocal_separation_rx.try_recv() {
            let elapsed_secs = self.vocal_separation_elapsed_secs();
            self.vocal_separation_running = false;
            self.vocal_separation_cancel = None;
            self.vocal_separation_target = None;
            self.vocal_separation_kind = None;
            self.vocal_separation_started_at = None;
            match message {
                VocalSeparationMessage::Cancelled => {
                    self.vocal_separation_last_result = None;
                    self.clear_status();
                }
                VocalSeparationMessage::Finished {
                    target,
                    kind,
                    result,
                } => match target {
                    VocalSeparationTarget::RecordingReview { source_path } => match result {
                        Ok(path) => {
                            let record_sound_id = self.recording_draft.as_ref().and_then(|draft| {
                                (draft.source_path == source_path).then_some(draft.sound.id)
                            });
                            if let Some(draft) = self.recording_draft.as_mut()
                                && draft.source_path == source_path
                            {
                                match kind {
                                    SeparationStemKind::Vocal => {
                                        draft.vocal_separated_path = Some(path.clone());
                                    }
                                    SeparationStemKind::Music => {
                                        draft.music_separated_path = Some(path.clone());
                                    }
                                }
                            }
                            if let Some(sound_id) = record_sound_id {
                                match kind {
                                    SeparationStemKind::Vocal => {
                                        let _ = self.cached_stem_waveform(
                                            &self.vocal_waveform_cache,
                                            sound_id,
                                            &path,
                                        );
                                    }
                                    SeparationStemKind::Music => {
                                        let _ = self.cached_stem_waveform(
                                            &self.music_waveform_cache,
                                            sound_id,
                                            &path,
                                        );
                                    }
                                }
                            }
                            if let Some(elapsed_secs) = elapsed_secs {
                                self.vocal_separation_last_result = Some((
                                    VocalSeparationTarget::RecordingReview {
                                        source_path: source_path.clone(),
                                    },
                                    kind,
                                    elapsed_secs,
                                ));
                            }
                            self.clear_status();
                        }
                        Err(error) => {
                            if let Some(draft) = self.recording_draft.as_mut()
                                && draft.source_path == source_path
                            {
                                match kind {
                                    SeparationStemKind::Vocal => {
                                        draft.keep_vocal = false;
                                        draft.vocal_separated_path = None;
                                    }
                                    SeparationStemKind::Music => {
                                        draft.keep_music = false;
                                        draft.music_separated_path = None;
                                    }
                                }
                            }
                            self.vocal_separation_last_result = None;
                            self.set_error_status(error);
                        }
                    },
                    VocalSeparationTarget::LibrarySound {
                        sound_id,
                        source_path,
                    } => match result {
                        Ok(temp_path) => {
                            let mut preload_path = None;
                            let mut refresh_cursor_secs = None;
                            let mut copy_error: Option<String> = None;
                            let mut cache_path: Option<PathBuf> = None;
                            if let Some(index) =
                                self.sounds.iter().position(|sound| sound.id == sound_id)
                            {
                                let root_dir = self.storage.root_dir().to_path_buf();
                                let is_playing = self
                                    .audio
                                    .as_ref()
                                    .is_some_and(|audio| audio.is_playing(sound_id));
                                if is_playing {
                                    refresh_cursor_secs =
                                        Some(self.preview_cursor_secs_for(&self.sounds[index]));
                                }
                                let sound = &mut self.sounds[index];
                                let stable_path = match kind {
                                    SeparationStemKind::Vocal => root_dir
                                        .join("sound-vocals")
                                        .join(Storage::vocal_asset_file_name(sound_id)),
                                    SeparationStemKind::Music => root_dir
                                        .join("sound-music")
                                        .join(Storage::music_asset_file_name(sound_id)),
                                };
                                if let Some(parent) = stable_path.parent() {
                                    let _ = fs::create_dir_all(parent);
                                }
                                if temp_path != stable_path {
                                    if stable_path.exists() {
                                        let _ = fs::remove_file(&stable_path);
                                    }
                                    if let Err(error) = fs::copy(&temp_path, &stable_path) {
                                        copy_error = Some(error.to_string());
                                    } else {
                                        let _ = fs::remove_file(&temp_path);
                                    }
                                }
                                if copy_error.is_none() {
                                    match kind {
                                        SeparationStemKind::Vocal => {
                                            let requested_vocal_only = sound.vocal_only;
                                            sound.vocal_asset_file =
                                                Some(Storage::vocal_asset_file_name(sound_id));
                                            preload_path =
                                                self.storage.vocal_asset_path_for(&*sound);
                                            cache_path = Some(stable_path.clone());
                                            if !requested_vocal_only {
                                                refresh_cursor_secs = None;
                                            }
                                        }
                                        SeparationStemKind::Music => {
                                            let requested_music_only = sound.music_only;
                                            let music_file =
                                                Storage::music_asset_file_name(sound_id);
                                            sound.music_asset_file = Some(music_file);
                                            preload_path =
                                                self.storage.music_asset_path_for(&*sound);
                                            cache_path = Some(stable_path.clone());
                                            if !requested_music_only {
                                                refresh_cursor_secs = None;
                                            }
                                        }
                                    }
                                }
                            } else {
                                let _ = fs::remove_file(&temp_path);
                            }
                            if let Some(error) = copy_error {
                                self.set_error_status(error);
                                let _ = fs::remove_file(&temp_path);
                                return;
                            }
                            let saved_ok = if preload_path.is_some() {
                                self.mark_dirty(ctx);
                                self.save_now()
                            } else {
                                true
                            };
                            if let Some(path) = preload_path {
                                self.schedule_audio_preload(path);
                            }
                            if let Some(cursor_secs) = refresh_cursor_secs {
                                self.preview_sound_from_position(sound_id, Some(cursor_secs));
                            }
                            if copy_error.is_none()
                                && let Some(cache_path) = cache_path.as_ref()
                            {
                                match kind {
                                    SeparationStemKind::Vocal => {
                                        let _ = self.cached_stem_waveform(
                                            &self.vocal_waveform_cache,
                                            sound_id,
                                            cache_path,
                                        );
                                    }
                                    SeparationStemKind::Music => {
                                        let _ = self.cached_stem_waveform(
                                            &self.music_waveform_cache,
                                            sound_id,
                                            cache_path,
                                        );
                                    }
                                }
                            }
                            if self.selected == Some(sound_id) {
                                ctx.request_repaint();
                            }
                            if saved_ok {
                                if let Some(elapsed_secs) = elapsed_secs {
                                    self.vocal_separation_last_result = Some((
                                        VocalSeparationTarget::LibrarySound {
                                            sound_id,
                                            source_path: source_path.clone(),
                                        },
                                        kind,
                                        elapsed_secs,
                                    ));
                                }
                                self.clear_status();
                            }
                        }
                        Err(error) => {
                            self.vocal_separation_last_result = None;
                            self.set_error_status(error);
                        }
                    },
                },
            }
            ctx.request_repaint();
        }
    }

    pub(super) fn stop_preview(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.myinstants_preview_audio_url = None;
        self.pending_preview_after_preload = None;
    }

    pub(super) fn maybe_start_pending_processed_export(&mut self) {
        let Some(sound_id) = self.pending_processed_export_sound else {
            return;
        };
        if self.selected == Some(sound_id) {
            return;
        }

        if self.pending_save && !self.save_now() {
            return;
        }

        self.pending_processed_export_sound = None;
        self.spawn_processed_export_job(sound_id);
    }

    pub(super) fn poll_processed_export_jobs(&mut self, ctx: &Context) {
        let mut finished_exports = Vec::new();
        while let Ok(message) = self.processed_export_rx.try_recv() {
            match message {
                ProcessedExportMessage::Finished {
                    export_path,
                    result,
                } => {
                    self.processed_export_inflight.remove(&export_path);
                    if let Err(error) = result {
                        if self.selected.is_some() {
                            self.set_error_status(error);
                        }
                    }
                    finished_exports.push(export_path);
                }
            }
        }

        if !finished_exports.is_empty() {
            ctx.request_repaint();
        }
    }

    pub(super) fn poll_trim_commit_jobs(&mut self, ctx: &Context) {
        let mut changed = false;
        while let Ok(message) = self.trim_commit_rx.try_recv() {
            match message {
                TrimCommitMessage::Finished {
                    sound_id,
                    keep_old,
                    result,
                } => {
                    self.trim_commit_inflight.remove(&sound_id);
                    match result {
                        Ok(sound) => {
                            if keep_old {
                                self.selected = Some(sound.id);
                                self.sounds.insert(0, sound);
                            } else if let Some(index) =
                                self.sounds.iter().position(|item| item.id == sound_id)
                            {
                                self.sounds[index] = sound;
                            }
                            if self.save_now() {
                                self.clear_status();
                            }
                            changed = true;
                        }
                        Err(error) => self.set_error_status(error),
                    }
                }
            }
        }

        if changed {
            ctx.request_repaint();
        }
    }

    pub(super) fn preload_selected_sound_audio(&mut self) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound = &self.sounds[index];
        let asset_path = self.preview_asset_path_for_sound(sound);
        self.schedule_audio_preload(asset_path);
    }

    pub(super) fn render_record_review_panel(&mut self, ctx: &Context) {
        if !self.show_record_review_panel {
            return;
        }

        let Some(sound_snapshot) = self
            .recording_draft
            .as_ref()
            .map(|draft| draft.sound.clone())
        else {
            self.show_record_review_panel = false;
            return;
        };

        let sound_id = sound_snapshot.id;
        let is_playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound_id));
        let playhead_drag_active = ctx
            .data(|data| data.get_temp::<bool>(Self::trim_playhead_drag_id(sound_id)))
            .unwrap_or(false);
        let playback_position_secs = self
            .audio
            .as_ref()
            .and_then(|audio| audio.playback_position_secs(sound_id));
        if is_playing {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
        }

        let mut preview_cursor_secs = self.preview_cursor_secs_for(&sound_snapshot);
        if let Some(position_secs) = playback_position_secs
            && !playhead_drag_active
        {
            preview_cursor_secs = position_secs.clamp(0.0, sound_snapshot.safe_duration());
        }

        let mut open_panel = self.show_record_review_panel;
        let mut close_request = false;
        let mut save_audio = false;
        let mut export_video = false;
        let mut preview_toggle = false;
        let mut seek_request = false;
        let mut changed = false;
        let mut discard_request = false;
        let mut start_vocal_job = false;
        let mut start_music_job = false;
        let mut trim_timeline_zoom = self.trim_timeline_zoom;
        let record_vocal_job_running = self.vocal_separation_running
            && self.vocal_separation_kind == Some(SeparationStemKind::Vocal)
            && matches!(
                self.vocal_separation_target.as_ref(),
                Some(VocalSeparationTarget::RecordingReview { source_path })
                    if self
                        .recording_draft
                        .as_ref()
                        .is_some_and(|draft| draft.source_path == *source_path)
            );
        let record_music_job_running = self.vocal_separation_running
            && self.vocal_separation_kind == Some(SeparationStemKind::Music)
            && matches!(
                self.vocal_separation_target.as_ref(),
                Some(VocalSeparationTarget::RecordingReview { source_path })
                    if self
                        .recording_draft
                        .as_ref()
                        .is_some_and(|draft| draft.source_path == *source_path)
            );
        let vocal_only_label = self.t("editor.vocal_only");
        let vocal_ready_label = self.t("editor.vocal_ready");
        let vocal_loading_label = self.t("editor.vocal_loading");
        let music_only_label = self.t("editor.music_only");
        let music_loading_label = self.t("editor.music_loading");
        let music_ready_label = self.t("editor.music_ready");
        let effects_label = self.t("editor.effects");
        let effect_reverb_label = self.t("editor.effect_reverb");
        let effect_telephone_label = self.t("editor.effect_telephone");
        let effect_distortion_label = self.t("editor.effect_distortion");
        let effect_echo_label = self.t("editor.effect_echo");
        let effect_underwater_label = self.t("editor.effect_underwater");
        let effect_robot_label = self.t("editor.effect_robot");
        let effect_pitch_shift_label = self.t("editor.effect_pitch_shift");
        let effect_reverb_hint = self.t("editor.effect_reverb_hint");
        let effect_telephone_hint = self.t("editor.effect_telephone_hint");
        let effect_distortion_hint = self.t("editor.effect_distortion_hint");
        let effect_echo_hint = self.t("editor.effect_echo_hint");
        let effect_underwater_hint = self.t("editor.effect_underwater_hint");
        let effect_robot_hint = self.t("editor.effect_robot_hint");
        let effect_pitch_shift_hint = self.t("editor.effect_pitch_shift_hint");
        let vocal_elapsed_label = self.t("editor.vocal_elapsed");
        let vocal_elapsed_text = if record_vocal_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let music_elapsed_text = if record_music_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let vocal_last_elapsed_text = self
            .recording_draft
            .as_ref()
            .and_then(|draft| {
                self.vocal_separation_last_elapsed_for_recording(
                    &draft.source_path,
                    SeparationStemKind::Vocal,
                )
            })
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let music_last_elapsed_text = self
            .recording_draft
            .as_ref()
            .and_then(|draft| {
                self.vocal_separation_last_elapsed_for_recording(
                    &draft.source_path,
                    SeparationStemKind::Music,
                )
            })
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let waveform_samples = self
            .recording_draft
            .as_ref()
            .map(|draft| self.recording_waveform_samples(draft))
            .unwrap_or_default();
        let export_progress = self
            .active_record_video_export
            .as_ref()
            .map(|export| (export.progress, export.stage.clone()));
        let exporting_video = export_progress.is_some();
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(680.0, 560.0), vec2(360.0, 300.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("record-review-panel"))
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
                    .corner_radius(32.0)
                    .inner_margin(Margin::same(22)),
            )
            .show(ctx, |ui| {
                let draft = self.recording_draft.as_mut().expect("record draft missing");
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe061, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                let response = ui.add_sized(
                    [ui.available_width(), 48.0],
                    TextEdit::singleline(&mut draft.sound.name)
                        .font(egui::TextStyle::Heading)
                        .desired_width(f32::INFINITY)
                        .margin(Vec2::new(14.0, 14.0)),
                );
                if response.changed() {
                    changed = true;
                }

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        let (timeline_changed, timeline_seek_request, timeline_preview_commit) =
                            Self::draw_trim_timeline(
                                ui,
                                &mut draft.sound,
                                &waveform_samples,
                                &mut preview_cursor_secs,
                                &mut trim_timeline_zoom,
                                !is_playing,
                                true,
                                false,
                            );
                        changed |= timeline_changed;
                        seek_request |= timeline_seek_request;
                        if timeline_preview_commit {
                            seek_request = true;
                        }
                    });

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(26.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        Self::with_slider_visuals(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(Self::icon(0xe050, 16.0, Self::muted_text_color()));
                                let (volume_response, volume_slider_changed) = Self::click_slider(
                                    ui,
                                    &mut draft.sound.volume,
                                    0.0..=5.0,
                                    0.0,
                                    vec2(128.0, 24.0),
                                );
                                let volume_input = ui.add(
                                    DragValue::new(&mut draft.sound.volume)
                                        .range(0.0..=5.0)
                                        .speed(0.01)
                                        .max_decimals(2)
                                        .suffix("x"),
                                );
                                draft.sound.volume = draft.sound.volume.clamp(0.0, 5.0);
                                ui.add_space(10.0);
                                ui.label(Self::icon(0xe9e4, 16.0, Self::muted_text_color()));
                                let (speed_response, speed_slider_changed) = Self::click_slider(
                                    ui,
                                    &mut draft.sound.speed,
                                    0.25..=2.0,
                                    0.0,
                                    vec2(128.0, 24.0),
                                );
                                let speed_input = ui.add(
                                    DragValue::new(&mut draft.sound.speed)
                                        .range(0.25..=2.0)
                                        .speed(0.01)
                                        .max_decimals(2)
                                        .suffix("x"),
                                );
                                draft.sound.speed = draft.sound.speed.clamp(0.25, 2.0);
                                if volume_response.changed()
                                    || speed_response.changed()
                                    || volume_input.changed()
                                    || speed_input.changed()
                                    || volume_slider_changed
                                    || speed_slider_changed
                                {
                                    changed = true;
                                }
                            });
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&effects_label)
                                        .size(12.0)
                                        .color(Self::muted_text_color()),
                                );
                                let reverb = ui
                                    .add_sized(
                                        [88.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_reverb_label).size(11.5),
                                            draft.sound.reverb_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_reverb_hint);
                                Self::decorate_button_response(ui, &reverb);
                                if reverb.clicked() {
                                    draft.sound.reverb_enabled = !draft.sound.reverb_enabled;
                                    changed = true;
                                }

                                let telephone = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_telephone_label).size(11.5),
                                            draft.sound.telephone_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_telephone_hint);
                                Self::decorate_button_response(ui, &telephone);
                                if telephone.clicked() {
                                    draft.sound.telephone_enabled = !draft.sound.telephone_enabled;
                                    changed = true;
                                }

                                let distortion = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_distortion_label).size(11.5),
                                            draft.sound.distortion_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_distortion_hint);
                                Self::decorate_button_response(ui, &distortion);
                                if distortion.clicked() {
                                    draft.sound.distortion_enabled =
                                        !draft.sound.distortion_enabled;
                                    changed = true;
                                }

                                let echo = ui
                                    .add_sized(
                                        [78.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_echo_label).size(11.5),
                                            draft.sound.echo_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_echo_hint);
                                Self::decorate_button_response(ui, &echo);
                                if echo.clicked() {
                                    draft.sound.echo_enabled = !draft.sound.echo_enabled;
                                    changed = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(54.0);
                                let underwater = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_underwater_label).size(11.5),
                                            draft.sound.underwater_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_underwater_hint);
                                Self::decorate_button_response(ui, &underwater);
                                if underwater.clicked() {
                                    draft.sound.underwater_enabled =
                                        !draft.sound.underwater_enabled;
                                    changed = true;
                                }

                                let robot = ui
                                    .add_sized(
                                        [78.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_robot_label).size(11.5),
                                            draft.sound.robot_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_robot_hint);
                                Self::decorate_button_response(ui, &robot);
                                if robot.clicked() {
                                    draft.sound.robot_enabled = !draft.sound.robot_enabled;
                                    changed = true;
                                }

                                let pitch_shift = ui
                                    .add_sized(
                                        [98.0, 30.0],
                                        Self::action_button(
                                            RichText::new(&effect_pitch_shift_label).size(11.5),
                                            draft.sound.pitch_shift_enabled,
                                            false,
                                        ),
                                    )
                                    .on_hover_text(&effect_pitch_shift_hint);
                                Self::decorate_button_response(ui, &pitch_shift);
                                if pitch_shift.clicked() {
                                    draft.sound.pitch_shift_enabled =
                                        !draft.sound.pitch_shift_enabled;
                                    changed = true;
                                }
                                if draft.sound.pitch_shift_enabled {
                                    let semitone_input = ui.add(
                                        DragValue::new(&mut draft.sound.pitch_shift_semitones)
                                            .range(-24.0..=24.0)
                                            .speed(0.1)
                                            .max_decimals(1)
                                            .suffix(" st"),
                                    );
                                    if Self::deferred_drag_value_commit(ctx, &semitone_input) {
                                        changed = true;
                                    }
                                }
                            });
                        });
                    });

                ui.add_space(14.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            let demucs_available = crate::vocal_separation::is_demucs_available();
                            let border = if self.dark_theme {
                                Color32::WHITE
                            } else {
                                Color32::BLACK
                            };

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&vocal_only_label)
                                        .size(12.5)
                                        .color(Self::strong_text_color()),
                                );
                                if self.demucs_installing {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new("Installing demucs-rs...")
                                            .size(11.5)
                                            .color(Self::muted_text_color()),
                                    );
                                } else if demucs_available {
                                    let vocal_toggle = ui
                                        .scope(|ui| {
                                            let visuals = &mut ui.style_mut().visuals;
                                            visuals.widgets.inactive.bg_stroke.color = border;
                                            visuals.widgets.hovered.bg_stroke.color = border;
                                            visuals.widgets.active.bg_stroke.color = border;
                                            ui.add(Checkbox::new(&mut draft.keep_vocal, ""))
                                        })
                                        .inner;
                                    if vocal_toggle.changed() {
                                        changed = true;
                                        ctx.request_repaint();
                                        if draft.keep_vocal {
                                            draft.keep_music = false;
                                            draft.vocal_separated_path = None;
                                            start_vocal_job = true;
                                        } else {
                                            draft.vocal_separated_path = None;
                                        }
                                    }
                                    if record_vocal_job_running && draft.keep_vocal {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(&vocal_loading_label)
                                                .size(11.5)
                                                .color(Self::muted_text_color()),
                                        );
                                        if let Some(vocal_elapsed_text) = &vocal_elapsed_text {
                                            ui.label(
                                                RichText::new(vocal_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                    } else if draft.keep_vocal
                                        && let Some(vocal_last_elapsed_text) =
                                            &vocal_last_elapsed_text
                                    {
                                        ui.label(
                                            RichText::new(&vocal_ready_label)
                                                .size(11.5)
                                                .color(Color32::from_rgb(100, 200, 100)),
                                        );
                                        ui.label(
                                            RichText::new(vocal_last_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                }
                            });

                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&music_only_label)
                                        .size(12.5)
                                        .color(Self::strong_text_color()),
                                );
                                if self.demucs_installing {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new("Installing demucs-rs...")
                                            .size(11.5)
                                            .color(Self::muted_text_color()),
                                    );
                                } else if demucs_available {
                                    let music_toggle = ui
                                        .scope(|ui| {
                                            let visuals = &mut ui.style_mut().visuals;
                                            visuals.widgets.inactive.bg_stroke.color = border;
                                            visuals.widgets.hovered.bg_stroke.color = border;
                                            visuals.widgets.active.bg_stroke.color = border;
                                            ui.add(Checkbox::new(&mut draft.keep_music, ""))
                                        })
                                        .inner;
                                    if music_toggle.changed() {
                                        changed = true;
                                        ctx.request_repaint();
                                        if draft.keep_music {
                                            draft.keep_vocal = false;
                                            draft.music_separated_path = None;
                                            start_music_job = true;
                                        } else {
                                            draft.music_separated_path = None;
                                        }
                                    }
                                    if record_music_job_running && draft.keep_music {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(&music_loading_label)
                                                .size(11.5)
                                                .color(Self::muted_text_color()),
                                        );
                                        if let Some(music_elapsed_text) = &music_elapsed_text {
                                            ui.label(
                                                RichText::new(music_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                    } else if draft.keep_music
                                        && let Some(music_last_elapsed_text) =
                                            &music_last_elapsed_text
                                    {
                                        ui.label(
                                            RichText::new(&music_ready_label)
                                                .size(11.5)
                                                .color(Color32::from_rgb(100, 200, 100)),
                                        );
                                        ui.label(
                                            RichText::new(music_last_elapsed_text)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                    }
                                }
                            });

                            if self.demucs_model_loading {
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new("Preparing vocal model...")
                                            .size(11.5)
                                            .color(Self::muted_text_color()),
                                    );
                                });
                            } else if self.demucs_model_ready {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new("(model ready)")
                                        .size(11.5)
                                        .color(Color32::from_rgb(100, 200, 100)),
                                );
                            } else if !demucs_available {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new("(install demucs-rs in Settings)")
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            } else if draft.keep_vocal || draft.keep_music {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new("(first run can be slower)")
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            }
                        });
                    });

                ui.add_space(18.0);
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(22.0)
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                vec2(44.0, 32.0),
                                egui::Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("Export")
                                            .size(12.5)
                                            .color(Self::muted_text_color()),
                                    );
                                },
                            );
                            ui.add_space(8.0);

                            let animation = ui.add_sized(
                                [108.0, 32.0],
                                Self::action_button(
                                    RichText::new("Animation").size(12.5),
                                    self.record_export_video_animation,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &animation);
                            if animation.clicked() {
                                self.record_export_video_animation =
                                    !self.record_export_video_animation;
                                ctx.request_repaint();
                            }

                            let sharp = ui.add_sized(
                                [88.0, 32.0],
                                Self::action_button(
                                    RichText::new(self.t("record.sharp")).size(12.5),
                                    self.record_export_video_sharps,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &sharp);
                            if sharp.clicked() {
                                self.record_export_video_sharps = !self.record_export_video_sharps;
                                ctx.request_repaint();
                            }

                            ui.add_space(10.0);
                            ui.allocate_ui_with_layout(
                                vec2(24.0, 32.0),
                                egui::Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("FPS")
                                            .size(12.5)
                                            .color(Self::muted_text_color()),
                                    );
                                },
                            );
                            for fps in RECORD_EXPORT_VIDEO_FPS_OPTIONS {
                                let active = self.record_export_video_fps == fps;
                                let response = ui.add_sized(
                                    [74.0, 32.0],
                                    Self::action_button(
                                        RichText::new(format!("{fps}fps")).size(12.5),
                                        active,
                                        false,
                                    ),
                                );
                                Self::decorate_button_response(ui, &response);
                                if response.clicked() {
                                    self.record_export_video_fps = fps;
                                    ctx.request_repaint();
                                }
                            }
                        });
                    });

                ui.add_space(12.0);
                if let Some((progress, stage)) = export_progress.as_ref() {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(22.0)
                        .inner_margin(Margin::same(16))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(stage.as_str())
                                    .size(13.0)
                                    .color(Self::strong_text_color()),
                            );
                            ui.add_space(8.0);
                            ui.add(
                                ProgressBar::new(*progress)
                                    .desired_width(ui.available_width())
                                    .fill(Color32::from_rgb(227, 82, 149))
                                    .text(format!("{:.0}%", *progress * 100.0)),
                            );
                        });
                    ui.add_space(12.0);
                }

                ui.horizontal_centered(|ui| {
                    let preview = ui.add_sized(
                        [118.0, 38.0],
                        Self::action_button(
                            RichText::new(if is_playing { "Stop" } else { "Preview" }).size(13.0),
                            is_playing,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &preview);
                    if preview.clicked() {
                        preview_toggle = true;
                    }

                    let save = ui.add_sized(
                        [132.0, 38.0],
                        Self::action_button(RichText::new("Save audio").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &save);
                    if save.clicked() {
                        save_audio = true;
                    }

                    let video = ui
                        .add_enabled_ui(!exporting_video, |ui| {
                            ui.add_sized(
                                [132.0, 38.0],
                                Self::action_button(
                                    RichText::new("Export SPN").size(13.0),
                                    false,
                                    false,
                                ),
                            )
                        })
                        .inner;
                    Self::decorate_button_response(ui, &video);
                    if video.clicked() {
                        export_video = true;
                    }

                    let discard = ui
                        .add_enabled_ui(!exporting_video, |ui| {
                            ui.add_sized(
                                [118.0, 38.0],
                                Self::action_button(
                                    RichText::new("Discard").size(13.0),
                                    false,
                                    false,
                                ),
                            )
                        })
                        .inner;
                    Self::decorate_button_response(ui, &discard);
                    if discard.clicked() {
                        discard_request = true;
                    }
                });
            });

        let Some(updated_sound) = self
            .recording_draft
            .as_ref()
            .map(|draft| draft.sound.clone())
        else {
            self.show_record_review_panel = false;
            return;
        };
        let sound_duration = updated_sound.safe_duration();
        self.trim_timeline_zoom = trim_timeline_zoom;
        self.set_preview_cursor_secs(sound_id, preview_cursor_secs, sound_duration);
        self.show_record_review_panel = open_panel;
        if start_vocal_job {
            self.start_vocal_separation_if_needed();
        }
        if start_music_job {
            self.start_music_separation_if_needed();
        }

        if seek_request && is_playing {
            self.preview_recording_draft_from_position(Some(preview_cursor_secs));
        }
        if preview_toggle {
            if is_playing {
                self.stop_preview();
            } else {
                let mut cursor_secs = self.preview_cursor_secs_for(&updated_sound);
                if cursor_secs >= updated_sound.trim_end_secs - 0.02 {
                    cursor_secs = updated_sound.trim_start_secs;
                    self.set_preview_cursor_secs(sound_id, cursor_secs, sound_duration);
                }
                self.preview_recording_draft_from_position(Some(cursor_secs));
            }
        }
        if changed {
            ctx.request_repaint();
        }
        if export_video {
            self.export_recording_review_video();
        }
        if save_audio {
            self.save_recording_review_to_library();
        } else if discard_request {
            self.close_recording_review(true);
        } else if close_request || !open_panel {
            self.hide_recording_review();
        }
    }
}
