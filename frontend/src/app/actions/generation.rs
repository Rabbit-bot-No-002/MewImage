use super::super::*;

struct PreparedGeneratedImage {
    assets: Vec<ImageAssetRef>,
    visible_asset_id: String,
    local_background_error: Option<String>,
}

fn submitted_reference_ids(
    assets: &[ImageAssetRef],
    selected_ids: &[String],
    continuation_asset_id: Option<&str>,
) -> Vec<String> {
    let mut ordered_ids = Vec::with_capacity(MAX_GENERATION_REFERENCE_ASSETS);
    let mut seen_ids = HashSet::new();
    let candidates = continuation_asset_id
        .into_iter()
        .chain(selected_ids.iter().map(String::as_str));

    for asset_id in candidates {
        if ordered_ids.len() >= MAX_GENERATION_REFERENCE_ASSETS || !seen_ids.insert(asset_id) {
            continue;
        }
        let is_usable = assets.iter().any(|asset| {
            asset.id == asset_id
                && !asset.metadata.contains_key("mask_base_asset_id")
                && !is_theme_background(asset)
        });
        if is_usable {
            ordered_ids.push(asset_id.to_string());
        }
    }
    ordered_ids
}

fn generation_byte_budget(device_memory_gib: Option<f64>) -> u64 {
    let Some(device_memory_gib) =
        device_memory_gib.filter(|value| value.is_finite() && *value > 0.0)
    else {
        return DEFAULT_ACTIVE_GENERATION_BYTE_BUDGET;
    };
    ((device_memory_gib * 64.0 * 1024.0 * 1024.0).round() as u64).clamp(
        MIN_ACTIVE_GENERATION_BYTE_BUDGET,
        MAX_ACTIVE_GENERATION_BYTE_BUDGET,
    )
}

fn browser_generation_byte_budget() -> u64 {
    let device_memory_gib = web_sys::window()
        .and_then(|window| {
            Reflect::get(
                window.navigator().as_ref(),
                &wasm_bindgen::JsValue::from_str("deviceMemory"),
            )
            .ok()
        })
        .and_then(|value| value.as_f64());
    generation_byte_budget(device_memory_gib)
}

fn estimated_generation_task_bytes(
    assets: &[ImageAssetRef],
    reference_ids: &[String],
    width: u32,
    height: u32,
    count: u32,
) -> u64 {
    let mut seen_ids = HashSet::new();
    let reference_bytes = reference_ids
        .iter()
        .filter(|asset_id| seen_ids.insert(asset_id.as_str()))
        .filter_map(|asset_id| assets.iter().find(|asset| asset.id == *asset_id))
        .map(|asset| asset.byte_len)
        .fold(0_u64, u64::saturating_add);
    let output_bytes = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        .saturating_mul(u64::from(count.max(1)));
    // 参考图会在 Blob/Base64/重编码间短暂并存，结果也包含解码与保存缓冲。
    reference_bytes
        .saturating_mul(2)
        .saturating_add(output_bytes.saturating_mul(2))
        .saturating_add(GENERATION_TASK_FIXED_BYTE_OVERHEAD)
}

fn try_reserve_generation_bytes(
    runtimes: &mut HashMap<String, ActiveGenerationRuntime>,
    task_id: &str,
    requested_bytes: u64,
    budget_bytes: u64,
) -> bool {
    let reserved_bytes = runtimes
        .values()
        .map(|runtime| runtime.reserved_bytes)
        .fold(0_u64, u64::saturating_add);
    let Some(runtime) = runtimes.get_mut(task_id) else {
        return false;
    };
    if runtime.reserved_bytes > 0 {
        return true;
    }
    if reserved_bytes.saturating_add(requested_bytes) > budget_bytes {
        runtime.progress_label = "等待浏览器内存预算".into();
        return false;
    }
    runtime.reserved_bytes = requested_bytes;
    runtime.progress_label = "正在加载参考图".into();
    true
}

#[allow(clippy::too_many_arguments)]
async fn prepare_generated_image(
    image: &mew_image_shared::GeneratedImageResult,
    result_index: usize,
    task_id: &str,
    fallback_width: u32,
    fallback_height: u32,
    local_background: bool,
    output_format: Option<&str>,
    output_compression: Option<u8>,
) -> Result<PreparedGeneratedImage, String> {
    let (data_url, remote_url, bytes, mime_type) = match (&image.data_url, &image.url) {
        (Some(data_url), _) => {
            let (mime_type, bytes) = decode_browser_data_url(data_url)?;
            (data_url.clone(), None, bytes, mime_type)
        }
        (None, Some(url)) => {
            let (bytes, mime_type) = fetch_image_bytes(url).await?;
            let data_url = bytes_to_data_url(&bytes, &mime_type);
            (data_url, Some(url.clone()), bytes, mime_type)
        }
        (None, None) => return Err("上游结果缺少图像数据。".into()),
    };
    let (width, height) = load_image_dimensions(&data_url)
        .await
        .unwrap_or((fallback_width, fallback_height));
    let now = now_rfc3339();
    let mut source = ImageAssetRef {
        id: new_id(),
        sha256: sha256_hex(&bytes),
        mime_type,
        byte_len: bytes.len() as u64,
        width: Some(width),
        height: Some(height),
        created_at: now.clone(),
        updated_at: now,
        data_url: Some(data_url.clone()),
        remote_object_key: None,
        remote_url,
        source_task_id: Some(task_id.to_string()),
        metadata: BTreeMap::new(),
    };

    if !local_background {
        add_generated_thumbnail(&mut source).await;
        return Ok(PreparedGeneratedImage {
            visible_asset_id: source.id.clone(),
            assets: vec![source],
            local_background_error: None,
        });
    }

    match remove_keyed_background_from_data_url(&data_url, output_format, output_compression).await
    {
        Ok(output) => {
            let now = now_rfc3339();
            let mut result = ImageAssetRef {
                id: new_id(),
                sha256: sha256_hex(&output.bytes),
                mime_type: output.mime_type,
                byte_len: output.bytes.len() as u64,
                width: Some(output.width),
                height: Some(output.height),
                created_at: now.clone(),
                updated_at: now,
                data_url: Some(output.data_url),
                remote_object_key: None,
                remote_url: None,
                source_task_id: Some(task_id.to_string()),
                metadata: BTreeMap::new(),
            };
            set_local_background_metadata(
                &mut result,
                LOCAL_BACKGROUND_ROLE_RESULT,
                result_index,
                Some(output.key_color),
                None,
            );
            add_generated_thumbnail(&mut result).await;
            Ok(PreparedGeneratedImage {
                visible_asset_id: result.id.clone(),
                // 去背成功后不再持久化纯色原图，避免同一结果占用双份空间。
                assets: vec![result],
                local_background_error: None,
            })
        }
        Err(error) => {
            set_local_background_metadata(
                &mut source,
                LOCAL_BACKGROUND_ROLE_FALLBACK,
                result_index,
                None,
                Some(&error),
            );
            add_generated_thumbnail(&mut source).await;
            Ok(PreparedGeneratedImage {
                visible_asset_id: source.id.clone(),
                assets: vec![source],
                local_background_error: Some(error),
            })
        }
    }
}

fn set_local_background_metadata(
    asset: &mut ImageAssetRef,
    role: &str,
    result_index: usize,
    key_color: Option<&str>,
    error: Option<&str>,
) {
    asset
        .metadata
        .insert(LOCAL_BACKGROUND_ROLE_KEY.into(), role.into());
    asset.metadata.insert(
        LOCAL_BACKGROUND_RESULT_INDEX_KEY.into(),
        result_index.to_string(),
    );
    if let Some(key_color) = key_color {
        asset
            .metadata
            .insert(LOCAL_BACKGROUND_KEY_COLOR_KEY.into(), key_color.into());
    }
    if let Some(error) = error {
        asset
            .metadata
            .insert(LOCAL_BACKGROUND_ERROR_KEY.into(), error.into());
    }
}

async fn add_generated_thumbnail(asset: &mut ImageAssetRef) {
    if let Ok(thumbnail) = thumbnail_data_url_from_asset(asset, THUMBNAIL_MAX_EDGE).await {
        asset
            .metadata
            .insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
    }
}

fn generation_provider_allowed(provider_kind: ProviderKind, user_status: Option<&str>) -> bool {
    provider_kind != ProviderKind::CustomHttp || user_status == Some("approved")
}

pub(crate) fn build_generation_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    commit_current_thread_draft: impl Fn() + Copy + Send + Sync + 'static,
) -> (
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();
    let persistence = expect_context::<PersistenceState>();
    let derived = expect_context::<AppDerived>();
    let configs = workspace.configs;
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let tombstones = workspace.tombstones;
    let templates = workspace.templates;
    let current_thread_id = workspace.current_thread_id;
    let current_config_id = workspace.current_config_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let continuation_asset_id = composer.continuation_asset_id;
    let draft_prompt = composer.draft_prompt;
    let draft_prompt_ref = composer.draft_prompt_ref;
    let custom_width = composer.custom_width;
    let custom_height = composer.custom_height;
    let resolution_mode = composer.resolution_mode;
    let resolution_group = composer.resolution_group;
    let aspect_ratio = composer.aspect_ratio;
    let effective_custom_aspect_ratio = composer.effective_custom_aspect_ratio;
    let quality = composer.quality;
    let count = composer.count;
    let status_text = composer.status_text;
    let queue_mode_enabled = composer.queue_mode_enabled;
    let active_generation_ids = composer.active_generation_ids;
    let cancelled_generation_ids = composer.cancelled_generation_ids;
    let generation_runtimes = composer.generation_runtimes;
    let foreground_generation_task_id = composer.foreground_generation_task_id;
    let generating = composer.generating;
    let show_settings = ui.show_settings;
    let gallery_page = ui.gallery_page;
    let current_config = derived.current_config;
    let auth_user = account.auth_user;

    let run_generation = move || {
        let queued_submission = queue_mode_enabled.get_untracked();
        let active_count = active_generation_ids.with_untracked(HashSet::len);
        if active_count >= MAX_ACTIVE_GENERATION_TASKS {
            status_text.set(format!(
                "当前已有 {MAX_ACTIVE_GENERATION_TASKS} 个活动任务，请等待完成或停止部分任务后再提交。"
            ));
            return;
        }
        if !queued_submission && foreground_generation_task_id.get_untracked().is_some() {
            status_text.set("当前普通生成任务尚未结束，请等待完成或先停止任务。".into());
            return;
        }
        let Some(config) = current_config.get_untracked() else {
            status_text.set("请先在设置中准备一个服务商配置。".into());
            return;
        };
        let provider_allowed = auth_user.with_untracked(|user| {
            generation_provider_allowed(
                config.provider_kind,
                user.as_ref().map(|user| user.status.as_str()),
            )
        });
        if !provider_allowed {
            status_text.set("CustomHttp 仅限已审批账号".into());
            return;
        }
        if config
            .api_key_plaintext
            .clone()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            status_text.set("请先在设置中填写 API Key。".into());
            show_settings.set(true);
            return;
        }
        let prompt = draft_prompt.get_untracked();
        let prompt = draft_prompt_ref
            .get()
            .map(|textarea: HtmlTextAreaElement| textarea.value())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(prompt);
        if prompt.trim().is_empty() {
            status_text.set("请输入提示词后再开始生成。".into());
            return;
        }
        let local_background = local_background_enabled(&config);
        let effective_prompt = if local_background {
            build_local_background_prompt(&prompt)
        } else {
            prompt.clone()
        };
        prepare_generation_notification_audio();
        let thread_id = current_thread_id.get_untracked();
        let template = templates
            .get_untracked()
            .into_iter()
            .find(|template| template.id == config.provider_template_id)
            .unwrap_or_else(ProviderTemplate::builtin_openai);
        commit_current_thread_draft();
        let selected_ids = selected_reference_ids.get_untracked();
        let continuation_id = continuation_asset_id.get_untracked();
        let submitted_reference_ids = assets.with_untracked(|items| {
            submitted_reference_ids(items, &selected_ids, continuation_id.as_deref())
        });
        let reference_size = assets.with_untracked(|items| {
            submitted_reference_ids.iter().find_map(|asset_id| {
                items
                    .iter()
                    .find(|asset| asset.id == *asset_id)
                    .and_then(|asset| asset.width.zip(asset.height))
            })
        });
        let (resolved_width, resolved_height) = resolve_dimensions_from_reference_size(
            resolution_mode.get_untracked().as_str(),
            resolution_group.get_untracked().as_str(),
            aspect_ratio.get_untracked().as_str(),
            effective_custom_aspect_ratio.get_untracked().as_str(),
            custom_width.get_untracked(),
            custom_height.get_untracked(),
            reference_size,
        );
        custom_width.set(resolved_width);
        custom_height.set(resolved_height);
        let quality_value = quality.get_untracked();
        let count_value = count.get_untracked();
        let task_estimated_bytes = assets.with_untracked(|items| {
            estimated_generation_task_bytes(
                items,
                &submitted_reference_ids,
                resolved_width,
                resolved_height,
                count_value,
            )
        });
        let generation_byte_budget = browser_generation_byte_budget();
        let Ok(abort_controller) = web_sys::AbortController::new() else {
            status_text.set("当前浏览器无法创建请求中止控制器。".into());
            return;
        };
        let abort_signal = abort_controller.signal();

        let task_id = new_id();
        let dependency_asset_ids = submitted_reference_ids.iter().cloned().collect();
        generation_runtimes.update(|items| {
            items.insert(
                task_id.clone(),
                ActiveGenerationRuntime {
                    abort_controller,
                    dependency_asset_ids,
                    thread_id: thread_id.clone(),
                    progress_label: "等待浏览器资源".into(),
                    reserved_bytes: 0,
                },
            );
        });
        active_generation_ids.update(|items| {
            items.insert(task_id.clone());
        });
        if !queued_submission {
            foreground_generation_task_id.set(Some(task_id.clone()));
        }
        generating.set(true);
        gallery_page.set(1);
        let active_count = active_generation_ids.with_untracked(HashSet::len);
        status_text.set(if queued_submission {
            format!("任务已提交，当前有 {active_count} 个任务等待结果。")
        } else {
            "正在提交后台生成任务，长耗时请求会自动轮询结果……".into()
        });

        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                thread.draft_prompt = prompt.clone();
                thread.updated_at = now_rfc3339();
                if !thread.task_ids.contains(&task_id) {
                    thread.task_ids.push(task_id.clone());
                }
                if thread.title == "新的会话" {
                    thread.title = summarize_prompt(&prompt);
                }
            }
        });
        tasks.update(|items| {
            items.push(LocalTaskRecord {
                id: task_id.clone(),
                thread_id: thread_id.clone(),
                config_id: config.id.clone(),
                prompt: prompt.clone(),
                requested_model: config.model.clone(),
                reference_asset_ids: submitted_reference_ids.clone(),
                generation_settings: Some(GenerationSettingsSnapshot {
                    width: resolved_width,
                    height: resolved_height,
                    quality: Some(quality_value.clone()),
                    count: count_value,
                    endpoint_mode: config.endpoint_mode,
                    output_format: config.output_format.clone(),
                    output_compression: config.output_compression,
                    background: config.background.clone(),
                    moderation: config.moderation.clone(),
                    responses_model: config.responses_model.clone(),
                }),
                result: None,
                favorite: false,
                favorite_folder_id: None,
                detached_from_thread: false,
                source_gallery_template_id: None,
                status: TaskStatus::Running,
                error_message: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            });
        });
        persist_state();

        let tasks_signal = tasks;
        let assets_signal = assets;
        let status_signal = status_text;
        let generating_signal = generating;
        let active_generation_ids_signal = active_generation_ids;
        let cancelled_generation_ids_signal = cancelled_generation_ids;
        let generation_runtimes_signal = generation_runtimes;
        let foreground_generation_task_id_signal = foreground_generation_task_id;
        let continuation_signal = continuation_asset_id;
        let threads_signal = threads;
        let tombstones_signal = tombstones;
        let persist = persist_state;
        let reference_ids_for_request = submitted_reference_ids;
        spawn_local(async move {
            let finish_runtime = || {
                // 删除运行时记录即释放该任务持有的浏览器字节预算。
                generation_runtimes_signal.update(|items| {
                    items.remove(&task_id);
                });
                cancelled_generation_ids_signal.update(|items| {
                    items.remove(&task_id);
                });
                active_generation_ids_signal.update(|items| {
                    items.remove(&task_id);
                });
                foreground_generation_task_id_signal.update(|current| {
                    if current.as_deref() == Some(task_id.as_str()) {
                        *current = None;
                    }
                });
                let remaining = active_generation_ids_signal.with_untracked(HashSet::len);
                generating_signal.set(remaining > 0);
                remaining
            };
            let finish_cancelled = || {
                if !cancelled_generation_ids_signal.with_untracked(|items| items.contains(&task_id))
                {
                    return false;
                }
                tasks_signal.update(|items| items.retain(|task| task.id != task_id));
                threads_signal.update(|items| {
                    if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                        thread.task_ids.retain(|id| id != &task_id);
                        thread.updated_at = now_rfc3339();
                    }
                });
                record_sync_tombstones(
                    tombstones_signal,
                    [(SyncEntityKind::Task, task_id.clone())],
                );
                persist();
                let remaining = finish_runtime();
                status_signal.set(if remaining == 0 {
                    "生成任务已停止。".into()
                } else {
                    format!("生成任务已停止，仍有 {remaining} 个任务等待结果。")
                });
                true
            };

            if task_estimated_bytes > generation_byte_budget {
                let error = format!(
                    "该任务预计需要 {} 浏览器内存，超过当前设备单任务预算 {}。请减少参考图、输出数量或分辨率。",
                    format_byte_size(task_estimated_bytes),
                    format_byte_size(generation_byte_budget),
                );
                tasks_signal.update(|items| {
                    if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                        task.status = TaskStatus::Failed;
                        task.updated_at = now_rfc3339();
                        task.error_message = Some(error.clone());
                    }
                });
                persist();
                let remaining = finish_runtime();
                status_signal.set(if remaining == 0 {
                    format!("生成未开始：{error}")
                } else {
                    format!("生成未开始：{error}；仍有 {remaining} 个任务等待结果。")
                });
                play_generation_notification(false);
                return;
            }

            // 等待卡可以先展示，但获得预算前不加载或克隆参考图原文件。
            loop {
                if finish_cancelled() {
                    return;
                }
                let mut acquired = false;
                generation_runtimes_signal.update(|items| {
                    acquired = try_reserve_generation_bytes(
                        items,
                        &task_id,
                        task_estimated_bytes,
                        generation_byte_budget,
                    );
                });
                if acquired {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(200).await;
            }

            let payload_result =
                ensure_asset_payloads_loaded(assets_signal, &reference_ids_for_request).await;
            if finish_cancelled() {
                return;
            }
            if let Err(error) = payload_result {
                tasks_signal.update(|items| {
                    if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                        task.status = TaskStatus::Failed;
                        task.updated_at = now_rfc3339();
                        task.error_message = Some(error.clone());
                    }
                });
                persist();
                let remaining = finish_runtime();
                status_signal.set(if remaining == 0 {
                    format!("生成失败：{error}")
                } else {
                    format!("生成失败：{error}；仍有 {remaining} 个任务等待结果。")
                });
                play_generation_notification(false);
                return;
            }
            let references = assets_signal.with_untracked(|items| {
                selected_reference_assets(items, &reference_ids_for_request)
            });
            generation_runtimes_signal.update(|items| {
                if let Some(runtime) = items.get_mut(&task_id) {
                    runtime.progress_label = "等待上游结果".into();
                }
            });
            let request = mew_image_shared::GenerationRequest {
                prompt: effective_prompt,
                model: config.model.clone(),
                width: resolved_width,
                height: resolved_height,
                quality: Some(quality_value),
                count: count_value,
                endpoint_mode: config.endpoint_mode,
                reference_assets: references,
            };
            trim_asset_payload_cache(assets_signal);
            let generation_result =
                generate_with_strategy(&template, &config, &request, Some(&abort_signal)).await;
            // 请求结束后立即释放其中克隆的参考图 payload，给结果解码和本地保存腾出空间。
            drop(request);
            if cancelled_generation_ids_signal.with_untracked(|items| items.contains(&task_id)) {
                if let Ok(execution) = &generation_result {
                    crate::providers::remove_proxy_generation_jobs(
                        execution.pending_proxy_poll_urls.clone(),
                    );
                }
                let _ = finish_cancelled();
                return;
            }
            match generation_result {
                Ok(execution) => {
                    let mut result = execution.result;
                    let used_proxy = execution.used_proxy;
                    let pending_proxy_poll_urls = execution.pending_proxy_poll_urls;
                    let upstream_result_count = result.images.len();
                    let mut produced_assets = Vec::new();
                    let mut visible_asset_ids = Vec::new();
                    let mut asset_build_errors = Vec::new();
                    let mut local_background_errors = Vec::new();
                    for (index, image) in result.images.iter().enumerate() {
                        if cancelled_generation_ids_signal
                            .with_untracked(|items| items.contains(&task_id))
                        {
                            break;
                        }
                        if local_background {
                            let progress_label =
                                format!("本地去背 {}/{}", index + 1, upstream_result_count);
                            generation_runtimes_signal.update(|items| {
                                if let Some(runtime) = items.get_mut(&task_id) {
                                    runtime.progress_label = progress_label.clone();
                                }
                            });
                            if !queued_submission {
                                status_signal.set(format!("{progress_label}……"));
                            }
                            gloo_timers::future::TimeoutFuture::new(0).await;
                        }
                        match prepare_generated_image(
                            image,
                            index,
                            &task_id,
                            resolved_width,
                            resolved_height,
                            local_background,
                            config.output_format.as_deref(),
                            config.output_compression,
                        )
                        .await
                        {
                            Ok(prepared) => {
                                if let Some(error) = prepared.local_background_error {
                                    local_background_errors.push(format!(
                                        "第 {} 张：{}",
                                        index + 1,
                                        error
                                    ));
                                }
                                visible_asset_ids.push(prepared.visible_asset_id);
                                produced_assets.extend(prepared.assets);
                            }
                            Err(error) => asset_build_errors.push(format!(
                                "第 {} 张结果保存失败：{}",
                                index + 1,
                                error
                            )),
                        }
                        if local_background {
                            gloo_timers::future::TimeoutFuture::new(0).await;
                        }
                    }
                    if cancelled_generation_ids_signal
                        .with_untracked(|items| items.contains(&task_id))
                    {
                        crate::providers::remove_proxy_generation_jobs(pending_proxy_poll_urls);
                        let _ = finish_cancelled();
                        return;
                    }
                    if produced_assets.is_empty() {
                        let upstream_count = result
                            .images
                            .iter()
                            .filter(|image| {
                                image
                                    .data_url
                                    .as_deref()
                                    .map(|value| !value.trim().is_empty())
                                    .unwrap_or(false)
                                    || image
                                        .url
                                        .as_deref()
                                        .map(|value| !value.trim().is_empty())
                                        .unwrap_or(false)
                            })
                            .count();
                        let error = if upstream_count == 0 {
                            "上游没有返回任何可用图片结果。".to_string()
                        } else if !asset_build_errors.is_empty() {
                            format!(
                                "上游返回了结果，但没有任何图片成功写入本地。{}",
                                asset_build_errors.join("；")
                            )
                        } else {
                            "上游结果未能落成本地可用图片，可能是网络、尺寸或响应异常导致。"
                                .to_string()
                        };
                        let mut diagnostic_result = Some(result);
                        tasks_signal.update(|items| {
                            if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                                task.status = TaskStatus::Failed;
                                task.updated_at = now_rfc3339();
                                task.result = diagnostic_result.take();
                                task.error_message = Some(error.clone());
                                strip_task_payloads(std::slice::from_mut(task));
                            }
                        });
                        persist();
                        crate::providers::remove_proxy_generation_jobs(pending_proxy_poll_urls);
                        let remaining = finish_runtime();
                        status_signal.set(if remaining == 0 {
                            format!("生成失败：{error}")
                        } else {
                            format!("生成失败：{error}；仍有 {remaining} 个任务等待结果。")
                        });
                        play_generation_notification(false);
                        return;
                    }
                    let (actual_width, actual_height) = produced_assets
                        .iter()
                        .filter(|asset| visible_asset_ids.contains(&asset.id))
                        .find_map(|asset| asset.width.zip(asset.height))
                        .unwrap_or((resolved_width, resolved_height));
                    let first_generated_id = visible_asset_ids.first().cloned();
                    let produced_payloads = asset_payload_pairs(&produced_assets);
                    for asset in &mut produced_assets {
                        asset.data_url = None;
                    }
                    let produced_asset_ids = produced_assets
                        .iter()
                        .map(|asset| asset.id.clone())
                        .collect::<Vec<_>>();
                    result.parameter_snapshot.actual_width = Some(actual_width);
                    result.parameter_snapshot.actual_height = Some(actual_height);
                    for image in &mut result.images {
                        image.url = None;
                        image.data_url = None;
                    }
                    result.raw_response_json = None;

                    // 结果未落盘前任务始终保持 Running；空间释放后会自动恢复保存，
                    // 避免一次临时配额错误把已经生成的图片永久标记为失败。
                    const SAVE_RETRY_DELAYS_MS: [u32; 7] =
                        [500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
                    let mut save_failures = 0_usize;
                    loop {
                        if cancelled_generation_ids_signal
                            .with_untracked(|items| items.contains(&task_id))
                        {
                            crate::providers::remove_proxy_generation_jobs(pending_proxy_poll_urls);
                            let _ = finish_cancelled();
                            return;
                        }
                        match apply_asset_payload_changes(&produced_payloads, &[]).await {
                            Ok(()) => break,
                            Err(error) => {
                                save_failures = save_failures.saturating_add(1);
                                let retry_delay = SAVE_RETRY_DELAYS_MS[save_failures
                                    .saturating_sub(1)
                                    .min(SAVE_RETRY_DELAYS_MS.len() - 1)];
                                let progress_label =
                                    format!("结果已生成，等待本地保存（重试 {save_failures}）");
                                generation_runtimes_signal.update(|items| {
                                    if let Some(runtime) = items.get_mut(&task_id) {
                                        runtime.progress_label = progress_label.clone();
                                    }
                                });
                                if !queued_submission {
                                    status_signal.set(format!(
                                        "{progress_label}：{error}。可释放浏览器存储空间，任务会自动继续。"
                                    ));
                                }

                                // 分段等待，使“停止任务”无需等满 30 秒退避周期。
                                let mut remaining_delay = retry_delay;
                                while remaining_delay > 0 {
                                    let slice = remaining_delay.min(250);
                                    gloo_timers::future::TimeoutFuture::new(slice).await;
                                    remaining_delay -= slice;
                                    if cancelled_generation_ids_signal
                                        .with_untracked(|items| items.contains(&task_id))
                                    {
                                        crate::providers::remove_proxy_generation_jobs(
                                            pending_proxy_poll_urls,
                                        );
                                        let _ = finish_cancelled();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    if cancelled_generation_ids_signal
                        .with_untracked(|items| items.contains(&task_id))
                    {
                        let _ = apply_asset_payload_changes(&[], &produced_asset_ids).await;
                        crate::providers::remove_proxy_generation_jobs(pending_proxy_poll_urls);
                        let _ = finish_cancelled();
                        return;
                    }
                    drop(produced_payloads);
                    assets_signal.update(|items| {
                        items.extend(produced_assets);
                        touch_and_trim_asset_payload_cache(items, &produced_asset_ids, false);
                    });
                    let local_background_error_message = (!local_background_errors.is_empty())
                        .then(|| local_background_errors.join("；"));
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            task.status = TaskStatus::Succeeded;
                            task.updated_at = now_rfc3339();
                            task.result = Some(result);
                            task.error_message = local_background_error_message.clone();
                            strip_task_payloads(std::slice::from_mut(task));
                        }
                    });
                    if !queued_submission {
                        continuation_signal.set(first_generated_id);
                    }
                    persist();
                    // 原图和元数据使用两个独立事务；必须等包含成功任务的工作区修订真正落盘，
                    // 才能确认删除服务端结果，避免崩溃窗口留下无法恢复的孤儿 Blob。
                    if !pending_proxy_poll_urls.is_empty()
                        && let Some(revision) = requested_workspace_persist_revision(persistence)
                    {
                        spawn_local(async move {
                            if wait_for_workspace_persist_revision(persistence, revision).await {
                                crate::providers::remove_proxy_generation_jobs(
                                    pending_proxy_poll_urls,
                                );
                            }
                        });
                    }
                    let completion_message =
                        if local_background && !local_background_errors.is_empty() {
                            let mut detail = local_background_errors.join("；");
                            if !asset_build_errors.is_empty() {
                                detail = format!("{detail}；{}", asset_build_errors.join("；"));
                            }
                            format!(
                                "生成完成，其中 {}/{} 张本地去背景失败，已保留原图。{}",
                                local_background_errors.len(),
                                upstream_result_count,
                                detail
                            )
                        } else if local_background && !asset_build_errors.is_empty() {
                            format!(
                                "本地去背景完成，但有 {} 张上游结果未能保存。{}",
                                asset_build_errors.len(),
                                asset_build_errors.join("；")
                            )
                        } else if local_background {
                            if queued_submission {
                                "队列任务完成，已在浏览器本地去除背景。".into()
                            } else {
                                "生成完成，已在浏览器本地去除背景，并自动进入连续修改模式。".into()
                            }
                        } else if !asset_build_errors.is_empty() {
                            format!(
                                "生成完成，但有 {} 张结果未能保存到本地。{}",
                                asset_build_errors.len(),
                                asset_build_errors.join("；")
                            )
                        } else if queued_submission {
                            "队列任务生成完成。".into()
                        } else if used_proxy {
                            "生成完成，已自动进入连续修改模式。".into()
                        } else {
                            "生成完成，已自动进入连续修改模式，结果已写入当前会话。".into()
                        };
                    let remaining = finish_runtime();
                    status_signal.set(if remaining == 0 {
                        completion_message
                    } else {
                        format!("{completion_message}仍有 {remaining} 个任务等待结果。")
                    });
                    play_generation_notification(true);
                }
                Err(error) => {
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            task.status = TaskStatus::Failed;
                            task.updated_at = now_rfc3339();
                            task.error_message = Some(error.clone());
                        }
                    });
                    persist();
                    let remaining = finish_runtime();
                    status_signal.set(if remaining == 0 {
                        format!("生成失败：{error}")
                    } else {
                        format!("生成失败：{error}；仍有 {remaining} 个任务等待结果。")
                    });
                    play_generation_notification(false);
                }
            }
        });
    };

    let rerun_task = move |task_id: String| {
        let Some(task) = tasks
            .get_untracked()
            .into_iter()
            .find(|task| task.id == task_id)
        else {
            status_text.set("未找到需要重新生成的历史任务。".into());
            return;
        };
        let selected_config_id = current_config_id.get_untracked();
        let Some(config) = configs
            .get_untracked()
            .into_iter()
            .find(|config| config.id == selected_config_id)
        else {
            status_text.set("请先选择当前要用于重新生成的服务商配置。".into());
            return;
        };
        let settings = generation_settings_for_rerun(&task, &config);
        let thread_list = threads.get_untracked();
        let target_thread_id =
            task_target_thread_id(&task, &thread_list, &current_thread_id.get_untracked());

        current_thread_id.set(target_thread_id.clone());
        draft_prompt.set(task.prompt.clone());
        if let Some(textarea) = draft_prompt_ref.get() {
            textarea.set_value(&task.prompt);
        }
        selected_reference_ids.set(task.reference_asset_ids.clone());
        continuation_asset_id.set(None);
        reference_menu_asset_id.set(None);
        quality.set(settings.quality.clone().unwrap_or_else(|| "high".into()));
        count.set(settings.count.clamp(1, 4));
        resolution_mode.set("custom".into());
        custom_width.set(settings.width);
        custom_height.set(settings.height);
        threads.update(|items| {
            if let Some(thread) = items
                .iter_mut()
                .find(|thread| thread.id == target_thread_id)
            {
                thread.draft_prompt = task.prompt.clone();
                thread.updated_at = now_rfc3339();
            }
        });
        persist_state();
        status_text.set("已恢复历史生成条件，正在使用当前服务商配置重新生成。".into());

        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            run_generation();
        });
    };

    let cancel_generation = move |task_id: String| {
        if !active_generation_ids.with_untracked(|items| items.contains(&task_id)) {
            return;
        }
        cancelled_generation_ids.update(|items| {
            items.insert(task_id.clone());
        });
        // 先移除等待卡；取消标记会一直保留到异步任务收尾，阻止迟到响应重新写回。
        tasks.update(|items| items.retain(|task| task.id != task_id));
        threads.update(|items| {
            for thread in items {
                thread.task_ids.retain(|id| id != &task_id);
            }
        });
        record_sync_tombstones(tombstones, [(SyncEntityKind::Task, task_id.clone())]);
        persist_state();
        let controller = generation_runtimes.with_untracked(|items| {
            items
                .get(&task_id)
                .map(|runtime| runtime.abort_controller.clone())
        });
        if let Some(controller) = controller {
            controller.abort();
        }
        status_text.set("正在停止所选生成任务……".into());
    };

    let cancel_all_generations = move || {
        let task_ids = active_generation_ids.get_untracked();
        if task_ids.is_empty() {
            return;
        }
        cancelled_generation_ids.update(|items| {
            items.extend(task_ids.iter().cloned());
        });
        // 批量移除任务记录，使画廊立即清掉所有等待卡。
        tasks.update(|items| items.retain(|task| !task_ids.contains(&task.id)));
        threads.update(|items| {
            for thread in items {
                thread.task_ids.retain(|id| !task_ids.contains(id));
            }
        });
        record_sync_tombstones(
            tombstones,
            task_ids
                .iter()
                .cloned()
                .map(|task_id| (SyncEntityKind::Task, task_id)),
        );
        persist_state();
        generation_runtimes.with_untracked(|items| {
            for task_id in &task_ids {
                if let Some(runtime) = items.get(task_id) {
                    runtime.abort_controller.abort();
                }
            }
        });
        status_text.set(format!("正在停止 {} 个生成任务……", task_ids.len()));
    };

    (
        run_generation,
        rerun_task,
        cancel_generation,
        cancel_all_generations,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn test_asset(id: &str, byte_len: u64) -> ImageAssetRef {
        ImageAssetRef {
            id: id.into(),
            sha256: format!("sha-{id}"),
            mime_type: "image/png".into(),
            byte_len,
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
    fn submitted_references_prioritize_continuation_filter_and_cap() {
        let mut assets = (0..18)
            .map(|index| test_asset(&format!("asset-{index}"), 1))
            .collect::<Vec<_>>();
        let mut mask = test_asset("mask", 1);
        mask.metadata
            .insert("mask_base_asset_id".into(), "asset-0".into());
        let mut background = test_asset("background", 1);
        background.metadata.insert(
            THEME_BACKGROUND_ROLE_KEY.into(),
            THEME_BACKGROUND_ROLE.into(),
        );
        assets.extend([mask, background]);

        let mut selected_ids = vec!["missing".into(), "mask".into(), "background".into()];
        selected_ids.extend((0..18).map(|index| format!("asset-{index}")));
        selected_ids.push("asset-5".into());
        let submitted = submitted_reference_ids(&assets, &selected_ids, Some("asset-5"));

        assert_eq!(submitted.len(), MAX_GENERATION_REFERENCE_ASSETS);
        assert_eq!(submitted.first().map(String::as_str), Some("asset-5"));
        assert_eq!(
            submitted
                .iter()
                .filter(|id| id.as_str() == "asset-5")
                .count(),
            1
        );
        assert!(
            !submitted
                .iter()
                .any(|id| matches!(id.as_str(), "missing" | "mask" | "background"))
        );
    }

    #[test]
    fn byte_budget_uses_device_memory_and_clamps_extremes() {
        assert_eq!(
            generation_byte_budget(None),
            DEFAULT_ACTIVE_GENERATION_BYTE_BUDGET
        );
        assert_eq!(
            generation_byte_budget(Some(1.0)),
            MIN_ACTIVE_GENERATION_BYTE_BUDGET
        );
        assert_eq!(generation_byte_budget(Some(4.0)), 256 * 1024 * 1024);
        assert_eq!(
            generation_byte_budget(Some(32.0)),
            MAX_ACTIVE_GENERATION_BYTE_BUDGET
        );
    }

    #[test]
    fn task_estimate_deduplicates_references_and_includes_output_buffer() {
        let assets = vec![test_asset("asset-a", 10), test_asset("asset-b", 20)];
        let references = vec!["asset-a".into(), "asset-b".into(), "asset-a".into()];

        assert_eq!(
            estimated_generation_task_bytes(&assets, &references, 100, 50, 2),
            GENERATION_TASK_FIXED_BYTE_OVERHEAD + (10 + 20) * 2 + 100 * 50 * 4 * 2 * 2
        );
    }

    #[test]
    fn custom_http_requires_an_approved_account() {
        assert!(!generation_provider_allowed(ProviderKind::CustomHttp, None));
        assert!(!generation_provider_allowed(
            ProviderKind::CustomHttp,
            Some("pending")
        ));
        assert!(generation_provider_allowed(
            ProviderKind::CustomHttp,
            Some("approved")
        ));
        assert!(generation_provider_allowed(ProviderKind::OpenAiImage, None));
    }
}
