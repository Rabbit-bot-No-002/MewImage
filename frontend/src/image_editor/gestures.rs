use std::collections::BTreeMap;

use super::{Point, Viewport};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GestureAction {
    None,
    Begin(Point),
    Append(Point),
    Commit,
    Cancel,
}

#[derive(Default)]
pub(super) struct Gestures {
    pointers: BTreeMap<i32, Point>,
    drawing: bool,
    panning: bool,
}

impl Gestures {
    fn first_pair(&self) -> Option<[Point; 2]> {
        let mut points = self.pointers.values();
        Some([*points.next()?, *points.next()?])
    }

    pub fn begin(&mut self, id: i32, point: Point, pan: bool) -> GestureAction {
        if self.pointers.contains_key(&id) {
            return GestureAction::None;
        }
        self.pointers.insert(id, point);
        if self.pointers.len() > 1 {
            self.panning = true;
            return if std::mem::take(&mut self.drawing) {
                GestureAction::Cancel
            } else {
                GestureAction::None
            };
        }
        self.panning = pan;
        self.drawing = !pan;
        if pan {
            GestureAction::None
        } else {
            GestureAction::Begin(point)
        }
    }

    pub fn update(&mut self, id: i32, point: Point, viewport: &mut Viewport) -> GestureAction {
        let Some(previous) = self.pointers.get(&id).copied() else {
            return GestureAction::None;
        };
        if let Some(before) = self.first_pair() {
            self.pointers.insert(id, point);
            if let Some(after) = self.first_pair() {
                viewport.pinch(before, after);
            }
            return GestureAction::None;
        }
        self.pointers.insert(id, point);
        if self.panning {
            viewport.pan(Point {
                x: point.x - previous.x,
                y: point.y - previous.y,
            });
            GestureAction::None
        } else if self.drawing {
            GestureAction::Append(point)
        } else {
            GestureAction::None
        }
    }

    pub fn end(&mut self, id: i32, cancelled: bool) -> GestureAction {
        if self.pointers.remove(&id).is_none() {
            return GestureAction::None;
        }
        let drawing = std::mem::take(&mut self.drawing);
        if self.pointers.is_empty() {
            self.panning = false;
        }
        if !drawing {
            GestureAction::None
        } else if cancelled {
            GestureAction::Cancel
        } else {
            GestureAction::Commit
        }
    }

    pub fn active(&self) -> bool {
        !self.pointers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_finger_cancels_live_stroke_and_does_not_resume_after_pinch() {
        let mut gestures = Gestures::default();
        let point = Point { x: 10.0, y: 10.0 };
        assert_eq!(gestures.begin(1, point, false), GestureAction::Begin(point));
        assert_eq!(
            gestures.begin(2, Point { x: 100.0, y: 10.0 }, false),
            GestureAction::Cancel
        );
        assert_eq!(gestures.end(2, false), GestureAction::None);
        let mut viewport = Viewport::fit(1024, 1024, 512.0, 512.0).unwrap();
        assert_eq!(
            gestures.update(1, Point { x: 30.0, y: 20.0 }, &mut viewport),
            GestureAction::None
        );
        assert_eq!(gestures.end(1, false), GestureAction::None);
        assert!(!gestures.active());
    }

    #[test]
    fn cancelled_pointer_never_commits_and_unknown_pointer_is_ignored() {
        let mut gestures = Gestures::default();
        let point = Point { x: 10.0, y: 10.0 };
        gestures.begin(1, point, false);
        assert_eq!(gestures.end(9, false), GestureAction::None);
        assert_eq!(gestures.end(1, true), GestureAction::Cancel);
        assert_eq!(gestures.end(1, false), GestureAction::None);
    }

    #[test]
    fn pan_updates_only_viewport_and_pen_commits_once() {
        let mut gestures = Gestures::default();
        let mut viewport = Viewport::fit(1024, 1024, 512.0, 512.0).unwrap();
        let point = Point { x: 10.0, y: 10.0 };
        assert_eq!(gestures.begin(1, point, true), GestureAction::None);
        gestures.update(1, Point { x: 30.0, y: 40.0 }, &mut viewport);
        assert_eq!(viewport.offset(), Point { x: 20.0, y: 30.0 });
        assert_eq!(gestures.end(1, false), GestureAction::None);
        gestures.begin(2, point, false);
        assert_eq!(gestures.end(2, false), GestureAction::Commit);
        assert_eq!(gestures.end(2, false), GestureAction::None);
    }
}
