use super::super::*;

pub(crate) fn asset_payload_pairs(assets: &[ImageAssetRef]) -> Vec<(String, String)> {
    assets
        .iter()
        .filter_map(|asset| {
            asset.data_url.as_ref().and_then(|data_url| {
                if data_url.trim().is_empty() {
                    None
                } else {
                    Some((asset.id.clone(), data_url.clone()))
                }
            })
        })
        .collect()
}

pub(crate) fn strip_asset_payloads_for_snapshot(assets: &[ImageAssetRef]) -> Vec<ImageAssetRef> {
    assets
        .iter()
        .cloned()
        .map(|mut asset| {
            asset.data_url = None;
            asset
        })
        .collect()
}

pub(crate) fn merge_asset_payloads(
    assets: &mut [ImageAssetRef],
    payloads: &HashMap<String, String>,
) -> bool {
    let mut changed = false;
    for asset in assets {
        if asset.data_url.is_some() {
            continue;
        }
        if let Some(data_url) = payloads.get(&asset.id) {
            asset.data_url = Some(data_url.clone());
            changed = true;
        }
    }
    changed
}

pub(crate) async fn ensure_asset_payloads_loaded(
    assets_signal: RwSignal<Vec<ImageAssetRef>>,
    asset_ids: &[String],
) -> Result<(), String> {
    if asset_ids.is_empty() {
        return Ok(());
    }
    let mut unique_ids = HashSet::new();
    let missing_asset_ids = assets_signal.with_untracked(|items| {
        asset_ids
            .iter()
            .filter(|asset_id| unique_ids.insert((*asset_id).clone()))
            .filter(|asset_id| {
                items
                    .iter()
                    .find(|asset| asset.id == **asset_id)
                    .map(|asset| {
                        asset
                            .data_url
                            .as_deref()
                            .map(|value| value.trim().is_empty())
                            .unwrap_or(true)
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    if missing_asset_ids.is_empty() {
        return Ok(());
    }
    let payloads = load_asset_payloads(&missing_asset_ids).await?;
    if payloads.is_empty() {
        return Ok(());
    }
    assets_signal.update(|items| {
        let _ = merge_asset_payloads(items, &payloads);
        touch_and_trim_asset_payload_cache(items, asset_ids, true);
    });
    Ok(())
}

pub(crate) fn touch_and_trim_asset_payload_cache(
    assets: &mut [ImageAssetRef],
    touched_ids: &[String],
    protect_touched: bool,
) {
    ASSET_PAYLOAD_LRU.with(|cache| {
        let mut order = cache.borrow_mut();
        let resident_ids = assets
            .iter()
            .filter(|asset| asset.data_url.is_some())
            .map(|asset| asset.id.clone())
            .collect::<HashSet<_>>();
        order.retain(|id| resident_ids.contains(id));
        for asset in assets.iter().filter(|asset| asset.data_url.is_some()) {
            if !order.contains(&asset.id) {
                order.push(asset.id.clone());
            }
        }
        for touched_id in touched_ids {
            if !resident_ids.contains(touched_id) {
                continue;
            }
            order.retain(|id| id != touched_id);
            order.push(touched_id.clone());
        }

        let protected = if protect_touched {
            touched_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        loop {
            let resident_count = assets
                .iter()
                .filter(|asset| asset.data_url.is_some())
                .count();
            let resident_bytes = assets
                .iter()
                .filter(|asset| asset.data_url.is_some())
                .map(|asset| asset.byte_len)
                .sum::<u64>();
            if resident_count <= ASSET_PAYLOAD_CACHE_MAX_ITEMS
                && resident_bytes <= ASSET_PAYLOAD_CACHE_MAX_BYTES
            {
                break;
            }
            let Some(index) = order.iter().position(|id| !protected.contains(id.as_str())) else {
                break;
            };
            let evicted_id = order.remove(index);
            if let Some(asset) = assets.iter_mut().find(|asset| asset.id == evicted_id) {
                asset.data_url = None;
            }
        }
    });
}

pub(crate) fn trim_asset_payload_cache(assets_signal: RwSignal<Vec<ImageAssetRef>>) {
    assets_signal.update(|items| touch_and_trim_asset_payload_cache(items, &[], false));
}

pub(crate) fn schedule_background_task(callback: impl FnOnce() + 'static) {
    let Some(window) = web_sys::window() else {
        callback();
        return;
    };
    let callback = Rc::new(RefCell::new(Some(Box::new(callback) as Box<dyn FnOnce()>)));
    if let Ok(idle_callback) = Reflect::get(
        window.as_ref(),
        &wasm_bindgen::JsValue::from_str("requestIdleCallback"),
    ) {
        if idle_callback.is_function() {
            let idle_callback: Function = idle_callback.unchecked_into();
            let callback_for_idle = callback.clone();
            let idle_closure = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::once(move |_| {
                if let Some(callback) = callback_for_idle.borrow_mut().take() {
                    callback();
                }
            });
            if idle_callback
                .call1(window.as_ref(), idle_closure.as_ref().unchecked_ref())
                .is_ok()
            {
                idle_closure.forget();
                return;
            }
        }
    }
    let callback_for_timeout = callback.clone();
    let timeout_closure = Closure::<dyn FnMut()>::once(move || {
        if let Some(callback) = callback_for_timeout.borrow_mut().take() {
            callback();
        }
    });
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        timeout_closure.as_ref().unchecked_ref(),
        900,
    );
    timeout_closure.forget();
}

pub(crate) fn request_workspace_persist(
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    checkpoint: RwSignal<SyncCheckpoint>,
    tombstones: RwSignal<Vec<SyncTombstone>>,
    scheduled: RwSignal<bool>,
    inflight: RwSignal<bool>,
    pending: RwSignal<bool>,
) {
    pending.set(true);
    if scheduled.get_untracked() || inflight.get_untracked() {
        return;
    }
    scheduled.set(true);
    schedule_background_task(move || {
        scheduled.set(false);
        if inflight.get_untracked() {
            pending.set(true);
            return;
        }
        if !pending.get_untracked() {
            return;
        }
        pending.set(false);
        inflight.set(true);
        let snapshot = snapshot_workspace_state(tasks, threads, assets, checkpoint, tombstones);
        spawn_local(async move {
            let _ = save_workspace_snapshot(&snapshot).await;
            inflight.set(false);
            if pending.get_untracked() {
                request_workspace_persist(
                    tasks, threads, assets, checkpoint, tombstones, scheduled, inflight, pending,
                );
            }
        });
    });
}

pub(crate) fn request_ui_state_persist(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    preferences: RwSignal<AppPreferences>,
    scheduled: RwSignal<bool>,
    inflight: RwSignal<bool>,
    pending: RwSignal<bool>,
) {
    pending.set(true);
    if scheduled.get_untracked() || inflight.get_untracked() {
        return;
    }
    scheduled.set(true);
    schedule_background_task(move || {
        scheduled.set(false);
        if inflight.get_untracked() {
            pending.set(true);
            return;
        }
        if !pending.get_untracked() {
            return;
        }
        pending.set(false);
        inflight.set(true);
        let configs_snapshot = configs.get_untracked();
        let preferences_snapshot = preferences.get_untracked();
        spawn_local(async move {
            let _ = save_ui_state(&configs_snapshot, &preferences_snapshot).await;
            inflight.set(false);
            if pending.get_untracked() {
                request_ui_state_persist(configs, preferences, scheduled, inflight, pending);
            }
        });
    });
}

pub(crate) fn request_payload_flush(
    payload_write_queue: RwSignal<HashMap<String, String>>,
    payload_delete_queue: RwSignal<HashSet<String>>,
    scheduled: RwSignal<bool>,
    inflight: RwSignal<bool>,
    pending: RwSignal<bool>,
) {
    pending.set(true);
    if scheduled.get_untracked() || inflight.get_untracked() {
        return;
    }
    scheduled.set(true);
    schedule_background_task(move || {
        scheduled.set(false);
        if inflight.get_untracked() {
            pending.set(true);
            return;
        }
        if !pending.get_untracked() {
            return;
        }
        let writes = payload_write_queue.with_untracked(|queued| {
            queued
                .iter()
                .map(|(asset_id, data_url)| (asset_id.clone(), data_url.clone()))
                .collect::<Vec<_>>()
        });
        let deletes = payload_delete_queue
            .with_untracked(|queued| queued.iter().cloned().collect::<Vec<_>>());
        if writes.is_empty() && deletes.is_empty() {
            pending.set(false);
            return;
        }
        pending.set(false);
        inflight.set(true);
        spawn_local(async move {
            let _ = apply_asset_payload_changes(&writes, &deletes).await;
            payload_write_queue.update(|queued| {
                for (asset_id, data_url) in &writes {
                    if queued.get(asset_id) == Some(data_url) {
                        queued.remove(asset_id);
                    }
                }
            });
            payload_delete_queue.update(|queued| {
                for asset_id in &deletes {
                    queued.remove(asset_id);
                }
            });
            inflight.set(false);
            if pending.get_untracked()
                || !payload_write_queue.with_untracked(|queued| queued.is_empty())
                || !payload_delete_queue.with_untracked(|queued| queued.is_empty())
            {
                request_payload_flush(
                    payload_write_queue,
                    payload_delete_queue,
                    scheduled,
                    inflight,
                    pending,
                );
            }
        });
    });
}

pub(crate) fn snapshot_local_state(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    preferences: RwSignal<AppPreferences>,
    checkpoint: RwSignal<SyncCheckpoint>,
    tombstones: RwSignal<Vec<SyncTombstone>>,
) -> LocalAppState {
    let mut task_snapshot = tasks.with_untracked(|items| items.clone());
    strip_successful_task_payloads(&mut task_snapshot);
    LocalAppState {
        configs: configs.with_untracked(|items| items.clone()),
        tasks: task_snapshot,
        threads: threads.with_untracked(|items| items.clone()),
        assets: assets.with_untracked(|items| items.clone()),
        preferences: preferences.get_untracked(),
        checkpoint: checkpoint.get_untracked(),
        tombstones: tombstones.get_untracked(),
    }
}

pub(crate) fn snapshot_workspace_state(
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    checkpoint: RwSignal<SyncCheckpoint>,
    tombstones: RwSignal<Vec<SyncTombstone>>,
) -> LocalAppState {
    let mut task_snapshot = tasks.with_untracked(|items| items.clone());
    strip_successful_task_payloads(&mut task_snapshot);
    LocalAppState {
        configs: Vec::new(),
        tasks: task_snapshot,
        threads: threads.with_untracked(|items| items.clone()),
        assets: assets.with_untracked(|items| strip_asset_payloads_for_snapshot(items)),
        preferences: AppPreferences::default(),
        checkpoint: checkpoint.get_untracked(),
        tombstones: tombstones.get_untracked(),
    }
}

pub(crate) fn apply_local_state(
    mut state: LocalAppState,
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    preferences: RwSignal<AppPreferences>,
    checkpoint: RwSignal<SyncCheckpoint>,
    tombstones: RwSignal<Vec<SyncTombstone>>,
) {
    for config in &mut state.configs {
        normalize_api_config(config);
    }
    configs.set(state.configs);
    tasks.set(state.tasks);
    threads.set(state.threads);
    assets.set(state.assets);
    preferences.set(state.preferences);
    checkpoint.set(state.checkpoint);
    tombstones.set(state.tombstones);
}

pub(crate) fn generation_settings_for_rerun(
    task: &LocalTaskRecord,
    config: &EncryptedApiConfig,
) -> GenerationSettingsSnapshot {
    if let Some(settings) = &task.generation_settings {
        return settings.clone();
    }

    let parameters = task
        .result
        .as_ref()
        .map(|result| &result.parameter_snapshot);
    GenerationSettingsSnapshot {
        width: parameters
            .and_then(|value| value.requested_width.or(value.actual_width))
            .unwrap_or(1024),
        height: parameters
            .and_then(|value| value.requested_height.or(value.actual_height))
            .unwrap_or(1024),
        quality: parameters
            .and_then(|value| value.requested_quality.clone())
            .or_else(|| Some("high".into())),
        count: task
            .result
            .as_ref()
            .map(|result| result.images.len() as u32)
            .unwrap_or(1)
            .max(1),
        endpoint_mode: config.endpoint_mode,
        output_format: config.output_format.clone(),
        output_compression: config.output_compression,
        moderation: config.moderation.clone(),
        responses_model: config.responses_model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mew_image_shared::{
        BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, GenerationSettingsSnapshot, ImageAssetRef,
        LocalTaskRecord, ProviderEndpointMode, TaskStatus, now_rfc3339,
    };

    use super::*;
    use crate::{app::ASSET_PAYLOAD_LRU, providers};

    fn test_asset(id: &str) -> ImageAssetRef {
        ImageAssetRef {
            id: id.into(),
            sha256: format!("sha-{id}"),
            mime_type: "image/png".into(),
            byte_len: 4,
            width: Some(1),
            height: Some(1),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn rerun_prefers_historical_generation_settings() {
        let config = providers::default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        let expected = GenerationSettingsSnapshot {
            width: 2048,
            height: 1152,
            quality: Some("medium".into()),
            count: 3,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            output_format: Some("webp".into()),
            output_compression: Some(90),
            moderation: Some("low".into()),
            responses_model: Some("gpt-5.6".into()),
        };
        let task = LocalTaskRecord {
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: config.id.clone(),
            prompt: "test".into(),
            requested_model: "gpt-image-2".into(),
            reference_asset_ids: vec!["asset-1".into()],
            generation_settings: Some(expected.clone()),
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };

        assert_eq!(generation_settings_for_rerun(&task, &config), expected);
    }

    #[test]
    fn asset_payload_lru_evicts_oldest_unprotected_originals() {
        ASSET_PAYLOAD_LRU.with(|cache| cache.borrow_mut().clear());
        let mut assets = (0..8)
            .map(|index| {
                let mut asset = test_asset(&format!("lru-{index}"));
                asset.byte_len = 10 * 1024 * 1024;
                asset.data_url = Some(format!("data:image/png;base64,{index}"));
                asset
            })
            .collect::<Vec<_>>();
        let touched = assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect::<Vec<_>>();

        touch_and_trim_asset_payload_cache(&mut assets, &touched, false);

        assert_eq!(
            assets
                .iter()
                .filter(|asset| asset.data_url.is_some())
                .count(),
            4
        );
        assert!(assets[..4].iter().all(|asset| asset.data_url.is_none()));
        assert!(assets[4..].iter().all(|asset| asset.data_url.is_some()));
    }
}
