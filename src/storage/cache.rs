use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavSpec, WavWriter};
use rodio::{Decoder, Source};
use std::fs::{self, File};
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use super::SoundEffect;

const EXPORT_POP_FADE_MS: f32 = 18.0;

pub(super) struct AudioAnalysis {
    pub(super) duration_secs: f32,
    pub(super) waveform: Vec<f32>,
}

struct DecodedAudio {
    channels: u16,
    sample_rate: u32,
    samples: Vec<f32>,
}

pub(crate) struct MixedAudioClip {
    pub(crate) path: PathBuf,
    pub(crate) start_secs: f32,
    pub(crate) clip_start_secs: f32,
    pub(crate) clip_end_secs: f32,
}

pub(super) fn analyze_audio_file(path: &Path, buckets: usize) -> Result<AudioAnalysis> {
    let decoded = decode_audio_file(path)?;
    let bucket_count = buckets.max(64);
    let samples_per_bucket = (decoded.samples.len() / bucket_count).max(1);
    let mut peaks = vec![0.0f32; bucket_count];
    let mut energy = vec![0.0f32; bucket_count];
    let mut counts = vec![0usize; bucket_count];
    for (sample_index, sample) in decoded.samples.iter().enumerate() {
        let bucket = (sample_index / samples_per_bucket).min(bucket_count - 1);
        peaks[bucket] = peaks[bucket].max(sample.abs());
        energy[bucket] += sample * sample;
        counts[bucket] += 1;
    }

    let decoded_duration_secs = decoded.samples.len() as f32
        / decoded.channels.max(1) as f32
        / decoded.sample_rate.max(1) as f32;

    let mut waveform = Vec::with_capacity(bucket_count);
    for bucket_index in 0..bucket_count {
        let rms = if counts[bucket_index] == 0 {
            0.0
        } else {
            (energy[bucket_index] / counts[bucket_index] as f32).sqrt()
        };
        waveform.push((peaks[bucket_index] * 0.62 + rms * 0.38).powf(1.12));
    }

    let peak_max = waveform.iter().copied().fold(0.0f32, f32::max);
    if peak_max > 0.0 {
        for value in &mut waveform {
            *value = (*value / peak_max).clamp(0.0, 1.0);
            if *value < 0.06 {
                *value *= 0.5;
            }
        }
    }

    Ok(AudioAnalysis {
        duration_secs: decoded_duration_secs,
        waveform,
    })
}

fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    let path_buf = path.to_path_buf();
    catch_unwind(AssertUnwindSafe(|| -> Result<DecodedAudio> {
        let decoder = open_decoder(&path_buf)?;
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();

        if samples.is_empty() {
            bail!("audio file is empty");
        }

        Ok(DecodedAudio {
            channels,
            sample_rate,
            samples,
        })
    }))
    .map_err(|_| anyhow::anyhow!("audio decoder crashed while reading {}", path_buf.display()))?
}

pub(super) fn write_processed_wav(
    source_path: &Path,
    target_path: &Path,
    sound: &SoundEffect,
) -> Result<()> {
    let decoded = decode_audio_file(source_path)?;
    let channels = decoded.channels.max(1);
    let sample_rate = decoded.sample_rate.max(1);
    let total_frames = decoded.samples.len() / channels as usize;

    let actual_duration = total_frames as f32 / sample_rate as f32;
    let spec = WavSpec {
        channels,
        sample_rate: ((sample_rate as f32 * sound.speed.clamp(0.25, 2.0)).round() as u32).max(1),
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer =
        WavWriter::create(target_path, spec).context("unable to create exported wav file")?;
    let volume = sound.volume.clamp(0.0, 5.0);
    let mut processed = Vec::new();
    for (range_start, range_end) in sound.trim_ranges() {
        let range_end = if range_end >= actual_duration - 0.02 {
            actual_duration
        } else {
            range_end.clamp(0.0, actual_duration)
        };
        let start_frame = ((range_start * sample_rate as f32).floor() as usize).min(total_frames);
        let end_frame = if range_end >= actual_duration - 0.02 {
            total_frames
        } else {
            ((range_end * sample_rate as f32).ceil() as usize)
                .min(total_frames)
                .max(start_frame)
        };
        let start_sample = start_frame * channels as usize;
        let end_sample = end_frame * channels as usize;
        if end_sample > start_sample {
            processed.extend_from_slice(&decoded.samples[start_sample..end_sample]);
        }
    }
    if processed.is_empty() {
        processed
            .extend_from_slice(&decoded.samples[..decoded.samples.len().min(channels as usize)]);
    }
    soften_sample_edges(&mut processed, channels, sample_rate, EXPORT_POP_FADE_MS);
    apply_sound_effects(&mut processed, channels, sample_rate, sound);

    for sample in &processed {
        let scaled = (*sample * volume).clamp(-1.0, 1.0);
        let pcm = (scaled * i16::MAX as f32).round() as i16;
        writer
            .write_sample(pcm)
            .context("unable to write exported wav sample")?;
    }

    writer
        .finalize()
        .context("unable to finalize exported wav file")?;
    Ok(())
}

fn decode_audio_file_segment(
    path: &Path,
    start_secs: f32,
    end_secs: f32,
) -> Result<DecodedAudio> {
    let path_buf = path.to_path_buf();
    let start_secs = start_secs.max(0.0);
    let duration_secs = (end_secs - start_secs).max(0.05);

    catch_unwind(AssertUnwindSafe(|| -> Result<DecodedAudio> {
        let decoder = open_decoder(&path_buf)?;
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let segmented = decoder
            .skip_duration(std::time::Duration::from_secs_f32(start_secs))
            .take_duration(std::time::Duration::from_secs_f32(duration_secs));
        let samples = segmented.convert_samples::<f32>().collect::<Vec<_>>();

        if samples.is_empty() {
            bail!("audio file segment is empty");
        }

        Ok(DecodedAudio {
            channels,
            sample_rate,
            samples,
        })
    }))
    .map_err(|_| anyhow::anyhow!("audio decoder crashed while reading {}", path_buf.display()))?
}

pub(crate) fn write_mixed_wav(clips: &[MixedAudioClip], target_path: &Path) -> Result<()> {
    const TARGET_SAMPLE_RATE: u32 = 44_100;
    const TARGET_CHANNELS: u16 = 2;

    if clips.is_empty() {
        let total_frames = (60.0 * TARGET_SAMPLE_RATE as f32) as usize;
        let mixed = vec![0i16; total_frames * TARGET_CHANNELS as usize];
        let spec = WavSpec {
            channels: TARGET_CHANNELS,
            sample_rate: TARGET_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = WavWriter::create(target_path, spec)?;
        for sample in mixed {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
        return Ok(());
    }

    let mut prepared = Vec::with_capacity(clips.len());
    let mut total_frames = 0usize;
    for clip in clips {
        let decoded = decode_audio_file_segment(&clip.path, clip.clip_start_secs, clip.clip_end_secs)?;
        let mut stereo = convert_to_stereo(&decoded.samples, decoded.channels.max(1));
        if decoded.sample_rate.max(1) != TARGET_SAMPLE_RATE {
            stereo = resample_stereo_linear(&stereo, decoded.sample_rate.max(1), TARGET_SAMPLE_RATE);
        }
        soften_sample_edges(&mut stereo, TARGET_CHANNELS, TARGET_SAMPLE_RATE, EXPORT_POP_FADE_MS);
        let frames = stereo.len() / TARGET_CHANNELS as usize;
        let start_frame = (clip.start_secs.max(0.0) * TARGET_SAMPLE_RATE as f32).round() as usize;
        total_frames = total_frames.max(start_frame + frames);
        prepared.push((start_frame, stereo));
    }

    if total_frames == 0 {
        bail!("mixed audio is empty");
    }

    let mut mixed = vec![0.0f32; total_frames * TARGET_CHANNELS as usize];
    for (start_frame, stereo) in prepared {
        let start_sample = start_frame * TARGET_CHANNELS as usize;
        for (index, sample) in stereo.iter().enumerate() {
            let target = start_sample + index;
            if target < mixed.len() {
                mixed[target] += *sample;
            }
        }
    }

    let peak = mixed.iter().copied().fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    if peak > 1.0 {
        for sample in &mut mixed {
            *sample /= peak;
        }
    }

    let spec = WavSpec {
        channels: TARGET_CHANNELS,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer =
        WavWriter::create(target_path, spec).context("unable to create mixed wav file")?;
    for sample in &mixed {
        let pcm = ((*sample).clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_sample(pcm)
            .context("unable to write mixed wav sample")?;
    }
    writer.finalize().context("unable to finalize mixed wav file")?;
    Ok(())
}

fn convert_to_stereo(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 2 {
        return samples.to_vec();
    }

    let frames = samples.len() / channels;
    let mut stereo = Vec::with_capacity(frames * 2);
    for frame_index in 0..frames {
        let base = frame_index * channels;
        if channels == 1 {
            let sample = samples[base];
            stereo.push(sample);
            stereo.push(sample);
            continue;
        }

        let mut mixed = 0.0f32;
        for channel in 0..channels {
            mixed += samples[base + channel];
        }
        mixed /= channels as f32;
        stereo.push(mixed);
        stereo.push(mixed);
    }
    stereo
}

fn resample_stereo_linear(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.len() < 4 {
        return samples.to_vec();
    }

    let source_frames = samples.len() / 2;
    if source_frames <= 1 {
        return samples.to_vec();
    }

    let target_frames =
        ((source_frames as f64 * target_rate as f64) / source_rate as f64).round() as usize;
    let target_frames = target_frames.max(1);
    let ratio = source_rate as f64 / target_rate as f64;
    let mut out = Vec::with_capacity(target_frames * 2);

    for frame_index in 0..target_frames {
        let source_pos = frame_index as f64 * ratio;
        let left_index = source_pos.floor() as usize;
        let right_index = (left_index + 1).min(source_frames - 1);
        let frac = (source_pos - left_index as f64) as f32;

        let base_a = left_index * 2;
        let base_b = right_index * 2;
        let left = samples[base_a] + (samples[base_b] - samples[base_a]) * frac;
        let right = samples[base_a + 1] + (samples[base_b + 1] - samples[base_a + 1]) * frac;
        out.push(left);
        out.push(right);
    }

    out
}

fn slice_stereo_segment(
    samples: &[f32],
    sample_rate: u32,
    clip_start_secs: f32,
    clip_end_secs: f32,
) -> Vec<f32> {
    if samples.len() < 2 {
        return samples.to_vec();
    }

    let total_frames = samples.len() / 2;
    let start_frame =
        ((clip_start_secs.max(0.0) * sample_rate as f32).floor() as usize).min(total_frames);
    let mut end_frame =
        ((clip_end_secs.max(clip_start_secs + 0.05) * sample_rate as f32).ceil() as usize)
            .min(total_frames);
    if end_frame <= start_frame {
        end_frame = (start_frame + 1).min(total_frames);
    }
    let start_sample = start_frame * 2;
    let end_sample = (end_frame * 2).min(samples.len());
    samples[start_sample..end_sample].to_vec()
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

pub fn apply_sound_effects(
    samples: &mut [f32],
    channels: u16,
    sample_rate: u32,
    sound: &SoundEffect,
) {
    let channels = channels.max(1) as usize;
    if sound.telephone_enabled {
        apply_telephone_effect(samples, channels, sample_rate.max(1));
    }
    if sound.underwater_enabled {
        apply_underwater_effect(samples, channels, sample_rate.max(1));
    }
    if sound.reverb_enabled {
        apply_reverb_effect(samples, channels, sample_rate.max(1));
    }
    if sound.echo_enabled {
        apply_echo_effect(samples, channels, sample_rate.max(1));
    }
    if sound.distortion_enabled {
        apply_distortion_effect(samples, channels);
    }
    if sound.robot_enabled {
        apply_robot_effect(samples, channels, sample_rate.max(1));
    }
    if sound.pitch_shift_enabled {
        apply_pitch_shift_effect(
            samples,
            channels,
            sample_rate.max(1),
            sound.pitch_shift_semitones,
        );
    }
}

fn apply_distortion_effect(samples: &mut [f32], _channels: usize) {
    let gain = 3.5;
    for sample in samples.iter_mut() {
        let x = *sample * gain;
        *sample = x.tanh();
    }
}

fn apply_echo_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let delay_secs = 0.25;
    let feedback = 0.45;
    let wet = 0.5;
    let delay_samples = ((sample_rate as f32 * delay_secs).round() as usize).max(1) * channels;
    let mut delay_buffer = vec![0.0f32; delay_samples];
    let mut write_pos = 0;

    for sample_index in 0..samples.len() {
        let dry = samples[sample_index];
        let delayed = delay_buffer[write_pos];

        samples[sample_index] = dry + delayed * wet;
        delay_buffer[write_pos] = dry + delayed * feedback;

        write_pos += 1;
        if write_pos >= delay_samples {
            write_pos = 0;
        }
    }
}

fn apply_underwater_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let dt = 1.0 / sample_rate.max(1) as f32;
    let low_pass_cutoff = 350.0;
    let low_pass_rc = 1.0 / (std::f32::consts::TAU * low_pass_cutoff);
    let low_pass_alpha = dt / (low_pass_rc + dt);

    let mut lp_prev_y = vec![0.0f32; channels];

    for frame in samples.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let lp = lp_prev_y[channel] + low_pass_alpha * (x - lp_prev_y[channel]);
            lp_prev_y[channel] = lp;
            *sample = lp;
        }
    }
}

fn apply_robot_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let freq = 50.0;
    let t_step = 1.0 / sample_rate as f32;
    let comb_delay_secs = 0.005;
    let comb_delay_samples =
        ((sample_rate as f32 * comb_delay_secs).round() as usize).max(1) * channels;
    let feedback = 0.6;
    let mut comb_buffer = vec![0.0f32; comb_delay_samples];
    let mut write_pos = 0;

    for sample_index in 0..samples.len() {
        let time = (sample_index / channels) as f32 * t_step;
        let modulator = (std::f32::consts::TAU * freq * time).sin();
        let ring_mod = samples[sample_index] * (0.4 + 0.6 * modulator);

        let delayed = comb_buffer[write_pos];
        let comb_out = ring_mod + delayed * feedback;
        comb_buffer[write_pos] = comb_out;

        samples[sample_index] = comb_out.clamp(-1.0, 1.0);

        write_pos += 1;
        if write_pos >= comb_delay_samples {
            write_pos = 0;
        }
    }
}

fn apply_pitch_shift_effect(
    samples: &mut [f32],
    channels: usize,
    sample_rate: u32,
    semitones: f32,
) {
    if semitones.abs() < 0.05 {
        return;
    }
    let ratio = 2.0f32.powf(semitones / 12.0);
    let size = ((sample_rate as f32 * 0.08).round() as usize).max(256);
    let mut delay_buf = vec![vec![0.0f32; size]; channels];
    let mut write_pos = 0;

    let mut read_ptr_offset = 0.0f32;
    let original = samples.to_vec();

    for frame_idx in 0..(samples.len() / channels) {
        let delay_a = read_ptr_offset;
        let delay_b = (read_ptr_offset + (size as f32 / 2.0)) % size as f32;

        let w_a = if delay_a < (size as f32 / 2.0) {
            delay_a / (size as f32 / 2.0)
        } else {
            (size as f32 - delay_a) / (size as f32 / 2.0)
        };
        let w_b = 1.0 - w_a;

        for ch in 0..channels {
            let sample_val = original[frame_idx * channels + ch];
            delay_buf[ch][write_pos] = sample_val;

            let idx_a = (write_pos + size - delay_a.floor() as usize) % size;
            let idx_a_next = (idx_a + size - 1) % size;
            let frac_a = delay_a.fract();
            let val_a = delay_buf[ch][idx_a] * (1.0 - frac_a) + delay_buf[ch][idx_a_next] * frac_a;

            let idx_b = (write_pos + size - delay_b.floor() as usize) % size;
            let idx_b_next = (idx_b + size - 1) % size;
            let frac_b = delay_b.fract();
            let val_b = delay_buf[ch][idx_b] * (1.0 - frac_b) + delay_buf[ch][idx_b_next] * frac_b;

            samples[frame_idx * channels + ch] = val_a * w_a + val_b * w_b;
        }

        write_pos = (write_pos + 1) % size;
        read_ptr_offset += 1.0 - ratio;
        if read_ptr_offset < 0.0 {
            read_ptr_offset += size as f32;
        }
        read_ptr_offset %= size as f32;
    }
}

fn apply_telephone_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let dt = 1.0 / sample_rate.max(1) as f32;
    let high_pass_cutoff = 420.0;
    let low_pass_cutoff = 2_350.0;
    let high_pass_rc = 1.0 / (std::f32::consts::TAU * high_pass_cutoff);
    let low_pass_rc = 1.0 / (std::f32::consts::TAU * low_pass_cutoff);
    let high_pass_alpha = high_pass_rc / (high_pass_rc + dt);
    let low_pass_alpha = dt / (low_pass_rc + dt);

    let mut hp_prev_y = vec![0.0f32; channels];
    let mut hp_prev_x = vec![0.0f32; channels];
    let mut lp_prev_y = vec![0.0f32; channels];

    for frame in samples.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let hp = high_pass_alpha * (hp_prev_y[channel] + x - hp_prev_x[channel]);
            hp_prev_y[channel] = hp;
            hp_prev_x[channel] = x;

            let lp = lp_prev_y[channel] + low_pass_alpha * (hp - lp_prev_y[channel]);
            lp_prev_y[channel] = lp;

            *sample = (lp * 1.35).clamp(-1.0, 1.0);
        }
    }
}

fn apply_reverb_effect(samples: &mut [f32], channels: usize, sample_rate: u32) {
    let delay_a = ((sample_rate as f32 * 0.085).round() as usize).max(1) * channels;
    let delay_b = ((sample_rate as f32 * 0.16).round() as usize).max(1) * channels;
    let dry = 0.82f32;
    let wet_a = 0.24f32;
    let wet_b = 0.16f32;
    let feedback = 0.22f32;
    let original = samples.to_vec();

    for index in 0..samples.len() {
        let mut value = original[index] * dry;
        if index >= delay_a {
            value += original[index - delay_a] * wet_a;
        }
        if index >= delay_b {
            value += original[index - delay_b] * wet_b;
        }
        if index >= delay_b + delay_a {
            value += samples[index - delay_a] * feedback * 0.5;
        }
        samples[index] = value.clamp(-1.0, 1.0);
    }
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    Decoder::new(BufReader::new(file)).context("unsupported audio file")
}
