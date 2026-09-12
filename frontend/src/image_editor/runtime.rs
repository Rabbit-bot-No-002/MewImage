use std::{cell::Cell, collections::HashMap};

use mew_image_shared::{ImageEditingSnapshot, LocalAppState};
use rexie::TransactionMode;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

pub const RUNTIME_KEY: &str = "image_editor_runtime_v1";
thread_local! { static WRITE_REVISION: Cell<u64> = const { Cell::new(0) }; }

/// 浏览器运行时偏好，不参与云同步和项目包。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditorRuntime {
    pub editing_by_thread: HashMap<String, ImageEditingSnapshot>,
    pub thread_id: String,
    pub reference_ids: Vec<String>,
    pub continuation_id: Option<String>,
    #[serde(default)]
    pub continuation_task_id: Option<String>,
}

impl EditorRuntime {
    pub fn retain_existing_threads(&mut self, state: &LocalAppState) {
        self.editing_by_thread
            .retain(|id, _| state.threads.iter().any(|thread| &thread.id == id));
        if !state
            .threads
            .iter()
            .any(|thread| thread.id == self.thread_id)
        {
            self.thread_id = state
                .threads
                .first()
                .map(|thread| thread.id.clone())
                .unwrap_or_default();
            self.reference_ids.clear();
            self.continuation_id = None;
            self.continuation_task_id = None;
        }
        let continuation_is_valid = self
            .continuation_id
            .as_ref()
            .is_some_and(|asset_id| state.assets.iter().any(|asset| &asset.id == asset_id))
            && self
                .continuation_task_id
                .as_ref()
                .is_some_and(|task_id| state.tasks.iter().any(|task| &task.id == task_id));
        if (self.continuation_id.is_some() || self.continuation_task_id.is_some())
            && !continuation_is_valid
        {
            self.continuation_id = None;
            self.continuation_task_id = None;
        }
        // 缺失的编辑资源不静默移除，提交前由共享校验明确报错。
    }
}

pub async fn load_runtime() -> Result<EditorRuntime, String> {
    let db = crate::storage::open_db().await?;
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let value = transaction
        .store("kv")
        .map_err(|error| error.to_string())?
        .get(JsValue::from_str(RUNTIME_KEY))
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    match value {
        None => Ok(EditorRuntime::default()),
        Some(value) => serde_json::from_str(
            &value
                .as_string()
                .ok_or("编辑运行时记录类型异常，未重置。")?,
        )
        .map_err(|error| format!("编辑运行时记录损坏：{error}")),
    }
}

pub fn invalidate_runtime_writes() {
    WRITE_REVISION.set(WRITE_REVISION.get().wrapping_add(1));
}

/// 应用事务与普通选择保存共用修订号，避免旧异步写入覆盖已确认的编辑输入。
pub(crate) struct RuntimeWriteTicket(u64);

impl RuntimeWriteTicket {
    pub(crate) fn reserve() -> Self {
        invalidate_runtime_writes();
        Self(WRITE_REVISION.get())
    }

    pub(crate) fn is_current(&self) -> bool {
        WRITE_REVISION.get() == self.0
    }
}

pub fn save_runtime(
    runtime: EditorRuntime,
) -> impl std::future::Future<Output = Result<(), String>> {
    let ticket = RuntimeWriteTicket::reserve();
    async move {
        let value = serde_json::to_string(&runtime).map_err(|error| error.to_string())?;
        let db = crate::storage::open_db().await?;
        // 较早的打开数据库操作晚返回时，不得覆盖用户更新后的选择。
        if !ticket.is_current() {
            return Ok(());
        }
        let transaction = db
            .transaction(&["kv"], TransactionMode::ReadWrite)
            .map_err(|error| error.to_string())?;
        transaction
            .store("kv")
            .map_err(|error| error.to_string())?
            .put(
                &JsValue::from_str(&value),
                Some(&JsValue::from_str(RUNTIME_KEY)),
            )
            .await
            .map_err(|error| error.to_string())?;
        transaction
            .done()
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_and_normal_saves_invalidate_each_others_stale_tickets() {
        let old_application = RuntimeWriteTicket::reserve();
        assert!(old_application.is_current());
        let pending = save_runtime(EditorRuntime::default());
        assert!(!old_application.is_current());
        let latest_application = RuntimeWriteTicket::reserve();
        drop(pending);
        assert!(latest_application.is_current());
        invalidate_runtime_writes();
        assert!(!latest_application.is_current());
    }

    #[test]
    fn write_revision_is_reserved_before_future_execution_and_clear_invalidates_it() {
        let before = WRITE_REVISION.get();
        let pending = save_runtime(EditorRuntime::default());
        let reserved = WRITE_REVISION.get();
        assert_ne!(before, reserved);
        invalidate_runtime_writes();
        assert_ne!(reserved, WRITE_REVISION.get());
        drop(pending);
    }

    #[test]
    fn runtime_roundtrip_preserves_reference_order_and_editing_snapshot() {
        let state = LocalAppState::default();
        let thread_id = state.threads[0].id.clone();
        let editing = ImageEditingSnapshot {
            mode: mew_image_shared::ImageEditingMode::Mask,
            base_asset_id: "base".into(),
            mask_asset_id: Some("mask".into()),
            instruction: Some("仅修改选区".into()),
        };
        let runtime = EditorRuntime {
            thread_id: thread_id.clone(),
            reference_ids: vec!["base".into(), "second".into()],
            editing_by_thread: HashMap::from([(thread_id.clone(), editing.clone())]),
            continuation_id: None,
            continuation_task_id: None,
        };
        let mut restored: EditorRuntime =
            serde_json::from_str(&serde_json::to_string(&runtime).unwrap()).unwrap();
        restored.retain_existing_threads(&state);
        assert_eq!(restored.reference_ids, ["base", "second"]);
        assert_eq!(restored.editing_by_thread.get(&thread_id), Some(&editing));
        assert!(editing.validate_resources(&state.assets).is_err());
    }

    #[test]
    fn removed_thread_cannot_restore_old_reference_selection() {
        let state = LocalAppState::default();
        let mut runtime = EditorRuntime {
            thread_id: "deleted".into(),
            reference_ids: vec!["old".into()],
            continuation_id: Some("old".into()),
            ..Default::default()
        };
        runtime.retain_existing_threads(&state);
        assert_eq!(runtime.thread_id, state.threads[0].id);
        assert!(runtime.reference_ids.is_empty());
        assert!(runtime.continuation_id.is_none());
    }
}
