use super::*;
#[cfg(windows)]
use clipboard_win::Getter;

const TRIM_HISTORY_LIMIT: usize = 128;

fn next_default_sound_name<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let names = names.into_iter().collect::<Vec<_>>();
    (1usize..)
        .map(|index| format!("Sound {index}"))
        .find(|candidate| {
            names
                .iter()
                .all(|name| !name.eq_ignore_ascii_case(candidate))
        })
        .expect("unbounded sound numbering should always find a name")
}

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

    pub(super) fn copy_selected_processed_sound(&mut self, ctx: &Context) {
        let Some(index) = self.selected_sound_index() else {
            return;
        };
        let sound_id = self.sounds[index].id;

        match self.copy_sound_file_to_clipboard(&self.sounds[index]) {
            Ok(()) => {
                self.clear_status();
                self.mark_sound_copied(ctx, sound_id);
                ctx.request_repaint();
            }
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
            selected_clip_ids: HashSet::new(),
            rows: Self::default_trim_timeline_rows(),
            timeline_is_playing: false,
        }
    }

    fn default_trim_timeline_rows() -> Vec<TrimTimelineRow> {
        vec![
            TrimTimelineRow::default(),
            TrimTimelineRow::default(),
            TrimTimelineRow::default(),
        ]
    }

    fn normalize_trim_timeline_rows(rows: &mut Vec<TrimTimelineRow>) {
        while rows.len() < 3 {
            rows.push(TrimTimelineRow::default());
        }
        while rows.len() > 3
            && rows.last().is_some_and(|row| row.clips.is_empty())
            && rows
                .get(rows.len().saturating_sub(2))
                .is_some_and(|row| row.clips.is_empty())
        {
            rows.pop();
        }
        if rows.last().is_some_and(|row| !row.clips.is_empty()) {
            rows.push(TrimTimelineRow::default());
        }
    }

    fn resolve_trim_timeline_anchor_sound_id(&self, fallback_sound_id: Uuid) -> Uuid {
        let contains_sound = |sound_id| self.sounds.iter().any(|sound| sound.id == sound_id);

        if contains_sound(fallback_sound_id) {
            return fallback_sound_id;
        }
        if let Some(sound_id) = self
            .trim_timeline_state
            .as_ref()
            .map(|state| state.sound_id)
            .filter(|sound_id| contains_sound(*sound_id))
        {
            return sound_id;
        }
        if let Some(sound_id) = self.trim_timeline_state.as_ref().and_then(|state| {
            state
                .rows
                .iter()
                .flat_map(|row| row.clips.iter())
                .find_map(|clip| contains_sound(clip.source_sound_id).then_some(clip.source_sound_id))
        }) {
            return sound_id;
        }
        if let Some(sound_id) = self.selected.filter(|sound_id| contains_sound(*sound_id)) {
            return sound_id;
        }
        self.sounds
            .first()
            .map(|sound| sound.id)
            .unwrap_or(fallback_sound_id)
    }

    fn sync_trim_timeline_state_for(&mut self, sound_id: Uuid) -> Uuid {
        let resolved_sound_id = self.resolve_trim_timeline_anchor_sound_id(sound_id);
        if self.trim_timeline_state.is_none() {
            self.trim_timeline_state = Some(Self::default_trim_timeline_state(resolved_sound_id));
            self.trim_timeline_view_start_secs = 0.0;
            self.trim_timeline_clip_delete_animating.clear();
            self.trim_timeline_segment_delete_animations.clear();
            self.trim_timeline_scrub_resume_pending = false;
        }

        let timeline_zoom = self.trim_timeline_zoom;
        let timeline_view_start_secs = self.trim_timeline_view_start_secs.max(0.0);

        let Some(state) = self.trim_timeline_state.as_mut() else {
            return resolved_sound_id;
        };
        state.sound_id = resolved_sound_id;
        Self::normalize_trim_timeline_rows(&mut state.rows);
        let total_duration = state
            .rows
            .iter()
            .flat_map(|row| row.clips.iter())
            .map(|clip| clip.start_secs.max(0.0) + Self::timeline_clip_duration(clip))
            .fold(0.0f32, f32::max)
            .max(0.25);
        let visible_duration = (12.0 / timeline_zoom.max(0.1)).max(0.25);
        let workspace_padding_secs = visible_duration.max(24.0);
        let workspace_duration =
            total_duration.max(timeline_view_start_secs + visible_duration) + workspace_padding_secs;
        state.playhead_secs = state.playhead_secs.clamp(0.0, workspace_duration.max(0.05));
        resolved_sound_id
    }

    pub(super) fn timeline_mode_active_sound_id(&self) -> Option<Uuid> {
        self.trim_timeline_state
            .as_ref()
            .filter(|state| state.enabled)
            .map(|state| state.sound_id)
    }

    pub(super) fn toggle_timeline_mode(&mut self, ctx: &Context) {
        let fallback_sound_id = self
            .selected
            .or_else(|| self.sounds.first().map(|sound| sound.id))
            .unwrap_or_else(Uuid::nil);
        self.sync_trim_timeline_state_for(fallback_sound_id);

        let was_enabled = self
            .trim_timeline_state
            .as_ref()
            .is_some_and(|state| state.enabled);
        let mut open_workspace = false;
        let mut clear_drop_target = false;

        if let Some(state) = self.trim_timeline_state.as_mut() {
            if !was_enabled {
                state.selected_clip_id = None;
                state.playhead_secs = 0.0;
                if state.rows.is_empty() {
                    state.rows = Self::default_trim_timeline_rows();
                }
                open_workspace = true;
            } else {
                clear_drop_target = true;
            }
            state.enabled = !was_enabled;
        }

        if open_workspace {
            self.trim_timeline_view_start_secs = 0.0;
            self.app_view = AppView::Editor;
        }
        if clear_drop_target {
            self.trim_timeline_drop_target = None;
        }
        self.stop_preview();
        ctx.request_repaint();
    }

    pub(super) fn trim_timeline_drag_capture_active(&self) -> bool {
        self.pending_sound_drag.is_some()
            && self
                .trim_timeline_state
                .as_ref()
                .is_some_and(|state| state.enabled)
    }

    fn trim_timeline_clip_duration(&self, clip: &TrimTimelineClip) -> f32 {
        Self::timeline_clip_duration(clip)
    }

    fn timeline_clip_duration(clip: &TrimTimelineClip) -> f32 {
        (clip.clip_end_secs - clip.clip_start_secs).max(0.05)
            / clip.audio.speed.clamp(0.25, 2.0)
    }

    fn trim_timeline_clip_source_duration(&self, source_sound_id: Uuid) -> f32 {
        self.sounds
            .iter()
            .find(|sound| sound.id == source_sound_id)
            .map(SoundEffect::trimmed_length)
            .unwrap_or(0.05)
            .max(0.05)
    }

    fn trim_timeline_resize_clip_edge_to_pointer(
        &mut self,
        sound_id: Uuid,
        row_index: usize,
        clip_index: usize,
        original_clip: &TrimTimelineClip,
        pointer_time_secs: f32,
        trim_left: bool,
    ) -> bool {
        let source_duration = self.trim_timeline_clip_source_duration(original_clip.source_sound_id);
        let Some(state) = self.trim_timeline_state.as_mut() else {
            return false;
        };
        if !state.enabled || state.sound_id != sound_id {
            return false;
        }
        let Some(row) = state.rows.get(row_index) else {
            return false;
        };
        let Some(current_clip) = row.clips.get(clip_index) else {
            return false;
        };
        if current_clip.id != original_clip.id {
            return false;
        }

        let min_start_secs = if clip_index > 0 {
            let previous = &row.clips[clip_index - 1];
            previous.start_secs.max(0.0)
                + (previous.clip_end_secs - previous.clip_start_secs).max(0.05)
        } else {
            0.0
        };
        let max_end_secs = row
            .clips
            .get(clip_index + 1)
            .map(|next| next.start_secs.max(0.0))
            .unwrap_or(f32::INFINITY);

        let original_start_secs = original_clip.start_secs.max(0.0);
        let original_duration = (original_clip.clip_end_secs - original_clip.clip_start_secs).max(0.05);
        let original_end_secs = original_start_secs + original_duration;
        let desired_pointer_secs = pointer_time_secs.max(0.0);

        let Some(target_clip) = state
            .rows
            .get_mut(row_index)
            .and_then(|row| row.clips.get_mut(clip_index))
        else {
            return false;
        };

        if trim_left {
            let desired_start_secs =
                desired_pointer_secs.clamp(min_start_secs, original_end_secs - 0.05);
            let desired_delta = desired_start_secs - original_start_secs;
            let allowed_negative_delta =
                (original_start_secs - min_start_secs).min(original_clip.clip_start_secs.max(0.0));
            let actual_delta = desired_delta.clamp(-allowed_negative_delta, original_duration - 0.05);
            target_clip.start_secs = (original_start_secs + actual_delta).max(0.0);
            target_clip.clip_start_secs =
                (original_clip.clip_start_secs + actual_delta).clamp(0.0, original_clip.clip_end_secs - 0.05);
        } else {
            let desired_end_secs =
                desired_pointer_secs.clamp(original_start_secs + 0.05, max_end_secs);
            let desired_delta = desired_end_secs - original_end_secs;
            let allowed_positive_delta =
                (max_end_secs - original_end_secs).min(source_duration - original_clip.clip_end_secs);
            let actual_delta =
                desired_delta.clamp(-(original_duration - 0.05), allowed_positive_delta.max(0.0));
            target_clip.clip_end_secs = (original_clip.clip_end_secs + actual_delta)
                .clamp(original_clip.clip_start_secs + 0.05, source_duration);
        }

        true
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
        Self::compact_timeline_waveform(&preview, max_bars)
    }

    fn trim_timeline_waveform_bars(width: f32) -> usize {
        ((width / 4.2).round() as usize).clamp(4, 168)
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

    fn trim_timeline_snap_playhead(
        desired_secs: f32,
        snap_points: &[f32],
        snap_enabled: bool,
        snap_threshold_secs: f32,
    ) -> (f32, Option<f32>) {
        let next_secs = desired_secs.max(0.0);
        if !snap_enabled {
            return (next_secs, None);
        }

        let mut best: Option<(f32, f32)> = None;
        for snap_point in snap_points.iter().copied() {
            let distance = (snap_point - next_secs).abs();
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, snap_point));
            }
        }

        if let Some((distance, snapped_secs)) = best
            && distance <= snap_threshold_secs.max(0.05)
        {
            return (snapped_secs.max(0.0), Some(snapped_secs.max(0.0)));
        }

        (next_secs, None)
    }

    fn trim_timeline_row_has_clip_at_time(
        row: &TrimTimelineRow,
        time_secs: f32,
        ignored_clip_id: Option<Uuid>,
    ) -> bool {
        row.clips.iter().any(|clip| {
            if Some(clip.id) == ignored_clip_id {
                return false;
            }
            let start = clip.start_secs.max(0.0);
            let end = start + Self::timeline_clip_duration(clip);
            time_secs >= start && time_secs < end
        })
    }

    fn trim_timeline_pick_paste_row(
        state: &mut TrimTimelineState,
        preferred_row_index: usize,
        playhead_secs: f32,
    ) -> usize {
        let clamped_preferred = preferred_row_index.min(state.rows.len().saturating_sub(1));
        if !Self::trim_timeline_row_has_clip_at_time(
            &state.rows[clamped_preferred],
            playhead_secs,
            None,
        ) {
            return clamped_preferred;
        }

        let mut best_row = None;
        let mut best_distance = usize::MAX;
        for (row_index, row) in state.rows.iter().enumerate() {
            if Self::trim_timeline_row_has_clip_at_time(row, playhead_secs, None) {
                continue;
            }
            let distance = row_index.abs_diff(clamped_preferred);
            if distance < best_distance {
                best_distance = distance;
                best_row = Some(row_index);
            }
        }

        if let Some(row_index) = best_row {
            return row_index;
        }

        state.rows.push(TrimTimelineRow::default());
        state.rows.len().saturating_sub(1)
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
            .filter(|row| !row.muted)
            .flat_map(|row| row.clips.iter())
            .filter_map(|clip| {
                self.sounds
                    .iter()
                    .find(|sound| sound.id == clip.source_sound_id)
                    .cloned()
                    .map(|mut sound| {
                        sound.volume = clip.audio.volume;
                        sound.speed = clip.audio.speed;
                        sound.reverb_enabled = clip.audio.reverb_enabled;
                        sound.telephone_enabled = clip.audio.telephone_enabled;
                        sound.distortion_enabled = clip.audio.distortion_enabled;
                        sound.echo_enabled = clip.audio.echo_enabled;
                        sound.underwater_enabled = clip.audio.underwater_enabled;
                        sound.robot_enabled = clip.audio.robot_enabled;
                        sound.pitch_shift_enabled = clip.audio.pitch_shift_enabled;
                        sound.pitch_shift_semitones = clip.audio.pitch_shift_semitones;
                        sound.vocal_only = clip.audio.vocal_only;
                        sound.music_only = clip.audio.music_only;
                        (
                            sound,
                            clip.start_secs.max(0.0),
                            clip.clip_start_secs.max(0.0),
                            clip.clip_end_secs.max(clip.clip_start_secs + 0.05),
                        )
                    })
            })
            .collect::<Vec<_>>();
        Some(clips)
    }

    pub(super) fn finalize_pending_trim_timeline_drop(&mut self) -> bool {
        let Some(drag_sound_id) = self.pending_sound_drag else {
            self.trim_timeline_drop_target = None;
            return false;
        };
        let Some(target) = self.trim_timeline_drop_target.take() else {
            return false;
        };
        let Some(selected_sound_id) = self.timeline_mode_active_sound_id() else {
            return false;
        };
        if self
            .sounds
            .iter()
            .all(|sound| sound.id != drag_sound_id)
        {
            return false;
        }

        let selected_sound_id = self.sync_trim_timeline_state_for(selected_sound_id);
        let Some(state) = self.trim_timeline_state.as_mut() else {
            return false;
        };
        if !state.enabled || state.sound_id != selected_sound_id || target.row_index >= state.rows.len() {
            if !state.enabled || state.sound_id != selected_sound_id {
                return false;
            }
        }
        while state.rows.len() <= target.row_index {
            state.rows.push(TrimTimelineRow::default());
        }
        let (clip_end_secs, clip_audio) = self
            .sounds
            .iter()
            .find(|sound| sound.id == drag_sound_id)
            .map(|sound| (sound.trimmed_length(), TimelineClipAudioSettings::from_sound(sound)))
            .unwrap_or((0.25, TimelineClipAudioSettings::default()));

        let inserted_clip_id = Uuid::new_v4();
        state.rows[target.row_index].clips.push(TrimTimelineClip {
            id: inserted_clip_id,
            source_sound_id: drag_sound_id,
            start_secs: target.start_secs.max(0.0),
            clip_start_secs: 0.0,
            clip_end_secs,
            audio: clip_audio,
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
        Self::normalize_trim_timeline_rows(&mut state.rows);
        self.pending_sound_drag = None;
        self.refresh_trim_timeline_preview_after_edit(selected_sound_id);
        true
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

    fn push_trim_timeline_undo_snapshot_changed(&mut self, before: TrimTimelineState) {
        self.trim_timeline_undo_stack.push(before);
        if self.trim_timeline_undo_stack.len() > TRIM_HISTORY_LIMIT {
            self.trim_timeline_undo_stack.remove(0);
        }
        self.trim_timeline_redo_stack.clear();
    }

    fn trim_timeline_selected_clip_from_state(
        state: &TrimTimelineState,
    ) -> Option<(usize, usize, TrimTimelineClip)> {
        let selected_clip_id = state.selected_clip_id?;
        state.rows.iter().enumerate().find_map(|(row_index, row)| {
            row.clips
                .iter()
                .enumerate()
                .find(|(_, clip)| clip.id == selected_clip_id)
                .map(|(clip_index, clip)| (row_index, clip_index, clip.clone()))
        })
    }

    fn trim_timeline_selected_clip_snapshot(
        &self,
        sound_id: Uuid,
    ) -> Option<(usize, usize, TrimTimelineClip)> {
        let state = self
            .trim_timeline_state
            .as_ref()
            .filter(|state| state.sound_id == sound_id && state.enabled)?;
        Self::trim_timeline_selected_clip_from_state(state)
    }

    fn trim_timeline_cut_local_time_from_playhead(
        state: &TrimTimelineState,
        clip: &TrimTimelineClip,
    ) -> Option<f32> {
        let clip_start = clip.clip_start_secs;
        let clip_end = clip.clip_end_secs.max(clip_start + 0.05);
        let timeline_local =
            clip_start + (state.playhead_secs.max(0.0) - clip.start_secs.max(0.0));
        let cut_local_time = timeline_local.clamp(clip_start, clip_end);
        (cut_local_time > clip_start + 0.05 && cut_local_time < clip_end - 0.05)
            .then_some(cut_local_time)
    }

    fn trim_timeline_begin_clip_delete_animation(
        &mut self,
        ctx: &Context,
        clip_id: Uuid,
    ) -> bool {
        if self.trim_timeline_clip_delete_animating.contains_key(&clip_id) {
            return false;
        }
        self.trim_timeline_clip_delete_animating
            .insert(clip_id, Instant::now());
        ctx.request_repaint();
        true
    }

    fn trim_timeline_delete_selected_clip(&mut self, ctx: &Context, sound_id: Uuid) -> bool {
        let Some((_, _, clip)) = self.trim_timeline_selected_clip_snapshot(sound_id) else {
            return false;
        };
        self.trim_timeline_begin_clip_delete_animation(ctx, clip.id)
    }

    fn trim_timeline_copy_selected_clip(&mut self, sound_id: Uuid) -> bool {
        let Some((_, _, clip)) = self.trim_timeline_selected_clip_snapshot(sound_id) else {
            return false;
        };
        self.trim_timeline_copied_clip = Some(TrimTimelineClipboardClip {
            source_sound_id: clip.source_sound_id,
            clip_start_secs: clip.clip_start_secs.max(0.0),
            clip_end_secs: clip.clip_end_secs.max(clip.clip_start_secs + 0.05),
            audio: clip.audio.clone(),
        });
        true
    }

    fn trim_timeline_paste_copied_clip(&mut self, ctx: &Context, sound_id: Uuid) -> bool {
        let Some(copied_clip) = self.trim_timeline_copied_clip.clone() else {
            return false;
        };

        self.sync_trim_timeline_state_for(sound_id);
        let Some(before) = self.trim_timeline_state.clone() else {
            return false;
        };
        let Some((playhead_secs, selected_row_index)) = self.trim_timeline_state.as_ref().map(|state| {
            let row_index = Self::trim_timeline_selected_clip_from_state(state)
                .map(|(row_index, _, _)| row_index)
                .unwrap_or(0);
            (state.playhead_secs.max(0.0), row_index)
        }) else {
            return false;
        };

        let clip_duration = (copied_clip.clip_end_secs - copied_clip.clip_start_secs).max(0.05)
            / copied_clip.audio.speed.clamp(0.25, 2.0);
        let inserted_clip_id = Uuid::new_v4();
        if let Some(state) = self.trim_timeline_state.as_mut() {
            if !state.enabled || state.sound_id != sound_id {
                return false;
            }
            let row_index =
                Self::trim_timeline_pick_paste_row(state, selected_row_index, playhead_secs);
            let resolved_start = Self::trim_timeline_resolve_row_start(
                &state.rows[row_index],
                None,
                playhead_secs,
                clip_duration,
            );
            state.rows[row_index].clips.push(TrimTimelineClip {
                id: inserted_clip_id,
                source_sound_id: copied_clip.source_sound_id,
                start_secs: resolved_start,
                clip_start_secs: copied_clip.clip_start_secs,
                clip_end_secs: copied_clip.clip_end_secs,
                audio: copied_clip.audio,
            });
            state.rows[row_index]
                .clips
                .sort_by(|left, right| left.start_secs.total_cmp(&right.start_secs));
            state.selected_clip_id = Some(inserted_clip_id);
            Self::normalize_trim_timeline_rows(&mut state.rows);
        }

        self.push_trim_timeline_undo_snapshot_changed(before);
        self.refresh_trim_timeline_preview_after_edit(sound_id);
        self.mark_dirty(ctx);
        ctx.request_repaint();
        true
    }

    fn trim_timeline_push_segment_delete_animation(
        &mut self,
        sound_id: Uuid,
        row_index: usize,
        clip: &TrimTimelineClip,
        removed_clip_start_secs: f32,
        removed_clip_end_secs: f32,
    ) {
        if removed_clip_end_secs <= removed_clip_start_secs + 0.01 {
            return;
        }
        let segment_timeline_start =
            clip.start_secs + (removed_clip_start_secs - clip.clip_start_secs).max(0.0);
        self.trim_timeline_segment_delete_animations.push(TrimTimelineSegmentDeleteAnimation {
            owner_sound_id: sound_id,
            row_index,
            source_sound_id: clip.source_sound_id,
            start_secs: segment_timeline_start.max(0.0),
            clip_start_secs: removed_clip_start_secs.max(0.0),
            clip_end_secs: removed_clip_end_secs.max(removed_clip_start_secs + 0.05),
            started_at: Instant::now(),
        });
    }

    fn trim_timeline_trim_clip_edge_at_local_time(
        &mut self,
        ctx: &Context,
        sound_id: Uuid,
        row_index: usize,
        clip_index: usize,
        cut_local_time: f32,
        trim_left: bool,
    ) -> bool {
        let Some(before) = self.trim_timeline_state.clone() else {
            return false;
        };

        let mut animation = None;
        if let Some(state) = self.trim_timeline_state.as_mut() {
            if !state.enabled || state.sound_id != sound_id {
                return false;
            }
            let Some(target_clip) = state
                .rows
                .get_mut(row_index)
                .and_then(|row| row.clips.get_mut(clip_index))
            else {
                return false;
            };
            let original_clip = target_clip.clone();
            let clip_start = original_clip.clip_start_secs;
            let clip_end = original_clip.clip_end_secs.max(clip_start + 0.05);
            let next_cut = cut_local_time.clamp(clip_start, clip_end);
            if next_cut <= clip_start + 0.05 || next_cut >= clip_end - 0.05 {
                return false;
            }

            if trim_left {
                animation = Some((clip_start, next_cut, original_clip.clone()));
                let removed_duration = next_cut - clip_start;
                target_clip.start_secs = original_clip.start_secs + removed_duration;
                target_clip.clip_start_secs = next_cut;
            } else {
                animation = Some((next_cut, clip_end, original_clip.clone()));
                target_clip.clip_end_secs = next_cut;
            }
            state.selected_clip_id = Some(original_clip.id);
        }

        if let Some((removed_start, removed_end, original_clip)) = animation {
            self.trim_timeline_push_segment_delete_animation(
                sound_id,
                row_index,
                &original_clip,
                removed_start,
                removed_end,
            );
        }
        self.push_trim_timeline_undo_snapshot_changed(before);
        self.refresh_trim_timeline_preview_after_edit(sound_id);
        self.mark_dirty(ctx);
        ctx.request_repaint();
        true
    }

    fn trim_timeline_trim_selected_clip_at_playhead(
        &mut self,
        ctx: &Context,
        sound_id: Uuid,
        trim_left: bool,
    ) -> bool {
        let Some(state) = self
            .trim_timeline_state
            .as_ref()
            .filter(|state| state.enabled && state.sound_id == sound_id)
        else {
            return false;
        };
        let Some((row_index, clip_index, clip)) = Self::trim_timeline_selected_clip_from_state(state) else {
            return false;
        };
        let Some(cut_local_time) = Self::trim_timeline_cut_local_time_from_playhead(state, &clip) else {
            return false;
        };
        self.trim_timeline_trim_clip_edge_at_local_time(
            ctx,
            sound_id,
            row_index,
            clip_index,
            cut_local_time,
            trim_left,
        )
    }

    fn trim_timeline_split_selected_clip_at_playhead(
        &mut self,
        ctx: &Context,
        sound_id: Uuid,
    ) -> bool {
        self.sync_trim_timeline_state_for(sound_id);
        let Some(before) = self.trim_timeline_state.clone() else {
            return false;
        };
        let Some((row_index, clip_index, clip, cut_local_time)) = self
            .trim_timeline_state
            .as_ref()
            .and_then(|state| {
                let (row_index, clip_index, clip) = Self::trim_timeline_selected_clip_from_state(state)?;
                let cut_local_time = Self::trim_timeline_cut_local_time_from_playhead(state, &clip)?;
                Some((row_index, clip_index, clip, cut_local_time))
            })
        else {
            return false;
        };

        let timeline_split_secs =
            clip.start_secs + (cut_local_time - clip.clip_start_secs).max(0.0);
        let new_clip_id = Uuid::new_v4();
        if let Some(state) = self.trim_timeline_state.as_mut() {
            let Some(row) = state.rows.get_mut(row_index) else {
                return false;
            };
            let Some(target_clip) = row.clips.get_mut(clip_index) else {
                return false;
            };
            target_clip.clip_end_secs = cut_local_time;
            row.clips.push(TrimTimelineClip {
                id: new_clip_id,
                source_sound_id: clip.source_sound_id,
                start_secs: timeline_split_secs,
                clip_start_secs: cut_local_time,
                clip_end_secs: clip.clip_end_secs,
                audio: clip.audio.clone(),
            });
            row.clips
                .sort_by(|left, right| left.start_secs.total_cmp(&right.start_secs));
            state.selected_clip_id = Some(new_clip_id);
            Self::normalize_trim_timeline_rows(&mut state.rows);
        }

        self.push_trim_timeline_undo_snapshot_changed(before);
        self.refresh_trim_timeline_preview_after_edit(sound_id);
        self.mark_dirty(ctx);
        ctx.request_repaint();
        true
    }

    fn commit_trim_timeline_playhead_after_scrub(&mut self, sound_id: Uuid, playhead_secs: f32) {
        let secs = playhead_secs.max(0.0);
        let should_resume = std::mem::take(&mut self.trim_timeline_scrub_resume_pending);
        if !should_resume {
            self.set_trim_timeline_playhead(sound_id, secs);
            return;
        }

        if let Some(state) = self.trim_timeline_state.as_mut()
            && state.sound_id == sound_id
        {
            state.playhead_secs = secs;
        }

        self.stop_preview();
        self.preview_timeline_mix_from_position(sound_id, secs);
    }

    fn preview_timeline_mix_from_position(&mut self, sound_id: Uuid, start_secs: f32) {
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };
        let Some(clips) = self.collect_trim_timeline_render_clips(sound_id) else {
            return;
        };
        let sound = self.sounds[index].clone();
        let preview_path = match Storage::export_timeline_mix_preview_at(
            self.storage.root_dir(),
            &sound,
            &clips,
            start_secs,
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
        if let Err(error) = audio.play_file_from(&preview_path, 0.0) {
            self.set_error_status(error);
            return;
        }
        let previous_preview = self.trim_timeline_preview_path.replace(preview_path.clone());
        if previous_preview.as_ref() != Some(&preview_path)
            && let Some(previous_preview) = previous_preview
            && previous_preview
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("timeline-preview-"))
        {
            let _ = fs::remove_file(previous_preview);
        }
        self.trim_timeline_preview_start_secs = start_secs.max(0.0);
        self.trim_timeline_preview_dirty = false;
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

        let preview_path = self.trim_timeline_preview_path.clone();
        let preview_active = self
            .audio
            .as_ref()
            .is_some_and(|audio| preview_path.as_ref().is_some_and(|path| audio.is_playing_file(path)));
        if !preview_active {
            return;
        }

        let was_paused = self.audio.as_ref().is_some_and(|audio| audio.is_paused());
        if self.trim_timeline_preview_dirty {
            self.stop_preview();
            self.preview_timeline_mix_from_position(sound_id, secs);
            if was_paused && let Some(audio) = self.audio.as_mut() {
                audio.pause();
            }
            return;
        }
        let preview_local_secs = secs - self.trim_timeline_preview_start_secs;
        if !(0.0..90.0).contains(&preview_local_secs) {
            self.stop_preview();
            self.preview_timeline_mix_from_position(sound_id, secs);
        } else if let Some(path) = preview_path
            && let Some(audio) = self.audio.as_mut()
            && let Err(error) = audio.play_file_from(&path, preview_local_secs)
        {
            self.set_error_status(error);
            return;
        }
        if was_paused && let Some(audio) = self.audio.as_mut() {
            audio.pause();
        }
    }

    fn prepare_trim_timeline_preview_mix(&mut self, sound_id: Uuid) {
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };
        let Some(clips) = self.collect_trim_timeline_render_clips(sound_id) else {
            return;
        };
        let sound = self.sounds[index].clone();
        if let Ok(preview_path) = Storage::export_timeline_mix_preview_at(
            self.storage.root_dir(),
            &sound,
            &clips,
            0.0,
        ) {
            if let Some(audio) = self.audio.as_mut() {
                audio.evict_cached_audio(&preview_path);
            }
            self.trim_timeline_preview_path = Some(preview_path);
            self.trim_timeline_preview_start_secs = 0.0;
            self.trim_timeline_preview_dirty = false;
        }
    }

    pub(super) fn refresh_trim_timeline_preview_after_edit(&mut self, sound_id: Uuid) {
        let Some(timeline_state) = self
            .trim_timeline_state
            .as_ref()
            .filter(|state| state.sound_id == sound_id)
        else {
            return;
        };
        let current_playhead_secs = timeline_state.playhead_secs.max(0.0);
        self.trim_timeline_preview_dirty = true;

        // Capture what we need for background thread
        let Some(index) = self.sounds.iter().position(|s| s.id == sound_id) else {
            return;
        };
        let Some(clips) = self.collect_trim_timeline_render_clips(sound_id) else {
            return;
        };
        let sound = self.sounds[index].clone();
        let root_dir = self.storage.root_dir().to_path_buf();
        let tx = self.timeline_mix_tx.clone();
        let resume_secs = current_playhead_secs.max(0.0);

        // Build mix in background; UI stays responsive
        self.pending_timeline_mix_restart = Some((sound_id, resume_secs));
        thread::spawn(move || {
            match Storage::export_timeline_mix_preview_at(&root_dir, &sound, &clips, resume_secs) {
                Ok(preview_path) => {
                    let _ = tx.send(TimelineMixMessage::Ready { sound_id, preview_path, resume_secs });
                }
                Err(_) => {
                    let _ = tx.send(TimelineMixMessage::Failed);
                }
            }
        });
    }

    pub(super) fn poll_timeline_mix_jobs(&mut self, ctx: &Context) {
        while let Ok(msg) = self.timeline_mix_rx.try_recv() {
            match msg {
                TimelineMixMessage::Ready { sound_id, preview_path, resume_secs } => {
                    if self.pending_timeline_mix_restart != Some((sound_id, resume_secs)) {
                        let _ = fs::remove_file(preview_path);
                        continue;
                    }
                    self.pending_timeline_mix_restart = None;
                    if let Some(audio) = self.audio.as_mut() {
                        audio.evict_cached_audio(&preview_path);
                    }
                    let was_playing = self
                        .trim_timeline_state
                        .as_ref()
                        .is_some_and(|state| state.timeline_is_playing);
                    let current_live_secs = self
                        .trim_timeline_state
                        .as_ref()
                        .map(|state| state.playhead_secs)
                        .unwrap_or(resume_secs);
                    let previous_preview = self
                        .trim_timeline_preview_path
                        .replace(preview_path.clone());
                    self.trim_timeline_preview_start_secs = resume_secs;
                    self.trim_timeline_preview_dirty = false;
                    if was_playing {
                        if let Some(audio) = self.audio.as_mut() {
                            let local_secs = (current_live_secs - resume_secs).max(0.0);
                            let _ = audio.play_file_from(&preview_path, local_secs);
                        }
                    }
                    if previous_preview.as_ref() != Some(&preview_path)
                        && let Some(previous_preview) = previous_preview
                        && previous_preview
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("timeline-preview-"))
                    {
                        let _ = fs::remove_file(previous_preview);
                    }
                    ctx.request_repaint();
                }
                TimelineMixMessage::Failed => {
                    self.pending_timeline_mix_restart = None;
                }
            }
        }
    }


    pub(super) fn handle_trim_timeline_hotkeys(&mut self, ctx: &Context) {
        const BASE_VISIBLE_SECS: f32 = 12.0;
        let Some(sound_id) = self.timeline_mode_active_sound_id() else {
            return;
        };
        if self.has_modal_panel() || self.show_record_review_panel {
            return;
        }

        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            let is_currently_playing = self
                .trim_timeline_state
                .as_ref()
                .is_some_and(|state| state.timeline_is_playing);

            if is_currently_playing {
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.timeline_is_playing = false;
                }
                if let Some(audio) = self.audio.as_mut() {
                    audio.pause();
                }
            } else {
                let timeline_restart_secs = self
                    .trim_timeline_state
                    .as_ref()
                    .filter(|state| state.sound_id == sound_id)
                    .map(|state| {
                        let total_duration = self.trim_timeline_total_duration(state);
                        if state.playhead_secs >= total_duration - 0.05 {
                            0.0
                        } else {
                            state.playhead_secs.max(0.0)
                        }
                    })
                    .unwrap_or(0.0);

                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.timeline_is_playing = true;
                    state.playhead_secs = timeline_restart_secs;
                }
                self.preview_timeline_mix_from_position(sound_id, timeline_restart_secs);
            }
            ctx.request_repaint();
        }

        let timeline_playhead_drag_active = ctx
            .data(|data| data.get_temp::<bool>(Self::trim_timeline_playhead_drag_id(sound_id)))
            .unwrap_or(false);

        if !timeline_playhead_drag_active {
            let is_playing = self
                .trim_timeline_state
                .as_ref()
                .is_some_and(|state| state.timeline_is_playing);

            if is_playing {
                let preview_ready = !self.trim_timeline_preview_dirty
                    && self.pending_timeline_mix_restart.is_none();
                let audio_pos = self.audio.as_ref().and_then(|audio| {
                    self.trim_timeline_preview_path
                        .as_ref()
                        .and_then(|path| audio.playback_position_secs_for_file(path))
                });
                let total_duration = self
                    .trim_timeline_state
                    .as_ref()
                    .map(|state| self.trim_timeline_total_duration(state))
                    .unwrap_or(0.25);

                if let Some(state) = self.trim_timeline_state.as_mut()
                    && let Some(audio_pos) = audio_pos
                {
                    state.playhead_secs = (self.trim_timeline_preview_start_secs + audio_pos)
                        .clamp(0.0, total_duration);
                    if state.playhead_secs >= total_duration - 0.001 {
                        state.playhead_secs = total_duration;
                        state.timeline_is_playing = false;
                    }
                } else if preview_ready && self.trim_timeline_preview_path.is_some() {
                    if let Some(state) = self.trim_timeline_state.as_mut() {
                        state.playhead_secs = total_duration;
                        state.timeline_is_playing = false;
                    }
                }
                if let Some(state) = self.trim_timeline_state.as_ref()
                    && !state.timeline_is_playing
                    && let Some(audio) = self.audio.as_mut()
                {
                    audio.stop();
                }
                ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
            }
        }
        let selected_clip_active = self
            .trim_timeline_state
            .as_ref()
            .is_some_and(|state| state.sound_id == sound_id && state.selected_clip_id.is_some());
        if selected_clip_active {
            ctx.memory_mut(|memory| memory.stop_text_input());
        }

        let ctrl_modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let command_z = ctx.input(|input| {
            input.key_pressed(egui::Key::Z)
                && (input.modifiers.ctrl || input.modifiers.command || input.modifiers.mac_cmd)
                && !input.modifiers.shift
                && !input.modifiers.alt
        });
        let command_shift_z = ctx.input(|input| {
            input.key_pressed(egui::Key::Z)
                && (input.modifiers.ctrl || input.modifiers.command || input.modifiers.mac_cmd)
                && input.modifiers.shift
                && !input.modifiers.alt
        });
        let command_c = ctx.input_mut(|input| input.consume_key(ctrl_modifiers, egui::Key::C))
            || ctx.input(|input| {
                input.events.iter().any(|event| matches!(event, egui::Event::Copy))
                    || (input.key_pressed(egui::Key::C)
                        && (input.modifiers.command || input.modifiers.mac_cmd)
                        && !input.modifiers.ctrl
                        && !input.modifiers.shift
                        && !input.modifiers.alt)
            });
        let physical_ctrl_v_down = platform::hotkey::physical_hotkey_down(Hotkey {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
            key: egui::Key::V,
        });
        let physical_ctrl_v_pressed = ctx.data_mut(|data| {
            let hotkey_id = Self::trim_timeline_physical_paste_hotkey_id(sound_id);
            let was_down = data.get_temp::<bool>(hotkey_id).unwrap_or(false);
            data.insert_temp(hotkey_id, physical_ctrl_v_down);
            physical_ctrl_v_down && !was_down
        });
        let logical_command_v_down = ctx.input(|input| {
            input.key_down(egui::Key::V)
                && (input.modifiers.ctrl || input.modifiers.command || input.modifiers.mac_cmd)
                && !input.modifiers.shift
                && !input.modifiers.alt
        });
        let command_v_raw = physical_ctrl_v_pressed
            || ctx.input_mut(|input| input.consume_key(ctrl_modifiers, egui::Key::V))
            || ctx.input(|input| {
                input.key_pressed(egui::Key::V)
                    && (input.modifiers.command || input.modifiers.mac_cmd)
                    && !input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt
            });
        let command_v = ctx.data_mut(|data| {
            let latch_id = Self::trim_timeline_paste_shortcut_latch_id(sound_id);
            let latched = data.get_temp::<bool>(latch_id).unwrap_or(false);
            if !(physical_ctrl_v_down || logical_command_v_down) {
                data.insert_temp(latch_id, false);
            }
            if command_v_raw && !latched {
                data.insert_temp(latch_id, true);
                true
            } else {
                false
            }
        });
        let split_shortcut =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::B));

        if command_shift_z
            && let Some(snapshot) = self.trim_timeline_redo_stack.pop()
        {
            if let Some(current) = self.trim_timeline_state.clone() {
                self.trim_timeline_undo_stack.push(current);
            }
            self.trim_timeline_state = Some(snapshot);
            ctx.request_repaint();
            return;
        }
        if command_z
            && let Some(snapshot) = self.trim_timeline_undo_stack.pop()
        {
            if let Some(current) = self.trim_timeline_state.clone() {
                self.trim_timeline_redo_stack.push(current);
            }
            self.trim_timeline_state = Some(snapshot);
            ctx.request_repaint();
            return;
        }

        if command_c {
            if self.trim_timeline_copy_selected_clip(sound_id) {
                ctx.request_repaint();
            }
            return;
        }

        if command_v {
            self.trim_timeline_paste_copied_clip(ctx, sound_id);
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
            self.trim_timeline_delete_selected_clip(ctx, sound_id);
            return;
        }

        if split_shortcut {
            self.trim_timeline_split_selected_clip_at_playhead(ctx, sound_id);
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
            let timeline_restart_secs = self
                .trim_timeline_state
                .as_ref()
                .filter(|state| state.sound_id == sound_id)
                .map(|state| {
                    let total_duration = self.trim_timeline_total_duration(state);
                    if state.playhead_secs >= total_duration - 0.05 {
                        0.0
                    } else {
                        state.playhead_secs.max(0.0)
                    }
                })
                .unwrap_or(0.0);
            if is_playing && !is_paused {
                if let Some(audio) = self.audio.as_mut() {
                    audio.pause();
                }
            } else if is_playing && is_paused {
                if self.trim_timeline_preview_dirty {
                    self.stop_preview();
                    self.preview_timeline_mix_from_position(sound_id, timeline_restart_secs);
                } else if let Some(audio) = self.audio.as_mut() {
                    audio.resume();
                }
            } else if self.timeline_mode_active_sound_id().is_some() {
                self.preview_timeline_mix_from_position(sound_id, timeline_restart_secs);
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
        let fallback_sound_id = self
            .timeline_mode_active_sound_id()
            .or(self.selected)
            .or_else(|| self.sounds.first().map(|sound| sound.id))
            .unwrap_or_else(Uuid::nil);
        let sound_id = self.sync_trim_timeline_state_for(fallback_sound_id);
        let Some(index) = self.sounds.iter().position(|sound| sound.id == sound_id) else {
            return;
        };

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
        let timeline_output_name = timeline_clips.as_ref().map(|_| {
            let entered = self.trim_commit_output_name.trim();
            if entered.is_empty() {
                next_default_sound_name(self.sounds.iter().map(|sound| sound.name.as_str()))
            } else {
                entered.to_owned()
            }
        });
        if timeline_clips.is_some() {
            self.stop_preview();
        }
        let root_dir = self.storage.root_dir().to_path_buf();
        let tx = self.trim_commit_tx.clone();
        self.trim_commit_inflight.insert(sound_id);
        thread::spawn(move || {
            let result = if let Some(timeline_clips) = timeline_clips {
                Storage::commit_timeline_mix_at(
                    &root_dir,
                    &sound,
                    &timeline_clips,
                    keep_old,
                    timeline_output_name.as_deref().unwrap_or("Sound 1"),
                )
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

        self.stop_preview();

        if ready_marker.exists() {
            let mut frame_paths = fs::read_dir(&frames_dir)
                .with_context(|| format!("unable to read {}", frames_dir.display()))?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("ppm"))
                .collect::<Vec<_>>();
            frame_paths.sort();
            if !frame_paths.is_empty() {
                self.video_viewer = Some(VideoViewerState {
                    video: video.clone(),
                    frame_paths,
                    audio_path,
                    progress: 0.0,
                    current_frame: None,
                    receiver: None,
                });
                self.load_video_frame_texture(ctx, 0)?;
                let _ = self.play_video_viewer_from_current_playhead();
                ctx.request_repaint();
                self.clear_status();
                return Ok(());
            }
        }

        // ponytail: Generate downscaled preview frames and audio WAV in background thread to prevent UI freezing
        let (tx, rx) = std::sync::mpsc::channel();
        self.video_viewer = Some(VideoViewerState {
            video: video.clone(),
            frame_paths: Vec::new(),
            audio_path: audio_path.clone(),
            progress: 0.0,
            current_frame: None,
            receiver: Some(rx),
        });
        ctx.request_repaint();
        self.clear_status();

        let preview_fps = 20;
        let frame_pattern = frames_dir.join("frame_%05d.ppm");
        thread::spawn(move || {
            let result = (|| -> Result<Vec<PathBuf>> {
                if preview_root.exists() {
                    let _ = fs::remove_dir_all(&preview_root);
                }
                fs::create_dir_all(&frames_dir)
                    .with_context(|| format!("unable to create {}", frames_dir.display()))?;

                let filter = format!(
                    "fps={preview_fps},scale='min(720,iw)':'min(430,ih)':force_original_aspect_ratio=decrease"
                );
                Self::run_ffmpeg_command(
                    &ffmpeg_path,
                    [
                        "-y",
                        "-i",
                        &source_path.to_string_lossy(),
                        "-vf",
                        &filter,
                        "-pix_fmt",
                        "rgb24",
                        &frame_pattern.to_string_lossy(),
                        "-vn",
                        "-acodec",
                        "pcm_s16le",
                        &audio_path.to_string_lossy(),
                    ],
                )?;
                fs::write(&ready_marker, b"ok")
                    .with_context(|| format!("unable to write {}", ready_marker.display()))?;

                let mut frame_paths = fs::read_dir(&frames_dir)
                    .with_context(|| format!("unable to read {}", frames_dir.display()))?
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("ppm"))
                    .collect::<Vec<_>>();
                frame_paths.sort();
                if frame_paths.is_empty() {
                    anyhow::bail!("video preview is empty");
                }
                Ok(frame_paths)
            })();
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });

        Ok(())
    }

    pub(super) fn poll_video_viewer_jobs(&mut self, ctx: &Context) {
        let Some(viewer) = self.video_viewer.as_mut() else {
            return;
        };
        let Some(receiver) = viewer.receiver.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(frame_paths)) => {
                viewer.frame_paths = frame_paths;
                viewer.receiver = None;
                if let Err(error) = self.load_video_frame_texture(ctx, 0) {
                    self.set_error_status(error);
                }
                if let Err(error) = self.play_video_viewer_from_current_playhead() {
                    self.set_error_status(error);
                }
                ctx.request_repaint();
            }
            Ok(Err(error_msg)) => {
                self.video_viewer = None;
                self.set_error_status(anyhow::anyhow!(error_msg));
                ctx.request_repaint();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(JOB_POLL_REPAINT_MS));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.video_viewer = None;
                self.set_error_status(anyhow::anyhow!("video preview generation terminated unexpectedly"));
                ctx.request_repaint();
            }
        }
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
        if viewer.frame_paths.is_empty() {
            return Ok(());
        }
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
        if let Some((current_idx, texture, current_size)) = viewer.current_frame.as_mut() {
            *current_idx = frame_index;
            *current_size = size;
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            let texture = ctx.load_texture(
                format!("video-frame-{}", viewer.video.id),
                image,
                egui::TextureOptions::LINEAR,
            );
            viewer.current_frame = Some((frame_index, texture, size));
        }
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
            .trim_timeline_state
            .as_ref()
            .is_some_and(|state| state.enabled && state.timeline_is_playing)
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
        if self.timeline_mode_active_sound_id().is_some() {
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

    pub(super) fn trim_timeline_playhead_drag_id(sound_id: Uuid) -> egui::Id {
        egui::Id::new((sound_id, "trim-timeline-playhead-drag"))
    }

    pub(super) fn trim_timeline_physical_paste_hotkey_id(sound_id: Uuid) -> egui::Id {
        egui::Id::new((sound_id, "trim-timeline-physical-paste-hotkey"))
    }

    pub(super) fn trim_timeline_paste_shortcut_latch_id(sound_id: Uuid) -> egui::Id {
        egui::Id::new((sound_id, "trim-timeline-paste-shortcut-latch"))
    }

    pub(super) fn trim_timeline_clip_drag_snapshot_id(
        sound_id: Uuid,
        clip_id: Uuid,
    ) -> egui::Id {
        egui::Id::new((sound_id, clip_id, "trim-timeline-clip-drag-snapshot"))
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
                            let refresh_timeline = self.trim_timeline_state.as_ref().is_some_and(|state| {
                                state.rows.iter().flat_map(|row| row.clips.iter()).any(|clip| {
                                    clip.source_sound_id == sound_id
                                        && match kind {
                                            SeparationStemKind::Vocal => clip.audio.vocal_only,
                                            SeparationStemKind::Music => clip.audio.music_only,
                                        }
                                })
                            });
                            if refresh_timeline
                                && let Some(timeline_sound_id) = self.timeline_mode_active_sound_id()
                            {
                                self.refresh_trim_timeline_preview_after_edit(timeline_sound_id);
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
        if let Some(state) = self.trim_timeline_state.as_mut() {
            state.timeline_is_playing = false;
        }
        self.myinstants_preview_audio_url = None;
        self.pending_preview_after_preload = None;
        self.trim_timeline_scrub_resume_pending = false;
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
                                ui.spacing_mut().item_spacing.x = 8.0;
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
        self.handle_trim_undo_redo(ctx);
        if let Some(sound_id) = self.timeline_mode_active_sound_id() {
            self.draw_timeline_workspace(ui, ctx, sound_id);
            return;
        }

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
        let copy_feedback_active = self.sound_copy_feedback_active(ctx, sound_id);
        let copied_label = self.t("common.copied");
        if copy_feedback_active {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

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
                let row_gap = 8.0;
                let controls_width = 52.0 * 5.0 + row_gap * 4.0;
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
                            ui.spacing_mut().item_spacing.x = row_gap;
                            if Self::icon_action(ui, [52.0, 34.0], 0xe872, false, false).clicked() {
                                delete_request = true;
                            }
                            let spn = ui.add_sized(
                                [52.0, 34.0],
                                Self::action_button(RichText::new("SPN").size(12.0), false, false),
                            );
                            Self::decorate_button_response(ui, &spn);
                            if spn.clicked() {
                                open_spn_export_request = true;
                            }
                            if Self::icon_action(ui, [52.0, 34.0], 0xe14e, false, false).clicked() {
                                commit_trim_request = true;
                            }
                            let copy_button = Self::icon_action(
                                ui,
                                [52.0, 34.0],
                                if copy_feedback_active { 0xe5ca } else { 0xe14d },
                                copy_feedback_active,
                                copy_feedback_active,
                            );
                            if copy_feedback_active {
                                egui::show_tooltip_at(
                                    ui.ctx(),
                                    ui.layer_id(),
                                    copy_button.id.with("copy-success"),
                                    copy_button.rect.left_bottom() + vec2(0.0, 4.0),
                                    |ui| {
                                    ui.label(&copied_label);
                                    },
                                );
                            }
                            if copy_button.clicked() {
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
                                    ui.spacing_mut().item_spacing.x = 8.0;
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
            });

        let sound_id = self.sounds[index].id;
        let sound_duration = self.sounds[index].safe_duration();
        self.trim_timeline_zoom = trim_timeline_zoom;
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
            self.copy_selected_processed_sound(ctx);
        }

        if normalize_request {
            self.start_normalize_job(sound_id);
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
            self.trim_commit_output_name.clear();
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

    fn draw_timeline_workspace(&mut self, ui: &mut Ui, ctx: &Context, sound_id: Uuid) {
        let sound_id = self.sync_trim_timeline_state_for(sound_id);
        let mut save_mix_request = false;

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
                Frame::new()
                    .fill(Self::panel_fill())
                    .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                    .corner_radius(30.0)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ScrollArea::vertical()
                            .id_salt(("trim-timeline-workspace-scroll", sound_id))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                save_mix_request |= self.render_trim_composer(ui, ctx, sound_id);
                            });
                    });
            });

        if save_mix_request {
            self.trim_commit_output_name.clear();
            self.show_trim_commit_panel = true;
        }
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
                        Sense::hover(),
                    );
                    let middle_region_response = ui.interact(
                        middle_region_rect,
                        ui.make_persistent_id((sound.id, "trim-delete-middle")),
                        Sense::hover(),
                    );
                    let right_region_response = ui.interact(
                        right_region_rect,
                        ui.make_persistent_id((sound.id, "trim-delete-right")),
                        Sense::hover(),
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
                        && ui.input(|input| input.pointer.secondary_clicked())
                    {
                        hovered_delete_region
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
                        && (start_response.clicked()
                            || start_response.dragged()
                            || start_response.is_pointer_button_down_on())
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
                        && (end_response.clicked()
                            || end_response.dragged()
                            || end_response.is_pointer_button_down_on())
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
                        && (response.clicked()
                            || response.dragged()
                            || response.is_pointer_button_down_on())
                    {
                        let ratio = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        *preview_cursor_secs = (ratio * duration)
                            .clamp(sound.display_trim_start(), sound.display_trim_end());
                        if response.clicked() {
                            seek_requested = true;
                        }
                        if response.dragged() || response.is_pointer_button_down_on() {
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
        const TRIM_TIMELINE_DELETE_ANIM_SECS: f32 = 0.16;

        self.sync_trim_timeline_state_for(sound_id);
        let Some(state_snapshot) = self.trim_timeline_state.as_ref().cloned() else {
            return false;
        };

        let next_enabled = state_snapshot.enabled;
        let mut remove_row = None;
        let mut save_request = false;
        let mut timeline_state_changed = false;
        let mut zoom = self.trim_timeline_zoom;
        let delete_anim_now = Instant::now();
        let selected_clip_snapshot = Self::trim_timeline_selected_clip_from_state(&state_snapshot);
        let clip_selected = selected_clip_snapshot.is_some();
        let can_edit_selected_clip = selected_clip_snapshot
            .as_ref()
            .and_then(|(_, _, clip)| {
                Self::trim_timeline_cut_local_time_from_playhead(&state_snapshot, clip)
            })
            .is_some();
        let can_paste_clip = self.trim_timeline_copied_clip.is_some();
        let mut toolbar_trim_left = false;
        let mut toolbar_trim_right = false;
        let mut toolbar_copy_clip = false;
        let mut toolbar_paste_clip = false;
        let mut toolbar_split_clip = false;
        let mut toolbar_delete_clip = false;
        let mut normalize_selected = false;
        let mut selected_stem_request = None;

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
            if self.pending_timeline_mix_restart.is_some() {
                ui.add_space(6.0);
                ui.add(egui::Spinner::new().size(14.0));
            }
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
                ui.set_max_width(280.0);
                ui.label(
                    RichText::new("Timeline mix shortcuts")
                        .size(13.0)
                        .color(Self::strong_text_color())
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label("Space: play or pause");
                ui.label("S: restart from beginning");
                ui.label("A / D: pan timeline left or right");
                ui.label("B: split selected clip at playhead");
                ui.label("Delete: delete selected clip");
                ui.label("Ctrl + C: copy selected clip");
                ui.label("Ctrl + V: paste clip");
                ui.label("Ctrl + Z: undo");
                ui.label("Ctrl + Shift + Z: redo");
                ui.label("Ctrl + mouse wheel: zoom");
            });
            ui.add_space(10.0);

            let trim_left_button = ui
                .add_enabled_ui(can_edit_selected_clip, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe5c4, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Trim left at playhead");
            Self::decorate_button_response(ui, &trim_left_button);
            if trim_left_button.clicked() {
                toolbar_trim_left = true;
            }

            let trim_right_button = ui
                .add_enabled_ui(can_edit_selected_clip, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe5c8, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Trim right at playhead");
            Self::decorate_button_response(ui, &trim_right_button);
            if trim_right_button.clicked() {
                toolbar_trim_right = true;
            }

            let copy_clip_button = ui
                .add_enabled_ui(clip_selected, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe14d, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Copy selected clip");
            Self::decorate_button_response(ui, &copy_clip_button);
            if copy_clip_button.clicked() {
                toolbar_copy_clip = true;
            }

            let paste_clip_button = ui
                .add_enabled_ui(can_paste_clip, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe14f, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Paste copied clip at playhead");
            Self::decorate_button_response(ui, &paste_clip_button);
            if paste_clip_button.clicked() {
                toolbar_paste_clip = true;
            }

            let split_clip_button = ui
                .add_enabled_ui(can_edit_selected_clip, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe14e, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Split selected clip at playhead");
            Self::decorate_button_response(ui, &split_clip_button);
            if split_clip_button.clicked() {
                toolbar_split_clip = true;
            }

            let delete_clip_button = ui
                .add_enabled_ui(clip_selected, |ui| {
                    ui.add_sized(
                        [32.0, 32.0],
                        Self::action_button_with_radius(
                            Self::icon(0xe872, 16.0, Self::strong_text_color()),
                            false,
                            false,
                            8,
                        ),
                    )
                })
                .inner
                .on_hover_text("Delete selected clip");
            Self::decorate_button_response(ui, &delete_clip_button);
            if delete_clip_button.clicked() {
                toolbar_delete_clip = true;
            }

            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let snap_enabled = state_snapshot.snap_enabled;
                let snap_button = ui.add_sized(
                    [32.0, 32.0],
                    Self::action_button_with_radius(
                        Self::icon(0xe5d5, 16.0, Self::strong_text_color()),
                        snap_enabled,
                        false,
                        8,
                    ),
                );
                let snap_button = snap_button.on_hover_text("Toggle snap");
                Self::decorate_button_response(ui, &snap_button);
                if snap_button.clicked()
                    && let Some(state) = self.trim_timeline_state.as_mut()
                {
                    state.snap_enabled = !state.snap_enabled;
                }
                let save_button = ui.add_sized(
                    [32.0, 32.0],
                    Self::action_button_with_radius(
                        Self::icon(0xe161, 16.0, Self::strong_text_color()),
                        false,
                        false,
                        8,
                    ),
                );
                let save_button = save_button.on_hover_text("Save timeline mix");
                Self::decorate_button_response(ui, &save_button);
                if save_button.clicked() {
                    save_request = true;
                }
            });
        });

        let clip_controls_enabled = selected_clip_snapshot.is_some();
        let mut audio = selected_clip_snapshot
            .as_ref()
            .map(|(_, _, clip)| clip.audio.clone())
            .unwrap_or_default();
        let before = audio.clone();
        let mut controls_commit = false;
        Frame::new()
                .fill(Self::panel_fill())
                .stroke(Stroke::new(1.0, Self::subtle_border_color()))
                .corner_radius(14.0)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.add_enabled_ui(clip_controls_enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(Self::icon(0xe050, 15.0, Self::muted_text_color()));
                        let (_, volume_commit) = Self::click_slider_deferred(
                            ui, &mut audio.volume, 0.0..=5.0, 0.0, vec2(108.0, 22.0),
                        );
                        controls_commit |= volume_commit;
                        ui.label(RichText::new(format!("{:.2}x", audio.volume)).size(11.0));
                        ui.add_space(12.0);
                        ui.label(Self::icon(0xe9e4, 15.0, Self::muted_text_color()));
                        let (_, speed_commit) = Self::click_slider_deferred(
                            ui, &mut audio.speed, 0.25..=2.0, 0.0, vec2(108.0, 22.0),
                        );
                        controls_commit |= speed_commit;
                        ui.label(RichText::new(format!("{:.2}x", audio.speed)).size(11.0));
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        let mut toggle = |label: &str, enabled: &mut bool| {
                            let response = ui.add_sized(
                                [86.0, 26.0],
                                Button::new(RichText::new(label).size(10.5))
                                    .fill(if *enabled { Color32::from_rgb(125, 34, 88) } else { Self::surface_fill() })
                                    .stroke(Stroke::new(1.0, Self::border_color()))
                                    .corner_radius(10.0),
                            );
                            Self::decorate_button_response(ui, &response);
                            if response.clicked() {
                                *enabled = !*enabled;
                                controls_commit = true;
                            }
                        };
                        toggle("Reverb", &mut audio.reverb_enabled);
                        toggle("Telephone", &mut audio.telephone_enabled);
                        toggle("Distortion", &mut audio.distortion_enabled);
                        toggle("Echo", &mut audio.echo_enabled);
                        toggle("Underwater", &mut audio.underwater_enabled);
                        toggle("Robot", &mut audio.robot_enabled);
                        toggle("Pitch Shift", &mut audio.pitch_shift_enabled);
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let action = |label: &str| {
                            Button::new(
                                RichText::new(label)
                                    .size(10.5)
                                    .color(Self::strong_text_color()),
                            )
                            .fill(Self::surface_fill())
                            .stroke(Stroke::new(1.0, Self::border_color()))
                            .corner_radius(10.0)
                        };
                        if ui.add_sized([86.0, 26.0], action("Normalize")).clicked() {
                            normalize_selected = true;
                        }
                        if ui.add_sized([86.0, 26.0], action("Vocal")).clicked() {
                            audio.vocal_only = true;
                            audio.music_only = false;
                            controls_commit = true;
                            selected_stem_request = Some(SeparationStemKind::Vocal);
                        }
                        if ui.add_sized([96.0, 26.0], action("Instrumental")).clicked() {
                            audio.music_only = true;
                            audio.vocal_only = false;
                            controls_commit = true;
                            selected_stem_request = Some(SeparationStemKind::Music);
                        }
                    });
                });
                });
        if clip_controls_enabled && (audio != before || controls_commit) {
                let selected_ids = if state_snapshot.selected_clip_ids.is_empty() {
                    state_snapshot.selected_clip_id.into_iter().collect::<HashSet<_>>()
                } else {
                    state_snapshot.selected_clip_ids.clone()
                };
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    for clip in state.rows.iter_mut().flat_map(|row| row.clips.iter_mut()) {
                        if selected_ids.contains(&clip.id) {
                            clip.audio.apply_changed_from(&before, &audio);
                        }
                    }
                }
                timeline_state_changed |= controls_commit;
            }
        ui.add_space(8.0);

        let selected_ids = if state_snapshot.selected_clip_ids.is_empty() {
            state_snapshot.selected_clip_id.into_iter().collect::<HashSet<_>>()
        } else {
            state_snapshot.selected_clip_ids.clone()
        };
        if normalize_selected {
            let mut clips_by_sound = HashMap::<Uuid, Vec<Uuid>>::new();
            for clip in state_snapshot.rows.iter().flat_map(|row| row.clips.iter()) {
                if selected_ids.contains(&clip.id) {
                    clips_by_sound.entry(clip.source_sound_id).or_default().push(clip.id);
                }
            }
            for (source_sound_id, clip_ids) in clips_by_sound {
                if let Some(sound) = self.sounds.iter().find(|sound| sound.id == source_sound_id) {
                    let path = sound.playback_asset_path(self.storage.root_dir());
                    let tx = self.normalize_tx.clone();
                    thread::spawn(move || {
                        let result = calculate_normalization_gain(&path).map_err(|error| error.to_string());
                        let _ = tx.send(NormalizeMessage::TimelineFinished { clip_ids, result });
                    });
                }
            }
        }
        if let Some(kind) = selected_stem_request
            && let Some(source_sound_id) = state_snapshot
                .rows
                .iter()
                .flat_map(|row| row.clips.iter())
                .filter(|clip| selected_ids.contains(&clip.id))
                .map(|clip| clip.source_sound_id)
                .find(|source_sound_id| {
                    self.sounds
                        .iter()
                        .find(|sound| sound.id == *source_sound_id)
                        .is_some_and(|sound| match kind {
                            SeparationStemKind::Vocal => sound
                                .vocal_asset_path(self.storage.root_dir())
                                .is_none_or(|path| !path.exists()),
                            SeparationStemKind::Music => sound
                                .music_asset_path(self.storage.root_dir())
                                .is_none_or(|path| !path.exists()),
                        })
                })
        {
            match kind {
                SeparationStemKind::Vocal => self.start_library_vocal_separation(source_sound_id),
                SeparationStemKind::Music => self.start_library_music_separation(source_sound_id),
            }
        }
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

        let ruler_height = 32.0;
        let rows_top_padding = 40.0;
        let row_height = 92.0;
        let row_spacing = 0.0;
        let row_count = state_snapshot.rows.len().max(3);
        let viewport_width = ui.available_width().max(320.0);
        let viewport_height = rows_top_padding
            + row_count as f32 * row_height
            + row_count.saturating_sub(1) as f32 * row_spacing;
        let mut view_start_secs = self.trim_timeline_view_start_secs.max(0.0);
        let mut requested_view_start_secs: Option<f32> = None;
        let (viewport_rect, _) =
            ui.allocate_exact_size(
                vec2(
                    viewport_width,
                    viewport_height.max(rows_top_padding + row_height),
                ),
                Sense::hover(),
            );
        let viewport_clip_rect = viewport_rect.intersect(ui.clip_rect());
        let viewport_painter = ui.painter().with_clip_rect(viewport_clip_rect);
        viewport_painter.rect_filled(viewport_rect, 0.0, Self::surface_fill());
        viewport_painter.rect_stroke(
            viewport_rect,
            0.0,
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
        let mut view_end_secs = view_start_secs + visible_duration;
        let shared_timeline_left = viewport_rect.left() + 92.0;
        let shared_timeline_right = viewport_rect.right() - 46.0;
        let shared_timeline_width = (shared_timeline_right - shared_timeline_left).max(1.0);
        let snap_threshold_secs = ((visible_duration / shared_timeline_width) * 18.0).clamp(0.08, 1.0);
        let mut global_snap_x = None;
        let mut timeline_playhead_secs = state_snapshot.playhead_secs.max(0.0);
        if state_snapshot.timeline_is_playing {
            if timeline_playhead_secs >= view_end_secs - 0.05 {
                view_start_secs = timeline_playhead_secs;
                view_end_secs = view_start_secs + visible_duration;
            } else if timeline_playhead_secs < view_start_secs {
                view_start_secs = timeline_playhead_secs;
                view_end_secs = view_start_secs + visible_duration;
            }
        }
        let timeline_playhead_drag_id = Self::trim_timeline_playhead_drag_id(sound_id);
        let timeline_snap_points = Self::trim_timeline_collect_snap_points(&state_snapshot.rows, None);
        let timeline_snap_enabled = state_snapshot.snap_enabled;
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
        let ruler_rect = Rect::from_min_max(
            Pos2::new(shared_timeline_left, viewport_rect.top() + 4.0),
            Pos2::new(shared_timeline_right, viewport_rect.top() + ruler_height),
        );
        viewport_painter.rect_filled(ruler_rect, 0.0, Self::input_fill());
        viewport_painter.line_segment(
            [
                Pos2::new(ruler_rect.left(), ruler_rect.bottom() - 8.0),
                Pos2::new(ruler_rect.right(), ruler_rect.bottom() - 8.0),
            ],
            Stroke::new(1.0, Self::subtle_border_color()),
        );
        let first_tick = (view_start_secs / tick_step).floor() * tick_step;
        let mut tick_time = first_tick;
        while tick_time <= view_end_secs + tick_step {
            if tick_time >= view_start_secs {
                let tick_ratio =
                    ((tick_time - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                let tick_x = ruler_rect.left() + tick_ratio * ruler_rect.width();
                viewport_painter.line_segment(
                    [
                        Pos2::new(tick_x, ruler_rect.bottom() - 12.0),
                        Pos2::new(tick_x, ruler_rect.bottom() - 4.0),
                    ],
                    Stroke::new(1.0, Self::subtle_border_color()),
                );
                viewport_painter.text(
                    Pos2::new(tick_x + 4.0, ruler_rect.top() + 4.0),
                    Align2::LEFT_TOP,
                    format_time(tick_time.max(0.0)),
                    FontId::proportional(9.5),
                    Self::muted_text_color(),
                );
            }
            tick_time += tick_step;
        }
        let ruler_response = ui.interact(
            ruler_rect,
            ui.id().with(("trim-timeline-ruler", sound_id)),
            Sense::click_and_drag(),
        );
        if ruler_response.hovered() || ruler_response.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if ruler_response.drag_started() {
            ui.ctx().memory_mut(|memory| memory.stop_text_input());
            let preview_path = self.trim_timeline_preview_path.clone();
            let preview_is_playing = self.audio.as_ref().is_some_and(|audio| {
                preview_path
                    .as_ref()
                    .is_some_and(|path| audio.is_playing_file(path) && !audio.is_paused())
            });
            self.trim_timeline_scrub_resume_pending = preview_is_playing;
            if preview_is_playing
                && let Some(audio) = self.audio.as_mut()
            {
                audio.pause();
            }
        }
        if let Some(pointer) = ruler_response.interact_pointer_pos()
            && (ruler_response.clicked()
                || ruler_response.dragged()
                || ruler_response.is_pointer_button_down_on())
        {
            let desired_secs = (view_start_secs
                + ((pointer.x - ruler_rect.left()) / ruler_rect.width()).clamp(0.0, 1.0)
                    * visible_duration)
                .clamp(0.0, workspace_duration.max(0.05));
            let (secs, snapped_point) = Self::trim_timeline_snap_playhead(
                desired_secs,
                &timeline_snap_points,
                timeline_snap_enabled,
                snap_threshold_secs,
            );
            if let Some(snap_point) = snapped_point {
                let snap_ratio =
                    ((snap_point - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                global_snap_x = Some(shared_timeline_left + snap_ratio * shared_timeline_width);
            }
            timeline_playhead_secs = secs;
            if let Some(state) = self.trim_timeline_state.as_mut() {
                state.playhead_secs = secs;
            }
            if ruler_response.dragged() || ruler_response.is_pointer_button_down_on() {
                ctx.data_mut(|data| data.insert_temp(timeline_playhead_drag_id, true));
            }
            if ruler_response.clicked() {
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
                self.set_trim_timeline_playhead(sound_id, secs);
                ctx.data_mut(|data| data.remove::<bool>(timeline_playhead_drag_id));
            }
        }
        if ruler_response.drag_stopped()
            && ctx
                .data(|data| data.get_temp::<bool>(timeline_playhead_drag_id))
                .unwrap_or(false)
        {
            self.commit_trim_timeline_playhead_after_scrub(sound_id, timeline_playhead_secs);
            ctx.data_mut(|data| data.remove::<bool>(timeline_playhead_drag_id));
        }
        if !ctx.input(|input| input.pointer.primary_down()) {
            let drag_active = ctx
                .data(|data| data.get_temp::<bool>(timeline_playhead_drag_id))
                .unwrap_or(false);
            if drag_active {
                let committed_secs = self
                    .trim_timeline_state
                    .as_ref()
                    .filter(|state| state.sound_id == sound_id)
                    .map(|state| state.playhead_secs)
                    .unwrap_or(timeline_playhead_secs);
                self.commit_trim_timeline_playhead_after_scrub(sound_id, committed_secs);
            }
            ctx.data_mut(|data| data.remove::<bool>(timeline_playhead_drag_id));
        }

        let marquee_origin_id = ui.id().with(("trim-timeline-marquee-origin", sound_id));
        let mut rendered_clip_rects = Vec::new();
        for row_index in 0..row_count {
            let row_snapshot = state_snapshot
                .rows
                .get(row_index)
                .cloned()
                .unwrap_or_default();
            let row = &row_snapshot;
            let row_top =
                viewport_rect.top() + rows_top_padding + row_index as f32 * (row_height + row_spacing);
            let row_rect = Rect::from_min_size(
                Pos2::new(viewport_rect.left(), row_top),
                vec2(viewport_rect.width(), row_height),
            );
            if !row_rect.expand2(vec2(0.0, row_height)).intersects(viewport_clip_rect) {
                continue;
            }
            let painter = viewport_painter.clone();

            let label_rect = Rect::from_min_max(
                Pos2::new(row_rect.left() + 12.0, row_rect.top() + 6.0),
                Pos2::new(row_rect.left() + 84.0, row_rect.top() + 20.0),
            );
            painter.text(
                label_rect.left_top(),
                Align2::LEFT_TOP,
                format!("Row {}", row_index + 1),
                FontId::proportional(11.5),
                Self::muted_text_color(),
            );

            let row_is_muted = row.muted;
            let mute_rect = Rect::from_min_size(
                Pos2::new(row_rect.left() + 10.0, row_rect.top() + 22.0),
                vec2(58.0, 20.0),
            );
            let mute_bg = if row_is_muted {
                Color32::from_rgb(180, 40, 60)
            } else {
                Self::panel_fill()
            };
            painter.rect_filled(mute_rect, 6.0, mute_bg);
            painter.rect_stroke(
                mute_rect,
                6.0,
                Stroke::new(1.0, if row_is_muted { Color32::from_rgb(220, 60, 80) } else { Self::border_color() }),
                StrokeKind::Outside,
            );
            let mute_icon = if row_is_muted { 0xe04f } else { 0xe050 };
            painter.text(
                Pos2::new(mute_rect.left() + 5.0, mute_rect.center().y),
                Align2::LEFT_CENTER,
                char::from_u32(mute_icon).unwrap_or('?').to_string(),
                FontId::proportional(12.0),
                if row_is_muted { Color32::WHITE } else { Self::muted_text_color() },
            );
            painter.text(
                Pos2::new(mute_rect.left() + 20.0, mute_rect.center().y),
                Align2::LEFT_CENTER,
                if row_is_muted { "Muted" } else { "Mute" },
                FontId::proportional(11.0),
                if row_is_muted { Color32::WHITE } else { Self::muted_text_color() },
            );
            let mute_response = ui.interact(
                mute_rect,
                ui.id().with(("trim-mix-mute-row", sound_id, row_index)),
                Sense::click(),
            );
            if mute_response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if mute_response.clicked() {
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    if let Some(r) = state.rows.get_mut(row_index) {
                        r.muted = !r.muted;
                        timeline_state_changed = true;
                    }
                }
            }

            let remove_row_rect = Rect::from_min_size(
                Pos2::new(row_rect.right() - 36.0, row_rect.center().y - 14.0),
                vec2(28.0, 28.0),
            );
            if row_index > 0 && row_index < state_snapshot.rows.len() {
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

            let timeline_radius = CornerRadius::ZERO;
            let timeline_rect = Rect::from_min_max(
                Pos2::new(shared_timeline_left, row_rect.top()),
                Pos2::new(shared_timeline_right, row_rect.bottom()),
            );
            let row_hovered = ctx
                .input(|input| input.pointer.hover_pos())
                .is_some_and(|pointer| timeline_rect.contains(pointer));
            let track_bg = if row_is_muted {
                Color32::from_rgb(18, 18, 22)
            } else {
                Self::input_fill()
            };
            painter.rect_filled(
                timeline_rect,
                timeline_radius,
                track_bg,
            );
            if row_is_muted {
                painter.rect_filled(
                    row_rect,
                    0.0,
                    Color32::from_rgba_premultiplied(0, 0, 0, 110),
                );
            }
            if row_hovered && pending_drag_sound.is_some() {
                painter.rect_stroke(
                    timeline_rect,
                    timeline_radius,
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 92)),
                    StrokeKind::Outside,
                );
            }
            let timeline_click_response = ui.interact(
                timeline_rect,
                ui.id().with(("trim-timeline-track", sound_id, row_index)),
                Sense::click_and_drag(),
            );
            if timeline_click_response.drag_started()
                && let Some(pointer) = timeline_click_response.interact_pointer_pos()
            {
                ctx.data_mut(|data| data.insert_temp(marquee_origin_id, pointer));
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.selected_clip_id = None;
                    state.selected_clip_ids.clear();
                }
            }
            if timeline_click_response.clicked()
                && let Some(pointer) = timeline_click_response.interact_pointer_pos()
            {
                ui.ctx().memory_mut(|memory| memory.stop_text_input());
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.selected_clip_id = None;
                    state.selected_clip_ids.clear();
                }
                let is_playing = self.audio.as_ref().is_some_and(|audio| {
                    self.trim_timeline_preview_path
                        .as_ref()
                        .is_some_and(|path| audio.is_playing_file(path))
                        && !audio.is_paused()
                });
                if !is_playing {
                    let desired_secs = (view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration)
                        .max(0.0);
                    let (secs, snapped_point) = Self::trim_timeline_snap_playhead(
                        desired_secs,
                        &timeline_snap_points,
                        timeline_snap_enabled,
                        snap_threshold_secs,
                    );
                    if let Some(snap_point) = snapped_point {
                        let snap_ratio =
                            ((snap_point - view_start_secs) / visible_duration).clamp(0.0, 1.0);
                        global_snap_x = Some(shared_timeline_left + snap_ratio * shared_timeline_width);
                    }
                    timeline_playhead_secs = secs;
                    self.set_trim_timeline_playhead(sound_id, secs);
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
                let delete_progress = self
                    .trim_timeline_clip_delete_animating
                    .get(&clip.id)
                    .map(|started| {
                        (delete_anim_now.saturating_duration_since(*started).as_secs_f32()
                            / TRIM_TIMELINE_DELETE_ANIM_SECS)
                            .clamp(0.0, 1.0)
                    });
                let clip_is_deleting = delete_progress.is_some();
                let clip_duration = Self::timeline_clip_duration(clip);
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
                rendered_clip_rects.push((clip.id, clip_rect));
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
                    if clip_is_deleting {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let drag_anchor_id = ui.id().with(("trim-mix-drag-anchor", sound_id, clip.id));
                let drag_snapshot_id =
                    Self::trim_timeline_clip_drag_snapshot_id(sound_id, clip.id);
                let left_edge_original_id =
                    ui.id().with(("trim-mix-left-edge-original", sound_id, clip.id));
                let left_edge_snapshot_id =
                    ui.id().with(("trim-mix-left-edge-snapshot", sound_id, clip.id));
                let right_edge_original_id =
                    ui.id().with(("trim-mix-right-edge-original", sound_id, clip.id));
                let right_edge_snapshot_id =
                    ui.id().with(("trim-mix-right-edge-snapshot", sound_id, clip.id));
                let edge_hit_width = clip_hit_rect.width().clamp(8.0, 16.0);
                let left_edge_hit_rect = Rect::from_min_max(
                    clip_hit_rect.left_top(),
                    Pos2::new(
                        (clip_hit_rect.left() + edge_hit_width).min(clip_hit_rect.center().x),
                        clip_hit_rect.bottom(),
                    ),
                );
                let right_edge_hit_rect = Rect::from_min_max(
                    Pos2::new(
                        (clip_hit_rect.right() - edge_hit_width).max(clip_hit_rect.center().x),
                        clip_hit_rect.top(),
                    ),
                    clip_hit_rect.right_bottom(),
                );
                let left_edge_response = ui.interact(
                    left_edge_hit_rect,
                    ui.id().with(("trim-mix-left-edge", sound_id, clip.id)),
                    if clip_is_deleting {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let right_edge_response = ui.interact(
                    right_edge_hit_rect,
                    ui.id().with(("trim-mix-right-edge", sound_id, clip.id)),
                    if clip_is_deleting {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let left_edge_can_restore = clip.clip_start_secs > 0.001;
                let right_edge_can_restore =
                    clip.clip_end_secs < self.trim_timeline_clip_source_duration(clip.source_sound_id) - 0.001;
                let left_edge_drag_started =
                    left_edge_response.drag_started_by(egui::PointerButton::Primary);
                let right_edge_drag_started =
                    right_edge_response.drag_started_by(egui::PointerButton::Primary);
                let left_edge_dragging = left_edge_response.dragged_by(egui::PointerButton::Primary);
                let right_edge_dragging =
                    right_edge_response.dragged_by(egui::PointerButton::Primary);
                let left_edge_drag_stopped =
                    left_edge_response.drag_stopped_by(egui::PointerButton::Primary);
                let right_edge_drag_stopped =
                    right_edge_response.drag_stopped_by(egui::PointerButton::Primary);
                let edge_drag_started = left_edge_drag_started || right_edge_drag_started;
                let edge_dragging = left_edge_dragging || right_edge_dragging;
                let edge_drag_stopped = left_edge_drag_stopped || right_edge_drag_stopped;
                let selected_clip = state_snapshot.selected_clip_id == Some(clip.id)
                    || state_snapshot.selected_clip_ids.contains(&clip.id);
                let rendered_clip_rect = if let Some(progress) = delete_progress {
                    let shrink_x = ((clip_rect.width() - 8.0).max(0.0) * 0.12).min(14.0) * progress;
                    let shrink_y =
                        ((clip_rect.height() - 8.0).max(0.0) * 0.28).min(16.0) * progress;
                    clip_rect.shrink2(vec2(shrink_x, shrink_y))
                } else {
                    clip_rect
                };
                let fill_color = if selected_clip {
                    Color32::from_rgba_premultiplied(92, 38, 71, 228)
                } else {
                    Color32::from_rgba_premultiplied(34, 28, 40, 220)
                }
                .linear_multiply(1.0 - delete_progress.unwrap_or(0.0) * 0.45);
                let stroke_color = if selected_clip {
                    Color32::from_rgb(255, 112, 181)
                } else if clip_response.hovered() && removable {
                    Color32::from_rgb(255, 182, 214)
                } else {
                    Self::border_color()
                }
                .linear_multiply(1.0 - delete_progress.unwrap_or(0.0) * 0.3);
                let title_color = if row_is_muted {
                    Color32::from_rgba_premultiplied(100, 95, 105, 120)
                } else {
                    Self::strong_text_color()
                }
                .linear_multiply(1.0 - delete_progress.unwrap_or(0.0) * 0.35);
                let waveform_color = if row_is_muted {
                    Color32::from_rgba_premultiplied(45, 40, 48, 60)
                } else {
                    Color32::from_rgb(241, 78, 162)
                }
                .linear_multiply(1.0 - delete_progress.unwrap_or(0.0) * 0.2);

                painter.rect_filled(
                    rendered_clip_rect,
                    0.0,
                    fill_color,
                );
                painter.rect_stroke(
                    rendered_clip_rect,
                    0.0,
                    Stroke::new(1.0, stroke_color),
                    StrokeKind::Outside,
                );
                if left_edge_dragging || (left_edge_can_restore && left_edge_response.hovered()) {
                    painter.line_segment(
                        [
                            Pos2::new(rendered_clip_rect.left() + 1.0, rendered_clip_rect.top() + 3.0),
                            Pos2::new(rendered_clip_rect.left() + 1.0, rendered_clip_rect.bottom() - 3.0),
                        ],
                        Stroke::new(2.0, Color32::from_rgba_premultiplied(108, 231, 255, 210)),
                    );
                }
                if right_edge_dragging || (right_edge_can_restore && right_edge_response.hovered()) {
                    painter.line_segment(
                        [
                            Pos2::new(rendered_clip_rect.right() - 1.0, rendered_clip_rect.top() + 3.0),
                            Pos2::new(rendered_clip_rect.right() - 1.0, rendered_clip_rect.bottom() - 3.0),
                        ],
                        Stroke::new(2.0, Color32::from_rgba_premultiplied(108, 231, 255, 210)),
                    );
                }
                let clip_speed = clip.audio.speed.clamp(0.25, 2.0);
                let visible_local_start = clip.clip_start_secs
                    + (visible_clip_start - clip_time_start).max(0.0) * clip_speed;
                let visible_local_end = clip.clip_start_secs
                    + (visible_clip_end - clip_time_start).max(0.0) * clip_speed;
                let waveform_bars = Self::trim_timeline_waveform_bars(clip_rect.width());
                let preview = Self::scale_waveform_for_volume(Self::timeline_clip_waveform_preview_from_samples(
                    sound,
                    &self.raw_sound_waveform_samples(sound),
                    visible_local_start,
                    visible_local_end,
                    waveform_bars,
                ), clip.audio.volume);
                if rendered_clip_rect.width() >= 18.0 {
                    let waveform_inset_x = 1.0;
                    let title_rect = Rect::from_min_max(
                        rendered_clip_rect.left_top() + vec2(8.0, 2.0),
                        Pos2::new(
                            (rendered_clip_rect.right() - 8.0).max(rendered_clip_rect.left() + 8.0),
                            rendered_clip_rect.top() + 15.0,
                        ),
                    );
                    let waveform_rect = Rect::from_min_max(
                        Pos2::new(
                            rendered_clip_rect.left() + waveform_inset_x,
                            rendered_clip_rect.top() + 15.0,
                        ),
                        Pos2::new(
                            (rendered_clip_rect.right() - waveform_inset_x)
                                .max(rendered_clip_rect.left() + waveform_inset_x),
                            rendered_clip_rect.bottom() - 4.0,
                        ),
                    );
                    Self::paint_timeline_waveform_columns(
                        &painter,
                        waveform_rect,
                        &preview,
                        waveform_color,
                        selected_clip,
                    );
                    if rendered_clip_rect.width() >= 52.0 {
                        let title_max_chars =
                            ((title_rect.width() / 7.0).floor() as usize).clamp(6, 72);
                        let title_text = Self::truncate_middle_ascii(&sound.name, title_max_chars);
                        painter.with_clip_rect(title_rect).text(
                            title_rect.left_top(),
                            Align2::LEFT_TOP,
                            title_text,
                            FontId::proportional(11.0),
                            title_color,
                        );
                    }
                }

                let volume_y = egui::lerp(
                    (rendered_clip_rect.bottom() - 5.0)..=(rendered_clip_rect.top() + 5.0),
                    (clip.audio.volume / 5.0).clamp(0.0, 1.0),
                );
                let volume_line_rect = Rect::from_min_max(
                    Pos2::new(rendered_clip_rect.left() + 3.0, volume_y - 5.0),
                    Pos2::new(rendered_clip_rect.right() - 3.0, volume_y + 5.0),
                );
                let volume_response = ui.interact(
                    volume_line_rect,
                    ui.id().with(("trim-mix-volume", sound_id, clip.id)),
                    Sense::drag(),
                )
                .on_hover_text("Drag up to increase volume, down to decrease");
                let volume_adjusting = volume_response.hovered() || volume_response.dragged();
                if (clip_response.hovered() || volume_response.dragged()) && !clip_is_deleting {
                    painter.line_segment(
                        [
                            Pos2::new(rendered_clip_rect.left() + 3.0, volume_y),
                            Pos2::new(rendered_clip_rect.right() - 3.0, volume_y),
                        ],
                        Stroke::new(2.0, Color32::from_rgb(108, 231, 255)),
                    );
                }
                if volume_response.dragged()
                    && let Some(pointer) = volume_response.interact_pointer_pos()
                {
                    let volume = ((rendered_clip_rect.bottom() - pointer.y)
                        / rendered_clip_rect.height().max(1.0)
                        * 5.0)
                        .clamp(0.0, 5.0);
                    let selected_ids = self
                        .trim_timeline_state
                        .as_ref()
                        .filter(|state| state.selected_clip_ids.contains(&clip.id))
                        .map(|state| state.selected_clip_ids.clone())
                        .filter(|selected| !selected.is_empty())
                        .unwrap_or_else(|| HashSet::from([clip.id]));
                    if let Some(state) = self.trim_timeline_state.as_mut() {
                        for target in state.rows.iter_mut().flat_map(|row| row.clips.iter_mut()) {
                            if selected_ids.contains(&target.id) {
                                target.audio.volume = volume;
                            }
                        }
                    }
                    ctx.request_repaint();
                }
                if volume_response.drag_stopped() {
                    timeline_state_changed = true;
                }

                if clip_is_deleting {
                    ctx.request_repaint();
                }
                let right_button_down = ctx.input(|input| input.pointer.secondary_down());
                let hover_pos = ctx.input(|input| input.pointer.hover_pos());
                let right_sweep_delete = !clip_is_deleting
                    && removable
                    && right_button_down
                    && hover_pos.is_some_and(|pointer| clip_hit_rect.contains(pointer));

                if volume_adjusting {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
                } else if edge_dragging || left_edge_response.hovered() || right_edge_response.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                } else if clip_response.dragged_by(egui::PointerButton::Primary) {
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                } else if clip_response.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }
                if !clip_is_deleting
                    && clip_response.clicked()
                {
                    ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    if let Some(state) = self.trim_timeline_state.as_mut() {
                        let additive = ctx.input(|input| input.modifiers.shift);
                        if additive {
                            if !state.selected_clip_ids.insert(clip.id) {
                                state.selected_clip_ids.remove(&clip.id);
                            }
                            state.selected_clip_id = state.selected_clip_ids.iter().next().copied();
                        } else {
                            state.selected_clip_ids.clear();
                            state.selected_clip_ids.insert(clip.id);
                            state.selected_clip_id = Some(clip.id);
                        }
                    }
                    let is_playing = self.audio.as_ref().is_some_and(|audio| {
                        self.trim_timeline_preview_path
                            .as_ref()
                            .is_some_and(|path| audio.is_playing_file(path))
                            && !audio.is_paused()
                    });
                    if !is_playing {
                        let pointer_time = clip_response
                            .interact_pointer_pos()
                            .map(|pointer| {
                                view_start_secs
                                    + ((pointer.x - timeline_rect.left()) / timeline_rect.width())
                                        .clamp(0.0, 1.0)
                                        * visible_duration
                            })
                            .unwrap_or(clip.start_secs);
                        timeline_playhead_secs = pointer_time;
                        self.set_trim_timeline_playhead(sound_id, pointer_time);
                    }
                }
                if !clip_is_deleting
                    && edge_drag_started
                {
                    ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    if let Some(state) = self.trim_timeline_state.as_mut() {
                        state.selected_clip_id = Some(clip.id);
                    }
                    if left_edge_drag_started {
                        if let Some(before) = self.trim_timeline_state.clone() {
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(left_edge_snapshot_id, before);
                                data.insert_temp(left_edge_original_id, clip.clone());
                            });
                        }
                    }
                    if right_edge_drag_started {
                        if let Some(before) = self.trim_timeline_state.clone() {
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(right_edge_snapshot_id, before);
                                data.insert_temp(right_edge_original_id, clip.clone());
                            });
                        }
                    }
                }
                if !clip_is_deleting
                    && clip_response.drag_started_by(egui::PointerButton::Primary)
                    && !edge_drag_started
                    && let Some(pointer) = clip_response.interact_pointer_pos()
                {
                    ui.ctx().memory_mut(|memory| memory.stop_text_input());
                    let pointer_time = view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration;
                    let grab_offset_secs = (pointer_time - clip.start_secs).clamp(0.0, clip_duration);
                    if let Some(before) = self.trim_timeline_state.clone() {
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(drag_anchor_id, grab_offset_secs);
                            data.insert_temp(drag_snapshot_id, before);
                        });
                    } else {
                        ui.ctx().data_mut(|data| data.insert_temp(drag_anchor_id, grab_offset_secs));
                    }
                }
                if !clip_is_deleting
                    && left_edge_dragging
                    && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
                {
                    let pointer_time = view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration;
                    let original_clip = ui
                        .ctx()
                        .data(|data| data.get_temp::<TrimTimelineClip>(left_edge_original_id))
                        .unwrap_or_else(|| clip.clone());
                    self.trim_timeline_resize_clip_edge_to_pointer(
                        sound_id,
                        row_index,
                        clip_index,
                        &original_clip,
                        pointer_time,
                        true,
                    );
                }
                if !clip_is_deleting
                    && right_edge_dragging
                    && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
                {
                    let pointer_time = view_start_secs
                        + ((pointer.x - timeline_rect.left()) / timeline_rect.width()).clamp(0.0, 1.0)
                            * visible_duration;
                    let original_clip = ui
                        .ctx()
                        .data(|data| data.get_temp::<TrimTimelineClip>(right_edge_original_id))
                        .unwrap_or_else(|| clip.clone());
                    self.trim_timeline_resize_clip_edge_to_pointer(
                        sound_id,
                        row_index,
                        clip_index,
                        &original_clip,
                        pointer_time,
                        false,
                    );
                }
                if !clip_is_deleting && selected_clip {
                    let playhead_time = timeline_playhead_secs.max(0.0);
                    let clip_playhead_x = timeline_rect.left()
                        + ((playhead_time - view_start_secs) / visible_duration).clamp(0.0, 1.0)
                            * timeline_rect.width();
                    let playhead_inside_clip =
                        playhead_time >= clip_time_start && playhead_time <= clip_time_end;
                    if playhead_inside_clip {
                        let local_time = clip.clip_start_secs
                            + (playhead_time - clip.start_secs).clamp(0.0, clip_duration);
                        let hint_x = clip_playhead_x.clamp(clip_rect.left(), clip_rect.right());
                    painter.line_segment(
                        [
                            Pos2::new(hint_x, clip_rect.top() + 6.0),
                            Pos2::new(hint_x, clip_rect.bottom() - 6.0),
                        ],
                        Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 150)),
                    );
                    painter.circle_filled(
                        Pos2::new(hint_x, clip_rect.top() + 10.0),
                        2.4,
                        Color32::from_rgba_premultiplied(108, 231, 255, 180),
                    );
                        if ctx.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::Q)
                        }) {
                            self.trim_timeline_trim_clip_edge_at_local_time(
                                ctx,
                                sound_id,
                                row_index,
                                clip_index,
                                local_time,
                                true,
                            );
                        }
                        if ctx.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::W)
                        }) {
                            self.trim_timeline_trim_clip_edge_at_local_time(
                                ctx,
                                sound_id,
                                row_index,
                                clip_index,
                                local_time,
                                false,
                            );
                        }
                    }
                }
                if !clip_is_deleting && removable && (clip_response.secondary_clicked() || right_sweep_delete) {
                    self.trim_timeline_begin_clip_delete_animation(ctx, clip.id);
                }
                if !clip_is_deleting
                    && removable
                    && clip_response.dragged_by(egui::PointerButton::Primary)
                    && !edge_dragging
                    && let Some(pointer) = clip_response.interact_pointer_pos()
                    && let Some(state) = self.trim_timeline_state.as_mut()
                {
                    let target_row = (((pointer.y - (viewport_rect.top() + rows_top_padding))
                        / (row_height + row_spacing).max(1.0))
                        .floor() as isize)
                        .clamp(0, row_count.saturating_sub(1) as isize)
                        as usize;
                    while state.rows.len() <= target_row {
                        state.rows.push(TrimTimelineRow::default());
                    }
                    let target_row_top =
                        viewport_rect.top()
                            + rows_top_padding
                            + target_row as f32 * (row_height + row_spacing);
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
                }
                if !clip_is_deleting
                    && clip_response.drag_stopped_by(egui::PointerButton::Primary)
                    && !edge_drag_stopped
                {
                    let before = ui
                        .ctx()
                        .data(|data| data.get_temp::<TrimTimelineState>(drag_snapshot_id));
                    ui.ctx().data_mut(|data| {
                        data.remove::<f32>(drag_anchor_id);
                        data.remove::<TrimTimelineState>(drag_snapshot_id);
                    });
                    if let Some(before) = before {
                        self.push_trim_timeline_undo_snapshot(before);
                    }
                    timeline_state_changed = true;
                }
                if left_edge_drag_stopped {
                    let before = ui
                        .ctx()
                        .data(|data| data.get_temp::<TrimTimelineState>(left_edge_snapshot_id));
                    ui.ctx().data_mut(|data| {
                        data.remove::<TrimTimelineState>(left_edge_snapshot_id);
                        data.remove::<TrimTimelineClip>(left_edge_original_id);
                    });
                    if let Some(before) = before {
                        self.push_trim_timeline_undo_snapshot(before);
                    }
                    timeline_state_changed = true;
                }
                if right_edge_drag_stopped {
                    let before = ui
                        .ctx()
                        .data(|data| data.get_temp::<TrimTimelineState>(right_edge_snapshot_id));
                    ui.ctx().data_mut(|data| {
                        data.remove::<TrimTimelineState>(right_edge_snapshot_id);
                        data.remove::<TrimTimelineClip>(right_edge_original_id);
                    });
                    if let Some(before) = before {
                        self.push_trim_timeline_undo_snapshot(before);
                    }
                    timeline_state_changed = true;
                }

                if !clip_is_deleting
                    && selected_clip
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

            for animation in self.trim_timeline_segment_delete_animations.iter().filter(|animation| {
                animation.owner_sound_id == sound_id && animation.row_index == row_index
            }) {
                let progress = (delete_anim_now
                    .saturating_duration_since(animation.started_at)
                    .as_secs_f32()
                    / TRIM_TIMELINE_DELETE_ANIM_SECS)
                    .clamp(0.0, 1.0);
                let Some(sound) = self
                    .sounds
                    .iter()
                    .find(|candidate| candidate.id == animation.source_sound_id)
                else {
                    continue;
                };
                let clip_duration =
                    (animation.clip_end_secs - animation.clip_start_secs).max(0.05);
                let clip_time_start = animation.start_secs.max(0.0);
                let clip_time_end = clip_time_start + clip_duration;
                if clip_time_end <= view_start_secs || clip_time_start >= view_end_secs {
                    continue;
                }
                let visible_clip_start = clip_time_start.max(view_start_secs);
                let visible_clip_end = clip_time_end.min(view_end_secs);
                let clip_left = timeline_rect.left()
                    + ((visible_clip_start - view_start_secs) / visible_duration)
                        * timeline_rect.width();
                let clip_right = timeline_rect.left()
                    + ((visible_clip_end - view_start_secs) / visible_duration)
                        * timeline_rect.width();
                let animation_rect = Rect::from_min_max(
                    Pos2::new(clip_left, timeline_rect.top() + 1.0),
                    Pos2::new(
                        clip_right.max(clip_left + 1.5).min(timeline_rect.right()),
                        timeline_rect.bottom() - 1.0,
                    ),
                );
                let rendered_animation_rect = animation_rect.shrink2(vec2(
                    ((animation_rect.width() - 8.0).max(0.0) * 0.12).min(14.0) * progress,
                    ((animation_rect.height() - 8.0).max(0.0) * 0.28).min(16.0) * progress,
                ));
                let waveform_bars =
                    Self::trim_timeline_waveform_bars(rendered_animation_rect.width());
                let preview = Self::timeline_clip_waveform_preview_from_samples(
                    sound,
                    &self.sound_waveform_samples(sound),
                    animation.clip_start_secs,
                    animation.clip_end_secs,
                    waveform_bars,
                );
                painter.rect_filled(
                    rendered_animation_rect,
                    0.0,
                    Color32::from_rgba_premultiplied(92, 38, 71, 220)
                        .linear_multiply(1.0 - progress * 0.45),
                );
                painter.rect_stroke(
                    rendered_animation_rect,
                    0.0,
                    Stroke::new(
                        1.0,
                        Color32::from_rgb(255, 112, 181).linear_multiply(1.0 - progress * 0.3),
                    ),
                    StrokeKind::Outside,
                );
                if rendered_animation_rect.width() >= 18.0 {
                    let waveform_inset_x = 1.0;
                    let waveform_rect = Rect::from_min_max(
                        Pos2::new(
                            rendered_animation_rect.left() + waveform_inset_x,
                            rendered_animation_rect.top() + 15.0,
                        ),
                        Pos2::new(
                            (rendered_animation_rect.right() - waveform_inset_x)
                                .max(rendered_animation_rect.left() + waveform_inset_x),
                            rendered_animation_rect.bottom() - 4.0,
                        ),
                    );
                    Self::paint_timeline_waveform_columns(
                        &painter,
                        waveform_rect,
                        &preview,
                        Color32::from_rgb(241, 78, 162).linear_multiply(1.0 - progress * 0.2),
                        false,
                    );
                }
                ctx.request_repaint();
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
                        0.0,
                        Color32::from_rgba_premultiplied(56, 34, 49, 170),
                    );
                    painter.rect_stroke(
                        ghost_rect,
                        0.0,
                        Stroke::new(1.25, Color32::from_rgba_premultiplied(255, 112, 181, 196)),
                        StrokeKind::Outside,
                    );
                    let ghost_waveform = self.trim_timeline_track_waveform(
                        drag_sound,
                        Self::trim_timeline_waveform_bars(ghost_rect.width()),
                    );
                    if ghost_rect.width() >= 18.0 {
                        let waveform_inset_x = 1.0;
                        let ghost_title_rect = Rect::from_min_max(
                            ghost_rect.left_top() + vec2(8.0, 4.0),
                            Pos2::new((ghost_rect.right() - 8.0).max(ghost_rect.left() + 8.0), ghost_rect.top() + 18.0),
                        );
                        let ghost_waveform_rect = Rect::from_min_max(
                            Pos2::new(ghost_rect.left() + waveform_inset_x, ghost_rect.top() + 18.0),
                            Pos2::new(
                                (ghost_rect.right() - waveform_inset_x)
                                    .max(ghost_rect.left() + waveform_inset_x),
                                ghost_rect.bottom() - 8.0,
                            ),
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

        if let Some(origin) = ctx.data(|data| data.get_temp::<Pos2>(marquee_origin_id)) {
            if let Some(pointer) = ctx.input(|input| input.pointer.hover_pos()) {
                let marquee_rect = Rect::from_two_pos(origin, pointer).intersect(viewport_rect);
                viewport_painter.rect_filled(
                    marquee_rect,
                    0.0,
                    Color32::from_rgba_premultiplied(108, 231, 255, 28),
                );
                viewport_painter.rect_stroke(
                    marquee_rect,
                    0.0,
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 231, 255, 180)),
                    StrokeKind::Inside,
                );
                let selected_ids = rendered_clip_rects
                    .iter()
                    .filter_map(|(clip_id, rect)| marquee_rect.intersects(*rect).then_some(*clip_id))
                    .collect::<HashSet<_>>();
                if let Some(state) = self.trim_timeline_state.as_mut() {
                    state.selected_clip_id = selected_ids.iter().next().copied();
                    state.selected_clip_ids = selected_ids;
                }
                ctx.request_repaint();
            }
            if !ctx.input(|input| input.pointer.primary_down()) {
                ctx.data_mut(|data| data.remove::<Pos2>(marquee_origin_id));
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
        let global_playhead_x = shared_timeline_left
            + ((timeline_playhead_secs.max(0.0) - view_start_secs) / visible_duration)
                .clamp(0.0, 1.0)
                * shared_timeline_width;
        viewport_painter.line_segment(
            [
                Pos2::new(global_playhead_x, ruler_rect.top() + 3.0),
                Pos2::new(global_playhead_x, viewport_rect.bottom() - 3.0),
            ],
            Stroke::new(1.75, Color32::from_rgb(108, 231, 255)),
        );
        viewport_painter.circle_filled(
            Pos2::new(global_playhead_x, ruler_rect.top() + 9.0),
            3.0,
            Color32::from_rgb(108, 231, 255),
        );

        let mut removal_before_snapshot = None;
        if let Some(state) = self.trim_timeline_state.as_mut() {
            state.enabled = next_enabled;
            state.playhead_secs = timeline_playhead_secs.max(0.0);
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
            let rows_before_normalize = state.rows.clone();
            Self::normalize_trim_timeline_rows(&mut state.rows);
            if state.rows != rows_before_normalize {
                timeline_state_changed = true;
            }
            let mut finalized_removed_clip_ids = Vec::new();
            self.trim_timeline_clip_delete_animating
                .retain(|clip_id, started| {
                    let keep = delete_anim_now
                        .saturating_duration_since(*started)
                        .as_secs_f32()
                        < TRIM_TIMELINE_DELETE_ANIM_SECS;
                    if !keep {
                        finalized_removed_clip_ids.push(*clip_id);
                    }
                    keep
                });
            if !self.trim_timeline_clip_delete_animating.is_empty() {
                ctx.request_repaint();
            }
            let mut removed_any_clip = false;
            for removed_clip_id in finalized_removed_clip_ids {
                if removal_before_snapshot.is_none() {
                    removal_before_snapshot = Some(state.clone());
                }
                let mut removed = false;
                for row in &mut state.rows {
                    if let Some(clip_index) = row.clips.iter().position(|clip| clip.id == removed_clip_id) {
                        row.clips.remove(clip_index);
                        removed = true;
                        break;
                    }
                }
                if removed {
                    removed_any_clip = true;
                }
                if state.selected_clip_id == Some(removed_clip_id) {
                    state.selected_clip_id = None;
                }
            }
            if removed_any_clip {
                let rows_before_normalize = state.rows.clone();
                Self::normalize_trim_timeline_rows(&mut state.rows);
                timeline_state_changed = state.rows != rows_before_normalize || removed_any_clip;
            }
        }
        if let Some(before) = removal_before_snapshot {
            self.push_trim_timeline_undo_snapshot(before);
        }
        self.trim_timeline_segment_delete_animations
            .retain(|animation| {
                delete_anim_now
                    .saturating_duration_since(animation.started_at)
                    .as_secs_f32()
                    < TRIM_TIMELINE_DELETE_ANIM_SECS
            });
        if !self.trim_timeline_segment_delete_animations.is_empty() {
            ctx.request_repaint();
        }
        if toolbar_trim_left {
            self.trim_timeline_trim_selected_clip_at_playhead(ctx, sound_id, true);
        }
        if toolbar_trim_right {
            self.trim_timeline_trim_selected_clip_at_playhead(ctx, sound_id, false);
        }
        if toolbar_copy_clip {
            self.trim_timeline_copy_selected_clip(sound_id);
        }
        if toolbar_paste_clip {
            self.trim_timeline_paste_copied_clip(ctx, sound_id);
        }
        if toolbar_split_clip {
            self.trim_timeline_split_selected_clip_at_playhead(ctx, sound_id);
        }
        if toolbar_delete_clip {
            self.trim_timeline_delete_selected_clip(ctx, sound_id);
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
        let timeline_mix = self
            .timeline_mode_active_sound_id()
            .and_then(|sound_id| self.collect_trim_timeline_render_clips(sound_id))
            .is_some();
        let automatic_name = timeline_mix.then(|| {
            next_default_sound_name(self.sounds.iter().map(|sound| sound.name.as_str()))
        });
        let desired_size = if timeline_mix {
            vec2(380.0, 250.0)
        } else {
            vec2(360.0, 180.0)
        };
        let minimum_size = if timeline_mix {
            vec2(300.0, 220.0)
        } else {
            vec2(280.0, 160.0)
        };
        let (_panel_bounds, panel_size, panel_pos) =
            self.centered_modal_placement(ctx, desired_size, minimum_size, 0.0);

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
                if timeline_mix {
                    ui.label(
                        RichText::new("Output name")
                            .size(13.0)
                            .color(Self::strong_text_color())
                            .strong(),
                    );
                    ui.add_space(6.0);
                    ui.add_sized(
                        [ui.available_width(), 32.0],
                        TextEdit::singleline(&mut self.trim_commit_output_name)
                            .hint_text(format!(
                                "Leave blank for {}",
                                automatic_name.as_deref().unwrap_or("Sound 1")
                            )),
                    );
                    ui.add_space(12.0);
                }
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

#[cfg(test)]
mod timeline_name_tests {
    use super::next_default_sound_name;

    #[test]
    fn default_timeline_name_uses_first_available_number() {
        assert_eq!(
            next_default_sound_name(["Sound 1", "other", "sound 2"]),
            "Sound 3"
        );
    }

    #[test]
    fn video_preview_filter_is_valid() {
        let preview_fps = 20;
        let filter = format!(
            "fps={preview_fps},scale='min(720,iw)':'min(430,ih)':force_original_aspect_ratio=decrease"
        );
        assert_eq!(
            filter,
            "fps=20,scale='min(720,iw)':'min(430,ih)':force_original_aspect_ratio=decrease"
        );
    }
}
