use super::{Geometry, Point};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawTool {
    #[default]
    Pen,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Select,
}

impl DrawTool {
    pub(crate) fn geometry(self, points: Vec<Point>, erase: bool) -> Option<Geometry> {
        let start = *points.first()?;
        let end = *points.last()?;
        match self {
            Self::Pen => Some(Geometry::Stroke { points, erase }),
            Self::Arrow => Some(Geometry::Arrow { start, end }),
            Self::Rectangle => Some(Geometry::Rectangle { start, end }),
            Self::Ellipse => Some(Geometry::Ellipse { start, end }),
            Self::Text | Self::Select => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "画笔",
            Self::Arrow => "箭头",
            Self::Rectangle => "矩形",
            Self::Ellipse => "椭圆",
            Self::Text => "文字",
            Self::Select => "选择/移动",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_keeps_reverse_drag_coordinates_and_ignores_intermediate_samples() {
        let start = Point { x: 100.0, y: 200.0 };
        let end = Point { x: 10.0, y: 20.0 };
        assert_eq!(
            DrawTool::Rectangle.geometry(vec![start, Point { x: 50.0, y: 80.0 }, end], false),
            Some(Geometry::Rectangle { start, end })
        );
        assert_eq!(
            DrawTool::Ellipse.geometry(vec![start, end], false),
            Some(Geometry::Ellipse { start, end })
        );
        assert_eq!(
            DrawTool::Arrow.geometry(vec![start, end], false),
            Some(Geometry::Arrow { start, end })
        );
    }

    #[test]
    fn pen_preserves_path_and_non_drawing_tools_emit_no_geometry() {
        let points = vec![Point { x: 1.0, y: 2.0 }, Point { x: 3.0, y: 4.0 }];
        assert_eq!(
            DrawTool::Pen.geometry(points.clone(), true),
            Some(Geometry::Stroke {
                points: points.clone(),
                erase: true
            })
        );
        assert_eq!(DrawTool::Text.geometry(points.clone(), false), None);
        assert_eq!(DrawTool::Select.geometry(points, false), None);
        assert_eq!(DrawTool::Arrow.geometry(Vec::new(), false), None);
    }
}
