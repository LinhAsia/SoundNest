use crate::pitch::OfflinePitchFrame;
use anyhow::{Context, Result, bail};
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const VIDEO_WIDTH: usize = 960;
const VIDEO_HEIGHT: usize = 540;
pub const STANDARD_VIDEO_FPS: u32 = 60;
pub const HIGH_VIDEO_FPS: u32 = 144;
pub const LOW_VIDEO_FPS: u32 = 30;

pub fn export_record_pitch_video<F>(
    root_dir: &Path,
    ffmpeg_path: &Path,
    audio_path: &Path,
    frames: &[OfflinePitchFrame],
    duration_secs: f32,
    fps: u32,
    animated: bool,
    mut progress: F,
) -> Result<PathBuf>
where
    F: FnMut(f32, &str),
{
    if frames.is_empty() {
        bail!("recording is empty");
    }
    let fps = normalize_export_fps(fps);

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
        write_ppm_frame(&path, frame, animated)?;
        if index % 4 == 0 || index + 1 == frames.len() {
            let frame_progress = (index + 1) as f32 / frames.len().max(1) as f32;
            progress(0.45 + frame_progress * 0.35, "Rendering frames");
        }
    }

    let ass_path = temp_dir.join("notes.ass");
    fs::write(
        &ass_path,
        build_ass_script(frames, duration_secs, fps, animated),
    )
    .with_context(|| format!("unable to write {}", ass_path.display()))?;

    let mut cmd = Command::new(ffmpeg_path);
    cmd.current_dir(&temp_dir)
        .arg("-y")
        .arg("-framerate")
        .arg(fps.to_string())
        .arg("-i")
        .arg("frame_%05d.ppm")
        .arg("-i")
        .arg(audio_path)
        .arg("-vf")
        .arg(format!("ass=notes.ass,fps={fps}"))
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

fn write_ppm_frame(path: &Path, frame: &OfflinePitchFrame, animated: bool) -> Result<()> {
    let mut buffer = vec![0u8; VIDEO_WIDTH * VIDEO_HEIGHT * 3];
    fill(&mut buffer, [8, 6, 12]);

    if animated {
        render_animated_frame(&mut buffer, frame);
    } else {
        render_static_frame(&mut buffer, frame);
    }

    let mut bytes = Vec::with_capacity(24 + buffer.len());
    bytes.extend_from_slice(format!("P6\n{} {}\n255\n", VIDEO_WIDTH, VIDEO_HEIGHT).as_bytes());
    bytes.extend_from_slice(&buffer);
    fs::write(path, bytes).with_context(|| format!("unable to write {}", path.display()))
}

fn render_static_frame(buffer: &mut [u8], frame: &OfflinePitchFrame) {
    let pulse = frame.level.clamp(0.04, 1.0);
    let pitch_shift = (frame.pitch_ratio - 0.5) * 2.0;

    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        260.0 + pitch_shift * 18.0,
        162.0 - pitch_shift * 20.0,
        120.0 + pulse * 26.0,
        [28, 14, 32],
    );
    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        712.0 - pitch_shift * 22.0,
        376.0 + pitch_shift * 18.0,
        138.0 + pulse * 24.0,
        [18, 10, 24],
    );

    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        56.0,
        56.0,
        848.0,
        428.0,
        56.0,
        [39, 20, 40],
    );
    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        76.0,
        76.0,
        808.0,
        388.0,
        50.0,
        [19, 15, 26],
    );
    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        104.0,
        106.0,
        752.0,
        76.0,
        36.0,
        [30, 23, 36],
    );
    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        104.0,
        206.0,
        752.0,
        142.0,
        34.0,
        [24, 19, 30],
    );
    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        104.0,
        376.0,
        752.0,
        22.0,
        11.0,
        [40, 30, 46],
    );

    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        144.0,
        144.0,
        20.0 + pulse * 10.0,
        [235, 96, 171],
    );
    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        144.0,
        144.0,
        8.5 + pulse * 3.5,
        [255, 227, 240],
    );

    draw_waveform(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        480.0,
        277.0,
        648.0,
        84.0,
        &frame.waveform,
    );

    let level_width = 752.0 * pulse.max(0.08);
    draw_capsule(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        104.0,
        376.0,
        level_width,
        22.0,
        11.0,
        [236, 116, 179],
    );
}

fn render_animated_frame(buffer: &mut [u8], frame: &OfflinePitchFrame) {
    let pulse = frame.level.clamp(0.04, 1.0);
    let pitch_shift = (frame.pitch_ratio - 0.5) * 2.0;
    let center_x = 480.0 + pitch_shift * 18.0;
    let center_y = 270.0 - pitch_shift * 24.0;
    let base_w = 148.0 + pulse * 26.0 + pitch_shift.abs() * 10.0;
    let base_h = 132.0 + pulse * 22.0;
    let phase = frame.pitch_ratio * std::f32::consts::TAU * 1.15 + pulse * 1.8;

    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        center_x - 116.0,
        center_y - 82.0,
        base_w * 0.92,
        [22, 10, 30],
    );
    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        center_x + 108.0,
        center_y + 94.0,
        base_h * 0.98,
        [16, 10, 24],
    );
    draw_circle(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        center_x,
        center_y,
        (base_w + base_h) * 0.62,
        [33, 14, 36],
    );

    draw_organic_blob(
        buffer,
        center_x,
        center_y + 10.0,
        base_w * 1.16,
        base_h * 1.14,
        2.95,
        0.056 + pulse * 0.03,
        phase + 0.35,
        [18, 12, 24],
    );
    draw_organic_blob(
        buffer,
        center_x,
        center_y,
        base_w,
        base_h,
        3.15,
        0.068 + pulse * 0.04,
        phase,
        [78, 28, 68],
    );
    draw_organic_blob(
        buffer,
        center_x,
        center_y - 4.0,
        base_w * 0.72,
        base_h * 0.7,
        3.35,
        0.034 + pulse * 0.022,
        phase - 0.42,
        [255, 213, 232],
    );

    draw_waveform(
        buffer,
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
        center_x,
        center_y + base_h * 0.58,
        260.0 + pulse * 54.0,
        34.0 + pulse * 12.0,
        &frame.waveform,
    );

    for index in 0..6 {
        let angle = index as f32 / 6.0 * std::f32::consts::TAU + phase * 0.18;
        let orbit = base_w * (0.96 + (index % 3) as f32 * 0.1);
        let x = center_x + angle.cos() * orbit;
        let y = center_y + angle.sin() * orbit * 0.68;
        draw_circle(
            buffer,
            VIDEO_WIDTH,
            VIDEO_HEIGHT,
            x,
            y,
            4.6 + (index % 3) as f32 * 1.8 + pulse * 1.7,
            [244, 124, 186],
        );
    }
}

fn draw_organic_blob(
    buffer: &mut [u8],
    center_x: f32,
    center_y: f32,
    half_w: f32,
    half_h: f32,
    exponent: f32,
    wobble: f32,
    phase: f32,
    color: [u8; 3],
) {
    let left = (center_x - half_w * 1.18).max(0.0) as usize;
    let top = (center_y - half_h * 1.18).max(0.0) as usize;
    let right = (center_x + half_w * 1.18).min(VIDEO_WIDTH as f32 - 1.0) as usize;
    let bottom = (center_y + half_h * 1.18).min(VIDEO_HEIGHT as f32 - 1.0) as usize;
    let power = exponent.max(2.0);

    for py in top..=bottom {
        for px in left..=right {
            let fx = px as f32 + 0.5 - center_x;
            let fy = py as f32 + 0.5 - center_y;
            let norm_x = fx / half_w.max(1.0);
            let norm_y = fy / half_h.max(1.0);
            let angle = norm_y.atan2(norm_x);
            let radial = 1.0
                + wobble * (angle * 2.0 + phase).sin()
                + wobble * 0.55 * (angle * 3.0 - phase * 0.8).cos()
                + wobble * 0.32 * (angle * 5.0 + phase * 1.3).sin();
            let shape = (norm_x / radial).abs().powf(power) + (norm_y / radial).abs().powf(power);
            if shape <= 1.0 {
                set_pixel(buffer, VIDEO_WIDTH, px, py, color);
            }
        }
    }
}

fn build_ass_script(
    frames: &[OfflinePitchFrame],
    duration_secs: f32,
    fps: u32,
    animated: bool,
) -> String {
    let note_style = if animated {
        "Style: Note,Segoe UI,58,&H00FCE2F1,&H00FCE2F1,&H00511431,&H00000000,1,0,0,0,100,100,0,0,1,1.8,0,5,0,0,140,1"
    } else {
        "Style: Note,Segoe UI,54,&H00FCE2F1,&H00FCE2F1,&H00511431,&H00000000,1,0,0,0,100,100,0,0,1,1.6,0,8,0,0,112,1"
    };
    let mut script = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 960\nPlayResY: 540\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n{note_style}\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
    );

    let mut start = 0usize;
    while start < frames.len() {
        let note = &frames[start].note;
        let mut end = start + 1;
        while end < frames.len() && frames[end].note == *note {
            end += 1;
        }
        if note != "--" {
            let from = start as f32 / fps as f32;
            let to = (end as f32 / fps as f32).min(duration_secs.max(from + 0.05));
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

fn normalize_export_fps(fps: u32) -> u32 {
    match fps {
        LOW_VIDEO_FPS => LOW_VIDEO_FPS,
        HIGH_VIDEO_FPS => HIGH_VIDEO_FPS,
        _ => STANDARD_VIDEO_FPS,
    }
}
