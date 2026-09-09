use crate::storage::SoundEffect;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct DuplicatePair {
    pub sound_a_id: Uuid,
    pub sound_b_id: Uuid,
    pub similarity: f32,
}

/// Resample a waveform vector to a standardized target bucket count using linear interpolation.
pub fn resample_waveform(waveform: &[f32], target_len: usize) -> Vec<f32> {
    if waveform.is_empty() || target_len == 0 {
        return vec![0.0; target_len];
    }
    if waveform.len() == target_len {
        return waveform.to_vec();
    }
    let src_len = waveform.len();
    let mut res = Vec::with_capacity(target_len);
    for i in 0..target_len {
        let t = (i as f32 / (target_len.max(1) - 1).max(1) as f32) * (src_len - 1) as f32;
        let idx = (t.floor() as usize).min(src_len - 1);
        let next_idx = (idx + 1).min(src_len - 1);
        let frac = t - idx as f32;
        let val = waveform[idx] * (1.0 - frac) + waveform[next_idx] * frac;
        res.push(val);
    }
    res
}

/// Compute waveform similarity using Pearson Correlation with mean subtraction and lag/shift.
/// Mean subtraction ensures that baseline/silence DC offsets do not cause false positives.
/// Returns a value in [0.0, 1.0].
pub fn compute_waveform_similarity(w1: &[f32], w2: &[f32]) -> f32 {
    if w1.is_empty() || w2.is_empty() {
        return 0.0;
    }

    const TARGET_LEN: usize = 320;
    let a = resample_waveform(w1, TARGET_LEN);
    let b = resample_waveform(w2, TARGET_LEN);

    let mean_a = a.iter().sum::<f32>() / TARGET_LEN as f32;
    let mean_b = b.iter().sum::<f32>() / TARGET_LEN as f32;

    let var_a: f32 = a.iter().map(|&x| (x - mean_a) * (x - mean_a)).sum();
    let var_b: f32 = b.iter().map(|&x| (x - mean_b) * (x - mean_b)).sum();

    // If either waveform is flat / silent (variance near 0), they cannot be meaningful duplicates
    if var_a < 1e-4 || var_b < 1e-4 {
        return 0.0;
    }

    let std_a = var_a.sqrt();
    let std_b = var_b.sqrt();

    // Max lag: 12 buckets out of 320 (~3.7% shift tolerance for encoder padding)
    let max_shift = 12isize;
    let mut best_r = 0.0f32;

    for shift in -max_shift..=max_shift {
        let start_i = (0isize).max(-shift) as usize;
        let end_i = (TARGET_LEN as isize).min(TARGET_LEN as isize - shift) as usize;

        if end_i <= start_i {
            continue;
        }

        let mut cov = 0.0f32;
        let mut count = 0usize;

        for i in start_i..end_i {
            let j = (i as isize + shift) as usize;
            cov += (a[i] - mean_a) * (b[j] - mean_b);
            count += 1;
        }

        if count >= TARGET_LEN - (max_shift as usize * 2) {
            let r = cov / (std_a * std_b);
            if r > best_r {
                best_r = r;
            }
        }
    }

    if best_r <= 0.0 {
        return 0.0;
    }

    // Mean absolute error between normalized envelope shapes
    let mut mae = 0.0f32;
    for i in 0..TARGET_LEN {
        mae += (a[i] - b[i]).abs();
    }
    mae /= TARGET_LEN as f32;
    let shape_sim = (1.0 - mae / 0.35).clamp(0.0, 1.0);

    // 70% Pearson correlation (phase/peaks align) + 30% envelope shape closeness
    (best_r * 0.70 + shape_sim * 0.30).clamp(0.0, 1.0)
}

/// Compute combined sound similarity taking into account both waveform correlation
/// and duration match. Returns a value in [0.0, 1.0].
pub fn compute_sound_similarity(d1: f32, w1: &[f32], d2: f32, w2: &[f32]) -> f32 {
    let max_dur = d1.max(d2);
    let min_dur = d1.min(d2);
    if max_dur <= 0.01 {
        return 0.0;
    }

    let dur_ratio = min_dur / max_dur;
    let dur_diff = (d1 - d2).abs();

    // Stricter duration constraint:
    // Duplicate audio clips must have very close durations (within 12% and within 0.40s)
    if dur_ratio < 0.88 || dur_diff > 0.40 {
        return 0.0;
    }

    let wave_sim = compute_waveform_similarity(w1, w2);
    if wave_sim < 0.70 {
        return 0.0;
    }

    // Weighted blend: 85% waveform match + 15% duration ratio
    (wave_sim * 0.85 + dur_ratio * 0.15).clamp(0.0, 1.0)
}

/// Fast scan of the entire sound library for duplicates using duration pre-sorting.
/// Runs in O(N * k) time (a few milliseconds for 1,000+ sounds).
pub fn find_library_duplicates(sounds: &[SoundEffect], threshold: f32) -> Vec<DuplicatePair> {
    if sounds.len() < 2 {
        return Vec::new();
    }

    let mut indexed: Vec<(usize, &SoundEffect)> = sounds.iter().enumerate().collect();
    indexed.sort_by(|a, b| a.1.duration_secs.total_cmp(&b.1.duration_secs));

    let mut pairs = Vec::new();

    for i in 0..indexed.len() {
        let (_, sound_a) = indexed[i];
        if sound_a.waveform.is_empty() || sound_a.duration_secs <= 0.01 {
            continue;
        }

        for j in (i + 1)..indexed.len() {
            let (_, sound_b) = indexed[j];
            if sound_b.waveform.is_empty() || sound_b.duration_secs <= 0.01 {
                continue;
            }

            let dur_diff = sound_b.duration_secs - sound_a.duration_secs;
            // Since sorted ascending by duration, once difference exceeds 0.40s, all remaining will be larger
            if dur_diff > 0.40 {
                break;
            }

            let dur_ratio = sound_a.duration_secs / sound_b.duration_secs;
            if dur_ratio < 0.88 {
                continue;
            }

            let sim = compute_sound_similarity(
                sound_a.duration_secs,
                &sound_a.waveform,
                sound_b.duration_secs,
                &sound_b.waveform,
            );

            if sim >= threshold {
                pairs.push(DuplicatePair {
                    sound_a_id: sound_a.id,
                    sound_b_id: sound_b.id,
                    similarity: sim,
                });
            }
        }
    }

    // Sort by highest similarity first
    pairs.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    pairs
}


/// Check a candidate audio clip (e.g. downloaded or imported) against the existing sound library.
/// Returns the best match sound and similarity score if similarity >= threshold.
#[allow(dead_code)]
pub fn check_sound_against_library<'a>(
    duration: f32,
    waveform: &[f32],
    sounds: &'a [SoundEffect],
    threshold: f32,
) -> Option<(&'a SoundEffect, f32)> {
    if waveform.is_empty() || duration <= 0.01 {
        return None;
    }

    let mut best_match: Option<(&'a SoundEffect, f32)> = None;

    for sound in sounds {
        if sound.waveform.is_empty() || sound.duration_secs <= 0.01 {
            continue;
        }
        let dur_ratio = (duration.min(sound.duration_secs)) / (duration.max(sound.duration_secs));
        let dur_diff = (duration - sound.duration_secs).abs();
        if dur_ratio < 0.70 && dur_diff > 0.8 {
            continue;
        }

        let sim = compute_sound_similarity(
            duration,
            waveform,
            sound.duration_secs,
            &sound.waveform,
        );

        if sim >= threshold {
            if let Some((_, best_sim)) = best_match {
                if sim > best_sim {
                    best_match = Some((sound, sim));
                }
            } else {
                best_match = Some((sound, sim));
            }
        }
    }

    best_match
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_waveforms() {
        let wave = vec![0.1, 0.4, 0.8, 1.0, 0.6, 0.2, 0.05];
        let sim = compute_waveform_similarity(&wave, &wave);
        assert!(sim > 0.99, "Identical waveforms should have similarity ~1.0, got {sim}");
        let sound_sim = compute_sound_similarity(2.5, &wave, 2.5, &wave);
        assert!(sound_sim > 0.99, "Identical sounds should have similarity ~1.0, got {sound_sim}");
    }

    #[test]
    fn test_shifted_waveforms() {
        let mut wave1 = vec![0.0; 320];
        let mut wave2 = vec![0.0; 320];

        // Put a peak at bucket 100 in wave1 and at bucket 105 in wave2 (5 buckets shift ~ MP3 padding)
        for i in 90..110 {
            let val = ((i as f32 - 100.0) / 10.0 * std::f32::consts::PI * 0.5).cos().max(0.0);
            wave1[i] = val;
        }
        for i in 95..115 {
            let val = ((i as f32 - 105.0) / 10.0 * std::f32::consts::PI * 0.5).cos().max(0.0);
            wave2[i] = val;
        }

        let sim = compute_waveform_similarity(&wave1, &wave2);
        assert!(
            sim > 0.95,
            "Shifted waveform within max lag should match with high similarity, got {sim}"
        );
    }

    #[test]
    fn test_different_waveforms() {
        // Wave 1: early impulse
        let mut wave1 = vec![0.0; 320];
        for i in 10..30 {
            wave1[i] = 1.0;
        }

        // Wave 2: late impulse
        let mut wave2 = vec![0.0; 320];
        for i in 280..300 {
            wave2[i] = 1.0;
        }

        let sim = compute_waveform_similarity(&wave1, &wave2);
        assert!(
            sim < 0.20,
            "Completely different waveforms should have low similarity, got {sim}"
        );
    }


    #[test]
    fn test_resample_waveform() {
        let wave = vec![0.0, 1.0];
        let resampled = resample_waveform(&wave, 5);
        assert_eq!(resampled.len(), 5);
        assert!((resampled[0] - 0.0).abs() < 1e-5);
        assert!((resampled[4] - 1.0).abs() < 1e-5);
        assert!((resampled[2] - 0.5).abs() < 1e-5);
    }
}
