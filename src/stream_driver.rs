#[cfg(windows)]
mod windows_impl {
    use std::process::Command;

    const PACKAGE_ID: &str = "VB-Audio.Voicemeeter.Banana";

    fn winget_command() -> Command {
        let mut command = Command::new("winget");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }

    fn winget_success(args: &[&str]) -> Result<String, String> {
        let output = winget_command()
            .args(args)
            .output()
            .map_err(|error| format!("Failed to run winget: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if output.status.success() {
            Ok(stdout)
        } else {
            Err(if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            })
        }
    }

    pub fn is_stream_driver_installed() -> bool {
        winget_success(&[
            "list",
            "--id",
            PACKAGE_ID,
            "--exact",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .is_ok_and(|stdout| stdout.contains(PACKAGE_ID))
    }

    pub fn install_stream_driver() -> Result<(), String> {
        let _ = winget_success(&[
            "install",
            "--id",
            PACKAGE_ID,
            "--exact",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])?;
        Ok(())
    }

    pub fn uninstall_stream_driver() -> Result<(), String> {
        let _ = winget_success(&[
            "uninstall",
            "--id",
            PACKAGE_ID,
            "--exact",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])?;
        Ok(())
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
