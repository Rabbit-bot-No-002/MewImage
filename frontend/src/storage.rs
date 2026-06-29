use std::collections::HashMap;

use mew_image_shared::{AppPreferences, EncryptedApiConfig, LocalAppState};
use rexie::{ObjectStore, Rexie, TransactionMode};
use wasm_bindgen::JsValue;

const DB_NAME: &str = "mew-image-local";
const STORE_NAME: &str = "kv";
const ASSET_STORE_NAME: &str = "asset_payloads";
const SNAPSHOT_KEY: &str = "app_state";
const CONFIGS_KEY: &str = "configs_state";
const PREFERENCES_KEY: &str = "preferences_state";

async fn open_db() -> Result<Rexie, String> {
    Rexie::builder(DB_NAME)
        .version(2)
        .add_object_store(ObjectStore::new(STORE_NAME))
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
    let db = open_db().await?;
    let transaction = db
        .transaction(&[ASSET_STORE_NAME], TransactionMode::ReadWrite)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    for asset_id in payload_deletes {
        store
            .delete(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
    }
    for (asset_id, data_url) in payload_writes {
        store
            .put(
                &JsValue::from_str(data_url),
                Some(&JsValue::from_str(asset_id)),
            )
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
    let db = open_db().await?;
    let transaction = db
        .transaction(&[ASSET_STORE_NAME], TransactionMode::ReadOnly)
        .map_err(|error| error.to_string())?;
    let store = transaction
        .store(ASSET_STORE_NAME)
        .map_err(|error| error.to_string())?;
    let mut loaded = HashMap::with_capacity(asset_ids.len());
    for asset_id in asset_ids {
        let value = store
            .get(JsValue::from_str(asset_id))
            .await
            .map_err(|error| error.to_string())?;
        if let Some(value) = value.and_then(|value| value.as_string()) {
            loaded.insert(asset_id.clone(), value);
        }
    }
    transaction
        .done()
        .await
        .map_err(|error| error.to_string())?;
    Ok(loaded)
}
