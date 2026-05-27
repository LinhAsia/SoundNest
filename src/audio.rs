use crate::storage::SoundEffect;
use anyhow::{Context, Result, bail};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source, buffer::SamplesBuffer};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub fn calculate_normalization_gain(asset_path: &Path) -> Result<f32> {
    let (_channels, _sample_rate, samples) = decode_audio_file(asset_path)?;
    if samples.is_empty() {
        bail!("audio file is empty");
    }

    let mean_square = samples
        .iter()
        .map(|&sample| sample * sample)
        .sum::<f32>()
        / samples.len() as f32;
    let current_rms = mean_square.sqrt();

    if current_rms < 0.00001 {
        return Ok(1.0);
    }

    let target_rms = 0.10; // Matches audiobookmaker target
    let mut gain = target_rms / current_rms;

    // Prevent massive clipping distortion on transient effects
    let mut max_peak = 0.0f32;
    for &s in &samples {
        let abs = s.abs();
        if abs > max_peak {
            max_peak = abs;
        }
    }

    if max_peak > 0.0 {
        let peak_with_gain = max_peak * gain;
        let safety_max = 29000.0 / 32768.0; // Matches audiobookmaker safety max (~0.885)
        if peak_with_gain > safety_max {
            gain = safety_max / max_peak;
        }
    }

    Ok(gain)
}

const POP_FADE_MS: f32 = 18.0;

struct CachedAudio {
    channels: u16,
    sample_rate: u32,
    samples: Arc<[f32]>,
}

struct SharedSamplesSource {
    samples: Arc<[f32]>,
    index: usize,
    end: usize,
    channels: u16,
    sample_rate: u32,
}

impl Iterator for SharedSamplesSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.end {
            return None;
        }
        let sample = self.samples.get(self.index).copied();
        self.index = self.index.saturating_add(1);
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.end.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl Source for SharedSamplesSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.end.saturating_sub(self.index) / self.channels.max(1) as usize)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        let frames = self.end.saturating_sub(self.index) / self.channels.max(1) as usize;
        Some(std::time::Duration::from_secs_f32(
            frames as f32 / self.sample_rate.max(1) as f32,
        ))
    }
}

pub struct AudioEngine {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    cached_audio: HashMap<PathBuf, CachedAudio>,
    current_id: Option<Uuid>,
    current_file_path: Option<PathBuf>,
    current_total_duration_secs: f32,
    current_trim_start_secs: f32,
    current_start_offset_secs: f32,
    current_speed: f32,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("unable to open default audio output")?;

        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            cached_audio: HashMap::new(),
            current_id: None,
            current_file_path: None,
            current_total_duration_secs: 0.0,
            current_trim_start_secs: 0.0,
            current_start_offset_secs: 0.0,
            current_speed: 1.0,
        })
    }

    pub fn play(&mut self, sound: &SoundEffect, asset_path: &Path) -> Result<()> {
        self.play_from(sound, asset_path, sound.trim_start_secs)
    }

    pub fn play_from(
        &mut self,
        sound: &SoundEffect,
        asset_path: &Path,
        start_position_secs: f32,
    ) -> Result<()> {
        self.stop();

        self.ensure_cached_audio(asset_path)?;
        let cached = self
            .cached_audio
            .get(asset_path)
            .expect("cached audio should exist after ensure_cached_audio");
        let channels = cached.channels;
        let sample_rate = cached.sample_rate;
        let (trim_start_sample, trim_end_sample) =
            trim_sample_range(sound, channels, sample_rate, cached.samples.len())?;
        let speed = sound.speed.clamp(0.25, 2.0);
        let total_duration_secs = (trim_end_sample - trim_start_sample) as f32
            / channels.max(1) as f32
            / sample_rate.max(1) as f32;
        let total_frames = (trim_end_sample - trim_start_sample) / channels as usize;
        let start_offset_secs =
            (start_position_secs - sound.trim_start_secs).clamp(0.0, total_duration_secs.max(0.0));
        let start_frame = ((start_offset_secs * sample_rate as f32).floor() as usize)
            .min(total_frames.saturating_sub(1));
        let start_offset_secs = start_frame as f32 / sample_rate as f32;
        let start_sample = trim_start_sample + start_frame * channels as usize;

        let preview = SharedSamplesSource {
            samples: Arc::clone(&cached.samples),
            index: start_sample,
            end: trim_end_sample,
            channels,
            sample_rate,
        }
        .speed(speed)
        .amplify(sound.volume.max(0.0));

        let sink = Sink::try_new(&self.handle).context("unable to create audio sink")?;
        sink.append(preview);
        sink.play();

        self.current_id = Some(sound.id);
        self.current_file_path = None;
        self.current_total_duration_secs = total_duration_secs;
        self.current_trim_start_secs = sound.trim_start_secs;
        self.current_start_offset_secs = start_offset_secs;
        self.current_speed = speed;
        self.sink = Some(sink);
        Ok(())
    }

    pub fn play_processed_file(
        &mut self,
        sound: &SoundEffect,
        asset_path: &Path,
        start_position_secs: f32,
    ) -> Result<()> {
        self.stop();

        self.ensure_cached_audio(asset_path)?;
        let cached = self
            .cached_audio
            .get(asset_path)
            .expect("cached audio should exist after ensure_cached_audio");
        let channels = cached.channels;
        let sample_rate = cached.sample_rate;
        if cached.samples.is_empty() {
            bail!("audio file is empty");
        }

        let speed = sound.speed.clamp(0.25, 2.0);
        let total_duration_secs = sound.trimmed_length().max(0.05);
        let original_offset_secs =
            (start_position_secs - sound.trim_start_secs).clamp(0.0, total_duration_secs);
        let file_offset_secs = (original_offset_secs / speed).clamp(0.0, total_duration_secs);
        let total_frames = cached.samples.len() / channels as usize;
        let start_frame = ((file_offset_secs * sample_rate as f32).floor() as usize)
            .min(total_frames.saturating_sub(1));
        let start_sample = start_frame * channels as usize;

        let preview = SharedSamplesSource {
            samples: Arc::clone(&cached.samples),
            index: start_sample,
            end: cached.samples.len(),
            channels,
            sample_rate,
        };

        let sink = Sink::try_new(&self.handle).context("unable to create audio sink")?;
        sink.append(preview);
        sink.play();

        self.current_id = Some(sound.id);
        self.current_file_path = Some(asset_path.to_path_buf());
        self.current_total_duration_secs = total_duration_secs;
        self.current_trim_start_secs = sound.trim_start_secs;
        self.current_start_offset_secs = original_offset_secs;
        self.current_speed = speed;
        self.sink = Some(sink);
        Ok(())
    }

    pub fn play_file(&mut self, asset_path: &Path) -> Result<()> {
        self.play_file_from(asset_path, 0.0)
    }

    pub fn play_file_from(&mut self, asset_path: &Path, start_position_secs: f32) -> Result<()> {
        self.stop();

        self.ensure_cached_audio(asset_path)?;
        let cached = self
            .cached_audio
            .get(asset_path)
            .expect("cached audio should exist after ensure_cached_audio");
        let channels = cached.channels;
        let sample_rate = cached.sample_rate;
        if cached.samples.is_empty() {
            bail!("audio file is empty");
        }

        let total_duration_secs =
            cached.samples.len() as f32 / channels.max(1) as f32 / sample_rate.max(1) as f32;
        let total_frames = cached.samples.len() / channels as usize;
        let start_offset_secs = start_position_secs.clamp(0.0, total_duration_secs.max(0.0));
        let start_frame = ((start_offset_secs * sample_rate as f32).floor() as usize)
            .min(total_frames.saturating_sub(1));
        let start_offset_secs = start_frame as f32 / sample_rate as f32;
        let start_sample = start_frame * channels as usize;

        let preview = SharedSamplesSource {
            samples: Arc::clone(&cached.samples),
            index: start_sample,
            end: cached.samples.len(),
            channels,
            sample_rate,
        };
        let sink = Sink::try_new(&self.handle).context("unable to create audio sink")?;
        sink.append(preview);
        sink.play();

        self.current_id = None;
        self.current_file_path = Some(asset_path.to_path_buf());
        self.current_total_duration_secs = total_duration_secs;
        self.current_trim_start_secs = 0.0;
        self.current_start_offset_secs = start_offset_secs;
        self.current_speed = 1.0;
        self.sink = Some(sink);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.current_id = None;
        self.current_file_path = None;
        self.current_total_duration_secs = 0.0;
        self.current_trim_start_secs = 0.0;
        self.current_start_offset_secs = 0.0;
        self.current_speed = 1.0;
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

    pub fn is_playing_file(&self, path: &Path) -> bool {
        self.current_file_path.as_deref() == Some(path) && self.sink.is_some()
    }

    pub fn playback_progress(&self, sound_id: Uuid) -> Option<f32> {
        if self.current_id != Some(sound_id) {
            return None;
        }

        let sink = self.sink.as_ref()?;
        let total_duration = self.current_total_duration_secs.max(0.05);
        let elapsed = sink.get_pos().as_secs_f32() * self.current_speed.max(0.25);
        let played = (self.current_start_offset_secs + elapsed).clamp(0.0, total_duration);
        Some((played / total_duration).clamp(0.0, 1.0))
    }

    pub fn playback_position_secs(&self, sound_id: Uuid) -> Option<f32> {
        if self.current_id != Some(sound_id) {
            return None;
        }

        let sink = self.sink.as_ref()?;
        let total_duration = self.current_total_duration_secs.max(0.05);
        let elapsed = sink.get_pos().as_secs_f32() * self.current_speed.max(0.25);
        let played = (self.current_start_offset_secs + elapsed).clamp(0.0, total_duration);
        Some(self.current_trim_start_secs + played)
    }

    pub fn playback_progress_for_file(&self, path: &Path) -> Option<f32> {
        if self.current_file_path.as_deref() != Some(path) {
            return None;
        }

        let sink = self.sink.as_ref()?;
        let total_duration = self.current_total_duration_secs.max(0.05);
        let elapsed = sink.get_pos().as_secs_f32();
        Some((elapsed / total_duration).clamp(0.0, 1.0))
    }

    pub fn has_active_playback(&self) -> bool {
        self.sink.is_some()
    }

    pub fn has_cached_audio(&self, asset_path: &Path) -> bool {
        self.cached_audio.contains_key(asset_path)
    }

    pub fn insert_cached_audio(
        &mut self,
        asset_path: PathBuf,
        channels: u16,
        sample_rate: u32,
        samples: Vec<f32>,
    ) {
        self.cached_audio.insert(
            asset_path,
            CachedAudio {
                channels,
                sample_rate,
                samples: Arc::<[f32]>::from(samples),
            },
        );
    }

    pub fn decode_audio_for_cache(asset_path: &Path) -> Result<(u16, u32, Vec<f32>)> {
        decode_audio_file(asset_path)
    }

    fn ensure_cached_audio(&mut self, asset_path: &Path) -> Result<()> {
        if !self.cached_audio.contains_key(asset_path) {
            let (channels, sample_rate, samples) = decode_audio_file(asset_path)?;
            self.cached_audio.insert(
                asset_path.to_path_buf(),
                CachedAudio {
                    channels,
                    sample_rate,
                    samples: Arc::<[f32]>::from(samples),
                },
            );
        }
        Ok(())
    }
}

pub fn play_file_blocking(asset_path: &Path) -> Result<()> {
    let (_stream, handle) =
        OutputStream::try_default().context("unable to open default audio output")?;
    let (channels, sample_rate, mut samples) = decode_audio_file(asset_path)?;
    soften_sample_edges(&mut samples, channels, sample_rate, POP_FADE_MS);
    let sink = Sink::try_new(&handle).context("unable to create audio sink")?;
    sink.append(SamplesBuffer::new(channels, sample_rate, samples));
    sink.sleep_until_end();
    Ok(())
}

fn trim_sample_range(
    sound: &SoundEffect,
    channels: u16,
    sample_rate: u32,
    sample_len: usize,
) -> Result<(usize, usize)> {
    if sample_len == 0 {
        bail!("audio file is empty");
    }

    let total_frames = sample_len / channels as usize;
    let actual_duration = total_frames as f32 / sample_rate as f32;
    let trim_start = sound.trim_start_secs.clamp(0.0, actual_duration);
    let trim_end = if sound.trim_end_secs >= sound.safe_duration() - 0.02 {
        actual_duration
    } else {
        sound.trim_end_secs.clamp(0.0, actual_duration)
    };
    let start_frame = ((trim_start * sample_rate as f32).floor() as usize).min(total_frames);
    let end_frame = if trim_end >= actual_duration - 0.02 {
        total_frames
    } else {
        ((trim_end * sample_rate as f32).ceil() as usize)
            .min(total_frames)
            .max(start_frame + 1)
    };
    let start_sample = start_frame * channels as usize;
    let end_sample = (end_frame * channels as usize).min(sample_len);

    Ok((start_sample, end_sample))
}

fn decode_audio_file(asset_path: &Path) -> Result<(u16, u32, Vec<f32>)> {
    let path = asset_path.to_path_buf();
    catch_unwind(AssertUnwindSafe(|| -> Result<(u16, u32, Vec<f32>)> {
        let file =
            File::open(&path).with_context(|| format!("unable to open {}", path.display()))?;
        let decoder = Decoder::new(BufReader::new(file)).context("unsupported audio file")?;
        let channels = decoder.channels().max(1);
        let sample_rate = decoder.sample_rate().max(1);
        let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();
        Ok((channels, sample_rate, samples))
    }))
    .map_err(|_| anyhow::anyhow!("audio decoder crashed while reading {}", path.display()))?
}

fn soften_sample_edges(samples: &mut [f32], channels: u16, sample_rate: u32, fade_ms: f32) {
    if samples.is_empty() || channels == 0 || sample_rate == 0 {
        return;
    }

    let total_frames = samples.len() / channels as usize;
    if total_frames < 2 {
        return;
    }

    let fade_frames =
        ((sample_rate as f32 * (fade_ms / 1000.0)).round() as usize).clamp(1, total_frames / 2);
    if fade_frames == 0 {
        return;
    }

    let denom = fade_frames.saturating_sub(1).max(1) as f32;
    for frame in 0..fade_frames {
        let fade_in = frame as f32 / denom;
        let fade_out = (fade_frames.saturating_sub(1) - frame) as f32 / denom;
        let start_base = frame * channels as usize;
        let end_base = (total_frames - 1 - frame) * channels as usize;
        for channel in 0..channels as usize {
            samples[start_base + channel] *= fade_in;
            samples[end_base + channel] *= fade_out;
        }
    }
}
