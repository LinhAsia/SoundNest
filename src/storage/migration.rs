use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use super::paths::WINDOWS_STORAGE_ROOT;

fn custom_root_override_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "codex", "soundfx_manager")
        .map(|dirs| dirs.data_local_dir().join("custom_root.txt"))
}

pub(super) fn load_custom_root() -> Option<PathBuf> {
    let path = custom_root_override_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = PathBuf::from(trimmed);
    if p.exists() { Some(p) } else { None }
}

pub(super) fn save_custom_root(root: Option<&Path>) -> Result<()> {
    let Some(override_path) = custom_root_override_path() else {
        return Ok(());
    };
    if let Some(parent) = override_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    match root {
        Some(path) => fs::write(&override_path, path.to_string_lossy().as_bytes())
            .context("unable to save custom root"),
        None => {
            if override_path.exists() {
                fs::remove_file(&override_path).context("unable to clear custom root")?;
            }
            Ok(())
        }
    }
}

pub(super) fn preferred_storage_root() -> Result<PathBuf> {
    // Check for user-chosen root override first (written by "Change Folder" button)
    if let Some(custom) = load_custom_root() {
        return Ok(custom);
    }

    #[cfg(windows)]
    {
        Ok(PathBuf::from(WINDOWS_STORAGE_ROOT))
    }

    #[cfg(not(windows))]
    {
        legacy_storage_root()?.context("unable to resolve app data directory")
    }
}

fn legacy_storage_root() -> Result<Option<PathBuf>> {
    Ok(ProjectDirs::from("dev", "codex", "soundfx_manager")
        .map(|dirs| dirs.data_local_dir().to_path_buf()))
}

pub(super) fn migrate_storage_root_if_needed(target_root: &Path) -> Result<()> {
    let Some(legacy_root) = legacy_storage_root()? else {
        return Ok(());
    };

    if legacy_root == target_root || !legacy_root.exists() {
        return Ok(());
    }

    fs::create_dir_all(target_root)
        .with_context(|| format!("unable to create {}", target_root.display()))?;
    merge_directory_contents(&legacy_root, target_root)?;
    remove_dir_if_empty(&legacy_root)?;
    Ok(())
}

fn merge_directory_contents(source_dir: &Path, target_dir: &Path) -> Result<()> {
    if !source_dir.exists() {
        return Ok(());
    }

    fs::create_dir_all(target_dir)
        .with_context(|| format!("unable to create {}", target_dir.display()))?;

    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("unable to read {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| format!("unable to read {}", source_dir.display()))?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        merge_path(&source_path, &target_path)?;
    }

    Ok(())
}

fn merge_path(source_path: &Path, target_path: &Path) -> Result<()> {
    if !target_path.exists() {
        move_path(source_path, target_path)?;
        return Ok(());
    }

    let source_meta = fs::metadata(source_path)
        .with_context(|| format!("unable to read {}", source_path.display()))?;
    let target_meta = fs::metadata(target_path)
        .with_context(|| format!("unable to read {}", target_path.display()))?;

    if source_meta.is_dir() && target_meta.is_dir() {
        merge_directory_contents(source_path, target_path)?;
        remove_dir_if_empty(source_path)?;
        return Ok(());
    }

    if source_meta.is_file() && target_meta.is_file() {
        return Ok(());
    }

    bail!(
        "unable to merge {} into {}",
        source_path.display(),
        target_path.display()
    )
}

fn move_path(source_path: &Path, target_path: &Path) -> Result<()> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }

    match fs::rename(source_path, target_path) {
        Ok(()) => Ok(()),
        Err(_) => copy_then_remove(source_path, target_path),
    }
}

fn copy_then_remove(source_path: &Path, target_path: &Path) -> Result<()> {
    let metadata = fs::metadata(source_path)
        .with_context(|| format!("unable to read {}", source_path.display()))?;

    if metadata.is_dir() {
        copy_directory_recursive(source_path, target_path)?;
        fs::remove_dir_all(source_path)
            .with_context(|| format!("unable to remove {}", source_path.display()))?;
    } else {
        fs::copy(source_path, target_path).with_context(|| {
            format!(
                "unable to copy {} to {}",
                source_path.display(),
                target_path.display()
            )
        })?;
        fs::remove_file(source_path)
            .with_context(|| format!("unable to remove {}", source_path.display()))?;
    }

    Ok(())
}

fn copy_directory_recursive(source_dir: &Path, target_dir: &Path) -> Result<()> {
    fs::create_dir_all(target_dir)
        .with_context(|| format!("unable to create {}", target_dir.display()))?;

    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("unable to read {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| format!("unable to read {}", source_dir.display()))?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        let metadata = fs::metadata(&source_path)
            .with_context(|| format!("unable to read {}", source_path.display()))?;
        if metadata.is_dir() {
            copy_directory_recursive(&source_path, &target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("unable to create {}", parent.display()))?;
            }
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "unable to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn remove_dir_if_empty(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }

    let mut entries =
        fs::read_dir(path).with_context(|| format!("unable to read {}", path.display()))?;
    if entries.next().is_none() {
        fs::remove_dir(path).with_context(|| format!("unable to remove {}", path.display()))?;
    }
    Ok(())
}
