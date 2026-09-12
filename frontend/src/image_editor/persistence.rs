use rexie::TransactionMode;
use wasm_bindgen::JsValue;

use super::EditorDraft;

thread_local! {
    static WRITE_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// 删除前同步作废尚在等待打开数据库的旧保存操作。
pub fn invalidate_pending_draft_writes() {
    WRITE_EPOCH.set(WRITE_EPOCH.get().wrapping_add(1));
}

fn draft_key(thread_id: &str) -> Result<JsValue, String> {
    if thread_id.is_empty() || thread_id.len() > 128 {
        return Err("编辑草稿会话 ID 无效。".into());
    }
    Ok(JsValue::from_str(&format!(
        "image_editor_draft:{thread_id}"
    )))
}

/// 草稿写入独立 KV，不混入同步快照；事务提交前不报告保存成功。
pub async fn save_draft(draft: &EditorDraft) -> Result<(), String> {
    let epoch = WRITE_EPOCH.get();
    let value = draft.encode()?;
    let key = draft_key(&draft.thread_id)?;
    let db = crate::storage::open_db().await?;
    if WRITE_EPOCH.get() != epoch {
        return Err("编辑草稿保存已因数据清理而取消。".into());
    }
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    transaction
        .store("kv")
        .map_err(|error| error.to_string())?
        .put(&JsValue::from_str(&value), Some(&key))
        .await
        .map_err(|error| format!("编辑草稿保存失败，请检查浏览器存储配额：{error}"))?;
    transaction
        .done()
        .await
        .map_err(|error| format!("编辑草稿保存未完成：{error}"))?;
    Ok(())
}

pub async fn load_draft(thread_id: &str) -> Result<Option<EditorDraft>, String> {
    let key = draft_key(thread_id)?;
    let db = crate::storage::open_db().await?;
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let value = transaction
        .store("kv")
        .map_err(|error| error.to_string())?
        .get(key)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    match value {
        None => Ok(None),
        Some(value) => {
            let serialized = value
                .as_string()
                .ok_or("编辑草稿类型异常，原记录保留，未重置。")?;
            EditorDraft::decode(&serialized, thread_id).map(Some)
        }
    }
}

/// 删除原图前检查所有本地草稿；读取失败不能当作“没有引用”。
pub async fn draft_references_asset(asset_id: &str) -> Result<bool, String> {
    Ok(draft_asset_ids(None).await?.contains(asset_id))
}

/// 批量删除只扫描一次草稿；删除会话时可排除该会话自身的草稿。
pub async fn draft_asset_ids(
    excluded_thread: Option<&str>,
) -> Result<std::collections::HashSet<String>, String> {
    let db = crate::storage::open_db().await?;
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let store = transaction.store("kv").map_err(|error| error.to_string())?;
    let mut referenced = std::collections::HashSet::new();
    for key in store
        .get_all_keys(None, None)
        .await
        .map_err(|error| error.to_string())?
    {
        let Some(name) = key.as_string() else {
            continue;
        };
        let Some(thread_id) = name.strip_prefix("image_editor_draft:") else {
            continue;
        };
        if Some(thread_id) == excluded_thread {
            continue;
        }
        let value = store
            .get(key)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("编辑草稿在检查时丢失，请重试。")?;
        let serialized = value.as_string().ok_or("编辑草稿类型异常，已停止删除。")?;
        let draft = EditorDraft::decode(&serialized, thread_id)?;
        referenced.extend(draft.input_asset_ids().cloned());
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(referenced)
}

pub async fn delete_draft(thread_id: &str) -> Result<(), String> {
    let key = draft_key(thread_id)?;
    let draft = load_draft(thread_id).await?;
    let db = crate::storage::open_db().await?;
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    transaction
        .store("kv")
        .map_err(|error| error.to_string())?
        .delete(key)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    if let Some(mask_id) = draft.and_then(|draft| draft.imported_mask_asset_id) {
        crate::storage::delete_editor_draft_blobs(&[mask_id]).await?;
    }
    Ok(())
}

pub async fn clear_drafts() -> Result<(), String> {
    let db = crate::storage::open_db().await?;
    let transaction = db
        .transaction(&["kv"], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    let store = transaction.store("kv").map_err(|error| error.to_string())?;
    let mut mask_ids = Vec::new();
    let keys = store
        .get_all_keys(None, None)
        .await
        .map_err(|error| error.to_string())?;
    for key in keys {
        if let Some(name) = key.as_string().filter(|key| is_draft_key(key)) {
            if let Some(value) = store
                .get(key.clone())
                .await
                .map_err(|error| error.to_string())?
            {
                let serialized = value.as_string().ok_or("编辑草稿类型异常，已停止清理。")?;
                let thread_id = name.strip_prefix("image_editor_draft:").unwrap_or_default();
                mask_ids
                    .extend(EditorDraft::decode(&serialized, thread_id)?.imported_mask_asset_id);
            }
            store.delete(key).await.map_err(|error| error.to_string())?;
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    crate::storage::delete_editor_draft_blobs(&mask_ids).await?;
    Ok(())
}

fn is_draft_key(key: &str) -> bool {
    key.starts_with("image_editor_draft:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_does_not_target_other_local_records() {
        assert!(is_draft_key("image_editor_draft:thread-1"));
        for key in [
            "workspace",
            "settings",
            "image_editor_draft_backup",
            "generation_staging:task",
        ] {
            assert!(!is_draft_key(key));
        }
    }

    #[test]
    fn cleanup_invalidates_previous_write_epoch() {
        let before = WRITE_EPOCH.get();
        invalidate_pending_draft_writes();
        assert_ne!(before, WRITE_EPOCH.get());
    }
}
