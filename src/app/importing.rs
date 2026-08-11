use super::*;

impl SoundFxApp {
    pub(crate) fn add_sound(&mut self) {
        self.refresh_import_audio_entries();
        self.show_import_panel = true;
    }

    pub(crate) fn import_paths(&mut self, paths: Vec<PathBuf>) {
        self.library_current_folder = None;
        self.folder_import_select_mode = None;
        if let Err(error) = self.import_paths_to_folder(paths, None) {
            self.set_error_status(error);
        }
    }

    pub(crate) fn begin_import_paths_to_folder(
        &mut self,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
    ) {
        if paths.is_empty() {
            return;
        }
        if self.library_import_job.is_some() {
            self.set_error_status("Another import is already running");
            return;
        }

        let total = paths
            .iter()
            .map(|path| Self::count_importable_audio_files(path))
            .sum::<usize>();
        let job_id = Uuid::new_v4();
        self.library_import_job = Some(ActiveLibraryImport {
            job_id,
            target_folder_id,
            completed: 0,
            total,
            current_label: "Preparing import...".to_owned(),
        });
        self.status = Some(if total > 0 {
            format!("Importing {total} sound(s)...")
        } else {
            "Scanning dropped files...".to_owned()
        });

        let tx = self.library_import_tx.clone();
        let root_dir = self.storage.root_dir().to_path_buf();
        thread::spawn(move || {
            let result =
                Self::run_library_import_job(root_dir, paths, target_folder_id, &tx, job_id)
                    .map_err(|error| error.to_string());
            let _ = tx.send(LibraryImportMessage::Finished { job_id, result });
        });
    }

    pub(crate) fn import_paths_to_folder(
        &mut self,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
    ) -> Result<usize> {
        let mut imported = Vec::new();
        let mut ignored = 0usize;
        let mut folders_changed = false;

        for path in paths {
            self.import_path_entry(
                &path,
                target_folder_id,
                &mut imported,
                &mut ignored,
                &mut folders_changed,
            )?;
        }

        if imported.is_empty() {
            if ignored > 0 && self.status.is_none() {
                anyhow::bail!("No supported audio files were found");
            }
            return Ok(0);
        }

        imported.reverse();
        let imported_count = imported.len();
        for sound in imported {
            self.selected = Some(sound.id);
            self.sounds.insert(0, sound);
        }

        self.library_audio_tag_filters.clear();
        if target_folder_id.is_some() {
            self.library_current_folder = target_folder_id;
        }
        let _ = folders_changed;
        self.save_now();
        Ok(imported_count)
    }

    pub(crate) fn import_path_entry(
        &mut self,
        path: &Path,
        target_folder_id: Option<Uuid>,
        imported: &mut Vec<SoundEffect>,
        ignored: &mut usize,
        folders_changed: &mut bool,
    ) -> Result<bool> {
        if Self::should_skip_import_path(path) {
            *ignored += 1;
            return Ok(false);
        }

        if path.is_dir() {
            if !Self::path_contains_supported_audio(path) {
                *ignored += 1;
                return Ok(false);
            }

            let folder_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Folder")
                .to_owned();
            let folder_id = self.create_folder_record(folder_name, target_folder_id);
            *folders_changed = true;

            for child_path in Self::sorted_directory_entries(path)? {
                let _ = self.import_path_entry(
                    &child_path,
                    Some(folder_id),
                    imported,
                    ignored,
                    folders_changed,
                )?;
            }
            return Ok(true);
        }

        if !is_supported_audio(path) {
            *ignored += 1;
            return Ok(false);
        }

        let mut sound = self.storage.import_sound(path)?;
        sound.folder_id = target_folder_id;
        imported.push(sound);
        Ok(true)
    }

    pub(crate) fn count_importable_audio_files(path: &Path) -> usize {
        if Self::should_skip_import_path(path) {
            return 0;
        }
        if path.is_file() {
            return usize::from(is_supported_audio(path));
        }
        if !path.is_dir() {
            return 0;
        }

        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| Self::count_importable_audio_files(&entry.path()))
            .sum()
    }

    pub(crate) fn run_library_import_job(
        root_dir: PathBuf,
        paths: Vec<PathBuf>,
        target_folder_id: Option<Uuid>,
        tx: &Sender<LibraryImportMessage>,
        job_id: Uuid,
    ) -> Result<LibraryImportResult> {
        let mut imported_sounds = Vec::new();
        let mut imported_folders = Vec::new();
        let mut ignored_count = 0usize;
        let total = paths
            .iter()
            .map(|path| Self::count_importable_audio_files(path))
            .sum::<usize>();
        let mut completed = 0usize;

        let _ = tx.send(LibraryImportMessage::Progress {
            job_id,
            completed,
            total,
            current_label: "Preparing import...".to_owned(),
        });

        for path in paths {
            Self::import_path_entry_background(
                &root_dir,
                &path,
                target_folder_id,
                &mut imported_sounds,
                &mut imported_folders,
                &mut ignored_count,
                &mut completed,
                total,
                tx,
                job_id,
            )?;
        }

        Ok(LibraryImportResult {
            target_folder_id,
            imported_sounds,
            imported_folders,
            ignored_count,
        })
    }

    pub(crate) fn import_path_entry_background(
        root_dir: &Path,
        path: &Path,
        target_folder_id: Option<Uuid>,
        imported_sounds: &mut Vec<SoundEffect>,
        imported_folders: &mut Vec<Folder>,
        ignored_count: &mut usize,
        completed: &mut usize,
        total: usize,
        tx: &Sender<LibraryImportMessage>,
        job_id: Uuid,
    ) -> Result<bool> {
        if Self::should_skip_import_path(path) {
            *ignored_count += 1;
            return Ok(false);
        }

        if path.is_dir() {
            if !Self::path_contains_supported_audio(path) {
                *ignored_count += 1;
                return Ok(false);
            }

            let folder_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Folder")
                .to_owned();
            let folder_id = Uuid::new_v4();
            imported_folders.push(Folder {
                id: folder_id,
                name: folder_name,
                parent_id: target_folder_id,
            });

            for child_path in Self::sorted_directory_entries(path)? {
                let _ = Self::import_path_entry_background(
                    root_dir,
                    &child_path,
                    Some(folder_id),
                    imported_sounds,
                    imported_folders,
                    ignored_count,
                    completed,
                    total,
                    tx,
                    job_id,
                )?;
            }
            return Ok(true);
        }

        if !is_supported_audio(path) {
            *ignored_count += 1;
            return Ok(false);
        }

        let mut sound = Storage::import_sound_at(root_dir, path)?;
        sound.folder_id = target_folder_id;
        imported_sounds.push(sound);
        *completed = completed.saturating_add(1);
        let current_label = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "sound".to_owned());
        let _ = tx.send(LibraryImportMessage::Progress {
            job_id,
            completed: *completed,
            total,
            current_label,
        });
        Ok(true)
    }

    pub(crate) fn sorted_directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("unable to read {}", path.display()))?
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| left.file_name().cmp(&right.file_name()))
        });
        Ok(entries)
    }

    pub(crate) fn path_contains_supported_audio(path: &Path) -> bool {
        if Self::should_skip_import_path(path) {
            return false;
        }
        if path.is_file() {
            return is_supported_audio(path);
        }
        if !path.is_dir() {
            return false;
        }

        let Ok(entries) = fs::read_dir(path) else {
            return false;
        };
        for entry in entries.flatten() {
            if Self::path_contains_supported_audio(&entry.path()) {
                return true;
            }
        }
        false
    }
}
