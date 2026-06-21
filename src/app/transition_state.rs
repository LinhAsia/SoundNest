use super::*;

impl SoundFxApp {
    pub(super) fn play_startup_sound_if_needed(&mut self, ctx: &Context) {
        if !self.app_transition_animation {
            self.startup_sound_played = true;
            return;
        }
        if self.startup_sound_played || self.startup.phase != TransitionPhase::Intro {
            return;
        }

        if let Ok(Some(path)) = self.storage.resolved_startup_sound_path() {
            let _ = self.play_file_if_exists(&path);
        }
        self.startup_sound_played = true;
        self.startup.started_at = Some(ctx.input(|input| input.time));
    }

    pub(super) fn intercept_close_request(&mut self, ctx: &Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.startup.close_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(ctx);
        }
    }

    pub(super) fn transition_progress(&mut self, ctx: &Context) -> Option<(TransitionPhase, f32)> {
        let phase = self.startup.phase;
        if phase == TransitionPhase::Live {
            return None;
        }

        let now = ctx.input(|input| input.time);
        let started_at = self.startup.started_at.get_or_insert(now);
        let progress =
            ((now - *started_at) / self.startup.duration_sec as f64).clamp(0.0, 1.0) as f32;

        if phase == TransitionPhase::Outro {
            self.update_outro_audio_fade(progress);
        }

        if progress >= 1.0 {
            match phase {
                TransitionPhase::Intro => {
                    self.startup.phase = TransitionPhase::Live;
                    self.startup.started_at = None;
                    self.startup.live_started_at = Some(now);
                    self.startup.duration_sec = 0.0;
                    return None;
                }
                TransitionPhase::Outro => {
                    if !self.startup.close_sent {
                        if let Some(audio) = self.audio.as_mut() {
                            audio.set_volume(0.0);
                            audio.stop();
                        }
                        self.finalize_close_cleanup(ctx);
                        self.startup.close_sent = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    ctx.request_repaint_after(Duration::from_millis(ACTIVE_UI_REPAINT_MS));
                    return Some((phase, 1.0));
                }
                TransitionPhase::Live => {}
            }
        }

        ctx.request_repaint();
        Some((phase, progress))
    }

    pub(super) fn live_ui_reveal_progress(&mut self, ctx: &Context) -> f32 {
        let Some(started_at) = self.startup.live_started_at else {
            return 1.0;
        };

        let now = ctx.input(|input| input.time);
        let progress = ((now - started_at) / LIVE_UI_FADE_SEC as f64).clamp(0.0, 1.0) as f32;
        if progress >= 1.0 {
            self.startup.live_started_at = None;
            return 1.0;
        }

        Self::ease_in_out_cubic(progress)
    }
}
