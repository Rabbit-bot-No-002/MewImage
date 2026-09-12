use super::super::*;

#[allow(clippy::type_complexity)]
pub(crate) fn build_data_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    enqueue_payload_deletes: impl Fn(Vec<String>) + Copy + Send + Sync + 'static,
    commit_current_thread_draft: impl Fn() + Copy + Send + Sync + 'static,
) -> (
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(Event) + Copy + Send + Sync + 'static,
    impl Fn(LocalDataClearScope) + Copy + Send + Sync + 'static,
    impl Fn(CloudDataClearScope) + Copy + Send + Sync + 'static,
    impl Fn(LocalDataClearScope, MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(CloudDataClearScope, MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();
    let persistence = expect_context::<PersistenceState>();
    let configs = workspace.configs;
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let preferences = workspace.preferences;
    let checkpoint = workspace.checkpoint;
    let tombstones = workspace.tombstones;
    let templates = workspace.templates;
    let current_thread_id = workspace.current_thread_id;
    let current_config_id = workspace.current_config_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let continuation_asset_id = composer.continuation_asset_id;
    let continuation_task_id = composer.continuation_task_id;
    let conversation_rebase_requested = composer.conversation_rebase_requested;
    let queue_mode_enabled = composer.queue_mode_enabled;
    let draft_prompt = composer.draft_prompt;
    let status_text = composer.status_text;
    let generating = composer.generating;
    let auth_user = account.auth_user;
    let sync_secret = account.sync_secret;
    let legacy_sync_secret = account.legacy_sync_secret;
    let sync_status_text = account.sync_status_text;
    let data_management_busy = ui.data_management_busy;
    let data_management_message = ui.data_management_message;
    let cloud_data_stats = ui.cloud_data_stats;
    let show_settings = ui.show_settings;
    let show_settings_menu = ui.show_settings_menu;
    let preview_state = ui.preview_state;
    let preview_panel_state = ui.preview_panel_state;
    let gallery_page = ui.gallery_page;
    let confirm_popover = ui.confirm_popover;
    let payload_write_queue = persistence.payload_write_queue;
    let payload_delete_queue = persistence.payload_delete_queue;

    let refresh_cloud_data_stats = move || {
        let approved = auth_user
            .get_untracked()
            .map(|user| user.status == "approved")
            .unwrap_or(false);
        if !approved {
            data_management_message.set(Some("账号审批通过后才能查看云端数据。".into()));
            return;
        }
        data_management_busy.set(true);
        data_management_message.set(Some("正在读取云端数据统计……".into()));
        spawn_local(async move {
            match Request::get(&api_url("/api/data/stats"))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                Ok(response) if response.ok() => {
                    match response.json::<CloudDataStatsResponse>().await {
                        Ok(stats) => {
                            cloud_data_stats.set(Some(stats));
                            data_management_message.set(Some("云端数据统计已刷新。".into()));
                        }
                        Err(error) => {
                            data_management_message.set(Some(format!("云端统计解析失败：{error}")))
                        }
                    }
                }
                Ok(response) => data_management_message.set(Some(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "读取云端统计失败。".into()),
                )),
                Err(error) => {
                    data_management_message.set(Some(format!("读取云端统计失败：{error}")))
                }
            }
            data_management_busy.set(false);
        });
    };

    let export_local_backup = move |_| {
        if generating.get_untracked() {
            data_management_message.set(Some("图片正在生成，请完成后再导出。".into()));
            return;
        }
        if data_management_busy.get_untracked() {
            return;
        }
        commit_current_thread_draft();
        data_management_busy.set(true);
        data_management_message.set(Some("正在读取本地图片并生成备份……".into()));
        let state = snapshot_local_state(
            configs,
            tasks,
            threads,
            assets,
            preferences,
            checkpoint,
            tombstones,
        );
        spawn_local(async move {
            let result: Result<Vec<u8>, String> = async {
                let payloads = collect_backup_payloads(&state).await?;
                data_management::build_backup(state, &payloads)
            }
            .await;
            let file_name = format!("mew-image-backup-{}.zip", today_compact());
            match result.and_then(|bytes| download_backup_bytes(&bytes, &file_name)) {
                Ok(()) => data_management_message
                    .set(Some("备份已生成，请在浏览器下载记录中确认保存。".into())),
                Err(error) => data_management_message.set(Some(format!("导出失败：{error}"))),
            }
            data_management_busy.set(false);
        });
    };

    let export_session_backup = move |thread_id: String| {
        if generating.get_untracked() {
            let message = "图片正在生成，请完成后再导出会话。".to_string();
            data_management_message.set(Some(message.clone()));
            status_text.set(message);
            return;
        }
        if data_management_busy.get_untracked() {
            return;
        }
        commit_current_thread_draft();
        let state = snapshot_local_state(
            configs,
            tasks,
            threads,
            assets,
            preferences,
            checkpoint,
            tombstones,
        );
        let prepared = match data_management::prepare_session_backup(&state, &thread_id) {
            Ok(prepared) => prepared,
            Err(error) => {
                data_management_message.set(Some(format!("导出失败：{error}")));
                status_text.set(format!("会话导出失败：{error}"));
                return;
            }
        };
        let thread_title = prepared
            .threads
            .first()
            .map(thread_display_name)
            .unwrap_or_else(|| "新的会话".into());
        let file_name = session_backup_file_name(&thread_title);
        data_management_busy.set(true);
        data_management_message.set(Some(format!("正在导出会话“{thread_title}”……")));
        status_text.set(format!("正在整理会话“{thread_title}”的项目包……"));
        spawn_local(async move {
            let result: Result<Vec<u8>, String> = async {
                let payloads = collect_backup_payloads(&prepared).await?;
                data_management::build_session_backup(prepared, &payloads)
            }
            .await;
            match result.and_then(|bytes| download_backup_bytes(&bytes, &file_name)) {
                Ok(()) => {
                    data_management_message.set(Some(format!(
                        "会话“{thread_title}”已导出，请在浏览器下载记录中确认保存。"
                    )));
                    status_text.set(format!("会话“{thread_title}”项目包已生成。"));
                }
                Err(error) => {
                    data_management_message.set(Some(format!("导出失败：{error}")));
                    status_text.set(format!("会话导出失败：{error}"));
                }
            }
            data_management_busy.set(false);
        });
    };

    let import_local_backup = move |event: Event| {
        if generating.get_untracked() {
            data_management_message.set(Some("图片正在生成，请完成后再导入。".into()));
            return;
        }
        let Some(input) = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
        else {
            return;
        };
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        input.set_value("");
        data_management_busy.set(true);
        data_management_message.set(Some("正在校验并合并本地备份……".into()));
        let local = snapshot_local_state(
            configs,
            tasks,
            threads,
            assets,
            preferences,
            checkpoint,
            tombstones,
        );
        spawn_local(async move {
            let file = File::from(file);
            let result = read_as_bytes(&file)
                .await
                .map_err(|error| error.to_string())
                .and_then(|bytes| data_management::import_backup(&bytes, &local));
            match result {
                Ok(mut imported) => {
                    let imported_payload_map =
                        imported.payloads.iter().cloned().collect::<HashMap<_, _>>();
                    merge_asset_payloads(&mut imported.state.assets, &imported_payload_map);
                    if let Err(error) = apply_asset_payload_changes(&imported.payloads, &[]).await {
                        let imported_payload_ids = imported
                            .payloads
                            .iter()
                            .map(|(asset_id, _)| asset_id.clone())
                            .collect::<Vec<_>>();
                        // 导入器只返回新 ID 的 payload，因此可以安全清理已成功的前序批次。
                        let _ = apply_asset_payload_changes(&[], &imported_payload_ids).await;
                        data_management_message.set(Some(format!(
                            "导入失败：图片原文件未能写入浏览器存储：{error}。现有工作区未被修改。"
                        )));
                        data_management_busy.set(false);
                        return;
                    }
                    for asset in &mut imported.state.assets {
                        asset.data_url = None;
                    }
                    reconcile_task_integrity(
                        &mut imported.state.tasks,
                        &imported.state.assets,
                        true,
                    );
                    let is_session_backup =
                        imported.backup_kind == data_management::BackupKind::Session;
                    let target_thread = imported
                        .imported_thread_id
                        .as_ref()
                        .and_then(|thread_id| {
                            imported
                                .state
                                .threads
                                .iter()
                                .find(|thread| thread.id == *thread_id)
                        })
                        .cloned()
                        .or_else(|| imported.state.threads.first().cloned())
                        .unwrap_or_else(default_thread);
                    current_thread_id.set(target_thread.id.clone());
                    if !is_session_backup {
                        current_config_id.set(
                            imported
                                .state
                                .configs
                                .first()
                                .map(|config| config.id.clone())
                                .unwrap_or_default(),
                        );
                    }
                    draft_prompt.set(target_thread.draft_prompt.clone());
                    composer.editing_by_thread.update(HashMap::clear);
                    selected_reference_ids.set(Vec::new());
                    continuation_asset_id.set(None);
                    continuation_task_id.set(None);
                    conversation_rebase_requested.set(false);
                    reference_menu_asset_id.set(None);
                    apply_local_state(
                        imported.state,
                        configs,
                        tasks,
                        threads,
                        assets,
                        preferences,
                        checkpoint,
                        tombstones,
                    );
                    persist_state();
                    persist_ui_state();
                    if is_session_backup {
                        show_settings.set(false);
                        show_settings_menu.set(false);
                        let title = imported
                            .imported_thread_title
                            .unwrap_or_else(|| thread_display_name(&target_thread));
                        let message = format!(
                            "会话“{title}”已作为新副本导入，共 {} 条任务、{} 张图片。",
                            imported.imported_task_count, imported.imported_asset_count,
                        );
                        data_management_message.set(Some(message.clone()));
                        status_text.set(message);
                    } else {
                        data_management_message.set(Some(format!(
                            "导入完成：读取 {} 条任务、{} 个图片引用，复用 {} 张重复图片。",
                            imported.imported_task_count,
                            imported.imported_asset_count,
                            imported.deduplicated_asset_count,
                        )));
                    }
                }
                Err(error) => data_management_message.set(Some(format!("导入失败：{error}"))),
            }
            data_management_busy.set(false);
        });
    };

    let perform_clear_local_data = move |scope: LocalDataClearScope| {
        if generating.get_untracked() {
            data_management_message.set(Some("图片正在生成，请完成后再清除数据。".into()));
            return;
        }
        data_management_busy.set(true);
        let clear_workspace = matches!(
            scope,
            LocalDataClearScope::Workspace | LocalDataClearScope::All
        );
        let clear_configs = matches!(
            scope,
            LocalDataClearScope::Configs | LocalDataClearScope::All
        );
        let clear_preferences = matches!(
            scope,
            LocalDataClearScope::Preferences | LocalDataClearScope::All
        );
        if clear_workspace {
            composer.editing_by_thread.update(HashMap::clear);
            crate::image_editor::invalidate_pending_draft_writes();
            ui.image_editor_thread.set(None);
            payload_write_queue.set(HashMap::new());
            payload_delete_queue.set(HashSet::new());
            tasks.set(Vec::new());
            assets.set(Vec::new());
            let thread = default_thread();
            current_thread_id.set(thread.id.clone());
            threads.set(vec![thread]);
            checkpoint.set(SyncCheckpoint::default());
            tombstones.set(Vec::new());
            draft_prompt.set(String::new());
            selected_reference_ids.set(Vec::new());
            continuation_asset_id.set(None);
            continuation_task_id.set(None);
            conversation_rebase_requested.set(false);
            reference_menu_asset_id.set(None);
            preview_state.set(None);
            preview_panel_state.set(None);
            gallery_page.set(1);
            preferences.update(|value| {
                value.appearance.custom_background = Default::default();
            });
            ui.appearance_message.set(None);
        }
        if clear_configs {
            let config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
            current_config_id.set(config.id.clone());
            configs.set(vec![config]);
            if let Some(user) = auth_user.get_untracked() {
                let _ = clear_trusted_sync_secret(&user.id);
                sync_secret.set(String::new());
                legacy_sync_secret.set(String::new());
                sync_status_text.set(Some(
                    "本地服务商配置和可信设备密钥已清除，云端密文未受影响。".into(),
                ));
            }
        }
        if clear_preferences {
            let theme_background_ids = assets.with_untracked(|items| {
                items
                    .iter()
                    .filter(|asset| is_theme_background(asset))
                    .map(|asset| asset.id.clone())
                    .collect::<Vec<_>>()
            });
            if !theme_background_ids.is_empty() {
                assets.update(|items| items.retain(|asset| !is_theme_background(asset)));
                record_sync_tombstones(
                    tombstones,
                    theme_background_ids
                        .iter()
                        .cloned()
                        .map(|id| (SyncEntityKind::Asset, id)),
                );
                enqueue_payload_deletes(theme_background_ids);
            }
            preferences.set(AppPreferences::default());
            ui.appearance_message.set(None);
            queue_mode_enabled.set(false);
            let _ = clear_generation_queue_mode();
            tasks.update(|items| {
                for task in items.iter_mut().filter(|task| task.favorite) {
                    task.favorite_folder_id = Some(DEFAULT_FAVORITE_FOLDER_ID.into());
                    task.updated_at = now_rfc3339();
                }
            });
        }
        // 若此前有写盘仍在执行，保留 pending 可保证旧快照完成后再落一次当前空状态。
        if clear_workspace || clear_preferences {
            persist_state();
        }
        if clear_configs || clear_preferences || clear_workspace {
            persist_ui_state();
        }
        spawn_local(async move {
            let mut errors = Vec::new();
            if clear_workspace && let Err(error) = crate::image_editor::clear_drafts().await {
                errors.push(format!("清除编辑草稿失败：{error}"));
            }
            if clear_workspace && let Err(error) = clear_asset_payloads().await {
                errors.push(format!("清除图片失败：{error}"));
            }
            data_management_message.set(Some(if errors.is_empty() {
                "所选本地数据已清除，云端数据未受影响。".into()
            } else {
                errors.join("；")
            }));
            data_management_busy.set(false);
        });
    };

    let perform_clear_cloud_data = move |scope: CloudDataClearScope| {
        data_management_busy.set(true);
        data_management_message.set(Some("正在清除所选云端数据……".into()));
        let clears_sync_data = matches!(
            scope,
            CloudDataClearScope::SyncData | CloudDataClearScope::All
        );
        spawn_local(async move {
            let request = Request::post(&api_url("/api/data/clear"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&CloudDataClearRequest { scope });
            let result = match request {
                Ok(builder) => builder.send().await,
                Err(error) => {
                    data_management_message.set(Some(format!("清除请求序列化失败：{error}")));
                    data_management_busy.set(false);
                    return;
                }
            };
            match result {
                Ok(response) if response.ok() => {
                    if clears_sync_data {
                        assets.update(|items| {
                            for asset in items {
                                if asset.remote_object_key.is_none() && asset.remote_url.is_none() {
                                    continue;
                                }
                                asset.remote_object_key = None;
                                asset.remote_url = None;
                                asset.updated_at = now_rfc3339();
                            }
                        });
                        checkpoint.set(SyncCheckpoint::default());
                        persist_state();
                    }
                    match response.json::<CloudDataStatsResponse>().await {
                        Ok(stats) => {
                            cloud_data_stats.set(Some(stats));
                            data_management_message.set(Some(if clears_sync_data {
                                "云端同步数据与图片已清除；本地原图仍保留，下次同步会重新上传。"
                                    .into()
                            } else {
                                "所选云端数据已清除，本地工作区未受影响。".into()
                            }));
                        }
                        Err(error) => {
                            data_management_message.set(Some(format!("清除结果解析失败：{error}")))
                        }
                    }
                }
                Ok(response) => data_management_message.set(Some(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "清除云端数据失败。".into()),
                )),
                Err(error) => {
                    data_management_message.set(Some(format!("清除云端数据失败：{error}")))
                }
            }
            data_management_busy.set(false);
        });
    };

    let confirm_local_clear = move |scope: LocalDataClearScope, event: MouseEvent| {
        let (title, message) = local_clear_confirmation(scope);
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::ClearLocalData(scope),
            title: title.into(),
            message: message.into(),
            x: event.client_x() as f64,
            y: event.client_y() as f64,
        }));
    };

    let confirm_cloud_clear = move |scope: CloudDataClearScope, event: MouseEvent| {
        let (title, message) = cloud_clear_confirmation(&scope);
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::ClearCloudData(scope),
            title: title.into(),
            message: message.into(),
            x: event.client_x() as f64,
            y: event.client_y() as f64,
        }));
    };

    let add_config = move |_| {
        let template = templates
            .get_untracked()
            .first()
            .cloned()
            .unwrap_or_else(ProviderTemplate::builtin_openai);
        configs.update(|items| {
            let mut config = default_config(&template.id);
            config.name = "新配置001".into();
            config.base_url = template.base_url.clone();
            config.provider_kind = template.kind;
            config.known_requires_proxy = template.known_requires_proxy;
            normalize_api_config(&mut config);
            items.push(config);
            if let Some(last) = items.last() {
                current_config_id.set(last.id.clone());
            }
        });
        persist_ui_state();
    };

    let perform_delete_config = move |config_id: String| {
        if !configs.with_untracked(|items| items.iter().any(|config| config.id == config_id)) {
            return;
        }
        configs.update(|items| {
            items.retain(|config| config.id != config_id);
        });
        record_sync_tombstones(tombstones, [(SyncEntityKind::Config, config_id.clone())]);
        let next_id = configs
            .get_untracked()
            .first()
            .map(|config| config.id.clone())
            .unwrap_or_default();
        current_config_id.set(next_id);
        persist_state();
        persist_ui_state();
    };

    let delete_config = move |event: MouseEvent| {
        let Some(current) = configs.with_untracked(|items| {
            items
                .iter()
                .find(|config| config.id == current_config_id.get_untracked())
                .cloned()
        }) else {
            return;
        };
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteConfig(current.id),
            title: "删除配置".into(),
            message: format!("删除配置「{}」后无法恢复，是否继续？", current.name),
            x: event.client_x() as f64,
            y: event.client_y() as f64,
        }));
    };

    (
        refresh_cloud_data_stats,
        export_local_backup,
        export_session_backup,
        import_local_backup,
        perform_clear_local_data,
        perform_clear_cloud_data,
        confirm_local_clear,
        confirm_cloud_clear,
        add_config,
        perform_delete_config,
        delete_config,
    )
}
