use anyhow::Result;
#[cfg(not(windows))]
use anyhow::bail;
#[cfg(windows)]
use anyhow::{Context, bail};
#[cfg(windows)]
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
#[cfg(windows)]
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInputConfig {
    pub route_system_audio: bool,
    pub route_microphone: bool,
    pub monitor_microphone: bool,
    pub microphone_device_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StreamInputSnapshot {
    pub running: bool,
    pub route_system_audio: bool,
    pub route_microphone: bool,
    pub monitor_microphone: bool,
    pub target_device_name: Option<String>,
    pub monitor_device_name: Option<String>,
    pub level: f32,
    pub waveform: Vec<f32>,
    pub error: Option<String>,
}

impl Default for StreamInputSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            route_system_audio: false,
            route_microphone: false,
            monitor_microphone: false,
            target_device_name: None,
            monitor_device_name: None,
            level: 0.0,
            waveform: Vec::new(),
            error: None,
        }
    }
}

pub struct StreamInputRouter {
    snapshot: Arc<Mutex<StreamInputSnapshot>>,
    stop_flag: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
    active_config: Option<StreamInputConfig>,
}

impl StreamInputRouter {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(StreamInputSnapshot::default())),
            stop_flag: None,
            worker: None,
            active_config: None,
        }
    }

    pub fn snapshot(&self) -> StreamInputSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn configure(&mut self, config: Option<StreamInputConfig>) -> Result<()> {
        if self.active_config == config {
            return Ok(());
        }

        self.stop();
        if let Some(config) = config {
            self.start(config)?;
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(stop_flag) = self.stop_flag.take() {
            stop_flag.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.active_config = None;
        *self.snapshot.lock().unwrap() = StreamInputSnapshot::default();
    }

    fn start(&mut self, config: StreamInputConfig) -> Result<()> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_flag);
        let snapshot = Arc::clone(&self.snapshot);
        let config_for_thread = config.clone();
        let worker = thread::Builder::new()
            .name("stream-input-router".to_owned())
            .spawn(move || {
                {
                    let mut state = snapshot.lock().unwrap();
                    state.running = true;
                    state.route_system_audio = config_for_thread.route_system_audio;
                    state.route_microphone = config_for_thread.route_microphone;
                    state.monitor_microphone = config_for_thread.monitor_microphone;
                    state.target_device_name = None;
                    state.monitor_device_name = None;
                    state.level = 0.0;
                    state.waveform.clear();
                    state.error = None;
                }
                let result = run_loop(Arc::clone(&snapshot), stop_for_thread, config_for_thread);
                let mut state = snapshot.lock().unwrap();
                state.running = false;
                state.level = 0.0;
                if let Err(error) = result {
                    state.error = Some(error.to_string());
                }
            })
            .context("unable to spawn stream input router thread")?;

        self.stop_flag = Some(stop_flag);
        self.worker = Some(worker);
        self.active_config = Some(config);
        Ok(())
    }
}

impl Drop for StreamInputRouter {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
const ROUTE_SAMPLE_RATE: usize = 44_100;
#[cfg(windows)]
const ROUTE_CHANNELS: usize = 2;
#[cfg(windows)]
const ROUTE_FRAME_BYTES: usize = ROUTE_CHANNELS * std::mem::size_of::<f32>();
#[cfg(windows)]
const MAX_SAMPLE_QUEUE: usize = ROUTE_SAMPLE_RATE * ROUTE_CHANNELS * 3;
#[cfg(windows)]
const STREAM_WAVE_BUCKETS: usize = 40;

#[cfg(windows)]
struct CaptureSource {
    audio_client: wasapi::AudioClient,
    capture_client: wasapi::AudioCaptureClient,
    byte_queue: VecDeque<u8>,
    sample_queue: VecDeque<f32>,
}

#[cfg(windows)]
struct RenderTarget {
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
}

#[cfg(windows)]
fn run_loop(
    snapshot: Arc<Mutex<StreamInputSnapshot>>,
    stop_flag: Arc<AtomicBool>,
    config: StreamInputConfig,
) -> Result<()> {
    use wasapi::{DeviceCollection, Direction, SampleType, WaveFormat, initialize_mta};

    let _ = initialize_mta();
    if !config.route_system_audio && !config.route_microphone && !config.monitor_microphone {
        bail!("No stream sources enabled");
    }

    let desired_format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        ROUTE_SAMPLE_RATE,
        ROUTE_CHANNELS,
        None,
    );
    let mut virtual_target = if config.route_system_audio || config.route_microphone {
        let render_devices = DeviceCollection::new(&Direction::Render)?;
        let (render_device, render_name) = find_cable_render_device(render_devices)?;
        {
            let mut state = snapshot.lock().unwrap();
            state.target_device_name = Some(render_name);
        }
        Some(open_render_target(render_device, &desired_format)?)
    } else {
        None
    };
    let mut monitor_target = if config.monitor_microphone {
        let monitor_device = resolve_system_loopback_device()?;
        let monitor_name = monitor_device
            .get_friendlyname()
            .unwrap_or_else(|_| "Default speaker".to_owned());
        {
            let mut state = snapshot.lock().unwrap();
            state.monitor_device_name = Some(monitor_name);
        }
        Some(open_render_target(monitor_device, &desired_format)?)
    } else {
        None
    };

    let mut system_source = if config.route_system_audio {
        Some(
            open_capture_source(PitchSource::System, None, &desired_format)
                .context("unable to capture system audio")?,
        )
    } else {
        None
    };
    let mut mic_source = if config.route_microphone || config.monitor_microphone {
        Some(
            open_capture_source(
                PitchSource::Microphone,
                config.microphone_device_name.as_deref(),
                &desired_format,
            )
            .context("unable to capture microphone audio")?,
        )
    } else {
        None
    };

    if let Some(target) = virtual_target.as_mut() {
        target.audio_client.start_stream()?;
    }
    if let Some(target) = monitor_target.as_mut() {
        target.audio_client.start_stream()?;
    }

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        if let Some(source) = system_source.as_mut() {
            drain_capture_source(source)?;
        }
        if let Some(source) = mic_source.as_mut() {
            drain_capture_source(source)?;
        }

        let mut available_frames: Option<usize> = None;
        if let Some(target) = virtual_target.as_ref() {
            available_frames = Some(target.audio_client.get_available_space_in_frames()? as usize);
        }
        if let Some(target) = monitor_target.as_ref() {
            let monitor_frames = target.audio_client.get_available_space_in_frames()? as usize;
            available_frames = Some(
                available_frames.map_or(monitor_frames, |current| current.min(monitor_frames)),
            );
        }
        let available_frames = available_frames.unwrap_or(0);
        if available_frames > 0 {
            let mut route_data = Vec::with_capacity(available_frames * ROUTE_FRAME_BYTES);
            let mut monitor_data = Vec::with_capacity(available_frames * ROUTE_FRAME_BYTES);
            let mut level_sum = 0.0f32;
            for _ in 0..available_frames {
                let system_frame = system_source
                    .as_mut()
                    .and_then(|source| pop_stereo_frame(&mut source.sample_queue));
                let mic_frame = mic_source
                    .as_mut()
                    .and_then(|source| pop_stereo_frame(&mut source.sample_queue));

                let routed_mic = if config.route_microphone {
                    mic_frame
                } else {
                    None
                };
                let route_sources =
                    usize::from(system_frame.is_some()) + usize::from(routed_mic.is_some());
                let route_gain = match route_sources {
                    0 => 0.0,
                    1 => 0.94,
                    _ => 0.68,
                };
                let route_left = (system_frame.map(|frame| frame[0]).unwrap_or(0.0)
                    + routed_mic.map(|frame| frame[0]).unwrap_or(0.0))
                    * route_gain;
                let route_right = (system_frame.map(|frame| frame[1]).unwrap_or(0.0)
                    + routed_mic.map(|frame| frame[1]).unwrap_or(0.0))
                    * route_gain;
                let mixed_left = route_left.clamp(-1.0, 1.0);
                let mixed_right = route_right.clamp(-1.0, 1.0);
                if virtual_target.is_some() {
                    route_data.extend_from_slice(&mixed_left.to_le_bytes());
                    route_data.extend_from_slice(&mixed_right.to_le_bytes());
                }

                let monitor_left = mic_frame.map(|frame| frame[0]).unwrap_or(0.0) * 0.92;
                let monitor_right = mic_frame.map(|frame| frame[1]).unwrap_or(0.0) * 0.92;
                let monitor_left = monitor_left.clamp(-1.0, 1.0);
                let monitor_right = monitor_right.clamp(-1.0, 1.0);
                if monitor_target.is_some() {
                    monitor_data.extend_from_slice(&monitor_left.to_le_bytes());
                    monitor_data.extend_from_slice(&monitor_right.to_le_bytes());
                }

                let level_left = if config.monitor_microphone {
                    monitor_left
                } else {
                    mixed_left
                };
                let level_right = if config.monitor_microphone {
                    monitor_right
                } else {
                    mixed_right
                };
                level_sum += ((level_left.abs() + level_right.abs()) * 0.5).clamp(0.0, 1.0);
            }
            if let Some(target) = virtual_target.as_mut() {
                target
                    .render_client
                    .write_to_device(available_frames, &route_data, None)?;
            }
            if let Some(target) = monitor_target.as_mut() {
                target
                    .render_client
                    .write_to_device(available_frames, &monitor_data, None)?;
            }

            let instant_level = if available_frames == 0 {
                0.0
            } else {
                (level_sum / available_frames as f32).clamp(0.0, 1.0)
            };
            let mut state = snapshot.lock().unwrap();
            state.level = state.level * 0.72 + instant_level * 0.28;
            let wave_level = state.level.max(instant_level);
            push_wave_level(&mut state.waveform, wave_level);
            state.error = None;
        }
        thread::sleep(Duration::from_millis(8));
    }

    if let Some(source) = system_source.as_mut() {
        let _ = source.audio_client.stop_stream();
    }
    if let Some(source) = mic_source.as_mut() {
        let _ = source.audio_client.stop_stream();
    }
    if let Some(target) = virtual_target.as_mut() {
        let _ = target.audio_client.stop_stream();
    }
    if let Some(target) = monitor_target.as_mut() {
        let _ = target.audio_client.stop_stream();
    }
    Ok(())
}

#[cfg(windows)]
fn open_render_target(
    device: wasapi::Device,
    desired_format: &wasapi::WaveFormat,
) -> Result<RenderTarget> {
    use wasapi::{Direction, StreamMode};

    let mut audio_client = device.get_iaudioclient()?;
    let (_, render_min_time) = audio_client.get_device_period()?;
    audio_client.initialize_client(
        desired_format,
        &Direction::Render,
        &StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: render_min_time,
        },
    )?;
    let _ = audio_client.set_get_eventhandle()?;
    let initial_frames = audio_client.get_buffer_size()? as usize;
    let render_client = audio_client.get_audiorenderclient()?;
    if initial_frames > 0 {
        let silence = vec![0u8; initial_frames * ROUTE_FRAME_BYTES];
        render_client.write_to_device(initial_frames, &silence, None)?;
    }
    Ok(RenderTarget {
        audio_client,
        render_client,
    })
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum PitchSource {
    System,
    Microphone,
}

#[cfg(windows)]
fn open_capture_source(
    source: PitchSource,
    device_name: Option<&str>,
    desired_format: &wasapi::WaveFormat,
) -> Result<CaptureSource> {
    use wasapi::{DeviceCollection, Direction, StreamMode, get_default_device};

    let device = match source {
        PitchSource::System => resolve_system_loopback_device()?,
        PitchSource::Microphone => {
            if let Some(device_name) = device_name {
                DeviceCollection::new(&Direction::Capture)?.get_device_with_name(device_name)?
            } else {
                get_default_device(&Direction::Capture)?
            }
        }
    };

    let mut audio_client = device.get_iaudioclient()?;
    let (_, min_time) = audio_client.get_device_period()?;
    audio_client.initialize_client(
        desired_format,
        &Direction::Capture,
        &StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: min_time,
        },
    )?;
    let _ = audio_client.set_get_eventhandle()?;
    let capture_client = audio_client.get_audiocaptureclient()?;
    audio_client.start_stream()?;

    Ok(CaptureSource {
        audio_client,
        capture_client,
        byte_queue: VecDeque::with_capacity(ROUTE_FRAME_BYTES * 4096),
        sample_queue: VecDeque::with_capacity(MAX_SAMPLE_QUEUE),
    })
}

#[cfg(windows)]
fn resolve_system_loopback_device() -> Result<wasapi::Device> {
    use wasapi::{DeviceCollection, Direction, get_default_device};

    let default_device = get_default_device(&Direction::Render)?;
    let default_name = default_device
        .get_friendlyname()
        .unwrap_or_else(|_| String::new())
        .to_ascii_lowercase();
    if !default_name.contains("cable input") {
        return Ok(default_device);
    }

    let devices = DeviceCollection::new(&Direction::Render)?;
    for device in &devices {
        let device = device?;
        let name = device.get_friendlyname()?.to_ascii_lowercase();
        if !name.contains("cable input") && !name.contains("cable output") {
            return Ok(device);
        }
    }

    Ok(default_device)
}

#[cfg(windows)]
fn drain_capture_source(source: &mut CaptureSource) -> Result<()> {
    loop {
        let Some(nbr_frames) = source.capture_client.get_next_packet_size()? else {
            break;
        };
        if nbr_frames == 0 {
            break;
        }

        source
            .capture_client
            .read_from_device_to_deque(&mut source.byte_queue)?;
        while source.byte_queue.len() >= std::mem::size_of::<f32>() {
            let mut bytes = [0u8; 4];
            for byte in &mut bytes {
                *byte = source.byte_queue.pop_front().unwrap_or_default();
            }
            source.sample_queue.push_back(f32::from_le_bytes(bytes));
        }
        while source.sample_queue.len() > MAX_SAMPLE_QUEUE {
            let _ = source.sample_queue.pop_front();
        }
    }
    Ok(())
}

#[cfg(windows)]
fn pop_stereo_frame(queue: &mut VecDeque<f32>) -> Option<[f32; 2]> {
    if queue.len() < ROUTE_CHANNELS {
        return None;
    }
    let left = queue.pop_front().unwrap_or_default();
    let right = queue.pop_front().unwrap_or(left);
    Some([left, right])
}

#[cfg(windows)]
fn push_wave_level(waveform: &mut Vec<f32>, level: f32) {
    waveform.push(level.clamp(0.04, 1.0));
    if waveform.len() > STREAM_WAVE_BUCKETS {
        let drain_count = waveform.len() - STREAM_WAVE_BUCKETS;
        waveform.drain(0..drain_count);
    }
}

#[cfg(windows)]
fn find_cable_render_device(devices: wasapi::DeviceCollection) -> Result<(wasapi::Device, String)> {
    let mut fallback: Option<(wasapi::Device, String)> = None;
    let mut available_devices = Vec::new();
    for device in &devices {
        let device = device?;
        let name = device.get_friendlyname()?;
        let lower = name.to_ascii_lowercase();
        available_devices.push(name.clone());
        if lower.contains("cable input") {
            return Ok((device, name));
        }
        if fallback.is_none()
            && (lower.contains("vb-cable")
                || lower.contains("vb-audio")
                || lower.contains("vb audio")
                || lower.contains("virtual cable")
                || (lower.contains("cable") && lower.contains("input")))
        {
            fallback = Some((device, name));
        }
    }

    fallback.ok_or_else(|| {
        let device_list = if available_devices.is_empty() {
            "none".to_owned()
        } else {
            available_devices.join(" | ")
        };
        anyhow::anyhow!(
            "VB-CABLE playback endpoint was not found.\nWindows playback devices: {device_list}"
        )
    })
}

#[cfg(not(windows))]
fn run_loop(
    _snapshot: Arc<Mutex<StreamInputSnapshot>>,
    _stop_flag: Arc<AtomicBool>,
    _config: StreamInputConfig,
) -> Result<()> {
    bail!("Stream input routing is only available on Windows")
}
