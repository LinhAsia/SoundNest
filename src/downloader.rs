use anyhow::{Context, Result, bail};
use open::that_detached;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use zip::ZipArchive;

const YTDLP_DOWNLOAD_URL: &str =
    "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp.exe";
const YTDLP_RELEASE_PAGE_URL: &str =
    "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest";
const YTDLP_RELEASE_API_URL: &str =
    "https://api.github.com/repos/yt-dlp/yt-dlp-nightly-builds/releases/latest";
const FFMPEG_RELEASE_API_URL: &str =
    "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";
const DENO_DOWNLOAD_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
const FFMPEG_MARKER_FILE: &str = "ffmpeg_static.ok";

#[derive(Clone, Default)]
pub struct DownloadSnapshot {
    pub running: bool,
    pub searching: bool,
    pub stage: String,
    pub progress: Option<f32>,
    pub last_file: Option<PathBuf>,
    pub error: Option<String>,
    pub can_add_to_library: bool,
    pub added_to_library: bool,
    pub youtube_results: Vec<YoutubeSearchResult>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct YoutubeSearchResult {
    pub id: String,
    pub title: String,
    pub webpage_url: String,
    pub uploader: Option<String>,
    pub duration: Option<u64>,
    pub view_count: Option<u64>,
}

pub struct YoutubeAudioDownloader {
    bin_dir: PathBuf,
    download_dir: PathBuf,
    state: Arc<Mutex<DownloadSnapshot>>,
    cancel_requested: Arc<AtomicBool>,
    active_process_id: Arc<Mutex<Option<u32>>>,
}

impl YoutubeAudioDownloader {
    pub fn new(root_dir: &Path) -> Result<Self> {
        let bin_dir = root_dir.join("bin");
        let download_dir = root_dir.join("youtube-downloads");
        fs::create_dir_all(&bin_dir).context("unable to create downloader bin directory")?;
        fs::create_dir_all(&download_dir)
            .context("unable to create downloader output directory")?;

        Ok(Self {
            bin_dir,
            download_dir,
            state: Arc::new(Mutex::new(DownloadSnapshot::default())),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            active_process_id: Arc::new(Mutex::new(None)),
        })
    }

    pub fn snapshot(&self) -> DownloadSnapshot {
        self.state.lock().unwrap().clone()
    }

    pub fn clear_result(&self) {
        let mut state = self.state.lock().unwrap();
        if state.running {
            return;
        }
        state.stage.clear();
        state.progress = None;
        state.last_file = None;
        state.error = None;
        state.can_add_to_library = false;
        state.added_to_library = false;
    }

    pub fn mark_added_to_library(&self) {
        let mut state = self.state.lock().unwrap();
        state.added_to_library = true;
        state.can_add_to_library = false;
    }

    pub fn clear_youtube_results(&self) {
        let mut state = self.state.lock().unwrap();
        if state.searching {
            return;
        }
        state.youtube_results.clear();
        if !state.running {
            state.error = None;
            state.stage.clear();
        }
    }

    pub fn start_youtube_search(&self, query: String) -> Result<()> {
        let query = query.trim().to_owned();
        if query.is_empty() {
            bail!("Search is empty");
        }

        {
            let mut state = self.state.lock().unwrap();
            if state.running || state.searching {
                bail!("Downloader is busy");
            }
            state.searching = true;
            state.stage = "Search YouTube".to_owned();
            state.progress = None;
            state.error = None;
            state.youtube_results.clear();
        }

        let bin_dir = self.bin_dir.clone();
        let state = self.state.clone();
        thread::spawn(move || {
            let result =
                run_youtube_search_job(&state, &bin_dir, &query).map_err(|e| e.to_string());
            let mut snapshot = state.lock().unwrap();
            snapshot.searching = false;
            snapshot.progress = None;
            match result {
                Ok(results) => {
                    snapshot.stage = "YouTube ready".to_owned();
                    snapshot.error = None;
                    snapshot.youtube_results = results;
                }
                Err(error) => {
                    snapshot.stage = "Error".to_owned();
                    snapshot.error = Some(error);
                    snapshot.youtube_results.clear();
                }
            }
        });

        Ok(())
    }

    pub fn start_audio_download(&self, url: String) -> Result<()> {
        let url = url.trim().to_owned();
        if url.is_empty() {
            bail!("URL is empty");
        }

        {
            let mut state = self.state.lock().unwrap();
            if state.running || state.searching {
                bail!("Downloader is busy");
            }
            self.cancel_requested.store(false, Ordering::Relaxed);
            *self.active_process_id.lock().unwrap() = None;
            state.running = true;
            state.stage = "Prepare".to_owned();
            state.progress = None;
            state.last_file = None;
            state.error = None;
            state.can_add_to_library = false;
            state.added_to_library = false;
        }

        let bin_dir = self.bin_dir.clone();
        let download_dir = self.download_dir.clone();
        let state = self.state.clone();
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let active_process_id = Arc::clone(&self.active_process_id);

        thread::spawn(move || {
            let result = run_download_job(
                &state,
                &bin_dir,
                &download_dir,
                &url,
                &cancel_requested,
                &active_process_id,
            )
            .map_err(|e| e.to_string());
            let mut snapshot = state.lock().unwrap();
            snapshot.running = false;
            snapshot.progress = None;
            *active_process_id.lock().unwrap() = None;
            match result {
                Ok(path) => {
                    snapshot.stage = "Done".to_owned();
                    snapshot.last_file = Some(path);
                    snapshot.error = None;
                    snapshot.can_add_to_library = true;
                }
                Err(error) => {
                    snapshot.stage = "Error".to_owned();
                    snapshot.error = Some(error);
                    snapshot.last_file = None;
                    snapshot.can_add_to_library = false;
                }
            }
        });

        Ok(())
    }

    pub fn stop_audio_download(&self) {
        self.cancel_requested.store(true, Ordering::Relaxed);
        if let Some(pid) = *self.active_process_id.lock().unwrap() {
            let _ = stop_process(pid);
        }
        let mut state = self.state.lock().unwrap();
        if state.running {
            state.stage = "Stopping".to_owned();
        }
    }

    pub fn open_file(&self, path: &Path) -> Result<()> {
        that_detached(path).context("unable to open file")
    }

    pub fn open_folder(&self, path: &Path) -> Result<()> {
        let folder = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.download_dir.clone());
        that_detached(folder).context("unable to open folder")
    }

    pub fn ensure_ffmpeg_available(&self) -> Result<PathBuf> {
        ensure_ffmpeg_installed(&self.state, &self.bin_dir, &self.cancel_requested)?;
        Ok(self.bin_dir.join("ffmpeg.exe"))
    }
}

fn run_youtube_search_job(
    state: &Arc<Mutex<DownloadSnapshot>>,
    _bin_dir: &Path,
    query: &str,
) -> Result<Vec<YoutubeSearchResult>> {
    update_state(state, "Search YouTube", None, None);
    search_youtube(query)
}

fn run_download_job(
    state: &Arc<Mutex<DownloadSnapshot>>,
    bin_dir: &Path,
    download_dir: &Path,
    url: &str,
    cancel_requested: &Arc<AtomicBool>,
    active_process_id: &Arc<Mutex<Option<u32>>>,
) -> Result<PathBuf> {
    update_state(state, "yt-dlp", Some(0.0), None);
    ensure_ytdlp_installed(state, bin_dir, cancel_requested)?;
    ensure_not_cancelled(cancel_requested)?;

    update_state(state, "ffmpeg", Some(0.0), None);
    ensure_ffmpeg_installed(state, bin_dir, cancel_requested)?;
    ensure_not_cancelled(cancel_requested)?;

    update_state(state, "deno", Some(0.0), None);
    ensure_deno_installed(state, bin_dir, cancel_requested)?;
    ensure_not_cancelled(cancel_requested)?;

    update_state(state, "Download", None, None);
    let ytdlp_exe = bin_dir.join("yt-dlp.exe");
    let output_template = download_dir.join("%(title)s [%(id)s].%(ext)s");
    let deno_exe = bin_dir.join("deno.exe");
    let mut args = vec![
        "--encoding".to_owned(),
        "utf-8".to_owned(),
        "--ffmpeg-location".to_owned(),
        bin_dir.to_string_lossy().to_string(),
        "--newline".to_owned(),
        "--no-playlist".to_owned(),
        "--windows-filenames".to_owned(),
        "--force-overwrites".to_owned(),
        "--print".to_owned(),
        "after_move:filepath".to_owned(),
        "-x".to_owned(),
        "--audio-format".to_owned(),
        "mp3".to_owned(),
        "--audio-quality".to_owned(),
        "0".to_owned(),
        "-o".to_owned(),
        output_template.to_string_lossy().to_string(),
    ];
    if deno_exe.exists() {
        args.push("--js-runtimes".to_owned());
        args.push(format!("deno:{}", deno_exe.to_string_lossy()));
    }
    args.push(url.to_owned());

    match run_ytdlp_download_attempt(
        &ytdlp_exe,
        &args,
        download_dir,
        cancel_requested,
        active_process_id,
    ) {
        Ok(path) => Ok(path),
        Err(first_error) => {
            update_state(state, "Refresh yt-dlp", None, None);
            let refresh_note = refresh_ytdlp_after_failure(state, bin_dir, cancel_requested)?;
            update_state(state, "Retry download", None, None);
            match run_ytdlp_download_attempt(
                &ytdlp_exe,
                &args,
                download_dir,
                cancel_requested,
                active_process_id,
            ) {
                Ok(path) => Ok(path),
                Err(retry_error) => bail!("{first_error} | {refresh_note} | {retry_error}"),
            }
        }
    }
}

fn run_ytdlp_download_attempt(
    ytdlp_exe: &Path,
    args: &[String],
    download_dir: &Path,
    cancel_requested: &Arc<AtomicBool>,
    active_process_id: &Arc<Mutex<Option<u32>>>,
) -> Result<PathBuf> {
    let mut cmd = Command::new(&ytdlp_exe);
    cmd.args(args);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to launch {}", ytdlp_exe.display()))?;
    let pid = child.id();
    *active_process_id.lock().unwrap() = Some(pid);
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {}", ytdlp_exe.display()))?;
    *active_process_id.lock().unwrap() = None;
    ensure_not_cancelled(cancel_requested)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        bail!(
            "{}",
            if detail.is_empty() {
                format!("yt-dlp failed: {}", output.status)
            } else {
                detail
            }
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(path) = parse_downloaded_path(&stdout) {
        return Ok(path);
    }

    let newest = newest_audio_file(download_dir)?;
    Ok(newest)
}

fn search_youtube(query: &str) -> Result<Vec<YoutubeSearchResult>> {
    let encoded = urlencoding::encode(query);
    let url = format!("https://www.youtube.com/results?search_query={encoded}&hl=en");
    let body = ureq::get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36",
        )
        .header(
            "Accept-Language",
            "en-US,en;q=0.9",
        )
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .into_body()
        .read_to_string()
        .context("unable to read YouTube search response")?;

    let json = extract_yt_initial_data(&body).context("unable to parse YouTube search data")?;
    let root: Value =
        serde_json::from_str(json).context("invalid YouTube search metadata from webpage")?;

    let contents = root
        .pointer(
            "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents",
        )
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("YouTube results payload missing entries"))?;

    let mut results = Vec::new();
    for section in contents {
        let Some(items) = section
            .get("itemSectionRenderer")
            .and_then(|v| v.get("contents"))
            .and_then(Value::as_array)
        else {
            continue;
        };

        for item in items {
            let Some(video) = item.get("videoRenderer") else {
                continue;
            };
            let Some(id) = video.get("videoId").and_then(Value::as_str) else {
                continue;
            };
            let title = youtube_text(video.get("title")).unwrap_or_default();
            if title.trim().is_empty() {
                continue;
            }
            let uploader = youtube_text(video.get("ownerText"))
                .or_else(|| youtube_text(video.get("longBylineText")))
                .or_else(|| youtube_text(video.get("shortBylineText")));
            let duration =
                youtube_text(video.get("lengthText")).and_then(|text| parse_duration_text(&text));
            let view_count = youtube_text(video.get("viewCountText"))
                .and_then(|text| parse_view_count_text(&text));
            results.push(YoutubeSearchResult {
                id: id.to_owned(),
                title,
                webpage_url: format!("https://www.youtube.com/watch?v={id}"),
                uploader,
                duration,
                view_count,
            });
            if results.len() >= 12 {
                break;
            }
        }

        if results.len() >= 12 {
            break;
        }
    }

    if results.is_empty() {
        bail!("No YouTube videos found");
    }

    Ok(results)
}

fn extract_yt_initial_data(body: &str) -> Option<&str> {
    for marker in ["var ytInitialData = ", "window[\"ytInitialData\"] = "] {
        let start = body.find(marker)? + marker.len();
        let rest = &body[start..];
        let end = rest.find(";</script>").or_else(|| rest.find(";</body>"))?;
        return Some(rest[..end].trim());
    }
    None
}

fn youtube_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.get("simpleText").and_then(Value::as_str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let runs = value.get("runs").and_then(Value::as_array)?;
    let mut combined = String::new();
    for run in runs {
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            combined.push_str(text);
        }
    }
    let trimmed = combined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_duration_text(text: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut parts = text
        .split(':')
        .filter_map(|part| part.trim().parse::<u64>().ok())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    while parts.len() < 3 {
        parts.insert(0, 0);
    }
    for part in parts {
        total = total.checked_mul(60)?.checked_add(part)?;
    }
    Some(total)
}

fn parse_view_count_text(text: &str) -> Option<u64> {
    let digits = text
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn ensure_ytdlp_installed(
    state: &Arc<Mutex<DownloadSnapshot>>,
    bin_dir: &Path,
    cancel_requested: &Arc<AtomicBool>,
) -> Result<()> {
    let ytdlp_path = bin_dir.join("yt-dlp.exe");
    if ytdlp_path.exists() && validate_tool(&ytdlp_path, "--version").is_ok() {
        update_state(state, "yt-dlp ready", Some(1.0), None);
        return Ok(());
    }

    download_file(
        YTDLP_DOWNLOAD_URL,
        &ytdlp_path,
        state,
        "yt-dlp",
        "yt-dlp ready",
        cancel_requested,
    )
}

fn refresh_ytdlp_after_failure(
    state: &Arc<Mutex<DownloadSnapshot>>,
    bin_dir: &Path,
    cancel_requested: &Arc<AtomicBool>,
) -> Result<String> {
    let ytdlp_path = bin_dir.join("yt-dlp.exe");
    let local_version = read_local_ytdlp_version(&ytdlp_path).ok();
    let remote_version = fetch_latest_ytdlp_version().ok();

    if let (Some(local), Some(remote)) = (&local_version, &remote_version)
        && local == remote
    {
        return Ok(format!("yt-dlp already up to date ({local})"));
    }

    let stage = if let Some(remote) = &remote_version {
        format!("Updating yt-dlp {remote}")
    } else {
        "Updating yt-dlp".to_owned()
    };
    download_file(
        YTDLP_DOWNLOAD_URL,
        &ytdlp_path,
        state,
        &stage,
        "yt-dlp ready",
        cancel_requested,
    )?;
    let installed = read_local_ytdlp_version(&ytdlp_path)
        .ok()
        .or(remote_version)
        .unwrap_or_else(|| "latest".to_owned());
    Ok(format!("yt-dlp updated ({installed})"))
}

fn ensure_ffmpeg_installed(
    state: &Arc<Mutex<DownloadSnapshot>>,
    bin_dir: &Path,
    cancel_requested: &Arc<AtomicBool>,
) -> Result<()> {
    let ffmpeg_path = bin_dir.join("ffmpeg.exe");
    let ffprobe_path = bin_dir.join("ffprobe.exe");
    let marker_path = bin_dir.join(FFMPEG_MARKER_FILE);
    if ffmpeg_path.exists()
        && ffprobe_path.exists()
        && marker_path.exists()
        && validate_tool(&ffmpeg_path, "-version").is_ok()
        && validate_tool(&ffprobe_path, "-version").is_ok()
    {
        update_state(state, "ffmpeg ready", Some(1.0), None);
        return Ok(());
    }

    cleanup_ffmpeg_files(bin_dir);
    let _ = fs::remove_file(&ffmpeg_path);
    let _ = fs::remove_file(&ffprobe_path);
    let _ = fs::remove_file(&marker_path);

    let zip_path = bin_dir.join("ffmpeg.zip");
    let ffmpeg_download_url = resolve_ffmpeg_download_url()?;
    download_file(
        &ffmpeg_download_url,
        &zip_path,
        state,
        "ffmpeg",
        "ffmpeg extract",
        cancel_requested,
    )?;
    extract_ffmpeg(&zip_path, bin_dir)?;
    let _ = fs::remove_file(&zip_path);
    validate_tool(&bin_dir.join("ffmpeg.exe"), "-version")?;
    validate_tool(&bin_dir.join("ffprobe.exe"), "-version")?;
    let _ = fs::write(&marker_path, "static");
    update_state(state, "ffmpeg ready", Some(1.0), None);
    Ok(())
}

fn ensure_deno_installed(
    state: &Arc<Mutex<DownloadSnapshot>>,
    bin_dir: &Path,
    cancel_requested: &Arc<AtomicBool>,
) -> Result<()> {
    let deno_path = bin_dir.join("deno.exe");
    if deno_path.exists() && validate_tool(&deno_path, "--version").is_ok() {
        update_state(state, "deno ready", Some(1.0), None);
        return Ok(());
    }

    let _ = fs::remove_file(&deno_path);
    let zip_path = bin_dir.join("deno.zip");
    download_file(
        DENO_DOWNLOAD_URL,
        &zip_path,
        state,
        "deno",
        "deno extract",
        cancel_requested,
    )?;
    extract_file_from_zip(&zip_path, "deno.exe", &deno_path)?;
    let _ = fs::remove_file(&zip_path);
    validate_tool(&deno_path, "--version")?;
    update_state(state, "deno ready", Some(1.0), None);
    Ok(())
}

fn update_state(
    state: &Arc<Mutex<DownloadSnapshot>>,
    stage: &str,
    progress: Option<f32>,
    error: Option<String>,
) {
    let mut snapshot = state.lock().unwrap();
    snapshot.stage = stage.to_owned();
    snapshot.progress = progress;
    snapshot.error = error;
}

fn download_file(
    url: &str,
    path: &Path,
    state: &Arc<Mutex<DownloadSnapshot>>,
    stage: &str,
    done_stage: &str,
    cancel_requested: &Arc<AtomicBool>,
) -> Result<()> {
    let response = ureq::get(url)
        .header("User-Agent", "SoundFxManager")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let total_size = response
        .headers()
        .get("Content-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    let temp_path = path.with_extension("tmp");
    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(&temp_path)
        .with_context(|| format!("unable to create {}", temp_path.display()))?;
    let mut downloaded = 0u64;
    let mut buffer = [0u8; 8192];

    loop {
        ensure_not_cancelled(cancel_requested)?;
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])?;
        downloaded += bytes_read as u64;

        if total_size > 0 {
            update_state(
                state,
                stage,
                Some((downloaded as f32 / total_size as f32).clamp(0.0, 1.0)),
                None,
            );
        }
    }

    drop(file);
    fs::rename(&temp_path, path).with_context(|| format!("unable to write {}", path.display()))?;
    update_state(state, done_stage, Some(1.0), None);
    Ok(())
}

fn ensure_not_cancelled(cancel_requested: &Arc<AtomicBool>) -> Result<()> {
    if cancel_requested.load(Ordering::Relaxed) {
        bail!("Download canceled");
    }
    Ok(())
}

fn stop_process(pid: u32) -> Result<()> {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.spawn().context("unable to stop download process")?;
    Ok(())
}

fn fetch_latest_ytdlp_version() -> Result<String> {
    let _ = ureq::get(YTDLP_RELEASE_PAGE_URL)
        .header("User-Agent", "Mozilla/5.0")
        .call();

    let response = ureq::get(YTDLP_RELEASE_API_URL)
        .header("User-Agent", "SoundFxManager")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let body = response
        .into_body()
        .read_to_string()
        .context("unable to read yt-dlp release metadata")?;
    extract_json_string_field(&body, "tag_name")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("could not parse latest yt-dlp tag"))
}

fn read_local_ytdlp_version(ytdlp_path: &Path) -> Result<String> {
    let mut cmd = Command::new(ytdlp_path);
    cmd.arg("--version");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    let output = cmd
        .output()
        .with_context(|| format!("failed to launch {}", ytdlp_path.display()))?;
    if !output.status.success() {
        bail!("yt-dlp --version failed: {}", output.status);
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        bail!("yt-dlp --version returned empty output");
    }
    Ok(version)
}

fn extract_json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let position = json.find(&needle)?;
    let after_key = &json[position + needle.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let quote_start = after_colon.find('"')?;
    let value_start = quote_start + 1;
    let quote_end = after_colon[value_start..].find('"')?;
    Some(after_colon[value_start..value_start + quote_end].to_owned())
}

fn resolve_ffmpeg_download_url() -> Result<String> {
    #[derive(Deserialize)]
    struct GithubRelease {
        assets: Vec<GithubAsset>,
    }

    #[derive(Deserialize)]
    struct GithubAsset {
        browser_download_url: String,
        name: String,
    }

    let response = ureq::get(FFMPEG_RELEASE_API_URL)
        .header("User-Agent", "SoundFxManager")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let body = response
        .into_body()
        .read_to_string()
        .context("unable to read ffmpeg release metadata")?;
    let release: GithubRelease =
        serde_json::from_str(&body).context("invalid ffmpeg release metadata")?;

    release
        .assets
        .into_iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.contains("win64-gpl") && name.ends_with(".zip") && !name.contains("shared")
        })
        .map(|asset| asset.browser_download_url)
        .ok_or_else(|| anyhow::anyhow!("unable to locate a Windows ffmpeg build"))
}

fn extract_ffmpeg(zip_path: &Path, bin_dir: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)
        .with_context(|| format!("unable to open {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("invalid ffmpeg archive")?;

    let mut found_ffmpeg = false;
    let mut found_ffprobe = false;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("unable to read ffmpeg archive entry")?;
        let name = entry.name().to_owned();

        if name.ends_with("ffmpeg.exe") {
            let mut out = fs::File::create(bin_dir.join("ffmpeg.exe"))?;
            std::io::copy(&mut entry, &mut out)?;
            found_ffmpeg = true;
        } else if name.ends_with("ffprobe.exe") {
            let mut out = fs::File::create(bin_dir.join("ffprobe.exe"))?;
            std::io::copy(&mut entry, &mut out)?;
            found_ffprobe = true;
        }
    }

    if !found_ffmpeg {
        bail!("ffmpeg.exe not found in archive");
    }
    if !found_ffprobe {
        bail!("ffprobe.exe not found in archive");
    }
    Ok(())
}

fn extract_file_from_zip(zip_path: &Path, file_name: &str, destination: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)
        .with_context(|| format!("unable to open {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("invalid zip archive")?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("unable to read zip archive entry")?;
        let name = entry.name().to_owned();
        if name.ends_with(file_name) {
            let mut out = fs::File::create(destination)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }

    bail!("{file_name} not found in archive");
}

fn validate_tool(path: &Path, arg: &str) -> Result<()> {
    let mut cmd = Command::new(path);
    cmd.arg(arg);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    let output = cmd
        .output()
        .with_context(|| format!("failed to launch {}", path.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim());
    }
}

fn cleanup_ffmpeg_files(bin_dir: &Path) {
    let Ok(entries) = fs::read_dir(bin_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let should_remove = lower == "ffmpeg.exe"
            || lower == "ffprobe.exe"
            || lower == "ffplay.exe"
            || lower == FFMPEG_MARKER_FILE
            || lower.starts_with("avcodec-")
            || lower.starts_with("avdevice-")
            || lower.starts_with("avfilter-")
            || lower.starts_with("avformat-")
            || lower.starts_with("avutil-")
            || lower.starts_with("postproc-")
            || lower.starts_with("swresample-")
            || lower.starts_with("swscale-");
        if should_remove {
            let _ = fs::remove_file(path);
        }
    }
}

fn parse_downloaded_path(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.exists())
}

fn newest_audio_file(dir: &Path) -> Result<PathBuf> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("unable to read {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();

    entries.sort_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok());

    entries
        .last()
        .map(|entry| entry.path())
        .ok_or_else(|| anyhow::anyhow!("download finished but file was not found"))
}
