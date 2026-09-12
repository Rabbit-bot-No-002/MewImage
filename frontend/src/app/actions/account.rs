use super::super::*;

#[allow(clippy::type_complexity)]
pub(crate) fn build_account_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    enqueue_payload_deletes: impl Fn(Vec<String>) + Copy + Send + Sync + 'static,
) -> (
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(Event) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(&'static str) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(&'static str, String) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();
    let configs = workspace.configs;
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let preferences = workspace.preferences;
    let checkpoint = workspace.checkpoint;
    let tombstones = workspace.tombstones;
    let current_thread_id = workspace.current_thread_id;
    let current_config_id = workspace.current_config_id;
    let status_text = composer.status_text;
    let selected_reference_ids = composer.selected_reference_ids;
    let continuation_asset_id = composer.continuation_asset_id;
    let continuation_task_id = composer.continuation_task_id;
    let conversation_rebase_requested = composer.conversation_rebase_requested;
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let draft_prompt = composer.draft_prompt;
    let auth_user = account.auth_user;
    let login_username = account.login_username;
    let login_password = account.login_password;
    let register_password_confirm = account.register_password_confirm;
    let admin_setup_token = account.admin_setup_token;
    let show_admin_setup_token = account.show_admin_setup_token;
    let admin_setup_allowed = account.admin_setup_allowed;
    let auth_form_message = account.auth_form_message;
    let username_check_message = account.username_check_message;
    let change_old_password = account.change_old_password;
    let change_new_password = account.change_new_password;
    let change_new_password_confirm = account.change_new_password_confirm;
    let password_form_message = account.password_form_message;
    let admin_users = account.admin_users;
    let loading_admin_users = account.loading_admin_users;
    let sync_secret = account.sync_secret;
    let legacy_sync_secret = account.legacy_sync_secret;
    let sync_api_keys_enabled = account.sync_api_keys_enabled;
    let sync_unlock_password = account.sync_unlock_password;
    let sync_unlocking = account.sync_unlocking;
    let sync_status_text = account.sync_status_text;
    let syncing = account.syncing;
    let confirm_popover = ui.confirm_popover;
    let preview_state = ui.preview_state;
    let preview_panel_state = ui.preview_panel_state;

    let sync_action = move || {
        let Some(user) = auth_user.get_untracked() else {
            sync_status_text.set(Some("登录后才会启用跨设备同步。".into()));
            return;
        };
        if user.status != "approved" {
            sync_status_text.set(Some("账号仍在等待管理员审批，暂不能使用云端同步。".into()));
            return;
        }
        syncing.set(true);
        sync_status_text.set(Some("正在整理本地同步数据……".into()));
        let state = snapshot_local_state(
            configs,
            tasks,
            threads,
            assets,
            preferences,
            checkpoint,
            tombstones,
        );
        let sync_api_keys = sync_api_keys_enabled.get_untracked();
        let has_api_key_material = state
            .configs
            .iter()
            .any(|config| config.api_key_plaintext.is_some() || config.api_key_encrypted.is_some());
        if sync_api_keys && sync_secret.get_untracked().is_empty() && has_api_key_material {
            syncing.set(false);
            sync_status_text.set(Some(
                "API Key 同步尚未解锁，请先在下方输入账号密码完成可信设备解锁。".into(),
            ));
            return;
        }
        let secret = sync_secret.get_untracked();
        let legacy_secret = legacy_sync_secret.get_untracked();
        let status_signal = sync_status_text;
        let syncing_signal = syncing;
        let persist = persist_state;
        let configs_signal = configs;
        let tasks_signal = tasks;
        let threads_signal = threads;
        let assets_signal = assets;
        let preferences_signal = preferences;
        let checkpoint_signal = checkpoint;
        let auth_user_signal = auth_user;
        let enqueue_deleted_payloads = enqueue_payload_deletes;
        let selected_reference_ids_signal = selected_reference_ids;
        let continuation_asset_id_signal = continuation_asset_id;
        let continuation_task_id_signal = continuation_task_id;
        let conversation_rebase_requested_signal = conversation_rebase_requested;
        let reference_menu_asset_id_signal = reference_menu_asset_id;
        let preview_state_signal = preview_state;
        let preview_panel_state_signal = preview_panel_state;
        let current_config_id_signal = current_config_id;
        let current_thread_id_signal = current_thread_id;
        let draft_prompt_signal = draft_prompt;
        spawn_local(async move {
            let started_at = js_sys::Date::now();
            let mut state = state;
            if let Err(error) = ensure_sync_capabilities(&state).await {
                syncing_signal.set(false);
                status_signal.set(Some(error));
                return;
            }
            let indexed_asset_ids = state
                .assets
                .iter()
                .filter(|asset| asset.remote_object_key.is_some())
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>();
            if !indexed_asset_ids.is_empty() {
                status_signal.set(Some("正在核验云端图片完整性……".into()));
                let missing_remote_ids = match missing_remote_asset_ids(indexed_asset_ids).await {
                    Ok(asset_ids) => asset_ids,
                    Err(error) => {
                        syncing_signal.set(false);
                        status_signal.set(Some(error));
                        return;
                    }
                };
                if !missing_remote_ids.is_empty() {
                    let missing_remote_ids = missing_remote_ids.into_iter().collect::<HashSet<_>>();
                    let invalidate_remote_fields = |asset: &mut ImageAssetRef| {
                        if missing_remote_ids.contains(&asset.id) {
                            asset.remote_object_key = None;
                            asset.remote_url = None;
                            asset.updated_at = now_rfc3339();
                        }
                    };
                    for asset in &mut state.assets {
                        invalidate_remote_fields(asset);
                    }
                    assets_signal.update(|items| {
                        for asset in items {
                            invalidate_remote_fields(asset);
                        }
                    });
                    persist();
                    status_signal.set(Some(format!(
                        "发现 {} 张云端原图缺失，正在从当前设备恢复……",
                        missing_remote_ids.len()
                    )));
                }
            }
            let pending_asset_ids = state
                .assets
                .iter()
                .filter(|asset| asset.remote_object_key.is_none())
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>();
            let mut missing_local_asset_ids = Vec::new();
            let mut uploaded_asset_count = 0usize;
            let mut uploaded_asset_bytes = 0u64;
            for (index, asset_id) in pending_asset_ids.iter().enumerate() {
                let Some(asset) = state
                    .assets
                    .iter()
                    .find(|asset| asset.id == *asset_id)
                    .cloned()
                else {
                    continue;
                };
                status_signal.set(Some(format!(
                    "正在分批上传图片 {}/{}，当前 {}……",
                    index + 1,
                    pending_asset_ids.len(),
                    format_byte_size(asset.byte_len),
                )));
                let data_url = if let Some(data_url) = asset.data_url.clone() {
                    data_url
                } else {
                    match load_asset_payloads(std::slice::from_ref(asset_id)).await {
                        Ok(mut payloads) => match payloads.remove(asset_id) {
                            Some(data_url) => data_url,
                            None => {
                                missing_local_asset_ids.push(asset_id.clone());
                                continue;
                            }
                        },
                        Err(error) => {
                            syncing_signal.set(false);
                            status_signal.set(Some(format!("读取本地图片失败：{error}")));
                            return;
                        }
                    }
                };
                let uploaded = match upload_asset_for_sync(&asset, &data_url).await {
                    Ok(uploaded) => uploaded,
                    Err(error) => {
                        syncing_signal.set(false);
                        status_signal.set(Some(format!(
                            "第 {} 张图片上传失败：{error}。已完成部分会保留，下次可继续同步。",
                            index + 1
                        )));
                        return;
                    }
                };
                let apply_remote_fields = |target: &mut ImageAssetRef| {
                    target.sha256 = uploaded.sha256.clone();
                    target.mime_type = uploaded.mime_type.clone();
                    target.byte_len = uploaded.byte_len;
                    target.remote_object_key = uploaded.remote_object_key.clone();
                    target.remote_url = uploaded.remote_url.clone();
                };
                if let Some(target) = state.assets.iter_mut().find(|item| item.id == *asset_id) {
                    apply_remote_fields(target);
                }
                assets_signal.update(|items| {
                    if let Some(target) = items.iter_mut().find(|item| item.id == *asset_id) {
                        apply_remote_fields(target);
                    }
                });
                uploaded_asset_count += 1;
                uploaded_asset_bytes = uploaded_asset_bytes.saturating_add(uploaded.byte_len);
                persist();
            }
            status_signal.set(Some(if uploaded_asset_count == 0 {
                "正在同步配置、会话和图片索引……".into()
            } else {
                format!(
                    "已上传 {uploaded_asset_count} 张新图片，共约 {}，正在同步索引……",
                    format_byte_size(uploaded_asset_bytes)
                )
            }));
            let mut envelope_state = state.clone();
            for asset in &mut envelope_state.assets {
                asset.data_url = None;
            }
            let envelope = match prepare_sync_envelope(
                &envelope_state,
                if !sync_api_keys || secret.is_empty() {
                    None
                } else {
                    Some(secret.as_str())
                },
                sync_api_keys,
            ) {
                Ok(envelope) => envelope,
                Err(error) => {
                    syncing_signal.set(false);
                    status_signal.set(Some(format!("同步前加密失败：{error}")));
                    return;
                }
            };
            let request = mew_image_shared::SyncPushRequest {
                client_updated_at: now_rfc3339(),
                envelope,
            };
            let response = Request::post(&api_url("/api/sync/push"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request);
            let Ok(builder) = response else {
                syncing_signal.set(false);
                status_signal.set(Some("同步请求序列化失败。".into()));
                return;
            };
            let request_started_at = js_sys::Date::now();
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<SyncPullResponse>().await {
                    Ok(pulled) => {
                        let hydrated = hydrate_local_state(
                            &state,
                            pulled.envelope,
                            pulled.checkpoint,
                            if !sync_api_keys || secret.is_empty() {
                                None
                            } else {
                                Some(secret.as_str())
                            },
                            if !sync_api_keys || legacy_secret.is_empty() {
                                None
                            } else {
                                Some(legacy_secret.as_str())
                            },
                        );
                        let mut hydrated = hydrated;
                        reconcile_task_integrity(&mut hydrated.tasks, &hydrated.assets, true);
                        let needs_legacy_key_migration = sync_api_keys
                            && hydrated.configs.iter().any(|config| {
                                config.api_key_plaintext.is_some()
                                    && config.api_key_encrypted.is_none()
                            });
                        if needs_legacy_key_migration && !secret.is_empty() {
                            let mut migration_state = hydrated.clone();
                            for asset in &mut migration_state.assets {
                                asset.data_url = None;
                            }
                            if let Ok(envelope) =
                                prepare_sync_envelope(&migration_state, Some(secret.as_str()), true)
                            {
                                let migration_request = mew_image_shared::SyncPushRequest {
                                    client_updated_at: now_rfc3339(),
                                    envelope,
                                };
                                if let Ok(builder) = Request::post(&api_url("/api/sync/push"))
                                    .credentials(web_sys::RequestCredentials::Include)
                                    .json(&migration_request)
                                    && let Ok(response) = builder.send().await
                                    && response.ok()
                                    && let Ok(migrated) = response.json::<SyncPullResponse>().await
                                {
                                    hydrated = hydrate_local_state(
                                        &hydrated,
                                        migrated.envelope,
                                        migrated.checkpoint,
                                        Some(secret.as_str()),
                                        None,
                                    );
                                }
                            }
                        }
                        if hydrated.threads.is_empty() {
                            hydrated.threads.push(default_thread());
                        }
                        let unresolved_asset_count = missing_local_asset_ids
                            .iter()
                            .filter(|asset_id| {
                                hydrated
                                    .assets
                                    .iter()
                                    .find(|asset| asset.id == asset_id.as_str())
                                    .map(|asset| asset.remote_object_key.is_none())
                                    .unwrap_or(false)
                            })
                            .count();
                        let retained_asset_ids = hydrated
                            .assets
                            .iter()
                            .map(|asset| asset.id.as_str())
                            .collect::<HashSet<_>>();
                        let removed_asset_ids = state
                            .assets
                            .iter()
                            .filter(|asset| !retained_asset_ids.contains(asset.id.as_str()))
                            .map(|asset| asset.id.clone())
                            .collect::<Vec<_>>();
                        if !removed_asset_ids.is_empty() {
                            enqueue_deleted_payloads(removed_asset_ids.clone());
                            selected_reference_ids_signal
                                .update(|ids| ids.retain(|id| !removed_asset_ids.contains(id)));
                            if continuation_asset_id_signal
                                .get_untracked()
                                .as_ref()
                                .map(|id| removed_asset_ids.contains(id))
                                .unwrap_or(false)
                            {
                                continuation_asset_id_signal.set(None);
                                continuation_task_id_signal.set(None);
                                conversation_rebase_requested_signal.set(false);
                            }
                            if reference_menu_asset_id_signal
                                .get_untracked()
                                .as_ref()
                                .map(|id| removed_asset_ids.contains(id))
                                .unwrap_or(false)
                            {
                                reference_menu_asset_id_signal.set(None);
                            }
                            if preview_state_signal
                                .get_untracked()
                                .as_ref()
                                .and_then(|preview| preview.asset_id.as_ref())
                                .map(|asset_id| removed_asset_ids.contains(asset_id))
                                .unwrap_or(false)
                            {
                                preview_state_signal.set(None);
                                preview_panel_state_signal.set(None);
                            }
                        }
                        let continuation_task_missing = continuation_task_id_signal
                            .get_untracked()
                            .is_some_and(|task_id| {
                                !hydrated.tasks.iter().any(|task| task.id == task_id)
                            });
                        if continuation_task_missing {
                            continuation_asset_id_signal.set(None);
                            continuation_task_id_signal.set(None);
                            conversation_rebase_requested_signal.set(false);
                        }
                        let current_config_id_value = current_config_id_signal.get_untracked();
                        if !hydrated
                            .configs
                            .iter()
                            .any(|config| config.id == current_config_id_value)
                        {
                            current_config_id_signal.set(
                                hydrated
                                    .configs
                                    .first()
                                    .map(|config| config.id.clone())
                                    .unwrap_or_default(),
                            );
                        }
                        let current_thread_id_value = current_thread_id_signal.get_untracked();
                        if !hydrated
                            .threads
                            .iter()
                            .any(|thread| thread.id == current_thread_id_value)
                            && let Some(thread) = hydrated.threads.first()
                        {
                            current_thread_id_signal.set(thread.id.clone());
                            draft_prompt_signal.set(thread.draft_prompt.clone());
                            selected_reference_ids_signal.set(Vec::new());
                            continuation_asset_id_signal.set(None);
                            continuation_task_id_signal.set(None);
                            conversation_rebase_requested_signal.set(false);
                        }
                        apply_local_state(
                            hydrated,
                            configs_signal,
                            tasks_signal,
                            threads_signal,
                            assets_signal,
                            preferences_signal,
                            checkpoint_signal,
                            tombstones,
                        );
                        persist();
                        persist_ui_state();
                        let elapsed_seconds = (js_sys::Date::now() - started_at) / 1_000.0;
                        let remote_seconds = (js_sys::Date::now() - request_started_at) / 1_000.0;
                        status_signal.set(Some(if unresolved_asset_count == 0 {
                            format!(
                                "已完成与 {} 的云端同步，总用时 {:.1} 秒（网络与服务器 {:.1} 秒）。",
                                user.username, elapsed_seconds, remote_seconds
                            )
                        } else {
                            format!(
                                "同步已完成，但有 {unresolved_asset_count} 张图片在当前设备和云端都缺少原文件；其余数据已正常同步。"
                            )
                        }));
                        if let Ok(response) = Request::get(&api_url("/api/auth/me"))
                            .credentials(web_sys::RequestCredentials::Include)
                            .send()
                            .await
                            && let Ok(me) = response.json::<MeResponse>().await
                        {
                            auth_user_signal.set(me.user);
                        }
                    }
                    Err(error) => status_signal.set(Some(format!("同步响应解析失败：{error}"))),
                },
                Ok(response) => {
                    status_signal.set(Some(
                        response.text().await.unwrap_or_else(|_| "同步失败".into()),
                    ));
                }
                Err(error) => status_signal.set(Some(format!("同步失败：{error}"))),
            }
            syncing_signal.set(false);
        });
    };

    let toggle_api_key_sync = move |event: Event| {
        let enabled = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
            .map(|input| input.checked())
            .unwrap_or(false);
        let Some(user) = auth_user.get_untracked() else {
            return;
        };
        sync_api_keys_enabled.set(enabled);
        let _ = save_api_key_sync_enabled(&user.id, enabled);
        if enabled {
            if let Some(secret) = load_trusted_sync_secret(&user.id) {
                sync_secret.set(secret);
                configs.update(|items| mark_api_keys_for_reencryption(items));
                persist_ui_state();
                sync_status_text.set(Some("可信设备已解锁，可同步 API Key。".into()));
            } else {
                sync_status_text.set(Some(
                    "API Key 同步已启用，请输入账号密码解锁当前设备。".into(),
                ));
            }
            return;
        }

        let _ = clear_trusted_sync_secret(&user.id);
        sync_secret.set(String::new());
        legacy_sync_secret.set(String::new());
        configs.update(|items| {
            let updated_at = now_rfc3339();
            for config in items {
                if config.api_key_encrypted.take().is_some() {
                    config.updated_at = updated_at.clone();
                }
            }
        });
        persist_ui_state();
        sync_status_text.set(Some(
            "API Key 同步已关闭；下次手动同步会移除云端密文，本地 Key 保留。".into(),
        ));
    };

    let unlock_api_key_sync = move |_| {
        let Some(user) = auth_user.get_untracked() else {
            sync_status_text.set(Some("请先登录账号。".into()));
            return;
        };
        let password = sync_unlock_password.get_untracked();
        if password.is_empty() {
            sync_status_text.set(Some("请输入当前账号密码。".into()));
            return;
        }
        sync_unlocking.set(true);
        sync_status_text.set(Some("正在验证账号并解锁可信设备……".into()));
        spawn_local(async move {
            let request = Request::post(&api_url("/api/auth/login"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&AuthRequest {
                    username: user.username.clone(),
                    password: password.clone(),
                });
            let result = match request {
                Ok(builder) => builder.send().await,
                Err(error) => {
                    sync_status_text.set(Some(format!("解锁请求序列化失败：{error}")));
                    sync_unlocking.set(false);
                    return;
                }
            };
            match result {
                Ok(response) if response.ok() => {
                    let trusted_secret = derive_trusted_sync_secret(&user.id, &password);
                    match save_trusted_sync_secret(&user.id, &trusted_secret) {
                        Ok(()) => {
                            sync_secret.set(trusted_secret);
                            legacy_sync_secret.set(password);
                            configs.update(|items| mark_api_keys_for_reencryption(items));
                            persist_ui_state();
                            sync_unlock_password.set(String::new());
                            sync_status_text.set(Some("可信设备已解锁，可同步 API Key。".into()));
                        }
                        Err(error) => {
                            sync_status_text.set(Some(format!("可信设备密钥保存失败：{error}")))
                        }
                    }
                }
                Ok(response) => {
                    let raw = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "账号密码验证失败。".into());
                    sync_status_text.set(Some(api_error_message(raw, "账号密码验证失败。")));
                }
                Err(error) => sync_status_text.set(Some(format!("解锁失败：{error}"))),
            }
            sync_unlocking.set(false);
        });
    };

    let check_username_availability = move || {
        let username = login_username.get_untracked();
        let trimmed = username.trim().to_string();
        username_check_message.set(None);
        if trimmed.len() < 3 {
            username_check_message.set(Some("用户名至少 3 个字符。".into()));
            return;
        }
        let username_check_message = username_check_message;
        spawn_local(async move {
            let url = format!(
                "/api/auth/check-username?username={}",
                percent_encode_query_value(&trimmed)
            );
            match Request::get(&api_url(&url)).send().await {
                Ok(response) if response.ok() => {
                    match response.json::<UsernameAvailabilityResponse>().await {
                        Ok(payload) if payload.available => {
                            username_check_message.set(Some("这个用户名可以注册。".into()));
                        }
                        Ok(_) => {
                            username_check_message
                                .set(Some("这个用户名已被使用，请换一个。".into()));
                        }
                        Err(error) => {
                            username_check_message
                                .set(Some(format!("用户名检查解析失败：{error}")));
                        }
                    }
                }
                Ok(response) => {
                    username_check_message.set(Some(
                        response
                            .text()
                            .await
                            .unwrap_or_else(|_| "用户名检查失败，请稍后再试。".into()),
                    ));
                }
                Err(error) => {
                    username_check_message.set(Some(format!("用户名检查失败：{error}")));
                }
            }
        });
    };

    let submit_auth = move |mode: &'static str| {
        let username = login_username.get_untracked();
        let password = login_password.get_untracked();
        auth_form_message.set(None);
        if username.trim().is_empty() || password.is_empty() {
            auth_form_message.set(Some("请先填写用户名和密码。".into()));
            return;
        }
        if mode == "register" {
            let confirm = register_password_confirm.get_untracked();
            if let Err(message) = validate_frontend_password_strength(&password, &confirm) {
                auth_form_message.set(Some(message));
                return;
            }
        }
        status_text.set("正在处理账号状态……".into());
        let auth_user = auth_user;
        let sync_secret = sync_secret;
        let legacy_sync_secret = legacy_sync_secret;
        let sync_api_keys_enabled = sync_api_keys_enabled;
        let sync_status_text = sync_status_text;
        let status_text = status_text;
        let auth_form_message = auth_form_message;
        let admin_setup_allowed = admin_setup_allowed;
        let login_password_signal = login_password;
        let register_password_confirm_signal = register_password_confirm;
        let password_confirm = register_password_confirm.get_untracked();
        let setup_token = admin_setup_token.get_untracked();
        spawn_local(async move {
            let request = if mode == "register" {
                Request::post(&api_url("/api/auth/register"))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&RegisterRequest {
                        username,
                        password: password.clone(),
                        password_confirm,
                        admin_setup_token: non_empty_string(setup_token),
                    })
            } else {
                Request::post(&api_url("/api/auth/login"))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&AuthRequest {
                        username,
                        password: password.clone(),
                    })
            };
            let Ok(builder) = request else {
                auth_form_message.set(Some("认证请求序列化失败。".into()));
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<AuthResponse>().await {
                    Ok(auth) => {
                        let enabled = load_api_key_sync_enabled(&auth.user.id);
                        sync_api_keys_enabled.set(enabled);
                        if enabled {
                            let trusted_secret =
                                derive_trusted_sync_secret(&auth.user.id, &password);
                            if save_trusted_sync_secret(&auth.user.id, &trusted_secret).is_ok() {
                                sync_secret.set(trusted_secret);
                                legacy_sync_secret.set(password);
                                sync_status_text
                                    .set(Some("可信设备已解锁，可同步 API Key。".into()));
                            } else {
                                sync_status_text
                                    .set(Some("登录成功，但浏览器无法保存可信设备密钥。".into()));
                            }
                        } else {
                            sync_secret.set(String::new());
                            legacy_sync_secret.set(String::new());
                        }
                        auth_user.set(Some(auth.user.clone()));
                        login_password_signal.set(String::new());
                        register_password_confirm_signal.set(String::new());
                        if auth.user.role == "admin" {
                            admin_setup_allowed.set(false);
                            show_admin_setup_token.set(false);
                        }
                        status_text.set(auth_status_message(&auth.user));
                    }
                    Err(error) => auth_form_message.set(Some(format!("认证响应解析失败：{error}"))),
                },
                Ok(response) => {
                    let raw = response.text().await.unwrap_or_else(|_| "认证失败".into());
                    auth_form_message.set(Some(api_error_message(raw, "认证失败")));
                }
                Err(error) => auth_form_message.set(Some(format!("认证失败：{error}"))),
            }
        });
    };

    let bootstrap_current_user_as_admin = move |_| {
        auth_form_message.set(None);
        let token = admin_setup_token.get_untracked();
        if token.trim().is_empty() {
            auth_form_message.set(Some("请先填写管理员初始化口令。".into()));
            return;
        }
        if auth_user.get_untracked().is_none() {
            auth_form_message.set(Some("请先登录要升级的旧账号。".into()));
            return;
        }
        status_text.set("正在尝试初始化当前账号为管理员……".into());
        let auth_user = auth_user;
        let status_text = status_text;
        let auth_form_message = auth_form_message;
        let admin_setup_allowed = admin_setup_allowed;
        spawn_local(async move {
            let request = Request::post(&api_url("/api/auth/bootstrap-admin"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&AdminBootstrapRequest {
                    admin_setup_token: token,
                });
            let Ok(builder) = request else {
                auth_form_message.set(Some("管理员初始化请求序列化失败。".into()));
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<AuthResponse>().await {
                    Ok(payload) => {
                        auth_user.set(Some(payload.user.clone()));
                        admin_setup_allowed.set(false);
                        show_admin_setup_token.set(false);
                        status_text.set("当前账号已升级为管理员，可以审批其他用户。".into());
                    }
                    Err(error) => {
                        auth_form_message.set(Some(format!("管理员初始化响应解析失败：{error}")));
                    }
                },
                Ok(response) => {
                    let raw = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "管理员初始化失败。".into());
                    auth_form_message.set(Some(api_error_message(raw, "管理员初始化失败。")));
                }
                Err(error) => auth_form_message.set(Some(format!("管理员初始化失败：{error}"))),
            }
        });
    };

    let change_password = move |_| {
        let old_password = change_old_password.get_untracked();
        let new_password = change_new_password.get_untracked();
        let new_password_confirm = change_new_password_confirm.get_untracked();
        password_form_message.set(None);
        let Some(current_user) = auth_user.get_untracked() else {
            password_form_message.set(Some("请先登录后再修改密码。".into()));
            return;
        };
        if let Err(message) =
            validate_frontend_password_strength(&new_password, &new_password_confirm)
        {
            password_form_message.set(Some(message));
            return;
        }
        if old_password.is_empty() {
            password_form_message.set(Some("请填写当前密码。".into()));
            return;
        }
        status_text.set("正在更新密码……".into());
        let status_text = status_text;
        let password_form_message = password_form_message;
        let sync_secret = sync_secret;
        let legacy_sync_secret = legacy_sync_secret;
        let sync_api_keys_enabled = sync_api_keys_enabled;
        let sync_status_text = sync_status_text;
        let change_old_password = change_old_password;
        let change_new_password = change_new_password;
        let change_new_password_confirm = change_new_password_confirm;
        let previous_sync_secret = sync_secret.get_untracked();
        spawn_local(async move {
            let request = Request::post(&api_url("/api/auth/change-password"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&ChangePasswordRequest {
                    old_password,
                    new_password: new_password.clone(),
                    new_password_confirm,
                });
            let Ok(builder) = request else {
                password_form_message.set(Some("改密请求序列化失败。".into()));
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => {
                    if sync_api_keys_enabled.get_untracked() {
                        let trusted_secret =
                            derive_trusted_sync_secret(&current_user.id, &new_password);
                        if save_trusted_sync_secret(&current_user.id, &trusted_secret).is_ok() {
                            legacy_sync_secret.set(previous_sync_secret);
                            sync_secret.set(trusted_secret);
                            sync_status_text.set(Some(
                                "密码已更新；下次同步会使用新的可信设备密钥重新加密 API Key。"
                                    .into(),
                            ));
                        }
                    }
                    change_old_password.set(String::new());
                    change_new_password.set(String::new());
                    change_new_password_confirm.set(String::new());
                    status_text.set("密码已更新，下次登录请使用新密码。".into());
                }
                Ok(response) => {
                    let raw = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "修改密码失败".into());
                    password_form_message.set(Some(api_error_message(raw, "修改密码失败")));
                }
                Err(error) => password_form_message.set(Some(format!("修改密码失败：{error}"))),
            }
        });
    };

    let refresh_admin_users = move |_| {
        if !auth_user
            .get_untracked()
            .map(|user| user.role == "admin")
            .unwrap_or(false)
        {
            status_text.set("只有管理员可以查看用户管理列表。".into());
            return;
        }
        loading_admin_users.set(true);
        status_text.set("正在刷新用户列表……".into());
        let admin_users = admin_users;
        let loading_admin_users = loading_admin_users;
        let status_text = status_text;
        spawn_local(async move {
            match Request::get(&api_url("/api/admin/users"))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                Ok(response) if response.ok() => {
                    match response.json::<AdminUsersResponse>().await {
                        Ok(payload) => {
                            let count = payload.users.len();
                            admin_users.set(payload.users);
                            status_text.set(format!("已刷新用户列表，共 {count} 个账号。"));
                        }
                        Err(error) => status_text.set(format!("用户列表解析失败：{error}")),
                    }
                }
                Ok(response) => {
                    status_text.set(
                        response
                            .text()
                            .await
                            .unwrap_or_else(|_| "刷新用户列表失败".into()),
                    );
                }
                Err(error) => status_text.set(format!("刷新用户列表失败：{error}")),
            }
            loading_admin_users.set(false);
        });
    };

    let admin_user_action = move |endpoint: &'static str, user_id: String| {
        status_text.set("正在提交管理员操作……".into());
        let status_text = status_text;
        let admin_users = admin_users;
        let loading_admin_users = loading_admin_users;
        spawn_local(async move {
            let request = Request::post(&api_url(endpoint))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&AdminUserActionRequest { user_id });
            let Ok(builder) = request else {
                status_text.set("管理员操作序列化失败。".into());
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => {
                    status_text.set("管理员操作已完成，正在刷新列表。".into());
                    loading_admin_users.set(true);
                    match Request::get(&api_url("/api/admin/users"))
                        .credentials(web_sys::RequestCredentials::Include)
                        .send()
                        .await
                    {
                        Ok(response) if response.ok() => {
                            if let Ok(payload) = response.json::<AdminUsersResponse>().await {
                                admin_users.set(payload.users);
                            }
                        }
                        _ => {}
                    }
                    loading_admin_users.set(false);
                }
                Ok(response) => {
                    status_text.set(
                        response
                            .text()
                            .await
                            .unwrap_or_else(|_| "管理员操作失败".into()),
                    );
                }
                Err(error) => status_text.set(format!("管理员操作失败：{error}")),
            }
        });
    };

    let delete_managed_user = move |user_id: String, x: f64, y: f64| {
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteUser(user_id),
            title: "删除用户与云端数据".into(),
            message: "此操作会永久删除该用户的账号、服务器图片、同步快照和服务商模板；用户浏览器里的本地数据不会被远程删除。是否继续？".into(),
            x,
            y,
        }));
    };

    (
        sync_action,
        toggle_api_key_sync,
        unlock_api_key_sync,
        check_username_availability,
        submit_auth,
        bootstrap_current_user_as_admin,
        change_password,
        refresh_admin_users,
        admin_user_action,
        delete_managed_user,
    )
}
