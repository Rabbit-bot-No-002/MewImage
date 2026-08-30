use super::super::*;

#[allow(clippy::type_complexity)]
pub(crate) fn build_workspace_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    enqueue_payload_writes: impl Fn(Vec<(String, String)>) + Copy + Send + Sync + 'static,
    enqueue_payload_deletes: impl Fn(Vec<String>) + Copy + Send + Sync + 'static,
    commit_current_thread_draft: impl Fn() + Copy + Send + Sync + 'static,
    build_preview_panel_state: impl Fn(&str, Option<&str>) -> Option<PreviewPanelState>
    + Copy
    + Send
    + Sync
    + 'static,
) -> (
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(FileList) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, String) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, String) + Copy + Send + Sync + 'static,
    impl Fn(String) + Copy + Send + Sync + 'static,
    impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
    impl Fn(TextPopoverKind, &'static str, String, f64, f64) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let tombstones = workspace.tombstones;
    let current_thread_id = workspace.current_thread_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let dragging_reference_id = composer.dragging_reference_id;
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let continuation_asset_id = composer.continuation_asset_id;
    let queue_mode_enabled = composer.queue_mode_enabled;
    let generation_runtimes = composer.generation_runtimes;
    let draft_prompt = composer.draft_prompt;
    let status_text = composer.status_text;
    let text_popover = ui.text_popover;
    let text_popover_value = ui.text_popover_value;
    let confirm_popover = ui.confirm_popover;
    let show_thread_archive_menu = ui.show_thread_archive_menu;
    let preview_state = ui.preview_state;
    let preview_panel_state = ui.preview_panel_state;
    let preview_fullscreen = ui.preview_fullscreen;
    let preview_zoom = ui.preview_zoom;
    let preview_offset_x = ui.preview_offset_x;
    let preview_offset_y = ui.preview_offset_y;
    let preview_dragging = ui.preview_dragging;
    let context_menu_state = ui.context_menu_state;

    let new_thread = move |_| {
        commit_current_thread_draft();
        let thread = default_thread();
        current_thread_id.set(thread.id.clone());
        draft_prompt.set(String::new());
        selected_reference_ids.set(Vec::new());
        reference_menu_asset_id.set(None);
        continuation_asset_id.set(None);
        threads.update(|items| items.push(thread));
        persist_state();
        status_text.set("已新建会话，可以开始新的连续修改。".into());
    };

    let open_text_popover =
        move |kind: TextPopoverKind, title: &str, value: String, x: f64, y: f64| {
            text_popover_value.set(value);
            text_popover.set(Some(TextPopoverState {
                kind,
                title: title.into(),
                x,
                y,
            }));
        };

    let rename_thread = move |thread_id: String, x: f64, y: f64| {
        let current_name = threads
            .get_untracked()
            .iter()
            .find(|thread| thread.id == thread_id)
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| "新的会话".into());
        open_text_popover(
            TextPopoverKind::RenameThread(thread_id),
            "重命名会话",
            current_name,
            x,
            y,
        );
    };

    let perform_delete_thread = move |thread_id: String| {
        // 确认框可能在任务提交前已经打开，因此执行删除时必须再次校验。
        if generation_runtimes
            .with_untracked(|items| items.values().any(|runtime| runtime.thread_id == thread_id))
        {
            status_text.set("该会话仍有生成任务运行，请先停止或等待任务完成。".into());
            return;
        }
        let result = delete_thread_preserving_favorites(
            tasks.get_untracked(),
            assets.get_untracked(),
            &thread_id,
            &now_rfc3339(),
        );
        let mut deleted_entities =
            Vec::with_capacity(1 + result.removed_task_ids.len() + result.removed_asset_ids.len());
        deleted_entities.push((SyncEntityKind::Thread, thread_id.clone()));
        deleted_entities.extend(
            result
                .removed_task_ids
                .iter()
                .cloned()
                .map(|id| (SyncEntityKind::Task, id)),
        );
        deleted_entities.extend(
            result
                .removed_asset_ids
                .iter()
                .cloned()
                .map(|id| (SyncEntityKind::Asset, id)),
        );
        record_sync_tombstones(tombstones, deleted_entities);
        let removed_asset_ids = result.removed_asset_ids.clone();
        let retained_favorite_count = result.retained_favorite_count;
        tasks.set(result.tasks);
        assets.set(result.assets);
        if !removed_asset_ids.is_empty() {
            enqueue_payload_deletes(removed_asset_ids.clone());
        }
        threads.update(|items| {
            items.retain(|thread| thread.id != thread_id);
            if items.is_empty() {
                items.push(default_thread());
            }
        });
        selected_reference_ids.update(|ids| ids.retain(|id| !removed_asset_ids.contains(id)));
        if continuation_asset_id
            .get_untracked()
            .as_ref()
            .map(|id| removed_asset_ids.contains(id))
            .unwrap_or(false)
        {
            continuation_asset_id.set(None);
        }
        if current_thread_id.get_untracked() == thread_id {
            let fallback = threads
                .get_untracked()
                .first()
                .cloned()
                .unwrap_or_else(default_thread);
            current_thread_id.set(fallback.id.clone());
            draft_prompt.set(fallback.draft_prompt);
            selected_reference_ids.set(Vec::new());
            reference_menu_asset_id.set(None);
            continuation_asset_id.set(None);
        }
        persist_state();
        status_text.set(if retained_favorite_count == 0 {
            "会话已删除。".into()
        } else {
            format!("会话已删除，已在全局收藏夹独立保留 {retained_favorite_count} 条收藏。")
        });
    };

    let delete_thread = move |thread_id: String, x: f64, y: f64| {
        if generation_runtimes
            .with_untracked(|items| items.values().any(|runtime| runtime.thread_id == thread_id))
        {
            status_text.set("该会话仍有生成任务运行，请先停止或等待任务完成。".into());
            return;
        }
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteThread(thread_id),
            title: "删除会话".into(),
            message: "删除会话后，未收藏记录会被移除；收藏夹中的提示词、结果图和参考图会独立保留。是否继续？".into(),
            x,
            y,
        }));
    };

    let select_thread = move |thread_id: String| {
        if current_thread_id.get_untracked() == thread_id {
            return;
        }
        commit_current_thread_draft();
        current_thread_id.set(thread_id.clone());
        if let Some(selected_thread) =
            threads.with_untracked(|items| items.iter().find(|item| item.id == thread_id).cloned())
        {
            draft_prompt.set(selected_thread.draft_prompt.clone());
        }
        selected_reference_ids.set(Vec::new());
        reference_menu_asset_id.set(None);
        continuation_asset_id.set(None);
        show_thread_archive_menu.set(false);
    };

    let import_reference_assets = move |files: FileList| {
        let assets_signal = assets;
        let selected_reference_ids = selected_reference_ids;
        let status_text = status_text;
        let persist = persist_state;
        let thread_id = current_thread_id.get_untracked();
        spawn_local(async move {
            match import_file_list(files).await {
                Ok(mut imported) => {
                    let existing_thread_assets = assets_signal.with_untracked(|items| {
                        let mut by_hash = HashMap::new();
                        for asset in items.iter().filter(|asset| {
                            asset.source_task_id.is_none()
                                && !asset.metadata.contains_key("mask_base_asset_id")
                        }) {
                            let belongs_to_current_thread = asset
                                .metadata
                                .get("thread_id")
                                .map(|value| value == &thread_id)
                                .unwrap_or(false);
                            if belongs_to_current_thread {
                                by_hash
                                    .entry(asset.sha256.clone())
                                    .or_insert_with(|| asset.id.clone());
                            }
                        }
                        by_hash
                    });
                    let mut reused_ids = Vec::new();
                    imported.retain(|asset| {
                        if let Some(existing_id) = existing_thread_assets.get(&asset.sha256) {
                            reused_ids.push(existing_id.clone());
                            false
                        } else {
                            true
                        }
                    });
                    if imported.is_empty() && reused_ids.is_empty() {
                        status_text.set("没有可导入的参考图。".into());
                        return;
                    }
                    for asset in &mut imported {
                        asset.metadata.insert("thread_id".into(), thread_id.clone());
                        if let Ok(thumbnail) =
                            thumbnail_data_url_from_asset(asset, THUMBNAIL_MAX_EDGE).await
                        {
                            asset
                                .metadata
                                .insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                        }
                    }
                    let payloads = asset_payload_pairs(&imported);
                    let imported_ids: Vec<String> =
                        imported.iter().map(|asset| asset.id.clone()).collect();
                    assets_signal.update(|items| {
                        items.extend(imported);
                        touch_and_trim_asset_payload_cache(items, &imported_ids, false);
                    });
                    enqueue_payload_writes(payloads);
                    selected_reference_ids.update(|current| {
                        for id in reused_ids.iter().chain(imported_ids.iter()) {
                            if !current.contains(&id) {
                                current.push(id.clone());
                            }
                        }
                    });
                    persist();
                    let message = match (imported_ids.len(), reused_ids.len()) {
                        (0, reused_count) => {
                            format!("检测到 {reused_count} 张重复参考图，已自动加入当前参考列表。")
                        }
                        (imported_count, 0) => format!(
                            "已导入 {imported_count} 张参考图，可点击缩略图打开参考图操作菜单。"
                        ),
                        (imported_count, reused_count) => format!(
                            "已导入 {imported_count} 张参考图，并复用 {reused_count} 张重复参考图。"
                        ),
                    };
                    status_text.set(message);
                }
                Err(error) => status_text.set(format!("导入图片失败：{error}")),
            }
        });
    };

    let open_reference_menu = move |asset_id: String| {
        let assets_signal = assets;
        let preload_asset_id = asset_id.clone();
        reference_menu_asset_id.set(Some(asset_id));
        spawn_local(async move {
            let _ = ensure_asset_payloads_loaded(assets_signal, &[preload_asset_id]).await;
        });
    };

    let reorder_selected_references = move |dragged_id: String, target_id: String| {
        if dragged_id == target_id {
            return;
        }
        selected_reference_ids.update(|ids| {
            let Some(from_index) = ids.iter().position(|id| id == &dragged_id) else {
                return;
            };
            let Some(to_index) = ids.iter().position(|id| id == &target_id) else {
                return;
            };
            let item = ids.remove(from_index);
            ids.insert(to_index, item);
        });
    };

    let perform_delete_asset = move |asset_id: String| {
        // 防止旧确认框在图片成为运行任务依赖后继续执行物理删除。
        if generation_runtimes.with_untracked(|items| {
            items
                .values()
                .any(|runtime| runtime.dependency_asset_ids.contains(&asset_id))
        }) {
            status_text.set("这张图片正被生成任务使用，请先停止或等待任务完成。".into());
            return;
        }
        assets.update(|items| items.retain(|asset| asset.id != asset_id));
        selected_reference_ids.update(|ids| ids.retain(|id| id != &asset_id));
        if dragging_reference_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            dragging_reference_id.set(None);
        }
        if reference_menu_asset_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            reference_menu_asset_id.set(None);
        }
        if continuation_asset_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            continuation_asset_id.set(None);
        }
        let removed_asset_ids = vec![asset_id.clone()];
        record_sync_tombstones(tombstones, [(SyncEntityKind::Asset, asset_id.clone())]);
        enqueue_payload_deletes(removed_asset_ids);
        persist_state();
        status_text.set("参考图已删除。".into());
    };

    let delete_asset = move |asset_id: String, x: f64, y: f64| {
        if generation_runtimes.with_untracked(|items| {
            items
                .values()
                .any(|runtime| runtime.dependency_asset_ids.contains(&asset_id))
        }) {
            status_text.set("这张图片正被生成任务使用，请先停止或等待任务完成。".into());
            return;
        }
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteAsset(asset_id),
            title: "删除参考图".into(),
            message: "删除后将从当前浏览器移除这张参考图，并让所有引用它的结果失效，是否继续？"
                .into(),
            x,
            y,
        }));
    };

    let continue_from_task = move |task_id: String| {
        let task_list = tasks.get_untracked();
        let Some(task) = task_list.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let thread_list = threads.get_untracked();
        let target_thread_id =
            task_target_thread_id(&task, &thread_list, &current_thread_id.get_untracked());
        selected_reference_ids.set(task.reference_asset_ids.clone());
        reference_menu_asset_id.set(None);
        current_thread_id.set(target_thread_id.clone());
        draft_prompt.set(task.prompt.clone());
        continuation_asset_id.set(None);
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
        status_text.set("已复用配置，下一次会继续沿用该提示词和参考图。".into());
    };

    let enter_continuation_context = move |task_id: String, asset_id: String| {
        let task_list = tasks.get_untracked();
        let Some(task) = task_list.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let thread_list = threads.get_untracked();
        let target_thread_id =
            task_target_thread_id(&task, &thread_list, &current_thread_id.get_untracked());
        current_thread_id.set(target_thread_id.clone());
        draft_prompt.set(task.prompt.clone());
        selected_reference_ids.set(task.reference_asset_ids.clone());
        continuation_asset_id.set(Some(asset_id.clone()));
        queue_mode_enabled.set(false);
        let _ = save_generation_queue_mode(false);
        reference_menu_asset_id.set(None);
        threads.update(|items| {
            if let Some(thread) = items
                .iter_mut()
                .find(|thread| thread.id == target_thread_id)
            {
                thread.draft_prompt = task.prompt.clone();
                thread.updated_at = now_rfc3339();
            }
        });
        let assets_signal = assets;
        let mut preload_asset_ids = task.reference_asset_ids.clone();
        preload_asset_ids.push(asset_id);
        spawn_local(async move {
            let _ = ensure_asset_payloads_loaded(assets_signal, &preload_asset_ids).await;
        });
        persist_state();
        status_text.set("已进入连续修改模式。".into());
    };

    let perform_delete_task = move |task_id: String| {
        let deleting_current_preview = preview_state
            .get_untracked()
            .as_ref()
            .map(|preview| preview.task_id == task_id)
            .unwrap_or(false);
        let mut next_tasks = tasks.get_untracked();
        let deleted_reference_ids = next_tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| {
                task.reference_asset_ids
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        next_tasks.retain(|task| task.id != task_id);
        let remaining_reference_ids = next_tasks
            .iter()
            .flat_map(|task| task.reference_asset_ids.iter().cloned())
            .collect::<HashSet<_>>();
        let mut next_assets = assets.get_untracked();
        let mut removed_asset_ids = Vec::new();
        let updated_at = now_rfc3339();
        next_assets.retain_mut(|asset| {
            let generated_by_deleted_task =
                asset.source_task_id.as_deref() == Some(task_id.as_str());
            if generated_by_deleted_task && remaining_reference_ids.contains(&asset.id) {
                asset.source_task_id = None;
                asset
                    .metadata
                    .insert(FAVORITE_ARCHIVE_ASSET_KEY.into(), "true".into());
                asset.updated_at = updated_at.clone();
                return true;
            }
            let unused_archived_reference = deleted_reference_ids.contains(&asset.id)
                && !remaining_reference_ids.contains(&asset.id)
                && asset.metadata.contains_key(FAVORITE_ARCHIVE_ASSET_KEY);
            if generated_by_deleted_task || unused_archived_reference {
                removed_asset_ids.push(asset.id.clone());
                return false;
            }
            true
        });
        let mut deleted_entities = Vec::with_capacity(1 + removed_asset_ids.len());
        deleted_entities.push((SyncEntityKind::Task, task_id.clone()));
        deleted_entities.extend(
            removed_asset_ids
                .iter()
                .cloned()
                .map(|id| (SyncEntityKind::Asset, id)),
        );
        record_sync_tombstones(tombstones, deleted_entities);
        assets.set(next_assets);
        tasks.set(next_tasks);
        threads.update(|items| {
            for thread in items {
                let previous_len = thread.task_ids.len();
                thread.task_ids.retain(|id| id != &task_id);
                if thread.task_ids.len() != previous_len {
                    thread.updated_at = now_rfc3339();
                }
            }
        });
        selected_reference_ids.update(|ids| ids.retain(|id| !removed_asset_ids.contains(id)));
        if let Some(asset_id) = continuation_asset_id.get_untracked() {
            if removed_asset_ids.contains(&asset_id) {
                continuation_asset_id.set(None);
            }
        }
        if !removed_asset_ids.is_empty() {
            enqueue_payload_deletes(removed_asset_ids.clone());
        }
        if deleting_current_preview {
            preview_state.set(None);
            preview_panel_state.set(None);
            preview_fullscreen.set(false);
            preview_zoom.set(1.0);
            preview_offset_x.set(0.0);
            preview_offset_y.set(0.0);
            preview_dragging.set(false);
            context_menu_state.set(None);
            trim_asset_payload_cache(assets);
        }
        persist_state();
        status_text.set("历史记录已删除。".into());
    };

    let delete_task = move |task_id: String, x: f64, y: f64| {
        confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteTask(task_id),
            title: "删除记录".into(),
            message: "删除后会从当前浏览器移除这条生成记录和对应图片，是否继续？".into(),
            x,
            y,
        }));
    };

    let open_preview = move |task_id: String, asset_id: Option<String>| {
        if let Some(preview_asset_id) = asset_id.clone() {
            let assets_signal = assets;
            spawn_local(async move {
                let _ = ensure_asset_payloads_loaded(assets_signal, &[preview_asset_id]).await;
            });
        }
        preview_panel_state.set(build_preview_panel_state(&task_id, asset_id.as_deref()));
        preview_state.set(Some(PreviewState { task_id, asset_id }));
        preview_fullscreen.set(false);
        preview_zoom.set(1.0);
        preview_offset_x.set(0.0);
        preview_offset_y.set(0.0);
        preview_dragging.set(false);
        context_menu_state.set(None);
    };

    (
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
    )
}
