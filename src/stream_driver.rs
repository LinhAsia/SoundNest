#[cfg(windows)]
mod windows_impl {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use wasapi::{Direction, Role, get_default_device_for_role};
    use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PCWSTR};

    const CABLE_DOWNLOAD_URL: &str =
        "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip";
    const CABLE_ZIP_NAME: &str = "VBCABLE_Driver_Pack45.zip";
    const CABLE_TERM: &str = "VB-CABLE";
    const VOICEMEETER_TERM: &str = "Voicemeeter";
    const DRIVER_PROVIDER_HINT: &str = "VB-Audio Software";

    #[derive(Clone, Debug)]
    struct DriverEntry {
        published_name: String,
        original_name: String,
        provider_name: String,
    }

    #[repr(transparent)]
    #[derive(Clone, PartialEq, Eq)]
    struct IPolicyConfig(IUnknown);

    unsafe impl Interface for IPolicyConfig {
        type Vtable = IPolicyConfig_Vtbl;
        const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    }

    #[repr(C)]
    struct IPolicyConfig_Vtbl {
        base__: IUnknown_Vtbl,
        get_mix_format: usize,
        get_device_format: usize,
        reset_device_format: usize,
        set_device_format: usize,
        get_processing_period: usize,
        set_processing_period: usize,
        get_share_mode: usize,
        set_share_mode: usize,
        get_property_value: usize,
        set_property_value: usize,
        set_default_endpoint: unsafe extern "system" fn(
            this: *mut core::ffi::c_void,
            device_id: PCWSTR,
            role: ERole,
        ) -> HRESULT,
        set_endpoint_visibility: usize,
    }

    #[derive(Clone)]
    struct DefaultRenderEndpoint {
        role: ERole,
        device_id: String,
    }

    fn snapshot_default_render_endpoints() -> Vec<DefaultRenderEndpoint> {
        let mut entries = Vec::new();
        for (role, wasapi_role) in [
            (eConsole, Role::Console),
            (eMultimedia, Role::Multimedia),
            (eCommunications, Role::Communications),
        ] {
            if let Ok(device) = get_default_device_for_role(&Direction::Render, &wasapi_role)
                && let Ok(device_id) = device.get_id()
            {
                entries.push(DefaultRenderEndpoint { role, device_id });
            }
        }
        entries
    }

    fn restore_default_render_endpoints(entries: &[DefaultRenderEndpoint]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }

        let policy: IPolicyConfig = unsafe {
            CoCreateInstance(
                &GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9),
                None,
                CLSCTX_ALL,
            )
        }
        .map_err(|error| format!("Unable to create audio policy client: {error}"))?;

        for entry in entries {
            let device_id = HSTRING::from(entry.device_id.as_str());
            unsafe {
                (Interface::vtable(&policy).set_default_endpoint)(
                    Interface::as_raw(&policy),
                    PCWSTR(device_id.as_ptr()),
                    entry.role,
                )
            }
            .ok()
            .map_err(|error| {
                format!(
                    "Unable to restore previous default audio output for role {:?}: {error}",
                    entry.role
                )
            })?;
        }

        Ok(())
    }

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

    fn quote_for_powershell(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn run_powershell_script(script: &str) -> Result<(String, String, i32), String> {
        let mut command = hidden_program_command("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        run_and_capture(command)
    }

    fn run_elevated_command(program: &str, args: &[&str]) -> Result<(String, String, i32), String> {
        let argument_list = if args.is_empty() {
            "@()".to_owned()
        } else {
            let joined = args
                .iter()
                .map(|arg| format!("'{}'", quote_for_powershell(arg)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("@({joined})")
        };
        let script = format!(
            "$p = Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
            quote_for_powershell(program),
            argument_list,
        );
        run_powershell_script(&script)
    }

    fn stream_cache_dir() -> PathBuf {
        env::temp_dir().join("soundfx_manager_vbcable")
    }

    fn cable_zip_path() -> PathBuf {
        stream_cache_dir().join(CABLE_ZIP_NAME)
    }

    fn cable_extract_dir() -> PathBuf {
        stream_cache_dir().join("extracted")
    }

    fn voicemeeter_install_dir() -> PathBuf {
        PathBuf::from(r"C:\Program Files (x86)\VB\Voicemeeter")
    }

    fn extract_exe_path(value: &str) -> Option<PathBuf> {
        let trimmed = value.trim().trim_matches('"');
        let lower = trimmed.to_ascii_lowercase();
        let exe_index = lower.find(".exe")?;
        let candidate = trimmed[..exe_index + 4].trim().trim_matches('"');
        let path = PathBuf::from(candidate);
        path.exists().then_some(path)
    }

    fn query_registry_uninstall_output(search_term: &str) -> String {
        let roots = [
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ];
        let mut combined = String::new();
        for root in roots {
            let mut command = hidden_program_command("reg");
            command.args(["query", root, "/s", "/f", search_term]);
            if let Ok((stdout, _, _)) = run_and_capture(command) {
                combined.push_str(&stdout);
                combined.push('\n');
            }
        }
        combined
    }

    fn find_registry_uninstall_exe(search_term: &str) -> Option<PathBuf> {
        for line in query_registry_uninstall_output(search_term).lines() {
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

    fn recursive_find_named_file(root: &Path, names: &[&str]) -> Option<PathBuf> {
        if !root.exists() {
            return None;
        }
        let wanted = names
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                let lower = file_name.to_ascii_lowercase();
                if wanted.iter().any(|wanted_name| wanted_name == &lower) {
                    return Some(path);
                }
            }
        }
        None
    }

    fn download_vbcable_package(zip_path: &Path) -> Result<(), String> {
        let parent = zip_path
            .parent()
            .ok_or_else(|| "Invalid VB-CABLE cache path".to_owned())?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let script = format!(
            "$ProgressPreference='SilentlyContinue'; [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{}' -OutFile '{}'",
            CABLE_DOWNLOAD_URL,
            quote_for_powershell(&zip_path.display().to_string()),
        );
        let (stdout, stderr, code) = run_powershell_script(&script)?;
        if code == 0 {
            Ok(())
        } else if stderr.trim().is_empty() {
            Err(stdout)
        } else {
            Err(stderr)
        }
    }

    fn expand_vbcable_package(zip_path: &Path, extract_dir: &Path) -> Result<(), String> {
        if extract_dir.exists() {
            fs::remove_dir_all(extract_dir).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(extract_dir).map_err(|error| error.to_string())?;
        let script = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            quote_for_powershell(&zip_path.display().to_string()),
            quote_for_powershell(&extract_dir.display().to_string()),
        );
        let (stdout, stderr, code) = run_powershell_script(&script)?;
        if code == 0 {
            Ok(())
        } else if stderr.trim().is_empty() {
            Err(stdout)
        } else {
            Err(stderr)
        }
    }

    fn ensure_vbcable_extract_dir() -> Result<PathBuf, String> {
        let zip_path = cable_zip_path();
        if !zip_path.exists() {
            download_vbcable_package(&zip_path)?;
        }
        let extract_dir = cable_extract_dir();
        expand_vbcable_package(&zip_path, &extract_dir)?;
        Ok(extract_dir)
    }

    fn ensure_vbcable_setup_exe() -> Result<PathBuf, String> {
        if let Some(path) = find_registry_uninstall_exe(CABLE_TERM) {
            return Ok(path);
        }
        if let Some(path) = recursive_find_named_file(
            &voicemeeter_install_dir(),
            &["VBCABLE_Setup_x64.exe", "VBCABLE_Setup.exe"],
        ) {
            return Ok(path);
        }
        let extract_dir = ensure_vbcable_extract_dir()?;
        recursive_find_named_file(
            &extract_dir,
            &["VBCABLE_Setup_x64.exe", "VBCABLE_Setup.exe"],
        )
        .ok_or_else(|| {
            "Downloaded VB-CABLE package but could not find setup executable.".to_owned()
        })
    }

    fn ensure_vbcable_driver_inf() -> Result<PathBuf, String> {
        let extract_dir = ensure_vbcable_extract_dir()?;
        recursive_find_named_file(
            &extract_dir,
            &[
                "vbMmeCable64_win10.inf",
                "vbMmeCable64_win7.inf",
                "vbMmeCable64_vista.inf",
                "vbMmeCable64_2003.inf",
                "vbMmeCable_win7.inf",
                "vbMmeCable_vista.inf",
                "vbMmeCable_2003.inf",
                "vbMmeCable_xp.inf",
            ],
        )
        .ok_or_else(|| "Downloaded VB-CABLE package but could not find driver inf.".to_owned())
    }

    fn parse_driver_entries() -> Result<Vec<DriverEntry>, String> {
        let mut command = hidden_program_command("pnputil");
        command.arg("/enum-drivers");
        let (stdout, _, _) = run_and_capture(command)?;
        let mut entries = Vec::new();
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

            if let Some(published_name) = current_published.clone()
                && !current_original.is_empty()
                && !current_provider.is_empty()
            {
                entries.push(DriverEntry {
                    published_name,
                    original_name: current_original.clone(),
                    provider_name: current_provider.clone(),
                });
                current_published = None;
                current_original.clear();
                current_provider.clear();
            }
        }

        Ok(entries)
    }

    fn is_cable_driver_entry(entry: &DriverEntry) -> bool {
        let original = entry.original_name.to_ascii_lowercase();
        entry
            .provider_name
            .eq_ignore_ascii_case(DRIVER_PROVIDER_HINT)
            && (original.contains("vbmmecable") || original.contains("vbcable"))
    }

    fn is_voicemeeter_driver_entry(entry: &DriverEntry) -> bool {
        let original = entry.original_name.to_ascii_lowercase();
        entry
            .provider_name
            .eq_ignore_ascii_case(DRIVER_PROVIDER_HINT)
            && (original.contains("vbvm")
                || original.contains("vbvoicemeeter")
                || original.contains("voicemeeter"))
    }

    fn has_matching_driver_traces(predicate: impl Fn(&DriverEntry) -> bool) -> bool {
        parse_driver_entries()
            .map(|entries| entries.iter().any(predicate))
            .unwrap_or(false)
    }

    fn purge_matching_driver_traces(
        predicate: impl Fn(&DriverEntry) -> bool,
    ) -> Result<(), String> {
        let entries = parse_driver_entries()?;
        let mut failures = Vec::new();
        for entry in entries.into_iter().filter(predicate) {
            match run_elevated_command(
                "pnputil",
                &[
                    "/delete-driver",
                    &entry.published_name,
                    "/uninstall",
                    "/force",
                ],
            ) {
                Ok((stdout, stderr, code)) if code == 0 => {
                    let _ = (stdout, stderr);
                }
                Ok((stdout, stderr, code)) => {
                    failures.push(if stderr.trim().is_empty() {
                        format!(
                            "{}: pnputil failed with exit code {code}\n{}",
                            entry.published_name, stdout
                        )
                    } else {
                        format!("{}: {}", entry.published_name, stderr)
                    });
                }
                Err(error) => failures.push(format!("{}: {error}", entry.published_name)),
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    fn run_setup(exe_path: &Path, args: &[&str]) -> Result<(), String> {
        let (stdout, stderr, code) = run_elevated_command(&exe_path.display().to_string(), args)?;
        if code == 0 || code == 1 {
            Ok(())
        } else if stderr.trim().is_empty() {
            Err(stdout)
        } else {
            Err(stderr)
        }
    }

    fn run_elevated_pnputil(args: &[&str]) -> Result<(), String> {
        let (stdout, stderr, code) = run_elevated_command("pnputil", args)?;
        if code == 0 || code == 3010 {
            Ok(())
        } else if stderr.trim().is_empty() {
            Err(stdout)
        } else {
            Err(stderr)
        }
    }

    fn uninstall_legacy_voicemeeter() -> Result<(), String> {
        let mut candidates = Vec::new();
        if let Some(exe) = find_registry_uninstall_exe(VOICEMEETER_TERM) {
            candidates.push(exe);
        }
        if let Some(exe) = recursive_find_named_file(
            &voicemeeter_install_dir(),
            &["voicemeeterprosetup.exe", "voicemeetersetup.exe"],
        ) {
            if !candidates.iter().any(|candidate| candidate == &exe) {
                candidates.push(exe);
            }
        }
        let mut failures = Vec::new();
        for exe in candidates {
            if let Err(error) = run_setup(&exe, &["-h", "-u"]) {
                let lower = error.to_ascii_lowercase();
                if !lower.contains("another setup program")
                    && !lower.contains("not installed")
                    && !lower.contains("cancel")
                {
                    failures.push(error);
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    fn has_vbcable_traces() -> bool {
        find_registry_uninstall_exe(CABLE_TERM).is_some()
            || has_matching_driver_traces(is_cable_driver_entry)
    }

    fn has_voicemeeter_traces() -> bool {
        find_registry_uninstall_exe(VOICEMEETER_TERM).is_some()
            || has_matching_driver_traces(is_voicemeeter_driver_entry)
    }

    pub fn is_stream_driver_installed() -> bool {
        has_vbcable_traces()
    }

    pub fn install_stream_driver() -> Result<(), String> {
        let prior_defaults = snapshot_default_render_endpoints();
        let mut cleanup_errors = Vec::new();
        if has_voicemeeter_traces() {
            if let Err(error) = uninstall_legacy_voicemeeter() {
                cleanup_errors.push(error);
            }
            if let Err(error) = purge_matching_driver_traces(is_voicemeeter_driver_entry) {
                cleanup_errors.push(error);
            }
        }
        if has_vbcable_traces()
            && let Err(error) = purge_matching_driver_traces(is_cable_driver_entry)
        {
            cleanup_errors.push(error);
        }

        let inf = ensure_vbcable_driver_inf()?;
        let inf_string = inf.display().to_string();
        run_elevated_pnputil(&["/add-driver", &inf_string, "/install"])?;
        let _ = run_elevated_pnputil(&["/scan-devices"]);
        if let Err(error) = restore_default_render_endpoints(&prior_defaults) {
            cleanup_errors.push(error);
        }

        if is_stream_driver_installed() {
            if cleanup_errors.is_empty() {
                Ok(())
            } else {
                Err(cleanup_errors.join("\n"))
            }
        } else if cleanup_errors.is_empty() {
            Err(
                "VB-CABLE setup finished but Windows has not exposed the cable yet. A reboot may be required."
                    .to_owned(),
            )
        } else {
            Err(format!(
                "{}\n\nVB-CABLE setup finished but Windows has not exposed the cable yet. A reboot may be required.",
                cleanup_errors.join("\n")
            ))
        }
    }

    pub fn uninstall_stream_driver() -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(exe) = find_registry_uninstall_exe(CABLE_TERM) {
            if let Err(error) = run_setup(&exe, &["-h", "-u"]) {
                errors.push(error);
            }
        } else if let Ok(exe) = ensure_vbcable_setup_exe() {
            if let Err(error) = run_setup(&exe, &["-h", "-u"]) {
                errors.push(error);
            }
        }

        if has_vbcable_traces()
            && let Err(error) = purge_matching_driver_traces(is_cable_driver_entry)
        {
            errors.push(error);
        }
        let _ = run_elevated_pnputil(&["/scan-devices"]);

        if has_voicemeeter_traces() {
            if let Err(error) = uninstall_legacy_voicemeeter() {
                errors.push(error);
            }
            if let Err(error) = purge_matching_driver_traces(is_voicemeeter_driver_entry) {
                errors.push(error);
            }
        }

        if is_stream_driver_installed() || has_voicemeeter_traces() {
            if errors.is_empty() {
                Err(
                    "VB-Audio traces still remain after uninstall. A reboot may be required before Windows drops the devices."
                        .to_owned(),
                )
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
