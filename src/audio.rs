use crate::storage::SoundEffect;
use anyhow::{Context, Result, bail};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source, buffer::SamplesBuffer};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

pub struct AudioEngine {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    current_id: Option<Uuid>,
    started_at: Option<Instant>,
    current_duration_secs: f32,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("unable to open default audio output")?;

        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            current_id: None,
            started_at: None,
            current_duration_secs: 0.0,
        })
    }

    pub fn play(&mut self, sound: &SoundEffect, asset_path: &Path) -> Result<()> {
        self.stop();

        let (channels, sample_rate, trimmed_samples) = load_trimmed_samples(sound, asset_path)?;
        let speed = sound.speed.clamp(0.25, 2.0);

        let preview = SamplesBuffer::new(channels, sample_rate, trimmed_samples)
            .speed(speed)
            .amplify(sound.volume.max(0.0));

        let sink = Sink::try_new(&self.handle).context("unable to create audio sink")?;
        sink.append(preview);
        sink.play();

        self.current_id = Some(sound.id);
        self.started_at = Some(Instant::now());
        self.current_duration_secs = sound.trimmed_length() / speed;
        self.sink = Some(sink);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.current_id = None;
        self.started_at = None;
        self.current_duration_secs = 0.0;
    }

    pub fn tick(&mut self) {
        let finished = self.sink.as_ref().is_some_and(|sink| sink.empty());
        if finished {
            self.stop();
        }
    }

    pub fn is_playing(&self, sound_id: Uuid) -> bool {
        self.current_id == Some(sound_id)
    }

    pub fn playback_progress(&self, sound_id: Uuid) -> Option<f32> {
        if self.current_id != Some(sound_id) {
            return None;
        }

        let started_at = self.started_at?;
        let duration = self.current_duration_secs.max(0.05);
        Some((started_at.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0))
    }

    pub fn has_active_playback(&self) -> bool {
        self.sink.is_some()
    }
}

fn load_trimmed_samples(sound: &SoundEffect, asset_path: &Path) -> Result<(u16, u32, Vec<f32>)> {
    let file = File::open(asset_path)
        .with_context(|| format!("unable to open {}", asset_path.display()))?;
    let decoder = Decoder::new(BufReader::new(file)).context("unsupported audio file")?;
    let channels = decoder.channels().max(1);
    let sample_rate = decoder.sample_rate().max(1);
    let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();

    if samples.is_empty() {
        bail!("audio file is empty");
    }

    let total_frames = samples.len() / channels as usize;
    let safe_duration = sound.safe_duration();
    let start_frame = ((sound.trim_start_secs.clamp(0.0, safe_duration) * sample_rate as f32)
        .floor() as usize)
        .min(total_frames);
    let end_frame = ((sound.trim_end_secs.clamp(0.0, safe_duration) * sample_rate as f32).ceil()
        as usize)
        .min(total_frames)
        .max(start_frame + 1);
    let start_sample = start_frame * channels as usize;
    let end_sample = (end_frame * channels as usize).min(samples.len());

    Ok((
        channels,
        sample_rate,
        samples[start_sample..end_sample].to_vec(),
    ))
}
