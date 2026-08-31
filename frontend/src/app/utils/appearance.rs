use super::super::*;

const MAX_BACKGROUND_INPUT_BYTES: u64 = 15 * 1024 * 1024;
const MAX_BACKGROUND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_BACKGROUND_PIXELS: u64 = 50_000_000;
const MAX_BACKGROUND_EDGE: u32 = 4096;
const BACKGROUND_WEBP_QUALITY: f64 = 0.8;

pub(crate) struct ProcessedThemeBackground {
    pub(crate) asset: ImageAssetRef,
    pub(crate) data_url: String,
}

pub(crate) fn is_theme_background(asset: &ImageAssetRef) -> bool {
    asset
        .metadata
        .get(THEME_BACKGROUND_ROLE_KEY)
        .map(String::as_str)
        == Some(THEME_BACKGROUND_ROLE)
}

pub(crate) fn resolved_night_mode(theme: ThemePreference, system_dark: bool) -> bool {
    match theme {
        ThemePreference::Day => false,
        ThemePreference::Night => true,
        ThemePreference::System => system_dark,
    }
}

pub(crate) fn system_prefers_dark() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-color-scheme: dark)")
                .ok()
                .flatten()
        })
        .map(|query| query.matches())
        .unwrap_or(false)
}

pub(crate) fn visual_theme_attribute(theme: VisualTheme) -> &'static str {
    match theme {
        VisualTheme::Classic => "classic",
        VisualTheme::Aurora => "aurora",
        VisualTheme::LiquidGlass => "liquid-glass",
    }
}

pub(crate) fn panel_opacity_values(
    theme: VisualTheme,
    night: bool,
    configured_opacity: u8,
) -> (f64, f64) {
    let factor = f64::from(configured_opacity.clamp(20, 100)) / 100.0;
    let (panel, panel_strong) = match (theme, night) {
        (VisualTheme::LiquidGlass, false) => (0.28, 0.40),
        (VisualTheme::LiquidGlass, true) => (0.24, 0.36),
        (_, false) => (0.88, 0.96),
        (_, true) => (0.86, 0.94),
    };
    (panel * factor, panel_strong * factor)
}

pub(crate) fn apply_appearance(preferences: &AppPreferences, system_dark: bool) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(body) = document.body() else {
        return;
    };
    let night = resolved_night_mode(preferences.theme, system_dark);
    let visual_theme = visual_theme_attribute(preferences.appearance.visual_theme);
    let decoration = match preferences.appearance.decoration_level {
        DecorationLevel::Off => "off",
        DecorationLevel::Subtle => "subtle",
        DecorationLevel::Standard => "standard",
    };
    let background_layer = match preferences.appearance.custom_background.layer {
        BackgroundLayer::BelowDecorations => "below-decorations",
        BackgroundLayer::AboveDecorations => "above-decorations",
    };
    let (panel_opacity, panel_strong_opacity) = panel_opacity_values(
        preferences.appearance.visual_theme,
        night,
        preferences.appearance.panel_opacity,
    );
    let _ = body.set_attribute("data-visual-theme", visual_theme);
    let _ = body.set_attribute("data-color-scheme", if night { "night" } else { "day" });
    let _ = body.set_attribute("data-decoration", decoration);
    let _ = body.set_attribute("data-background-layer", background_layer);
    // 面板颜色令牌定义在根节点，预先计算 Alpha 可避免依赖浏览器对 calc 乘法的支持。
    if let Some(root) = document
        .document_element()
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = root
            .style()
            .set_property("--panel-opacity", &panel_opacity.to_string());
        let _ = root
            .style()
            .set_property("--panel-strong-opacity", &panel_strong_opacity.to_string());
    }
    // 暂时保留旧属性，避免迁移过程中遗漏的选择器出现明暗回归。
    let _ = body.set_attribute("data-theme", if night { "night" } else { "day" });
}

pub(crate) fn background_position_css(position: BackgroundPosition) -> &'static str {
    match position {
        BackgroundPosition::TopLeft => "left top",
        BackgroundPosition::Top => "center top",
        BackgroundPosition::TopRight => "right top",
        BackgroundPosition::Left => "left center",
        BackgroundPosition::Center => "center center",
        BackgroundPosition::Right => "right center",
        BackgroundPosition::BottomLeft => "left bottom",
        BackgroundPosition::Bottom => "center bottom",
        BackgroundPosition::BottomRight => "right bottom",
    }
}

pub(crate) async fn process_theme_background_file(
    raw_file: web_sys::File,
) -> Result<ProcessedThemeBackground, String> {
    if raw_file.size() > MAX_BACKGROUND_INPUT_BYTES as f64 {
        return Err("背景原图不能超过 15 MiB。".into());
    }
    let file = File::from(raw_file);
    let bytes = read_as_bytes(&file)
        .await
        .map_err(|error| error.to_string())?;
    let mime_type = sniff_supported_background_mime(&bytes)?;
    if mime_type == "image/webp" && is_animated_webp(&bytes) {
        return Err("暂不支持动画 WebP，请使用静态 PNG、JPEG 或 WebP。".into());
    }
    let source_data_url = bytes_to_data_url(&bytes, mime_type);
    let image = load_html_image(&source_data_url).await?;
    let source_width = image.natural_width();
    let source_height = image.natural_height();
    if source_width == 0 || source_height == 0 {
        return Err("背景图片尺寸无效。".into());
    }
    let pixel_count = u64::from(source_width) * u64::from(source_height);
    if pixel_count > MAX_BACKGROUND_PIXELS {
        return Err("背景图片解码后超过 5000 万像素，请先缩小图片。".into());
    }
    let (target_width, target_height) = scaled_background_dimensions(source_width, source_height);
    let canvas = create_background_canvas(target_width, target_height)?;
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("读取背景画布失败：{error:?}"))?
        .ok_or_else(|| "浏览器不支持 2D 画布。".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element_and_dw_and_dh(
            &image,
            0.0,
            0.0,
            f64::from(target_width),
            f64::from(target_height),
        )
        .map_err(|error| format!("绘制背景图片失败：{error:?}"))?;
    let data_url = canvas
        .to_data_url_with_type_and_encoder_options(
            "image/webp",
            &wasm_bindgen::JsValue::from_f64(BACKGROUND_WEBP_QUALITY),
        )
        .map_err(|error| format!("背景 WebP 编码失败：{error:?}"))?;
    let (encoded_mime, encoded_bytes) = decode_browser_data_url(&data_url)?;
    if encoded_mime != "image/webp" {
        return Err("当前浏览器不支持 WebP 编码，背景未保存。".into());
    }
    if encoded_bytes.len() > MAX_BACKGROUND_OUTPUT_BYTES {
        return Err("处理后的背景超过 8 MiB，请选择尺寸更小或细节更少的图片。".into());
    }
    let now = now_rfc3339();
    let mut metadata = BTreeMap::new();
    metadata.insert(
        THEME_BACKGROUND_ROLE_KEY.into(),
        THEME_BACKGROUND_ROLE.into(),
    );
    Ok(ProcessedThemeBackground {
        asset: ImageAssetRef {
            id: new_id(),
            sha256: sha256_hex(&encoded_bytes),
            mime_type: "image/webp".into(),
            byte_len: encoded_bytes.len() as u64,
            width: Some(target_width),
            height: Some(target_height),
            created_at: now.clone(),
            updated_at: now,
            data_url: Some(data_url.clone()),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata,
        },
        data_url,
    })
}

fn sniff_supported_background_mime(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    Err("仅支持静态 PNG、JPEG 和 WebP 背景图片。".into())
}

fn is_animated_webp(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return false;
    }
    let mut offset = 12usize;
    while offset.saturating_add(8) <= bytes.len() {
        let chunk_type = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        if chunk_type == b"ANIM" {
            return true;
        }
        let padded_size = chunk_size.saturating_add(chunk_size % 2);
        let Some(next_offset) = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(padded_size))
        else {
            return false;
        };
        if next_offset <= offset || next_offset > bytes.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn scaled_background_dimensions(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= MAX_BACKGROUND_EDGE {
        return (width, height);
    }
    let scale = f64::from(MAX_BACKGROUND_EDGE) / f64::from(longest);
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
}

fn create_background_canvas(width: u32, height: u32) -> Result<HtmlCanvasElement, String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "浏览器文档不可用。".to_string())?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建背景画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "背景画布元素类型错误。".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_dimensions_keep_ratio_and_limit_longest_edge() {
        assert_eq!(scaled_background_dimensions(8000, 4000), (4096, 2048));
        assert_eq!(scaled_background_dimensions(1024, 768), (1024, 768));
    }

    #[test]
    fn animated_webp_marker_is_rejected() {
        assert!(is_animated_webp(b"RIFFxxxxWEBPANIM\x00\x00\x00\x00"));
        assert!(!is_animated_webp(b"RIFFxxxxWEBPVP8 \x00\x00\x00\x00"));
    }

    #[test]
    fn liquid_glass_theme_uses_low_alpha_in_day_and_night_modes() {
        assert_eq!(
            panel_opacity_values(VisualTheme::LiquidGlass, false, 100),
            (0.28, 0.40)
        );
        assert_eq!(
            panel_opacity_values(VisualTheme::LiquidGlass, true, 100),
            (0.24, 0.36)
        );
        assert_eq!(
            panel_opacity_values(VisualTheme::LiquidGlass, false, 50),
            (0.14, 0.20)
        );
        assert_eq!(
            visual_theme_attribute(VisualTheme::LiquidGlass),
            "liquid-glass"
        );
    }

    #[test]
    fn existing_themes_keep_their_original_panel_alpha() {
        assert_eq!(
            panel_opacity_values(VisualTheme::Aurora, false, 100),
            (0.88, 0.96)
        );
        assert_eq!(
            panel_opacity_values(VisualTheme::Classic, true, 100),
            (0.86, 0.94)
        );
    }
}
