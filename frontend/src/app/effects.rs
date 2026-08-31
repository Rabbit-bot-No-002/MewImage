use super::*;

/// 安装仅应在应用根部创建一次的响应式副作用和初始化任务。
pub(crate) fn install_app_effects() {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();
    let persistence = expect_context::<PersistenceState>();
    let derived = expect_context::<AppDerived>();

    Effect::new(move |_| {
        apply_appearance(&workspace.preferences.get(), ui.system_dark.get());
    });

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(Some(query)) = window.match_media("(prefers-color-scheme: dark)") else {
            return;
        };
        ui.system_dark.set(query.matches());
        let on_change = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |event: web_sys::MediaQueryListEvent| {
                ui.system_dark.set(event.matches());
            },
        );
        let _ =
            query.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref());
        on_change.forget();
    });

    Effect::new(move |_| {
        let preferences = workspace.preferences.get();
        let background = preferences.appearance.custom_background;
        let Some(asset_id) = background.asset_id.filter(|_| background.enabled) else {
            clear_background_display(ui);
            return;
        };
        if ui.background_display_asset_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            return;
        }
        let asset_sources = workspace.assets.with(|items| {
            items
                .iter()
                .find(|asset| asset.id == asset_id && is_theme_background(asset))
                .map(|asset| (asset.data_url.clone(), asset.remote_url.clone()))
        });
        let Some((data_url, remote_url)) = asset_sources else {
            clear_background_display(ui);
            return;
        };
        ui.background_display_asset_id.set(Some(asset_id.clone()));
        if let Some(data_url) = data_url {
            if let Err(error) = set_background_display_from_data_url(ui, &data_url) {
                clear_background_display(ui);
                ui.appearance_message.set(Some(error));
            }
            return;
        }
        spawn_local(async move {
            let local_payload = load_asset_payloads(std::slice::from_ref(&asset_id))
                .await
                .ok()
                .and_then(|mut payloads| payloads.remove(&asset_id));
            let data_url = if let Some(data_url) = local_payload {
                data_url
            } else {
                let Some(remote_url) = remote_url else {
                    clear_background_display(ui);
                    ui.appearance_message
                        .set(Some("主题背景原文件不可用，可尝试重新同步或上传。".into()));
                    return;
                };
                let source = if remote_url.starts_with('/') {
                    api_url(&remote_url)
                } else {
                    remote_url
                };
                let (bytes, mime_type) = match fetch_authenticated_image_bytes(&source).await {
                    Ok(result) => result,
                    Err(error) => {
                        clear_background_display(ui);
                        ui.appearance_message
                            .set(Some(format!("下载云端主题背景失败：{error}")));
                        return;
                    }
                };
                let data_url = bytes_to_data_url(&bytes, &mime_type);
                let _ =
                    apply_asset_payload_changes(&[(asset_id.clone(), data_url.clone())], &[]).await;
                data_url
            };
            if workspace
                .preferences
                .get_untracked()
                .appearance
                .custom_background
                .asset_id
                .as_deref()
                != Some(asset_id.as_str())
            {
                return;
            }
            if let Err(error) = set_background_display_from_data_url(ui, &data_url) {
                clear_background_display(ui);
                ui.appearance_message.set(Some(error));
                return;
            }
            workspace.assets.update(|items| {
                if let Some(asset) = items.iter_mut().find(|asset| asset.id == asset_id) {
                    asset.data_url = Some(data_url.clone());
                }
            });
        });
    });

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let on_keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if event.key() != "Escape" {
                return;
            }
            if ui.show_config_switcher.get_untracked() {
                ui.show_config_switcher.set(false);
                return;
            }
            if ui.preview_state.get_untracked().is_none() {
                return;
            }
            if ui.preview_fullscreen.get_untracked() {
                ui.preview_fullscreen.set(false);
                ui.preview_zoom.set(1.0);
                ui.preview_offset_x.set(0.0);
                ui.preview_offset_y.set(0.0);
                ui.preview_dragging.set(false);
            } else {
                ui.preview_state.set(None);
                ui.preview_panel_state.set(None);
                ui.preview_fullscreen.set(false);
                ui.preview_zoom.set(1.0);
                ui.preview_offset_x.set(0.0);
                ui.preview_offset_y.set(0.0);
                ui.preview_dragging.set(false);
                ui.context_menu_state.set(None);
            }
        });
        let _ =
            window.add_event_listener_with_callback("keydown", on_keydown.as_ref().unchecked_ref());
        on_keydown.forget();
    });

    Effect::new(move |_| {
        if let Some(tip) = ui.floating_tip_state.get() {
            if tip.persistent {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let token = tip.token;
            let callback = Closure::<dyn FnMut()>::once(move || {
                if ui
                    .floating_tip_state
                    .get_untracked()
                    .map(|current| current.token == token)
                    .unwrap_or(false)
                {
                    ui.floating_tip_state.set(None);
                }
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                1400,
            );
            callback.forget();
        }
    });

    spawn_local(async move {
        initialize_app_state(workspace, composer, account, persistence).await;
    });

    Effect::new(move |_| {
        let value = composer.draft_prompt.get();
        if let Some(textarea) = composer.draft_prompt_ref.get()
            && textarea.value() != value
        {
            textarea.set_value(&value);
        }
    });

    Effect::new(move |_| {
        let _ = workspace.current_thread_id.get();
        ui.gallery_page.set(1);
    });

    Effect::new(move |_| {
        let _ = derived.active_favorite_folder_id.get();
        ui.favorite_page.set(1);
    });
}

fn set_background_display_from_data_url(ui: UiState, data_url: &str) -> Result<(), String> {
    let (mime_type, bytes) = decode_browser_data_url(data_url)?;
    let blob = blob_from_bytes(&bytes, &mime_type)?;
    let object_url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|error| format!("创建主题背景地址失败：{error:?}"))?;
    if let Some(previous) = ui.background_display_src.get_untracked()
        && previous.starts_with("blob:")
    {
        let _ = web_sys::Url::revoke_object_url(&previous);
    }
    ui.background_display_src.set(Some(object_url));
    Ok(())
}

fn clear_background_display(ui: UiState) {
    if let Some(previous) = ui.background_display_src.get_untracked()
        && previous.starts_with("blob:")
    {
        let _ = web_sys::Url::revoke_object_url(&previous);
    }
    ui.background_display_src.set(None);
    ui.background_display_asset_id.set(None);
}

async fn initialize_app_state(
    workspace: WorkspaceState,
    composer: ComposerState,
    account: AccountState,
    persistence: PersistenceState,
) {
    let mut state = load_snapshot().await.unwrap_or_else(|_| {
        let mut next = LocalAppState::default();
        next.threads = vec![default_thread()];
        next.configs
            .push(default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID));
        next
    });
    if state.threads.is_empty() {
        state.threads.push(default_thread());
    }
    if state.configs.is_empty() {
        state
            .configs
            .push(default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID));
    }
    for config in &mut state.configs {
        normalize_api_config(config);
    }
    state.preferences.appearance.normalize();
    let stripped_task_payloads = strip_successful_task_payloads(&mut state.tasks);
    state
        .assets
        .retain(|asset| !asset.metadata.contains_key("mask_base_asset_id"));
    let removed_local_background_source_ids =
        data_management::discard_legacy_local_background_sources(&mut state);
    record_sync_tombstones_in(
        &mut state.tombstones,
        removed_local_background_source_ids
            .iter()
            .cloned()
            .map(|asset_id| (SyncEntityKind::Asset, asset_id)),
    );
    let had_embedded_payloads = state.assets.iter().any(|asset| asset.data_url.is_some());
    reconcile_task_integrity(&mut state.tasks, &state.assets, true);
    let initial_thread_id = state
        .threads
        .first()
        .map(|thread| thread.id.clone())
        .unwrap_or_default();
    workspace.current_thread_id.set(initial_thread_id.clone());
    workspace.current_config_id.set(
        state
            .configs
            .first()
            .map(|config| config.id.clone())
            .unwrap_or_default(),
    );
    composer.draft_prompt.set(
        state
            .threads
            .first()
            .map(|thread| thread.draft_prompt.clone())
            .unwrap_or_default(),
    );
    apply_local_state(
        state.clone(),
        workspace.configs,
        workspace.tasks,
        workspace.threads,
        workspace.assets,
        workspace.preferences,
        workspace.checkpoint,
        workspace.tombstones,
    );
    composer
        .status_text
        .set("本地工作台已恢复，缩略图正在后台补全……".into());

    let tasks_for_thumbnails = state.tasks.clone();
    let mut assets_for_thumbnails = state.assets.clone();
    let initial_payloads = asset_payload_pairs(&state.assets);
    if had_embedded_payloads {
        persistence.payload_write_queue.update(|queued| {
            for (asset_id, data_url) in initial_payloads {
                queued.insert(asset_id, data_url);
            }
        });
        request_payload_flush_for_state(persistence);
    }
    if !removed_local_background_source_ids.is_empty() {
        persistence.payload_write_queue.update(|queued| {
            for asset_id in &removed_local_background_source_ids {
                queued.remove(asset_id);
            }
        });
        persistence.payload_delete_queue.update(|queued| {
            queued.extend(removed_local_background_source_ids.iter().cloned());
        });
        request_payload_flush_for_state(persistence);
    }
    if had_embedded_payloads
        || stripped_task_payloads
        || !removed_local_background_source_ids.is_empty()
    {
        request_workspace_persist_for_state(workspace, persistence);
    }

    let thumbnail_order = prioritized_asset_indexes_for_thread(
        &assets_for_thumbnails,
        &tasks_for_thumbnails,
        &initial_thread_id,
    );
    spawn_local(async move {
        let mut changed = false;
        let mut first_batch_changed = false;
        for (position, asset_index) in thumbnail_order.into_iter().enumerate() {
            let Some(asset) = assets_for_thumbnails.get_mut(asset_index) else {
                continue;
            };
            if asset.metadata.contains_key(THUMBNAIL_DATA_URL_KEY) {
                continue;
            }
            if is_theme_background(asset) {
                continue;
            }
            if let Ok(thumbnail) = thumbnail_data_url_from_asset(asset, THUMBNAIL_MAX_EDGE).await {
                asset
                    .metadata
                    .insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                changed = true;
                if position < 6 {
                    first_batch_changed = true;
                }
            }
            if first_batch_changed && position == 5 {
                workspace.assets.set(assets_for_thumbnails.clone());
            }
        }
        if changed {
            workspace.assets.set(assets_for_thumbnails.clone());
            let payloads = asset_payload_pairs(&assets_for_thumbnails);
            persistence.payload_write_queue.update(|queued| {
                for (asset_id, data_url) in payloads {
                    queued.insert(asset_id, data_url);
                }
            });
            request_payload_flush_for_state(persistence);
            request_workspace_persist_for_state(workspace, persistence);
        }
        composer
            .status_text
            .set("本地工作台已恢复，可以直接开始生成或继续修改。".into());
    });

    if let Ok(remote_templates) = load_templates().await
        && !remote_templates.is_empty()
    {
        workspace.templates.set(remote_templates);
    }

    if let Ok(response) = Request::get(&api_url("/api/auth/me"))
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        && let Ok(me) = response.json::<MeResponse>().await
    {
        if let Some(user) = me.user.as_ref() {
            let enabled = load_api_key_sync_enabled(&user.id);
            account.sync_api_keys_enabled.set(enabled);
            if enabled {
                if let Some(secret) = load_trusted_sync_secret(&user.id) {
                    account.sync_secret.set(secret);
                    account
                        .sync_status_text
                        .set(Some("可信设备已解锁，可同步 API Key。".into()));
                } else {
                    account.sync_status_text.set(Some(
                        "当前登录会话尚未解锁 API Key 同步，请输入账号密码解锁。".into(),
                    ));
                }
            }
        }
        account.auth_user.set(me.user);
    }

    if let Ok(response) = Request::get(&api_url("/api/auth/setup-status"))
        .send()
        .await
        && let Ok(status) = response.json::<AdminSetupStatusResponse>().await
    {
        account.admin_setup_allowed.set(!status.admin_exists);
    }

    if !composer
        .status_text
        .get_untracked()
        .contains("可以直接开始")
    {
        composer
            .status_text
            .set("本地工作台已恢复，可以直接开始生成或继续修改。".into());
    }
}

fn request_payload_flush_for_state(persistence: PersistenceState) {
    request_payload_flush(
        persistence.payload_write_queue,
        persistence.payload_delete_queue,
        persistence.payload_flush_scheduled,
        persistence.payload_flush_inflight,
        persistence.payload_flush_pending,
    );
}

fn request_workspace_persist_for_state(workspace: WorkspaceState, persistence: PersistenceState) {
    request_workspace_persist(
        workspace.tasks,
        workspace.threads,
        workspace.assets,
        workspace.checkpoint,
        workspace.tombstones,
        persistence.workspace_persist_scheduled,
        persistence.workspace_persist_inflight,
        persistence.workspace_persist_pending,
    );
}
