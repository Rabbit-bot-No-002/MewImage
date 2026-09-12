use super::*;
use crate::image_editor::runtime::{EditorRuntime, RUNTIME_KEY, RuntimeWriteTicket};

/// 已持久化的 Blob、正式索引及当前编辑参数在一次事务中确认；失败不应用半份输入。
pub(crate) async fn commit_editor_application(
    staging_key: &str,
    snapshot: &LocalAppState,
    runtime: &EditorRuntime,
    is_current: impl Fn() -> bool,
) -> Result<(), String> {
    if !staging_key.starts_with(EDITOR_STAGING_KEY_PREFIX) {
        return Err("编辑暂存键无效。".into());
    }
    let snapshot_value = serde_wasm_bindgen::to_value(snapshot).map_err(|e| e.to_string())?;
    let runtime_value = serde_json::to_string(runtime).map_err(|e| e.to_string())?;
    let ticket = RuntimeWriteTicket::reserve();
    let db = open_db().await?;
    let valid = || ticket.is_current() && is_current();
    if !valid() {
        return Err("编辑输入已变化或应用已取消，未覆盖当前工作台。".into());
    }
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|e| e.to_string())?;
    let write = async {
        let store = transaction.store(STORE_NAME).map_err(|e| e.to_string())?;
        let value = store
            .get(JsValue::from_str(staging_key))
            .await
            .map_err(|e| e.to_string())?
            .and_then(|value| value.as_string())
            .ok_or("编辑暂存清单已移除，未应用输入。")?;
        let staged: EditorAssetStaging = serde_json::from_str(&value).map_err(|e| e.to_string())?;
        validate_application(&staged, snapshot, runtime)?;
        let blobs = transaction
            .store(ASSET_BLOB_STORE_NAME)
            .map_err(|e| e.to_string())?;
        for asset in &staged.assets {
            let blob = blobs
                .get(JsValue::from_str(&asset.id))
                .await
                .map_err(|e| e.to_string())?
                .and_then(|value| value.dyn_into::<Blob>().ok())
                .ok_or("编辑原图已移除，未应用输入。")?;
            if blob.size() as u64 != asset.byte_len || blob.type_() != asset.mime_type {
                return Err("编辑原图大小或格式不匹配。".into());
            }
        }
        store
            .put(&snapshot_value, Some(&JsValue::from_str(SNAPSHOT_KEY)))
            .await
            .map_err(|e| e.to_string())?;
        store
            .put(
                &JsValue::from_str(&runtime_value),
                Some(&JsValue::from_str(RUNTIME_KEY)),
            )
            .await
            .map_err(|e| e.to_string())?;
        store
            .delete(JsValue::from_str(staging_key))
            .await
            .map_err(|e| e.to_string())?;
        if !valid() {
            return Err("编辑输入已变化或应用已取消，未覆盖当前工作台。".into());
        }
        Ok::<_, String>(())
    }
    .await;
    if let Err(error) = write {
        // 显式中止，不能因 Rust 提前返回而让 IndexedDB 自动提交之前已成功的 put。
        let _ = transaction.abort().await;
        return Err(error);
    }
    transaction.commit().await.map_err(|e| e.to_string())
}

fn validate_application(
    staged: &EditorAssetStaging,
    snapshot: &LocalAppState,
    runtime: &EditorRuntime,
) -> Result<(), String> {
    if staged.thread_id != runtime.thread_id
        || !snapshot
            .threads
            .iter()
            .any(|thread| thread.id == runtime.thread_id)
        || staged.assets.is_empty()
        || staged.assets.len() > 2
        || runtime.reference_ids.len() > mew_image_shared::MAX_GENERATION_REFERENCE_IMAGES
        || snapshot.tombstones.iter().any(|item| {
            item.entity_kind == mew_image_shared::SyncEntityKind::Thread
                && item.entity_id == runtime.thread_id
        })
    {
        return Err("编辑应用与会话快照不一致。".into());
    }
    let mut unique_ids = HashSet::new();
    let editing = runtime.editing_by_thread.get(&runtime.thread_id);
    for asset in &staged.assets {
        if !unique_ids.insert(&asset.id)
            || !snapshot.assets.iter().any(|saved| {
                saved.id == asset.id
                    && saved.sha256 == asset.sha256
                    && saved.byte_len == asset.byte_len
                    && saved.mime_type == asset.mime_type
                    && saved.width == asset.width
                    && saved.height == asset.height
            })
            || snapshot.tombstones.iter().any(|tombstone| {
                tombstone.entity_kind == mew_image_shared::SyncEntityKind::Asset
                    && tombstone.entity_id == asset.id
            })
            || (!runtime.reference_ids.contains(&asset.id)
                && editing.is_none_or(|editing| editing.mask_asset_id.as_ref() != Some(&asset.id)))
        {
            return Err("编辑资源未完整纳入工作区快照。".into());
        }
    }
    let mut reference_ids = HashSet::new();
    for id in &runtime.reference_ids {
        if !reference_ids.insert(id)
            || !snapshot
                .assets
                .iter()
                .any(|asset| &asset.id == id && !mew_image_shared::is_edit_mask(asset))
        {
            return Err("普通参考图重复、缺失或包含独立遮罩。".into());
        }
    }
    if let Some(editing) = editing {
        editing.validate_resources(&snapshot.assets)?;
        if runtime.reference_ids.first() != Some(&editing.base_asset_id) {
            return Err("编辑底图必须为第一张参考图。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{ImageEditingMode, ImageEditingSnapshot, SyncEntityKind, SyncTombstone};

    fn application(masked: bool) -> (EditorAssetStaging, LocalAppState, EditorRuntime) {
        let mut snapshot = LocalAppState::default();
        let thread_id = snapshot.threads[0].id.clone();
        let base: ImageAssetRef = serde_json::from_value(serde_json::json!({
            "id": "base", "sha256": "hash", "mime_type": "image/png", "byte_len": 100,
            "width": 32, "height": 32, "created_at": "now", "updated_at": "now", "metadata": {}
        }))
        .unwrap();
        snapshot.assets.push(base);
        let mut runtime = EditorRuntime {
            thread_id: thread_id.clone(),
            reference_ids: vec!["base".into()],
            ..Default::default()
        };
        if masked {
            let mut mask = snapshot.assets[0].clone();
            mask.id = "mask".into();
            mask.metadata
                .insert("asset_role".into(), mew_image_shared::EDIT_MASK_ROLE.into());
            snapshot.assets.push(mask);
            runtime.editing_by_thread.insert(
                thread_id.clone(),
                ImageEditingSnapshot {
                    mode: ImageEditingMode::Mask,
                    base_asset_id: "base".into(),
                    mask_asset_id: Some("mask".into()),
                    instruction: None,
                },
            );
        }
        let staged = EditorAssetStaging {
            thread_id,
            assets: snapshot.assets.clone(),
        };
        (staged, snapshot, runtime)
    }

    #[test]
    fn complete_sketch_and_mask_applications_validate_without_embedded_payloads() {
        for masked in [false, true] {
            let (staged, snapshot, runtime) = application(masked);
            assert!(snapshot.assets.iter().all(|asset| asset.data_url.is_none()));
            assert!(validate_application(&staged, &snapshot, &runtime).is_ok());
        }
    }

    #[test]
    fn missing_or_changed_resources_cannot_be_committed() {
        let (staged, snapshot, runtime) = application(true);
        for change in 0..6 {
            let mut changed = snapshot.clone();
            match change {
                0 => {
                    changed.assets.pop();
                }
                1 => changed.assets[1].sha256 = "changed".into(),
                2 => changed.assets[1].byte_len += 1,
                3 => changed.assets[1].width = Some(16),
                4 => changed.assets[1].mime_type = "image/jpeg".into(),
                _ => changed.tombstones.push(SyncTombstone {
                    entity_kind: SyncEntityKind::Asset,
                    entity_id: "mask".into(),
                    deleted_at: "now".into(),
                }),
            }
            assert!(
                validate_application(&staged, &changed, &runtime).is_err(),
                "change {change}"
            );
        }
    }

    #[test]
    fn changed_thread_reference_order_or_mask_in_normal_inputs_is_rejected() {
        let (staged, snapshot, runtime) = application(true);
        for change in 0..4 {
            let mut changed = runtime.clone();
            match change {
                0 => changed.thread_id = "another".into(),
                1 => changed.reference_ids.clear(),
                2 => changed.reference_ids.push("mask".into()),
                _ => changed.reference_ids.push("base".into()),
            }
            assert!(validate_application(&staged, &snapshot, &changed).is_err());
        }
    }

    #[test]
    fn duplicate_staged_ids_and_deleted_threads_are_rejected() {
        let (mut staged, mut snapshot, runtime) = application(false);
        staged.assets.push(staged.assets[0].clone());
        assert!(validate_application(&staged, &snapshot, &runtime).is_err());
        staged.assets.pop();
        snapshot.tombstones.push(SyncTombstone {
            entity_kind: SyncEntityKind::Thread,
            entity_id: runtime.thread_id.clone(),
            deleted_at: "now".into(),
        });
        assert!(validate_application(&staged, &snapshot, &runtime).is_err());
        snapshot.tombstones.clear();
        snapshot.threads.clear();
        assert!(validate_application(&staged, &snapshot, &runtime).is_err());
    }
}
