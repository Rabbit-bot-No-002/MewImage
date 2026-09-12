use super::{MAX_EDITOR_EDGE, Point};

/// 仅计算建议工作尺寸；调用方必须征得用户确认后才创建缩小副本。
pub fn fitted_work_dimensions(width: u32, height: u32) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("原图尺寸无效。".into());
    }
    let longest = width.max(height);
    if longest <= MAX_EDITOR_EDGE {
        return Ok((width, height));
    }
    let scale = f64::from(MAX_EDITOR_EDGE) / f64::from(longest);
    Ok((
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    ))
}

/// CSS 像素空间与工作图像坐标的变换；与设备像素比和预览降采样解耦。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    scale: f64,
    offset: Point,
    minimum_scale: f64,
    maximum_scale: f64,
}

fn finite_point(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

impl Viewport {
    pub fn fit(
        width: u32,
        height: u32,
        viewport_width: f64,
        viewport_height: f64,
    ) -> Result<Self, String> {
        if width == 0
            || height == 0
            || !viewport_width.is_finite()
            || !viewport_height.is_finite()
            || viewport_width <= 0.0
            || viewport_height <= 0.0
        {
            return Err("画布或视口尺寸无效。".into());
        }
        let scale = (viewport_width / f64::from(width))
            .min(viewport_height / f64::from(height))
            .min(1.0);
        Ok(Self {
            scale,
            offset: Point {
                x: (viewport_width - f64::from(width) * scale) / 2.0,
                y: (viewport_height - f64::from(height) * scale) / 2.0,
            },
            minimum_scale: scale / 8.0,
            maximum_scale: scale.max(1.0) * 16.0,
        })
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn to_image(&self, screen: Point) -> Point {
        Point {
            x: (screen.x - self.offset.x) / self.scale,
            y: (screen.y - self.offset.y) / self.scale,
        }
    }

    pub fn to_screen(&self, image: Point) -> Point {
        Point {
            x: image.x * self.scale + self.offset.x,
            y: image.y * self.scale + self.offset.y,
        }
    }

    /// 鼠标位置保持不动，避免滚轮缩放时画面跳向左上角。
    pub fn zoom_at(&mut self, anchor: Point, factor: f64) {
        if !finite_point(anchor) || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let image = self.to_image(anchor);
        let scale = (self.scale * factor).clamp(self.minimum_scale, self.maximum_scale);
        let offset = Point {
            x: anchor.x - image.x * scale,
            y: anchor.y - image.y * scale,
        };
        if !finite_point(offset) {
            return;
        }
        self.scale = scale;
        self.offset = offset;
    }

    pub fn pan(&mut self, delta: Point) {
        let offset = Point {
            x: self.offset.x + delta.x,
            y: self.offset.y + delta.y,
        };
        if finite_point(offset) {
            self.offset = offset;
        }
    }

    /// 双指移动的中点和平移量一起参与变换，不只改变缩放比例。
    pub fn pinch(&mut self, previous: [Point; 2], current: [Point; 2]) {
        if !previous.into_iter().chain(current).all(finite_point) {
            return;
        }
        let midpoint = |points: [Point; 2]| Point {
            x: points[0].x / 2.0 + points[1].x / 2.0,
            y: points[0].y / 2.0 + points[1].y / 2.0,
        };
        let distance =
            |points: [Point; 2]| (points[0].x - points[1].x).hypot(points[0].y - points[1].y);
        let previous_distance = distance(previous);
        let current_distance = distance(current);
        if previous_distance < 1.0
            || current_distance < 1.0
            || !previous_distance.is_finite()
            || !current_distance.is_finite()
        {
            return;
        }
        let previous_center = midpoint(previous);
        let current_center = midpoint(current);
        self.zoom_at(previous_center, current_distance / previous_distance);
        self.pan(Point {
            x: current_center.x - previous_center.x,
            y: current_center.y - previous_center.y,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_point(actual: Point, expected: Point) {
        assert!((actual.x - expected.x).abs() < 1e-8);
        assert!((actual.y - expected.y).abs() < 1e-8);
    }

    #[test]
    fn work_copy_preserves_aspect_and_never_enlarges() {
        assert_eq!(fitted_work_dimensions(8000, 4000).unwrap(), (4096, 2048));
        assert_eq!(fitted_work_dimensions(640, 480).unwrap(), (640, 480));
        assert_eq!(fitted_work_dimensions(1, u32::MAX).unwrap(), (1, 4096));
        assert!(fitted_work_dimensions(0, 100).is_err());
    }

    #[test]
    fn zoom_preserves_cursor_anchor_and_coordinate_round_trip() {
        let mut viewport = Viewport::fit(4096, 2048, 800.0, 600.0).unwrap();
        let anchor = Point { x: 300.0, y: 200.0 };
        let image = viewport.to_image(anchor);
        viewport.zoom_at(anchor, 2.0);
        assert_point(viewport.to_screen(image), anchor);
        viewport.pan(Point { x: 40.0, y: -20.0 });
        assert_point(viewport.to_image(viewport.to_screen(image)), image);
    }

    #[test]
    fn pinch_keeps_image_at_moving_midpoint() {
        let mut viewport = Viewport::fit(1024, 1024, 512.0, 512.0).unwrap();
        let image = viewport.to_image(Point { x: 100.0, y: 100.0 });
        viewport.pinch(
            [Point { x: 50.0, y: 100.0 }, Point { x: 150.0, y: 100.0 }],
            [Point { x: 20.0, y: 140.0 }, Point { x: 220.0, y: 140.0 }],
        );
        assert_eq!(viewport.scale(), 1.0);
        assert_point(viewport.to_screen(image), Point { x: 120.0, y: 140.0 });
    }

    #[test]
    fn invalid_gestures_do_not_poison_transform_and_zoom_is_bounded() {
        let mut viewport = Viewport::fit(1024, 1024, 512.0, 512.0).unwrap();
        let original = viewport;
        viewport.pan(Point {
            x: f64::NAN,
            y: 0.0,
        });
        viewport.zoom_at(Point { x: 0.0, y: 0.0 }, f64::INFINITY);
        assert_eq!(viewport, original);
        viewport.zoom_at(Point { x: 0.0, y: 0.0 }, 1e30);
        assert_eq!(viewport.scale(), 16.0);
        viewport.zoom_at(Point { x: 0.0, y: 0.0 }, 1e-30);
        assert_eq!(viewport.scale(), 0.0625);
        assert!(Viewport::fit(1024, 1024, f64::NAN, 512.0).is_err());
    }
}
