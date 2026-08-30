use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use mew_image_shared::{
    ConversationThread, DEFAULT_FAVORITE_FOLDER_ID, EncryptedApiConfig, FavoriteFolder,
    ImageAssetRef, LocalTaskRecord, SyncEntityKind, SyncTombstone, TaskStatus, now_rfc3339,
};

use crate::app::{
    FAVORITE_ARCHIVE_ASSET_KEY, VISIBLE_THREAD_LIMIT, aspect_ratio_label, asset_display_src,
};

#[derive(Clone, PartialEq)]
pub(crate) struct GalleryItem {
    pub(crate) key: String,
    pub(crate) task_id: String,
    pub(crate) asset_id: Option<String>,
    pub(crate) prompt: String,
    pub(crate) src: Option<String>,
    pub(crate) config_name: String,
    pub(crate) model: String,
    pub(crate) size_label: String,
    pub(crate) ratio_label: String,
    pub(crate) favorite: bool,
    pub(crate) status: TaskStatus,
    pub(crate) error_message: Option<String>,
}

pub(crate) fn gallery_items(
    tasks: &[LocalTaskRecord],
    configs: &[EncryptedApiConfig],
    assets: &[ImageAssetRef],
) -> Vec<GalleryItem> {
    let mut assets_by_task: HashMap<&str, Vec<&ImageAssetRef>> = HashMap::new();
    let config_names: HashMap<&str, &str> = configs
        .iter()
        .map(|config| (config.id.as_str(), config.name.as_str()))
        .collect();
    for asset in assets {
        if let Some(task_id) = asset.source_task_id.as_deref() {
            assets_by_task.entry(task_id).or_default().push(asset);
        }
    }
    let mut items = Vec::new();
    for task in tasks {
        if let Some(generated_assets) = assets_by_task.get(task.id.as_str()) {
            for asset in generated_assets {
                items.push(GalleryItem {
                    key: format!("{}-{}", task.id, asset.id),
                    task_id: task.id.clone(),
                    asset_id: Some(asset.id.clone()),
                    prompt: task.prompt.clone(),
                    src: Some(asset_display_src(asset)),
                    config_name: config_names
                        .get(task.config_id.as_str())
                        .copied()
                        .unwrap_or("默认配置")
                        .to_string(),
                    model: task.requested_model.clone(),
                    size_label: format!(
                        "{}x{}",
                        asset.width.unwrap_or(0),
                        asset.height.unwrap_or(0)
                    ),
                    ratio_label: aspect_ratio_label(
                        asset.width.unwrap_or(0),
                        asset.height.unwrap_or(0),
                    ),
                    favorite: task.favorite,
                    status: task.status,
                    error_message: (task.status == TaskStatus::Failed)
                        .then(|| task.error_message.clone())
                        .flatten(),
                });
            }
        } else if task.status == TaskStatus::Running {
            items.push(GalleryItem {
                key: format!("{}-running", task.id),
                task_id: task.id.clone(),
                asset_id: None,
                prompt: task.prompt.clone(),
                src: None,
                config_name: config_names
                    .get(task.config_id.as_str())
                    .copied()
                    .unwrap_or("默认配置")
                    .to_string(),
                model: task.requested_model.clone(),
                size_label: "等待".into(),
                ratio_label: "生成中".into(),
                favorite: false,
                status: TaskStatus::Running,
                error_message: None,
            });
        } else if let Some(error) = &task.error_message {
            items.push(GalleryItem {
                key: format!("{}-error", task.id),
                task_id: task.id.clone(),
                asset_id: None,
                prompt: task.prompt.clone(),
                src: None,
                config_name: config_names
                    .get(task.config_id.as_str())
                    .copied()
                    .unwrap_or("默认配置")
                    .to_string(),
                model: task.requested_model.clone(),
                size_label: "-".into(),
                ratio_label: "失败".into(),
                favorite: task.favorite,
                status: task.status,
                error_message: Some(error.clone()),
            });
        }
    }
    items
}

pub(crate) fn first_displayable_generated_asset(
    assets: &[ImageAssetRef],
    task_id: &str,
) -> Option<ImageAssetRef> {
    assets
        .iter()
        .find(|asset| asset.source_task_id.as_deref() == Some(task_id))
        .cloned()
}

pub(crate) fn paged_items<T: Clone>(items: &[T], page: usize, page_size: usize) -> Vec<T> {
    let start = page.max(1).saturating_sub(1).saturating_mul(page_size);
    if start >= items.len() {
        return Vec::new();
    }
    let end = start.saturating_add(page_size).min(items.len());
    items[start..end].to_vec()
}

pub(crate) fn normalized_favorite_folders(mut folders: Vec<FavoriteFolder>) -> Vec<FavoriteFolder> {
    if folders.is_empty() {
        let now = now_rfc3339();
        folders.push(FavoriteFolder {
            id: DEFAULT_FAVORITE_FOLDER_ID.into(),
            name: "默认".into(),
            created_at: now.clone(),
            updated_at: now,
        });
        return folders;
    }
    if !folders
        .iter()
        .any(|folder| folder.id == DEFAULT_FAVORITE_FOLDER_ID)
    {
        let now = now_rfc3339();
        folders.insert(
            0,
            FavoriteFolder {
                id: DEFAULT_FAVORITE_FOLDER_ID.into(),
                name: "默认".into(),
                created_at: now.clone(),
                updated_at: now,
            },
        );
    }
    folders
}

pub(crate) fn record_sync_tombstones(
    tombstones: RwSignal<Vec<SyncTombstone>>,
    entities: impl IntoIterator<Item = (SyncEntityKind, String)>,
) {
    tombstones.update(|items| record_sync_tombstones_in(items, entities));
}

pub(crate) fn record_sync_tombstones_in(
    items: &mut Vec<SyncTombstone>,
    entities: impl IntoIterator<Item = (SyncEntityKind, String)>,
) {
    let deleted_at = now_rfc3339();
    for (entity_kind, entity_id) in entities {
        if let Some(existing) = items
            .iter_mut()
            .find(|item| item.entity_kind == entity_kind && item.entity_id == entity_id)
        {
            if existing.deleted_at < deleted_at {
                existing.deleted_at = deleted_at.clone();
            }
        } else {
            items.push(SyncTombstone {
                entity_kind,
                entity_id,
                deleted_at: deleted_at.clone(),
            });
        }
    }
}

pub(crate) fn visible_thread_items(
    threads: &[ConversationThread],
    current_thread_id: &str,
) -> Vec<ConversationThread> {
    if threads.len() <= VISIBLE_THREAD_LIMIT {
        let mut items = threads.to_vec();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        return items;
    }

    let mut by_newest = threads.to_vec();
    by_newest.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut visible = by_newest
        .iter()
        .take(VISIBLE_THREAD_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    if visible
        .iter()
        .any(|thread| thread.id.as_str() == current_thread_id)
    {
        return visible;
    }

    if let Some(current_thread) = by_newest
        .iter()
        .find(|thread| thread.id.as_str() == current_thread_id)
        .cloned()
    {
        if let Some(last) = visible.last_mut() {
            *last = current_thread;
        }
    }
    visible
}

pub(crate) fn reconcile_task_integrity(
    tasks: &mut [LocalTaskRecord],
    assets: &[ImageAssetRef],
    repair_running_tasks: bool,
) -> bool {
    let mut asset_count_by_task: HashMap<&str, usize> = HashMap::new();
    for asset in assets {
        if let Some(task_id) = asset.source_task_id.as_deref() {
            *asset_count_by_task.entry(task_id).or_default() += 1;
        }
    }

    let mut changed = false;
    for task in tasks {
        let produced_count = asset_count_by_task
            .get(task.id.as_str())
            .copied()
            .unwrap_or(0);
        match task.status {
            TaskStatus::Succeeded if produced_count == 0 => {
                task.status = TaskStatus::Failed;
                if task
                    .error_message
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    let upstream_count = task
                        .result
                        .as_ref()
                        .map(|result| {
                            result
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
                                .count()
                        })
                        .unwrap_or(0);
                    task.error_message = Some(if upstream_count == 0 {
                        "上游没有返回任何可用图片结果，任务已改判为失败。".into()
                    } else {
                        "上游结果未能落成本地可用图片，可能是网络、尺寸或响应异常导致。".into()
                    });
                }
                changed = true;
            }
            TaskStatus::Running if repair_running_tasks && produced_count == 0 => {
                task.status = TaskStatus::Failed;
                if task
                    .error_message
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    task.error_message = Some("上次生成未正常结束，已自动标记为失败。".into());
                }
                changed = true;
            }
            _ => {}
        }
    }

    changed
}

pub(crate) struct ThreadDeletionResult {
    pub(crate) tasks: Vec<LocalTaskRecord>,
    pub(crate) assets: Vec<ImageAssetRef>,
    pub(crate) removed_task_ids: Vec<String>,
    pub(crate) removed_asset_ids: Vec<String>,
    pub(crate) retained_favorite_count: usize,
}

pub(crate) fn delete_thread_preserving_favorites(
    mut tasks: Vec<LocalTaskRecord>,
    mut assets: Vec<ImageAssetRef>,
    thread_id: &str,
    updated_at: &str,
) -> ThreadDeletionResult {
    let thread_tasks = tasks
        .iter()
        .filter(|task| task.thread_id == thread_id && !task.detached_from_thread)
        .collect::<Vec<_>>();
    let favorite_task_ids = thread_tasks
        .iter()
        .filter(|task| task.favorite)
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let removed_task_ids = thread_tasks
        .iter()
        .filter(|task| !task.favorite)
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let removed_task_id_set = removed_task_ids.iter().cloned().collect::<HashSet<_>>();
    let protected_reference_ids = tasks
        .iter()
        .filter(|task| task.favorite)
        .flat_map(|task| task.reference_asset_ids.iter().cloned())
        .collect::<HashSet<_>>();
    let removed_reference_ids = thread_tasks
        .iter()
        .filter(|task| !task.favorite)
        .flat_map(|task| task.reference_asset_ids.iter().cloned())
        .collect::<HashSet<_>>();

    for task in &mut tasks {
        if favorite_task_ids.contains(&task.id) {
            task.detached_from_thread = true;
            task.updated_at = updated_at.to_string();
        }
    }
    tasks.retain(|task| !removed_task_id_set.contains(&task.id));

    let mut removed_asset_ids = Vec::new();
    assets.retain_mut(|asset| {
        let belongs_to_thread = asset
            .metadata
            .get("thread_id")
            .map(|value| value == thread_id)
            .unwrap_or(false);
        let source_task_id = asset.source_task_id.as_deref();
        let source_is_favorite = source_task_id
            .map(|id| favorite_task_ids.contains(id))
            .unwrap_or(false);
        let source_is_removed = source_task_id
            .map(|id| removed_task_id_set.contains(id))
            .unwrap_or(false);
        let protected = source_is_favorite || protected_reference_ids.contains(&asset.id);
        let archived_removed_reference = asset.metadata.contains_key(FAVORITE_ARCHIVE_ASSET_KEY)
            && removed_reference_ids.contains(&asset.id);
        let should_remove =
            (belongs_to_thread || source_is_removed || archived_removed_reference) && !protected;
        if should_remove {
            removed_asset_ids.push(asset.id.clone());
            return false;
        }

        if protected && belongs_to_thread {
            asset.metadata.remove("thread_id");
            asset
                .metadata
                .insert(FAVORITE_ARCHIVE_ASSET_KEY.into(), "true".into());
            asset.updated_at = updated_at.to_string();
        }
        if protected && source_is_removed {
            asset.source_task_id = None;
            asset
                .metadata
                .insert(FAVORITE_ARCHIVE_ASSET_KEY.into(), "true".into());
            asset.updated_at = updated_at.to_string();
        }
        true
    });

    ThreadDeletionResult {
        tasks,
        assets,
        removed_task_ids,
        removed_asset_ids,
        retained_favorite_count: favorite_task_ids.len(),
    }
}

pub(crate) fn task_target_thread_id(
    task: &LocalTaskRecord,
    threads: &[ConversationThread],
    current_thread_id: &str,
) -> String {
    if !task.detached_from_thread && threads.iter().any(|thread| thread.id == task.thread_id) {
        return task.thread_id.clone();
    }
    if threads.iter().any(|thread| thread.id == current_thread_id) {
        return current_thread_id.to_string();
    }
    threads
        .first()
        .map(|thread| thread.id.clone())
        .unwrap_or_default()
}

pub(crate) fn selected_reference_assets(
    assets: &[ImageAssetRef],
    selected_reference_ids: &[String],
) -> Vec<ImageAssetRef> {
    let mut selected_assets = Vec::new();
    for selected_id in selected_reference_ids {
        if let Some(asset) = assets.iter().find(|asset| {
            asset.id == *selected_id && !asset.metadata.contains_key("mask_base_asset_id")
        }) {
            selected_assets.push(asset.clone());
        }
    }
    selected_assets
}

pub(crate) fn thread_reference_assets(
    assets: &[ImageAssetRef],
    tasks: &[LocalTaskRecord],
    thread_id: &str,
    selected_reference_ids: &[String],
) -> Vec<ImageAssetRef> {
    let mut references = selected_reference_assets(assets, selected_reference_ids);
    let mut included_ids = references
        .iter()
        .map(|asset| asset.id.clone())
        .collect::<HashSet<_>>();
    let assets_by_id = assets
        .iter()
        .filter(|asset| !asset.metadata.contains_key("mask_base_asset_id"))
        .map(|asset| (asset.id.as_str(), asset))
        .collect::<HashMap<_, _>>();

    // 历史任务可能引用其他会话生成的图片，因此按实际引用关系收集，而不是按图片来源过滤。
    for reference_id in tasks
        .iter()
        .filter(|task| task.thread_id == thread_id && !task.detached_from_thread)
        .flat_map(|task| task.reference_asset_ids.iter())
    {
        let Some(asset) = assets_by_id.get(reference_id.as_str()).copied() else {
            continue;
        };
        if !included_ids.insert(asset.id.clone()) {
            continue;
        }
        references.push(asset.clone());
    }

    // 仅补充本会话独立上传的资源，避免把从未用作参考的生成结果混入列表。
    for asset in assets.iter().filter(|asset| {
        asset.source_task_id.is_none()
            && asset.metadata.get("thread_id").map(String::as_str) == Some(thread_id)
            && !asset.metadata.contains_key("mask_base_asset_id")
    }) {
        if included_ids.insert(asset.id.clone()) {
            references.push(asset.clone());
        }
    }

    references
}

pub(crate) fn prioritized_asset_indexes_for_thread(
    assets: &[ImageAssetRef],
    tasks: &[LocalTaskRecord],
    thread_id: &str,
) -> Vec<usize> {
    let task_ids: HashSet<&str> = tasks
        .iter()
        .filter(|task| task.thread_id == thread_id)
        .map(|task| task.id.as_str())
        .collect();
    let mut prioritized = Vec::with_capacity(assets.len());
    let mut deferred = Vec::with_capacity(assets.len());
    for (index, asset) in assets.iter().enumerate() {
        let is_current_thread_asset = asset
            .metadata
            .get("thread_id")
            .map(|value| value == thread_id)
            .unwrap_or(false)
            || asset
                .source_task_id
                .as_deref()
                .map(|task_id| task_ids.contains(task_id))
                .unwrap_or(false);
        if is_current_thread_asset {
            prioritized.push(index);
        } else {
            deferred.push(index);
        }
    }
    prioritized.extend(deferred);
    prioritized
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mew_image_shared::{
        DEFAULT_FAVORITE_FOLDER_ID, ImageAssetRef, LocalTaskRecord, TaskStatus,
    };

    use super::*;
    use crate::app::{
        FAVORITE_ARCHIVE_ASSET_KEY, FAVORITE_PAGE_SIZE, LOCAL_BACKGROUND_RESULT_INDEX_KEY,
        LOCAL_BACKGROUND_ROLE_KEY, LOCAL_BACKGROUND_ROLE_RESULT,
    };

    fn test_task(
        id: &str,
        thread_id: &str,
        favorite: bool,
        reference_asset_ids: &[&str],
    ) -> LocalTaskRecord {
        LocalTaskRecord {
            id: id.into(),
            thread_id: thread_id.into(),
            config_id: "config-1".into(),
            prompt: format!("prompt-{id}"),
            requested_model: "gpt-image-2".into(),
            reference_asset_ids: reference_asset_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
            generation_settings: None,
            result: None,
            favorite,
            favorite_folder_id: favorite.then(|| DEFAULT_FAVORITE_FOLDER_ID.into()),
            detached_from_thread: false,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    fn test_asset(
        id: &str,
        source_task_id: Option<&str>,
        thread_id: Option<&str>,
    ) -> ImageAssetRef {
        let mut metadata = BTreeMap::new();
        if let Some(thread_id) = thread_id {
            metadata.insert("thread_id".into(), thread_id.into());
        }
        ImageAssetRef {
            id: id.into(),
            sha256: format!("sha-{id}"),
            mime_type: "image/png".into(),
            byte_len: 4,
            width: Some(1),
            height: Some(1),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: source_task_id.map(str::to_string),
            metadata,
        }
    }

    #[test]
    fn deleting_thread_preserves_favorite_task_and_its_assets() {
        let tasks = vec![
            test_task("favorite", "thread-1", true, &["reference"]),
            test_task("ordinary", "thread-1", false, &[]),
        ];
        let assets = vec![
            test_asset("reference", None, Some("thread-1")),
            test_asset("favorite-output", Some("favorite"), None),
            test_asset("ordinary-output", Some("ordinary"), None),
        ];
        let result = delete_thread_preserving_favorites(
            tasks,
            assets,
            "thread-1",
            "2026-01-02T00:00:00+00:00",
        );

        assert_eq!(result.retained_favorite_count, 1);
        assert_eq!(result.removed_task_ids, ["ordinary"]);
        assert_eq!(result.removed_asset_ids, ["ordinary-output"]);
        assert_eq!(result.tasks.len(), 1);
        assert!(result.tasks[0].detached_from_thread);
        assert!(
            result
                .assets
                .iter()
                .any(|asset| asset.id == "favorite-output")
        );
        let reference = result
            .assets
            .iter()
            .find(|asset| asset.id == "reference")
            .unwrap();
        assert!(!reference.metadata.contains_key("thread_id"));
        assert_eq!(
            reference.metadata.get(FAVORITE_ARCHIVE_ASSET_KEY),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn deleting_source_task_preserves_result_referenced_by_favorite() {
        let tasks = vec![
            test_task("favorite", "thread-1", true, &["source-output"]),
            test_task("source", "thread-1", false, &[]),
        ];
        let assets = vec![
            test_asset("favorite-output", Some("favorite"), None),
            test_asset("source-output", Some("source"), None),
        ];
        let result = delete_thread_preserving_favorites(
            tasks,
            assets,
            "thread-1",
            "2026-01-02T00:00:00+00:00",
        );
        let source_output = result
            .assets
            .iter()
            .find(|asset| asset.id == "source-output")
            .unwrap();

        assert_eq!(source_output.source_task_id, None);
        assert_eq!(
            source_output.metadata.get(FAVORITE_ARCHIVE_ASSET_KEY),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn selected_archived_references_keep_explicit_order() {
        let mut archived = test_asset("archived", Some("deleted-source"), None);
        archived
            .metadata
            .insert(FAVORITE_ARCHIVE_ASSET_KEY.into(), "true".into());
        let current = test_asset("current", None, Some("thread-2"));
        let selected =
            selected_reference_assets(&[current, archived], &["archived".into(), "current".into()]);

        assert_eq!(
            selected
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            ["archived", "current"]
        );
    }

    #[test]
    fn thread_references_include_selected_history_and_thread_uploads_without_leaking() {
        let tasks = vec![
            test_task(
                "current",
                "thread-1",
                false,
                &["cross-thread", "historical"],
            ),
            test_task("other", "thread-2", false, &["other-reference"]),
        ];
        let assets = vec![
            test_asset("selected", None, Some("thread-1")),
            test_asset("historical", None, None),
            test_asset("cross-thread", Some("external-task"), None),
            test_asset("unused-upload", None, Some("thread-1")),
            test_asset("unused-result", Some("current"), None),
            test_asset("other-reference", None, Some("thread-2")),
            test_asset("other-upload", None, Some("thread-2")),
        ];

        let references = thread_reference_assets(&assets, &tasks, "thread-1", &["selected".into()]);

        assert_eq!(
            references
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            ["selected", "cross-thread", "historical", "unused-upload"]
        );
    }

    #[test]
    fn thread_references_keep_selected_order_and_exclude_masks() {
        let mut mask = test_asset("mask", None, Some("thread-1"));
        mask.metadata
            .insert("mask_base_asset_id".into(), "base".into());
        let assets = vec![
            test_asset("first", None, Some("thread-1")),
            test_asset("second", None, Some("thread-1")),
            mask,
        ];

        let references = thread_reference_assets(
            &assets,
            &[],
            "thread-1",
            &["second".into(), "first".into(), "mask".into()],
        );

        assert_eq!(
            references
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
    }

    #[test]
    fn favorite_pagination_uses_nine_items_per_page() {
        let items = (1..=20).collect::<Vec<_>>();
        assert_eq!(
            paged_items(&items, 1, FAVORITE_PAGE_SIZE),
            (1..=9).collect::<Vec<_>>()
        );
        assert_eq!(
            paged_items(&items, 2, FAVORITE_PAGE_SIZE),
            (10..=18).collect::<Vec<_>>()
        );
        assert_eq!(paged_items(&items, 3, FAVORITE_PAGE_SIZE), vec![19, 20]);
    }

    #[test]
    fn local_background_result_is_the_only_generated_asset() {
        let task = test_task("task", "thread", false, &[]);
        let mut result = test_asset("result", Some("task"), None);
        result.metadata.insert(
            LOCAL_BACKGROUND_ROLE_KEY.into(),
            LOCAL_BACKGROUND_ROLE_RESULT.into(),
        );
        result
            .metadata
            .insert(LOCAL_BACKGROUND_RESULT_INDEX_KEY.into(), "0".into());

        let assets = vec![result];
        let gallery = gallery_items(std::slice::from_ref(&task), &[], &assets);
        assert_eq!(gallery.len(), 1);
        assert_eq!(gallery[0].asset_id.as_deref(), Some("result"));
        assert_eq!(
            first_displayable_generated_asset(&assets, "task")
                .unwrap()
                .id,
            "result"
        );

        let mut tasks = vec![task];
        assert!(!reconcile_task_integrity(&mut tasks, &assets, false));
        assert_eq!(tasks[0].status, TaskStatus::Succeeded);
    }

    #[test]
    fn running_task_without_assets_creates_one_waiting_card() {
        let mut task = test_task("running", "thread", false, &[]);
        task.status = TaskStatus::Running;

        let gallery = gallery_items(&[task], &[], &[]);

        assert_eq!(gallery.len(), 1);
        assert_eq!(gallery[0].task_id, "running");
        assert_eq!(gallery[0].asset_id, None);
        assert_eq!(gallery[0].status, TaskStatus::Running);
    }

    #[test]
    fn succeeded_task_replaces_waiting_card_with_all_results() {
        let task = test_task("finished", "thread", false, &[]);
        let assets = vec![
            test_asset("result-1", Some("finished"), None),
            test_asset("result-2", Some("finished"), None),
        ];

        let gallery = gallery_items(&[task], &[], &assets);

        assert_eq!(gallery.len(), 2);
        assert!(
            gallery
                .iter()
                .all(|item| item.status == TaskStatus::Succeeded)
        );
        assert!(gallery.iter().all(|item| item.asset_id.is_some()));
    }

    #[test]
    fn failed_task_without_assets_creates_failure_card() {
        let mut task = test_task("failed", "thread", false, &[]);
        task.status = TaskStatus::Failed;
        task.error_message = Some("上游容量已满".into());

        let gallery = gallery_items(&[task], &[], &[]);

        assert_eq!(gallery.len(), 1);
        assert_eq!(gallery[0].status, TaskStatus::Failed);
        assert_eq!(gallery[0].prompt, "prompt-failed");
        assert_eq!(gallery[0].error_message.as_deref(), Some("上游容量已满"));
    }
}
