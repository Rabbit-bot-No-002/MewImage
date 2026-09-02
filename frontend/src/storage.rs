use std::{cell::RefCell, collections::HashMap};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use js_sys::{Array, Uint8Array};
use mew_image_shared::{AppPreferences, EncryptedApiConfig, LocalAppState};
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
const ASSET_WRITE_BATCH_MAX_BYTES: usize = 32 * 1024 * 1024;

thread_local! {
    static ASSET_OBJECT_URLS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
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

async fn open_db() -> Result<Rexie, String> {
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
    Ok(state)
}

pub async fn save_workspace_snapshot(state: &LocalAppState) -> Result<(), String> {
    let db = open_db().await?;
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

async fn apply_asset_blob_changes(
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
    let db = open_db().await?;
    let transaction = db
        .transaction(
            &[ASSET_BLOB_STORE_NAME, ASSET_STORE_NAME],
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
    blob_store
        .clear()
        .await
        .map_err(|error| error.to_string())?;
    legacy_store
        .clear()
        .await
        .map_err(|error| error.to_string())?;
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
