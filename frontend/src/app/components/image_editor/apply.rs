use std::collections::{BTreeMap, HashSet};

use leptos::prelude::*;
use mew_image_shared::{ImageAssetRef, MAX_GENERATION_REFERENCE_IMAGES, now_rfc3339};
use sha2::{Digest, Sha256};
use wasm_bindgen_futures::JsFuture;

use crate::{
    app::{
        state::{ComposerState, PersistenceState, UiState, WorkspaceState},
        utils::{editor_budget::acquire_editor_budget, persistence::snapshot_workspace_state},
    },
    image_editor::{EditMode, EditorBase, EditorDraft, encode_edit, save_draft},
    storage::{
        commit_editor_application, discard_editor_staging, load_asset_object_urls,
        revoke_asset_object_url, stage_editor_assets,
    },
};

mod persistence;
use persistence::{
    ApplicationPersistGuard, current_runtime, preserve_current_runtime, runtime_matches,
};

pub(super) struct EditorInputs {
    pub(super) base: Option<std::rc::Rc<EditorBase>>,
    pub(super) imported_mask: Option<std::rc::Rc<crate::image_editor::EditorMask>>,
}

fn can_apply(
    draft: &EditorDraft,
    workspace: WorkspaceState,
    composer: ComposerState,
    ui: UiState,
) -> Result<Vec<String>, String> {
    if workspace.current_thread_id.get_untracked() != draft.thread_id
        || ui.image_editor_thread.get_untracked().as_deref() != Some(&draft.thread_id)
    {
        return Err("会话已切换，草稿未应用。".into());
    }
    if composer
        .foreground_generation_task_id
        .get_untracked()
        .is_some()
    {
        return Err("前台正在生成，请完成后再应用编辑输入。".into());
    }
    let current_edit = composer
        .editing_by_thread
        .with_untracked(|items| items.get(&draft.thread_id).cloned());
    let references = retained_references(
        draft,
        &composer.selected_reference_ids.get_untracked(),
        composer.continuation_asset_id.get_untracked().as_deref(),
        current_edit
            .as_ref()
            .map(|editing| editing.base_asset_id.as_str()),
    );
    if references.len() >= MAX_GENERATION_REFERENCE_IMAGES {
        return Err("最多 10 张普通参考图，请先移除一张再应用编辑输入。".into());
    }
    Ok(references)
}

/// 替换当前编辑输入而非追加副本；这里只改选择，不删除历史任务可能仍引用的资源。
fn retained_references(
    draft: &EditorDraft,
    selected: &[String],
    continuation: Option<&str>,
    previous_edit: Option<&str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    selected
        .iter()
        .map(String::as_str)
        .chain(continuation)
        .filter(|id| {
            if draft.mode != EditMode::Sketch
                && (Some(*id) == draft.base_asset_id.as_deref() || Some(*id) == previous_edit)
            {
                return false;
            }
            seen.insert(*id)
        })
        .map(str::to_owned)
        .collect()
}

pub(super) async fn apply_edit(
    draft: EditorDraft,
    inputs: EditorInputs,
    workspace: WorkspaceState,
    composer: ComposerState,
    ui: UiState,
    persistence: PersistenceState,
    cancelled: RwSignal<bool>,
) -> Result<(), String> {
    let EditorInputs {
        base,
        imported_mask,
    } = inputs;
    if draft.mode != EditMode::Sketch && base.is_none() {
        return Err("编辑底图尚未就绪，未改变工作台。".into());
    }
    if draft.imported_mask_asset_id.as_deref() != imported_mask.as_ref().map(|mask| mask.id()) {
        return Err("导入遮罩尚未就绪或已更换，未改变工作台。".into());
    }
    if !persistence
        .local_state_status
        .with_untracked(|state| state.is_ready())
    {
        return Err("本地数据尚未就绪，不能应用编辑输入。".into());
    }
    can_apply(&draft, workspace, composer, ui)?;
    save_draft(&draft).await?;
    let _budget = acquire_editor_budget(composer, draft.width, draft.height, || {
        cancelled.try_get_untracked() != Some(false)
            || can_apply(&draft, workspace, composer, ui).is_err()
    })
    .await?;
    let encoded = encode_edit(
        &draft,
        base.as_ref().map(|base| base.image()),
        imported_mask.as_ref().map(|mask| mask.image()),
    )
    .await?;
    if cancelled.try_get_untracked() != Some(false) {
        return Err("已取消应用，草稿已保留。".into());
    }
    let mut asset = encoded_asset(&draft, &encoded.image, false).await?;
    let mask = match encoded.mask.as_ref() {
        Some(blob) => Some(encoded_asset(&draft, blob, true).await?),
        None => None,
    };
    let id = asset.id.clone();
    let mut blobs = vec![(id.clone(), encoded.image)];
    if let Some((mask, blob)) = mask.as_ref().zip(encoded.mask) {
        blobs.push((mask.id.clone(), blob));
    }
    let ids = blobs.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
    // 底图和遮罩在同一事务中写入，任何失败都不应用半份输入。
    let metadata = std::iter::once(asset.clone())
        .chain(mask.iter().cloned())
        .collect::<Vec<_>>();
    let staging_key = stage_editor_assets(&draft.thread_id, &metadata, &blobs).await?;
    drop(blobs);
    let loaded = load_asset_object_urls(std::slice::from_ref(&id)).await;
    if loaded.is_ok()
        && let Ok(thumbnail) = crate::app::utils::image::thumbnail_data_url_from_asset(
            &asset,
            crate::app::THUMBNAIL_MAX_EDGE,
        )
        .await
    {
        asset
            .metadata
            .insert(crate::app::THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
    }
    let validation = if cancelled.try_get_untracked() != Some(false) {
        Err("已取消应用，草稿已保留。".into())
    } else {
        can_apply(&draft, workspace, composer, ui)
    };
    if loaded.is_err() || validation.is_err() {
        revoke_asset_object_url(&id);
        let cleanup = discard_editor_staging(&staging_key, &ids).await;
        let error = loaded
            .err()
            .or_else(|| validation.err())
            .unwrap_or_default();
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}；暂存图片清理失败：{cleanup}"),
        });
    }
    let guard = ApplicationPersistGuard::acquire(workspace, persistence, || {
        cancelled.try_get_untracked() != Some(false)
            || can_apply(&draft, workspace, composer, ui).is_err()
    })
    .await;
    let _guard = match guard {
        Ok(guard) => guard,
        Err(error) => return Err(cleanup_failed_application(&staging_key, &ids, &id, error).await),
    };
    let previous_runtime = current_runtime(workspace, composer);
    let mut references = can_apply(&draft, workspace, composer, ui)?;
    if draft.mode == EditMode::Sketch {
        references.push(id.clone());
    } else {
        references.insert(0, id.clone());
    }
    let editing = match draft.mode {
        EditMode::Sketch => None,
        mode => Some(mew_image_shared::ImageEditingSnapshot {
            mode: if mode == EditMode::Mask {
                mew_image_shared::ImageEditingMode::Mask
            } else {
                mew_image_shared::ImageEditingMode::Annotation
            },
            base_asset_id: id.clone(),
            mask_asset_id: mask.as_ref().map(|mask| mask.id.clone()),
            instruction: (mode == EditMode::Annotation)
                .then(|| "请依据图片中的辅助标记修改内容，最终结果不要保留辅助标记。".into()),
        }),
    };
    let mut next_runtime = previous_runtime.clone();
    next_runtime.reference_ids = references;
    if let Some(editing) = editing {
        next_runtime
            .editing_by_thread
            .insert(draft.thread_id.clone(), editing);
    } else {
        next_runtime.editing_by_thread.remove(&draft.thread_id);
    }
    if draft.mode != EditMode::Sketch {
        next_runtime.continuation_id = None;
    }
    let mut snapshot = snapshot_workspace_state(
        workspace.tasks,
        workspace.threads,
        workspace.assets,
        workspace.checkpoint,
        workspace.tombstones,
    );
    snapshot.assets.push(asset.clone());
    snapshot.assets.extend(mask.iter().cloned());
    let still_current = || {
        cancelled.try_get_untracked() == Some(false)
            && persistence
                .local_state_status
                .with_untracked(|state| state.is_ready())
            && can_apply(&draft, workspace, composer, ui).is_ok()
            && runtime_matches(&previous_runtime, workspace, composer)
    };
    if let Err(error) =
        commit_editor_application(&staging_key, &snapshot, &next_runtime, still_current).await
    {
        // 事务占用过修订号；失败后重新保存当前选择，不能丢掉被它作废的普通保存任务。
        let error = match preserve_current_runtime(workspace, composer, persistence).await {
            Ok(()) => error,
            Err(save_error) => format!("{error}；当前选择保存失败，请勿刷新：{save_error}"),
        };
        return Err(cleanup_failed_application(&staging_key, &ids, &id, error).await);
    }
    // 提交完成是应用的确定点。若等待提交确认时发生切换，不覆盖用户的新选择。
    let apply_selection = still_current();
    batch(|| {
        let keep_assets = persistence
            .local_state_status
            .with_untracked(|state| state.is_ready())
            && workspace.threads.with_untracked(|threads| {
                threads.iter().any(|thread| thread.id == draft.thread_id)
            });
        workspace.assets.update(|assets| {
            for asset in std::iter::once(asset).chain(mask) {
                if keep_assets && !assets.iter().any(|saved| saved.id == asset.id) {
                    assets.push(asset);
                }
            }
        });
        if apply_selection {
            composer
                .selected_reference_ids
                .set(next_runtime.reference_ids);
            composer
                .editing_by_thread
                .set(next_runtime.editing_by_thread);
            composer
                .continuation_asset_id
                .set(next_runtime.continuation_id);
        }
    });
    if !apply_selection {
        preserve_current_runtime(workspace, composer, persistence).await?;
    }
    composer.status_text.set(
        if apply_selection {
            "编辑输入已保存并应用到工作台，未自动生成。"
        } else {
            "编辑输入已保存；当前选择已变化，未覆盖新选择。"
        }
        .into(),
    );
    Ok(())
}

async fn cleanup_failed_application(
    staging_key: &str,
    ids: &[String],
    display_id: &str,
    error: String,
) -> String {
    revoke_asset_object_url(display_id);
    match discard_editor_staging(staging_key, ids).await {
        Ok(()) => error,
        Err(cleanup) => format!("{error}；暂存图片清理失败：{cleanup}"),
    }
}

async fn encoded_asset(
    draft: &EditorDraft,
    blob: &web_sys::Blob,
    mask: bool,
) -> Result<ImageAssetRef, String> {
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(|error| format!("读取编码图片失败：{error:?}"))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    drop(bytes);
    drop(buffer);
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let mut asset = ImageAssetRef {
        id: id.clone(),
        sha256,
        mime_type: "image/png".into(),
        byte_len: blob.size() as u64,
        width: Some(draft.width),
        height: Some(draft.height),
        created_at: now.clone(),
        updated_at: now,
        data_url: None,
        remote_object_key: None,
        remote_url: None,
        source_task_id: None,
        metadata: BTreeMap::from([
            ("thread_id".into(), draft.thread_id.clone()),
            (
                "editor_mode".into(),
                format!("{:?}", draft.mode).to_lowercase(),
            ),
        ]),
    };
    if mask {
        asset
            .metadata
            .insert("asset_role".into(), mew_image_shared::EDIT_MASK_ROLE.into());
    }
    Ok(asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_edit_preserves_other_references_and_frees_one_slot() {
        let mut draft = EditorDraft::new("thread".into(), Some("original".into()), 32, 32).unwrap();
        draft.mode = EditMode::Mask;
        let mut selected = (0..9)
            .map(|index| format!("other-{index}"))
            .collect::<Vec<_>>();
        selected.insert(0, "previous-copy".into());
        let before = selected.clone();
        let retained =
            retained_references(&draft, &selected, Some("original"), Some("previous-copy"));
        assert_eq!(retained.len(), 9);
        assert_eq!(retained[0], "other-0");
        assert_eq!(selected, before);
        assert!(
            !retained
                .iter()
                .any(|id| matches!(id.as_str(), "original" | "previous-copy"))
        );
        assert_eq!(
            retained_references(
                &draft,
                &["other".into(), "other".into()],
                Some("other"),
                None
            ),
            ["other"]
        );
    }

    #[test]
    fn sketch_keeps_existing_base_and_continuation_inputs() {
        let draft = EditorDraft::new("thread".into(), None, 32, 32).unwrap();
        let retained = retained_references(
            &draft,
            &["previous-copy".into()],
            Some("base"),
            Some("previous-copy"),
        );
        assert_eq!(retained, ["previous-copy", "base"]);
    }
}
