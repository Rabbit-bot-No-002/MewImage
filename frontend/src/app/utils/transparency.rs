use super::super::*;
use wasm_bindgen::Clamped;

pub(crate) const LOCAL_BACKGROUND_ROLE_KEY: &str = "local_background_role";
pub(crate) const LOCAL_BACKGROUND_RESULT_INDEX_KEY: &str = "local_background_result_index";
pub(crate) const LOCAL_BACKGROUND_KEY_COLOR_KEY: &str = "local_background_key_color";
pub(crate) const LOCAL_BACKGROUND_ERROR_KEY: &str = "local_background_error";
pub(crate) const LOCAL_BACKGROUND_ROLE_RESULT: &str = "result";
pub(crate) const LOCAL_BACKGROUND_ROLE_FALLBACK: &str = "fallback";

const GREEN_KEY_COLOR: KeyColor = KeyColor {
    hex: "#00FF00",
    red: 0,
    green: 255,
    blue: 0,
};
const MAGENTA_KEY_COLOR: KeyColor = KeyColor {
    hex: "#FF00FF",
    red: 255,
    green: 0,
    blue: 255,
};
const KEY_COLOR_DISTANCE_LIMIT: f64 = 100.0;
const MIN_BORDER_KEY_PERCENT: usize = 15;

const LOCAL_BACKGROUND_PROMPT: &str = r#"[背景指令]
背景色选择规则：如果主体包含绿色系（绿、青绿、黄绿、草绿等）颜色，使用纯洋红色（#FF00FF）背景；否则使用纯绿色（#00FF00）背景。
背景要求：整张画布仅由所选纯色填充，无任何渐变、纹理、阴影、光照变化、地面或环境元素。
主体要求：主体完整呈现，轮廓清晰锐利，与背景保持干净的边缘分离，不要出现颜色溢出或混合。
禁止：主体本身、描边、光晕、投影或反射中不能出现所选背景色。"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyColor {
    hex: &'static str,
    red: u8,
    green: u8,
    blue: u8,
}

#[derive(Debug)]
pub(crate) struct LocalTransparencyOutput {
    pub(crate) data_url: String,
    pub(crate) mime_type: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) key_color: &'static str,
}

pub(crate) fn local_background_enabled(config: &EncryptedApiConfig) -> bool {
    normalized_background_mode(config.background.as_deref()) == "local"
}

pub(crate) fn build_local_background_prompt(prompt: &str) -> String {
    format!("{}\n\n{LOCAL_BACKGROUND_PROMPT}", prompt.trim())
}

pub(crate) async fn remove_keyed_background_from_data_url(
    data_url: &str,
    output_format: Option<&str>,
    output_compression: Option<u8>,
) -> Result<LocalTransparencyOutput, String> {
    let image = load_html_image(data_url).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "浏览器文档不可用，无法执行本地去背景。".to_string())?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建本地去背景画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "本地去背景画布元素类型错误。".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("获取本地去背景画布失败：{error:?}"))?
        .ok_or_else(|| "当前浏览器不支持 Canvas 2D，无法执行本地去背景。".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element(&image, 0.0, 0.0)
        .map_err(|error| format!("绘制待处理图片失败：{error:?}"))?;

    let image_data = context
        .get_image_data(0.0, 0.0, width as f64, height as f64)
        .map_err(|error| format!("读取待处理图片像素失败：{error:?}"))?;
    let mut pixels = image_data.data().0;
    let key_color = remove_keyed_background_from_pixels(&mut pixels, width, height)?;
    let transparent_image = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(pixels.as_slice()),
        width,
        height,
    )
    .map_err(|error| format!("构建透明图片像素失败：{error:?}"))?;
    context
        .put_image_data(&transparent_image, 0.0, 0.0)
        .map_err(|error| format!("写入透明图片像素失败：{error:?}"))?;

    let target_format = normalized_image_output_format(output_format);
    let target_mime = if target_format == "webp" {
        "image/webp"
    } else {
        "image/png"
    };
    let encoded = if target_format == "webp" {
        let compression = output_compression.unwrap_or(0).min(100) as f64;
        let quality = (100.0 - compression) / 100.0;
        canvas
            .to_data_url_with_type_and_encoder_options(
                target_mime,
                &wasm_bindgen::JsValue::from_f64(quality),
            )
            .map_err(|error| format!("编码透明 WebP 失败：{error:?}"))?
    } else {
        canvas
            .to_data_url_with_type(target_mime)
            .map_err(|error| format!("编码透明 PNG 失败：{error:?}"))?
    };
    let (mime_type, bytes) = decode_browser_data_url(&encoded)?;
    if target_format == "webp" && mime_type != "image/webp" {
        return Err("当前浏览器无法编码带透明通道的 WebP，请改用 PNG。".into());
    }

    Ok(LocalTransparencyOutput {
        data_url: encoded,
        mime_type,
        bytes,
        width,
        height,
        key_color: key_color.hex,
    })
}

pub(crate) fn detect_key_color_from_pixels(
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<KeyColor, String> {
    validate_pixel_buffer(data, width, height)?;
    let width = width as usize;
    let height = height as usize;
    let mut green_score = 0usize;
    let mut magenta_score = 0usize;
    let mut border_count = 0usize;
    let mut sample = |index: usize| {
        border_count += 1;
        if color_distance(data, index, GREEN_KEY_COLOR) < KEY_COLOR_DISTANCE_LIMIT {
            green_score += 1;
        }
        if color_distance(data, index, MAGENTA_KEY_COLOR) < KEY_COLOR_DISTANCE_LIMIT {
            magenta_score += 1;
        }
    };

    for x in 0..width {
        sample(x);
        if height > 1 {
            sample((height - 1) * width + x);
        }
    }
    for y in 1..height.saturating_sub(1) {
        sample(y * width);
        if width > 1 {
            sample(y * width + width - 1);
        }
    }

    let (key_color, best_score) = if magenta_score > green_score {
        (MAGENTA_KEY_COLOR, magenta_score)
    } else {
        (GREEN_KEY_COLOR, green_score)
    };
    if best_score.saturating_mul(100) < border_count.saturating_mul(MIN_BORDER_KEY_PERCENT) {
        return Err("图片边缘没有形成足够稳定的纯绿或纯洋红背景。".into());
    }
    Ok(key_color)
}

pub(crate) fn remove_keyed_background_from_pixels(
    data: &mut [u8],
    width: u32,
    height: u32,
) -> Result<KeyColor, String> {
    let key_color = detect_key_color_from_pixels(data, width, height)?;
    let width = width as usize;
    let height = height as usize;
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "图片尺寸过大，无法执行本地去背景。".to_string())?;
    let mut mask = vec![0u8; pixel_count];
    let mut visited = vec![0u8; pixel_count];
    let mut queue = Vec::<u32>::with_capacity(pixel_count.min(1_048_576));

    build_connected_background_mask(
        data,
        width,
        height,
        key_color,
        &mut mask,
        &mut visited,
        &mut queue,
    );
    visited.fill(0);
    add_interior_key_color_islands(
        data,
        width,
        height,
        key_color,
        &mut mask,
        &mut visited,
        &mut queue,
    );
    let distance = compute_distance_to_background(&mask, width, height, &mut queue);
    let (transparent_count, visible_count) =
        write_transparent_pixels(data, &mask, &distance, key_color);
    if transparent_count == 0 || visible_count == 0 {
        return Err("本地去背景结果无效：未能同时保留主体和透明区域。".into());
    }
    Ok(key_color)
}

fn validate_pixel_buffer(data: &[u8], width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("本地去背景图片尺寸无效。".into());
    }
    let required = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| "图片尺寸过大，无法执行本地去背景。".to_string())?;
    if data.len() < required {
        return Err("本地去背景像素数据尺寸不匹配。".into());
    }
    Ok(())
}

fn build_connected_background_mask(
    data: &[u8],
    width: usize,
    height: usize,
    key_color: KeyColor,
    mask: &mut [u8],
    visited: &mut [u8],
    queue: &mut Vec<u32>,
) {
    queue.clear();
    for x in 0..width {
        enqueue_connected(data, key_color, mask, visited, queue, x);
        enqueue_connected(
            data,
            key_color,
            mask,
            visited,
            queue,
            (height - 1) * width + x,
        );
    }
    for y in 1..height.saturating_sub(1) {
        enqueue_connected(data, key_color, mask, visited, queue, y * width);
        enqueue_connected(data, key_color, mask, visited, queue, y * width + width - 1);
    }

    let mut head = 0usize;
    while head < queue.len() {
        let index = queue[head] as usize;
        head += 1;
        visit_neighbors(index, width, height, |neighbor| {
            enqueue_connected(data, key_color, mask, visited, queue, neighbor)
        });
    }
}

fn enqueue_connected(
    data: &[u8],
    key_color: KeyColor,
    mask: &mut [u8],
    visited: &mut [u8],
    queue: &mut Vec<u32>,
    index: usize,
) {
    if visited[index] != 0 {
        return;
    }
    visited[index] = 1;
    if background_confidence(data, index, key_color) < 0.18 {
        return;
    }
    mask[index] = 1;
    queue.push(index as u32);
}

fn add_interior_key_color_islands(
    data: &[u8],
    width: usize,
    height: usize,
    key_color: KeyColor,
    mask: &mut [u8],
    visited: &mut [u8],
    queue: &mut Vec<u32>,
) {
    for seed in 0..mask.len() {
        if mask[seed] != 0
            || visited[seed] != 0
            || background_confidence(data, seed, key_color) < 0.68
        {
            continue;
        }
        queue.clear();
        visited[seed] = 1;
        queue.push(seed as u32);
        let mut head = 0usize;
        let mut confidence_sum = 0.0;
        let mut strict_count = 0usize;
        let mut strong_count = 0usize;
        while head < queue.len() {
            let index = queue[head] as usize;
            head += 1;
            let confidence = background_confidence(data, index, key_color);
            confidence_sum += confidence;
            strict_count += usize::from(confidence >= 0.68);
            strong_count += usize::from(confidence >= 0.86);
            visit_neighbors(index, width, height, |neighbor| {
                if mask[neighbor] != 0 || visited[neighbor] != 0 {
                    return;
                }
                if background_confidence(data, neighbor, key_color) < 0.24 {
                    return;
                }
                visited[neighbor] = 1;
                queue.push(neighbor as u32);
            });
        }

        let length = queue.len().max(1);
        let average = confidence_sum / length as f64;
        let strict_ratio = strict_count as f64 / length as f64;
        let strong_ratio = strong_count as f64 / length as f64;
        let should_remove = average >= 0.42
            || strict_ratio >= 0.18
            || strong_ratio >= 0.05
            || (length <= 3 && average >= 0.34);
        if should_remove {
            for &index in queue.iter() {
                mask[index as usize] = 1;
            }
        }
    }
}

fn compute_distance_to_background(
    mask: &[u8],
    width: usize,
    height: usize,
    queue: &mut Vec<u32>,
) -> Vec<u8> {
    const MAX_DISTANCE: u8 = 4;
    let mut distance = vec![0u8; mask.len()];
    queue.clear();
    for index in 0..mask.len() {
        if mask[index] == 0 && touches_background(mask, index, width, height) {
            distance[index] = 1;
            queue.push(index as u32);
        }
    }

    let mut head = 0usize;
    while head < queue.len() {
        let index = queue[head] as usize;
        head += 1;
        let current = distance[index];
        if current >= MAX_DISTANCE {
            continue;
        }
        visit_neighbors(index, width, height, |neighbor| {
            if mask[neighbor] == 0 && distance[neighbor] == 0 {
                distance[neighbor] = current + 1;
                queue.push(neighbor as u32);
            }
        });
    }
    distance
}

fn touches_background(mask: &[u8], index: usize, width: usize, height: usize) -> bool {
    let mut touches = false;
    visit_neighbors(index, width, height, |neighbor| {
        touches |= mask[neighbor] != 0;
    });
    touches
}

fn visit_neighbors(mut index: usize, width: usize, height: usize, mut visit: impl FnMut(usize)) {
    let x = index % width;
    let y = index / width;
    if x > 0 {
        visit(index - 1);
    }
    if x + 1 < width {
        visit(index + 1);
    }
    if y > 0 {
        visit(index - width);
    }
    if y + 1 < height {
        index += width;
        visit(index);
    }
}

fn write_transparent_pixels(
    data: &mut [u8],
    mask: &[u8],
    distance: &[u8],
    key_color: KeyColor,
) -> (usize, usize) {
    let mut transparent_count = 0usize;
    let mut visible_count = 0usize;
    for index in 0..mask.len() {
        let offset = index * 4;
        let red = data[offset];
        let green = data[offset + 1];
        let blue = data[offset + 2];
        let confidence = background_confidence(data, index, key_color);
        let pixel_distance = distance[index];
        let alpha = pixel_alpha(
            red,
            green,
            blue,
            mask[index] != 0,
            confidence,
            pixel_distance,
            key_color,
        );
        let cleaned = remove_color_spill(
            red,
            green,
            blue,
            alpha,
            key_color,
            confidence,
            pixel_distance,
        );
        data[offset] = cleaned.0;
        data[offset + 1] = cleaned.1;
        data[offset + 2] = cleaned.2;
        data[offset + 3] = alpha;
        transparent_count += usize::from(alpha == 0);
        visible_count += usize::from(alpha > 0);
    }
    (transparent_count, visible_count)
}

fn pixel_alpha(
    red: u8,
    green: u8,
    blue: u8,
    is_background: bool,
    confidence: f64,
    distance: u8,
    key_color: KeyColor,
) -> u8 {
    if is_background {
        return 0;
    }
    if distance > 0 {
        let transparency = edge_transparency(red, green, blue, confidence, distance, key_color);
        let alpha = (255.0 * (1.0 - transparency)).round() as u8;
        return alpha.max(match distance {
            1 => 48,
            2 => 128,
            _ => 196,
        });
    }
    let isolated_spill = key_channel_mix(red, green, blue, key_color);
    if confidence >= 0.46 && isolated_spill >= 0.45 {
        return ((255.0 * (1.0 - isolated_spill * 0.75)).round() as u8).max(96);
    }
    255
}

fn edge_transparency(
    red: u8,
    green: u8,
    blue: u8,
    confidence: f64,
    distance: u8,
    key_color: KeyColor,
) -> f64 {
    let edge_strength = match distance {
        0 | 1 => 1.0,
        2 => 0.75,
        3 => 0.45,
        _ => 0.25,
    };
    let distance_estimate = clamp01(((confidence - 0.08) / 0.84) * edge_strength);
    let channel_estimate = key_channel_mix(red, green, blue, key_color) * edge_strength;
    clamp01(distance_estimate.max(channel_estimate))
}

fn remove_color_spill(
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
    key_color: KeyColor,
    confidence: f64,
    distance: u8,
) -> (u8, u8, u8) {
    if alpha == 0 {
        return (red, green, blue);
    }
    let edge_strength = match distance {
        0 if confidence >= 0.46 => 0.35,
        0 => 0.0,
        1 => 0.55,
        2 => 0.32,
        _ => 0.16,
    };
    let spill_mix = key_channel_mix(red, green, blue, key_color) * edge_strength;
    let background_mix = clamp01(
        ((255 - alpha) as f64 / 255.0)
            .max(((confidence - 0.1) / 0.9) * edge_strength)
            .max(spill_mix),
    );
    if background_mix <= 0.0 {
        return (red, green, blue);
    }
    let foreground_mix = (1.0 - background_mix).max(0.08);
    (
        clamp_byte((red as f64 - key_color.red as f64 * background_mix) / foreground_mix),
        clamp_byte((green as f64 - key_color.green as f64 * background_mix) / foreground_mix),
        clamp_byte((blue as f64 - key_color.blue as f64 * background_mix) / foreground_mix),
    )
}

fn key_channel_mix(red: u8, green: u8, blue: u8, key_color: KeyColor) -> f64 {
    if key_color.green == 255 {
        clamp01((green as f64 - red.min(blue) as f64) / 255.0)
    } else {
        clamp01((red.min(blue) as f64 - green as f64 * 0.65) / 255.0)
    }
}

fn background_confidence(data: &[u8], index: usize, key_color: KeyColor) -> f64 {
    clamp01((150.0 - color_distance(data, index, key_color)) / 150.0)
}

fn color_distance(data: &[u8], index: usize, key_color: KeyColor) -> f64 {
    let offset = index * 4;
    let red = data[offset] as f64 - key_color.red as f64;
    let green = data[offset + 1] as f64 - key_color.green as f64;
    let blue = data[offset + 2] as f64 - key_color.blue as f64;
    (red * red + green * green + blue * blue).sqrt()
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn clamp_byte(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_both_key_colors_without_changing_original_text() {
        let prompt = build_local_background_prompt("绿色机器人贴纸");
        assert!(prompt.starts_with("绿色机器人贴纸"));
        assert!(prompt.contains("#00FF00"));
        assert!(prompt.contains("#FF00FF"));
        assert!(prompt.contains("纯色填充"));
    }

    #[test]
    fn detects_green_and_magenta_from_border_pixels() {
        let mut green = solid_pixels(5, 5, [0, 255, 0, 255]);
        set_pixel(&mut green, 5, 2, 2, [180, 20, 20, 255]);
        assert_eq!(
            detect_key_color_from_pixels(&green, 5, 5).unwrap().hex,
            GREEN_KEY_COLOR.hex
        );

        let mut magenta = solid_pixels(5, 5, [255, 0, 255, 255]);
        set_pixel(&mut magenta, 5, 2, 2, [20, 190, 60, 255]);
        assert_eq!(
            detect_key_color_from_pixels(&magenta, 5, 5).unwrap().hex,
            MAGENTA_KEY_COLOR.hex
        );
    }

    #[test]
    fn rejects_border_without_reliable_key_color() {
        let pixels = solid_pixels(5, 5, [128, 128, 128, 255]);
        assert!(detect_key_color_from_pixels(&pixels, 5, 5).is_err());
    }

    #[test]
    fn removes_connected_background_and_interior_island() {
        let mut pixels = solid_pixels(5, 5, [0, 255, 0, 255]);
        for (x, y) in [(2, 0), (2, 1), (1, 2), (3, 2), (2, 3), (2, 4)] {
            set_pixel(&mut pixels, 5, x, y, [180, 20, 20, 255]);
        }
        remove_keyed_background_from_pixels(&mut pixels, 5, 5).unwrap();
        assert_eq!(pixel(&pixels, 5, 0, 0)[3], 0);
        assert_eq!(pixel(&pixels, 5, 2, 2)[3], 0);
        assert!(pixel(&pixels, 5, 2, 1)[3] > 0);
        assert!(pixel(&pixels, 5, 2, 1)[1] < 80);
    }

    #[test]
    fn magenta_key_keeps_green_foreground_opaque() {
        let mut pixels = solid_pixels(3, 3, [255, 0, 255, 255]);
        set_pixel(&mut pixels, 3, 1, 1, [20, 190, 60, 255]);
        remove_keyed_background_from_pixels(&mut pixels, 3, 3).unwrap();
        assert_eq!(pixel(&pixels, 3, 0, 0)[3], 0);
        assert_eq!(pixel(&pixels, 3, 1, 1), [20, 190, 60, 255]);
    }

    fn solid_pixels(width: usize, height: usize, color: [u8; 4]) -> Vec<u8> {
        let mut pixels = vec![0; width * height * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
        pixels
    }

    fn set_pixel(pixels: &mut [u8], width: usize, x: usize, y: usize, color: [u8; 4]) {
        let offset = (y * width + x) * 4;
        pixels[offset..offset + 4].copy_from_slice(&color);
    }

    fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * width + x) * 4;
        pixels[offset..offset + 4].try_into().unwrap()
    }
}
