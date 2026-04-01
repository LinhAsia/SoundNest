use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use hound::{SampleFormat, WavSpec, WavWriter};
use rodio::{Decoder, Source};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const WAVEFORM_BUCKETS: usize = 320;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoundEffect {
    pub id: Uuid,
    pub name: String,
    pub asset_file: String,
    pub duration_secs: f32,
    pub volume: f32,
    #[serde(default = "default_speed")]
    pub speed: f32,
    pub trim_start_secs: f32,
    pub trim_end_secs: f32,
    #[serde(default)]
    pub waveform: Vec<f32>,
}

impl SoundEffect {
    pub fn asset_path(&self, storage_dir: &Path) -> PathBuf {
        storage_dir.join("sounds").join(&self.asset_file)
    }

    pub fn safe_duration(&self) -> f32 {
        self.duration_secs.max(0.05)
    }

    pub fn trimmed_length(&self) -> f32 {
        (self.trim_end_secs - self.trim_start_secs).max(0.05)
    }

    pub fn clamp_trim(&mut self) {
        let duration = self.safe_duration();
        self.speed = self.speed.clamp(0.25, 2.0);
        self.trim_start_secs = self.trim_start_secs.clamp(0.0, duration);
        self.trim_end_secs = self.trim_end_secs.clamp(0.0, duration);

        if self.trim_end_secs <= self.trim_start_secs {
            self.trim_end_secs = (self.trim_start_secs + 0.05).min(duration);
            self.trim_start_secs = (self.trim_end_secs - 0.05).max(0.0);
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct LibraryFile {
    sounds: Vec<SoundEffect>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PreferencesFile {
    import_dir: Option<PathBuf>,
    pitch_update_hz: Option<f32>,
    overlay_animation: Option<bool>,
    pitch_show_sharps: Option<bool>,
}

pub struct Storage {
    root_dir: PathBuf,
    sounds_dir: PathBuf,
    exports_dir: PathBuf,
    library_path: PathBuf,
    preferences_path: PathBuf,
}

impl Storage {
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "codex", "soundfx_manager")
            .context("unable to resolve app data directory")?;
        let root_dir = dirs.data_local_dir().to_path_buf();
        let sounds_dir = root_dir.join("sounds");
        let exports_dir = root_dir.join("exports");
        fs::create_dir_all(&sounds_dir).context("unable to create sounds directory")?;
        fs::create_dir_all(&exports_dir).context("unable to create exports directory")?;
        let library_path = root_dir.join("library.json");
        let preferences_path = root_dir.join("preferences.json");

        Ok(Self {
            root_dir,
            sounds_dir,
            exports_dir,
            library_path,
            preferences_path,
        })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn load_library(&self) -> Result<Vec<SoundEffect>> {
        if !self.library_path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.library_path).context("unable to read library file")?;
        let mut library: LibraryFile =
            serde_json::from_str(&raw).context("invalid library file format")?;

        library
            .sounds
            .retain(|sound| sound.asset_path(&self.root_dir).exists());

        for sound in &mut library.sounds {
            sound.clamp_trim();
        }

        Ok(library.sounds)
    }

    pub fn save_library(&self, sounds: &[SoundEffect]) -> Result<()> {
        let payload = LibraryFile {
            sounds: sounds.to_vec(),
        };
        let json = serde_json::to_string_pretty(&payload).context("unable to serialize library")?;
        fs::write(&self.library_path, json).context("unable to write library file")?;
        Ok(())
    }

    pub fn load_import_dir(&self) -> Result<Option<PathBuf>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.import_dir.filter(|path| path.exists()))
    }

    pub fn save_import_dir(&self, import_dir: Option<&Path>) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.import_dir = import_dir.map(Path::to_path_buf);
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_update_hz(&self) -> Result<Option<f32>> {
        let preferences = self.load_preferences()?;
        Ok(preferences
            .pitch_update_hz
            .map(|value| value.clamp(1.0, 12.0)))
    }

    pub fn save_pitch_update_hz(&self, pitch_update_hz: f32) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_update_hz = Some(pitch_update_hz.clamp(1.0, 12.0));
        self.save_preferences(&preferences)
    }

    pub fn load_overlay_animation(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.overlay_animation)
    }

    pub fn save_overlay_animation(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.overlay_animation = Some(enabled);
        self.save_preferences(&preferences)
    }

    pub fn load_pitch_show_sharps(&self) -> Result<Option<bool>> {
        let preferences = self.load_preferences()?;
        Ok(preferences.pitch_show_sharps)
    }

    pub fn save_pitch_show_sharps(&self, enabled: bool) -> Result<()> {
        let mut preferences = self.load_preferences()?;
        preferences.pitch_show_sharps = Some(enabled);
        self.save_preferences(&preferences)
    }

    fn load_preferences(&self) -> Result<PreferencesFile> {
        if !self.preferences_path.exists() {
            return Ok(PreferencesFile::default());
        }

        let raw = fs::read_to_string(&self.preferences_path)
            .context("unable to read preferences file")?;
        serde_json::from_str(&raw).context("invalid preferences file format")
    }

    fn save_preferences(&self, preferences: &PreferencesFile) -> Result<()> {
        let json =
            serde_json::to_string_pretty(preferences).context("unable to serialize preferences")?;
        fs::write(&self.preferences_path, json).context("unable to write preferences file")?;
        Ok(())
    }

    pub fn import_sound(&self, source_path: &Path) -> Result<SoundEffect> {
        if !source_path.exists() {
            bail!("file not found");
        }

        let extension = source_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let id = Uuid::new_v4();
        let asset_file = format!("{id}.{extension}");
        let target_path = self.sounds_dir.join(&asset_file);

        fs::copy(source_path, &target_path).context("unable to copy imported sound")?;

        let analysis = match analyze_audio_file(&target_path, WAVEFORM_BUCKETS) {
            Ok(analysis) => analysis,
            Err(error) => {
                let _ = fs::remove_file(&target_path);
                return Err(error);
            }
        };

        let name = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Sound")
            .to_owned();

        Ok(SoundEffect {
            id,
            name,
            asset_file,
            duration_secs: analysis.duration_secs,
            volume: 1.0,
            speed: 1.0,
            trim_start_secs: 0.0,
            trim_end_secs: analysis.duration_secs,
            waveform: analysis.waveform,
        })
    }

    pub fn hydrate_sound(&self, sound: &mut SoundEffect) -> Result<bool> {
        if !sound.waveform.is_empty() && sound.duration_secs > 0.0 {
            sound.clamp_trim();
            return Ok(false);
        }

        let analysis = analyze_audio_file(&sound.asset_path(&self.root_dir), WAVEFORM_BUCKETS)?;
        sound.duration_secs = analysis.duration_secs;
        if sound.waveform.is_empty() {
            sound.waveform = analysis.waveform;
        }
        if sound.trim_end_secs <= 0.0 {
            sound.trim_end_secs = sound.duration_secs;
        }
        sound.clamp_trim();
        Ok(true)
    }

    pub fn remove_sound(&self, sound: &SoundEffect) -> Result<()> {
        let path = sound.asset_path(&self.root_dir);
        if path.exists() {
            fs::remove_file(path).context("unable to delete audio asset")?;
        }
        Ok(())
    }

    pub fn export_processed_sound(&self, sound: &SoundEffect) -> Result<PathBuf> {
        let export_name = format!(
            "{}-{}.wav",
            sanitize_stem(&sound.name),
            &sound.id.to_string()[..8]
        );
        let export_path = self.exports_dir.join(export_name);
        write_processed_wav(&sound.asset_path(&self.root_dir), &export_path, sound)?;
        Ok(export_path)
    }
}

struct AudioAnalysis {
    duration_secs: f32,
    waveform: Vec<f32>,
}

struct DecodedAudio {
    channels: u16,
    sample_rate: u32,
    samples: Vec<f32>,
}

fn analyze_audio_file(path: &Path, buckets: usize) -> Result<AudioAnalysis> {
    let decoder = open_decoder(path)?;
    let total_duration = decoder.total_duration();
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    let bucket_count = buckets.max(64);
    let estimated_total_samples = total_duration.map(|duration| {
        (duration.as_secs_f64() * sample_rate as f64 * channels as f64).round() as usize
    });
    let samples_per_bucket = estimated_total_samples
        .map(|total| (total / bucket_count).max(1))
        .unwrap_or(2048);

    let mut peaks = vec![0.0f32; bucket_count];
    let mut sample_index = 0usize;
    for sample in decoder.convert_samples::<f32>() {
        let bucket = (sample_index / samples_per_bucket).min(bucket_count - 1);
        peaks[bucket] = peaks[bucket].max(sample.abs());
        sample_index += 1;
    }

    let duration_secs = if let Some(duration) = total_duration {
        duration.as_secs_f32()
    } else {
        let channels = channels.max(1) as f32;
        sample_index as f32 / channels / sample_rate.max(1) as f32
    };

    if sample_index == 0 {
        bail!("audio file is empty");
    }

    let peak_max = peaks.iter().copied().fold(0.0f32, f32::max);
    if peak_max > 0.0 {
        for peak in &mut peaks {
            *peak /= peak_max;
        }
    }

    Ok(AudioAnalysis {
        duration_secs,
        waveform: peaks,
    })
}

fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    let decoder = open_decoder(path)?;
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    let samples = decoder.convert_samples::<f32>().collect::<Vec<_>>();

    if samples.is_empty() {
        bail!("audio file is empty");
    }

    Ok(DecodedAudio {
        channels,
        sample_rate,
        samples,
    })
}

fn write_processed_wav(source_path: &Path, target_path: &Path, sound: &SoundEffect) -> Result<()> {
    let decoded = decode_audio_file(source_path)?;
    let channels = decoded.channels.max(1);
    let sample_rate = decoded.sample_rate.max(1);
    let total_frames = decoded.samples.len() / channels as usize;

    let start_frame = ((sound.trim_start_secs.clamp(0.0, sound.safe_duration())
        * sample_rate as f32)
        .floor() as usize)
        .min(total_frames);
    let end_frame = ((sound.trim_end_secs.clamp(0.0, sound.safe_duration()) * sample_rate as f32)
        .ceil() as usize)
        .min(total_frames)
        .max(start_frame);

    let spec = WavSpec {
        channels,
        sample_rate: ((sample_rate as f32 * sound.speed.clamp(0.25, 2.0)).round() as u32).max(1),
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer =
        WavWriter::create(target_path, spec).context("unable to create exported wav file")?;
    let volume = sound.volume.clamp(0.0, 2.0);
    let start_sample = start_frame * channels as usize;
    let end_sample = end_frame * channels as usize;

    for sample in &decoded.samples[start_sample..end_sample] {
        let scaled = (*sample * volume).clamp(-1.0, 1.0);
        let pcm = (scaled * i16::MAX as f32).round() as i16;
        writer
            .write_sample(pcm)
            .context("unable to write exported wav sample")?;
    }

    writer
        .finalize()
        .context("unable to finalize exported wav file")?;
    Ok(())
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    Decoder::new(BufReader::new(file)).context("unsupported audio file")
}

pub fn format_time(seconds: f32) -> String {
    let total = seconds.max(0.0);
    let mins = (total / 60.0).floor() as u32;
    let secs = (total % 60.0).floor() as u32;
    let millis = ((total.fract()) * 100.0).round() as u32;
    format!("{mins:02}:{secs:02}.{millis:02}")
}

fn sanitize_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '_',
        })
        .collect();

    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "sound".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn default_speed() -> f32 {
    1.0
}
