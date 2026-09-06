use super::super::*;

pub(crate) const PENDING_BLOB_MIGRATION_KEY: &str = "local_pending_blob_migration";
const WORKSPACE_PERSIST_CONFIRM_POLL_MS: u32 = 100;
const WORKSPACE_PERSIST_CONFIRM_MAX_POLLS: usize = 10 * 60 * 1_000 / 100;

pub(crate) fn asset_payload_pairs(assets: &[ImageAssetRef]) -> Vec<(String, String)> {
    assets
        .iter()
        .filter_map(|asset| {
            asset
                .data_url
                .as_ref()
                .filter(|data_url| is_embedded_asset_data_url(data_url))
                .map(|data_url| (asset.id.clone(), data_url.clone()))
        })
        .collect()
}

/// 只有可跨刷新持久化、也能安全发送给上游的 Base64 data URL 才算原图载荷。
/// `blob:` 仅在当前页面生命周期内有效，绝不能进入 IndexedDB 快照或生成请求。
pub(crate) fn is_embedded_asset_data_url(value: &str) -> bool {
    let Some((header, payload)) = value.trim().split_once(',') else {
        return false;
    };
    header.starts_with("data:image/") && header.ends_with(";base64") && !payload.trim().is_empty()
}

pub(crate) fn strip_asset_payloads_for_snapshot(assets: &[ImageAssetRef]) -> Vec<ImageAssetRef> {
    assets
        .iter()
        .map(|asset| ImageAssetRef {
            id: asset.id.clone(),
            sha256: asset.sha256.clone(),
            mime_type: asset.mime_type.clone(),
            byte_len: asset.byte_len,
            width: asset.width,
            height: asset.height,
            created_at: asset.created_at.clone(),
            updated_at: asset.updated_at.clone(),
            // 极少数配额失败场景下，旧快照中的内嵌原图是最后一份可恢复副本。
            // 只有 Blob 写入成功后才允许从工作区快照移除它。
            data_url: asset
                .metadata
                .contains_key(PENDING_BLOB_MIGRATION_KEY)
                .then(|| asset.data_url.clone())
                .flatten()
                .filter(|value| is_embedded_asset_data_url(value)),
            remote_object_key: asset.remote_object_key.clone(),
            remote_url: asset.remote_url.clone(),
            source_task_id: asset.source_task_id.clone(),
            metadata: asset.metadata.clone(),
        })
        .collect()
}

/// 任务记录只保存参数与诊断文本，图片本体统一由 asset payload store 管理。
pub(crate) fn strip_task_payloads(tasks: &mut [LocalTaskRecord]) -> bool {
    let mut changed = false;
    for task in tasks {
        let Some(result) = task.result.as_mut() else {
            continue;
        };
        for image in &mut result.images {
            changed |= image.url.is_some() || image.data_url.is_some();
            image.url = None;
            image.data_url = None;
        }
        changed |= result.raw_response_json.is_some();
        result.raw_response_json = None;
    }
    changed
}

pub(crate) fn merge_asset_payloads(
    assets: &mut [ImageAssetRef],
    payloads: &HashMap<String, String>,
) -> bool {
    let mut changed = false;
    for asset in assets {
        if asset
            .data_url
            .as_deref()
            .map(is_embedded_asset_data_url)
            .unwrap_or(false)
        {
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
                        !asset
                            .data_url
                            .as_deref()
                            .map(is_embedded_asset_data_url)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    if missing_asset_ids.is_empty() {
        return Ok(());
    }
    let mut payloads = load_asset_payloads(&missing_asset_ids).await?;
    let remote_sources = assets_signal.with_untracked(|items| {
        missing_asset_ids
            .iter()
            .filter(|asset_id| !payloads.contains_key(*asset_id))
            .filter_map(|asset_id| {
                items
                    .iter()
                    .find(|asset| asset.id == *asset_id)
                    .and_then(|asset| asset.remote_url.clone())
                    .map(|remote_url| (asset_id.clone(), remote_url))
            })
            .collect::<Vec<_>>()
    });
    for (asset_id, remote_url) in remote_sources {
        let authenticated = remote_url.starts_with("/api/assets/");
        let source = if remote_url.starts_with('/') {
            api_url(&remote_url)
        } else {
            remote_url
        };
        let (bytes, mime_type) = if authenticated {
            fetch_authenticated_image_bytes(&source).await?
        } else {
            fetch_image_bytes(&source).await?
        };
        let data_url = bytes_to_data_url(&bytes, &mime_type);
        // 跨设备首次使用时同步落入 v3 Blob store，后续生成不再重复下载。
        apply_asset_payload_changes(&[(asset_id.clone(), data_url.clone())], &[]).await?;
        payloads.insert(asset_id, data_url);
    }
    let unresolved_ids = assets_signal.with_untracked(|items| {
        missing_asset_ids
            .iter()
            .filter(|asset_id| {
                !payloads.contains_key(*asset_id)
                    && items.iter().any(|asset| asset.id == **asset_id)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    if !unresolved_ids.is_empty() {
        return Err(format!(
            "参考图原文件不可用：{}。请重新同步或重新上传后再生成。",
            unresolved_ids.join("、")
        ));
    }
    if !payloads.is_empty() {
        assets_signal.update(|items| {
            let _ = merge_asset_payloads(items, &payloads);
            touch_and_trim_asset_payload_cache(items, asset_ids, true);
        });
    }
    Ok(())
}

/// 为画廊和预览载入 Blob URL，不把图片再次膨胀成 Base64 字符串。
pub(crate) async fn ensure_asset_display_sources_loaded(
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
                        !asset
                            .data_url
                            .as_deref()
                            .map(is_embedded_asset_data_url)
                            .unwrap_or(false)
                            && runtime_asset_object_url(&asset.id).is_none()
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    if missing_asset_ids.is_empty() {
        assets_signal.update(|items| touch_and_trim_asset_payload_cache(items, asset_ids, true));
        return Ok(());
    }
    let mut loaded = load_asset_object_urls(&missing_asset_ids).await?;
    let remote_sources = assets_signal.with_untracked(|items| {
        missing_asset_ids
            .iter()
            .filter(|asset_id| !loaded.contains_key(*asset_id))
            .filter_map(|asset_id| {
                items
                    .iter()
                    .find(|asset| asset.id == *asset_id)
                    .and_then(|asset| asset.remote_url.clone())
                    .map(|remote_url| (asset_id.clone(), remote_url))
            })
            .collect::<Vec<_>>()
    });
    for (asset_id, remote_url) in remote_sources {
        let authenticated = remote_url.starts_with("/api/assets/");
        let source = if remote_url.starts_with('/') {
            api_url(&remote_url)
        } else {
            remote_url
        };
        let (bytes, mime_type) = if authenticated {
            fetch_authenticated_image_bytes(&source).await?
        } else {
            fetch_image_bytes(&source).await?
        };
        let object_url = store_asset_bytes_for_display(&asset_id, &bytes, &mime_type).await?;
        loaded.insert(asset_id, object_url);
    }
    if loaded.is_empty() {
        return Ok(());
    }
    // Object URL 存在于运行时缓存中；触发一次信号更新让派生画廊和预览重新取源。
    assets_signal.update(|items| touch_and_trim_asset_payload_cache(items, asset_ids, true));
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
            .filter(|asset| {
                asset.data_url.is_some() || runtime_asset_object_url(&asset.id).is_some()
            })
            .map(|asset| asset.id.clone())
            .collect::<HashSet<_>>();
        order.retain(|id| resident_ids.contains(id));
        for asset in assets.iter().filter(|asset| {
            asset.data_url.is_some() || runtime_asset_object_url(&asset.id).is_some()
        }) {
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

        let mut protected = if protect_touched {
            touched_ids.iter().cloned().collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        protected.extend(
            assets
                .iter()
                .filter(|asset| asset.metadata.contains_key(PENDING_BLOB_MIGRATION_KEY))
                .map(|asset| asset.id.clone()),
        );
        loop {
            let resident_count = assets
                .iter()
                .filter(|asset| {
                    asset.data_url.is_some() || runtime_asset_object_url(&asset.id).is_some()
                })
                .count();
            let resident_bytes = assets
                .iter()
                .filter(|asset| {
                    asset.data_url.is_some() || runtime_asset_object_url(&asset.id).is_some()
                })
                .map(|asset| asset.byte_len)
                .sum::<u64>();
            if resident_count <= ASSET_PAYLOAD_CACHE_MAX_ITEMS
                && resident_bytes <= ASSET_PAYLOAD_CACHE_MAX_BYTES
            {
                break;
            }
            let Some(index) = order.iter().position(|id| !protected.contains(id)) else {
                break;
            };
            let evicted_id = order.remove(index);
            if let Some(asset) = assets.iter_mut().find(|asset| asset.id == evicted_id) {
                asset.data_url = None;
            }
            revoke_asset_object_url(&evicted_id);
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
    ) && idle_callback.is_function()
    {
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
    persistence: PersistenceState,
) {
    if !persistence
        .local_state_status
        .with_untracked(LocalStateLoadStatus::is_ready)
    {
        return;
    }
    persistence
        .workspace_persist_requested_revision
        .update(|revision| *revision = revision.saturating_add(1));
    let scheduled = persistence.workspace_persist_scheduled;
    let inflight = persistence.workspace_persist_inflight;
    let pending = persistence.workspace_persist_pending;
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
        let saving_revision = persistence
            .workspace_persist_requested_revision
            .get_untracked();
        let snapshot = snapshot_workspace_state(tasks, threads, assets, checkpoint, tombstones);
        spawn_local(async move {
            match save_workspace_snapshot(&snapshot).await {
                Ok(()) => persistence
                    .workspace_persist_completed_revision
                    .update(|revision| *revision = (*revision).max(saving_revision)),
                Err(_) => {
                    // 元数据保存失败时保持 pending，下一轮继续尝试，不能把失败当作已落盘。
                    pending.set(true);
                }
            }
            inflight.set(false);
            if pending.get_untracked() {
                request_workspace_persist(
                    tasks,
                    threads,
                    assets,
                    checkpoint,
                    tombstones,
                    persistence,
                );
            }
        });
    });
}

pub(crate) fn requested_workspace_persist_revision(persistence: PersistenceState) -> Option<u64> {
    persistence
        .local_state_status
        .with_untracked(LocalStateLoadStatus::is_ready)
        .then(|| {
            persistence
                .workspace_persist_requested_revision
                .get_untracked()
        })
        .filter(|revision| *revision > 0)
}

/// 等待包含指定修订的工作区快照真正提交到 IndexedDB。
/// 超时或本地状态进入阻断态时不确认后端结果，交由服务端 TTL 安全回收。
pub(crate) async fn wait_for_workspace_persist_revision(
    persistence: PersistenceState,
    target_revision: u64,
) -> bool {
    for _ in 0..WORKSPACE_PERSIST_CONFIRM_MAX_POLLS {
        if persistence
            .workspace_persist_completed_revision
            .get_untracked()
            >= target_revision
        {
            return true;
        }
        if !persistence
            .local_state_status
            .with_untracked(LocalStateLoadStatus::is_ready)
        {
            return false;
        }
        gloo_timers::future::TimeoutFuture::new(WORKSPACE_PERSIST_CONFIRM_POLL_MS).await;
    }
    false
}

pub(crate) fn request_ui_state_persist(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    preferences: RwSignal<AppPreferences>,
    persistence: PersistenceState,
) {
    if !persistence
        .local_state_status
        .with_untracked(LocalStateLoadStatus::is_ready)
    {
        return;
    }
    let scheduled = persistence.ui_persist_scheduled;
    let inflight = persistence.ui_persist_inflight;
    let pending = persistence.ui_persist_pending;
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
                request_ui_state_persist(configs, preferences, persistence);
            }
        });
    });
}

pub(crate) fn request_payload_flush(persistence: PersistenceState, status_text: RwSignal<String>) {
    if !persistence
        .local_state_status
        .with_untracked(LocalStateLoadStatus::is_ready)
    {
        return;
    }
    let payload_write_queue = persistence.payload_write_queue;
    let payload_delete_queue = persistence.payload_delete_queue;
    let scheduled = persistence.payload_flush_scheduled;
    let inflight = persistence.payload_flush_inflight;
    let pending = persistence.payload_flush_pending;
    let failures = persistence.payload_flush_failures;
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
        let writes = payload_write_queue.with_untracked(payload_write_batch);
        let deletes = payload_delete_queue
            .with_untracked(|queued| queued.iter().take(256).cloned().collect::<Vec<_>>());
        if writes.is_empty() && deletes.is_empty() {
            pending.set(false);
            return;
        }
        pending.set(false);
        inflight.set(true);
        spawn_local(async move {
            match apply_asset_payload_changes(&writes, &deletes).await {
                Ok(()) => {
                    let recovered_after_failure = failures.get_untracked() > 0;
                    failures.set(0);
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
                    if recovered_after_failure {
                        status_text.set("本地图片保存已恢复，待写入的原图已安全保存。".into());
                    }
                }
                Err(error) => {
                    // 保留失败批次，避免 IndexedDB 临时异常或配额问题造成原图永久丢失。
                    pending.set(true);
                    failures.update(|count| *count = count.saturating_add(1));
                    status_text.set(format!(
                        "本地图片保存失败，系统会自动重试：{error}。请检查浏览器存储配额或释放磁盘空间。"
                    ));
                    inflight.set(false);
                    schedule_payload_flush_retry(persistence, status_text);
                    return;
                }
            }
            inflight.set(false);
            if pending.get_untracked()
                || !payload_write_queue.with_untracked(|queued| queued.is_empty())
                || !payload_delete_queue.with_untracked(|queued| queued.is_empty())
            {
                request_payload_flush(persistence, status_text);
            }
        });
    });
}

fn payload_write_batch(queued: &HashMap<String, String>) -> Vec<(String, String)> {
    const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
    let mut batch = Vec::new();
    let mut total_bytes = 0_usize;
    for (asset_id, data_url) in queued {
        if !batch.is_empty() && total_bytes.saturating_add(data_url.len()) > MAX_BATCH_BYTES {
            break;
        }
        total_bytes = total_bytes.saturating_add(data_url.len());
        batch.push((asset_id.clone(), data_url.clone()));
    }
    batch
}

fn schedule_payload_flush_retry(persistence: PersistenceState, status_text: RwSignal<String>) {
    let scheduled = persistence.payload_flush_scheduled;
    let failures = persistence.payload_flush_failures;
    if scheduled.get_untracked() {
        return;
    }
    const RETRY_DELAYS_MS: [i32; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
    let failure_count = failures.get_untracked().max(1) as usize;
    let delay = RETRY_DELAYS_MS[failure_count
        .saturating_sub(1)
        .min(RETRY_DELAYS_MS.len() - 1)];
    scheduled.set(true);
    spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(delay as u32).await;
        scheduled.set(false);
        request_payload_flush(persistence, status_text);
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
    strip_task_payloads(&mut task_snapshot);
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
    strip_task_payloads(&mut task_snapshot);
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

#[allow(clippy::too_many_arguments)]
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
        background: config.background.clone(),
        moderation: config.moderation.clone(),
        responses_model: config.responses_model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mew_image_shared::{
        BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, GeneratedImageResult, GenerationResult,
        GenerationSettingsSnapshot, ImageAssetRef, LocalTaskRecord, ParameterSnapshot,
        ProviderEndpointMode, TaskStatus, now_rfc3339,
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
            background: Some("transparent".into()),
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
            source_gallery_template_id: None,
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

    #[test]
    fn pending_blob_migration_payload_is_protected_from_snapshot_and_lru_cleanup() {
        ASSET_PAYLOAD_LRU.with(|cache| cache.borrow_mut().clear());
        let mut assets = (0..8)
            .map(|index| {
                let mut asset = test_asset(&format!("migration-{index}"));
                asset.byte_len = 10 * 1024 * 1024;
                asset.data_url = Some(format!("data:image/png;base64,{index}"));
                asset
            })
            .collect::<Vec<_>>();
        assets[0]
            .metadata
            .insert(PENDING_BLOB_MIGRATION_KEY.into(), "true".into());

        let snapshot = strip_asset_payloads_for_snapshot(&assets);
        assert_eq!(snapshot[0].data_url, assets[0].data_url);
        assert!(snapshot[1..].iter().all(|asset| asset.data_url.is_none()));

        let touched = assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect::<Vec<_>>();
        touch_and_trim_asset_payload_cache(&mut assets, &touched, false);

        assert!(assets[0].data_url.is_some());
        assert_eq!(
            assets
                .iter()
                .filter(|asset| asset.data_url.is_some())
                .count(),
            4
        );
    }

    #[test]
    fn blob_urls_are_never_treated_as_persistable_payloads() {
        let mut asset = test_asset("runtime-only");
        asset.data_url = Some("blob:http://127.0.0.1/runtime-only".into());

        assert!(!is_embedded_asset_data_url(
            asset.data_url.as_deref().unwrap()
        ));
        assert!(asset_payload_pairs(std::slice::from_ref(&asset)).is_empty());

        let payloads =
            HashMap::from([(asset.id.clone(), "data:image/png;base64,AAAA".to_string())]);
        assert!(merge_asset_payloads(
            std::slice::from_mut(&mut asset),
            &payloads
        ));
        assert_eq!(
            asset.data_url.as_deref(),
            Some("data:image/png;base64,AAAA")
        );
    }

    #[test]
    fn embedded_image_data_url_requires_base64_payload() {
        assert!(is_embedded_asset_data_url(
            "data:image/webp;base64,UklGRg=="
        ));
        assert!(!is_embedded_asset_data_url("data:image/png;base64,"));
        assert!(!is_embedded_asset_data_url("data:text/plain;base64,QQ=="));
        assert!(!is_embedded_asset_data_url(
            "https://example.test/image.png"
        ));
    }

    #[test]
    fn task_snapshots_strip_large_payloads_for_success_and_failure() {
        let config = providers::default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        let make_task = |id: &str, status| LocalTaskRecord {
            id: id.into(),
            thread_id: "thread-1".into(),
            config_id: config.id.clone(),
            prompt: "test".into(),
            requested_model: "gpt-image-2".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: Some(GenerationResult {
                images: vec![GeneratedImageResult {
                    url: Some("https://example.test/result.png".into()),
                    data_url: Some("data:image/png;base64,AAAA".into()),
                }],
                parameter_snapshot: ParameterSnapshot::default(),
                raw_response_json: Some(serde_json::json!({ "image": "large" })),
            }),
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            source_gallery_template_id: None,
            status,
            error_message: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };
        let mut tasks = vec![
            make_task("succeeded", TaskStatus::Succeeded),
            make_task("failed", TaskStatus::Failed),
        ];

        assert!(strip_task_payloads(&mut tasks));
        for result in tasks.iter().filter_map(|task| task.result.as_ref()) {
            assert!(result.raw_response_json.is_none());
            assert!(
                result
                    .images
                    .iter()
                    .all(|image| image.url.is_none() && image.data_url.is_none())
            );
        }
    }
}
