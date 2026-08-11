use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
struct LocaleFile {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    languages: BTreeMap<String, LocaleDefinition>,
}

#[derive(Clone, Debug, Deserialize)]
struct LocaleDefinition {
    #[serde(rename = "name")]
    _name: String,
    #[serde(flatten)]
    strings: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Localization {
    default_code: String,
    current_code: String,
    languages: BTreeMap<String, LocaleDefinition>,
}

impl Localization {
    pub fn load() -> Self {
        if let Some(path) = resolve_locale_path()
            && let Ok(raw) = fs::read_to_string(&path)
            && let Ok(file) = serde_json::from_str::<LocaleFile>(&raw)
            && !file.languages.is_empty()
        {
            return Self::from_file(file);
        }

        Self {
            default_code: "en".to_owned(),
            current_code: "en".to_owned(),
            languages: BTreeMap::new(),
        }
    }

    fn from_file(file: LocaleFile) -> Self {
        let default_code = file
            .default
            .as_deref()
            .filter(|code| file.languages.contains_key(*code))
            .unwrap_or_else(|| {
                file.languages
                    .keys()
                    .next()
                    .map(String::as_str)
                    .unwrap_or("en")
            })
            .to_owned();
        Self {
            current_code: default_code.clone(),
            default_code,
            languages: file.languages,
        }
    }

    pub fn current_code(&self) -> &str {
        &self.current_code
    }

    pub fn set_current_code(&mut self, code: &str) {
        if self.languages.contains_key(code) {
            self.current_code = code.to_owned();
        } else {
            self.current_code = self.default_code.clone();
        }
    }

    pub fn text(&self, key: &str) -> String {
        self.languages
            .get(&self.current_code)
            .and_then(|language| language.strings.get(key))
            .or_else(|| {
                self.languages
                    .get(&self.default_code)
                    .and_then(|language| language.strings.get(key))
            })
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    }
}

fn resolve_locale_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        candidates.push(exe_dir.join("assets/locales.json"));
        candidates.push(exe_dir.join("../assets/locales.json"));
        candidates.push(exe_dir.join("../../assets/locales.json"));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("assets/locales.json"));
    }

    candidates
        .into_iter()
        .find(|path| path.exists())
        .map(|path| path.canonicalize().unwrap_or(path))
}

#[allow(dead_code)]
fn _validate_locale_file(path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path).context("unable to read locale file")?;
    let _: LocaleFile = serde_json::from_str(&raw).context("invalid locale file format")?;
    Ok(())
}
