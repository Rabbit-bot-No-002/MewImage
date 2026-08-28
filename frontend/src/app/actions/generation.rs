use super::super::*;

struct PreparedGeneratedImage {
    assets: Vec<ImageAssetRef>,
    visible_asset_id: String,
    local_background_error: Option<String>,
}

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

pub(crate) fn build_generation_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    enqueue_payload_writes: impl Fn(Vec<(String, String)>) + Copy + Send + Sync + 'static,
    commit_current_thread_draft: impl Fn() + Copy + Send + Sync + 'static,
) -> (
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
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
    let generating = composer.generating;
    let generation_cancel_requested = composer.generation_cancel_requested;
    let generation_abort_controller = composer.generation_abort_controller;
    let show_settings = ui.show_settings;
    let current_config = derived.current_config;

    let run_generation = move || {
        if generating.get_untracked() {
            return;
        }
        let Some(config) = current_config.get_untracked() else {
            status_text.set("请先在设置中准备一个服务商配置。".into());
            return;
        };
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
        let mut references = selected_reference_assets(&assets.get_untracked(), &selected_ids);
        references.truncate(16);
        if let Some(asset_id) = continuation_asset_id.get_untracked() {
            if let Some(asset) = assets
                .get_untracked()
                .iter()
                .find(|asset| {
                    asset.id == asset_id && !asset.metadata.contains_key("mask_base_asset_id")
                })
                .cloned()
            {
                references.retain(|item| item.id != asset.id);
                references.insert(0, asset);
                references.truncate(16);
            }
        }
        let (resolved_width, resolved_height) = resolve_dimensions(
            resolution_mode.get_untracked().as_str(),
            resolution_group.get_untracked().as_str(),
            aspect_ratio.get_untracked().as_str(),
            effective_custom_aspect_ratio.get_untracked().as_str(),
            custom_width.get_untracked(),
            custom_height.get_untracked(),
            &references,
        );
        custom_width.set(resolved_width);
        custom_height.set(resolved_height);
        let quality_value = quality.get_untracked();
        let count_value = count.get_untracked();
        let Ok(abort_controller) = web_sys::AbortController::new() else {
            status_text.set("当前浏览器无法创建请求中止控制器。".into());
            return;
        };
        let abort_signal = abort_controller.signal();

        let task_id = new_id();
        generation_cancel_requested.set(false);
        generation_abort_controller.set(Some(abort_controller));
        generating.set(true);
        status_text.set("正在提交后台生成任务，长耗时请求会自动轮询结果……".into());

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
                reference_asset_ids: selected_ids.clone(),
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
        let cancel_requested_signal = generation_cancel_requested;
        let abort_controller_signal = generation_abort_controller;
        let continuation_signal = continuation_asset_id;
        let threads_signal = threads;
        let tombstones_signal = tombstones;
        let persist = persist_state;
        let selected_ids_for_request = selected_ids.clone();
        let continuation_asset_id_for_request = continuation_asset_id.get_untracked();
        spawn_local(async move {
            let finish_cancelled = || {
                if !cancel_requested_signal.get_untracked() {
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
                status_signal.set("当前生成任务已停止。".into());
                abort_controller_signal.set(None);
                cancel_requested_signal.set(false);
                generating_signal.set(false);
                true
            };

            let mut required_asset_ids = selected_ids_for_request.clone();
            if let Some(asset_id) = continuation_asset_id_for_request.clone() {
                required_asset_ids.push(asset_id);
            }
            let payload_result =
                ensure_asset_payloads_loaded(assets_signal, &required_asset_ids).await;
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
                status_signal.set(format!("生成失败：{error}"));
                abort_controller_signal.set(None);
                generating_signal.set(false);
                play_generation_notification(false);
                return;
            }
            let references = assets_signal.with_untracked(|items| {
                let mut references = selected_reference_assets(items, &selected_ids_for_request);
                references.truncate(16);
                if let Some(asset_id) = continuation_asset_id_for_request.clone() {
                    if let Some(asset) = items
                        .iter()
                        .find(|asset| {
                            asset.id == asset_id
                                && !asset.metadata.contains_key("mask_base_asset_id")
                        })
                        .cloned()
                    {
                        references.retain(|item| item.id != asset.id);
                        references.insert(0, asset);
                        references.truncate(16);
                    }
                }
                references
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
            if finish_cancelled() {
                return;
            }
            match generation_result {
                Ok((result, used_proxy)) => {
                    let upstream_result_count = result.images.len();
                    let mut produced_assets = Vec::new();
                    let mut visible_asset_ids = Vec::new();
                    let mut asset_build_errors = Vec::new();
                    let mut local_background_errors = Vec::new();
                    for (index, image) in result.images.iter().enumerate() {
                        if cancel_requested_signal.get_untracked() {
                            break;
                        }
                        if local_background {
                            status_signal.set(format!(
                                "正在本地去除背景（{}/{}）……",
                                index + 1,
                                upstream_result_count
                            ));
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
                    if finish_cancelled() {
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
                        tasks_signal.update(|items| {
                            if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                                task.status = TaskStatus::Failed;
                                task.updated_at = now_rfc3339();
                                task.result = Some(result.clone());
                                task.error_message = Some(error.clone());
                            }
                        });
                        persist();
                        status_signal.set(format!("生成失败：{error}"));
                        abort_controller_signal.set(None);
                        generating_signal.set(false);
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
                    let produced_asset_ids = produced_assets
                        .iter()
                        .map(|asset| asset.id.clone())
                        .collect::<Vec<_>>();
                    enqueue_payload_writes(produced_payloads);
                    assets_signal.update(|items| {
                        items.extend(produced_assets);
                        touch_and_trim_asset_payload_cache(items, &produced_asset_ids, false);
                    });
                    let local_background_error_message = (!local_background_errors.is_empty())
                        .then(|| local_background_errors.join("；"));
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            let mut result = result;
                            result.parameter_snapshot.actual_width = Some(actual_width);
                            result.parameter_snapshot.actual_height = Some(actual_height);
                            task.status = TaskStatus::Succeeded;
                            task.updated_at = now_rfc3339();
                            task.result = Some(result);
                            task.error_message = local_background_error_message.clone();
                            strip_successful_task_payloads(std::slice::from_mut(task));
                        }
                    });
                    continuation_signal.set(first_generated_id);
                    persist();
                    status_signal.set(if local_background && !local_background_errors.is_empty() {
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
                        "生成完成，已在浏览器本地去除背景，并自动进入连续修改模式。".into()
                    } else if !asset_build_errors.is_empty() {
                        format!(
                            "生成完成，但有 {} 张结果未能保存到本地。{}",
                            asset_build_errors.len(),
                            asset_build_errors.join("；")
                        )
                    } else if used_proxy {
                        "生成完成，已自动进入连续修改模式。".into()
                    } else {
                        "生成完成，已自动进入连续修改模式，结果已写入当前会话。".into()
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
                    status_signal.set(format!("生成失败：{error}"));
                    play_generation_notification(false);
                }
            }
            abort_controller_signal.set(None);
            cancel_requested_signal.set(false);
            generating_signal.set(false);
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

    (run_generation, rerun_task)
}
