use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

const DEMUCS_RELEASE_URL: &str = "https://github.com/nikhilunni/demucs-rs/releases/download/v0.3.4/demucs-x86_64-pc-windows-msvc.zip";

/// Get the directory where demucs-rs is installed within the app data
fn demucs_install_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("soundfx_manager")
        .join("demucs")
}

/// Get the path to the demucs executable
fn demucs_exe_path() -> PathBuf {
    demucs_install_dir().join("demucs.exe")
}

fn demucs_model_ready_marker() -> PathBuf {
    demucs_install_dir().join(".model-ready")
}

/// Check if demucs-rs CLI is installed
pub fn is_demucs_installed() -> bool {
    demucs_exe_path().exists()
}

pub fn is_demucs_model_ready() -> bool {
    is_demucs_available() && demucs_model_ready_marker().exists()
}

pub fn clear_demucs_model_ready() -> Result<(), String> {
    let marker = demucs_model_ready_marker();
    if marker.exists() {
        fs::remove_file(&marker)
            .map_err(|error| format!("Failed to clear demucs model state: {error}"))?;
    }
    Ok(())
}

/// Check if demucs-rs CLI is available (either installed or in PATH)
pub fn is_demucs_available() -> bool {
    is_demucs_installed()
        || Command::new("demucs")
            .arg("--help")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Get the command to run demucs (uses installed version if available, falls back to PATH)
fn demucs_command() -> Command {
    let exe = if is_demucs_installed() {
        demucs_exe_path()
    } else {
        PathBuf::from("demucs")
    };
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Download and install demucs-rs CLI from GitHub releases.
/// Returns a progress callback receiver (progress 0.0..1.0, status message).
pub fn install_demucs() -> Result<(), String> {
    let install_dir = demucs_install_dir();
    fs::create_dir_all(&install_dir)
        .map_err(|e| format!("Failed to create install directory: {e}"))?;

    let zip_path = install_dir.join("demucs.zip");

    // Download the zip
    let resp = ureq::get(DEMUCS_RELEASE_URL)
        .call()
        .map_err(|e| format!("Failed to download demucs: {e}"))?;

    let total_size = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
        .unwrap_or(0);

    let limit = total_size.max(100 * 1024 * 1024); // at least 100MB
    let zip_data = resp
        .into_body()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| format!("Failed to read download data: {e}"))?;

    if total_size > 0 && zip_data.len() as u64 != total_size {
        return Err(format!(
            "Download incomplete: expected {total_size}, got {}",
            zip_data.len()
        ));
    }

    // Extract zip
    let cursor = std::io::Cursor::new(&zip_data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open zip: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry: {e}"))?;

        let out_path = match file.enclosed_name() {
            Some(path) => install_dir.join(path),
            None => continue,
        };

        if file.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent directory: {e}"))?;
            }
            let mut outfile =
                fs::File::create(&out_path).map_err(|e| format!("Failed to create file: {e}"))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Failed to write file: {e}"))?;
        }
    }

    // Clean up zip
    let _ = fs::remove_file(&zip_path);

    if !demucs_exe_path().exists() {
        return Err("demucs.exe not found after installation. The binary may not be in the expected location.".to_string());
    }

    Ok(())
}

pub fn uninstall_demucs() -> Result<(), String> {
    let install_dir = demucs_install_dir();
    if install_dir.exists() {
        fs::remove_dir_all(&install_dir)
            .map_err(|error| format!("Failed to uninstall demucs-rs: {error}"))?;
    }
    Ok(())
}

pub fn preload_demucs_model_cancellable(
    root_dir: &Path,
    cancel_flag: Arc<AtomicBool>,
) -> Result<bool, String> {
    if !is_demucs_available() {
        return Err("demucs-rs is not installed yet".to_owned());
    }

    let warmup_dir = root_dir.join("demucs-warmup");
    if warmup_dir.exists() {
        let _ = fs::remove_dir_all(&warmup_dir);
    }
    fs::create_dir_all(&warmup_dir)
        .map_err(|error| format!("Failed to create warmup directory: {error}"))?;

    let input_path = warmup_dir.join("warmup.wav");
    write_silent_wav(&input_path)
        .map_err(|error| format!("Failed to create warmup file: {error}"))?;
    let output_dir = warmup_dir.join("output");

    let result = (|| -> Result<bool, String> {
        let mut child = demucs_command()
            .arg(&input_path)
            .arg("-s")
            .arg("vocals")
            .arg("-o")
            .arg(&output_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Failed to run demucs: {error}"))?;

        loop {
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }

            if child
                .try_wait()
                .map_err(|error| format!("Failed waiting for demucs: {error}"))?
                .is_some()
            {
                break;
            }
            thread::sleep(Duration::from_millis(80));
        }

        let output = child
            .wait_with_output()
            .map_err(|error| format!("Failed to read demucs output: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("demucs failed: {stderr}"));
        }

        let vocal_path = output_dir.join("vocals.wav");
        if !vocal_path.exists() && find_named_wav_recursively(&output_dir, "vocals.wav").is_none() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Vocal output file not found after separation. stdout: {stdout} stderr: {stderr}"
            ));
        }

        fs::write(demucs_model_ready_marker(), b"ready")
            .map_err(|error| format!("Failed to save demucs model state: {error}"))?;
        Ok(true)
    })();

    let _ = fs::remove_dir_all(&warmup_dir);
    result
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
        return Err(
            "demucs-rs CLI not installed. Please install it from the Keep Vocal toggle."
                .to_string(),
        );
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

fn write_silent_wav(path: &Path) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for _ in 0..44_100 {
        writer.write_sample::<i16>(0)?;
    }
    writer.finalize()
}
