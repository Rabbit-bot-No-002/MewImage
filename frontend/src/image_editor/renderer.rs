use std::f64::consts::TAU;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement};

use super::{DrawingObject, EditMode, EditorDraft, Geometry, Point};

fn canvas_error(error: impl Into<JsValue>) -> String {
    let error = error.into();
    format!("编辑画布处理失败：{error:?}")
}

/// 持有临时画布；包括编码失败在内的所有退出路径都释放像素缓冲。
pub struct RenderedCanvas {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    is_mask: bool,
    full_resolution: bool,
}

impl Drop for RenderedCanvas {
    fn drop(&mut self) {
        self.canvas.set_width(0);
        self.canvas.set_height(0);
    }
}

impl RenderedCanvas {
    pub(super) fn validate_mask_image(
        mask: &HtmlImageElement,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if mask.natural_width() != width || mask.natural_height() != height {
            return Err("遮罩尺寸必须与编辑底图完全一致。".into());
        }
        let mut canvas = Self::new(width, height)?;
        canvas.is_mask = true;
        canvas
            .context
            .draw_image_with_html_image_element(mask, 0.0, 0.0)
            .map_err(canvas_error)?;
        if !canvas.has_editable_pixels()? {
            return Err("遮罩没有透明编辑区域。".into());
        }
        Ok(())
    }

    /// 仅用于已确认的等比缩小；不能作为普通编辑输出的隐式尺寸修正。
    pub(super) fn resized_base_copy(
        base: &HtmlImageElement,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if super::fitted_work_dimensions(base.natural_width(), base.natural_height())?
            != (width, height)
        {
            return Err("缩小尺寸不是原图的有效等比工作尺寸。".into());
        }
        let result = Self::new(width, height)?;
        result
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(
                base,
                0.0,
                0.0,
                f64::from(width),
                f64::from(height),
            )
            .map_err(canvas_error)?;
        Ok(result)
    }

    pub(super) fn base_copy(
        base: &HtmlImageElement,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if base.natural_width() != width || base.natural_height() != height {
            return Err("底图与编辑工作副本尺寸不一致。".into());
        }
        let result = Self::new(width, height)?;
        result
            .context
            .draw_image_with_html_image_element(base, 0.0, 0.0)
            .map_err(canvas_error)?;
        Ok(result)
    }

    fn new(width: u32, height: u32) -> Result<Self, String> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("浏览器文档不可用。")?;
        let canvas = document
            .create_element("canvas")
            .map_err(canvas_error)?
            .dyn_into::<HtmlCanvasElement>()
            .map_err(canvas_error)?;
        let context = canvas
            .get_context("2d")
            .map_err(canvas_error)?
            .ok_or("无法创建编辑画布上下文。")?
            .dyn_into::<CanvasRenderingContext2d>()
            .map_err(canvas_error)?;
        // 取得上下文后再分配像素，初始化失败时不留下大尺寸画布。
        canvas.set_width(width);
        canvas.set_height(height);
        Ok(Self {
            canvas,
            context,
            is_mask: false,
            full_resolution: true,
        })
    }

    /// 预览可指定较小的最长边；正式输出传入工作尺寸的最长边。
    /// 遮罩返回独立透明选区，不混入底图或预览高亮颜色。
    pub fn render(
        draft: &EditorDraft,
        base: Option<&HtmlImageElement>,
        imported_mask: Option<&HtmlImageElement>,
        maximum_edge: u32,
    ) -> Result<Self, String> {
        Self::render_preview(draft, base, imported_mask, maximum_edge, None)
    }

    /// 实时笔画单独传入，不复制已经保存的全部图层。
    pub fn render_preview(
        draft: &EditorDraft,
        base: Option<&HtmlImageElement>,
        imported_mask: Option<&HtmlImageElement>,
        maximum_edge: u32,
        live_object: Option<&DrawingObject>,
    ) -> Result<Self, String> {
        draft.validate_size()?;
        if draft.mode == EditMode::Mask
            && draft.imported_mask_asset_id.is_some() != imported_mask.is_some()
        {
            return Err("导入遮罩尚未加载，不能忽略遮罩继续渲染或生成。".into());
        }
        if let Some(object) = live_object {
            object.validate()?;
            if draft.mode == EditMode::Mask && !matches!(object.geometry, Geometry::Stroke { .. }) {
                return Err("遮罩预览只能包含画笔。".into());
            }
        }
        if maximum_edge == 0 {
            return Err("预览尺寸不能为零。".into());
        }
        if draft.mode == EditMode::Annotation && base.is_none() {
            return Err("标记模式缺少底图。".into());
        }
        if let Some(base) = base
            && (base.natural_width() != draft.width || base.natural_height() != draft.height)
        {
            return Err("底图与编辑工作副本尺寸不一致。".into());
        }
        let scale = (f64::from(maximum_edge) / f64::from(draft.width.max(draft.height))).min(1.0);
        let width = (f64::from(draft.width) * scale).round().max(1.0) as u32;
        let height = (f64::from(draft.height) * scale).round().max(1.0) as u32;
        let mut layer = Self::new(width, height)?;
        layer.is_mask = draft.mode == EditMode::Mask;
        layer.full_resolution = width == draft.width && height == draft.height;
        layer
            .context
            .scale(
                f64::from(width) / f64::from(draft.width),
                f64::from(height) / f64::from(draft.height),
            )
            .map_err(canvas_error)?;
        if draft.mode == EditMode::Mask {
            if let Some(mask) = imported_mask {
                layer
                    .context
                    .draw_image_with_html_image_element_and_dw_and_dh(
                        mask,
                        0.0,
                        0.0,
                        f64::from(draft.width),
                        f64::from(draft.height),
                    )
                    .map_err(canvas_error)?;
            } else {
                layer.context.set_fill_style_str("#ffffff");
                layer
                    .context
                    .fill_rect(0.0, 0.0, f64::from(draft.width), f64::from(draft.height));
            }
        }
        for object in draft.active_objects() {
            draw_object(&layer.context, object, draft.mode)?;
        }
        if let Some(object) = live_object {
            draw_object(&layer.context, object, draft.mode)?;
            layer.full_resolution = false;
        }
        if draft.mode == EditMode::Mask {
            return Ok(layer);
        }
        // 先在独立透明层擦除，再叠到底图，避免橡皮破坏原图像素。
        let mut result = Self::new(width, height)?;
        result.full_resolution = layer.full_resolution;
        if draft.mode == EditMode::Sketch {
            result.context.set_fill_style_str(&draft.background);
            result
                .context
                .fill_rect(0.0, 0.0, f64::from(width), f64::from(height));
        } else if let Some(base) = base {
            result
                .context
                .draw_image_with_html_image_element_and_dw_and_dh(
                    base,
                    0.0,
                    0.0,
                    f64::from(width),
                    f64::from(height),
                )
                .map_err(canvas_error)?;
        }
        result
            .context
            .draw_image_with_html_canvas_element(&layer.canvas, 0.0, 0.0)
            .map_err(canvas_error)?;
        Ok(result)
    }

    pub fn canvas(&self) -> &HtmlCanvasElement {
        &self.canvas
    }

    /// 仅供交互预览：遮罩透明区域显示为彩色覆盖层，正式编码仍输出原始 Alpha。
    pub fn render_editor_preview(
        draft: &EditorDraft,
        base: Option<&HtmlImageElement>,
        imported_mask: Option<&HtmlImageElement>,
        maximum_edge: u32,
        live_object: Option<&DrawingObject>,
    ) -> Result<Self, String> {
        let layer = Self::render_preview(draft, base, imported_mask, maximum_edge, live_object)?;
        if draft.mode != EditMode::Mask {
            return Ok(layer);
        }
        let base = base.ok_or("正在加载局部修改底图。")?;
        let width = layer.canvas.width();
        let height = layer.canvas.height();
        layer
            .context
            .set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
            .map_err(canvas_error)?;
        layer
            .context
            .set_global_composite_operation("source-out")
            .map_err(canvas_error)?;
        layer.context.set_fill_style_str("rgba(64, 150, 255, 0.45)");
        layer
            .context
            .fill_rect(0.0, 0.0, f64::from(width), f64::from(height));
        let mut preview = Self::new(width, height)?;
        preview.full_resolution = false;
        preview
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(
                base,
                0.0,
                0.0,
                f64::from(width),
                f64::from(height),
            )
            .map_err(canvas_error)?;
        preview
            .context
            .draw_image_with_html_canvas_element(&layer.canvas, 0.0, 0.0)
            .map_err(canvas_error)?;
        Ok(preview)
    }

    /// 消耗临时画布并直接编码为 PNG Blob，不生成中间 Data URL。
    /// 调用方须先获取编辑编码内存预算，并只对完整工作尺寸结果调用。
    pub async fn into_png_blob(self) -> Result<Blob, String> {
        if !self.full_resolution {
            return Err("降采样预览不能作为生成输入，请按工作尺寸重新绘制。".into());
        }
        if self.is_mask && !self.has_editable_pixels()? {
            return Err("遮罩没有有效的透明编辑区域，请先绘制选区。".into());
        }
        // 使用 Promise 自带回调，避免取消 Rust future 时遗留 Closure。
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            if let Err(error) = self.canvas.to_blob_with_type(&resolve, "image/png") {
                let _ = reject.call1(&JsValue::UNDEFINED, &error);
            }
        });
        let blob = JsFuture::from(promise)
            .await
            .map_err(canvas_error)?
            .dyn_into::<Blob>()
            .map_err(|_| "浏览器无法编码图片，请检查内存或图片来源权限。".to_string())?;
        validate_encoded_png(&blob.type_(), blob.size())?;
        Ok(blob)
    }

    fn has_editable_pixels(&self) -> Result<bool, String> {
        // 分条读取，避免为 4096² 遮罩额外分配整张 64 MiB RGBA。
        for top in (0..self.canvas.height()).step_by(64) {
            let rows = (self.canvas.height() - top).min(64);
            let pixels = self
                .context
                .get_image_data(
                    0.0,
                    f64::from(top),
                    f64::from(self.canvas.width()),
                    f64::from(rows),
                )
                .map_err(canvas_error)?
                .data();
            if contains_transparent_pixel(&pixels.0) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn validate_encoded_png(mime: &str, bytes: f64) -> Result<(), String> {
    if mime != "image/png" || !bytes.is_finite() || bytes <= 0.0 {
        return Err("浏览器未返回有效 PNG 图片。".into());
    }
    if bytes > 32.0 * 1024.0 * 1024.0 {
        return Err("编辑图片超过单文件 32 MiB 限制，请缩小工作副本。".into());
    }
    Ok(())
}

fn contains_transparent_pixel(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).any(|pixel| pixel[3] == 0)
}

fn composite_operation(mode: EditMode, erase: bool) -> &'static str {
    match (mode, erase) {
        (EditMode::Mask, false) | (EditMode::Annotation | EditMode::Sketch, true) => {
            "destination-out"
        }
        _ => "source-over",
    }
}

fn draw_object(
    context: &CanvasRenderingContext2d,
    object: &DrawingObject,
    mode: EditMode,
) -> Result<(), String> {
    let erase = matches!(object.geometry, Geometry::Stroke { erase: true, .. });
    context
        .set_global_composite_operation(composite_operation(mode, erase))
        .map_err(canvas_error)?;
    let color = if mode == EditMode::Mask {
        "#ffffff"
    } else {
        &object.color
    };
    context.set_stroke_style_str(color);
    context.set_fill_style_str(color);
    context.set_line_width(object.width);
    context.set_line_cap("round");
    context.set_line_join("round");
    context.begin_path();
    match &object.geometry {
        Geometry::Stroke { points, .. } => {
            let first = points.first().ok_or("笔画没有坐标。")?;
            if points.len() == 1 {
                context
                    .arc(first.x, first.y, object.width / 2.0, 0.0, TAU)
                    .map_err(canvas_error)?;
                context.fill();
                return Ok(());
            }
            context.move_to(first.x, first.y);
            for point in &points[1..] {
                context.line_to(point.x, point.y);
            }
        }
        Geometry::Rectangle { start, end } => {
            context.rect(start.x, start.y, end.x - start.x, end.y - start.y)
        }
        Geometry::Ellipse { start, end } => {
            context
                .ellipse(
                    (start.x + end.x) / 2.0,
                    (start.y + end.y) / 2.0,
                    (end.x - start.x).abs() / 2.0,
                    (end.y - start.y).abs() / 2.0,
                    0.0,
                    0.0,
                    TAU,
                )
                .map_err(canvas_error)?;
        }
        Geometry::Arrow { start, end } => {
            context.move_to(start.x, start.y);
            context.line_to(end.x, end.y);
            for tip in arrow_wings(*start, *end, object.width) {
                context.move_to(end.x, end.y);
                context.line_to(tip.x, tip.y);
            }
        }
        Geometry::Text { position, text } => {
            let size = object.width.max(12.0);
            context.set_font(&format!("{size}px sans-serif"));
            context.set_text_baseline("top");
            for (index, line) in text.lines().enumerate() {
                context
                    .fill_text(line, position.x, position.y + index as f64 * size * 1.2)
                    .map_err(canvas_error)?;
            }
            return Ok(());
        }
    }
    context.stroke();
    Ok(())
}

pub(super) fn arrow_wings(start: Point, end: Point, width: f64) -> [Point; 2] {
    let angle = (end.y - start.y).atan2(end.x - start.x);
    let length = (width * 3.0)
        .max(12.0)
        .min((end.x - start.x).hypot(end.y - start.y) / 2.0);
    [-0.5_f64, 0.5].map(|spread| Point {
        x: end.x - length * (angle + spread).cos(),
        y: end.y - length * (angle + spread).sin(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_validation_requires_fully_transparent_pixel() {
        assert!(!contains_transparent_pixel(&[]));
        assert!(!contains_transparent_pixel(&[0, 0, 0, 255, 0, 0, 0, 128]));
        assert!(contains_transparent_pixel(&[255, 255, 255, 0]));
        assert!(!contains_transparent_pixel(&[0, 0, 0]));
    }

    #[test]
    fn png_encoding_obeys_existing_file_limit() {
        let limit = 32.0 * 1024.0 * 1024.0;
        assert!(validate_encoded_png("image/png", limit).is_ok());
        assert!(validate_encoded_png("image/png", limit + 1.0).is_err());
        assert!(validate_encoded_png("image/webp", 100.0).is_err());
        assert!(validate_encoded_png("image/png", 0.0).is_err());
        assert!(validate_encoded_png("image/png", f64::NAN).is_err());
    }

    #[test]
    fn mask_brush_clears_alpha_and_eraser_restores_it() {
        assert_eq!(
            composite_operation(EditMode::Mask, false),
            "destination-out"
        );
        assert_eq!(composite_operation(EditMode::Mask, true), "source-over");
        for mode in [EditMode::Annotation, EditMode::Sketch] {
            assert_eq!(composite_operation(mode, false), "source-over");
            assert_eq!(composite_operation(mode, true), "destination-out");
        }
    }

    #[test]
    fn arrow_wings_are_symmetric_and_bounded() {
        let wings = arrow_wings(Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }, 8.0);
        assert_eq!(wings[0].x, wings[1].x);
        assert_eq!(wings[0].y, -wings[1].y);
        assert!(wings[0].x > 50.0);
    }
}
