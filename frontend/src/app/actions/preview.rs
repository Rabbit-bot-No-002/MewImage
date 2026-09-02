use super::super::*;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn build_preview_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    open_text_popover: impl Fn(TextPopoverKind, &'static str, String, f64, f64)
    + Copy
    + Send
    + Sync
    + 'static,
    perform_delete_asset: impl Fn(String) + Copy + Send + Sync + 'static,
    perform_delete_config: impl Fn(String) + Copy + Send + Sync + 'static,
    perform_delete_thread: impl Fn(String) + Copy + Send + Sync + 'static,
    perform_delete_task: impl Fn(String) + Copy + Send + Sync + 'static,
    perform_delete_theme_background: impl Fn() + Copy + Send + Sync + 'static,
    perform_clear_local_data: impl Fn(LocalDataClearScope) + Copy + Send + Sync + 'static,
    perform_clear_cloud_data: impl Fn(CloudDataClearScope) + Copy + Send + Sync + 'static,
    admin_user_action: impl Fn(&'static str, String) + Copy + Send + Sync + 'static,
    enter_continuation_context: impl Fn(String, String) + Copy + Send + Sync + 'static,
    cancel_generation: impl Fn(String) + Copy + Send + Sync + 'static,
    cancel_all_generations: impl Fn() + Copy + Send + Sync + 'static,
) -> (
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(String, String) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(String, String) + Copy + Send + Sync + 'static,
    impl Fn(&'static str, f64, f64, bool) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn() -> bool + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let preferences = workspace.preferences;
    let current_thread_id = workspace.current_thread_id;
    let status_text = composer.status_text;
    let favorite_folder_picker = ui.favorite_folder_picker;
    let text_popover = ui.text_popover;
    let text_popover_value = ui.text_popover_value;
    let confirm_popover = ui.confirm_popover;
    let preview_state = ui.preview_state;
    let preview_panel_state = ui.preview_panel_state;
    let preview_fullscreen = ui.preview_fullscreen;
    let preview_zoom = ui.preview_zoom;
    let preview_offset_x = ui.preview_offset_x;
    let preview_offset_y = ui.preview_offset_y;
    let preview_dragging = ui.preview_dragging;
    let context_menu_state = ui.context_menu_state;
    let show_settings = ui.show_settings;
    let floating_tip_state = ui.floating_tip_state;
    let floating_tip_token = ui.floating_tip_token;
    let failure_log_state = ui.failure_log_state;

    let open_failure_log = move |task_id: String| {
        let Some(task) =
            tasks.with_untracked(|items| items.iter().find(|task| task.id == task_id).cloned())
        else {
            return;
        };
        let raw_response = task
            .result
            .as_ref()
            .and_then(|result| result.raw_response_json.as_ref())
            .map(format_failure_raw_response)
            .unwrap_or_else(|| "无原始响应 JSON".into());
        let details = format!(
            "任务ID: {}\n状态: {:?}\n错误: {}\n创建时间: {}\n更新时间: {}\n\n原始响应:\n{}",
            task.id,
            task.status,
            task.error_message
                .clone()
                .unwrap_or_else(|| "无错误信息".into()),
            task.created_at,
            task.updated_at,
            raw_response,
        );
        failure_log_state.set(Some(FailureLogState {
            task_id: task.id.clone(),
            title: format!("失败日志：{}", task.prompt),
            summary: task.error_message.unwrap_or_else(|| "生成失败".into()),
            details,
        }));
    };

    let select_favorite_folder = move |folder_id: String| {
        preferences.update(|value| {
            value.favorite_folders = normalized_favorite_folders(value.favorite_folders.clone());
            value.active_favorite_folder_id = Some(folder_id);
        });
        persist_ui_state();
    };

    let add_favorite_folder = move |x: f64, y: f64| {
        let folders = normalized_favorite_folders(preferences.get_untracked().favorite_folders);
        let default_name = format!("文件夹 {}", folders.len() + 1);
        open_text_popover(
            TextPopoverKind::AddFavoriteFolder,
            "新增收藏文件夹",
            default_name,
            x,
            y,
        );
    };

    let rename_favorite_folder = move |folder_id: String, x: f64, y: f64| {
        let current_name = preferences
            .get_untracked()
            .favorite_folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .map(|folder| folder.name.clone())
            .unwrap_or_else(|| "默认".into());
        open_text_popover(
            TextPopoverKind::RenameFavoriteFolder(folder_id),
            "重命名收藏文件夹",
            current_name,
            x,
            y,
        );
    };

    let perform_delete_favorite_folder = move |folder_id: String| {
        preferences.update(|value| {
            value.favorite_folders = normalized_favorite_folders(value.favorite_folders.clone());
            if folder_id == DEFAULT_FAVORITE_FOLDER_ID {
                return;
            }
            let deleted_at = now_rfc3339();
            value
                .favorite_folders
                .retain(|folder| folder.id != folder_id);
            if let Some(tombstone) = value
                .favorite_folder_tombstones
                .iter_mut()
                .find(|item| item.folder_id == folder_id)
            {
                tombstone.deleted_at = deleted_at;
            } else {
                value
                    .favorite_folder_tombstones
                    .push(FavoriteFolderTombstone {
                        folder_id: folder_id.clone(),
                        deleted_at,
                    });
            }
            if value.active_favorite_folder_id.as_deref() == Some(folder_id.as_str()) {
                value.active_favorite_folder_id = Some(DEFAULT_FAVORITE_FOLDER_ID.into());
            }
        });
        tasks.update(|items| {
            for task in items {
                if task.favorite_folder_id.as_deref() == Some(folder_id.as_str()) {
                    task.favorite_folder_id = Some(DEFAULT_FAVORITE_FOLDER_ID.into());
                    task.updated_at = now_rfc3339();
                }
            }
        });
        persist_state();
        persist_ui_state();
    };

    let delete_favorite_folder = move |folder_id: String, x: f64, y: f64| {
        if folder_id == DEFAULT_FAVORITE_FOLDER_ID {
            return;
        }
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteFavoriteFolder(folder_id),
            title: "删除收藏文件夹".into(),
            message: "删除文件夹后，其中收藏图片会移动到“默认”文件夹，是否继续？".into(),
            x,
            y,
        }));
    };

    let submit_text_popover = move || {
        let Some(state) = text_popover.get_untracked() else {
            return;
        };
        let next_name = text_popover_value.get_untracked().trim().to_string();
        if next_name.is_empty() {
            text_popover.set(None);
            return;
        }
        match state.kind {
            TextPopoverKind::RenameThread(thread_id) => {
                threads.update(|items| {
                    if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                        thread.title = next_name;
                        thread.updated_at = now_rfc3339();
                    }
                });
                persist_state();
            }
            TextPopoverKind::AddFavoriteFolder => {
                preferences.update(|value| {
                    value.favorite_folders =
                        normalized_favorite_folders(value.favorite_folders.clone());
                    let now = now_rfc3339();
                    let folder = FavoriteFolder {
                        id: new_id(),
                        name: next_name,
                        created_at: now.clone(),
                        updated_at: now,
                    };
                    value.active_favorite_folder_id = Some(folder.id.clone());
                    value.favorite_folders.push(folder);
                });
                persist_ui_state();
            }
            TextPopoverKind::RenameFavoriteFolder(folder_id) => {
                preferences.update(|value| {
                    value.favorite_folders =
                        normalized_favorite_folders(value.favorite_folders.clone());
                    if let Some(folder) = value
                        .favorite_folders
                        .iter_mut()
                        .find(|folder| folder.id == folder_id)
                    {
                        folder.name = next_name;
                        folder.updated_at = now_rfc3339();
                    }
                });
                persist_ui_state();
            }
        }
        text_popover.set(None);
    };

    let submit_confirm_popover = move || {
        let Some(state) = confirm_popover.get_untracked() else {
            return;
        };
        let (x, y) = (state.x, state.y);
        confirm_popover.set(None);
        match state.kind {
            ConfirmPopoverKind::CancelGeneration(task_id) => cancel_generation(task_id),
            ConfirmPopoverKind::CancelAllGenerations => cancel_all_generations(),
            ConfirmPopoverKind::DeleteAsset(asset_id) => perform_delete_asset(asset_id),
            ConfirmPopoverKind::DeleteConfig(config_id) => perform_delete_config(config_id),
            ConfirmPopoverKind::DeleteThread(thread_id) => perform_delete_thread(thread_id),
            ConfirmPopoverKind::DeleteFavoriteFolder(folder_id) => {
                perform_delete_favorite_folder(folder_id)
            }
            ConfirmPopoverKind::DeleteTask(task_id) => perform_delete_task(task_id),
            ConfirmPopoverKind::DeleteThemeBackground => perform_delete_theme_background(),
            ConfirmPopoverKind::DeleteUser(user_id) => {
                admin_user_action("/api/admin/users/delete", user_id)
            }
            ConfirmPopoverKind::ClearLocalData(scope) => {
                let (title, message) = local_clear_final_confirmation(scope);
                confirm_popover.set(Some(ConfirmPopoverState {
                    kind: ConfirmPopoverKind::ClearLocalDataFinal(scope),
                    title: title.into(),
                    message: message.into(),
                    x,
                    y,
                }));
            }
            ConfirmPopoverKind::ClearLocalDataFinal(scope) => perform_clear_local_data(scope),
            ConfirmPopoverKind::ClearCloudData(scope) => {
                let (title, message) = cloud_clear_final_confirmation(&scope);
                confirm_popover.set(Some(ConfirmPopoverState {
                    kind: ConfirmPopoverKind::ClearCloudDataFinal(scope),
                    title: title.into(),
                    message: message.into(),
                    x,
                    y,
                }));
            }
            ConfirmPopoverKind::ClearCloudDataFinal(scope) => perform_clear_cloud_data(scope),
        }
    };

    let assign_favorite_folder = move |task_id: String, folder_id: String| {
        tasks.update(|items| {
            if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                task.favorite = true;
                task.favorite_folder_id = Some(folder_id);
                task.updated_at = now_rfc3339();
            }
        });
        preview_panel_state.update(|state| {
            if let Some(state) = state.as_mut()
                && state.task_id == task_id
            {
                state.favorite = true;
            }
        });
        favorite_folder_picker.set(None);
        persist_state();
    };

    let cancel_favorite_for_task = move |task_id: String| {
        let thread_list = threads.get_untracked();
        let current_id = current_thread_id.get_untracked();
        let target_thread_id = thread_list
            .iter()
            .find(|thread| thread.id == current_id)
            .or_else(|| thread_list.first())
            .map(|thread| thread.id.clone())
            .unwrap_or_default();
        let mut reattached_reference_ids = None;
        tasks.update(|items| {
            if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                if task.detached_from_thread && !target_thread_id.is_empty() {
                    task.thread_id = target_thread_id.clone();
                    task.detached_from_thread = false;
                    reattached_reference_ids = Some(task.reference_asset_ids.clone());
                }
                task.favorite = false;
                task.favorite_folder_id = None;
                task.updated_at = now_rfc3339();
            }
        });
        if let Some(reference_ids) = reattached_reference_ids.as_ref() {
            let reference_ids = reference_ids.iter().collect::<HashSet<_>>();
            let updated_at = now_rfc3339();
            assets.update(|items| {
                for asset in items {
                    if reference_ids.contains(&asset.id) {
                        asset
                            .metadata
                            .insert("thread_id".into(), target_thread_id.clone());
                        asset.updated_at = updated_at.clone();
                    }
                }
            });
            threads.update(|items| {
                if let Some(thread) = items
                    .iter_mut()
                    .find(|thread| thread.id == target_thread_id)
                {
                    if !thread.task_ids.contains(&task_id) {
                        thread.task_ids.push(task_id.clone());
                    }
                    thread.updated_at = now_rfc3339();
                }
            });
        }
        preview_panel_state.update(|state| {
            if let Some(state) = state.as_mut()
                && state.task_id == task_id
            {
                state.favorite = false;
            }
        });
        favorite_folder_picker.set(None);
        persist_state();
        if reattached_reference_ids.is_some() {
            status_text.set("已取消收藏，并将归档记录移回当前会话。".into());
        }
    };

    let toggle_favorite_for_task = move |task_id: String, x: f64, y: f64| {
        let is_favorite = tasks.with_untracked(|items| {
            items
                .iter()
                .find(|task| task.id == task_id)
                .map(|task| task.favorite)
                .unwrap_or(false)
        });
        if is_favorite {
            favorite_folder_picker.set(Some(FavoriteFolderPickerState {
                task_id,
                x,
                y,
                is_favorite: true,
            }));
            return;
        }

        let folders = normalized_favorite_folders(preferences.get_untracked().favorite_folders);
        if folders.len() <= 1 {
            let folder_id = folders
                .first()
                .map(|folder| folder.id.clone())
                .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into());
            assign_favorite_folder(task_id, folder_id);
        } else {
            favorite_folder_picker.set(Some(FavoriteFolderPickerState {
                task_id,
                x,
                y,
                is_favorite: false,
            }));
        }
    };

    let close_preview = move || {
        preview_state.set(None);
        preview_panel_state.set(None);
        preview_fullscreen.set(false);
        preview_zoom.set(1.0);
        preview_offset_x.set(0.0);
        preview_offset_y.set(0.0);
        preview_dragging.set(false);
        context_menu_state.set(None);
        trim_asset_payload_cache(assets);
    };

    let edit_output_asset = move |task_id: String, asset_id: String| {
        enter_continuation_context(task_id, asset_id);
        close_preview();
        show_settings.set(false);
    };

    let show_tip = move |text: &str, x: f64, y: f64, persistent: bool| {
        let token = floating_tip_token.get_untracked().saturating_add(1);
        floating_tip_token.set(token);
        floating_tip_state.set(Some(FloatingTipState {
            text: text.into(),
            x,
            y,
            token,
            persistent,
        }));
    };

    let hide_tip = move || {
        floating_tip_state.set(None);
    };

    let reference_tip_enabled = move || {
        !preview_panel_state
            .get()
            .map(|panel| panel.reference_thumbs.is_empty())
            .unwrap_or(true)
    };

    (
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
    )
}
