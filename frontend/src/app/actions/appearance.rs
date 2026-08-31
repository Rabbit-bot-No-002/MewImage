use super::super::*;

#[allow(clippy::type_complexity)]
pub(crate) fn build_appearance_actions(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    enqueue_payload_deletes: impl Fn(Vec<String>) + Copy + Send + Sync + 'static,
) -> (
    impl Fn(FileList) + Copy + Send + Sync + 'static,
    impl Fn() + Copy + Send + Sync + 'static,
    impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
) {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let assets = workspace.assets;
    let preferences = workspace.preferences;
    let tombstones = workspace.tombstones;

    let import_theme_background = move |files: FileList| {
        let Some(raw_file) = files.get(0) else {
            return;
        };
        ui.background_processing.set(true);
        ui.appearance_message
            .set(Some("正在优化并转换背景图……".into()));
        spawn_local(async move {
            let processed = match process_theme_background_file(raw_file).await {
                Ok(processed) => processed,
                Err(error) => {
                    ui.background_processing.set(false);
                    ui.appearance_message.set(Some(error));
                    return;
                }
            };
            let asset_id = processed.asset.id.clone();
            if let Err(error) =
                apply_asset_payload_changes(&[(asset_id.clone(), processed.data_url.clone())], &[])
                    .await
            {
                ui.background_processing.set(false);
                ui.appearance_message
                    .set(Some(format!("保存主题背景失败：{error}")));
                return;
            }
            let removed_ids = assets.with_untracked(|items| {
                items
                    .iter()
                    .filter(|asset| is_theme_background(asset))
                    .map(|asset| asset.id.clone())
                    .collect::<Vec<_>>()
            });
            assets.update(|items| {
                items.retain(|asset| !is_theme_background(asset));
                items.push(processed.asset);
            });
            preferences.update(|value| {
                value.appearance.custom_background.asset_id = Some(asset_id.clone());
                value.appearance.custom_background.enabled = true;
                value.appearance.normalize();
            });
            if !removed_ids.is_empty() {
                record_sync_tombstones(
                    tombstones,
                    removed_ids
                        .iter()
                        .cloned()
                        .map(|id| (SyncEntityKind::Asset, id)),
                );
                enqueue_payload_deletes(removed_ids);
            }
            persist_state();
            persist_ui_state();
            ui.background_processing.set(false);
            ui.appearance_message.set(Some(
                "背景已转换为 WebP 并保存在本地，下次手动同步时会上传云端。".into(),
            ));
        });
    };

    let perform_delete_theme_background = move || {
        let removed_ids = assets.with_untracked(|items| {
            items
                .iter()
                .filter(|asset| is_theme_background(asset))
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>()
        });
        preferences.update(|value| {
            value.appearance.custom_background = Default::default();
        });
        if !removed_ids.is_empty() {
            assets.update(|items| items.retain(|asset| !is_theme_background(asset)));
            record_sync_tombstones(
                tombstones,
                removed_ids
                    .iter()
                    .cloned()
                    .map(|id| (SyncEntityKind::Asset, id)),
            );
            enqueue_payload_deletes(removed_ids);
            persist_state();
        }
        persist_ui_state();
        ui.appearance_message.set(Some("自定义背景已删除。".into()));
    };

    let request_delete_theme_background = move |event: MouseEvent| {
        if preferences
            .get_untracked()
            .appearance
            .custom_background
            .asset_id
            .is_none()
        {
            return;
        }
        ui.confirm_popover.set(Some(ConfirmPopoverState {
            kind: ConfirmPopoverKind::DeleteThemeBackground,
            title: "删除自定义背景".into(),
            message: "背景原图及其云端资源会在下次同步时删除，是否继续？".into(),
            x: event.client_x() as f64,
            y: event.client_y() as f64,
        }));
    };

    (
        import_theme_background,
        perform_delete_theme_background,
        request_delete_theme_background,
    )
}
