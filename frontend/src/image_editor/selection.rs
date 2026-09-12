use super::{DrawingObject, EditorDraft, Geometry, Point, Viewport, renderer::arrow_wings};

fn distance_to_segment(point: Point, start: Point, end: Point) -> f64 {
    let delta = Point {
        x: end.x - start.x,
        y: end.y - start.y,
    };
    let squared = delta.x * delta.x + delta.y * delta.y;
    let fraction = if squared == 0.0 {
        0.0
    } else {
        (((point.x - start.x) * delta.x + (point.y - start.y) * delta.y) / squared).clamp(0.0, 1.0)
    };
    (point.x - start.x - fraction * delta.x).hypot(point.y - start.y - fraction * delta.y)
}

fn inside_box(point: Point, start: Point, end: Point, tolerance: f64) -> bool {
    point.x >= start.x.min(end.x) - tolerance
        && point.x <= start.x.max(end.x) + tolerance
        && point.y >= start.y.min(end.y) - tolerance
        && point.y <= start.y.max(end.y) + tolerance
}

impl EditorDraft {
    /// 返回当前模式最上层命中的对象，不复制对象或跨模式选择。
    /// 文字宽度由调用方使用与渲染相同的 Canvas 字体测量，避免中文字符估算偏差。
    pub fn object_at(
        &self,
        viewport: &Viewport,
        screen: Point,
        tolerance_css_pixels: f64,
        mut measure_text: impl FnMut(&str, f64) -> f64,
    ) -> Option<&DrawingObject> {
        if !screen.x.is_finite()
            || !screen.y.is_finite()
            || !tolerance_css_pixels.is_finite()
            || tolerance_css_pixels < 0.0
        {
            return None;
        }
        let point = viewport.to_image(screen);
        let tolerance = tolerance_css_pixels / viewport.scale();
        self.active_objects()
            .iter()
            .rev()
            .find(|object| hits(object, point, tolerance, &mut measure_text))
    }
}

fn hits(
    object: &DrawingObject,
    point: Point,
    tolerance: f64,
    measure_text: &mut impl FnMut(&str, f64) -> f64,
) -> bool {
    let radius = tolerance + object.width / 2.0;
    match &object.geometry {
        // 橡皮不是可见标记，不拦截底下对象的选择。
        Geometry::Stroke { erase: true, .. } => false,
        Geometry::Stroke { points, .. } => {
            points
                .first()
                .is_some_and(|first| distance_to_segment(point, *first, *first) <= radius)
                || points
                    .windows(2)
                    .any(|pair| distance_to_segment(point, pair[0], pair[1]) <= radius)
        }
        Geometry::Arrow { start, end } => {
            distance_to_segment(point, *start, *end) <= radius
                || arrow_wings(*start, *end, object.width)
                    .into_iter()
                    .any(|wing| distance_to_segment(point, *end, wing) <= radius)
        }
        // 形状内部也可拖动，细线矩形和椭圆在触屏上更容易选中。
        Geometry::Rectangle { start, end } => inside_box(point, *start, *end, radius),
        Geometry::Ellipse { start, end } => {
            let horizontal = (end.x - start.x).abs() / 2.0 + radius;
            let vertical = (end.y - start.y).abs() / 2.0 + radius;
            let center = Point {
                x: (start.x + end.x) / 2.0,
                y: (start.y + end.y) / 2.0,
            };
            ((point.x - center.x) / horizontal).powi(2) + ((point.y - center.y) / vertical).powi(2)
                <= 1.0
        }
        Geometry::Text { position, text } => {
            let size = object.width.max(12.0);
            text.lines().enumerate().any(|(index, line)| {
                let width = measure_text(line, size);
                if line.is_empty() || !width.is_finite() || width <= 0.0 {
                    return false;
                }
                let start = Point {
                    x: position.x,
                    y: position.y + index as f64 * size * 1.2,
                };
                inside_box(
                    point,
                    start,
                    Point {
                        x: start.x + width,
                        y: start.y + size,
                    },
                    tolerance,
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_editor::{EditMode, EditorSession};

    fn object(id: &str, geometry: Geometry) -> DrawingObject {
        DrawingObject {
            id: id.into(),
            geometry,
            color: "#112233".into(),
            width: 4.0,
        }
    }

    #[test]
    fn selection_prefers_topmost_and_stays_in_current_mode() {
        let mut session =
            EditorSession::new(EditorDraft::new("thread".into(), None, 1024, 1024).unwrap())
                .unwrap();
        for id in ["bottom", "top"] {
            session
                .put(object(
                    id,
                    Geometry::Rectangle {
                        start: Point { x: 10.0, y: 10.0 },
                        end: Point { x: 50.0, y: 50.0 },
                    },
                ))
                .unwrap();
        }
        let viewport = Viewport::fit(1024, 1024, 1024.0, 1024.0).unwrap();
        let point = Point { x: 30.0, y: 30.0 };
        assert_eq!(
            session
                .draft()
                .object_at(&viewport, point, 4.0, |_, _| 0.0)
                .unwrap()
                .id,
            "top"
        );
        session.set_mode(EditMode::Mask);
        assert!(
            session
                .draft()
                .object_at(&viewport, point, 4.0, |_, _| 0.0)
                .is_none()
        );
    }

    #[test]
    fn stroke_tolerance_is_in_screen_pixels_at_different_scales() {
        let mut session =
            EditorSession::new(EditorDraft::new("thread".into(), None, 1024, 1024).unwrap())
                .unwrap();
        session
            .put(object(
                "line",
                Geometry::Stroke {
                    points: vec![Point { x: 10.0, y: 10.0 }, Point { x: 100.0, y: 10.0 }],
                    erase: false,
                },
            ))
            .unwrap();
        for edge in [256.0, 512.0, 1024.0] {
            let viewport = Viewport::fit(1024, 1024, edge, edge).unwrap();
            let mut point = viewport.to_screen(Point { x: 50.0, y: 10.0 });
            point.y += 3.0;
            assert!(
                session
                    .draft()
                    .object_at(&viewport, point, 4.0, |_, _| 0.0)
                    .is_some()
            );
            point.y += 10.0;
            assert!(
                session
                    .draft()
                    .object_at(&viewport, point, 4.0, |_, _| 0.0)
                    .is_none()
            );
        }
    }

    #[test]
    fn multiline_text_uses_measured_width_and_eraser_is_not_selectable() {
        let text = object(
            "text",
            Geometry::Text {
                position: Point { x: 0.0, y: 0.0 },
                text: "中文\nabc".into(),
            },
        );
        assert!(hits(&text, Point { x: 25.0, y: 20.0 }, 0.0, &mut |_, _| {
            30.0
        }));
        assert!(!hits(
            &text,
            Point { x: 40.0, y: 20.0 },
            0.0,
            &mut |_, _| 30.0
        ));
        let eraser = object(
            "eraser",
            Geometry::Stroke {
                points: vec![Point { x: 0.0, y: 0.0 }],
                erase: true,
            },
        );
        assert!(!hits(
            &eraser,
            Point { x: 0.0, y: 0.0 },
            5.0,
            &mut |_, _| 0.0
        ));
    }
}
