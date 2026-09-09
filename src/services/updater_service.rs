use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const UPDATE_MANIFEST_URL: &str =
    "https://github.com/LinhAsia/soundnest/raw/master/update.json";
pub const UPDATE_MANIFEST_FALLBACK_URL: &str =
    "https://raw.githubusercontent.com/LinhAsia/soundnest/master/update.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Available {
        version: String,
        notes: String,
        url: String,
    },
    Downloading {
        version: String,
        progress: f32,
    },
    ReadyToRestart {
        version: String,
        new_exe_path: PathBuf,
    },
    UpToDate,
    Error(String),
}

pub fn update_download_file_stem(version: &str) -> String {
    let sanitized: String = version
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => ch,
            _ => '_',
        })
        .collect();
    format!("soundnest_update_{}", sanitized.trim_matches('_'))
}

pub fn update_download_final_path(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}.exe", update_download_file_stem(version)))
}

pub fn update_download_partial_path(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}.part", update_download_file_stem(version)))
}

pub fn update_download_ready_path(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}.ready", update_download_file_stem(version)))
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse_parts = |v: &str| -> Vec<u32> {
        v.trim()
            .trim_start_matches('v')
            .trim_start_matches('V')
            .split('.')
            .map(|s| s.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let p_latest = parse_parts(latest);
    let p_current = parse_parts(current);
    let max_len = p_latest.len().max(p_current.len());
    for i in 0..max_len {
        let l = *p_latest.get(i).unwrap_or(&0);
        let c = *p_current.get(i).unwrap_or(&0);
        if l > c {
            return true;
        }
        if l < c {
            return false;
        }
    }
    false
}

fn fetch_manifest_internal() -> Result<UpdateManifest> {
    let cache_buster = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let url = format!("{UPDATE_MANIFEST_URL}?ts={cache_buster}");

    let res = ureq::get(&url)
        .header("User-Agent", "SoundNest-Updater")
        .call();

    let res = match res {
        Ok(resp) => resp,
        Err(e) => {
            let err_text = e.to_string();
            let fallback_url = format!("{UPDATE_MANIFEST_FALLBACK_URL}?ts={cache_buster}");
            match ureq::get(&fallback_url)
                .header("User-Agent", "SoundNest-Updater")
                .call()
            {
                Ok(resp) => resp,
                Err(e2) => {
                    let err2_text = e2.to_string();
                    if err_text.contains("404") || err2_text.contains("404") {
                        bail!("Chưa tìm thấy update.json trên GitHub (HTTP 404). Hãy push repository lên GitHub để kích hoạt.");
                    } else {
                        bail!("Không thể kết nối máy chủ cập nhật: {err2_text}");
                    }
                }
            }
        }
    };

    let mut body = String::new();
    res.into_body()
        .into_reader()
        .read_to_string(&mut body)
        .context("Failed to read update manifest body")?;

    let manifest: UpdateManifest =
        serde_json::from_str(&body).context("Failed to parse update manifest JSON")?;
    Ok(manifest)
}

pub fn check_for_update(
    current_version: String,
    callback: impl FnOnce(Result<Option<UpdateManifest>, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        let result = fetch_manifest_internal().map_err(|e| e.to_string());
        match result {
            Ok(manifest) => {
                let latest_clean = manifest
                    .version
                    .trim()
                    .trim_start_matches('v')
                    .trim_start_matches('V');
                if is_newer_version(latest_clean, &current_version) {
                    callback(Ok(Some(manifest)));
                } else {
                    callback(Ok(None));
                }
            }
            Err(e) => callback(Err(e)),
        }
    });
}

pub fn start_download_update(
    version: String,
    download_url: String,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    callback: impl FnOnce(Result<PathBuf, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        let result: Result<PathBuf> = (|| {
            let temp_part_path = update_download_partial_path(&version);
            let temp_path = update_download_final_path(&version);
            let ready_path = update_download_ready_path(&version);

            let _ = fs::remove_file(&temp_part_path);
            let _ = fs::remove_file(&ready_path);

            let resp = ureq::get(&download_url)
                .header("User-Agent", "SoundNest-Updater")
                .call()
                .map_err(|e| anyhow::anyhow!("Failed to download update: {e}"))?;

            let total_size = resp
                .headers()
                .get("Content-Length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);

            let mut reader = resp.into_body().into_reader();
            let mut file = fs::File::create(&temp_part_path)
                .with_context(|| format!("unable to create {}", temp_part_path.display()))?;

            let mut downloaded = 0u64;
            let mut buffer = [0u8; 16_384];

            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = fs::remove_file(&temp_part_path);
                    bail!("Download canceled");
                }

                let bytes_read = reader.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }

                file.write_all(&buffer[..bytes_read])?;
                downloaded += bytes_read as u64;

                if total_size > 0 {
                    let value = ((downloaded as f32 / total_size as f32) * 1000.0)
                        .round()
                        .clamp(0.0, 1000.0) as u32;
                    progress.store(value, Ordering::SeqCst);
                }
            }

            file.flush()?;
            drop(file);

            fs::rename(&temp_part_path, &temp_path)?;
            fs::write(&ready_path, version.as_bytes())?;
            progress.store(1000, Ordering::SeqCst);

            Ok(temp_path)
        })();

        match result {
            Ok(path) => callback(Ok(path)),
            Err(e) => callback(Err(e.to_string())),
        }
    });
}

pub fn restart_and_apply_update(new_exe_path: &Path) -> Result<()> {
    if !new_exe_path.exists() {
        bail!("Downloaded update file was not found");
    }

    let current_exe = std::env::current_exe().context("Unable to get current exe path")?;
    let old_exe = current_exe.with_extension("exe.old");
    let ready_stamp = new_exe_path.with_extension("ready");
    let current_pid = std::process::id();
    let update_error_log = std::env::temp_dir().join("soundnest_update_error.txt");

    let current_exe_ps = current_exe.display().to_string().replace('\'', "''");
    let new_exe_ps = new_exe_path.display().to_string().replace('\'', "''");
    let old_exe_ps = old_exe.display().to_string().replace('\'', "''");
    let ready_stamp_ps = ready_stamp.display().to_string().replace('\'', "''");
    let current_dir_ps = current_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .display()
        .to_string()
        .replace('\'', "''");
    let error_log_ps = update_error_log.display().to_string().replace('\'', "''");

    let helper = format!(
        "$ErrorActionPreference='Stop'; \
         $pidToWait={current_pid}; \
         $currentExe='{current_exe_ps}'; \
         $newExe='{new_exe_ps}'; \
         $oldExe='{old_exe_ps}'; \
         $readyStamp='{ready_stamp_ps}'; \
         $currentDir='{current_dir_ps}'; \
         $errorLog='{error_log_ps}'; \
         if (Test-Path -LiteralPath $errorLog) {{ Remove-Item -LiteralPath $errorLog -Force -ErrorAction SilentlyContinue }}; \
         try {{ \
             $proc = Get-Process -Id $pidToWait -ErrorAction SilentlyContinue; \
             if ($proc) {{ Wait-Process -Id $pidToWait; }} \
             Start-Sleep -Milliseconds 350; \
             if (Test-Path -LiteralPath $oldExe) {{ Remove-Item -LiteralPath $oldExe -Force -ErrorAction SilentlyContinue }}; \
             if (Test-Path -LiteralPath $currentExe) {{ Move-Item -LiteralPath $currentExe -Destination $oldExe -Force }}; \
             Copy-Item -LiteralPath $newExe -Destination $currentExe -Force; \
             $launched = $null; \
             for ($i = 0; $i -lt 10 -and -not $launched; $i++) {{ \
                 try {{ \
                     $launched = Start-Process -FilePath $currentExe -WorkingDirectory $currentDir -PassThru -ErrorAction Stop; \
                 }} catch {{ \
                     Start-Sleep -Milliseconds 500; \
                 }} \
             }} \
             if (-not $launched) {{ throw 'Failed to relaunch updated app.' }}; \
             Remove-Item -LiteralPath $newExe -Force -ErrorAction SilentlyContinue; \
             Remove-Item -LiteralPath $readyStamp -Force -ErrorAction SilentlyContinue; \
             Remove-Item -LiteralPath $oldExe -Force -ErrorAction SilentlyContinue; \
         }} catch {{ \
             $_ | Out-File -LiteralPath $errorLog -Encoding utf8; \
             throw; \
         }}"
    );

    let mut command = Command::new("powershell");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command.args(["-NoProfile", "-NonInteractive", "-Command", &helper]);
    command.spawn().context("Failed to spawn update helper script")?;
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer_version("0.1.1", "0.1.0"));
        assert!(is_newer_version("v0.2.0", "0.1.9"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(is_newer_version("0.1.0.1", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.0.9", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.2.0"));
    }

    #[test]
    fn test_update_manifest_deserialization() {
        let json = r#"{
            "version": "0.2.0",
            "notes": "Added cool features",
            "url": "https://example.com/soundfx_manager.exe"
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.notes, "Added cool features");
        assert_eq!(manifest.url, "https://example.com/soundfx_manager.exe");
    }
}
