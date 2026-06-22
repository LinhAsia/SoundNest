use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

const BASE_URL: &str = "https://www.myinstants.com";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MyinstantsResult {
    pub title: String,
    pub page_url: String,
    pub audio_url: String,
}

#[derive(Clone, Debug, Default)]
pub struct MyinstantsSnapshot {
    pub searching: bool,
    pub downloading: bool,
    pub results: Vec<MyinstantsResult>,
    pub error: Option<String>,
    pub completed_download: Option<PathBuf>,
    pub completed_result: Option<MyinstantsResult>,
    pub completed_add_to_library: bool,
}

#[derive(Clone)]
pub struct MyinstantsClient {
    download_dir: PathBuf,
    preview_dir: PathBuf,
    state: Arc<Mutex<MyinstantsSnapshot>>,
}

impl MyinstantsClient {
    pub fn new(root_dir: &Path) -> Result<Self> {
        let download_dir = root_dir.join("myinstants-downloads");
        let preview_dir = root_dir.join("myinstants-preview");
        fs::create_dir_all(&download_dir).context("unable to create myinstants download folder")?;
        fs::create_dir_all(&preview_dir).context("unable to create myinstants preview folder")?;
        Ok(Self {
            download_dir,
            preview_dir,
            state: Arc::new(Mutex::new(MyinstantsSnapshot::default())),
        })
    }

    pub fn snapshot(&self) -> MyinstantsSnapshot {
        self.state.lock().unwrap().clone()
    }

    pub fn start_search(&self, query: String) -> Result<()> {
        let query = query.trim().to_owned();
        if query.is_empty() {
            bail!("Search is empty");
        }

        {
            let mut state = self.state.lock().unwrap();
            if state.searching || state.downloading {
                bail!("Search is busy");
            }
            state.searching = true;
            state.error = None;
            state.completed_download = None;
            state.completed_result = None;
            state.completed_add_to_library = false;
            state.results.clear();
        }

        let state = self.state.clone();
        thread::spawn(move || {
            let result = search_myinstants(&query).map_err(|error| error.to_string());
            let mut snapshot = state.lock().unwrap();
            snapshot.searching = false;
            match result {
                Ok(results) => {
                    snapshot.results = results;
                    snapshot.error = None;
                }
                Err(error) => {
                    snapshot.results.clear();
                    snapshot.error = Some(error);
                }
            }
        });

        Ok(())
    }

    pub fn start_download(&self, result: MyinstantsResult, add_to_library: bool) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if state.downloading {
                bail!("Download already running");
            }
            state.downloading = true;
            state.error = None;
            state.completed_download = None;
            state.completed_result = None;
            state.completed_add_to_library = false;
        }

        let state = self.state.clone();
        let download_dir = self.download_dir.clone();
        thread::spawn(move || {
            let outcome =
                download_result(&download_dir, &result).map_err(|error| error.to_string());
            let mut snapshot = state.lock().unwrap();
            snapshot.downloading = false;
            match outcome {
                Ok(path) => {
                    snapshot.completed_download = Some(path);
                    snapshot.completed_result = Some(result);
                    snapshot.completed_add_to_library = add_to_library;
                    snapshot.error = None;
                }
                Err(error) => {
                    snapshot.error = Some(error);
                    snapshot.completed_download = None;
                    snapshot.completed_result = None;
                    snapshot.completed_add_to_library = false;
                }
            }
        });

        Ok(())
    }

    pub fn ensure_preview_file(&self, result: &MyinstantsResult) -> Result<PathBuf> {
        download_preview_result(&self.preview_dir, result)
    }

    pub fn take_completed_download(&self) -> Option<(MyinstantsResult, PathBuf, bool)> {
        let mut state = self.state.lock().unwrap();
        let path = state.completed_download.take()?;
        let result = state.completed_result.take()?;
        let add = state.completed_add_to_library;
        state.completed_add_to_library = false;
        Some((result, path, add))
    }
}

fn search_myinstants(query: &str) -> Result<Vec<MyinstantsResult>> {
    let encoded = urlencoding::encode(query);
    let url = format!("{BASE_URL}/en/search/?name={encoded}");
    let body = ureq::get(&url)
        .header("User-Agent", "SoundFxManager")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .into_body()
        .read_to_string()
        .context("unable to read myinstants search response")?;

    parse_results(&body)
}

fn parse_results(html: &str) -> Result<Vec<MyinstantsResult>> {
    let item_re = Regex::new(
        r#"(?s)<div class="instant">.*?onclick="play\('(?P<audio>/media/sounds/[^']+)'[^"]*".*?<a href="(?P<page>/en/instant/[^"]+/)" class="instant-link[^"]*">(?P<title>.*?)</a>"#,
    )
    .context("invalid myinstants search parser")?;

    let mut results = Vec::new();
    for capture in item_re.captures_iter(html).take(60) {
        let title = decode_html(capture.name("title").map(|m| m.as_str()).unwrap_or("sound"));
        let page = capture.name("page").map(|m| m.as_str()).unwrap_or_default();
        let audio = capture
            .name("audio")
            .map(|m| m.as_str())
            .unwrap_or_default();
        if page.is_empty() || audio.is_empty() {
            continue;
        }
        results.push(MyinstantsResult {
            title,
            page_url: format!("{BASE_URL}{page}"),
            audio_url: format!("{BASE_URL}{audio}"),
        });
    }

    if results.is_empty() {
        bail!("No results found");
    }

    Ok(results)
}

fn download_result(download_dir: &Path, result: &MyinstantsResult) -> Result<PathBuf> {
    let extension = Path::new(&result.audio_url)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("mp3");
    let file_name = format!("{}.{extension}", sanitize_file_name(&result.title));
    let target_path = unique_path(download_dir.join(file_name));
    let temp_path = target_path.with_extension(format!("{extension}.tmp"));

    let response = ureq::get(&result.audio_url)
        .header("User-Agent", "SoundFxManager")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(&temp_path)
        .with_context(|| format!("unable to create {}", temp_path.display()))?;
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])?;
    }
    drop(file);
    fs::rename(&temp_path, &target_path)
        .with_context(|| format!("unable to write {}", target_path.display()))?;
    Ok(target_path)
}

fn download_preview_result(preview_dir: &Path, result: &MyinstantsResult) -> Result<PathBuf> {
    let extension = Path::new(&result.audio_url)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("mp3");
    let file_name = format!("preview-{}.{extension}", sanitize_file_name(&result.title));
    let target_path = preview_dir.join(file_name);
    if target_path.exists() {
        return Ok(target_path);
    }

    let temp_path = target_path.with_extension(format!("{extension}.tmp"));
    let response = ureq::get(&result.audio_url)
        .header("User-Agent", "SoundFxManager")
        .call()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(&temp_path)
        .with_context(|| format!("unable to create {}", temp_path.display()))?;
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])?;
    }
    drop(file);
    fs::rename(&temp_path, &target_path)
        .with_context(|| format!("unable to write {}", target_path.display()))?;
    Ok(target_path)
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }

    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sound")
        .to_owned();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_owned();

    for index in 2..500 {
        let candidate = if ext.is_empty() {
            path.with_file_name(format!("{stem}-{index}"))
        } else {
            path.with_file_name(format!("{stem}-{index}.{ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn sanitize_file_name(input: &str) -> String {
    let cleaned = input
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | ' ' => ch,
            _ => '_',
        })
        .collect::<String>();
    let trimmed = cleaned.trim().trim_matches('_').replace("  ", " ");
    if trimmed.is_empty() {
        "sound".to_owned()
    } else {
        trimmed
    }
}

fn decode_html(input: &str) -> String {
    decode_numeric_entities(
        &input
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">"),
    )
}

fn decode_numeric_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let chars = input.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '&' && index + 3 < chars.len() && chars[index + 1] == '#' {
            let mut cursor = index + 2;
            let is_hex = if cursor < chars.len() && (chars[cursor] == 'x' || chars[cursor] == 'X') {
                cursor += 1;
                true
            } else {
                false
            };
            let digits_start = cursor;
            while cursor < chars.len() && chars[cursor] != ';' {
                cursor += 1;
            }
            if cursor < chars.len() && cursor > digits_start {
                let digits = chars[digits_start..cursor].iter().collect::<String>();
                let parsed = if is_hex {
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    digits.parse::<u32>().ok()
                };
                if let Some(value) = parsed.and_then(char::from_u32) {
                    output.push(value);
                    index = cursor + 1;
                    continue;
                }
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}
