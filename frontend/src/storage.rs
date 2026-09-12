use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use js_sys::{Array, Uint8Array};
use mew_image_shared::{
    AppPreferences, EncryptedApiConfig, GeneratedImageResult, GenerationResult, ImageAssetRef,
    LocalAppState, ParameterSnapshot, TaskStatus, now_rfc3339,
};
use rexie::{ObjectStore, Rexie, TransactionMode};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag};

const DB_NAME: &str = "mew-image-local";
const STORE_NAME: &str = "kv";
const ASSET_BLOB_STORE_NAME: &str = "asset_blobs";
// v2 的字符串 data URL store 只用于兼容读取，读取成功后会懒迁移到 Blob store。
const ASSET_STORE_NAME: &str = "asset_payloads";
const SNAPSHOT_KEY: &str = "app_state";
const CONFIGS_KEY: &str = "configs_state";
const PREFERENCES_KEY: &str = "preferences_state";
const TRUSTED_SYNC_KEY_PREFIX: &str = "mew-image-trusted-sync-key:";
const API_KEY_SYNC_ENABLED_PREFIX: &str = "mew-image-api-key-sync-enabled:";
const GENERATION_QUEUE_MODE_KEY: &str = "mew-image-generation-queue-mode";
const GENERATION_STAGING_KEY_PREFIX: &str = "generation_staging:";
const EDITOR_STAGING_KEY_PREFIX: &str = "editor_asset_staging:";

mod editor_commit;
pub(crate) use editor_commit::commit_editor_application;

#[derive(serde::Serialize, serde::Deserialize)]
struct EditorAssetStaging {
    thread_id: String,
    assets: Vec<ImageAssetRef>,
}

/// Blob 与编辑结果元数据一起提交，跨越工作区后台快照的保存窗口。
pub(crate) async fn stage_editor_assets(
    thread_id: &str,
    assets: &[ImageAssetRef],
    blobs: &[(String, Blob)],
) -> Result<String, String> {
    if assets.is_empty()
        || assets.len() > 2
        || assets.len() != blobs.len()
        || assets.iter().any(|asset| {
            !blobs
                .iter()
                .any(|(id, blob)| id == &asset.id && blob.size() as u64 == asset.byte_len)
        })
    {
        return Err("编辑暂存资源与文件不一致。".into());
    }
    let key = format!("{EDITOR_STAGING_KEY_PREFIX}{}", uuid::Uuid::new_v4());
    let value = serde_json::to_string(&EditorAssetStaging {
        thread_id: thread_id.into(),
        assets: assets.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for (id, blob) in blobs {
        store
            .put(blob.as_ref(), Some(&JsValue::from_str(id)))
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?
        .put(&JsValue::from_str(&value), Some(&JsValue::from_str(&key)))
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(key)
}

pub(crate) async fn discard_editor_staging(key: &str, asset_ids: &[String]) -> Result<(), String> {
    if !key.starts_with(EDITOR_STAGING_KEY_PREFIX) {
        return Err("编辑暂存键无效。".into());
    }
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let manifest_store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let Some(value) = manifest_store
        .get(JsValue::from_str(key))
        .await
        .map_err(|error| error.to_string())?
    else {
        // 已确认的应用不再有清单，迟到的取消清理不得删除其正式原图。
        transaction
            .done()
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    };
    let staged: EditorAssetStaging =
        serde_json::from_str(&value.as_string().ok_or("编辑暂存清单类型异常。")?)
            .map_err(|error| error.to_string())?;
    let staged_ids = staged
        .assets
        .iter()
        .map(|asset| &asset.id)
        .collect::<HashSet<_>>();
    if staged_ids.len() != asset_ids.len() || asset_ids.iter().any(|id| !staged_ids.contains(id)) {
        return Err("编辑清理范围与暂存清单不一致，未删除图片。".into());
    }
    let store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for id in asset_ids {
        store
            .delete(JsValue::from_str(id))
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?
        .delete(JsValue::from_str(key))
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn recover_editor_assets(db: &Rexie, state: &mut LocalAppState) -> Result<(), String> {
    let draft_ids = crate::image_editor::draft_asset_ids(None).await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME],
            TransactionMode::ReadOnly,
        )
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let blobs = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let mut keys = Vec::new();
    let mut orphan_ids = HashSet::new();
    for key in store
        .get_all_keys(None, None)
        .await
        .map_err(|error| error.to_string())?
    {
        if !key
            .as_string()
            .is_some_and(|key| key.starts_with(EDITOR_STAGING_KEY_PREFIX))
        {
            continue;
        }
        let value = store
            .get(key.clone())
            .await
            .map_err(|error| error.to_string())?
            .and_then(|value| value.as_string())
            .ok_or("编辑暂存清单损坏，原数据未重置。")?;
        let staged: EditorAssetStaging =
            serde_json::from_str(&value).map_err(|error| error.to_string())?;
        for asset in &staged.assets {
            if editor_asset_needs_recovery(state, &staged.thread_id, &asset.id)
                || (draft_ids.contains(&asset.id)
                    && !state.assets.iter().any(|item| item.id == asset.id)
                    && !state.tombstones.iter().any(|item| {
                        item.entity_kind == mew_image_shared::SyncEntityKind::Asset
                            && item.entity_id == asset.id
                    }))
            {
                let blob = blobs
                    .get(JsValue::from_str(&asset.id))
                    .await
                    .map_err(|error| error.to_string())?
                    .and_then(|value| value.dyn_into::<Blob>().ok())
                    .ok_or("编辑暂存原图缺失，无法完成恢复。")?;
                if blob.size() as u64 != asset.byte_len {
                    return Err("编辑暂存原图大小不匹配。".into());
                }
                state.assets.push(asset.clone());
            } else if editor_asset_is_orphan(state, &draft_ids, &asset.id) {
                orphan_ids.insert(asset.id.clone());
            }
        }
        keys.push(key);
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    if keys.is_empty() {
        return Ok(());
    }
    // 先确认索引保存，再清理清单；任一步失败都允许下次重复恢复。
    save_workspace_snapshot_with_db(db, state).await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    for key in keys {
        store.delete(key).await.map_err(|error| error.to_string())?;
    }
    let blobs = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for id in orphan_ids {
        // 再次检查合并后的索引，避免另一份清单刚恢复的共享图片被回收。
        if editor_asset_is_orphan(state, &draft_ids, &id) {
            blobs
                .delete(JsValue::from_str(&id))
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn editor_asset_needs_recovery(state: &LocalAppState, thread_id: &str, asset_id: &str) -> bool {
    (state.threads.iter().any(|thread| thread.id == thread_id)
        || state
            .tasks
            .iter()
            .any(|task| task.input_asset_ids().any(|id| id == asset_id)))
        && !state.assets.iter().any(|asset| asset.id == asset_id)
        && !state.tombstones.iter().any(|item| {
            item.entity_kind == mew_image_shared::SyncEntityKind::Asset
                && item.entity_id == asset_id
        })
}

fn editor_asset_is_orphan(
    state: &LocalAppState,
    draft_ids: &HashSet<String>,
    asset_id: &str,
) -> bool {
    !state.assets.iter().any(|asset| asset.id == asset_id)
        && !state
            .tasks
            .iter()
            .any(|task| task.input_asset_ids().any(|id| id == asset_id))
        && !draft_ids.contains(asset_id)
}
const ASSET_WRITE_BATCH_MAX_BYTES: usize = 32 * 1024 * 1024;

thread_local! {
    static ASSET_OBJECT_URLS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationStagingManifest {
    pub task_id: String,
    pub expected_count: u32,
    pub assets: Vec<ImageAssetRef>,
    pub finished: bool,
    pub updated_at: String,
}

#[derive(Default)]
struct GenerationStagingRecovery {
    handled_task_ids: Vec<String>,
    orphan_asset_ids: Vec<String>,
}

impl GenerationStagingManifest {
    pub fn new(
        task_id: String,
        expected_count: u32,
        assets: Vec<ImageAssetRef>,
        finished: bool,
    ) -> Self {
        Self {
            task_id,
            expected_count,
            assets,
            finished,
            updated_at: now_rfc3339(),
        }
    }
}

fn generation_staging_key(task_id: &str) -> String {
    format!("{GENERATION_STAGING_KEY_PREFIX}{task_id}")
}

pub fn runtime_asset_object_url(asset_id: &str) -> Option<String> {
    ASSET_OBJECT_URLS.with(|urls| urls.borrow().get(asset_id).cloned())
}

pub fn revoke_asset_object_url(asset_id: &str) {
    let previous = ASSET_OBJECT_URLS.with(|urls| urls.borrow_mut().remove(asset_id));
    if let Some(previous) = previous {
        let _ = web_sys::Url::revoke_object_url(&previous);
    }
}

pub fn revoke_all_asset_object_urls() {
    let urls = ASSET_OBJECT_URLS.with(|urls| std::mem::take(&mut *urls.borrow_mut()));
    for url in urls.into_values() {
        let _ = web_sys::Url::revoke_object_url(&url);
    }
}

pub fn load_generation_queue_mode() -> bool {
    local_storage_value(GENERATION_QUEUE_MODE_KEY).as_deref() == Some("true")
}

pub fn save_generation_queue_mode(enabled: bool) -> Result<(), String> {
    set_local_storage_value(
        GENERATION_QUEUE_MODE_KEY,
        if enabled { "true" } else { "false" },
    )
}

pub fn clear_generation_queue_mode() -> Result<(), String> {
    let Some(storage) = web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
    else {
        return Err("浏览器本地存储不可用".into());
    };
    storage
        .remove_item(GENERATION_QUEUE_MODE_KEY)
        .map_err(|error| format!("{error:?}"))
}

pub fn load_trusted_sync_secret(user_id: &str) -> Option<String> {
    local_storage_value(&format!("{TRUSTED_SYNC_KEY_PREFIX}{user_id}"))
}

pub fn save_trusted_sync_secret(user_id: &str, secret: &str) -> Result<(), String> {
    set_local_storage_value(&format!("{TRUSTED_SYNC_KEY_PREFIX}{user_id}"), secret)
}

pub fn clear_trusted_sync_secret(user_id: &str) -> Result<(), String> {
    let Some(storage) = web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
    else {
        return Err("浏览器本地存储不可用".into());
    };
    storage
        .remove_item(&format!("{TRUSTED_SYNC_KEY_PREFIX}{user_id}"))
        .map_err(|error| format!("{error:?}"))
}

pub fn load_api_key_sync_enabled(user_id: &str) -> bool {
    local_storage_value(&format!("{API_KEY_SYNC_ENABLED_PREFIX}{user_id}"))
        .map(|value| value != "false")
        .unwrap_or(true)
}

pub fn save_api_key_sync_enabled(user_id: &str, enabled: bool) -> Result<(), String> {
    set_local_storage_value(
        &format!("{API_KEY_SYNC_ENABLED_PREFIX}{user_id}"),
        if enabled { "true" } else { "false" },
    )
}

fn local_storage_value(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
        .and_then(|storage| storage.get_item(key).ok())
        .flatten()
}

fn set_local_storage_value(key: &str, value: &str) -> Result<(), String> {
    let Some(storage) = web_sys::window()
        .and_then(|window| window.local_storage().ok())
        .flatten()
    else {
        return Err("浏览器本地存储不可用".into());
    };
    storage
        .set_item(key, value)
        .map_err(|error| format!("{error:?}"))
}

pub(crate) async fn open_db() -> Result<Rexie, String> {
    Rexie::builder(DB_NAME)
        .version(3)
        .add_object_store(ObjectStore::new(STORE_NAME))
        .add_object_store(ObjectStore::new(ASSET_BLOB_STORE_NAME))
        .add_object_store(ObjectStore::new(ASSET_STORE_NAME))
        .build()
        .await
        .map_err(|error| error.to_string())
}

pub async fn load_snapshot() -> Result<LocalAppState, String> {
    let db = open_db().await?;
    let transaction = db
        .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let snapshot_value = store
        .get(JsValue::from_str(SNAPSHOT_KEY))
        .await
        .map_err(|error| error.to_string())?;
    let configs_value = store
        .get(JsValue::from_str(CONFIGS_KEY))
        .await
        .map_err(|error| error.to_string())?;
    let preferences_value = store
        .get(JsValue::from_str(PREFERENCES_KEY))
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;

    let mut state = match snapshot_value {
        Some(value) => serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())?,
        None => LocalAppState::default(),
    };
    if let Some(value) = configs_value {
        state.configs = serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())?;
    }
    if let Some(value) = preferences_value {
        state.preferences =
            serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())?;
    }
    let recovery = recover_generation_staging(&db, &mut state).await?;
    if !recovery.handled_task_ids.is_empty() {
        save_workspace_snapshot_with_db(&db, &state).await?;
        clear_generation_staging_entries(&db, &recovery).await?;
    }
    recover_editor_assets(&db, &mut state).await?;
    Ok(state)
}

pub async fn save_workspace_snapshot(state: &LocalAppState) -> Result<(), String> {
    let db = open_db().await?;
    save_workspace_snapshot_with_db(&db, state).await
}

async fn save_workspace_snapshot_with_db(db: &Rexie, state: &LocalAppState) -> Result<(), String> {
    let transaction = db
        .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    store
        .put(
            &serde_wasm_bindgen::to_value(state).map_err(|error| error.to_string())?,
            Some(&JsValue::from_str(SNAPSHOT_KEY)),
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn recover_generation_staging(
    db: &Rexie,
    state: &mut LocalAppState,
) -> Result<GenerationStagingRecovery, String> {
    let transaction = db
        .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let mut manifests = Vec::new();
    let staging_keys = store
        .get_all_keys(None, None)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|key| key.as_string())
        .filter(|key| key.starts_with(GENERATION_STAGING_KEY_PREFIX))
        .collect::<Vec<_>>();
    for staging_key in staging_keys {
        let value = store
            .get(JsValue::from_str(&staging_key))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(value) = value {
            manifests.push(
                serde_wasm_bindgen::from_value::<GenerationStagingManifest>(value)
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;

    Ok(apply_generation_staging_manifests(state, manifests))
}

fn apply_generation_staging_manifests(
    state: &mut LocalAppState,
    manifests: Vec<GenerationStagingManifest>,
) -> GenerationStagingRecovery {
    let mut retained_asset_ids = state
        .assets
        .iter()
        .map(|asset| asset.id.clone())
        .collect::<HashSet<_>>();
    let mut recovery = GenerationStagingRecovery {
        handled_task_ids: Vec::with_capacity(manifests.len()),
        orphan_asset_ids: Vec::new(),
    };
    for manifest in manifests {
        recovery.handled_task_ids.push(manifest.task_id.clone());
        let Some(task) = state
            .tasks
            .iter_mut()
            .find(|task| task.id == manifest.task_id)
        else {
            recovery.orphan_asset_ids.extend(
                manifest
                    .assets
                    .into_iter()
                    .map(|asset| asset.id)
                    .filter(|asset_id| !retained_asset_ids.contains(asset_id)),
            );
            continue;
        };
        if task.status != TaskStatus::Running || manifest.assets.is_empty() {
            continue;
        }
        for asset in manifest.assets {
            if !retained_asset_ids.insert(asset.id.clone()) {
                continue;
            }
            state.assets.push(asset);
        }
        let total_assets = state
            .assets
            .iter()
            .filter(|asset| asset.source_task_id.as_deref() == Some(task.id.as_str()))
            .count();
        if total_assets == 0 {
            continue;
        }
        let mut parameter_snapshot = task
            .generation_settings
            .as_ref()
            .map(|settings| ParameterSnapshot {
                requested_width: Some(settings.width),
                requested_height: Some(settings.height),
                requested_quality: settings.quality.clone(),
                ..ParameterSnapshot::default()
            })
            .unwrap_or_default();
        if let Some((width, height)) = state
            .assets
            .iter()
            .find(|asset| asset.source_task_id.as_deref() == Some(task.id.as_str()))
            .and_then(|asset| asset.width.zip(asset.height))
        {
            parameter_snapshot.actual_width = Some(width);
            parameter_snapshot.actual_height = Some(height);
        }
        task.result = Some(GenerationResult {
            images: (0..total_assets)
                .map(|_| GeneratedImageResult {
                    url: None,
                    data_url: None,
                })
                .collect(),
            parameter_snapshot,
            upstream_response_id: None,
            raw_response_json: None,
        });
        task.updated_at = now_rfc3339();
        if manifest.finished {
            task.status = TaskStatus::Succeeded;
            task.error_message = None;
        } else {
            task.status = TaskStatus::Failed;
            task.error_message = Some(format!(
                "上次生成意外中断，已恢复 {total_assets}/{} 张已落盘结果。",
                manifest.expected_count.max(total_assets as u32)
            ));
        }
    }
    recovery.orphan_asset_ids.sort_unstable();
    recovery.orphan_asset_ids.dedup();
    recovery
}

async fn clear_generation_staging_entries(
    db: &Rexie,
    recovery: &GenerationStagingRecovery,
) -> Result<(), String> {
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for task_id in &recovery.handled_task_ids {
        store
            .delete(JsValue::from_str(&generation_staging_key(task_id)))
            .await
            .map_err(|error| error.to_string())?;
    }
    for asset_id in &recovery.orphan_asset_ids {
        blob_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        legacy_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    for asset_id in &recovery.orphan_asset_ids {
        revoke_asset_object_url(asset_id);
    }
    Ok(())
}

pub async fn save_ui_state(
    configs: &[EncryptedApiConfig],
    preferences: &AppPreferences,
) -> Result<(), String> {
    let db = open_db().await?;
    let transaction = db
        .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    store
        .put(
            &serde_wasm_bindgen::to_value(configs).map_err(|error| error.to_string())?,
            Some(&JsValue::from_str(CONFIGS_KEY)),
        )
        .await
        .map_err(|error| error.to_string())?;
    store
        .put(
            &serde_wasm_bindgen::to_value(preferences).map_err(|error| error.to_string())?,
            Some(&JsValue::from_str(PREFERENCES_KEY)),
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn apply_asset_payload_changes(
    payload_writes: &[(String, String)],
    payload_deletes: &[String],
) -> Result<(), String> {
    if payload_writes.is_empty() && payload_deletes.is_empty() {
        return Ok(());
    }
    if !payload_deletes.is_empty() {
        apply_asset_blob_changes(&[], payload_deletes).await?;
        for asset_id in payload_deletes {
            revoke_asset_object_url(asset_id);
        }
    }

    let mut batch_start = 0;
    while batch_start < payload_writes.len() {
        let batch_end = asset_write_batch_end(payload_writes, batch_start);
        let prepared_writes = payload_writes[batch_start..batch_end]
            .iter()
            .map(|(asset_id, data_url)| {
                data_url_to_blob(data_url).map(|blob| (asset_id.clone(), blob))
            })
            .collect::<Result<Vec<_>, _>>()?;
        apply_asset_blob_changes(&prepared_writes, &[]).await?;
        for (asset_id, blob) in &prepared_writes {
            replace_cached_asset_blob(asset_id, blob);
        }
        batch_start = batch_end;
    }
    Ok(())
}

/// 提前把压缩后的 Data URL 转为 Blob；保存重试期间不再重复解码 Base64。
pub fn prepare_generation_asset_blobs(
    payload_writes: &[(String, String)],
) -> Result<Vec<(String, Blob)>, String> {
    payload_writes
        .iter()
        .map(|(asset_id, data_url)| data_url_to_blob(data_url).map(|blob| (asset_id.clone(), blob)))
        .collect()
}

/// 将生成中的图片 Blob 与恢复清单放进同一事务；只有事务成功后才能确认服务端结果。
pub async fn stage_generation_asset_blobs(
    manifest: &GenerationStagingManifest,
    prepared_writes: &[(String, Blob)],
) -> Result<(), String> {
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let state_store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for (asset_id, blob) in prepared_writes {
        blob_store
            .put(blob.as_ref(), Some(&JsValue::from_str(asset_id)))
            .await
            .map_err(|error| error.to_string())?;
        legacy_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    state_store
        .put(
            &serde_wasm_bindgen::to_value(manifest).map_err(|error| error.to_string())?,
            Some(&JsValue::from_str(&generation_staging_key(
                &manifest.task_id,
            ))),
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    for (asset_id, blob) in prepared_writes {
        replace_cached_asset_blob(asset_id, blob);
    }
    Ok(())
}

pub async fn clear_generation_staging(
    task_id: &str,
    payload_deletes: &[String],
) -> Result<(), String> {
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[STORE_NAME, ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let state_store = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    state_store
        .delete(JsValue::from_str(&generation_staging_key(task_id)))
        .await
        .map_err(|error| error.to_string())?;
    for asset_id in payload_deletes {
        blob_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        legacy_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    for asset_id in payload_deletes {
        revoke_asset_object_url(asset_id);
    }
    Ok(())
}

fn asset_write_batch_end(payload_writes: &[(String, String)], start: usize) -> usize {
    let mut total_bytes = 0_usize;
    let mut end = start;
    while let Some((_, data_url)) = payload_writes.get(end) {
        let next_bytes = data_url.len();
        if end > start && total_bytes.saturating_add(next_bytes) > ASSET_WRITE_BATCH_MAX_BYTES {
            break;
        }
        total_bytes = total_bytes.saturating_add(next_bytes);
        end += 1;
    }
    end.max(start.saturating_add(1)).min(payload_writes.len())
}

pub(crate) async fn apply_asset_blob_changes(
    payload_writes: &[(String, Blob)],
    payload_deletes: &[String],
) -> Result<(), String> {
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for asset_id in payload_deletes {
        blob_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        legacy_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    for (asset_id, blob) in payload_writes {
        blob_store
            .put(blob.as_ref(), Some(&JsValue::from_str(asset_id)))
            .await
            .map_err(|error| error.to_string())?;
        legacy_store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn load_asset_payloads(asset_ids: &[String]) -> Result<HashMap<String, String>, String> {
    if asset_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let stored_values = load_stored_asset_payloads(asset_ids).await?;
    let mut loaded = HashMap::with_capacity(stored_values.len());
    let mut legacy_blobs = Vec::new();
    for (asset_id, payload) in stored_values {
        match payload {
            StoredAssetPayload::Blob(blob) => {
                loaded.insert(asset_id, blob_to_data_url(&blob).await?);
            }
            StoredAssetPayload::Legacy(data_url) => {
                legacy_blobs.push((asset_id.clone(), data_url_to_blob(&data_url)?));
                loaded.insert(asset_id, data_url);
            }
        }
    }
    if !legacy_blobs.is_empty() {
        // 迁移失败不影响本次读取；下次读取会继续尝试。
        let _ = apply_asset_blob_changes(&legacy_blobs, &[]).await;
    }
    Ok(loaded)
}

pub async fn load_asset_object_urls(
    asset_ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let mut loaded = HashMap::new();
    let missing_ids = asset_ids
        .iter()
        .filter_map(|asset_id| {
            if let Some(url) = runtime_asset_object_url(asset_id) {
                loaded.insert(asset_id.clone(), url);
                None
            } else {
                Some(asset_id.clone())
            }
        })
        .collect::<Vec<_>>();
    let stored_values = load_stored_asset_payloads(&missing_ids).await?;
    let mut legacy_blobs = Vec::new();
    let mut blobs = Vec::with_capacity(stored_values.len());
    for (asset_id, payload) in stored_values {
        let blob = match payload {
            StoredAssetPayload::Blob(blob) => blob,
            StoredAssetPayload::Legacy(data_url) => {
                let blob = data_url_to_blob(&data_url)?;
                legacy_blobs.push((asset_id.clone(), blob.clone()));
                blob
            }
        };
        blobs.push((asset_id, blob));
    }
    if !legacy_blobs.is_empty() {
        let _ = apply_asset_blob_changes(&legacy_blobs, &[]).await;
    }
    for (asset_id, blob) in blobs {
        let object_url = cache_asset_blob(&asset_id, &blob)?;
        loaded.insert(asset_id, object_url);
    }
    Ok(loaded)
}

pub async fn store_asset_bytes_for_display(
    asset_id: &str,
    bytes: &[u8],
    mime_type: &str,
) -> Result<String, String> {
    let byte_array = Uint8Array::from(bytes);
    let parts = Array::new();
    parts.push(&byte_array.buffer());
    let options = BlobPropertyBag::new();
    options.set_type(mime_type);
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|error| format!("构建本地图片 Blob 失败：{error:?}"))?;
    apply_asset_blob_changes(&[(asset_id.to_string(), blob.clone())], &[]).await?;
    cache_asset_blob(asset_id, &blob)
}

enum StoredAssetPayload {
    Blob(Blob),
    Legacy(String),
}

/// 编辑底图和遮罩必须读取原始持久化字节；不从远程地址回退或重新编码。
pub(crate) async fn load_edit_asset_blob(asset_id: &str) -> Result<Blob, String> {
    let stored = load_stored_asset_payloads(&[asset_id.to_string()]).await?;
    match stored.into_iter().next().map(|(_, payload)| payload) {
        Some(StoredAssetPayload::Blob(blob)) => Ok(blob),
        Some(StoredAssetPayload::Legacy(data_url)) => data_url_to_blob(&data_url),
        None => Err(format!(
            "编辑资源 `{asset_id}` 的本地原图缺失，请重新应用编辑草稿。"
        )),
    }
}

pub(crate) async fn store_editor_draft_blob(asset_id: &str, blob: &Blob) -> Result<(), String> {
    if !asset_id.starts_with("editor-mask-") || blob.type_() != "image/png" {
        return Err("编辑遮罩暂存标识或格式无效。".into());
    }
    apply_asset_blob_changes(&[(asset_id.to_string(), blob.clone())], &[]).await
}

pub(crate) async fn delete_editor_draft_blobs(asset_ids: &[String]) -> Result<(), String> {
    let ids = asset_ids
        .iter()
        .filter(|id| id.starts_with("editor-mask-"))
        .cloned()
        .collect::<Vec<_>>();
    apply_asset_blob_changes(&[], &ids).await
}

async fn load_stored_asset_payloads(
    asset_ids: &[String],
) -> Result<Vec<(String, StoredAssetPayload)>, String> {
    if asset_ids.is_empty() {
        return Ok(Vec::new());
    }
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
            TransactionMode::ReadOnly,
        )
        .map_err(|error| error.to_string())?;
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let mut stored_values = Vec::with_capacity(asset_ids.len());
    for asset_id in asset_ids {
        let blob_value = blob_store
            .get(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(value) = blob_value {
            let blob = value
                .dyn_into::<Blob>()
                .map_err(|_| format!("图片 {asset_id} 的本地 Blob 数据无效"))?;
            stored_values.push((asset_id.clone(), StoredAssetPayload::Blob(blob)));
            continue;
        }
        let legacy_value = legacy_store
            .get(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(value) = legacy_value {
            let data_url = value
                .as_string()
                .ok_or_else(|| format!("图片 {asset_id} 的旧版本地数据无效"))?;
            stored_values.push((asset_id.clone(), StoredAssetPayload::Legacy(data_url)));
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(stored_values)
}

fn cache_asset_blob(asset_id: &str, blob: &Blob) -> Result<String, String> {
    if let Some(existing) = runtime_asset_object_url(asset_id) {
        return Ok(existing);
    }
    let object_url = web_sys::Url::create_object_url_with_blob(blob)
        .map_err(|error| format!("创建本地图片地址失败：{error:?}"))?;
    ASSET_OBJECT_URLS.with(|urls| {
        urls.borrow_mut()
            .insert(asset_id.to_string(), object_url.clone());
    });
    Ok(object_url)
}

fn replace_cached_asset_blob(asset_id: &str, blob: &Blob) {
    let is_cached = ASSET_OBJECT_URLS.with(|urls| urls.borrow().contains_key(asset_id));
    if !is_cached {
        return;
    }
    let Ok(next_url) = web_sys::Url::create_object_url_with_blob(blob) else {
        revoke_asset_object_url(asset_id);
        return;
    };
    let previous =
        ASSET_OBJECT_URLS.with(|urls| urls.borrow_mut().insert(asset_id.to_string(), next_url));
    if let Some(previous) = previous {
        let _ = web_sys::Url::revoke_object_url(&previous);
    }
}

pub async fn clear_asset_payloads() -> Result<(), String> {
    crate::image_editor::runtime::invalidate_runtime_writes();
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME, STORE_NAME],
            TransactionMode::ReadWrite,
        )
        .map_err(|error| error.to_string())?;
    // 两个 handle 必须在第一次 await 前取得，避免事务在请求间隙自动结束。
    let blob_store = transaction
        .store(ASSET_BLOB_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let legacy_store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let kv = transaction
        .store(STORE_NAME)
        .map_err(|error| error.to_string())?;
    blob_store
        .clear()
        .await
        .map_err(|error| error.to_string())?;
    legacy_store
        .clear()
        .await
        .map_err(|error| error.to_string())?;
    for key in kv
        .get_all_keys(None, None)
        .await
        .map_err(|error| error.to_string())?
    {
        if key.as_string().is_some_and(|key| {
            key.starts_with(EDITOR_STAGING_KEY_PREFIX)
                || key == crate::image_editor::runtime::RUNTIME_KEY
        }) {
            kv.delete(key).await.map_err(|error| error.to_string())?;
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    revoke_all_asset_object_urls();
    Ok(())
}

fn data_url_to_blob(data_url: &str) -> Result<Blob, String> {
    let (header, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "图片 data URL 无效".to_string())?;
    let mime_type = header
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "图片 data URL 类型无效".to_string())?;
    let bytes = BASE64
        .decode(payload)
        .map_err(|error| format!("图片 Base64 解码失败：{error}"))?;
    let byte_array = Uint8Array::from(bytes.as_slice());
    let parts = Array::new();
    parts.push(&byte_array.buffer());
    let options = BlobPropertyBag::new();
    options.set_type(mime_type);
    Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|error| format!("构建本地图片 Blob 失败：{error:?}"))
}

async fn blob_to_data_url(blob: &Blob) -> Result<String, String> {
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(|error| format!("读取本地图片 Blob 失败：{error:?}"))?;
    let bytes = Uint8Array::new(&buffer).to_vec();
    let mime_type = if blob.type_().trim().is_empty() {
        "image/png".to_string()
    } else {
        blob.type_()
    };
    Ok(format!("data:{mime_type};base64,{}", BASE64.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mew_image_shared::LocalTaskRecord;

    use super::*;

    fn running_task(id: &str) -> LocalTaskRecord {
        LocalTaskRecord {
            editing: None,
            id: id.into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "test".into(),
            requested_model: "gpt-image-2".into(),
            reference_asset_ids: Vec::new(),
            conversation: None,
            generation_settings: None,
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            source_gallery_template_id: None,
            status: TaskStatus::Running,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    fn staged_asset(id: &str, task_id: &str) -> ImageAssetRef {
        ImageAssetRef {
            id: id.into(),
            sha256: format!("sha-{id}"),
            mime_type: "image/webp".into(),
            byte_len: 128,
            width: Some(64),
            height: Some(32),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: Some(task_id.into()),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn editor_orphan_cleanup_preserves_task_draft_and_restored_references() {
        let mut state = LocalAppState::default();
        state.threads.clear();
        let mut task = running_task("task");
        task.reference_asset_ids.push("base".into());
        state.tasks.push(task);
        let drafts = HashSet::from(["draft-base".into()]);
        assert!(editor_asset_needs_recovery(
            &state,
            "removed-thread",
            "base"
        ));
        assert!(!editor_asset_is_orphan(&state, &drafts, "base"));
        assert!(!editor_asset_is_orphan(&state, &drafts, "draft-base"));
        assert!(editor_asset_is_orphan(&state, &drafts, "unused"));
        state.assets.push(staged_asset("unused", "task"));
        assert!(!editor_asset_is_orphan(&state, &drafts, "unused"));
    }

    #[test]
    fn editor_recovery_never_resurrects_deleted_assets_or_removed_threads() {
        let mut state = LocalAppState::default();
        let thread_id = state.threads[0].id.clone();
        assert!(editor_asset_needs_recovery(&state, &thread_id, "base"));
        state.assets.push(staged_asset("base", "task"));
        assert!(!editor_asset_needs_recovery(&state, &thread_id, "base"));
        state.assets.clear();
        state.tombstones.push(mew_image_shared::SyncTombstone {
            entity_kind: mew_image_shared::SyncEntityKind::Asset,
            entity_id: "base".into(),
            deleted_at: "now".into(),
        });
        assert!(!editor_asset_needs_recovery(&state, &thread_id, "base"));
        state.tombstones.clear();
        state.threads.clear();
        assert!(!editor_asset_needs_recovery(&state, &thread_id, "base"));
    }

    #[test]
    fn staging_recovery_restores_complete_and_partial_tasks_and_marks_orphans() {
        let mut state = LocalAppState {
            tasks: vec![running_task("complete"), running_task("partial")],
            ..LocalAppState::default()
        };
        let manifests = vec![
            GenerationStagingManifest::new(
                "complete".into(),
                2,
                vec![
                    staged_asset("complete-1", "complete"),
                    staged_asset("complete-2", "complete"),
                ],
                true,
            ),
            GenerationStagingManifest::new(
                "partial".into(),
                3,
                vec![staged_asset("partial-1", "partial")],
                false,
            ),
            GenerationStagingManifest::new(
                "missing-task".into(),
                1,
                vec![staged_asset("orphan-1", "missing-task")],
                false,
            ),
        ];

        let recovery = apply_generation_staging_manifests(&mut state, manifests);

        assert_eq!(recovery.handled_task_ids.len(), 3);
        assert_eq!(recovery.orphan_asset_ids, ["orphan-1"]);
        assert_eq!(state.assets.len(), 3);
        let complete = state
            .tasks
            .iter()
            .find(|task| task.id == "complete")
            .unwrap();
        assert_eq!(complete.status, TaskStatus::Succeeded);
        assert_eq!(complete.result.as_ref().unwrap().images.len(), 2);
        let partial = state
            .tasks
            .iter()
            .find(|task| task.id == "partial")
            .unwrap();
        assert_eq!(partial.status, TaskStatus::Failed);
        assert_eq!(partial.result.as_ref().unwrap().images.len(), 1);
        assert!(
            partial
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("1/3"))
        );
    }
}
