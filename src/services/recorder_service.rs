use crate::pitch::PitchInputSource;
use anyhow::Result;
#[cfg(not(windows))]
use anyhow::bail;
#[cfg(windows)]
use anyhow::{Context, bail};
#[cfg(windows)]
use hound::{SampleFormat, WavSpec, WavWriter};
#[cfg(windows)]
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
#[cfg(windows)]
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct RecorderSnapshot {
    pub running: bool,
    pub elapsed_secs: f32,
    pub level: f32,
    pub waveform: Vec<f32>,
    pub error: Option<String>,
}

impl Default for RecorderSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            elapsed_secs: 0.0,
            level: 0.0,
            waveform: vec![0.04; 40],
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub source: PitchInputSource,
    pub input_device_name: Option<String>,
    pub output_path: PathBuf,
}

pub struct Recorder {
    state: Arc<Mutex<RecorderSnapshot>>,
    completed_path: Arc<Mutex<Option<PathBuf>>>,
    stop_flag: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

const RECORD_WAVE_BARS: usize = 40;

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecorderSnapshot::default())),
            completed_path: Arc::new(Mutex::new(None)),
            stop_flag: None,
            worker: None,
        }
    }

    pub fn snapshot(&self) -> RecorderSnapshot {
        self.state.lock().unwrap().clone()
    }

    pub fn start(&mut self, config: RecorderConfig) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        *self.completed_path.lock().unwrap() = None;
        {
            let mut snapshot = self.state.lock().unwrap();
            snapshot.running = true;
            snapshot.elapsed_secs = 0.0;
            snapshot.level = 0.0;
            snapshot.waveform = vec![0.04; RECORD_WAVE_BARS];
            snapshot.error = None;
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let state = Arc::clone(&self.state);
        let completed_path = Arc::clone(&self.completed_path);
        let stop_for_thread = Arc::clone(&stop_flag);

        let worker = thread::Builder::new()
            .name("audio-recorder".to_owned())
            .spawn(move || {
                let result = run_loop(
                    Arc::clone(&state),
                    Arc::clone(&completed_path),
                    Arc::clone(&stop_for_thread),
                    config,
                );
                let mut snapshot = state.lock().unwrap();
                snapshot.running = false;
                if let Err(error) = result {
                    snapshot.error = Some(error.to_string());
                }
            })?;

        self.stop_flag = Some(stop_flag);
        self.worker = Some(worker);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(stop_flag) = self.stop_flag.take() {
            stop_flag.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut snapshot = self.state.lock().unwrap();
        snapshot.running = false;
    }

    pub fn take_completed_path(&self) -> Option<PathBuf> {
        self.completed_path.lock().unwrap().take()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
fn run_loop(
    state: Arc<Mutex<RecorderSnapshot>>,
    completed_path: Arc<Mutex<Option<PathBuf>>>,
    stop_flag: Arc<AtomicBool>,
    config: RecorderConfig,
) -> Result<()> {
    use wasapi::{
        DeviceCollection, Direction, SampleType, StreamMode, WaveFormat, get_default_device,
        initialize_mta,
    };

    let _ = initialize_mta();
    if let Some(parent) = config.output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }

    let device = match config.source {
        PitchInputSource::System => get_default_device(&Direction::Render)?,
        PitchInputSource::Microphone => {
            if let Some(device_name) = config.input_device_name.as_deref() {
                DeviceCollection::new(&Direction::Capture)?.get_device_with_name(device_name)?
            } else {
                get_default_device(&Direction::Capture)?
            }
        }
    };

    let mut audio_client = device.get_iaudioclient()?;
    let desired_format = WaveFormat::new(32, 32, &SampleType::Float, 44_100, 2, None);
    let blockalign = desired_format.get_blockalign() as usize;
    let (_, min_time) = audio_client.get_device_period()?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };
    audio_client.initialize_client(&desired_format, &Direction::Capture, &mode)?;

    let event_handle = audio_client.set_get_eventhandle()?;
    let buffer_frame_count = audio_client.get_buffer_size()? as usize;
    let capture_client = audio_client.get_audiocaptureclient()?;
    let chunk_frames = 512usize;
    let chunk_bytes = chunk_frames * blockalign;
    let mut sample_queue =
        VecDeque::with_capacity(blockalign * (chunk_frames + buffer_frame_count * 4));
    let mut level_history = VecDeque::from(vec![0.04; RECORD_WAVE_BARS]);
    let started_at = Instant::now();
    let mut writer = WavWriter::create(
        &config.output_path,
        WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .context("unable to create recording file")?;
    let mut written_samples = 0usize;

    audio_client.start_stream()?;
    while !stop_flag.load(Ordering::Relaxed) {
        let _ = event_handle.wait_for_event(20);

        loop {
            let nbr_frames = match capture_client.get_next_packet_size()? {
                Some(frames) if frames > 0 => frames,
                _ => break,
            };
            let additional = (nbr_frames as usize * blockalign)
                .saturating_sub(sample_queue.capacity().saturating_sub(sample_queue.len()));
            sample_queue.reserve(additional);
            capture_client.read_from_device_to_deque(&mut sample_queue)?;
        }

        while sample_queue.len() >= chunk_bytes {
            let mut chunk = vec![0u8; chunk_bytes];
            for value in &mut chunk {
                *value = sample_queue.pop_front().unwrap_or_default();
            }

            let mono = bytes_to_mono_samples(&chunk, 2);
            let level = level_to_visual(rms_level(&mono));
            level_history.push_back(level.clamp(0.04, 1.0));
            while level_history.len() > RECORD_WAVE_BARS {
                let _ = level_history.pop_front();
            }

            for sample in chunk.chunks_exact(4) {
                let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]])
                    .clamp(-1.0, 1.0);
                writer
                    .write_sample((value * i16::MAX as f32).round() as i16)
                    .context("unable to write recording sample")?;
                written_samples += 1;
            }

            let mut snapshot = state.lock().unwrap();
            snapshot.running = true;
            snapshot.elapsed_secs = started_at.elapsed().as_secs_f32();
            snapshot.level = level;
            snapshot.waveform = level_history.iter().copied().collect();
            snapshot.error = None;
        }
    }

    let _ = audio_client.stop_stream();

    // Drain and flush any remaining trailing packets before closing WAV writer
    loop {
        let nbr_frames = match capture_client.get_next_packet_size() {
            Ok(Some(frames)) if frames > 0 => frames,
            _ => break,
        };
        let additional = (nbr_frames as usize * blockalign)
            .saturating_sub(sample_queue.capacity().saturating_sub(sample_queue.len()));
        sample_queue.reserve(additional);
        let _ = capture_client.read_from_device_to_deque(&mut sample_queue);
    }
    while sample_queue.len() >= 4 {
        let mut sample_bytes = [0u8; 4];
        for b in &mut sample_bytes {
            *b = sample_queue.pop_front().unwrap_or_default();
        }
        let value = f32::from_le_bytes(sample_bytes).clamp(-1.0, 1.0);
        writer
            .write_sample((value * i16::MAX as f32).round() as i16)
            .context("unable to write recording sample")?;
        written_samples += 1;
    }

    writer
        .finalize()
        .context("unable to finalize recording file")?;
    if written_samples == 0 {
        let _ = std::fs::remove_file(&config.output_path);
        bail!("Recording is empty");
    }

    *completed_path.lock().unwrap() = Some(config.output_path);
    Ok(())
}

#[cfg(not(windows))]
fn run_loop(
    _state: Arc<Mutex<RecorderSnapshot>>,
    _completed_path: Arc<Mutex<Option<PathBuf>>>,
    _stop_flag: Arc<AtomicBool>,
    _config: RecorderConfig,
) -> Result<()> {
    bail!("Recording is only available on Windows")
}

#[cfg(windows)]
fn bytes_to_mono_samples(bytes: &[u8], channels: usize) -> Vec<f32> {
    let frame_width = channels * std::mem::size_of::<f32>();
    let mut mono = Vec::with_capacity(bytes.len() / frame_width.max(1));

    for frame in bytes.chunks_exact(frame_width.max(4)) {
        let mut mixed = 0.0f32;
        let mut used = 0usize;
        for sample in frame.chunks_exact(4).take(channels) {
            mixed += f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
            used += 1;
        }
        mono.push(mixed / used.max(1) as f32);
    }

    mono
}

#[cfg(windows)]
fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum = samples.iter().map(|sample| sample * sample).sum::<f32>();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(windows)]
fn level_to_visual(level: f32) -> f32 {
    (level * 8.0).clamp(0.0, 1.0).powf(0.55).clamp(0.04, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_wasapi_capture_init() {
        use wasapi::{Direction, get_default_device, initialize_mta, WaveFormat, SampleType, StreamMode};
        let _ = initialize_mta();
        if let Ok(device) = get_default_device(&Direction::Capture) {
            if let Ok(mut audio_client) = device.get_iaudioclient() {
                let desired_format = WaveFormat::new(32, 32, &SampleType::Float, 44_100, 2, None);
                let (_, min_time) = audio_client.get_device_period().unwrap_or((0, 100000));
                let mode = StreamMode::EventsShared {
                    autoconvert: true,
                    buffer_duration_hns: min_time,
                };
                if audio_client.initialize_client(&desired_format, &Direction::Capture, &mode).is_ok() {
                    let event_handle = audio_client.set_get_eventhandle().unwrap();
                    let cap_client = audio_client.get_audiocaptureclient().unwrap();
                    audio_client.start_stream().unwrap();
                    let mut q = std::collections::VecDeque::new();
                    let start = std::time::Instant::now();
                    while start.elapsed().as_secs_f32() < 0.2 {
                        let _ = event_handle.wait_for_event(20);
                        loop {
                            let nbr = cap_client.get_next_packet_size().unwrap_or(None).unwrap_or(0);
                            if nbr == 0 { break; }
                            let _ = cap_client.read_from_device_to_deque(&mut q);
                        }
                    }
                    let _ = audio_client.stop_stream();
                    assert!(!q.is_empty(), "Capture queue should receive audio bytes");
                    let bytes: Vec<u8> = q.into_iter().collect();
                    let mono = bytes_to_mono_samples(&bytes, 2);
                    assert!(!mono.is_empty(), "Converted mono samples should not be empty");
                }
            }
        }
    }
}
