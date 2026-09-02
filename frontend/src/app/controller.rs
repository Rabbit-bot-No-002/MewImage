use super::*;

#[component]
pub(super) fn AppController() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let persistence = expect_context::<PersistenceState>();

    let WorkspaceState {
        configs,
        tasks,
        threads,
        assets,
        preferences,
        checkpoint,
        tombstones,
        current_thread_id,
        current_config_id,
        ..
    } = workspace;
    let ComposerState {
        draft_prompt,
        draft_prompt_ref,
        status_text,
        ..
    } = composer;
    let persist_state = move || {
        request_workspace_persist(tasks, threads, assets, checkpoint, tombstones, persistence);
    };
    let persist_ui_state = move || {
        request_ui_state_persist(configs, preferences, persistence);
    };
    let enqueue_payload_deletes = {
        move |asset_ids: Vec<String>| {
            if asset_ids.is_empty()
                || !persistence
                    .local_state_status
                    .with_untracked(LocalStateLoadStatus::is_ready)
            {
                return;
            }
            // 元数据删除后这些 URL 已不再可达，无需等待 IndexedDB 删除重试才释放内存。
            for asset_id in &asset_ids {
                revoke_asset_object_url(asset_id);
            }
            persistence.payload_write_queue.update(|queued| {
                for asset_id in &asset_ids {
                    queued.remove(asset_id);
                }
            });
            persistence.payload_delete_queue.update(|queued| {
                for asset_id in asset_ids {
                    queued.insert(asset_id);
                }
            });
            request_payload_flush(persistence, status_text);
        }
    };

    let derived = AppDerived::new(workspace, composer, ui);
    provide_context(derived);
    super::effects::install_app_effects();

    let build_preview_panel_state = move |task_id: &str, asset_id: Option<&str>| {
        let task =
            tasks.with_untracked(|items| items.iter().find(|task| task.id == task_id).cloned())?;
        let asset = asset_id.and_then(|asset_id| {
            assets.with_untracked(|items| items.iter().find(|asset| asset.id == asset_id).cloned())
        });
        if asset_id.is_some() && asset.is_none() {
            return None;
        }
        let preview_config = configs.with_untracked(|items| {
            items
                .iter()
                .find(|config| config.id == task.config_id)
                .cloned()
        });
        let moderation_label = task
            .generation_settings
            .as_ref()
            .and_then(|settings| settings.moderation.clone())
            .or_else(|| {
                preview_config
                    .as_ref()
                    .and_then(|config| config.moderation.clone())
            })
            .unwrap_or_else(|| "auto".into());
        let background_label = task
            .generation_settings
            .as_ref()
            .map(|settings| settings.background.as_deref().unwrap_or("auto"))
            .or_else(|| {
                preview_config
                    .as_ref()
                    .and_then(|config| config.background.as_deref())
            })
            .map(|value| match normalized_background_mode(Some(value)) {
                "transparent" => "API 原生透明",
                "local" => "本地去背景",
                _ => "自动",
            })
            .unwrap_or("自动")
            .to_string();
        let source_label = preview_config
            .as_ref()
            .map(|config| config.name.clone())
            .unwrap_or_else(|| "默认配置".into());
        let requested_quality_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.requested_quality.clone())
            .or_else(|| {
                task.generation_settings
                    .as_ref()
                    .and_then(|settings| settings.quality.clone())
            })
            .unwrap_or_else(|| "未设置".into());
        let actual_quality_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.actual_quality.clone())
            .unwrap_or_else(|| {
                if task.status == TaskStatus::Running {
                    "等待结果".into()
                } else {
                    "未记录".into()
                }
            });
        let duration_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.duration_ms)
            .map(format_duration_ms)
            .unwrap_or_else(|| {
                if task.status == TaskStatus::Running {
                    "进行中".into()
                } else {
                    "未记录".into()
                }
            });
        let reference_thumbs = assets.with_untracked(|items| {
            task.reference_asset_ids
                .iter()
                .filter_map(|id| {
                    items
                        .iter()
                        .find(|asset| asset.id == *id)
                        .map(|asset| PreviewReferenceThumb {
                            id: id.clone(),
                            src: asset_display_src(asset),
                        })
                })
                .collect::<Vec<_>>()
        });
        Some(PreviewPanelState {
            task_id: task.id.clone(),
            asset_id: asset.as_ref().map(|asset| asset.id.clone()),
            prompt: task.prompt.clone(),
            display_src: asset.as_ref().map(asset_display_src),
            width: asset
                .as_ref()
                .and_then(|asset| asset.width)
                .or_else(|| {
                    task.generation_settings
                        .as_ref()
                        .map(|settings| settings.width)
                })
                .unwrap_or(0),
            height: asset
                .as_ref()
                .and_then(|asset| asset.height)
                .or_else(|| {
                    task.generation_settings
                        .as_ref()
                        .map(|settings| settings.height)
                })
                .unwrap_or(0),
            source_label,
            requested_model: task.requested_model.clone(),
            moderation_label,
            background_label,
            requested_quality_label,
            actual_quality_label,
            format_label: asset
                .as_ref()
                .map(|asset| asset.mime_type.replace("image/", ""))
                .or_else(|| {
                    task.generation_settings
                        .as_ref()
                        .and_then(|settings| settings.output_format.clone())
                })
                .unwrap_or_else(|| "未设置".into()),
            image_count: task
                .generation_settings
                .as_ref()
                .map(|settings| settings.count as usize)
                .or_else(|| task.result.as_ref().map(|result| result.images.len()))
                .unwrap_or(1),
            created_at: task.created_at.clone(),
            duration_label,
            favorite: task.favorite,
            reference_thumbs,
        })
    };

    let update_current_config = move |updater: fn(&mut EncryptedApiConfig, String),
                                      value: String| {
        configs.update(|items| {
            if let Some(config) = items
                .iter_mut()
                .find(|config| config.id == current_config_id.get_untracked())
            {
                updater(config, value.clone());
                config.updated_at = now_rfc3339();
            }
        });
        persist_ui_state();
    };

    let commit_current_thread_draft = move || {
        let thread_id = current_thread_id.get_untracked();
        if thread_id.is_empty() {
            return;
        }
        let value = draft_prompt_ref
            .get()
            .map(|textarea: HtmlTextAreaElement| textarea.value())
            .unwrap_or_else(|| draft_prompt.get_untracked());
        draft_prompt.set(value.clone());
        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id)
                && thread.draft_prompt != value
            {
                thread.draft_prompt = value;
                thread.updated_at = now_rfc3339();
            }
        });
    };

    let (
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
    ) = build_account_actions(persist_state, persist_ui_state, enqueue_payload_deletes);
    let (
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
    ) = build_data_actions(
        persist_state,
        persist_ui_state,
        enqueue_payload_deletes,
        commit_current_thread_draft,
    );

    let (
        new_thread,
        rename_thread,
        perform_delete_thread,
        delete_thread,
        select_thread,
        import_reference_assets,
        open_reference_menu,
        reorder_selected_references,
        perform_delete_asset,
        delete_asset,
        continue_from_task,
        enter_continuation_context,
        perform_delete_task,
        delete_task,
        open_preview,
        open_text_popover,
    ) = build_workspace_actions(
        persist_state,
        enqueue_payload_deletes,
        commit_current_thread_draft,
        build_preview_panel_state,
    );

    let (import_theme_background, perform_delete_theme_background, request_delete_theme_background) =
        build_appearance_actions(persist_state, persist_ui_state, enqueue_payload_deletes);

    let (run_generation, rerun_task, cancel_generation, cancel_all_generations) =
        build_generation_actions(persist_state, commit_current_thread_draft);
    let generate = move |_| run_generation();

    let (
        select_favorite_folder,
        add_favorite_folder,
        rename_favorite_folder,
        delete_favorite_folder,
        submit_text_popover,
        submit_confirm_popover,
        assign_favorite_folder,
        cancel_favorite_for_task,
        toggle_favorite_for_task,
        close_preview,
        edit_output_asset,
        show_tip,
        hide_tip,
        reference_tip_enabled,
        open_failure_log,
    ) = build_preview_actions(
        persist_state,
        persist_ui_state,
        open_text_popover,
        perform_delete_asset,
        perform_delete_config,
        perform_delete_thread,
        perform_delete_task,
        perform_delete_theme_background,
        perform_clear_local_data,
        perform_clear_cloud_data,
        admin_user_action,
        enter_continuation_context,
        cancel_generation,
        cancel_all_generations,
    );

    let local_state_status = persistence.local_state_status;
    let reload_after_load_failure = move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    };

    view! {
        <ThemeBackdrop />
        <Show when=move || {
            !local_state_status.with(LocalStateLoadStatus::is_ready)
        }>
            <div class="local-state-gate" role="alertdialog" aria-modal="true">
                <div class="local-state-gate-card">
                    <span class="material-symbols-rounded" aria-hidden="true">
                        {move || match local_state_status.get() {
                            LocalStateLoadStatus::Failed(_) => "database_off",
                            LocalStateLoadStatus::Loading | LocalStateLoadStatus::Ready => "database",
                        }}
                    </span>
                    <h2>
                        {move || match local_state_status.get() {
                            LocalStateLoadStatus::Failed(_) => "本地数据暂时无法读取",
                            LocalStateLoadStatus::Loading | LocalStateLoadStatus::Ready => "正在恢复本地工作区",
                        }}
                    </h2>
                    <p>
                        {move || match local_state_status.get() {
                            LocalStateLoadStatus::Failed(error) => format!(
                                "为避免空数据覆盖原工作区，当前页面已停止写入。请关闭其他 MewImage 页面后刷新重试。错误：{error}"
                            ),
                            LocalStateLoadStatus::Loading | LocalStateLoadStatus::Ready => {
                                "正在读取会话、任务和图片索引，请稍候……".into()
                            }
                        }}
                    </p>
                    <Show when=move || matches!(
                        local_state_status.get(),
                        LocalStateLoadStatus::Failed(_)
                    )>
                        <button class="button primary" on:click=reload_after_load_failure>
                            <span class="material-symbols-rounded" aria-hidden="true">"refresh"</span>
                            "刷新重试"
                        </button>
                    </Show>
                </div>
            </div>
        </Show>
        <div
            class="shell shell-single"
            inert=move || !local_state_status.with(LocalStateLoadStatus::is_ready)
        >
            <TopBar persist_ui_state=persist_ui_state />
            <SettingsOverlay
                add_config=add_config
                admin_user_action=admin_user_action
                bootstrap_current_user_as_admin=bootstrap_current_user_as_admin
                change_password=change_password
                check_username_availability=check_username_availability
                confirm_cloud_clear=confirm_cloud_clear
                confirm_local_clear=confirm_local_clear
                delete_config=delete_config
                delete_managed_user=delete_managed_user
                export_local_backup=export_local_backup
                export_session_backup=export_session_backup
                import_local_backup=import_local_backup
                import_theme_background=import_theme_background
                persist_ui_state=persist_ui_state
                refresh_admin_users=refresh_admin_users
                refresh_cloud_data_stats=refresh_cloud_data_stats
                request_delete_theme_background=request_delete_theme_background
                submit_auth=submit_auth
                sync_action=sync_action
                toggle_api_key_sync=toggle_api_key_sync
                unlock_api_key_sync=unlock_api_key_sync
            />
            <FavoritesOverlay
                select_favorite_folder=select_favorite_folder
                add_favorite_folder=add_favorite_folder
                rename_favorite_folder=rename_favorite_folder
                delete_favorite_folder=delete_favorite_folder
                open_preview=open_preview
                enter_continuation_context=enter_continuation_context
                rerun_task=rerun_task
                toggle_favorite_for_task=toggle_favorite_for_task
                open_failure_log=open_failure_log
                delete_task=delete_task
            />
            <GlobalPopovers
                submit_text_popover=submit_text_popover
                submit_confirm_popover=submit_confirm_popover
                assign_favorite_folder=assign_favorite_folder
                cancel_favorite_for_task=cancel_favorite_for_task
            />
            <main class="workspace-layout">
                <GallerySidebar
                    open_preview=open_preview
                    enter_continuation_context=enter_continuation_context
                    rerun_task=rerun_task
                    toggle_favorite_for_task=toggle_favorite_for_task
                    open_failure_log=open_failure_log
                    delete_task=delete_task
                />
                <WorkspaceMain
                    commit_current_thread_draft=commit_current_thread_draft
                    delete_asset=delete_asset
                    delete_thread=delete_thread
                    export_session_backup=export_session_backup
                    generate=generate
                    import_reference_assets=import_reference_assets
                    new_thread=new_thread
                    open_reference_menu=open_reference_menu
                    persist_state=persist_state
                    persist_ui_state=persist_ui_state
                    rename_thread=rename_thread
                    reorder_selected_references=reorder_selected_references
                    select_thread=select_thread
                    update_current_config=update_current_config
                />
            </main>

            <ReferenceMenuOverlay delete_asset=delete_asset />

            <PreviewOverlay
                close_preview=close_preview
                continue_from_task=continue_from_task
                delete_task=delete_task
                edit_output_asset=edit_output_asset
                hide_tip=hide_tip
                reference_tip_enabled=reference_tip_enabled
                show_tip=show_tip
                toggle_favorite_for_task=toggle_favorite_for_task
            />
            <ContextMenuOverlay edit_output_asset=edit_output_asset />

            <FailureLogOverlay delete_task=delete_task />
            <FloatingTipOverlay />
        </div>
    }
}
