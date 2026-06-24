use super::*;
#[cfg(windows)]
use clipboard_win::Getter;

const TRIM_HISTORY_LIMIT: usize = 128;

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
        self.clear_trim_history_for_sound(sound.id);
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

    fn push_trim_history_entry(stack: &mut Vec<TrimSnapshot>, snapshot: TrimSnapshot) {
        stack.push(snapshot);
        if stack.len() > TRIM_HISTORY_LIMIT {
            stack.remove(0);
        }
    }

    fn clear_trim_history_for_sound(&mut self, sound_id: Uuid) {
        self.trim_undo_stack
            .retain(|snapshot| snapshot.sound_id != sound_id);
        self.trim_redo_stack
            .retain(|snapshot| snapshot.sound_id != sound_id);
    }

    fn push_trim_undo_snapshot(&mut self, before: TrimSnapshot, after: TrimSnapshot) {
        if before != after {
            Self::push_trim_history_entry(&mut self.trim_undo_stack, before);
            self.trim_redo_stack.clear();
        }
    }

    fn apply_selected_trim_snapshot(&mut self, snapshot: TrimSnapshot, ctx: &Context) -> bool {
        let Some(index) = self.selected_sound_index() else {
            return false;
        };
        let (sound_id, sound_duration, clamped_cursor) = {
            let sound = &mut self.sounds[index];
            if sound.id != snapshot.sound_id || snapshot.matches_sound(sound) {
                return false;
            }

            snapshot.apply_to(sound);
            let current_cursor = self
                .preview_cursor
                .and_then(|(preview_sound_id, secs)| (preview_sound_id == sound.id).then_some(secs))
                .unwrap_or(sound.display_trim_start());
            let clamped_cursor =
                current_cursor.clamp(sound.display_trim_start(), sound.display_trim_end());
            (sound.id, sound.safe_duration(), clamped_cursor)
        };

        self.set_preview_cursor_secs(sound_id, clamped_cursor, sound_duration);
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.is_playing(sound_id))
        {
            self.stop_preview();
        }
        self.schedule_processed_export(sound_id);
        self.mark_dirty(ctx);
        ctx.request_repaint();
        true
    }

    pub(super) fn handle_trim_undo_redo(&mut self, ctx: &Context) {
        if self.is_transition_active()
            || self.show_record_review_panel
            || self.has_modal_panel()
            || ctx.wants_keyboard_input()
        {
            return;
        }

        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound_id = self.sounds[index].id;
        let undo_modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let redo_modifiers = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };

        if ctx.input_mut(|input| input.consume_key(redo_modifiers, egui::Key::Z)) {
            let Some(position) = self
                .trim_redo_stack
                .iter()
                .rposition(|snapshot| snapshot.sound_id == sound_id)
            else {
                return;
            };
            let current = TrimSnapshot::from_sound(&self.sounds[index]);
            let snapshot = self.trim_redo_stack.remove(position);
            if self.apply_selected_trim_snapshot(snapshot, ctx) {
                Self::push_trim_history_entry(&mut self.trim_undo_stack, current);
            }
            return;
        }

        if ctx.input_mut(|input| input.consume_key(undo_modifiers, egui::Key::Z)) {
            let Some(position) = self
                .trim_undo_stack
                .iter()
                .rposition(|snapshot| snapshot.sound_id == sound_id)
            else {
                return;
            };
            let current = TrimSnapshot::from_sound(&self.sounds[index]);
            let snapshot = self.trim_undo_stack.remove(position);
            if self.apply_selected_trim_snapshot(snapshot, ctx) {
                Self::push_trim_history_entry(&mut self.trim_redo_stack, current);
            }
        }
    }

    fn default_trim_timeline_state(sound_id: Uuid) -> TrimTimelineState {
        TrimTimelineState {
            sound_id,
            enabled: false,
            playhead_secs: 0.0,
            snap_enabled: true,
            selected_clip_id: None,
            rows: Self::default_trim_timeline_rows(sound_id),
        }
    }

    fn default_trim_timeline_rows(sound_id: Uuid) -> Vec<TrimTimelineRow> {
        vec![
            TrimTimelineRow {
                clips: vec![TrimTimelineClip {
                    id: Uuid::new_v4(),
                    source_sound_id: sound_id,
                    start_secs: 0.0,
                    clip_start_secs: 0.0,
                    clip_end_secs: 0.0,
                }],
            },
            TrimTimelineRow::default(),
            TrimTimelineRow::default(),
        ]
    }

    fn sync_trim_timeline_state_for(&mut self, sound_id: Uuid) {
        let mut created_new = false;
        if self
            .trim_timeline_state
            .as_ref()
            .is_none_or(|state| state.sound_id != sound_id)
        {
            self.trim_timeline_state = Some(Self::default_trim_timeline_state(sound_id));
            self.trim_timeline_view_start_secs = 0.0;
            created_new = true;
        }

        let timeline_zoom = self.trim_timeline_zoom;
        let timeline_view_start_secs = self.trim_timeline_view_start_secs.max(0.0);

        let Some(state) = self.trim_timeline_state.as_mut() else {
            return;
        };
        if state.rows.is_empty() {
            state.rows.push(TrimTimelineRow::default());
        }
        if created_new
            && let Some(base_row) = state.rows.first_mut()
            && let Some(base_clip) = base_row.clips.first_mut()
            && let Some(base_sound) = self.sounds.iter().find(|sound| sound.id == sound_id)
        {
            let base_length = base_sound.trimmed_length();
            base_clip.clip_start_secs = 0.0;
            base_clip.clip_end_secs = base_length.max(0.05);
        }
        let total_duration = state
            .rows
            .iter()
            .flat_map(|row| row.clips.iter())
            .map(|clip| clip.start_secs.max(0.0) + (clip.clip_end_secs - clip.clip_start_secs).max(0.05))
            .fold(0.0f32, f32::max)
            .max(0.25);
        let visible_duration = (12.0 / timeline_zoom.max(0.1)).max(0.25);
        let workspace_padding_secs = visible_duration.max(24.0);
        let workspace_duration =
            total_duration.max(timeline_view_start_secs + visible_duration) + workspace_padding_secs;
        state.playhead_secs = state.playhead_secs.clamp(0.0, workspace_duration.max(0.05));
    }

    pub(super) fn trim_timeline_drag_capture_active(&self) -> bool {
        self.pending_sound_drag.is_some()
            && self
                .trim_timeline_state
                .as_ref()
                .is_some_and(|state| state.enabled)
    }

    fn trim_timeline_clip_duration(&self, clip: &TrimTimelineClip) -> f32 {
        (clip.clip_end_secs - clip.clip_start_secs).max(0.05)
    }

    fn trim_timeline_total_duration(&self, state: &TrimTimelineState) -> f32 {
        state
            .rows
            .iter()
            .flat_map(|row| row.clips.iter())
            .map(|clip| clip.start_secs.max(0.0) + self.trim_timeline_clip_duration(clip))
            .fold(0.0f32, f32::max)
            .max(0.25)
    }

    fn trim_timeline_track_waveform(&self, sound: &SoundEffect, max_bars: usize) -> Vec<f32> {
        let waveform = self.sound_waveform_samples(sound);
        let preview = Self::trimmed_waveform_preview_from_samples(sound, &waveform);
        if preview.len() <= max_bars {
            return preview;
        }

        let mut reduced = Vec::with_capacity(max_bars);
        let chunk = (preview.len() as f32 / max_bars as f32).ceil() as usize;
        for slice in preview.chunks(chunk.max(1)) {
            let level = slice.iter().copied().fold(0.0f32, f32::max);
            reduced.push(level);
        }
        reduced
    }

    fn trim_timeline_resolve_row_start(
        row: &TrimTimelineRow,
        moving_clip_id: Option<Uuid>,
        desired_start_secs: f32,
        clip_duration_secs: f32,
    ) -> f32 {
        let mut next_start = desired_start_secs.max(0.0);
        let clip_duration = clip_duration_secs.max(0.05);
        let mut sorted_clips = row
            .clips
            .iter()
            .filter(|clip| Some(clip.id) != moving_clip_id)
            .collect::<Vec<_>>();
        sorted_clips.sort_by(|left, right| left.start_secs.total_cmp(&right.start_secs));

        for other in sorted_clips {
            let other_start = other.start_secs.max(0.0);
            let other_end = other_start + (other.clip_end_secs - other.clip_start_secs).max(0.05);
            let next_end = next_start + clip_duration;
            if next_end <= other_start + 0.000_5 {
                break;
            }
            if next_start < other_end && next_end > other_start {
                next_start = other_end;
            }
        }

        next_start.max(0.0)
    }

    fn trim_timeline_collect_snap_points(
        rows: &[TrimTimelineRow],
        ignored_clip_id: Option<Uuid>,
    ) -> Vec<f32> {
        let mut snap_points = vec![0.0];
        for row in rows {
            for existing in &row.clips {
                if Some(existing.id) == ignored_clip_id {
                    continue;
                }
                let start = existing.start_secs.max(0.0);
                let end =
                    start + (existing.clip_end_secs - existing.clip_start_secs).max(0.05);
                snap_points.push(start);
                snap_points.push(end);
            }
        }
        snap_points
    }

    fn trim_timeline_snap_start(
        desired_start_secs: f32,
        clip_duration_secs: f32,
        snap_points: &[f32],
        snap_enabled: bool,
        snap_threshold_secs: f32,
    ) -> (f32, Option<f32>) {
        let next_start = desired_start_secs.max(0.0);
        if !snap_enabled {
            return (next_start, None);
        }

        let clip_duration = clip_duration_secs.max(0.05);
        let mut best: Option<(f32, f32, f32)> = None;
        for snap_point in snap_points.iter().copied() {
            for candidate_start in [snap_point, (snap_point - clip_duration).max(0.0)] {
                let distance = (candidate_start - next_start).abs();
                if best.is_none_or(|(best_distance, _, _)| distance < best_distance) {
                    best = Some((distance, candidate_start, snap_point));
                }
            }
        }

        if let Some((distance, snapped_start, snapped_point)) = best
            && distance <= snap_threshold_secs.max(0.05)
        {
            return (snapped_start.max(0.0), Some(snapped_point));
        }

        (next_start, None)
    }

    fn trim_timeline_preview_drop_start(
        rows: &[TrimTimelineRow],
        row: &TrimTimelineRow,
        desired_start_secs: f32,
        clip_duration_secs: f32,
        snap_enabled: bool,
        snap_threshold_secs: f32,
    ) -> (f32, Option<f32>) {
        let clip_duration = clip_duration_secs.max(0.05);
        let snap_points = Self::trim_timeline_collect_snap_points(rows, None);
        let (snapped_start, snapped_point) = Self::trim_timeline_snap_start(
            desired_start_secs,
            clip_duration,
            &snap_points,
            snap_enabled,
            snap_threshold_secs,
        );
        (
            Self::trim_timeline_resolve_row_start(row, None, snapped_start, clip_duration),
            snapped_point,
        )
    }

    fn collect_trim_timeline_render_clips(
        &self,
        sound_id: Uuid,
    ) -> Option<Vec<(SoundEffect, f32, f32, f32)>> {
        let state = self.trim_timeline_state.as_ref()?;
        if !state.enabled || state.sound_id != sound_id {
            return None;
        }

        let clips = state
            .rows
            .iter()
            .flat_map(|row| row.clips.iter())
            .filter_map(|clip| {
                self.sounds
                    .iter()
                    .find(|sound| sound.id == clip.source_sound_id)
                    .cloned()
                    .map(|sound| {
                        (
                            sound,
                            clip.start_secs.max(0.0),
                            clip.clip_start_secs.max(0.0),
                            clip.clip_end_secs.max(clip.clip_start_secs + 0.05),
                        )
                    })
            })
            .collect::<Vec<_>>();
        (!clips.is_empty()).then_some(clips)
    }

    pub(super) fn finalize_pending_trim_timeline_drop(&mut self) -> bool {
        let Some(drag_sound_id) = self.pending_sound_drag else {
            self.trim_timeline_drop_target = None;
            return false;
        };
        let Some(target) = self.trim_timeline_drop_target.take() else {
            return false;
        };
        let Some(selected_sound_id) = self.selected else {
            return false;
        };
        if self
            .sounds
            .iter()
            .all(|sound| sound.id != drag_sound_id)
        {
            return false;
        }

        self.sync_trim_timeline_state_for(selected_sound_id);
        let Some(state) = self.trim_timeline_state.as_mut() else {
            return false;
        };
        if !state.enabled || state.sound_id != selected_sound_id || target.row_index >= state.rows.len() {
            return false;
        }
        let clip_end_secs = self
            .sounds
            .iter()
            .find(|sound| sound.id == drag_sound_id)
            .map(SoundEffect::trimmed_length)
            .unwrap_or(0.25);

        let inserted_clip_id = Uuid::new_v4();
        state.rows[target.row_index].clips.push(TrimTimelineClip {
            id: inserted_clip_id,
            source_sound_id: drag_sound_id,
            start_secs: target.start_secs.max(0.0),
            clip_start_secs: 0.0,
            clip_end_secs,
        });
        let resolved_start = Self::trim_timeline_resolve_row_start(
            &state.rows[target.row_index],
            Some(inserted_clip_id),
            target.start_secs.max(0.0),
            clip_end_secs,
        );
        if let Some(clip) = state.rows[target.row_index].clips.last_mut() {
            clip.start_secs = resolved_start;
        }
        state.rows[target.row_index]
            .clips
            .sort_by(|left, right| left.start_secs.total_cmp(&right.start_secs));
        state.selected_clip_id = state.rows[target.row_index]
            .clips
            .last()
            .map(|clip| clip.id);
        self.pending_sound_drag = None;
        self.refresh_trim_timeline_preview_after_edit(selected_sound_id);
        true
    }

    pub(super) fn timeline_mode_active_for_selected(&self) -> Option<Uuid> {
        let sound_id = self.selected?;
        self.trim_timeline_state
            .as_ref()
            .filter(|state| state.enabled && state.sound_id == sound_id)
            .map(|state| state.sound_id)
    }

    fn push_trim_timeline_undo_snapshot(&mut self, before: TrimTimelineState) {
        if self.trim_timeline_state.as_ref() == Some(&before) {
            return;
        }
        self.trim_timeline_undo_stack.push(before);
        if self.trim_timeline_undo_stack.len() > TRIM_HISTORY_LIMIT {
            self.trim_timeline_undo_stack.remove(0);
        }
        self.trim_timeline_redo_stack.clear();
    }

    fn preview_timeline_mix_from_position(&mut self, sound_id: Uuid, start_secs: f32) {
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };
        let Some(clips) = self.collect_trim_timeline_render_clips(sound_id) else {
            let sound = self.sounds[index].clone();
            self.preview_sound_from_position(sound_id, Some(start_secs.clamp(0.0, sound.trimmed_length())));
            return;
        };
        let sound = self.sounds[index].clone();
        let preview_path = match Storage::export_timeline_mix_preview_at(
            self.storage.root_dir(),
            &sound,
            &clips,
        ) {
            Ok(path) => path,
            Err(error) => {
                self.set_error_status(error);
                return;
            }
        };
        let Some(audio) = self.audio.as_mut() else {
            self.set_error_status("Audio unavailable");
            return;
        };
        audio.evict_cached_audio(&preview_path);
        if let Err(error) = audio.play_file_from(&preview_path, start_secs.max(0.0)) {
            self.set_error_status(error);
            return;
        }
        self.trim_timeline_preview_path = Some(preview_path);
        if let Some(state) = self.trim_timeline_state.as_mut()
            && state.sound_id == sound_id
        {
            state.playhead_secs = start_secs.max(0.0);
        }
    }

    fn set_trim_timeline_playhead(&mut self, sound_id: Uuid, playhead_secs: f32) {
        let secs = playhead_secs.max(0.0);
        if let Some(state) = self.trim_timeline_state.as_mut()
            && state.sound_id == sound_id
        {
            state.playhead_secs = secs;
        }

        let preview_active = self
            .audio
            .as_ref()
            .is_some_and(|audio| {
                self.trim_timeline_preview_path
                    .as_ref()
                    .is_some_and(|path| audio.is_playing_file(path))
            });
        if !preview_active {
            return;
        }

        let was_paused = self.audio.as_ref().is_some_and(|audio| audio.is_paused());
        self.stop_preview();
        self.preview_timeline_mix_from_position(sound_id, secs);
        if was_paused && let Some(audio) = self.audio.as_mut() {
            audio.pause();
        }
    }

    fn sync_trim_timeline_playhead_from_audio(&mut self, sound_id: Uuid) {
        let Some(preview_path) = self.trim_timeline_preview_path.clone() else {
            return;
        };
        let Some(audio) = self.audio.as_ref() else {
            return;
        };
        if !audio.is_playing_file(&preview_path) || audio.is_paused() {
            return;
        }
        let Some(progress) = audio.playback_progress_for_file(&preview_path) else {
            return;
        };
        let Some(total) = self
            .trim_timeline_state
            .as_ref()
            .filter(|state| state.sound_id == sound_id)
            .map(|state| self.trim_timeline_total_duration(state))
        else {
            return;
        };
        if let Some(state) = self.trim_timeline_state.as_mut()
            && state.sound_id == sound_id
        {
            state.playhead_secs = (progress * total).clamp(0.0, total);
        }
    }

    fn refresh_trim_timeline_preview_after_edit(&mut self, sound_id: Uuid) {
        let Some(playhead_secs) = self
            .trim_timeline_state
            .as_ref()
            .filter(|state| state.sound_id == sound_id)
            .map(|state| state.playhead_secs)
        else {
            return;
        };
        let was_active = self
            .audio
            .as_ref()
            .is_some_and(|audio| self.trim_timeline_preview_path.as_ref().is_some_and(|path| audio.is_playing_file(path)));
        let was_paused = self.audio.as_ref().is_some_and(|audio| audio.is_paused());
        if was_active {
            self.stop_preview();
            self.preview_timeline_mix_from_position(sound_id, playhead_secs);
            if was_paused && let Some(audio) = self.audio.as_mut() {
                audio.pause();
            }
        }
    }

    pub(super) fn handle_trim_timeline_hotkeys(&mut self, ctx: &Context) {
        const BASE_VISIBLE_SECS: f32 = 12.0;
        let Some(sound_id) = self.timeline_mode_active_for_selected() else {
            return;
        };
        if self.has_modal_panel() || self.show_record_review_panel || ctx.wants_keyboard_input() {
            return;
        }

        self.sync_trim_timeline_playhead_from_audio(sound_id);

        let undo_modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let redo_modifiers = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        if ctx.input_mut(|input| input.consume_key(redo_modifiers, egui::Key::Z))
            && let Some(snapshot) = self.trim_timeline_redo_stack.pop()
        {
            if let Some(current) = self.trim_timeline_state.clone() {
                self.trim_timeline_undo_stack.push(current);
            }
            self.trim_timeline_state = Some(snapshot);
            ctx.request_repaint();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(undo_modifiers, egui::Key::Z))
            && let Some(snapshot) = self.trim_timeline_undo_stack.pop()
        {
            if let Some(current) = self.trim_timeline_state.clone() {
                self.trim_timeline_redo_stack.push(current);
            }
            self.trim_timeline_state = Some(snapshot);
            ctx.request_repaint();
            return;
        }

        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            let is_playing = self
                .trim_timeline_preview_path
                .as_ref()
                .is_some_and(|path| {
                    self.audio
                        .as_ref()
                        .is_some_and(|audio| audio.is_playing_file(path))
                });
            let is_paused = self.audio.as_ref().is_some_and(|audio| audio.is_paused());
            if is_playing && !is_paused {
                if let Some(audio) = self.audio.as_mut() {
                    audio.pause();
                }
            } else if is_playing && is_paused {
                if let Some(audio) = self.audio.as_mut() {
                    audio.resume();
                }
            } else if let Some(state) = self.trim_timeline_state.as_ref() {
                self.preview_timeline_mix_from_position(sound_id, state.playhead_secs);
            }
            ctx.request_repaint();
        }

        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::S)) {
            if let Some(state) = self.trim_timeline_state.as_mut() {
                state.playhead_secs = 0.0;
            }
            self.preview_timeline_mix_from_position(sound_id, 0.0);
            ctx.request_repaint();
        }

        let visible_duration = (BASE_VISIBLE_SECS / self.trim_timeline_zoom.max(0.1)).max(0.25);
        let move_step = ctx.input(|input| {
            (input.stable_dt * visible_duration * 2.2).clamp(0.06, visible_duration * 0.24)
        });
        let pan_left = ctx.input(|input| input.key_down(egui::Key::A));
        let pan_right = ctx.input(|input| input.key_down(egui::Key::D));
        if pan_left ^ pan_right {
            let delta = if pan_left { -move_step } else { move_step };
            self.trim_timeline_view_start_secs =
                (self.trim_timeline_view_start_secs + delta).max(0.0);
            ctx.request_repaint();
        }
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
        let timeline_clips = self.collect_trim_timeline_render_clips(sound_id);
        let root_dir = self.storage.root_dir().to_path_buf();
        let tx = self.trim_commit_tx.clone();
        self.trim_commit_inflight.insert(sound_id);
        thread::spawn(move || {
            let result = if let Some(timeline_clips) = timeline_clips {
                Storage::commit_timeline_mix_at(&root_dir, &sound, &timeline_clips, keep_old)
            } else if keep_old {
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
            kind: platform::DragGhostKind::Sound,
            waveform: Self::trimmed_waveform_preview_from_samples(sound, &drag_waveform),
            dark_theme: self.dark_theme,
        };
        let result = platform::drag_file_out(&drag_path, Some(&drag_ghost));
        ctx.request_repaint();
        result
    }

    pub(super) fn drag_folder_out(&mut self, folder_id: Uuid) -> Result<()> {
        let drag_path =
            self.storage
                .drag_folder_source_path(folder_id, &self.folders, &self.sounds)?;
        self.ignored_drop_path =
            Some(fs::canonicalize(&drag_path).unwrap_or_else(|_| drag_path.clone()));
        self.pending_folder_drag = None;
        let drag_ghost = platform::DragGhostSpec {
            kind: platform::DragGhostKind::Folder,
            waveform: self.folder_drag_ghost_waveform(folder_id),
            dark_theme: self.dark_theme,
        };
        platform::drag_file_out(&drag_path, Some(&drag_ghost))
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

    pub(super) fn clipboard_file_paths(&self) -> Result<Vec<PathBuf>> {
        #[cfg(windows)]
        {
            let _clipboard =
                Clipboard::new_attempts(10).context("unable to open system clipboard")?;
            let mut clipboard_paths = Vec::<PathBuf>::new();
            FileList
                .read_clipboard(&mut clipboard_paths)
                .context("unable to read files from clipboard")?;
            return Ok(clipboard_paths);
        }

        #[cfg(not(windows))]
        {
            anyhow::bail!("Clipboard file access is only available on Windows");
        }
    }

    pub(super) fn clipboard_has_supported_audio(&self) -> bool {
        self.clipboard_file_paths()
            .map(|paths| {
                paths
                    .iter()
                    .any(|path| Self::path_contains_supported_audio(path))
            })
            .unwrap_or(false)
    }

    pub(super) fn paste_clipboard_sounds_to_folder(
        &mut self,
        folder_id: Uuid,
        ctx: &Context,
    ) -> Result<usize> {
        #[cfg(windows)]
        {
            let clipboard_paths = self.clipboard_file_paths()?;
            if clipboard_paths.is_empty() {
                anyhow::bail!("Clipboard does not contain any files");
            }
            let imported_count = self.import_paths_to_folder(clipboard_paths, Some(folder_id))?;
            if imported_count == 0 {
                anyhow::bail!("Clipboard has no supported audio files");
            }
            let _ = ctx;
            return Ok(imported_count);
        }

        #[cfg(not(windows))]
        {
            let _ = folder_id;
            let _ = ctx;
            anyhow::bail!("Clipboard paste is only available on Windows");
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
        if self
            .trim_timeline_preview_path
            .as_ref()
            .is_some_and(|path| audio.is_playing_file(path) && !audio.is_paused())
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
        if self.timeline_mode_active_for_selected().is_some() {
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
        let path = path.to_path_buf();
        self.stop_preview();
        self.show_record_panel = false;
        self.show_record_review_panel = false;
        self.app_view = AppView::Editor;
        self.recording_review_pending_path = Some(path.clone());
        self.status = Some("Preparing recorded audio...".to_owned());

        let tx = self.recording_review_tx.clone();
        thread::spawn(move || {
            let result = Storage::new()
                .map_err(|error| error.to_string())
                .and_then(|storage| {
                    storage
                        .analyze_sound_as_effect(&path, &name)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(RecordingReviewMessage::Finished { path, result });
        });
    }

    pub(super) fn poll_recording_review_jobs(&mut self, ctx: &Context) {
        while let Ok(message) = self.recording_review_rx.try_recv() {
            match message {
                RecordingReviewMessage::Finished { path, result } => {
                    let is_current = self
                        .recording_review_pending_path
                        .as_ref()
                        .is_some_and(|pending| pending == &path);
                    if !is_current {
                        let _ = fs::remove_file(path);
                        continue;
                    }

                    self.recording_review_pending_path = None;
                    match result {
                        Ok(mut sound) => {
                            sound.waveform = Self::center_waveform_visual(&sound.waveform);
                            let preview_id = sound.id;
                            let preview_start = sound.trim_start_secs;
                            let preview_duration = sound.safe_duration();
                            self.trim_timeline_zoom = 1.0;
                            self.recording_draft = Some(RecordingDraft {
                                mode: RecordingDraftMode::Recording,
                                sound,
                                source_path: path,
                                source_is_temporary: true,
                                keep_vocal: false,
                                vocal_separated_path: None,
                                keep_music: false,
                                music_separated_path: None,
                            });
                            self.show_record_review_panel = true;
                            self.set_preview_cursor_secs(
                                preview_id,
                                preview_start,
                                preview_duration,
                            );
                            self.preview_recording_draft_from_position(Some(preview_start));
                            self.clear_status();
                        }
                        Err(error) => {
                            let _ = fs::remove_file(path);
                            self.set_error_status(error);
                        }
                    }
                    ctx.request_repaint();
                }
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
        let is_library_sounds =
            self.app_view == AppView::Library && self.library_tab == LibraryTab::Sounds;
        if (self.app_view != AppView::Editor && !is_library_sounds) || self.has_modal_panel() {
            self.editor_drop_armed = false;
            self.editor_drop_rect = None;
            return;
        }

        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let pointer_pos = self.external_drop_pointer_pos(ctx);
        let library_drop_target = if is_library_sounds {
            pointer_pos.and_then(|pos| self.resolve_library_drop_target_at(pos))
        } else {
            None
        };
        let dropped_in_rect = if is_library_sounds {
            library_drop_target.is_some()
        } else {
            self.editor_drop_rect
                .is_some_and(|rect| pointer_pos.is_some_and(|pos| rect.contains(pos)))
        };
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
            let target_folder_id = if is_library_sounds {
                library_drop_target.flatten()
            } else {
                None
            };
            if is_library_sounds {
                self.begin_import_paths_to_folder(paths, target_folder_id);
            } else if let Err(error) = self.import_paths_to_folder(paths, target_folder_id) {
                self.set_error_status(error);
            }
            self.library_drop_target_folder = None;
            self.library_drop_target_root = false;
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
                            self.trim_timeline_state = None;
                            self.trim_timeline_drop_target = None;
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
        let draft_mode = self
            .recording_draft
            .as_ref()
            .map(|draft| draft.mode)
            .unwrap_or(RecordingDraftMode::Recording);
        let is_video_export_mode = draft_mode == RecordingDraftMode::VideoExport;
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
        let vocal_unavailable_label = self.t("editor.vocal_unavailable");
        let first_run_slower_label = self.t("editor.first_run_slower");
        let export_label = self.t("editor.export");
        let animation_label = self.t("editor.animation");
        let fps_label = self.t("editor.fps");
        let stop_label = self.t("editor.stop");
        let preview_label = self.t("editor.preview");
        let export_spn_label = self.t("editor.export_spn");
        let save_audio_label = self.t("editor.save_audio");
        let close_label = self.t("editor.close");
        let discard_label = self.t("editor.discard");
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
        let export_progress = if is_video_export_mode {
            self.active_record_video_export
                .as_ref()
                .map(|export| (export.progress, export.stage.clone()))
        } else {
            None
        };
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
                        let (
                            timeline_changed,
                            timeline_seek_request,
                            timeline_preview_commit,
                            _timeline_trim_history_commit,
                        ) = Self::draw_trim_timeline(
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
                                if demucs_available {
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
                                if demucs_available {
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

                            if !demucs_available {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(&vocal_unavailable_label)
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            } else if draft.keep_vocal || draft.keep_music {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(&first_run_slower_label)
                                        .size(11.5)
                                        .color(Self::muted_text_color()),
                                );
                            }
                        });
                    });

                if is_video_export_mode {
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
                                            RichText::new(&export_label)
                                                .size(12.5)
                                                .color(Self::muted_text_color()),
                                        );
                                    },
                                );
                                ui.add_space(8.0);

                                let animation = ui.add_sized(
                                    [108.0, 32.0],
                                    Self::action_button(
                                        RichText::new(&animation_label).size(12.5),
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
                                    self.record_export_video_sharps =
                                        !self.record_export_video_sharps;
                                    ctx.request_repaint();
                                }

                                ui.add_space(10.0);
                                ui.allocate_ui_with_layout(
                                    vec2(24.0, 32.0),
                                    egui::Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(&fps_label)
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
                }

                ui.horizontal_centered(|ui| {
                    let preview = ui.add_sized(
                        [118.0, 38.0],
                        Self::action_button(
                            RichText::new(if is_playing {
                                &stop_label
                            } else {
                                &preview_label
                            })
                            .size(13.0),
                            is_playing,
                            false,
                        ),
                    );
                    Self::decorate_button_response(ui, &preview);
                    if preview.clicked() {
                        preview_toggle = true;
                    }

                    if is_video_export_mode {
                        let video = ui
                            .add_enabled_ui(!exporting_video, |ui| {
                                ui.add_sized(
                                    [132.0, 38.0],
                                    Self::action_button(
                                        RichText::new(&export_spn_label).size(13.0),
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
                    } else {
                        let save = ui.add_sized(
                            [132.0, 38.0],
                            Self::action_button(
                                RichText::new(&save_audio_label).size(13.0),
                                false,
                                false,
                            ),
                        );
                        Self::decorate_button_response(ui, &save);
                        if save.clicked() {
                            save_audio = true;
                        }
                    }

                    let discard = ui
                        .add_enabled_ui(!exporting_video, |ui| {
                            ui.add_sized(
                                [118.0, 38.0],
                                Self::action_button(
                                    RichText::new(if is_video_export_mode {
                                        &close_label
                                    } else {
                                        &discard_label
                                    })
                                    .size(13.0),
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
        if export_video && is_video_export_mode {
            self.export_recording_review_video();
        }
        if save_audio && !is_video_export_mode {
            self.save_recording_review_to_library();
        } else if discard_request {
            self.close_recording_review(true);
        } else if close_request || !open_panel {
            self.hide_recording_review();
        }
    }

    pub(super) fn draw_editor(&mut self, ui: &mut Ui, ctx: &Context) {
        if self.selected_sound_index().is_none() {
            self.draw_empty_editor(ui);
            return;
        }

        self.handle_trim_undo_redo(ctx);
        let Some(index) = self.selected_sound_index() else {
            self.draw_empty_editor(ui);
            return;
        };

        let (sound_id, vocal_job_running, vocal_ready, music_job_running, music_ready) = {
            let sound = &self.sounds[index];
            let vocal_asset_path = sound.vocal_asset_path(self.storage.root_dir());
            let vocal_ready = vocal_asset_path.as_ref().is_some_and(|path| path.exists());
            let music_asset_path = sound.music_asset_path(self.storage.root_dir());
            let music_ready = music_asset_path.as_ref().is_some_and(|path| path.exists());
            let vocal_job_running = self.vocal_separation_running
                && self.vocal_separation_kind == Some(SeparationStemKind::Vocal)
                && matches!(
                    self.vocal_separation_target.as_ref(),
                    Some(VocalSeparationTarget::LibrarySound {
                        sound_id: target_sound_id,
                        ..
                    }) if *target_sound_id == sound.id
                );
            let music_job_running = self.vocal_separation_running
                && self.vocal_separation_kind == Some(SeparationStemKind::Music)
                && matches!(
                    self.vocal_separation_target.as_ref(),
                    Some(VocalSeparationTarget::LibrarySound {
                        sound_id: target_sound_id,
                        ..
                    }) if *target_sound_id == sound.id
                );
            (
                sound.id,
                vocal_job_running,
                vocal_ready,
                music_job_running,
                music_ready,
            )
        };
        let preview_asset_path = self.preview_asset_path_for_sound(&self.sounds[index]);
        let editor_audio_loading = self.audio_preload_inflight.contains(&preview_asset_path)
            || self
                .pending_preview_after_preload
                .is_some_and(|(pending_sound_id, _)| pending_sound_id == sound_id);
        self.schedule_audio_preload(preview_asset_path);
        self.sync_editor_tags_input();
        let waveform_samples = self.sound_waveform_samples(&self.sounds[index]);
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
        let mut preview_cursor_secs = {
            let sound = &self.sounds[index];
            let mut cursor = self.preview_cursor_secs_for(sound);
            if let Some(position_secs) = playback_position_secs
                && !playhead_drag_active
            {
                cursor = position_secs.clamp(0.0, sound.safe_duration());
            }
            cursor
        };

        let mut delete_request = false;
        let mut copy_request = false;
        let mut open_location_request = false;
        let mut commit_trim_request = false;
        let mut open_spn_export_request = false;
        let mut seek_request = false;
        let mut playback_reapply_request = false;
        let mut normalize_request = false;
        let mut start_vocal_job = false;
        let mut start_music_job = false;
        let mut stop_vocal_job = false;
        let mut stop_music_job = false;
        let mut save_mix_request = false;
        let mut toggle_timeline_mix_request = false;
        let mut changed = false;
        let mut processed_export_dirty = false;
        let mut trim_history_commit: Option<TrimSnapshot> = None;
        let mut tags_changed = false;
        let mut vocal_reapply_request = false;
        let mut trim_timeline_zoom = self.trim_timeline_zoom;
        let editor_timeline_interactive = !self.has_modal_panel();
        let normalize_loading = self.normalize_inflight.contains(&sound_id);
        let vocal_only_label = self.t("editor.vocal_only");
        let vocal_separate_label = self.t("editor.vocal_separate");
        let vocal_stop_label = self.t("editor.vocal_stop");
        let vocal_ready_label = self.t("editor.vocal_ready");
        let vocal_loading_label = self.t("editor.vocal_loading");
        let music_only_label = self.t("editor.music_only");
        let music_separate_label = self.t("editor.music_separate");
        let music_ready_label = self.t("editor.music_ready");
        let music_loading_label = self.t("editor.music_loading");
        let music_hint_label = self.t("editor.music_hint");
        let vocal_elapsed_label = self.t("editor.vocal_elapsed");
        let vocal_hint_label = self.t("editor.vocal_hint");
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
        let vocal_elapsed_text = if vocal_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let vocal_last_elapsed_text = self
            .vocal_separation_last_elapsed_for_sound(sound_id, SeparationStemKind::Vocal)
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let music_elapsed_text = if music_job_running {
            self.vocal_separation_elapsed_secs().map(|elapsed_secs| {
                format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs))
            })
        } else {
            None
        };
        let music_last_elapsed_text = self
            .vocal_separation_last_elapsed_for_sound(sound_id, SeparationStemKind::Music)
            .map(|elapsed_secs| format!("{} {}", vocal_elapsed_label, format_time(elapsed_secs)));
        let tags_label = self.t("editor.tags");
        let tags_hint = self.t("editor.tags_hint");
        let tags_available_label = self.t("editor.tags_available");
        let available_tags = self.distinct_sound_tags();
        self.sync_trim_timeline_state_for(sound_id);
        let timeline_mix_enabled = self
            .trim_timeline_state
            .as_ref()
            .is_some_and(|state| state.sound_id == sound_id && state.enabled);

        Frame::new()
            .fill(Self::surface_fill())
            .stroke(Stroke::new(1.0, Self::border_color()))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Self::shadow_color(),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                let sound = &mut self.sounds[index];
                let controls_width = 52.0 + 52.0 + 52.0 + 64.0 + 64.0 + 92.0 + 36.0;
                let row_gap = 8.0;
                let back_button_width = if self.editing_from_folder.is_some() {
                    42.0 + 8.0
                } else {
                    0.0
                };
                let name_width =
                    (ui.available_width() - controls_width - row_gap - back_button_width)
                        .max(120.0);

                ui.horizontal(|ui| {
                    if let Some(folder_id) = self.editing_from_folder {
                        if Self::icon_action(ui, [42.0, 34.0], 0xe5c4, false, false).clicked() {
                            self.app_view = AppView::Library;
                            self.library_tab = LibraryTab::Sounds;
                            self.library_current_folder = Some(folder_id);
                            self.editing_from_folder = None;
                        }
                        ui.add_space(8.0);
                    }
                    let response = Frame::new()
                        .fill(Self::input_fill())
                        .stroke(Stroke::new(1.0, Self::border_color()))
                        .corner_radius(16.0)
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.add_sized(
                                [name_width - 24.0, 26.0],
                                TextEdit::singleline(&mut sound.name)
                                    .frame(false)
                                    .font(egui::TextStyle::Heading)
                                    .desired_width(name_width)
                                    .margin(Vec2::new(0.0, 5.0)),
                            )
                        })
                        .inner;
                    if response.changed() {
                        changed = true;
                    }

                    ui.add_space(row_gap);
                    ui.allocate_ui_with_layout(
                        vec2(controls_width, 34.0),
                        egui::Layout::right_to_left(Align::Center),
                        |ui| {
                            if Self::icon_action(ui, [52.0, 34.0], 0xe872, false, false).clicked() {
                                delete_request = true;
                            }
                            let timeline_button = ui.add_sized(
                                [92.0, 34.0],
                                Self::action_button(
                                    RichText::new("Timeline").size(11.5),
                                    timeline_mix_enabled,
                                    false,
                                ),
                            );
                            Self::decorate_button_response(ui, &timeline_button);
                            if timeline_button.clicked() {
                                toggle_timeline_mix_request = true;
                            }
                            let spn = ui.add_sized(
                                [64.0, 34.0],
                                Self::action_button(RichText::new("SPN").size(12.0), false, false),
                            );
                            Self::decorate_button_response(ui, &spn);
                            if spn.clicked() {
                                open_spn_export_request = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe14e, false, false).clicked() {
                                commit_trim_request = true;
                            }
                            if Self::icon_action(ui, [64.0, 34.0], 0xe14d, false, false).clicked() {
                                copy_request = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe2c8, false, false).clicked() {
                                open_location_request = true;
                            }
                        },
                    );
                });

                ui.add_space(2.0);

                egui::CollapsingHeader::new(
                    RichText::new(&tags_label)
                        .size(11.5)
                        .color(Self::muted_text_color())
                        .strong(),
                )
                .default_open(false)
                .show(ui, |ui| {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(26.0)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    let hint_color = Color32::from_rgba_premultiplied(
                                        Self::muted_text_color().r(),
                                        Self::muted_text_color().g(),
                                        Self::muted_text_color().b(),
                                        128,
                                    );
                                    let response = Frame::new()
                                        .fill(Self::input_fill())
                                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                                        .corner_radius(14.0)
                                        .inner_margin(Margin::symmetric(10, 4))
                                        .show(ui, |ui| {
                                            ui.add_sized(
                                                [ui.available_width(), 24.0],
                                                TextEdit::singleline(&mut self.editor_tags_input)
                                                    .frame(false)
                                                    .hint_text(
                                                        RichText::new(tags_hint.as_str())
                                                            .color(hint_color),
                                                    )
                                                    .desired_width(f32::INFINITY),
                                            )
                                        })
                                        .inner;
                                    if response.changed() {
                                        tags_changed = true;
                                    }
                                });
                                if !available_tags.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new(&tags_available_label)
                                            .size(11.0)
                                            .color(Self::muted_text_color()),
                                    );
                                    if Self::draw_sound_tag_picker(
                                        ui,
                                        &available_tags,
                                        &mut self.editor_tags_input,
                                    ) {
                                        tags_changed = true;
                                    }
                                }
                            });
                        });
                });

                ui.add_space(12.0);

                if timeline_mix_enabled {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(30.0)
                        .inner_margin(Margin::same(22))
                        .show(ui, |ui| {
                            let save_request = self.render_trim_composer(ui, ctx, sound_id);
                            save_mix_request |= save_request;
                        });
                } else {
                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(30.0)
                        .inner_margin(Margin::same(22))
                        .show(ui, |ui| {
                            let (
                                timeline_changed,
                                timeline_seek_request,
                                timeline_preview_commit,
                                timeline_trim_history_commit,
                            ) = Self::draw_trim_timeline(
                                ui,
                                sound,
                                &waveform_samples,
                                &mut preview_cursor_secs,
                                &mut trim_timeline_zoom,
                                !is_playing,
                                editor_timeline_interactive,
                                editor_audio_loading,
                            );
                            changed |= timeline_changed;
                            seek_request |= timeline_seek_request;
                            if timeline_preview_commit {
                                seek_request = true;
                                processed_export_dirty = true;
                            }
                            trim_history_commit = timeline_trim_history_commit;
                        });

                    ui.add_space(18.0);

                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(26.0)
                        .inner_margin(Margin::same(22))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            Self::with_slider_visuals(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(Self::icon(0xe050, 16.0, Self::muted_text_color()));
                                    let (volume_response, volume_slider_changed) =
                                        Self::click_slider_deferred(
                                            ui,
                                            &mut sound.volume,
                                            0.0..=5.0,
                                            0.0,
                                            vec2(128.0, 24.0),
                                        );
                                    let volume_input = ui.add(
                                        DragValue::new(&mut sound.volume)
                                            .range(0.0..=5.0)
                                            .speed(0.01)
                                            .max_decimals(2)
                                            .suffix("x"),
                                    );
                                    sound.volume = sound.volume.clamp(0.0, 5.0);
                                    ui.add_space(8.0);
                                    let normalize_response = ui.add_enabled(
                                        !normalize_loading,
                                        Button::new(
                                            RichText::new("Normalize")
                                                .size(11.0)
                                                .color(Color32::from_rgb(214, 51, 132)),
                                        )
                                        .fill(Self::surface_fill())
                                        .stroke(Stroke::new(1.0, Self::border_color()))
                                        .corner_radius(12.0),
                                    );
                                    if normalize_response
                                        .on_hover_text(
                                            "Automatically adjust volume to a standard listening level",
                                        )
                                        .clicked()
                                    {
                                        normalize_request = true;
                                    }
                                    if normalize_loading {
                                        ui.add_space(6.0);
                                        ui.add(egui::Spinner::new().size(16.0));
                                    }
                                    ui.add_space(10.0);
                                    ui.label(Self::icon(0xe9e4, 16.0, Self::muted_text_color()));
                                    let (speed_response, speed_slider_changed) =
                                        Self::click_slider_deferred(
                                            ui,
                                            &mut sound.speed,
                                            0.25..=2.0,
                                            0.0,
                                            vec2(128.0, 24.0),
                                        );
                                    let speed_input = ui.add(
                                        DragValue::new(&mut sound.speed)
                                            .range(0.25..=2.0)
                                            .speed(0.01)
                                            .max_decimals(2)
                                            .suffix("x"),
                                    );
                                    sound.speed = sound.speed.clamp(0.25, 2.0);
                                    let volume_input_commit =
                                        Self::deferred_drag_value_commit(ui.ctx(), &volume_input);
                                    let speed_input_commit =
                                        Self::deferred_drag_value_commit(ui.ctx(), &speed_input);
                                    if volume_response.changed()
                                        || speed_response.changed()
                                        || volume_input.changed()
                                        || speed_input.changed()
                                        || volume_slider_changed
                                        || speed_slider_changed
                                    {
                                        changed = true;
                                        processed_export_dirty = true;
                                    }
                                    if volume_slider_changed
                                        || speed_slider_changed
                                        || volume_input_commit
                                        || speed_input_commit
                                    {
                                        playback_reapply_request = true;
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
                                                sound.reverb_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_reverb_hint);
                                    Self::decorate_button_response(ui, &reverb);
                                    if reverb.clicked() {
                                        sound.reverb_enabled = !sound.reverb_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }

                                    let telephone = ui
                                        .add_sized(
                                            [98.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_telephone_label).size(11.5),
                                                sound.telephone_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_telephone_hint);
                                    Self::decorate_button_response(ui, &telephone);
                                    if telephone.clicked() {
                                        sound.telephone_enabled = !sound.telephone_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }

                                    let distortion = ui
                                        .add_sized(
                                            [98.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_distortion_label).size(11.5),
                                                sound.distortion_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_distortion_hint);
                                    Self::decorate_button_response(ui, &distortion);
                                    if distortion.clicked() {
                                        sound.distortion_enabled = !sound.distortion_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }

                                    let echo = ui
                                        .add_sized(
                                            [78.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_echo_label).size(11.5),
                                                sound.echo_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_echo_hint);
                                    Self::decorate_button_response(ui, &echo);
                                    if echo.clicked() {
                                        sound.echo_enabled = !sound.echo_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.add_space(54.0);
                                    let underwater = ui
                                        .add_sized(
                                            [98.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_underwater_label).size(11.5),
                                                sound.underwater_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_underwater_hint);
                                    Self::decorate_button_response(ui, &underwater);
                                    if underwater.clicked() {
                                        sound.underwater_enabled = !sound.underwater_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }

                                    let robot = ui
                                        .add_sized(
                                            [78.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_robot_label).size(11.5),
                                                sound.robot_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_robot_hint);
                                    Self::decorate_button_response(ui, &robot);
                                    if robot.clicked() {
                                        sound.robot_enabled = !sound.robot_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }

                                    let pitch_shift = ui
                                        .add_sized(
                                            [98.0, 30.0],
                                            Self::action_button(
                                                RichText::new(&effect_pitch_shift_label).size(11.5),
                                                sound.pitch_shift_enabled,
                                                false,
                                            ),
                                        )
                                        .on_hover_text(&effect_pitch_shift_hint);
                                    Self::decorate_button_response(ui, &pitch_shift);
                                    if pitch_shift.clicked() {
                                        sound.pitch_shift_enabled = !sound.pitch_shift_enabled;
                                        changed = true;
                                        processed_export_dirty = true;
                                        playback_reapply_request = true;
                                    }
                                    if sound.pitch_shift_enabled {
                                        let semitone_input = ui.add(
                                            DragValue::new(&mut sound.pitch_shift_semitones)
                                                .range(-24.0..=24.0)
                                                .speed(0.1)
                                                .max_decimals(1)
                                                .suffix(" st"),
                                        );
                                        if Self::deferred_drag_value_commit(ctx, &semitone_input) {
                                            changed = true;
                                            processed_export_dirty = true;
                                            playback_reapply_request = true;
                                        }
                                    }
                                });
                            });
                        });

                    ui.add_space(16.0);

                    Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                        .corner_radius(22.0)
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&vocal_only_label)
                                            .size(11.5)
                                            .color(Self::strong_text_color())
                                            .strong(),
                                    );
                                    let vocal_toggle = ui
                                        .add(Checkbox::new(&mut sound.vocal_only, ""))
                                        .on_hover_text(&vocal_hint_label);
                                    if vocal_toggle.changed() {
                                        changed = true;
                                        vocal_reapply_request = true;
                                        if sound.vocal_only {
                                            sound.music_only = false;
                                            if vocal_ready {
                                                vocal_reapply_request = true;
                                            } else if !vocal_job_running {
                                                start_vocal_job = true;
                                            }
                                        } else if vocal_job_running {
                                            stop_vocal_job = true;
                                        }
                                    }

                                    ui.add_space(8.0);

                                    if vocal_job_running {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(&vocal_loading_label)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                        if let Some(vocal_elapsed_text) = &vocal_elapsed_text {
                                            ui.label(
                                                RichText::new(vocal_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                        let stop = ui.add(
                                            Button::new(
                                                RichText::new(&vocal_stop_label)
                                                    .size(10.5)
                                                    .color(Color32::from_rgb(214, 51, 132)),
                                            )
                                            .fill(Self::surface_fill())
                                            .stroke(Stroke::new(1.0, Self::border_color()))
                                            .corner_radius(10.0),
                                        );
                                        if stop.clicked() {
                                            stop_vocal_job = true;
                                        }
                                    } else if vocal_ready {
                                        ui.label(
                                            RichText::new(&vocal_ready_label)
                                                .size(11.0)
                                                .color(Color32::from_rgb(100, 200, 100)),
                                        );
                                        if let Some(vocal_last_elapsed_text) = &vocal_last_elapsed_text
                                        {
                                            ui.label(
                                                RichText::new(vocal_last_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                    } else {
                                        let separate = ui.add(
                                            Button::new(
                                                RichText::new(&vocal_separate_label)
                                                    .size(10.5)
                                                    .color(Color32::from_rgb(214, 51, 132)),
                                            )
                                            .fill(Self::surface_fill())
                                            .stroke(Stroke::new(1.0, Self::border_color()))
                                            .corner_radius(10.0),
                                        );
                                        if separate.clicked() {
                                            if !vocal_job_running {
                                                sound.vocal_only = true;
                                                sound.music_only = false;
                                                changed = true;
                                                start_vocal_job = true;
                                            }
                                        }
                                    }
                                });
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&music_only_label)
                                            .size(11.5)
                                            .color(Self::strong_text_color())
                                            .strong(),
                                    );
                                    let music_toggle = ui
                                        .add(Checkbox::new(&mut sound.music_only, ""))
                                        .on_hover_text(&music_hint_label);
                                    if music_toggle.changed() {
                                        changed = true;
                                        vocal_reapply_request = true;
                                        if sound.music_only {
                                            sound.vocal_only = false;
                                            if music_ready {
                                                vocal_reapply_request = true;
                                            } else if !music_job_running {
                                                start_music_job = true;
                                            }
                                        } else if music_job_running {
                                            stop_music_job = true;
                                        }
                                    }

                                    ui.add_space(8.0);

                                    if music_job_running {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(&music_loading_label)
                                                .size(11.0)
                                                .color(Self::muted_text_color()),
                                        );
                                        if let Some(music_elapsed_text) = &music_elapsed_text {
                                            ui.label(
                                                RichText::new(music_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                        let stop = ui.add(
                                            Button::new(
                                                RichText::new(&vocal_stop_label)
                                                    .size(10.5)
                                                    .color(Color32::from_rgb(214, 51, 132)),
                                            )
                                            .fill(Self::surface_fill())
                                            .stroke(Stroke::new(1.0, Self::border_color()))
                                            .corner_radius(10.0),
                                        );
                                        if stop.clicked() {
                                            stop_music_job = true;
                                        }
                                    } else if music_ready {
                                        ui.label(
                                            RichText::new(&music_ready_label)
                                                .size(11.0)
                                                .color(Color32::from_rgb(100, 200, 100)),
                                        );
                                        if let Some(music_last_elapsed_text) = &music_last_elapsed_text
                                        {
                                            ui.label(
                                                RichText::new(music_last_elapsed_text)
                                                    .size(11.0)
                                                    .color(Self::muted_text_color()),
                                            );
                                        }
                                    } else {
                                        let separate = ui.add(
                                            Button::new(
                                                RichText::new(&music_separate_label)
                                                    .size(10.5)
                                                    .color(Color32::from_rgb(214, 51, 132)),
                                            )
                                            .fill(Self::surface_fill())
                                            .stroke(Stroke::new(1.0, Self::border_color()))
                                            .corner_radius(10.0),
                                        );
                                        if separate.clicked() {
                                            if !music_job_running {
                                                sound.music_only = true;
                                                sound.vocal_only = false;
                                                changed = true;
                                                start_music_job = true;
                                            }
                                        }
                                    }
                                });
                            });
                        });

                    ui.add_space(16.0);

                    let drop_response = Frame::new()
                        .fill(Self::panel_fill())
                        .stroke(Stroke::NONE)
                        .corner_radius(22.0)
                        .inner_margin(Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.set_min_height(76.0);
                            ui.vertical_centered(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new("Drop sound here")
                                        .size(14.5)
                                        .color(Self::strong_text_color())
                                        .strong(),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new("or click to open import browser")
                                        .size(12.5)
                                        .color(Self::muted_text_color()),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());
                    Self::paint_dashed_border(
                        ui.painter(),
                        drop_response.rect.shrink(8.0),
                        Self::subtle_border_color(),
                    );
                    self.editor_drop_rect = Some(drop_response.rect.expand(8.0));
                    if drop_response.hovered()
                        && ui.ctx().input(|input| input.raw.hovered_files.is_empty())
                    {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if drop_response.clicked() {
                        self.add_sound();
                    }
                }
            });

        if toggle_timeline_mix_request {
            self.sync_trim_timeline_state_for(sound_id);
            if let Some(state) = self.trim_timeline_state.as_mut() {
                state.enabled = !state.enabled;
                if !state.enabled {
                    self.trim_timeline_drop_target = None;
                }
            }
        }

        let sound_id = self.sounds[index].id;
        let sound_duration = self.sounds[index].safe_duration();
        if !timeline_mix_enabled {
            self.trim_timeline_zoom = trim_timeline_zoom;
        }
        if let Some(snapshot) = trim_history_commit {
            let current = TrimSnapshot::from_sound(&self.sounds[index]);
            self.push_trim_undo_snapshot(snapshot, current);
        }
        if tags_changed {
            let tags = Self::parse_tags(&self.editor_tags_input);
            self.sounds[index].tags = tags;
            changed = true;
        }
        self.set_preview_cursor_secs(sound_id, preview_cursor_secs, sound_duration);

        if start_vocal_job {
            self.start_library_vocal_separation(sound_id);
        }
        if start_music_job {
            self.start_library_music_separation(sound_id);
        }
        if stop_vocal_job {
            self.stop_vocal_separation();
        }
        if stop_music_job {
            self.stop_vocal_separation();
        }
        if vocal_reapply_request {
            let vocal_preview_path = self.preview_asset_path_for_sound(&self.sounds[index]);
            self.schedule_audio_preload(vocal_preview_path);
        }

        if (seek_request || playback_reapply_request || vocal_reapply_request) && is_playing {
            self.preview_sound_from_position(sound_id, Some(preview_cursor_secs));
        }

        if changed {
            self.mark_dirty(ctx);
        }

        if delete_request {
            self.delete_selected();
            return;
        }

        if copy_request {
            self.copy_selected_processed_sound();
        }

        if normalize_request {
            self.start_normalize_job(sound_id);
        }

        if save_mix_request {
            self.start_trim_commit_job(ctx, true);
        }

        if processed_export_dirty {
            self.schedule_processed_export(sound_id);
        }

        if open_location_request {
            let path = self.sounds[index].asset_path(self.storage.root_dir());
            if let Err(error) = Self::reveal_in_file_explorer(&path) {
                self.set_error_status(error);
            }
        }

        if commit_trim_request {
            self.show_trim_commit_panel = true;
        }
        if open_spn_export_request {
            self.open_selected_sound_for_record_export();
        }
    }

    pub(super) fn draw_empty_editor(&mut self, ui: &mut Ui) {
        Frame::new()
            .fill(Color32::from_rgba_premultiplied(255, 255, 255, 210))
            .stroke(Stroke::new(1.0, Color32::from_rgb(236, 223, 232)))
            .shadow(Shadow {
                offset: [0, 12],
                blur: 28,
                spread: 0,
                color: Color32::from_rgba_premultiplied(84, 48, 69, 16),
            })
            .corner_radius(36.0)
            .inner_margin(Margin::same(28))
            .show(ui, |ui| {
                let height = ui.available_height().clamp(280.0, 420.0);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    self.add_sound();
                }

                let painter = ui.painter_at(rect);
                let center = rect.center();
                painter.text(
                    center,
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::new(52.0, FontFamily::Proportional),
                    Color32::from_rgb(214, 51, 132),
                );
                painter.text(
                    Pos2::new(center.x, center.y + 42.0),
                    Align2::CENTER_CENTER,
                    "Click to import sound",
                    FontId::new(13.5, FontFamily::Proportional),
                    Color32::from_rgb(122, 96, 111),
                );
            });
    }

    pub(super) fn draw_trim_timeline(
        ui: &mut Ui,
        sound: &mut SoundEffect,
        waveform_samples: &[f32],
        preview_cursor_secs: &mut f32,
        zoom: &mut f32,
        clamp_cursor_to_trim: bool,
        interactive: bool,
        show_loading_indicator: bool,
    ) -> (bool, bool, bool, Option<TrimSnapshot>) {
        sound.clamp_trim();
        let duration = sound.display_duration_secs();
        *preview_cursor_secs = if clamp_cursor_to_trim {
            (*preview_cursor_secs).clamp(sound.display_trim_start(), sound.display_trim_end())
        } else {
            (*preview_cursor_secs).clamp(0.0, duration)
        };
        *zoom = (*zoom).clamp(0.1, 8.0);
        let playhead_drag_id = Self::trim_playhead_drag_id(sound.id);

        ui.horizontal(|ui| {
            ui.label(Self::icon(0xe14e, 14.0, Color32::from_rgb(118, 106, 116)));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Trim")
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.add_space(6.0);
            let help = ui.add_sized(
                [24.0, 24.0],
                Button::new(Self::icon(0xe887, 16.0, Color32::from_rgb(214, 51, 132)))
                    .fill(Self::surface_fill())
                    .stroke(Stroke::new(1.0, Self::border_color()))
                    .corner_radius(12.0),
            );
            if help.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
            }
            help.on_hover_ui_at_pointer(|ui| {
                ui.set_max_width(250.0);
                ui.label(
                    RichText::new("Trim shortcuts")
                        .size(13.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label("Space: preview or stop");
                ui.label("S: preview from the left trim");
                ui.label("Q: move the left trim to the mouse");
                ui.label("W: move the right trim to the mouse");
                ui.label("Right click: delete left / middle / right region");
                ui.label("Ctrl + Z: undo trim");
                ui.label("Ctrl + Shift + Z: redo trim");
                ui.label("A / D: pan timeline left or right");
                ui.label("Ctrl + mouse wheel: zoom around the hover playhead");
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{:.1}x", *zoom))
                        .size(12.0)
                        .color(Self::muted_text_color()),
                );
            });
        });
        ui.add_space(8.0);

        let viewport_width = ui.available_width().max(320.0);
        let zoom_scroll_offset_id = egui::Id::new((sound.id, "trim-zoom-offset"));
        let trim_adjusting_id = egui::Id::new((sound.id, "trim-adjusting"));
        let trim_hotkey_adjusting_id = egui::Id::new((sound.id, "trim-hotkey-adjusting"));
        let trim_history_snapshot_id = egui::Id::new((sound.id, "trim-history-snapshot"));
        let stored_scroll_offset = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(zoom_scroll_offset_id));
        let mut requested_scroll_offset: Option<f32> = None;
        let timeline_size = vec2((viewport_width * *zoom).max(viewport_width), 160.0);
        let dark_theme = Self::dark_theme_enabled();
        let mut changed = false;
        let mut seek_requested = false;
        let mut preview_commit_requested = false;
        let mut trim_history_commit = None;

        ui.allocate_ui_with_layout(
            vec2(viewport_width, timeline_size.y + 10.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                let mut scroll_area = ScrollArea::horizontal()
                    .id_salt((sound.id, "trim-timeline-scroll"))
                    .drag_to_scroll(false)
                    .auto_shrink([false, false]);
                if let Some(offset) = stored_scroll_offset {
                    scroll_area = scroll_area.horizontal_scroll_offset(offset);
                }
                let scroll_output = scroll_area.show(ui, |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(timeline_size, Sense::click_and_drag());
                    let viewport_rect = rect.intersect(ui.clip_rect());
                    let painter = ui.painter_at(rect);
                    let timeline_fill = if dark_theme {
                        Color32::from_rgb(11, 10, 14)
                    } else {
                        Color32::from_rgb(255, 255, 255)
                    };
                    let timeline_stroke = if dark_theme {
                        Color32::from_rgb(74, 61, 82)
                    } else {
                        Color32::from_rgb(235, 223, 232)
                    };
                    painter.rect_filled(rect, 18.0, timeline_fill);
                    painter.rect_stroke(
                        rect,
                        18.0,
                        Stroke::new(1.0, timeline_stroke),
                        StrokeKind::Outside,
                    );

                    let has_cutout = sound.has_cutout();
                    let display_trim_start = sound.display_trim_start();
                    let display_trim_end = sound.display_trim_end();
                    let start_t = display_trim_start / duration;
                    let end_t = display_trim_end / duration;
                    let start_x = rect.left() + rect.width() * start_t.clamp(0.0, 1.0);
                    let end_x = rect.left() + rect.width() * end_t.clamp(0.0, 1.0);
                    let display_waveform = if has_cutout {
                        Self::trimmed_waveform_preview_from_samples(sound, waveform_samples)
                    } else {
                        waveform_samples.to_vec()
                    };

                    Self::paint_waveform_bars(
                        &painter,
                        rect.shrink2(vec2(12.0, 14.0)),
                        &display_waveform,
                        start_x,
                        end_x,
                        None,
                    );

                    let selection = Rect::from_min_max(
                        Pos2::new(start_x, rect.top() + 10.0),
                        Pos2::new(end_x.max(start_x + 2.0), rect.bottom() - 10.0),
                    );
                    painter.rect_filled(
                        selection,
                        16.0,
                        if dark_theme {
                            Color32::from_rgba_premultiplied(227, 82, 149, 36)
                        } else {
                            Color32::from_rgba_premultiplied(227, 82, 149, 24)
                        },
                    );

                    let handle_stroke = Stroke::new(2.0, Color32::from_rgb(214, 51, 132));
                    painter.line_segment(
                        [
                            Pos2::new(start_x, rect.top() + 10.0),
                            Pos2::new(start_x, rect.bottom() - 10.0),
                        ],
                        handle_stroke,
                    );
                    painter.line_segment(
                        [
                            Pos2::new(end_x, rect.top() + 10.0),
                            Pos2::new(end_x, rect.bottom() - 10.0),
                        ],
                        handle_stroke,
                    );
                    painter.circle_filled(
                        Pos2::new(start_x, rect.center().y),
                        7.0,
                        Color32::from_rgb(214, 51, 132),
                    );
                    painter.circle_filled(
                        Pos2::new(end_x, rect.center().y),
                        7.0,
                        Color32::from_rgb(214, 51, 132),
                    );

                    let start_handle_rect = Rect::from_center_size(
                        Pos2::new(start_x, rect.center().y),
                        vec2(24.0, rect.height()),
                    );
                    let end_handle_rect = Rect::from_center_size(
                        Pos2::new(end_x, rect.center().y),
                        vec2(24.0, rect.height()),
                    );
                    let start_response = ui.interact(
                        start_handle_rect,
                        ui.make_persistent_id((sound.id, "trim-start")),
                        Sense::click_and_drag(),
                    );
                    let end_response = ui.interact(
                        end_handle_rect,
                        ui.make_persistent_id((sound.id, "trim-end")),
                        Sense::click_and_drag(),
                    );
                    if start_response.hovered() || start_response.dragged() {
                        painter.circle_stroke(
                            Pos2::new(start_x, rect.center().y),
                            11.0,
                            Stroke::new(1.5, Color32::from_rgb(255, 182, 214)),
                        );
                        painter.line_segment(
                            [
                                Pos2::new(start_x, rect.top() + 6.0),
                                Pos2::new(start_x, rect.bottom() - 6.0),
                            ],
                            Stroke::new(3.0, Color32::from_rgba_premultiplied(255, 182, 214, 96)),
                        );
                    }
                    if end_response.hovered() || end_response.dragged() {
                        painter.circle_stroke(
                            Pos2::new(end_x, rect.center().y),
                            11.0,
                            Stroke::new(1.5, Color32::from_rgb(255, 182, 214)),
                        );
                        painter.line_segment(
                            [
                                Pos2::new(end_x, rect.top() + 6.0),
                                Pos2::new(end_x, rect.bottom() - 6.0),
                            ],
                            Stroke::new(3.0, Color32::from_rgba_premultiplied(255, 182, 214, 96)),
                        );
                    }

                    let pointer_pos = interactive
                        .then(|| ui.ctx().input(|input| input.pointer.hover_pos()))
                        .flatten()
                        .filter(|pos| viewport_rect.contains(*pos));
                    #[derive(Clone, Copy, PartialEq, Eq)]
                    enum TrimDeleteRegion {
                        Left,
                        Middle,
                        Right,
                    }
                    let left_region_rect = Rect::from_min_max(
                        Pos2::new(rect.left(), rect.top() + 10.0),
                        Pos2::new(start_x.max(rect.left() + 2.0), rect.bottom() - 10.0),
                    );
                    let middle_region_rect = selection;
                    let right_region_rect = Rect::from_min_max(
                        Pos2::new(end_x.min(rect.right() - 2.0), rect.top() + 10.0),
                        Pos2::new(rect.right(), rect.bottom() - 10.0),
                    );
                    let left_region_enabled = display_trim_start > 0.001;
                    let right_region_enabled = display_trim_end < duration - 0.001;
                    let left_region_response = ui.interact(
                        left_region_rect,
                        ui.make_persistent_id((sound.id, "trim-delete-left")),
                        Sense::click(),
                    );
                    let middle_region_response = ui.interact(
                        middle_region_rect,
                        ui.make_persistent_id((sound.id, "trim-delete-middle")),
                        Sense::click(),
                    );
                    let right_region_response = ui.interact(
                        right_region_rect,
                        ui.make_persistent_id((sound.id, "trim-delete-right")),
                        Sense::click(),
                    );
                    let pointer_time = pointer_pos.map(|pointer| {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        ratio * duration
                    });
                    let hovered_delete_region =
                        if left_region_enabled && left_region_response.hovered() {
                            Some(TrimDeleteRegion::Left)
                        } else if right_region_enabled && right_region_response.hovered() {
                            Some(TrimDeleteRegion::Right)
                        } else if middle_region_response.hovered() {
                            Some(TrimDeleteRegion::Middle)
                        } else {
                            None
                        };
                    if let Some(region) = hovered_delete_region {
                        let region_rect = match region {
                            TrimDeleteRegion::Left => Some(left_region_rect),
                            TrimDeleteRegion::Middle => Some(middle_region_rect),
                            TrimDeleteRegion::Right => Some(right_region_rect),
                        };
                        if let Some(region_rect) = region_rect {
                            let hint_color = if dark_theme {
                                Color32::from_rgba_premultiplied(255, 120, 170, 180)
                            } else {
                                Color32::from_rgba_premultiplied(214, 51, 132, 168)
                            };
                            let top_y = region_rect.top() + 7.0;
                            let bottom_y = region_rect.bottom() - 7.0;
                            let left_x = region_rect.left() + 8.0;
                            let right_x = region_rect.right() - 8.0;
                            painter.line_segment(
                                [Pos2::new(left_x, top_y), Pos2::new(right_x, top_y)],
                                Stroke::new(1.25, hint_color),
                            );
                            painter.line_segment(
                                [Pos2::new(left_x, bottom_y), Pos2::new(right_x, bottom_y)],
                                Stroke::new(1.25, hint_color),
                            );
                            let center = region_rect.center();
                            painter.circle_filled(center, 2.2, hint_color);
                        }
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ContextMenu);
                    }
                    if interactive && (start_response.hovered() || end_response.hovered()) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    let playhead_outline = if dark_theme {
                        Color32::from_rgba_premultiplied(8, 13, 19, 224)
                    } else {
                        Color32::from_rgba_premultiplied(255, 255, 255, 232)
                    };
                    let playhead_color = if dark_theme {
                        Color32::from_rgb(108, 231, 255)
                    } else {
                        Color32::from_rgb(42, 39, 44)
                    };
                    let hover_playhead_color = if dark_theme {
                        Color32::from_rgba_premultiplied(108, 231, 255, 150)
                    } else {
                        Color32::from_rgba_premultiplied(42, 39, 44, 110)
                    };
                    let pan_left = interactive && ui.input(|input| input.key_down(egui::Key::A));
                    let pan_right = interactive && ui.input(|input| input.key_down(egui::Key::D));
                    let keyboard_panning = pan_left ^ pan_right;
                    let timeline_hovered =
                        interactive && (response.hovered() || pointer_pos.is_some());
                    let showing_hover_preview = pointer_pos.is_some()
                        && !keyboard_panning
                        && !start_response.is_pointer_button_down_on()
                        && !end_response.is_pointer_button_down_on()
                        && !response.dragged();

                    if showing_hover_preview && let Some(pointer) = pointer_pos {
                        painter.line_segment(
                            [
                                Pos2::new(pointer.x, rect.top() + 12.0),
                                Pos2::new(pointer.x, rect.bottom() - 12.0),
                            ],
                            Stroke::new(1.0, hover_playhead_color),
                        );
                        painter.circle_filled(
                            Pos2::new(pointer.x, rect.top() + 12.0),
                            4.0,
                            hover_playhead_color,
                        );
                        if let Some(pointer_time) = pointer_time {
                            let text_pos = Pos2::new(
                                (pointer.x + 8.0).clamp(rect.left() + 6.0, rect.right() - 56.0),
                                rect.top() + 12.0,
                            );
                            painter.text(
                                text_pos,
                                egui::Align2::LEFT_TOP,
                                format_time(pointer_time),
                                egui::FontId::proportional(11.5),
                                if dark_theme {
                                    Color32::from_rgb(208, 244, 255)
                                } else {
                                    Color32::from_rgb(42, 39, 44)
                                },
                            );
                        }
                    }

                    let cursor_ratio = (*preview_cursor_secs / duration).clamp(0.0, 1.0);
                    let cursor_x = rect.left() + rect.width() * cursor_ratio;
                    painter.line_segment(
                        [
                            Pos2::new(cursor_x, rect.top() + 8.0),
                            Pos2::new(cursor_x, rect.bottom() - 8.0),
                        ],
                        Stroke::new(4.0, playhead_outline),
                    );
                    painter.line_segment(
                        [
                            Pos2::new(cursor_x, rect.top() + 8.0),
                            Pos2::new(cursor_x, rect.bottom() - 8.0),
                        ],
                        Stroke::new(2.0, playhead_color),
                    );
                    painter.circle_filled(
                        Pos2::new(cursor_x, rect.top() + 10.0),
                        4.5,
                        playhead_color,
                    );

                    if timeline_hovered && keyboard_panning {
                        ui.ctx().memory_mut(|memory| memory.stop_text_input());
                        let pan_speed = (viewport_rect.width() * 2.4).max(420.0);
                        let pan_step =
                            pan_speed * ui.input(|input| input.stable_dt).max(1.0 / 240.0);
                        let max_offset = (rect.width() - viewport_rect.width()).max(0.0);
                        let delta = match (pan_left, pan_right) {
                            (true, false) => -pan_step,
                            (false, true) => pan_step,
                            _ => 0.0,
                        };
                        let current_offset = requested_scroll_offset
                            .unwrap_or_else(|| (viewport_rect.left() - rect.left()).max(0.0));
                        requested_scroll_offset =
                            Some((current_offset + delta).clamp(0.0, max_offset));
                        ui.ctx().request_repaint();
                    }

                    let begin_trim_history = |ctx: &Context, sound: &SoundEffect| {
                        if ctx
                            .data(|data| data.get_temp::<TrimSnapshot>(trim_history_snapshot_id))
                            .is_none()
                        {
                            ctx.data_mut(|data| {
                                data.insert_temp(
                                    trim_history_snapshot_id,
                                    TrimSnapshot::from_sound(sound),
                                );
                            });
                        }
                    };
                    let take_trim_history = |ctx: &Context| {
                        let snapshot = ctx
                            .data(|data| data.get_temp::<TrimSnapshot>(trim_history_snapshot_id));
                        if snapshot.is_some() {
                            ctx.data_mut(|data| {
                                data.remove::<TrimSnapshot>(trim_history_snapshot_id);
                            });
                        }
                        snapshot
                    };

                    if interactive && pointer_pos.is_some() && !ui.ctx().wants_keyboard_input() {
                        let zoom_delta = ui.input(|input| {
                            if input.modifiers.ctrl {
                                input.raw_scroll_delta.y
                            } else {
                                0.0
                            }
                        });
                        if zoom_delta.abs() > 0.0 {
                            let anchor_viewport_x = pointer_pos
                                .map(|pointer| {
                                    (pointer.x - viewport_rect.left())
                                        .clamp(0.0, viewport_rect.width())
                                })
                                .unwrap_or(viewport_rect.width() * cursor_ratio.clamp(0.0, 1.0));
                            let anchor_content_x = pointer_pos
                                .map(|pointer| (pointer.x - rect.left()).clamp(0.0, rect.width()))
                                .unwrap_or((cursor_ratio * rect.width()).clamp(0.0, rect.width()));
                            let factor = if zoom_delta > 0.0 { 1.12 } else { 1.0 / 1.12 };
                            *zoom = (*zoom * factor).clamp(0.1, 8.0);
                            let next_timeline_width = (viewport_width * *zoom).max(viewport_width);
                            let next_anchor_content_x =
                                (anchor_content_x / rect.width().max(1.0)) * next_timeline_width;
                            let max_offset = (next_timeline_width - viewport_width).max(0.0);
                            requested_scroll_offset = Some(
                                (next_anchor_content_x - anchor_viewport_x).clamp(0.0, max_offset),
                            );
                            ui.ctx().request_repaint();
                        }

                        let move_left = ui.input(|input| input.key_down(egui::Key::Q));
                        let move_right = ui.input(|input| input.key_down(egui::Key::W));

                        if let Some(pointer_time) = pointer_time {
                            if move_left {
                                begin_trim_history(ui.ctx(), sound);
                                sound.clear_cutout();
                                sound.trim_start_secs =
                                    pointer_time.min(sound.trim_end_secs - 0.05);
                                sound.display_trim_start_secs = None;
                                sound.display_trim_end_secs = None;
                                sound.clamp_trim();
                                changed = true;
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(trim_hotkey_adjusting_id, true)
                                });
                            }
                            if move_right {
                                begin_trim_history(ui.ctx(), sound);
                                sound.clear_cutout();
                                sound.trim_end_secs =
                                    pointer_time.max(sound.trim_start_secs + 0.05);
                                sound.display_trim_start_secs = None;
                                sound.display_trim_end_secs = None;
                                sound.clamp_trim();
                                changed = true;
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(trim_hotkey_adjusting_id, true)
                                });
                            }
                        }

                        if !move_left
                            && !move_right
                            && ui
                                .ctx()
                                .data(|data| data.get_temp::<bool>(trim_hotkey_adjusting_id))
                                .unwrap_or(false)
                        {
                            preview_commit_requested = true;
                            trim_history_commit = take_trim_history(ui.ctx());
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_hotkey_adjusting_id));
                        }
                    }

                    let delete_region = if interactive
                        && left_region_enabled
                        && left_region_response.secondary_clicked()
                    {
                        Some(TrimDeleteRegion::Left)
                    } else if interactive && middle_region_response.secondary_clicked() {
                        Some(TrimDeleteRegion::Middle)
                    } else if interactive
                        && right_region_enabled
                        && right_region_response.secondary_clicked()
                    {
                        Some(TrimDeleteRegion::Right)
                    } else {
                        None
                    };
                    if let Some(region) = delete_region {
                        let before = TrimSnapshot::from_sound(sound);
                        let current_trim_start = sound.trim_start_secs;
                        let current_trim_end = sound.trim_end_secs;
                        let current_display_start = sound.display_trim_start();
                        let current_display_end = sound.display_trim_end();
                        let source_duration = sound.safe_duration();
                        match region {
                            TrimDeleteRegion::Left => {
                                sound.trim_start_secs = 0.0;
                                sound.trim_end_secs = source_duration;
                                sound.cut_start_secs = Some(0.0);
                                sound.cut_end_secs = Some(current_trim_start.max(0.05));
                                sound.display_trim_start_secs = Some(0.0);
                                sound.display_trim_end_secs =
                                    Some((current_display_end - current_display_start).max(0.05));
                            }
                            TrimDeleteRegion::Middle => {
                                sound.trim_start_secs = 0.0;
                                sound.trim_end_secs = source_duration;
                                sound.cut_start_secs = Some(current_trim_start);
                                sound.cut_end_secs = Some(current_trim_end);
                                let remaining_duration =
                                    current_trim_start + (source_duration - current_trim_end);
                                let join =
                                    current_trim_start.min((remaining_duration - 0.05).max(0.0));
                                sound.display_trim_start_secs = Some(join);
                                sound.display_trim_end_secs =
                                    Some((join + 0.05).min(remaining_duration.max(0.05)));
                            }
                            TrimDeleteRegion::Right => {
                                sound.trim_start_secs = 0.0;
                                sound.trim_end_secs = source_duration;
                                sound.cut_start_secs =
                                    Some(current_trim_end.min(source_duration - 0.05));
                                sound.cut_end_secs = Some(source_duration);
                                sound.display_trim_start_secs = Some(current_display_start);
                                sound.display_trim_end_secs = Some(current_display_end);
                            }
                        }
                        sound.clamp_trim();
                        changed = true;
                        preview_commit_requested = true;
                        trim_history_commit = Some(before);
                        *preview_cursor_secs = sound.display_trim_start();
                        ui.ctx().data_mut(|data| {
                            data.remove::<TrimSnapshot>(trim_history_snapshot_id);
                            data.remove::<bool>(trim_adjusting_id);
                            data.remove::<bool>(trim_hotkey_adjusting_id);
                        });
                    }

                    if interactive
                        && duration > 0.0
                        && let Some(pointer) = start_response.interact_pointer_pos()
                        && (start_response.clicked() || start_response.dragged())
                    {
                        begin_trim_history(ui.ctx(), sound);
                        sound.clear_cutout();
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        let next = ratio * duration;
                        sound.trim_start_secs = next.min(sound.trim_end_secs - 0.05);
                        sound.display_trim_start_secs = None;
                        sound.display_trim_end_secs = None;
                        sound.clamp_trim();
                        changed = true;
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(trim_adjusting_id, true));
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if start_response.clicked() {
                            preview_commit_requested = true;
                            trim_history_commit = take_trim_history(ui.ctx());
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    } else if interactive
                        && duration > 0.0
                        && let Some(pointer) = end_response.interact_pointer_pos()
                        && (end_response.clicked() || end_response.dragged())
                    {
                        begin_trim_history(ui.ctx(), sound);
                        sound.clear_cutout();
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        let next = ratio * duration;
                        sound.trim_end_secs = next.max(sound.trim_start_secs + 0.05);
                        sound.display_trim_start_secs = None;
                        sound.display_trim_end_secs = None;
                        sound.clamp_trim();
                        changed = true;
                        ui.ctx()
                            .data_mut(|data| data.insert_temp(trim_adjusting_id, true));
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if end_response.clicked() {
                            preview_commit_requested = true;
                            trim_history_commit = take_trim_history(ui.ctx());
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    } else if interactive
                        && !start_response.is_pointer_button_down_on()
                        && !end_response.is_pointer_button_down_on()
                        && duration > 0.0
                        && let Some(pointer) = response.interact_pointer_pos()
                        && (response.clicked() || response.dragged())
                    {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        *preview_cursor_secs = (ratio * duration)
                            .clamp(sound.display_trim_start(), sound.display_trim_end());
                        if response.clicked() {
                            seek_requested = true;
                        }
                        if response.dragged() {
                            ui.ctx()
                                .data_mut(|data| data.insert_temp(playhead_drag_id, true));
                        }
                    }

                    if interactive
                        && response.drag_stopped()
                        && ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(playhead_drag_id))
                            .unwrap_or(false)
                    {
                        seek_requested = true;
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                    }

                    if interactive
                        && (start_response.drag_stopped() || end_response.drag_stopped())
                        && ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                            .unwrap_or(false)
                    {
                        preview_commit_requested = true;
                        trim_history_commit = take_trim_history(ui.ctx());
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                    }

                    if !interactive || !ui.input(|input| input.pointer.primary_down()) {
                        ui.ctx()
                            .data_mut(|data| data.remove::<bool>(playhead_drag_id));
                        if ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                            .unwrap_or(false)
                        {
                            preview_commit_requested = true;
                            trim_history_commit = take_trim_history(ui.ctx());
                            ui.ctx()
                                .data_mut(|data| data.remove::<bool>(trim_adjusting_id));
                        }
                    }

                    let trim_adjusting_active = ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(trim_adjusting_id))
                        .unwrap_or(false)
                        || ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(trim_hotkey_adjusting_id))
                            .unwrap_or(false);

                    if clamp_cursor_to_trim {
                        let clamped_cursor = (*preview_cursor_secs)
                            .clamp(sound.display_trim_start(), sound.display_trim_end());
                        if (clamped_cursor - *preview_cursor_secs).abs() > f32::EPSILON {
                            *preview_cursor_secs = clamped_cursor;
                            if trim_adjusting_active {
                                preview_commit_requested = true;
                                if trim_history_commit.is_none() {
                                    trim_history_commit = take_trim_history(ui.ctx());
                                }
                            } else {
                                seek_requested = true;
                            }
                        }
                    }

                    if trim_history_commit.is_none()
                        && !ui.input(|input| {
                            input.pointer.primary_down()
                                || input.key_down(egui::Key::Q)
                                || input.key_down(egui::Key::W)
                        })
                    {
                        ui.ctx().data_mut(|data| {
                            data.remove::<TrimSnapshot>(trim_history_snapshot_id);
                        });
                    }
                });
                let scroll_offset = requested_scroll_offset
                    .unwrap_or_else(|| scroll_output.state.offset.x.max(0.0));
                ui.ctx().data_mut(|data| {
                    data.insert_temp(zoom_scroll_offset_id, scroll_offset);
                });
            },
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if show_loading_indicator {
                ui.add(egui::Spinner::new().size(16.0));
                ui.add_space(8.0);
            }
            ui.label(
                RichText::new(format_time(sound.display_trim_start()))
                    .size(13.0)
                    .color(if dark_theme {
                        Color32::from_rgb(208, 196, 207)
                    } else {
                        Color32::from_rgb(118, 106, 116)
                    }),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.trimmed_length()))
                    .size(13.0)
                    .color(Color32::from_rgb(214, 51, 132)),
            );
            ui.separator();
            ui.label(
                RichText::new(format_time(sound.display_trim_end()))
                    .size(13.0)
                    .color(if dark_theme {
                        Color32::from_rgb(208, 196, 207)
                    } else {
                        Color32::from_rgb(118, 106, 116)
                    }),
            );
        });

        (
            changed,
            seek_requested,
            preview_commit_requested,
            trim_history_commit,
        )
    }

    fn render_trim_composer(
        &mut self,
        ui: &mut Ui,
        ctx: &Context,
        sound_id: Uuid,
    ) -> bool {
        self.sync_trim_timeline_state_for(sound_id);
        let Some(state_snapshot) = self.trim_timeline_state.as_ref().cloned() else {
            return false;
        };

        let next_enabled = state_snapshot.enabled;
        let mut add_row = false;
        let mut reset_rows = false;
        let mut remove_row = None;
        let mut remove_clip = None;
        let mut save_request = false;
        let mut timeline_state_changed = false;
        let mut zoom = self.trim_timeline_zoom;

        if !next_enabled {
            self.trim_timeline_drop_target = None;
            if let Some(state) = self.trim_timeline_state.as_mut() {
                state.enabled = false;
            }
            return false;
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Timeline mix")
                    .size(13.0)
                    .color(Self::strong_text_color())
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let snap_enabled = state_snapshot.snap_enabled;
                let snap_button = ui.add_sized(
                    [72.0, 30.0],
                    Self::action_button(RichText::new("Snap").size(11.5), snap_enabled, false),
                );
                Self::decorate_button_response(ui, &snap_button);
                if snap_button.clicked()
                    && let Some(state) = self.trim_timeline_state.as_mut()
                {
                    state.snap_enabled = !state.snap_enabled;
                }
                let save_button = ui.add_sized(
                    [82.0, 30.0],
                    Self::action_button(RichText::new("Save").size(11.5), false, false),
                );
                Self::decorate_button_response(ui, &save_button);
                if save_button.clicked() {
                    save_request = true;
                }

                let reset_button = ui.add_sized(
                    [74.0, 30.0],
                    Self::action_button(RichText::new("Reset").size(11.5), false, false),
                );
                Self::decorate_button_response(ui, &reset_button);
                if reset_button.clicked() {
                    reset_rows = true;
                }

                let add_row_button = ui.add_sized(
                    [76.0, 30.0],
                    Self::action_button(RichText::new("+ Row").size(11.5), false, false),
                );
                Self::decorate_button_response(ui, &add_row_button);
                if add_row_button.clicked() {
                    add_row = true;
                }
            });
        });
        ui.add_space(8.0);

        let pending_drag_sound = self
            .pending_sound_drag
            .and_then(|drag_sound_id| self.sounds.iter().find(|sound| sound.id == drag_sound_id))
            .cloned();
        let pending_drag_duration = pending_drag_sound
            .as_ref()
            .map(SoundEffect::trimmed_length)
            .unwrap_or(0.0);
        let timeline_content_duration = (self.trim_timeline_total_duration(&state_snapshot)
            + pending_drag_duration)
            .max(
                self.sounds
                    .iter()
                    .find(|sound| sound.id == sound_id)
                    .map(SoundEffect::trimmed_length)
                    .unwrap_or(0.25),
            );
        let mut next_drop_target = None;
        let base_visible_secs = 12.0f32;

        let row_height = 92.0;
        let row_spacing = 0.0;
        let row_count = state_snapshot.rows.len().max(1);
        let viewport_width = ui.available_width().max(320.0);
        let viewport_height =
            row_count as f32 * row_height + row_count.saturating_sub(1) as f32 * row_spacing;
        let mut view_start_secs = self.trim_timeline_view_start_secs.max(0.0);
        let mut requested_view_start_secs: Option<f32> = None;
        let (viewport_rect, _) =
            ui.allocate_exact_size(vec2(viewport_width, viewport_height.max(row_height)), Sense::hover());
        let viewport_painter = ui.painter().with_clip_rect(viewport_rect);
        viewport_painter.rect_filled(viewport_rect, 18.0, Self::surface_fill());
        viewport_painter.rect_stroke(
            viewport_rect,
            18.0,
            Stroke::new(1.0, Self::subtle_border_color()),
            StrokeKind::Outside,
        );

        if let Some(drag_sound) = pending_drag_sound.as_ref()
            && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
        {
            let overlay_painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("trim-timeline-drag-ghost", sound_id)),
            ));
            let ghost_rect = Rect::from_min_size(
                pointer + vec2(18.0, 18.0),
                vec2(220.0, 54.0),
            );
            overlay_painter.rect_filled(
                ghost_rect,
                16.0,
                Color32::from_rgba_premultiplied(29, 24, 34, 232),
            );
            overlay_painter.rect_stroke(
                ghost_rect,
                16.0,
                Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 210)),
                StrokeKind::Outside,
            );
            overlay_painter.text(
                ghost_rect.left_top() + vec2(12.0, 9.0),
                Align2::LEFT_TOP,
                &drag_sound.name,
                FontId::proportional(12.0),
                Self::strong_text_color(),
            );
            overlay_painter.text(
                ghost_rect.left_bottom() + vec2(12.0, -9.0),
                Align2::LEFT_BOTTOM,
                format_time(drag_sound.trimmed_length()),
                FontId::proportional(10.5),
                Self::muted_text_color(),
            );
        }

        let pointer_pos = ctx.input(|input| input.pointer.hover_pos());
        let pointer_over_timeline =
            pointer_pos.is_some_and(|pointer| viewport_rect.contains(pointer));
        let ctrl_scroll_y = if pointer_over_timeline {
            ctx.input(|input| {
                if input.modifiers.ctrl || input.modifiers.command || input.modifiers.mac_cmd {
                    input.raw_scroll_delta.y
                } else {
                    0.0
                }
            })
        } else {
            0.0
        };
        if ctrl_scroll_y.abs() > f32::EPSILON {
            let visible_duration_before_zoom = (base_visible_secs / zoom.max(0.1)).max(0.25);
            let visible_ratio = pointer_pos
                .map(|pointer| {
                    ((pointer.x - viewport_rect.left()) / viewport_rect.width()).clamp(0.0, 1.0)
                })
                .unwrap_or(0.5);
            let anchor_time = view_start_secs + visible_ratio * visible_duration_before_zoom;
            let zoom_factor = if ctrl_scroll_y > 0.0 { 1.12 } else { 1.0 / 1.12 };
            zoom = (zoom * zoom_factor).clamp(0.1, 8.0);
            let next_visible_duration = (base_visible_secs / zoom.max(0.1)).max(0.25);
            requested_view_start_secs = Some(
                (anchor_time - visible_ratio * next_visible_duration).max(0.0),
            );
            ctx.input_mut(|input| {
                input.smooth_scroll_delta = Vec2::ZERO;
                input.raw_scroll_delta = Vec2::ZERO;
            });
            ctx.request_repaint();
        }
        let requested_or_current_view_start = requested_view_start_secs.unwrap_or(view_start_secs).max(0.0);
        view_start_secs = requested_or_current_view_start;
        let visible_duration = (base_visible_secs / zoom.max(0.1)).max(0.25);
        let workspace_padding_secs = visible_duration.max(24.0);
        let workspace_duration = timeline_content_duration
            .max(requested_or_current_view_start + visible_duration)
            .max(self.trim_timeline_view_start_secs.max(0.0) + visible_duration)
            + workspace_padding_secs;
        let max_view_start_secs = (workspace_duration - visible_duration).max(0.0);
        view_start_secs = view_start_secs.clamp(0.0, max_view_start_secs);
        let view_end_secs = view_start_secs + visible_duration;
        let shared_timeline_left = viewport_rect.left() + 92.0;
        let shared_timeline_right = viewport_rect.right() - 46.0;
        let shared_timeline_width = (shared_timeline_right - shared_timeline_left).max(1.0);
        let snap_threshold_secs = ((visible_duration / shared_timeline_width) * 18.0).clamp(0.08, 1.0);
        let mut global_snap_x = None;
        let global_playhead_x = shared_timeline_left
            + ((state_snapshot.playhead_secs.max(0.0) - view_start_secs) / visible_duration)
                .clamp(0.0, 1.0)
                * shared_timeline_width;

        for (row_index, row) in state_snapshot.rows.iter().enumerate() {
            let row_top = viewport_rect.top() + row_index as f32 * (row_height + row_spacing);
            let row_rect = Rect::from_min_size(
                Pos2::new(viewport_rect.left(), row_top),
                vec2(viewport_rect.width(), row_height),
            );
            let painter = viewport_painter.clone();

            let label_rect = Rect::from_min_max(
                Pos2::new(row_rect.left() + 12.0, row_rect.top() + 12.0),
                Pos2::new(row_rect.left() + 84.0, row_rect.bottom() - 12.0),
            );
            painter.text(
                label_rect.left_top(),
                Align2::LEFT_TOP,
                format!("Row {}", row_index + 1),
                FontId::proportional(11.5),
                Self::muted_text_color(),
            );

            let remove_row_rect = Rect::from_min_size(
                Pos2::new(row_rect.right() - 36.0, row_rect.center().y - 14.0),
                vec2(28.0, 28.0),
            );
            if row_index > 0 {
                painter.rect_filled(remove_row_rect, 10.0, Self::panel_fill());
                painter.rect_stroke(
                    remove_row_rect,
                    10.0,
                    Stroke::new(1.0, Self::border_color()),
                    StrokeKind::Outside,
                );
                painter.text(
                    remove_row_rect.center(),
                    Align2::CENTER_CENTER,
                    "-",
                    FontId::proportional(20.0),
                    Self::strong_text_color(),
                );
                let response = ui.interact(
                    remove_row_rect,
                    ui.id().with(("trim-mix-remove-row", sound_id, row_index)),
                    Sense::click(),
                );
                if response.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    remove_row = Some(row_index);
                }
            }

            let timeline_radius = if row_count <= 1 {
                CornerRadius::same(14)
            } else if row_index == 0 {
                CornerRadius {
                    nw: 14,
                    ne: 14,
                    sw: 0,
                    se: 0,
                }
            } else if row_index + 1 == row_count {
                CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 14,
                    se: 14,
                }
            } else {
                CornerRadius::ZERO
            };
            let timeline_rect = Rect::from_min_max(
                Pos2::new(shared_timeline_left, row_rect.top()),
                Pos2::new(shared_timeline_right, row_rect.bottom()),
            );
            let row_hovered = ctx
                .input(|input| input.pointer.hover_pos())
                .is_some_and(|pointer| timeline_rect.contains(pointer));
            painter.rect_filled(
                timeline_rect,
                timeline_radius,
                Self::input_fill(),
            );
            if row_hovered && pending_drag_sound.is_some() {
                painter.rect_stroke(
                    timeline_rect,
                    timeline_radius,
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 92)),
                    StrokeKind::Outside,
                );
            }
            let tick_step = if visible_duration > 90.0 {
                15.0
            } else if visible_duration > 45.0 {
                10.0
            } else if visible_duration > 18.0 {
                5.0
            } else if visible_duration > 8.0 {
                2.0
            } else {
                1.0
            };
            let first_tick = (view_start_secs / tick_step).floor() * tick_step;
            let mut tick_time = first_tick;
            while tick_time <= view_end_secs + tick_step {
                if tick_time >= view_start_secs {
                    let tick_ratio = ((tick_time - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                    let tick_x = timeline_rect.left() + tick_ratio * timeline_rect.width();
                    painter.line_segment(
                        [
                            Pos2::new(tick_x, timeline_rect.top() + 6.0),
                            Pos2::new(tick_x, timeline_rect.top() + 14.0),
                        ],
                        Stroke::new(1.0, Self::subtle_border_color()),
                    );
                    painter.text(
                        Pos2::new(tick_x + 4.0, timeline_rect.top() + 4.0),
                        Align2::LEFT_TOP,
                        format_time(tick_time.max(0.0)),
                        FontId::proportional(9.5),
                        Self::muted_text_color(),
                    );
                }
                tick_time += tick_step;
            }
            let timeline_click_response = ui.interact(
                timeline_rect,
                ui.id().with(("trim-timeline-track", sound_id, row_index)),
                Sense::click(),
            );
            if timeline_click_response.clicked()
                && let Some(pointer) = timeline_click_response.interact_pointer_pos()
            {
                let secs = (view_start_secs
                    + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                        * visible_duration)
                    .max(0.0);
                self.set_trim_timeline_playhead(sound_id, secs);
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.selected_clip_id = None;
                }
            }

            for (clip_index, clip) in row.clips.iter().enumerate() {
                let Some(sound) = self
                    .sounds
                    .iter()
                    .find(|candidate| candidate.id == clip.source_sound_id)
                else {
                    continue;
                };
                let clip_duration = (clip.clip_end_secs - clip.clip_start_secs).max(0.05);
                let clip_time_start = clip.start_secs.max(0.0);
                let clip_time_end = clip_time_start + clip_duration;
                if clip_time_end <= view_start_secs || clip_time_start >= view_end_secs {
                    continue;
                }
                let visible_clip_start = clip_time_start.max(view_start_secs);
                let visible_clip_end = clip_time_end.min(view_end_secs);
                let clip_left = timeline_rect.left()
                    + ((visible_clip_start - view_start_secs) / visible_duration) * timeline_rect.width();
                let clip_right = timeline_rect.left()
                    + ((visible_clip_end - view_start_secs) / visible_duration) * timeline_rect.width();
                let clip_rect = Rect::from_min_max(
                    Pos2::new(clip_left, timeline_rect.top() + 1.0),
                    Pos2::new(
                        clip_right.max(clip_left + 1.5).min(timeline_rect.right()),
                        timeline_rect.bottom() - 1.0,
                    ),
                );
                let min_hit_width = 18.0;
                let clip_hit_rect = if clip_rect.width() >= min_hit_width {
                    clip_rect
                } else {
                    Rect::from_center_size(
                        clip_rect.center(),
                        vec2(min_hit_width.min(timeline_rect.width()), clip_rect.height()),
                    )
                    .intersect(timeline_rect.shrink2(vec2(0.0, 1.0)))
                };
                let removable = true;
                let clip_response = ui.interact(
                    clip_hit_rect,
                    ui.id().with(("trim-mix-clip", sound_id, clip.id)),
                    Sense::click_and_drag(),
                );
                let drag_anchor_id = ui.id().with(("trim-mix-drag-anchor", sound_id, clip.id));
                let selected_clip = state_snapshot.selected_clip_id == Some(clip.id);

                painter.rect_filled(
                    clip_rect,
                    12.0,
                    if selected_clip {
                        Color32::from_rgba_premultiplied(92, 38, 71, 228)
                    } else {
                        Color32::from_rgba_premultiplied(34, 28, 40, 220)
                    },
                );
                painter.rect_stroke(
                    clip_rect,
                    12.0,
                    Stroke::new(
                        1.0,
                        if selected_clip {
                            Color32::from_rgb(255, 112, 181)
                        } else if clip_response.hovered() && removable {
                            Color32::from_rgb(255, 182, 214)
                        } else {
                            Self::border_color()
                        },
                    ),
                    StrokeKind::Outside,
                );
                let visible_local_start =
                    clip.clip_start_secs + (visible_clip_start - clip_time_start).max(0.0);
                let visible_local_end =
                    clip.clip_end_secs - (clip_time_end - visible_clip_end).max(0.0);
                let waveform_bars = ((clip_rect.width() / 7.0).round() as usize).clamp(16, 96);
                let preview = Self::timeline_clip_waveform_preview_from_samples(
                    sound,
                    &self.sound_waveform_samples(sound),
                    visible_local_start,
                    visible_local_end,
                    waveform_bars,
                );
                if clip_rect.width() >= 18.0 {
                    let title_rect = Rect::from_min_max(
                        clip_rect.left_top() + vec2(8.0, 2.0),
                        Pos2::new((clip_rect.right() - 8.0).max(clip_rect.left() + 8.0), clip_rect.top() + 15.0),
                    );
                    let waveform_rect = Rect::from_min_max(
                        Pos2::new(clip_rect.left() + 8.0, clip_rect.top() + 15.0),
                        Pos2::new((clip_rect.right() - 8.0).max(clip_rect.left() + 8.0), clip_rect.bottom() - 4.0),
                    );
                    Self::paint_timeline_waveform_columns(
                        &painter,
                        waveform_rect,
                        &preview,
                        Color32::from_rgb(241, 78, 162),
                        selected_clip,
                    );
                    if clip_rect.width() >= 52.0 {
                        let title_max_chars =
                            ((title_rect.width() / 7.0).floor() as usize).clamp(6, 72);
                        let title_text = Self::truncate_middle_ascii(&sound.name, title_max_chars);
                        painter.with_clip_rect(title_rect).text(
                            title_rect.left_top(),
                            Align2::LEFT_TOP,
                            title_text,
                            FontId::proportional(11.0),
                            Self::strong_text_color(),
                        );
                    }
                }

                if clip_response.dragged() {
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                } else if clip_response.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }
                if clip_response.clicked()
                {
                    let pointer_time = clip_response
                        .interact_pointer_pos()
                        .map(|pointer| {
                            view_start_secs
                                + ((pointer.x - timeline_rect.left()) / timeline_rect.width())
                                    .clamp(0.0, 1.0)
                                    * visible_duration
                        })
                        .unwrap_or(clip.start_secs);
                    self.set_trim_timeline_playhead(sound_id, pointer_time);
                    if let Some(state) = self.trim_timeline_state.as_mut() {
                        state.selected_clip_id = Some(clip.id);
                    }
                }
                if clip_response.drag_started()
                    && let Some(pointer) = clip_response.interact_pointer_pos()
                {
                    let pointer_time = view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration;
                    let grab_offset_secs = (pointer_time - clip.start_secs).clamp(0.0, clip_duration);
                    ui.ctx().data_mut(|data| data.insert_temp(drag_anchor_id, grab_offset_secs));
                }
                if selected_clip
                    && clip_rect.contains(ctx.input(|input| input.pointer.hover_pos()).unwrap_or(clip_rect.center()))
                    && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
                {
                    let clip_before = self.trim_timeline_state.clone();
                    let pointer_time = view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration;
                    let local_time =
                        clip.clip_start_secs + (pointer_time - clip.start_secs).clamp(0.0, clip_duration);
                    painter.line_segment(
                        [
                            Pos2::new(pointer.x, clip_rect.top() + 6.0),
                            Pos2::new(pointer.x, clip_rect.bottom() - 6.0),
                        ],
                        Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 150)),
                    );
                    painter.circle_filled(
                        Pos2::new(pointer.x, clip_rect.top() + 10.0),
                        2.4,
                        Color32::from_rgba_premultiplied(108, 231, 255, 180),
                    );
                    let mut q_changed = false;
                    let mut w_changed = false;
                    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Q))
                        && let Some(state) = self.trim_timeline_state.as_mut()
                        && let Some(target_clip) = state.rows[row_index].clips.get_mut(clip_index)
                    {
                        target_clip.clip_start_secs =
                            local_time.clamp(0.0, target_clip.clip_end_secs - 0.05);
                        q_changed = true;
                    }
                    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::W))
                        && let Some(state) = self.trim_timeline_state.as_mut()
                        && let Some(target_clip) = state.rows[row_index].clips.get_mut(clip_index)
                    {
                        target_clip.clip_end_secs =
                            local_time.clamp(target_clip.clip_start_secs + 0.05, clip.clip_end_secs.max(local_time));
                        w_changed = true;
                    }
                    if (q_changed || w_changed)
                        && let Some(before) = clip_before
                    {
                        self.push_trim_timeline_undo_snapshot(before);
                        timeline_state_changed = true;
                        self.mark_dirty(ctx);
                        ctx.request_repaint();
                    }
                }
                if removable && clip_response.secondary_clicked() {
                    remove_clip = Some((row_index, clip_index));
                    timeline_state_changed = true;
                }
                if removable
                    && clip_response.dragged()
                    && let Some(pointer) = clip_response.interact_pointer_pos()
                    && let Some(before) = self.trim_timeline_state.clone()
                    && let Some(state) = self.trim_timeline_state.as_mut()
                {
                    let target_row = state
                        .rows
                        .iter()
                        .enumerate()
                        .find_map(|(candidate_row, _)| {
                            let top = viewport_rect.top()
                                + candidate_row as f32 * (row_height + row_spacing);
                            let rect = Rect::from_min_max(
                                Pos2::new(timeline_rect.left(), top),
                                Pos2::new(timeline_rect.right(), top + row_height),
                            );
                            rect.contains(pointer).then_some(candidate_row)
                        })
                        .unwrap_or(row_index);
                    let target_row_top =
                        viewport_rect.top() + target_row as f32 * (row_height + row_spacing);
                    let target_timeline_rect = Rect::from_min_max(
                        Pos2::new(timeline_rect.left(), target_row_top),
                        Pos2::new(timeline_rect.right(), target_row_top + row_height),
                    );
                    let pointer_time = (view_start_secs
                        + ((pointer.x - target_timeline_rect.left()) / target_timeline_rect.width())
                            .clamp(0.0, 1.0)
                            * visible_duration)
                        .max(0.0);
                    let grab_offset_secs = ui
                        .ctx()
                        .data(|data| data.get_temp::<f32>(drag_anchor_id))
                        .unwrap_or(0.0);
                    let desired_start = (pointer_time - grab_offset_secs).max(0.0);
                    let snap_points =
                        Self::trim_timeline_collect_snap_points(&state.rows, Some(clip.id));
                    let (next_start, _) = Self::trim_timeline_snap_start(
                        desired_start,
                        (clip.clip_end_secs - clip.clip_start_secs).max(0.05),
                        &snap_points,
                        state.snap_enabled,
                        snap_threshold_secs,
                    );
                    if state.snap_enabled {
                        let (_, snapped_point) = Self::trim_timeline_snap_start(
                            desired_start,
                            (clip.clip_end_secs - clip.clip_start_secs).max(0.05),
                            &snap_points,
                            true,
                            snap_threshold_secs,
                        );
                        if let Some(snap_point) = snapped_point {
                            let snap_ratio =
                                ((snap_point - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                            global_snap_x = Some(timeline_rect.left() + snap_ratio * timeline_rect.width());
                        }
                    }
                    let moving_clip = state.rows[row_index].clips.remove(clip_index);
                    let insert_row = target_row.min(state.rows.len().saturating_sub(1));
                    let resolved_start = Self::trim_timeline_resolve_row_start(
                        &state.rows[insert_row],
                        Some(moving_clip.id),
                        next_start,
                        (moving_clip.clip_end_secs - moving_clip.clip_start_secs).max(0.05),
                    );
                    let mut moved = moving_clip.clone();
                    moved.start_secs = resolved_start.max(0.0);
                    state.rows[insert_row].clips.push(moved);
                    state.rows[insert_row]
                        .clips
                        .sort_by(|left, right| left.start_secs.total_cmp(&right.start_secs));
                    state.selected_clip_id = Some(clip.id);
                    if before != *state {
                        self.push_trim_timeline_undo_snapshot(before);
                    }
                }
                if clip_response.drag_stopped() {
                    ui.ctx().data_mut(|data| {
                        data.remove::<f32>(drag_anchor_id);
                    });
                    timeline_state_changed = true;
                }

                if selected_clip
                    && clip_rect.contains(ctx.input(|input| input.pointer.hover_pos()).unwrap_or(clip_rect.center()))
                {
                    let hint_color = Color32::from_rgba_premultiplied(108, 231, 255, 92);
                    painter.text(
                        Pos2::new(clip_rect.left() + 8.0, clip_rect.bottom() - 9.0),
                        Align2::LEFT_BOTTOM,
                        "Q / W",
                        FontId::proportional(9.5),
                        hint_color,
                    );
                }
            }

            if let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
                && timeline_rect.contains(pointer)
                && pending_drag_sound.is_some()
            {
                let ratio = ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0);
                let desired_start_secs =
                    ((view_start_secs + ratio * visible_duration) * 20.0).round() / 20.0;
                let (start_secs, snapped_point) = pending_drag_sound
                    .as_ref()
                    .map(|drag_sound| {
                        Self::trim_timeline_preview_drop_start(
                            &state_snapshot.rows,
                            row,
                            desired_start_secs,
                            drag_sound.trimmed_length(),
                            state_snapshot.snap_enabled,
                            snap_threshold_secs,
                        )
                    })
                    .unwrap_or((desired_start_secs, None));
                next_drop_target = Some(TrimTimelineDropTarget {
                    row_index,
                    start_secs,
                });
                let marker_ratio =
                    ((start_secs - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                let marker_x = timeline_rect.left() + marker_ratio * timeline_rect.width();
                painter.line_segment(
                    [
                        Pos2::new(marker_x, timeline_rect.top() + 6.0),
                        Pos2::new(marker_x, timeline_rect.bottom() - 6.0),
                    ],
                    Stroke::new(2.0, Color32::from_rgb(108, 231, 255)),
                );
                if let Some(snap_point) = snapped_point {
                    let snap_ratio =
                        ((snap_point - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                    global_snap_x = Some(timeline_rect.left() + snap_ratio * timeline_rect.width());
                }
                if let Some(drag_sound) = pending_drag_sound.as_ref() {
                    let clip_width =
                        ((drag_sound.trimmed_length() / visible_duration) * timeline_rect.width()).max(1.5);
                    let ghost_rect = Rect::from_min_max(
                        Pos2::new(marker_x, timeline_rect.top() + 8.0),
                        Pos2::new(
                            (marker_x + clip_width).min(timeline_rect.right()),
                            timeline_rect.bottom() - 8.0,
                        ),
                    );
                    painter.rect_filled(
                        ghost_rect,
                        12.0,
                        Color32::from_rgba_premultiplied(56, 34, 49, 170),
                    );
                    painter.rect_stroke(
                        ghost_rect,
                        10.0,
                        Stroke::new(1.25, Color32::from_rgba_premultiplied(255, 112, 181, 196)),
                        StrokeKind::Outside,
                    );
                    let ghost_waveform = self.trim_timeline_track_waveform(drag_sound, 64);
                    if ghost_rect.width() >= 18.0 {
                        let ghost_title_rect = Rect::from_min_max(
                            ghost_rect.left_top() + vec2(8.0, 4.0),
                            Pos2::new((ghost_rect.right() - 8.0).max(ghost_rect.left() + 8.0), ghost_rect.top() + 18.0),
                        );
                        let ghost_waveform_rect = Rect::from_min_max(
                            Pos2::new(ghost_rect.left() + 8.0, ghost_rect.top() + 18.0),
                            Pos2::new((ghost_rect.right() - 8.0).max(ghost_rect.left() + 8.0), ghost_rect.bottom() - 8.0),
                        );
                        Self::paint_timeline_waveform_columns(
                            &painter,
                            ghost_waveform_rect,
                            &ghost_waveform,
                            Color32::from_rgb(241, 78, 162),
                            true,
                        );
                        if ghost_rect.width() >= 52.0 {
                            let ghost_title_max_chars =
                                ((ghost_title_rect.width() / 7.0).floor() as usize).clamp(6, 72);
                            let ghost_title =
                                Self::truncate_middle_ascii(&drag_sound.name, ghost_title_max_chars);
                            painter.with_clip_rect(ghost_title_rect).text(
                                ghost_title_rect.left_top(),
                                Align2::LEFT_TOP,
                                ghost_title,
                                FontId::proportional(11.0),
                                Color32::from_rgb(255, 236, 245),
                            );
                        }
                    }
                    painter.text(
                        Pos2::new(marker_x + 6.0, timeline_rect.top() - 8.0),
                        Align2::LEFT_BOTTOM,
                        format!("Drop at {}", format_time(start_secs)),
                        FontId::proportional(10.5),
                        Color32::from_rgb(208, 244, 255),
                    );
                }
                ctx.set_cursor_icon(egui::CursorIcon::Copy);
            }
        }

        if let Some(snap_x) = global_snap_x {
            viewport_painter.line_segment(
                [
                    Pos2::new(snap_x, viewport_rect.top() + 3.0),
                    Pos2::new(snap_x, viewport_rect.bottom() - 3.0),
                ],
                Stroke::new(1.2, Color32::from_rgba_premultiplied(255, 112, 181, 210)),
            );
        }
        viewport_painter.line_segment(
            [
                Pos2::new(global_playhead_x, viewport_rect.top() + 3.0),
                Pos2::new(global_playhead_x, viewport_rect.bottom() - 3.0),
            ],
            Stroke::new(1.75, Color32::from_rgb(108, 231, 255)),
        );
        viewport_painter.circle_filled(
            Pos2::new(global_playhead_x, viewport_rect.top() + 9.0),
            3.0,
            Color32::from_rgb(108, 231, 255),
        );

        if let Some(state) = self.trim_timeline_state.as_mut() {
            state.enabled = next_enabled;
            if add_row {
                state.rows.push(TrimTimelineRow::default());
                timeline_state_changed = true;
            }
            if reset_rows {
                state.rows = Self::default_trim_timeline_rows(sound_id);
                timeline_state_changed = true;
            }
            if let Some(row_index) = remove_row
                && row_index < state.rows.len()
                && row_index > 0
            {
                if state.rows[row_index]
                    .clips
                    .iter()
                    .any(|clip| Some(clip.id) == state.selected_clip_id)
                {
                    state.selected_clip_id = None;
                }
                state.rows.remove(row_index);
                timeline_state_changed = true;
            }
            if let Some((row_index, clip_index)) = remove_clip
                && let Some(row) = state.rows.get_mut(row_index)
                && clip_index < row.clips.len()
            {
                let removed_clip_id = row.clips[clip_index].id;
                row.clips.remove(clip_index);
                if state.selected_clip_id == Some(removed_clip_id) {
                    state.selected_clip_id = None;
                }
                timeline_state_changed = true;
            }
        }
        if timeline_state_changed {
            self.refresh_trim_timeline_preview_after_edit(sound_id);
        }
        self.trim_timeline_zoom = zoom;
        self.trim_timeline_view_start_secs = view_start_secs;
        self.trim_timeline_drop_target = next_drop_target;

        save_request
    }

    pub(super) fn render_trim_commit_panel(&mut self, ctx: &Context) {
        if !self.show_trim_commit_panel {
            return;
        }

        let mut open_panel = self.show_trim_commit_panel;
        let mut close_request = false;
        let mut keep_old = false;
        let mut replace_current = false;
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, vec2(360.0, 180.0), vec2(280.0, 160.0), 0.0);

        egui::Window::new("")
            .id(egui::Id::new("trim-commit-panel"))
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
                    .inner_margin(Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(0xe14e, 20.0, Color32::from_rgb(214, 51, 132)).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if Self::icon_titlebar(ui, [34.0, 28.0], 0xe5cd, false, true).clicked() {
                            close_request = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(
                    RichText::new("Keep old file?")
                        .size(15.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );

                ui.add_space(12.0);
                ui.horizontal_centered(|ui| {
                    let keep_response = ui.add_sized(
                        [120.0, 38.0],
                        Self::action_button(RichText::new("Yes").size(13.0), false, true),
                    );
                    Self::decorate_button_response(ui, &keep_response);
                    if keep_response.clicked() {
                        keep_old = true;
                    }

                    let replace_response = ui.add_sized(
                        [120.0, 38.0],
                        Self::action_button(RichText::new("No").size(13.0), false, false),
                    );
                    Self::decorate_button_response(ui, &replace_response);
                    if replace_response.clicked() {
                        replace_current = true;
                    }
                });
            });

        if close_request {
            open_panel = false;
        }
        self.show_trim_commit_panel = open_panel;

        if keep_old {
            self.show_trim_commit_panel = false;
            self.duplicate_selected_trimmed_sound(ctx);
        }
        if replace_current {
            self.show_trim_commit_panel = false;
            self.commit_selected_trimmed_sound(ctx);
        }
    }
}
