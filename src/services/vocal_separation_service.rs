use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

fn bundled_demucs_exe_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("soundfx_manager")
        .join("demucs")
        .join("demucs.exe")
}

fn command_with_hidden_window(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Check if demucs-rs CLI is available (either installed or in PATH)
pub fn is_demucs_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let bundled = bundled_demucs_exe_path();
        (bundled.exists()
            && command_with_hidden_window(&bundled)
                .arg("--help")
                .output()
                .is_ok_and(|output| output.status.success()))
            || command_with_hidden_window("demucs")
                .arg("--help")
                .output()
                .is_ok_and(|output| output.status.success())
    })
}

fn demucs_command() -> Command {
    let bundled = bundled_demucs_exe_path();
    if bundled.exists() {
        command_with_hidden_window(bundled)
    } else {
        command_with_hidden_window("demucs")
    }
}

/// Separate audio and extract only the vocal stem using demucs-rs CLI.
/// Returns the path to the vocal-only WAV file.
#[allow(dead_code)]
pub fn extract_vocals(input_path: &Path, output_dir: &Path) -> Result<PathBuf, String> {
    extract_vocals_cancellable(input_path, output_dir, Arc::new(AtomicBool::new(false)))
}

pub fn extract_vocals_cancellable(
    input_path: &Path,
    output_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    run_demucs_cancellable(input_path, output_dir, Some("vocals"), Arc::clone(&cancel))?;

    // demucs-rs CLI can output directly to <output_dir>/vocals.wav or place stems in nested
    // subdirectories depending on the build / model layout.
    let vocal_path = output_dir.join("vocals.wav");
    if vocal_path.exists() {
        return Ok(vocal_path);
    }

    if let Some(path) = find_named_wav_recursively(output_dir, "vocals.wav") {
        return Ok(path);
    }

    Err("Vocal output file not found after separation".to_owned())
}

pub fn extract_instrumental_cancellable(
    input_path: &Path,
    output_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    run_demucs_cancellable(input_path, output_dir, None, cancel)?;

    let stem_paths = find_instrument_stems(output_dir);
    if stem_paths.is_empty() {
        return Err("Instrumental stems were not found after separation".to_owned());
    }

    let instrumental_path = output_dir.join("instrumental.wav");
    mix_wav_stems(&stem_paths, &instrumental_path)?;
    Ok(instrumental_path)
}

fn run_demucs_cancellable(
    input_path: &Path,
    output_dir: &Path,
    stems: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    if !is_demucs_available() {
        return Err("demucs CLI is unavailable on this system.".to_string());
    }

    fs::create_dir_all(output_dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    let mut command = demucs_command();
    command.arg(input_path);
    if let Some(stems) = stems {
        command.arg("-s").arg(stems);
    }
    let mut child = command
        .arg("-o")
        .arg(output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run demucs: {e}"))?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".to_string());
        }

        match child
            .try_wait()
            .map_err(|e| format!("Failed waiting for demucs: {e}"))?
        {
            Some(_) => break,
            None => thread::sleep(Duration::from_millis(200)),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for demucs: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("demucs failed: {stderr}"));
    }
    Ok(())
}

fn find_named_wav_recursively(root: &Path, target_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(target_name))
        {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_named_wav_recursively(&path, target_name)
        {
            return Some(found);
        }
    }
    None
}

fn find_instrument_stems(root: &Path) -> Vec<PathBuf> {
    let mut stems = Vec::new();
    collect_instrument_stems(root, &mut stems);
    stems
}

fn collect_instrument_stems(root: &Path, stems: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_instrument_stems(&path, stems);
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".wav") {
            continue;
        }
        if file_name.eq_ignore_ascii_case("vocals.wav")
            || file_name.eq_ignore_ascii_case("instrumental.wav")
        {
            continue;
        }
        stems.push(path);
    }
}

fn mix_wav_stems(input_paths: &[PathBuf], output_path: &Path) -> Result<(), String> {
    let mut combined: Vec<f32> = Vec::new();
    let mut output_channels = 0u16;
    let mut output_sample_rate = 0u32;

    for input_path in input_paths {
        let mut reader = hound::WavReader::open(input_path)
            .map_err(|error| format!("Failed to open stem {}: {error}", input_path.display()))?;
        let spec = reader.spec();
        if output_channels == 0 {
            output_channels = spec.channels;
            output_sample_rate = spec.sample_rate;
        } else if spec.channels != output_channels || spec.sample_rate != output_sample_rate {
            return Err(format!("Stem format mismatch in {}", input_path.display()));
        }

        let samples = read_wav_samples_as_f32(&mut reader, spec)?;
        if combined.len() < samples.len() {
            combined.resize(samples.len(), 0.0);
        }
        for (index, sample) in samples.into_iter().enumerate() {
            combined[index] += sample;
        }
    }

    let spec = hound::WavSpec {
        channels: output_channels.max(1),
        sample_rate: output_sample_rate.max(44_100),
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(output_path, spec)
        .map_err(|error| format!("Failed to create instrumental file: {error}"))?;
    for sample in combined {
        writer
            .write_sample(sample.clamp(-1.0, 1.0))
            .map_err(|error| format!("Failed to write instrumental samples: {error}"))?;
    }
    writer
        .finalize()
        .map_err(|error| format!("Failed to finalize instrumental file: {error}"))?;
    Ok(())
}

fn read_wav_samples_as_f32(
    reader: &mut hound::WavReader<std::io::BufReader<std::fs::File>>,
    spec: hound::WavSpec,
) -> Result<Vec<f32>, String> {
    match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(|error| error.to_string()))
            .collect(),
        hound::SampleFormat::Int => {
            if spec.bits_per_sample <= 16 {
                reader
                    .samples::<i16>()
                    .map(|sample| {
                        sample
                            .map(|value| value as f32 / i16::MAX as f32)
                            .map_err(|error| error.to_string())
                    })
                    .collect()
            } else {
                let scale = ((1i64 << (spec.bits_per_sample.saturating_sub(1) as u32)) - 1) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| {
                        sample
                            .map(|value| value as f32 / scale.max(1.0))
                            .map_err(|error| error.to_string())
                    })
                    .collect()
            }
        }
    }
}
