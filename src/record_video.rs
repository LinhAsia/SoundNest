use crate::pitch::OfflinePitchFrame;
use anyhow::{Context, Result, bail};
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const VIDEO_WIDTH: usize = 960;
const VIDEO_HEIGHT: usize = 540;
const VIDEO_FPS: u32 = 20;

pub fn video_fps() -> u32 {
    VIDEO_FPS
}

pub fn export_record_pitch_video<F>(
    root_dir: &Path,
    ffmpeg_path: &Path,
    audio_path: &Path,
    frames: &[OfflinePitchFrame],
    duration_secs: f32,
    mut progress: F,
) -> Result<PathBuf>
where
    F: FnMut(f32, &str),
{
    if frames.is_empty() {
        bail!("recording is empty");
    }

    let output_dir = root_dir.join("record-videos");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("unable to create {}", output_dir.display()))?;
    let stem = audio_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("recording");
    let output_path = unique_path(output_dir.join(format!("{stem}-spn.mp4")));

    let temp_dir = root_dir.join("record-video-temp");
    if temp_dir.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
    }
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("unable to create {}", temp_dir.display()))?;

    progress(0.45, "Rendering frames");
    for (index, frame) in frames.iter().enumerate() {
        let path = temp_dir.join(format!("frame_{index:05}.ppm"));
        write_ppm_frame(&path, frame)?;
        if index % 4 == 0 || index + 1 == frames.len() {
            let frame_progress = (index + 1) as f32 / frames.len().max(1) as f32;
            progress(0.45 + frame_progress * 0.35, "Rendering frames");
        }
    }

    let ass_path = temp_dir.join("notes.ass");
    fs::write(&ass_path, build_ass_script(frames, duration_secs))
        .with_context(|| format!("unable to write {}", ass_path.display()))?;

    let mut cmd = Command::new(ffmpeg_path);
    cmd.current_dir(&temp_dir)
        .arg("-y")
        .arg("-framerate")
        .arg(VIDEO_FPS.to_string())
        .arg("-i")
        .arg("frame_%05d.ppm")
        .arg("-i")
        .arg(audio_path)
        .arg("-vf")
        .arg("ass=notes.ass")
        .arg("-c:v")
        .arg("libx264")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-shortest")
        .arg(&output_path);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);

    progress(0.84, "Encoding video");
    let output = cmd
        .output()
        .with_context(|| format!("failed to launch {}", ffmpeg_path.display()))?;
    let _ = fs::remove_dir_all(&temp_dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim());
    }

    progress(0.96, "Finishing");
    Ok(output_path)
}

fn write_ppm_frame(path: &Path, frame: &OfflinePitchFrame) -> Result<()> {
    let mut buffer = vec![0u8; VIDEO_WIDTH * VIDEO_HEIGHT * 3];
    fill(&mut buffer, [9, 7, 12]);

    draw_capsule(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        110.0,
        164.0,
        850.0,
        218.0,
        48.0,
        [48, 17, 39],
    );
    draw_capsule(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        126.0,
        180.0,
        818.0,
        186.0,
        44.0,
        [22, 18, 28],
    );

    let pulse = frame.level.clamp(0.04, 1.0);
    draw_circle(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        172.0,
        238.0,
        18.0 + pulse * 11.0,
        [227, 82, 149],
    );
    draw_circle(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        172.0,
        238.0,
        9.0,
        [255, 213, 233],
    );

    draw_waveform(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        420.0,
        238.0,
        360.0,
        56.0,
        &frame.waveform,
    );

    let level_width = 120.0 * pulse;
    draw_capsule(
        &mut buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        130.0,
        320.0,
        level_width.max(28.0),
        12.0,
        6.0,
        [236, 116, 179],
    );

    let mut bytes = Vec::with_capacity(24 + buffer.len());
    bytes.extend_from_slice(format!("P6\n{} {}\n255\n", VIDEO_WIDTH, VIDEO_HEIGHT).as_bytes());
    bytes.extend_from_slice(&buffer);
    fs::write(path, bytes).with_context(|| format!("unable to write {}", path.display()))
}

fn build_ass_script(frames: &[OfflinePitchFrame], duration_secs: f32) -> String {
    let mut script = String::from(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 960\nPlayResY: 540\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Note,Segoe UI,62,&H00FCE2F1,&H00FCE2F1,&H00511431,&H00000000,1,0,0,0,100,100,0,0,1,1.8,0,5,0,0,140,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );

    let mut start = 0usize;
    while start < frames.len() {
        let note = &frames[start].note;
        let mut end = start + 1;
        while end < frames.len() && frames[end].note == *note {
            end += 1;
        }
        if note != "--" {
            let from = start as f32 / VIDEO_FPS as f32;
            let to = (end as f32 / VIDEO_FPS as f32).min(duration_secs.max(from + 0.05));
            script.push_str(&format!(
                "Dialogue: 0,{},{},Note,,0,0,0,,{}\n",
                ass_time(from),
                ass_time(to),
                escape_ass(note)
            ));
        }
        start = end;
    }

    script
}

fn ass_time(seconds: f32) -> String {
    let total_cs = (seconds.max(0.0) * 100.0).round() as u32;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

fn escape_ass(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
}

fn draw_waveform(
    buffer: &mut [u8],
    width: usize,
    height: usize,
    center_x: f32,
    center_y: f32,
    wave_width: f32,
    wave_height: f32,
    waveform: &[f32],
) {
    if waveform.is_empty() {
        return;
    }
    let step = wave_width / waveform.len().max(1) as f32;
    for (index, level) in waveform.iter().enumerate() {
        let x = center_x - wave_width * 0.5 + (index as f32 + 0.5) * step;
        let half = level.clamp(0.04, 1.0) * wave_height * 0.46;
        draw_rect(
            buffer,
            width,
            height,
            x - step * 0.16,
            center_y - half,
            step * 0.32,
            half * 2.0,
            [236, 116, 179],
        );
    }
}

fn draw_rect(
    buffer: &mut [u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    rect_w: f32,
    rect_h: f32,
    color: [u8; 3],
) {
    let left = x.max(0.0) as usize;
    let top = y.max(0.0) as usize;
    let right = (x + rect_w).min(width as f32) as usize;
    let bottom = (y + rect_h).min(height as f32) as usize;
    for py in top..bottom {
        for px in left..right {
            set_pixel(buffer, width, px, py, color);
        }
    }
}

fn draw_circle(
    buffer: &mut [u8],
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    radius: f32,
    color: [u8; 3],
) {
    let left = (cx - radius).max(0.0) as usize;
    let top = (cy - radius).max(0.0) as usize;
    let right = (cx + radius).min(width as f32 - 1.0) as usize;
    let bottom = (cy + radius).min(height as f32 - 1.0) as usize;
    let radius_sq = radius * radius;
    for py in top..=bottom {
        for px in left..=right {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= radius_sq {
                set_pixel(buffer, width, px, py, color);
            }
        }
    }
}

fn draw_capsule(
    buffer: &mut [u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    capsule_w: f32,
    capsule_h: f32,
    radius: f32,
    color: [u8; 3],
) {
    let left = x.max(0.0) as usize;
    let top = y.max(0.0) as usize;
    let right = (x + capsule_w).min(width as f32 - 1.0) as usize;
    let bottom = (y + capsule_h).min(height as f32 - 1.0) as usize;
    let r = radius.min(capsule_h * 0.5).min(capsule_w * 0.5);

    for py in top..=bottom {
        for px in left..=right {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let inside_core = fx >= x + r && fx <= x + capsule_w - r;
            let inside_side = fy >= y + r && fy <= y + capsule_h - r;
            let left_dx = fx - (x + r);
            let right_dx = fx - (x + capsule_w - r);
            let dy_top = fy - (y + r);
            let dy_bottom = fy - (y + capsule_h - r);
            let inside_corner = left_dx * left_dx + dy_top * dy_top <= r * r
                || right_dx * right_dx + dy_top * dy_top <= r * r
                || left_dx * left_dx + dy_bottom * dy_bottom <= r * r
                || right_dx * right_dx + dy_bottom * dy_bottom <= r * r;

            if (inside_core && fy >= y && fy <= y + capsule_h)
                || (inside_side && fx >= x && fx <= x + capsule_w)
                || inside_corner
            {
                set_pixel(buffer, width, px, py, color);
            }
        }
    }
}

fn set_pixel(buffer: &mut [u8], width: usize, x: usize, y: usize, color: [u8; 3]) {
    let index = (y * width + x) * 3;
    if index + 2 < buffer.len() {
        buffer[index] = color[0];
        buffer[index + 1] = color[1];
        buffer[index + 2] = color[2];
    }
}

fn fill(buffer: &mut [u8], color: [u8; 3]) {
    for chunk in buffer.chunks_exact_mut(3) {
        chunk[0] = color[0];
        chunk[1] = color[1];
        chunk[2] = color[2];
    }
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }

    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("recording")
        .to_owned();
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("mp4")
        .to_owned();

    for index in 2..500 {
        let candidate = path.with_file_name(format!("{stem}-{index}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}
