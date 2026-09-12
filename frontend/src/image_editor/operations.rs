use super::{Change, EditorSession, Geometry, Point};

impl EditorSession {
    /// 背景是草图底层颜色，只记录新旧颜色，不复制对象或像素。
    pub fn set_background(&mut self, color: String) -> Result<bool, String> {
        if !super::valid_color(&color) {
            return Err("背景颜色必须为 #RRGGBB。".into());
        }
        if self.draft.background.eq_ignore_ascii_case(&color) {
            return Ok(false);
        }
        let change = Change {
            mode: self.draft.mode,
            index: 0,
            before: None,
            after: None,
            cleared: None,
            background: Some((self.draft.background.clone(), color)),
            mask: None,
        };
        self.apply(&change, true);
        self.record(change);
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 拖动结束时提交一次位移；指针移动中的预览不写入撤销历史。
    pub fn move_object(&mut self, id: &str, delta: Point) -> Result<(), String> {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return Err("对象位移无效。".into());
        }
        let mut object = self
            .draft
            .active_objects()
            .iter()
            .find(|object| object.id == id)
            .cloned()
            .ok_or("当前图层找不到该对象。")?;
        if delta.x == 0.0 && delta.y == 0.0 {
            return Ok(());
        }
        let translate = |point: &mut Point| {
            point.x += delta.x;
            point.y += delta.y;
        };
        match &mut object.geometry {
            Geometry::Stroke { points, .. } => points.iter_mut().for_each(translate),
            Geometry::Arrow { start, end }
            | Geometry::Rectangle { start, end }
            | Geometry::Ellipse { start, end } => {
                translate(start);
                translate(end);
            }
            Geometry::Text { position, .. } => translate(position),
        }
        self.put(object)
    }

    /// 组字结束且用户确认后调用，保留原文字对象的位置和样式。
    pub fn edit_text(&mut self, id: &str, text: String) -> Result<(), String> {
        let mut object = self
            .draft
            .active_objects()
            .iter()
            .find(|object| object.id == id)
            .cloned()
            .ok_or("当前图层找不到该文字。")?;
        let Geometry::Text { text: current, .. } = &mut object.geometry else {
            return Err("所选对象不是文字。".into());
        };
        if *current == text {
            return Ok(());
        }
        *current = text;
        self.put(object)
    }

    /// 清空仅影响当前模式，作为一个操作撤销；不逐个克隆全部对象。
    pub fn clear_active_layer(&mut self) -> bool {
        let mask = (self.draft.mode == super::EditMode::Mask)
            .then(|| self.draft.imported_mask_asset_id.clone())
            .flatten();
        let layer = self
            .draft
            .layers
            .iter_mut()
            .find(|layer| layer.mode == self.draft.mode)
            .expect("validated layer");
        if layer.objects.is_empty() && mask.is_none() {
            return false;
        }
        let objects = std::mem::take(&mut layer.objects);
        if mask.is_some() {
            self.draft.imported_mask_asset_id = None;
        }
        self.record(Change {
            mode: self.draft.mode,
            index: 0,
            before: None,
            after: None,
            cleared: Some(objects),
            background: None,
            mask: mask.map(|id| (Some(id), None)),
        });
        true
    }

    /// 替换导入遮罩和已有选区作为一个操作撤销，不把图片字节放入历史。
    pub fn replace_imported_mask(&mut self, asset_id: String) -> Result<bool, String> {
        if self.draft.mode != super::EditMode::Mask {
            return Err("请切换到局部修改模式后再导入遮罩。".into());
        }
        if self.draft.imported_mask_asset_id.as_ref() == Some(&asset_id) {
            return Ok(false);
        }
        let change = Change {
            mode: self.draft.mode,
            index: 0,
            before: None,
            after: None,
            cleared: Some(self.draft.active_objects().to_vec()),
            background: None,
            mask: Some((self.draft.imported_mask_asset_id.clone(), Some(asset_id))),
        };
        self.apply(&change, true);
        if let Err(error) = self.draft.validate_size() {
            self.apply(&change, false);
            return Err(error);
        }
        self.record(change);
        Ok(true)
    }

    /// 资源回收需要同时保护仍可撤销恢复的遮罩，而不只检查当前图层。
    pub fn referenced_asset_ids(&self) -> std::collections::HashSet<&String> {
        self.draft
            .input_asset_ids()
            .chain(self.undo.iter().chain(&self.redo).flat_map(|change| {
                change
                    .mask
                    .iter()
                    .flat_map(|(before, after)| before.iter().chain(after.iter()))
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_editor::{DrawingObject, EditMode, EditorDraft};

    fn session() -> EditorSession {
        let mut editor =
            EditorSession::new(EditorDraft::new("thread".into(), None, 1024, 1024).unwrap())
                .unwrap();
        editor
            .put(DrawingObject {
                id: "text".into(),
                geometry: Geometry::Text {
                    position: Point { x: 20.0, y: 30.0 },
                    text: "原文字".into(),
                },
                width: 20.0,
                color: "#123456".into(),
            })
            .unwrap();
        editor
    }

    #[test]
    fn move_and_text_edit_are_undoable_and_invalid_edits_are_atomic() {
        let mut editor = session();
        let original = editor.draft().clone();
        assert!(
            editor
                .move_object("text", Point { x: 5000.0, y: 0.0 })
                .is_err()
        );
        assert_eq!(editor.draft(), &original);
        editor
            .move_object("text", Point { x: 100.0, y: 50.0 })
            .unwrap();
        editor.edit_text("text", "中文更新\n第二行".into()).unwrap();
        assert!(editor.can_undo());
        editor.undo();
        editor.undo();
        assert_eq!(editor.draft(), &original);
        assert!(editor.can_redo());
        editor.redo();
        editor.redo();
        assert!(matches!(&editor.draft().active_objects()[0].geometry,
            Geometry::Text { position, text } if position.x == 120.0 && text == "中文更新\n第二行"));
    }

    #[test]
    fn clear_is_one_step_and_does_not_clear_other_modes() {
        let mut editor = session();
        let original = editor.draft().clone();
        editor.set_mode(EditMode::Mask);
        assert!(!editor.clear_active_layer());
        editor.set_mode(EditMode::Sketch);
        assert!(editor.clear_active_layer());
        assert!(editor.draft().active_objects().is_empty());
        editor.undo();
        assert_eq!(editor.draft(), &original);
        editor.redo();
        assert!(editor.draft().active_objects().is_empty());
    }

    #[test]
    fn background_changes_preserve_objects_and_round_trip_through_history() {
        let mut editor = session();
        let original = editor.draft().clone();
        assert!(!editor.set_background("#FFFFFF".into()).unwrap());
        assert!(editor.set_background("red".into()).is_err());
        assert_eq!(editor.draft(), &original);
        assert!(editor.set_background("#123456".into()).unwrap());
        assert_eq!(editor.draft().layers, original.layers);
        assert_eq!(editor.draft().background, "#123456");
        assert!(editor.undo());
        assert_eq!(editor.draft(), &original);
        assert!(editor.redo());
        let restored = EditorDraft::decode(&editor.draft().encode().unwrap(), "thread").unwrap();
        assert_eq!(restored.background, "#123456");
    }

    #[test]
    fn imported_mask_replacement_clear_and_undo_preserve_resource_references() {
        let mut draft = EditorDraft::new("thread".into(), Some("base".into()), 1024, 1024).unwrap();
        draft.mode = EditMode::Mask;
        let mut editor = EditorSession::new(draft).unwrap();
        assert!(
            editor
                .replace_imported_mask("editor-mask-a".into())
                .unwrap()
        );
        assert_eq!(
            editor.draft().imported_mask_asset_id.as_deref(),
            Some("editor-mask-a")
        );
        assert!(
            editor
                .replace_imported_mask("editor-mask-b".into())
                .unwrap()
        );
        assert!(
            editor
                .referenced_asset_ids()
                .contains(&"editor-mask-a".to_string())
        );
        assert!(editor.undo());
        assert_eq!(
            editor.draft().imported_mask_asset_id.as_deref(),
            Some("editor-mask-a")
        );
        assert!(editor.clear_active_layer());
        assert!(editor.draft().imported_mask_asset_id.is_none());
        assert!(editor.undo());
        assert_eq!(
            editor.draft().imported_mask_asset_id.as_deref(),
            Some("editor-mask-a")
        );
    }

    #[test]
    fn imported_mask_is_only_available_in_mask_mode_and_deduplicates() {
        let mut editor = session();
        assert!(
            editor
                .replace_imported_mask("editor-mask-a".into())
                .is_err()
        );
        editor.draft.base_asset_id = Some("base".into());
        editor.set_mode(EditMode::Mask);
        assert!(
            editor
                .replace_imported_mask("editor-mask-a".into())
                .unwrap()
        );
        assert!(
            !editor
                .replace_imported_mask("editor-mask-a".into())
                .unwrap()
        );
    }
}
