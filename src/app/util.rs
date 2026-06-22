use super::*;

pub(crate) fn load_ppm_color_image(path: &Path) -> Result<(egui::ColorImage, Vec2)> {
    let bytes = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut index = 0usize;
    let magic = next_ppm_token(&bytes, &mut index).context("invalid ppm header")?;
    if magic != "P6" {
        anyhow::bail!("unsupported ppm format");
    }
    let width = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm width")?
        .parse::<usize>()
        .context("invalid ppm width")?;
    let height = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm height")?
        .parse::<usize>()
        .context("invalid ppm height")?;
    let max_value = next_ppm_token(&bytes, &mut index)
        .context("invalid ppm max value")?
        .parse::<usize>()
        .context("invalid ppm max value")?;
    if max_value != 255 {
        anyhow::bail!("unsupported ppm color depth");
    }

    if index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("ppm frame too large")?;
    let data = bytes
        .get(index..index + expected)
        .context("ppm frame payload is incomplete")?;
    let image = egui::ColorImage::from_rgb([width, height], data);
    Ok((image, vec2(width as f32, height as f32)))
}

fn next_ppm_token<'a>(bytes: &'a [u8], index: &mut usize) -> Option<&'a str> {
    while *index < bytes.len() {
        let byte = bytes[*index];
        if byte == b'#' {
            while *index < bytes.len() && bytes[*index] != b'\n' {
                *index += 1;
            }
        } else if byte.is_ascii_whitespace() {
            *index += 1;
        } else {
            break;
        }
    }

    let start = *index;
    while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
    std::str::from_utf8(bytes.get(start..*index)?).ok()
}

pub(crate) fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            AUDIO_FILTERS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}
