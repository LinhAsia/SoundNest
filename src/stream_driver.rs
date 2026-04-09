#[cfg(windows)]
mod windows_impl {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const PACKAGE_ID: &str = "VB-Audio.Voicemeeter";
    const DRIVER_HINT: &str = "VB-Audio Software";

    fn hidden_command(program: &Path) -> Command {
        let mut command = Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }

    fn hidden_program_command(program: &str) -> Command {
        hidden_command(Path::new(program))
    }

    fn run_and_capture(mut command: Command) -> Result<(String, String, i32), String> {
        let program = command.get_program().to_string_lossy().to_string();
        let output = command
            .output()
            .map_err(|error| format!("Failed to run {program}: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok((stdout, stderr, output.status.code().unwrap_or(-1)))
    }

    fn voicemeeter_install_dir() -> PathBuf {
        PathBuf::from(r"C:\Program Files (x86)\VB\Voicemeeter")
    }

    fn local_setup_candidates() -> Vec<PathBuf> {
        [
            "voicemeeterprosetup.exe",
            "VBVoicemeeterVAIO_Setup_x64.exe",
            "VBVMAUX_Setup_x64.exe",
            "VBCABLE_Setup_x64.exe",
            "VBCABLE_Setup.exe",
        ]
        .into_iter()
        .map(|name| voicemeeter_install_dir().join(name))
        .filter(|path| path.exists())
        .collect()
    }

    fn extract_exe_path(value: &str) -> Option<PathBuf> {
        let trimmed = value.trim().trim_matches('"');
        let lower = trimmed.to_ascii_lowercase();
        let exe_index = lower.find(".exe")?;
        let candidate = trimmed[..exe_index + 4].trim().trim_matches('"');
        let path = PathBuf::from(candidate);
        path.exists().then_some(path)
    }

    fn query_registry_uninstall_output() -> String {
        let roots = [
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ];
        let mut combined = String::new();
        for root in roots {
            let mut command = hidden_program_command("reg");
            command.args(["query", root, "/s", "/f", "Voicemeeter"]);
            if let Ok((stdout, _, _)) = run_and_capture(command) {
                combined.push_str(&stdout);
                combined.push('\n');
            }
        }
        combined
    }

    fn find_registry_uninstall_exe() -> Option<PathBuf> {
        for line in query_registry_uninstall_output().lines() {
            if !line.contains("UninstallString") {
                continue;
            }
            let Some(value) = line
                .split("REG_SZ")
                .nth(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            if let Some(path) = extract_exe_path(value) {
                return Some(path);
            }
        }
        None
    }

    fn has_vb_audio_driver_traces() -> bool {
        let mut command = hidden_program_command("pnputil");
        command.arg("/enum-drivers");
        run_and_capture(command)
            .map(|(stdout, _, _)| stdout.contains(DRIVER_HINT))
            .unwrap_or(false)
    }

    fn run_setup(exe_path: &Path, args: &[&str]) -> Result<(), String> {
        let mut command = hidden_command(exe_path);
        command.args(args);
        let (stdout, stderr, code) = run_and_capture(command)?;
        if code == 0 || code == 1 {
            Ok(())
        } else {
            Err(if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            })
        }
    }

    fn run_winget_install() -> Result<(), String> {
        let mut command = hidden_program_command("winget");
        command.args([
            "install",
            "--id",
            PACKAGE_ID,
            "--exact",
            "--source",
            "winget",
            "--silent",
            "--disable-interactivity",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]);
        let (stdout, stderr, code) = run_and_capture(command)?;
        if code == 0 {
            Ok(())
        } else {
            Err(if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            })
        }
    }

    fn run_winget_uninstall() -> Result<(), String> {
        let mut command = hidden_program_command("winget");
        command.args([
            "uninstall",
            "--id",
            PACKAGE_ID,
            "--exact",
            "--source",
            "winget",
            "--silent",
            "--disable-interactivity",
            "--accept-source-agreements",
        ]);
        let (stdout, stderr, code) = run_and_capture(command)?;
        if code == 0 {
            Ok(())
        } else {
            Err(if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            })
        }
    }

    fn purge_vb_audio_driver_traces() -> Result<(), String> {
        let mut enum_command = hidden_program_command("pnputil");
        enum_command.arg("/enum-drivers");
        let (stdout, _, _) = run_and_capture(enum_command)?;
        let mut published = Vec::new();
        let mut current_published: Option<String> = None;
        let mut current_original = String::new();
        let mut current_provider = String::new();

        for raw_line in stdout.lines() {
            let line = raw_line.trim();
            if let Some(value) = line.strip_prefix("Published Name:") {
                current_published = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("Original Name:") {
                current_original = value.trim().to_owned();
            } else if let Some(value) = line.strip_prefix("Provider Name:") {
                current_provider = value.trim().to_owned();
            }

            if current_published.is_some()
                && !current_original.is_empty()
                && !current_provider.is_empty()
            {
                let matches_provider = current_provider.eq_ignore_ascii_case(DRIVER_HINT);
                let matches_original = current_original.to_ascii_lowercase().starts_with("vb");
                if (matches_provider || matches_original)
                    && let Some(name) = current_published.take()
                {
                    published.push(name);
                }
                current_original.clear();
                current_provider.clear();
            }
        }

        let mut failures = Vec::new();
        for inf in published {
            let mut delete_command = hidden_program_command("pnputil");
            delete_command.args(["/delete-driver", &inf, "/uninstall", "/force"]);
            if let Err(error) = run_and_capture(delete_command).and_then(|(_, stderr, code)| {
                if code == 0 {
                    Ok((String::new(), stderr, code))
                } else {
                    Err(if stderr.trim().is_empty() {
                        format!("pnputil failed for {inf} with exit code {code}")
                    } else {
                        stderr
                    })
                }
            }) {
                failures.push(format!("{inf}: {error}"));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    fn find_preferred_setup_exe() -> Option<PathBuf> {
        find_registry_uninstall_exe().or_else(|| local_setup_candidates().into_iter().next())
    }

    pub fn is_stream_driver_installed() -> bool {
        find_registry_uninstall_exe().is_some() || has_vb_audio_driver_traces()
    }

    pub fn install_stream_driver() -> Result<(), String> {
        if is_stream_driver_installed() {
            return Ok(());
        }

        if let Some(exe) = find_preferred_setup_exe() {
            run_setup(&exe, &["-install"])?;
        } else {
            run_winget_install()?;
        }

        if is_stream_driver_installed() {
            Ok(())
        } else {
            Err("Driver installer finished but no VB-Audio driver was detected afterwards.".to_owned())
        }
    }

    pub fn uninstall_stream_driver() -> Result<(), String> {
        let mut errors = Vec::new();
        let mut used_any_uninstaller = false;

        if let Some(exe) = find_registry_uninstall_exe() {
            used_any_uninstaller = true;
            if let Err(error) = run_setup(&exe, &["-u"]) {
                errors.push(error);
            }
        }

        for exe in local_setup_candidates() {
            used_any_uninstaller = true;
            if let Err(error) = run_setup(&exe, &["-u"]) {
                if !error.to_ascii_lowercase().contains("another setup program") {
                    errors.push(error);
                }
            }
        }

        if !used_any_uninstaller
            && let Err(error) = run_winget_uninstall()
        {
            errors.push(error);
        }

        if has_vb_audio_driver_traces()
            && let Err(error) = purge_vb_audio_driver_traces()
        {
            errors.push(error);
        }

        if is_stream_driver_installed() {
            if errors.is_empty() {
                Err("Some VB-Audio traces still remain. A reboot may be required before removing them.".to_owned())
            } else {
                Err(errors.join("\n"))
            }
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use windows_impl::*;

#[cfg(not(windows))]
pub fn is_stream_driver_installed() -> bool {
    false
}

#[cfg(not(windows))]
pub fn install_stream_driver() -> Result<(), String> {
    Err("Stream driver install is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn uninstall_stream_driver() -> Result<(), String> {
    Err("Stream driver uninstall is only available on Windows".to_owned())
}
