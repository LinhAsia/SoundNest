use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Check if demucs-rs CLI is installed
pub fn is_demucs_installed() -> bool {
    demucs_exe_path().exists()
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

/// Separate audio and extract only the vocal stem using demucs-rs CLI.
/// Returns the path to the vocal-only WAV file.
pub fn extract_vocals(input_path: &Path, output_dir: &Path) -> Result<PathBuf, String> {
    if !is_demucs_available() {
        return Err(
            "demucs-rs CLI not installed. Please install it from the Keep Vocal toggle."
                .to_string(),
        );
    }

    fs::create_dir_all(output_dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    let output = demucs_command()
        .arg(input_path)
        .arg("-s")
        .arg("vocals")
        .arg("-o")
        .arg(output_dir)
        .output()
        .map_err(|e| format!("Failed to run demucs: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("demucs failed: {stderr}"));
    }

    // demucs-rs CLI can output directly to <output_dir>/vocals.wav or place stems in nested
    // subdirectories depending on the build / model layout.
    let vocal_path = output_dir.join("vocals.wav");
    if vocal_path.exists() {
        return Ok(vocal_path);
    }

    if let Some(path) = find_vocals_recursively(output_dir) {
        return Ok(path);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "Vocal output file not found after separation. stdout: {stdout} stderr: {stderr}"
    ))
}

fn find_vocals_recursively(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("vocals.wav"))
        {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_vocals_recursively(&path)
        {
            return Some(found);
        }
    }
    None
}
