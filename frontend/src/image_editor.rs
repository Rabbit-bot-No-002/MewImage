//! 本地编辑草稿与增量撤销记录；不进入同步和模板包。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

mod base;
mod canvas;
mod export;
mod gestures;
mod operations;
mod persistence;
mod renderer;
pub(crate) mod runtime;
mod selection;
mod tools;
mod viewport;
pub use base::{EditorBase, EditorMask};
pub use canvas::{ImageEditorCanvas, ImageEditorCanvasProps};
pub use export::{EncodedEdit, encode_edit};
pub use persistence::{
    clear_drafts, delete_draft, draft_asset_ids, draft_references_asset,
    invalidate_pending_draft_writes, load_draft, save_draft,
};
pub use renderer::RenderedCanvas;
pub use tools::DrawTool;
pub use viewport::{Viewport, fitted_work_dimensions};

pub const MAX_HISTORY_STEPS: usize = 100;
pub const MAX_DRAFT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EDITOR_EDGE: u32 = 4096;

#[derive(Default)]
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// 预算检查只计数，不为每次笔画再分配一份完整 JSON。
fn serialized_bytes(value: &impl Serialize) -> Result<usize, String> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_err(|error| error.to_string())?;
    Ok(counter.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditMode {
    #[default]
    Mask,
    Annotation,
    Sketch,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    fn valid(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.x.abs() <= 4096.0 && self.y.abs() <= 4096.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    Stroke { points: Vec<Point>, erase: bool },
    Arrow { start: Point, end: Point },
    Rectangle { start: Point, end: Point },
    Ellipse { start: Point, end: Point },
    Text { position: Point, text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingObject {
    pub id: String,
    pub geometry: Geometry,
    pub color: String,
    pub width: f64,
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

impl DrawingObject {
    fn validate(&self) -> Result<(), String> {
        let geometry_valid = match &self.geometry {
            Geometry::Stroke { points, .. } => {
                !points.is_empty() && points.iter().all(|point| point.valid())
            }
            Geometry::Arrow { start, end }
            | Geometry::Rectangle { start, end }
            | Geometry::Ellipse { start, end } => start.valid() && end.valid(),
            Geometry::Text { position, text } => {
                position.valid() && !text.is_empty() && text.chars().count() <= 2048
            }
        };
        if self.id.is_empty()
            || self.id.len() > 128
            || !valid_color(&self.color)
            || !self.width.is_finite()
            || !(1.0..=256.0).contains(&self.width)
            || !geometry_valid
        {
            return Err("绘图对象的坐标、颜色、文字或笔刷大小无效。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingLayer {
    pub mode: EditMode,
    pub objects: Vec<DrawingObject>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorDraft {
    pub version: u32,
    pub thread_id: String,
    pub base_asset_id: Option<String>,
    /// 仅在用户确认缩小后记录原始尺寸；工作坐标始终使用 width/height。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_base_dimensions: Option<(u32, u32)>,
    /// 上传遮罩只属于局部修改图层，不占普通参考图名额。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_mask_asset_id: Option<String>,
    pub width: u32,
    pub height: u32,
    pub mode: EditMode,
    pub background: String,
    pub layers: Vec<DrawingLayer>,
}

impl EditorDraft {
    pub fn new(
        thread_id: String,
        base_asset_id: Option<String>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let draft = Self {
            version: 1,
            thread_id,
            base_asset_id,
            original_base_dimensions: None,
            imported_mask_asset_id: None,
            width,
            height,
            mode: EditMode::Sketch,
            background: "#ffffff".into(),
            layers: [EditMode::Mask, EditMode::Annotation, EditMode::Sketch]
                .into_iter()
                .map(|mode| DrawingLayer {
                    mode,
                    objects: Vec::new(),
                })
                .collect(),
        };
        draft.validate()?;
        Ok(draft)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.thread_id.is_empty()
            || self.thread_id.len() > 128
            || self
                .base_asset_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
            || self.width == 0
            || self.height == 0
            || self.width.max(self.height) > MAX_EDITOR_EDGE
            || !valid_color(&self.background)
            || self.layers.len() != 3
        {
            return Err("编辑草稿版本、归属、尺寸或背景色无效。".into());
        }
        if let Some((width, height)) = self.original_base_dimensions
            && (self.base_asset_id.is_none()
                || width.max(height) <= MAX_EDITOR_EDGE
                || fitted_work_dimensions(width, height)? != (self.width, self.height))
        {
            return Err("缩小工作副本与原始尺寸不一致，请重新确认底图。".into());
        }
        if let Some(id) = &self.imported_mask_asset_id
            && (id.is_empty()
                || id.len() > 128
                || self.base_asset_id.is_none()
                || self.base_asset_id.as_ref() == Some(id))
        {
            return Err("导入遮罩必须引用独立于底图的有效资源。".into());
        }
        for mode in [EditMode::Mask, EditMode::Annotation, EditMode::Sketch] {
            let layers: Vec<_> = self
                .layers
                .iter()
                .filter(|layer| layer.mode == mode)
                .collect();
            if layers.len() != 1 {
                return Err("编辑草稿图层缺失或重复。".into());
            }
            let mut ids = std::collections::HashSet::new();
            for object in &layers[0].objects {
                object.validate()?;
                if !ids.insert(&object.id) {
                    return Err("编辑对象 ID 重复。".into());
                }
                if mode == EditMode::Mask && !matches!(object.geometry, Geometry::Stroke { .. }) {
                    return Err("遮罩图层只能包含选区画笔和橡皮。".into());
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<String, String> {
        self.validate_size()?;
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    fn validate_size(&self) -> Result<usize, String> {
        self.validate()?;
        let bytes = serialized_bytes(self)?;
        if bytes > MAX_DRAFT_BYTES {
            return Err("草稿操作数据超过 8 MiB，请删除部分笔画。".into());
        }
        Ok(bytes)
    }

    pub fn decode(value: &str, thread_id: &str) -> Result<Self, String> {
        if value.len() > MAX_DRAFT_BYTES {
            return Err("草稿操作数据超过 8 MiB。".into());
        }
        let draft: Self =
            serde_json::from_str(value).map_err(|error| format!("编辑草稿无法读取：{error}"))?;
        draft.validate()?;
        if draft.thread_id != thread_id {
            return Err("编辑草稿不属于当前会话。".into());
        }
        Ok(draft)
    }

    pub fn active_objects(&self) -> &[DrawingObject] {
        self.layers
            .iter()
            .find(|layer| layer.mode == self.mode)
            .map(|layer| layer.objects.as_slice())
            .unwrap_or_default()
    }

    pub fn input_asset_ids(&self) -> impl Iterator<Item = &String> {
        self.base_asset_id
            .iter()
            .chain(self.imported_mask_asset_id.iter())
    }
}

#[derive(Clone, Serialize)]
struct Change {
    mode: EditMode,
    index: usize,
    before: Option<DrawingObject>,
    after: Option<DrawingObject>,
    cleared: Option<Vec<DrawingObject>>,
    background: Option<(String, String)>,
    mask: Option<(Option<String>, Option<String>)>,
}

pub struct EditorSession {
    draft: EditorDraft,
    undo: VecDeque<Change>,
    redo: Vec<Change>,
}

impl EditorSession {
    pub fn new(draft: EditorDraft) -> Result<Self, String> {
        draft.validate_size()?;
        Ok(Self {
            draft,
            undo: VecDeque::new(),
            redo: Vec::new(),
        })
    }

    pub fn draft(&self) -> &EditorDraft {
        &self.draft
    }

    pub fn set_mode(&mut self, mode: EditMode) {
        self.draft.mode = mode;
    }

    /// 只记录变更对象，不为每一步复制全画布或全部笔画。
    pub fn put(&mut self, object: DrawingObject) -> Result<(), String> {
        object.validate()?;
        let objects = self.draft.active_objects();
        let index = objects
            .iter()
            .position(|item| item.id == object.id)
            .unwrap_or(objects.len());
        let change = Change {
            mode: self.draft.mode,
            index,
            before: objects.get(index).cloned(),
            after: Some(object),
            cleared: None,
            background: None,
            mask: None,
        };
        self.apply(&change, true);
        if let Err(error) = self.draft.validate_size() {
            self.apply(&change, false);
            return Err(error);
        }
        self.record(change);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let objects = self.draft.active_objects();
        let Some(index) = objects.iter().position(|item| item.id == id) else {
            return false;
        };
        let change = Change {
            mode: self.draft.mode,
            index,
            before: Some(objects[index].clone()),
            after: None,
            cleared: None,
            background: None,
            mask: None,
        };
        self.apply(&change, true);
        self.record(change);
        true
    }

    fn record(&mut self, change: Change) {
        self.redo.clear();
        self.undo.push_back(change);
        while self.undo.len() > MAX_HISTORY_STEPS {
            self.undo.pop_front();
        }
        self.trim_history();
    }

    fn trim_history(&mut self) {
        // 草稿与撤销共享上限；丢弃最旧撤销记录不影响当前画布。
        let draft_bytes = serialized_bytes(&self.draft).unwrap_or(MAX_DRAFT_BYTES);
        while self.history_bytes().saturating_add(draft_bytes) > MAX_DRAFT_BYTES {
            if self.undo.pop_front().is_some() {
                continue;
            }
            if self.redo.is_empty() {
                break;
            }
            self.redo.remove(0);
        }
    }

    fn history_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .map(|change| serialized_bytes(change).unwrap_or(MAX_DRAFT_BYTES))
            .sum()
    }

    fn apply(&mut self, change: &Change, forward: bool) {
        if let Some((before, after)) = &change.mask {
            self.draft
                .imported_mask_asset_id
                .clone_from(if forward { after } else { before });
        }
        if let Some((before, after)) = &change.background {
            self.draft
                .background
                .clone_from(if forward { after } else { before });
            return;
        }
        let layer = self
            .draft
            .layers
            .iter_mut()
            .find(|layer| layer.mode == change.mode)
            .expect("validated layer");
        if let Some(objects) = &change.cleared {
            if forward {
                layer.objects.clear();
            } else {
                layer.objects.clone_from(objects);
            }
            return;
        }
        let (previous, next) = if forward {
            (&change.before, &change.after)
        } else {
            (&change.after, &change.before)
        };
        if previous.is_some() {
            layer.objects.remove(change.index);
        }
        if let Some(object) = next {
            layer.objects.insert(change.index, object.clone());
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(change) = self.undo.pop_back() else {
            return false;
        };
        self.apply(&change, false);
        self.redo.push(change);
        self.trim_history();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(change) = self.redo.pop() else {
            return false;
        };
        self.apply(&change, true);
        self.undo.push_back(change);
        self.trim_history();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(id: &str) -> DrawingObject {
        DrawingObject {
            id: id.into(),
            geometry: Geometry::Stroke {
                points: vec![Point { x: 1.0, y: 2.0 }],
                erase: false,
            },
            color: "#112233".into(),
            width: 8.0,
        }
    }

    fn session() -> EditorSession {
        EditorSession::new(EditorDraft::new("thread".into(), None, 1024, 1024).unwrap()).unwrap()
    }

    #[test]
    fn replacement_and_deletion_preserve_order_through_undo() {
        let mut editor = session();
        editor.put(stroke("a")).unwrap();
        editor.put(stroke("b")).unwrap();
        let mut changed = stroke("a");
        changed.width = 20.0;
        editor.put(changed).unwrap();
        assert!(editor.remove("a"));
        assert!(editor.undo());
        assert_eq!(editor.draft.active_objects()[0].width, 20.0);
        assert!(editor.undo());
        assert_eq!(editor.draft.active_objects()[0].width, 8.0);
        assert!(editor.redo());
        assert_eq!(editor.draft.active_objects()[1].id, "b");
    }

    #[test]
    fn resized_draft_roundtrip_requires_consistent_original_dimensions() {
        let mut draft =
            EditorDraft::new("thread".into(), Some("original".into()), 4096, 2048).unwrap();
        draft.original_base_dimensions = Some((8192, 4096));
        assert_eq!(
            EditorDraft::decode(&draft.encode().unwrap(), "thread").unwrap(),
            draft
        );
        for source in [(0, 4096), (4096, 2048), (8192, 8192)] {
            let mut changed = draft.clone();
            changed.original_base_dimensions = Some(source);
            assert!(changed.validate().is_err());
        }
        draft.base_asset_id = None;
        assert!(draft.validate().is_err());
    }

    #[test]
    fn legacy_draft_without_resize_metadata_is_preserved() {
        let draft = EditorDraft::new("thread".into(), Some("base".into()), 128, 64).unwrap();
        let json = draft.encode().unwrap();
        assert!(!json.contains("original_base_dimensions"));
        assert_eq!(EditorDraft::decode(&json, "thread").unwrap(), draft);
    }

    #[test]
    fn imported_mask_is_backward_compatible_and_requires_an_independent_base() {
        let mut draft = EditorDraft::new("thread".into(), Some("base".into()), 128, 64).unwrap();
        assert!(!draft.encode().unwrap().contains("imported_mask_asset_id"));
        draft.imported_mask_asset_id = Some("editor-mask-a".into());
        assert_eq!(
            EditorDraft::decode(&draft.encode().unwrap(), "thread").unwrap(),
            draft
        );
        for invalid in ["", "base", &"x".repeat(129)] {
            let mut changed = draft.clone();
            changed.imported_mask_asset_id = Some(invalid.into());
            assert!(changed.validate().is_err());
        }
    }

    #[test]
    fn modes_are_isolated_and_draft_round_trip_preserves_objects() {
        let mut editor = session();
        editor.put(stroke("sketch")).unwrap();
        editor.set_mode(EditMode::Mask);
        assert!(editor.draft.active_objects().is_empty());
        editor.put(stroke("mask")).unwrap();
        let draft = EditorDraft::decode(&editor.draft.encode().unwrap(), "thread").unwrap();
        assert_eq!(draft.active_objects()[0].id, "mask");
        assert!(EditorDraft::decode(&draft.encode().unwrap(), "other-thread").is_err());
        assert!(editor.undo());
        editor.set_mode(EditMode::Sketch);
        assert_eq!(editor.draft.active_objects()[0].id, "sketch");
    }

    #[test]
    fn invalid_edits_do_not_replace_current_draft() {
        let mut editor = session();
        editor.put(stroke("valid")).unwrap();
        let before = editor.draft.clone();
        let mut invalid = stroke("valid");
        invalid.width = f64::NAN;
        assert!(editor.put(invalid).is_err());
        assert_eq!(before, editor.draft);
        assert!(EditorDraft::new("thread".into(), None, 4097, 1024).is_err());
        assert!(EditorDraft::decode(&" ".repeat(MAX_DRAFT_BYTES + 1), "thread").is_err());
    }

    #[test]
    fn history_is_bounded_and_new_edits_clear_redo() {
        let mut editor = session();
        for index in 0..105 {
            editor.put(stroke(&index.to_string())).unwrap();
        }
        assert_eq!(editor.undo.len(), 100);
        assert!(editor.undo());
        editor.put(stroke("new")).unwrap();
        assert!(!editor.redo());
    }

    #[test]
    fn byte_count_matches_utf8_serialization() {
        let mut editor = session();
        let mut object = stroke("中文对象");
        object.geometry = Geometry::Text {
            position: Point { x: 10.0, y: 20.0 },
            text: "中文与 emoji 🐱".into(),
        };
        editor.put(object).unwrap();
        assert_eq!(
            serialized_bytes(editor.draft()).unwrap(),
            editor.draft().encode().unwrap().len()
        );
    }

    #[test]
    fn large_operations_share_budget_with_history() {
        let mut editor = session();
        let mut object = stroke("large");
        object.geometry = Geometry::Stroke {
            points: vec![Point { x: 1.0, y: 2.0 }; 140_000],
            erase: false,
        };
        editor.put(object.clone()).unwrap();
        object.width = 10.0;
        editor.put(object).unwrap();
        assert!(
            serialized_bytes(editor.draft()).unwrap() + editor.history_bytes() <= MAX_DRAFT_BYTES
        );
        assert!(editor.remove("large"));
        assert!(editor.undo());
        assert!(
            serialized_bytes(editor.draft()).unwrap() + editor.history_bytes() <= MAX_DRAFT_BYTES
        );
    }
}
