use super::super::*;

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
                prompt: prompt.clone(),
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
                    let mut produced_assets = Vec::new();
                    let mut asset_build_errors = Vec::new();
                    for (index, image) in result.images.iter().enumerate() {
                        if cancel_requested_signal.get_untracked() {
                            break;
                        }
                        let asset_payload = match (image.data_url.clone(), image.url.clone()) {
                            (Some(data_url), _) => match decode_browser_data_url(&data_url) {
                                Ok((mime_type, bytes)) => Some((
                                    Some(data_url),
                                    None,
                                    bytes.len() as u64,
                                    sha256_hex(&bytes),
                                    mime_type,
                                )),
                                Err(error) => {
                                    asset_build_errors.push(format!(
                                        "第 {} 张结果数据解析失败：{}",
                                        index + 1,
                                        error
                                    ));
                                    None
                                }
                            },
                            (None, Some(url)) => match fetch_image_bytes(&url).await {
                                Ok((bytes, mime_type)) => {
                                    let data_url = bytes_to_data_url(&bytes, &mime_type);
                                    let byte_len = bytes.len() as u64;
                                    let sha256 = sha256_hex(&bytes);
                                    Some((Some(data_url), Some(url), byte_len, sha256, mime_type))
                                }
                                Err(error) => {
                                    asset_build_errors.push(format!(
                                        "第 {} 张结果下载失败：{}",
                                        index + 1,
                                        error
                                    ));
                                    None
                                }
                            },
                            (None, None) => {
                                asset_build_errors
                                    .push(format!("第 {} 张结果缺少图像数据。", index + 1));
                                None
                            }
                        };
                        let Some((data_url, remote_url, byte_len, sha256, mime_type)) =
                            asset_payload
                        else {
                            continue;
                        };
                        let (actual_width, actual_height) = match data_url.as_deref() {
                            Some(data_url) => load_image_dimensions(data_url)
                                .await
                                .unwrap_or((resolved_width, resolved_height)),
                            None => (resolved_width, resolved_height),
                        };
                        let mut metadata = BTreeMap::new();
                        let thumbnail_source = ImageAssetRef {
                            id: String::new(),
                            sha256: sha256.clone(),
                            mime_type: mime_type.clone(),
                            byte_len,
                            width: Some(actual_width),
                            height: Some(actual_height),
                            created_at: String::new(),
                            updated_at: String::new(),
                            data_url: data_url.clone(),
                            remote_object_key: None,
                            remote_url: remote_url.clone(),
                            source_task_id: None,
                            metadata: BTreeMap::new(),
                        };
                        if let Ok(thumbnail) =
                            thumbnail_data_url_from_asset(&thumbnail_source, THUMBNAIL_MAX_EDGE)
                                .await
                        {
                            metadata.insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                        }
                        produced_assets.push(ImageAssetRef {
                            id: new_id(),
                            sha256,
                            mime_type,
                            byte_len,
                            width: Some(actual_width),
                            height: Some(actual_height),
                            created_at: now_rfc3339(),
                            updated_at: now_rfc3339(),
                            data_url,
                            remote_object_key: None,
                            remote_url,
                            source_task_id: Some(task_id.clone()),
                            metadata,
                        });
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
                        .find_map(|asset| asset.width.zip(asset.height))
                        .unwrap_or((resolved_width, resolved_height));
                    let first_generated_id = produced_assets.first().map(|asset| asset.id.clone());
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
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            let mut result = result;
                            result.parameter_snapshot.actual_width = Some(actual_width);
                            result.parameter_snapshot.actual_height = Some(actual_height);
                            task.status = TaskStatus::Succeeded;
                            task.updated_at = now_rfc3339();
                            task.result = Some(result);
                            strip_successful_task_payloads(std::slice::from_mut(task));
                        }
                    });
                    continuation_signal.set(first_generated_id);
                    persist();
                    status_signal.set(if !asset_build_errors.is_empty() {
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
