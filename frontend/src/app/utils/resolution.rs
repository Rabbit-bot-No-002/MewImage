use mew_image_shared::{ImageAssetRef, clamp_size};

pub(crate) fn resolve_dimensions(
    mode: &str,
    group: &str,
    ratio: &str,
    custom_ratio: &str,
    custom_width: u32,
    custom_height: u32,
    references: &[ImageAssetRef],
) -> (u32, u32) {
    let reference_size = references
        .iter()
        .find_map(|asset| asset.width.zip(asset.height));
    resolve_dimensions_from_reference_size(
        mode,
        group,
        ratio,
        custom_ratio,
        custom_width,
        custom_height,
        reference_size,
    )
}

pub(crate) fn resolve_dimensions_from_reference_size(
    mode: &str,
    group: &str,
    ratio: &str,
    custom_ratio: &str,
    custom_width: u32,
    custom_height: u32,
    reference_size: Option<(u32, u32)>,
) -> (u32, u32) {
    if mode == "custom" {
        return (custom_width, custom_height);
    }
    if mode == "model_auto" {
        return mew_image_shared::AUTO_IMAGE_BUDGET_DIMENSIONS;
    }
    if mode == "auto" {
        if let Some((width, height)) = reference_size {
            let result = clamp_size(width, height);
            return (result.width, result.height);
        }
        return (1024, 1024);
    }
    if ratio == "custom" {
        return custom_ratio_dimensions(group, custom_ratio)
            .unwrap_or_else(|| preset_dimensions(group, "1:1"));
    }
    preset_dimensions(group, ratio)
}

pub(crate) fn preset_dimensions(group: &str, ratio: &str) -> (u32, u32) {
    let size = match (group, ratio) {
        ("1k", "3:2") => (1152, 768),
        ("1k", "2:3") => (768, 1152),
        ("1k", "16:9") => (1280, 720),
        ("1k", "9:16") => (720, 1280),
        ("2k", "3:2") => (2016, 1344),
        ("2k", "2:3") => (1344, 2016),
        ("2k", "16:9") => (2048, 1152),
        ("2k", "9:16") => (1152, 2048),
        ("4k", "3:2") => (3504, 2336),
        ("4k", "2:3") => (2336, 3504),
        ("4k", "16:9") => (3840, 2160),
        ("4k", "9:16") => (2160, 3840),
        ("2k", _) => (2048, 2048),
        ("4k", _) => (2880, 2880),
        _ => (1024, 1024),
    };
    let result = clamp_size(size.0, size.1);
    (result.width, result.height)
}

pub(crate) fn parse_custom_ratio(ratio: &str) -> Option<(f64, f64)> {
    let separator_index = ratio.char_indices().find_map(|(index, character)| {
        matches!(character, ':' | 'x' | 'X' | '×').then_some(index)
    })?;
    let separator_length = ratio[separator_index..].chars().next()?.len_utf8();
    let ratio_width = ratio[..separator_index].trim().parse::<f64>().ok()?;
    let ratio_height = ratio[separator_index + separator_length..]
        .trim()
        .parse::<f64>()
        .ok()?;

    if !ratio_width.is_finite()
        || !ratio_height.is_finite()
        || ratio_width <= 0.0
        || ratio_height <= 0.0
    {
        return None;
    }
    Some((ratio_width, ratio_height))
}

pub(crate) fn effective_custom_ratio(ratio: &str) -> Option<(f64, Option<&'static str>)> {
    const MAX_ASPECT_RATIO: f64 = 3.0;

    let (ratio_width, ratio_height) = parse_custom_ratio(ratio)?;
    let requested_ratio = ratio_width / ratio_height;
    if requested_ratio > MAX_ASPECT_RATIO {
        return Some((MAX_ASPECT_RATIO, Some("3:1")));
    }
    if requested_ratio < 1.0 / MAX_ASPECT_RATIO {
        return Some((1.0 / MAX_ASPECT_RATIO, Some("1:3")));
    }
    Some((requested_ratio, None))
}

pub(crate) fn custom_ratio_dimensions(group: &str, ratio: &str) -> Option<(u32, u32)> {
    const SIZE_MULTIPLE: u32 = 16;
    const MIN_EDGE: u32 = 256;
    const MAX_EDGE: u32 = 3840;
    const MIN_PIXELS: u64 = 655_360;
    const MAX_ASPECT_RATIO: f64 = 3.0;
    const MAX_RATIO_ERROR: f64 = 0.01;

    let pixel_budget = match group {
        "2k" => 4_194_304,
        "4k" => 8_294_400,
        _ => 1_572_864,
    };
    // 接受任意正比例输入，但最终尺寸必须遵守官方最大 3:1 宽高比约束。
    let (target_ratio, _) = effective_custom_ratio(ratio)?;

    let mut best_dimensions = None;
    let mut best_pixels = 0;
    let mut best_ratio_error = f64::INFINITY;
    let mut best_is_close = false;
    for width in (MIN_EDGE..=MAX_EDGE).step_by(SIZE_MULTIPLE as usize) {
        let ideal_height = width as f64 / target_ratio;
        let lower_height = ((ideal_height / SIZE_MULTIPLE as f64).floor() * SIZE_MULTIPLE as f64)
            .clamp(MIN_EDGE as f64, MAX_EDGE as f64) as u32;
        let upper_height = ((ideal_height / SIZE_MULTIPLE as f64).ceil() * SIZE_MULTIPLE as f64)
            .clamp(MIN_EDGE as f64, MAX_EDGE as f64) as u32;

        for height in [lower_height, upper_height] {
            let pixels = width as u64 * height as u64;
            if pixels < MIN_PIXELS || pixels > pixel_budget {
                continue;
            }
            let actual_ratio = width as f64 / height as f64;
            if actual_ratio.max(1.0 / actual_ratio) > MAX_ASPECT_RATIO {
                continue;
            }
            let ratio_error = (actual_ratio - target_ratio).abs() / target_ratio;
            let is_close = ratio_error <= MAX_RATIO_ERROR;
            let should_replace = if best_dimensions.is_none() {
                true
            } else if is_close != best_is_close {
                is_close
            } else if is_close {
                pixels > best_pixels
            } else {
                ratio_error < best_ratio_error
                    || ((ratio_error - best_ratio_error).abs() < f64::EPSILON
                        && pixels > best_pixels)
            };
            if !should_replace {
                continue;
            }

            best_pixels = pixels;
            best_ratio_error = ratio_error;
            best_is_close = is_close;
            best_dimensions = Some((width, height));
        }
    }
    best_dimensions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_presets_fit_official_limits_and_ratios() {
        for group in ["1k", "2k", "4k"] {
            for (ratio, ratio_width, ratio_height) in [
                ("1:1", 1_u64, 1_u64),
                ("3:2", 3, 2),
                ("2:3", 2, 3),
                ("16:9", 16, 9),
                ("9:16", 9, 16),
            ] {
                let (width, height) = preset_dimensions(group, ratio);
                assert!(width <= 3840 && height <= 3840);
                assert_eq!(width % 16, 0);
                assert_eq!(height % 16, 0);
                assert!((width as u64) * (height as u64) <= 8_294_400);
                assert_eq!(width as u64 * ratio_height, height as u64 * ratio_width);
            }
        }
        assert_eq!(preset_dimensions("2k", "1:1"), (2048, 2048));
        assert_eq!(preset_dimensions("4k", "16:9"), (3840, 2160));
        assert_eq!(preset_dimensions("4k", "1:1"), (2880, 2880));
    }

    #[test]
    fn custom_ratio_supports_decimal_and_common_separators() {
        assert_eq!(parse_custom_ratio(" 5 : 4 "), Some((5.0, 4.0)));
        assert_eq!(parse_custom_ratio("2.39×1"), Some((2.39, 1.0)));
        assert_eq!(parse_custom_ratio("16X9"), Some((16.0, 9.0)));
        assert_eq!(parse_custom_ratio("0:1"), None);
        assert_eq!(parse_custom_ratio("invalid"), None);
    }

    #[test]
    fn custom_ratio_uses_largest_valid_size_in_tier_budget() {
        assert_eq!(custom_ratio_dimensions("2k", "5:4"), Some((2288, 1824)));
        let (width, height) = custom_ratio_dimensions("4k", "2.39:1").unwrap();
        assert!(width <= 3840 && height <= 3840);
        assert_eq!(width % 16, 0);
        assert_eq!(height % 16, 0);
        assert!(width as u64 * height as u64 <= 8_294_400);
        assert!(((width as f64 / height as f64) - 2.39).abs() / 2.39 <= 0.01);
    }

    #[test]
    fn custom_ratio_uses_nearest_official_ratio_for_extreme_inputs() {
        assert_eq!(custom_ratio_dimensions("4k", "2:1"), Some((3840, 1920)));
        assert_eq!(custom_ratio_dimensions("4k", "100:1"), Some((3840, 1280)));
        assert_eq!(custom_ratio_dimensions("4k", "1:100"), Some((1280, 3840)));
        assert_eq!(effective_custom_ratio("100:1"), Some((3.0, Some("3:1"))));

        for group in ["1k", "2k", "4k"] {
            let pixel_budget = match group {
                "2k" => 4_194_304,
                "4k" => 8_294_400,
                _ => 1_572_864,
            };
            for ratio in ["2:1", "4:1", "100:1", "1:100", "2.39:1"] {
                let (width, height) = custom_ratio_dimensions(group, ratio).unwrap();
                assert!(width <= 3840 && height <= 3840);
                assert_eq!(width % 16, 0);
                assert_eq!(height % 16, 0);
                assert!(width as u64 * height as u64 <= pixel_budget);
                assert!((width as f64 / height as f64).max(height as f64 / width as f64) <= 3.0);
            }
        }
        assert_eq!(custom_ratio_dimensions("1k", "1:0"), None);
        assert_eq!(custom_ratio_dimensions("1k", "abc"), None);
    }
}
