use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const GEMINI_TTS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-preview-tts:generateContent";
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

pub fn generate_speech_to_file(
    api_key: &str,
    text: &str,
    voice_name: &str,
    output_dir: &Path,
    output_name: &str,
) -> Result<PathBuf> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("Gemini API key is empty");
    }
    let text = text.trim();
    if text.is_empty() {
        bail!("Text is empty");
    }
    fs::create_dir_all(output_dir)
        .with_context(|| format!("unable to create {}", output_dir.display()))?;

    let body = json!({
        "contents": [{
            "parts": [{
                "text": text
            }]
        }],
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": {
                "voiceConfig": {
                    "prebuiltVoiceConfig": {
                        "voiceName": if voice_name.trim().is_empty() { "Kore" } else { voice_name.trim() }
                    }
                }
            }
        },
        "model": "gemini-2.5-flash-preview-tts"
    });

    let response = ureq::post(GEMINI_TTS_URL)
        .header("x-goog-api-key", api_key)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let raw = response
        .into_body()
        .read_to_string()
        .context("unable to read Gemini TTS response")?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("invalid Gemini TTS response")?;

    let inline_data = value["candidates"][0]["content"]["parts"][0]["inlineData"]["data"]
        .as_str()
        .or_else(|| value["candidates"][0]["content"]["parts"][0]["inline_data"]["data"].as_str())
        .context("Gemini TTS did not return audio data")?;
    let pcm = base64::engine::general_purpose::STANDARD
        .decode(inline_data)
        .context("invalid Gemini audio payload")?;
    let output_path = unique_output_path(output_dir, output_name);
    write_pcm_wave(&output_path, &pcm)?;
    Ok(output_path)
}

fn unique_output_path(output_dir: &Path, output_name: &str) -> PathBuf {
    let stem = sanitize_stem(output_name);
    let first = output_dir.join(format!("{stem}.wav"));
    if !first.exists() {
        return first;
    }
    for index in 2..200 {
        let candidate = output_dir.join(format!("{stem}-{index}.wav"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

fn sanitize_stem(input: &str) -> String {
    let cleaned = input
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | ' ' => ch,
            _ => '_',
        })
        .collect::<String>();
    let trimmed = cleaned.trim().trim_matches('_').replace("  ", " ");
    if trimmed.is_empty() {
        "gemini-tts".to_owned()
    } else {
        trimmed
    }
}

fn write_pcm_wave(path: &Path, pcm: &[u8]) -> Result<()> {
    if pcm.len() < 2 {
        bail!("Gemini TTS returned empty audio");
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: OUTPUT_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("unable to write {}", path.display()))?;
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}
