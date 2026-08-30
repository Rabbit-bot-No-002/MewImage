use std::collections::HashSet;

use leptos::prelude::*;
use mew_image_shared::{
    ConversationThread, DEFAULT_FAVORITE_FOLDER_ID, EncryptedApiConfig, FavoriteFolder,
    ImageAssetRef, LocalTaskRecord,
};

use super::{
    FAVORITE_PAGE_SIZE, GALLERY_PAGE_SIZE,
    state::{ComposerState, UiState, WorkspaceState},
    utils::workspace::{
        GalleryItem, gallery_items, normalized_favorite_folders, paged_items,
        selected_reference_assets, thread_reference_assets, visible_thread_items,
    },
};

#[derive(Clone, Copy)]
pub(crate) struct AppDerived {
    pub(crate) current_config: Memo<Option<EncryptedApiConfig>>,
    pub(crate) visible_threads: Memo<Vec<ConversationThread>>,
    pub(crate) archived_threads: Memo<Vec<ConversationThread>>,
    pub(crate) reference_assets: Memo<Vec<ImageAssetRef>>,
    pub(crate) continuation_asset: Memo<Option<ImageAssetRef>>,
    pub(crate) dimension_reference_assets: Memo<Vec<ImageAssetRef>>,
    pub(crate) current_reference_menu_asset: Memo<Option<ImageAssetRef>>,
    pub(crate) current_preview: Memo<Option<(LocalTaskRecord, Option<ImageAssetRef>)>>,
    pub(crate) gallery_entries: Memo<Vec<GalleryItem>>,
    pub(crate) favorite_folders: Memo<Vec<FavoriteFolder>>,
    pub(crate) active_favorite_folder_id: Memo<String>,
    pub(crate) favorite_gallery_entries: Memo<Vec<GalleryItem>>,
    pub(crate) gallery_page_count: Memo<usize>,
    pub(crate) paged_gallery_entries: Memo<Vec<GalleryItem>>,
    pub(crate) favorite_page_count: Memo<usize>,
    pub(crate) paged_favorite_gallery_entries: Memo<Vec<GalleryItem>>,
}

impl AppDerived {
    pub(crate) fn new(workspace: WorkspaceState, composer: ComposerState, ui: UiState) -> Self {
        let current_config = Memo::new(move |_| {
            workspace
                .configs
                .get()
                .into_iter()
                .find(|config| config.id == workspace.current_config_id.get())
        });
        let visible_threads = Memo::new(move |_| {
            let all_threads = workspace.threads.get();
            let current_id = workspace.current_thread_id.get();
            visible_thread_items(&all_threads, &current_id)
        });
        let archived_threads = Memo::new(move |_| {
            let visible_ids: HashSet<String> = visible_threads
                .get()
                .into_iter()
                .map(|thread| thread.id)
                .collect();
            let mut archived = workspace
                .threads
                .get()
                .into_iter()
                .filter(|thread| !visible_ids.contains(&thread.id))
                .collect::<Vec<_>>();
            archived.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            archived
        });
        let visible_tasks = Memo::new(move |_| {
            let thread_id = workspace.current_thread_id.get();
            let mut visible = workspace.tasks.with(|task_list| {
                task_list
                    .iter()
                    .filter(|task| task.thread_id == thread_id)
                    .cloned()
                    .collect::<Vec<_>>()
            });
            visible.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            visible
        });
        let reference_assets = Memo::new(move |_| {
            let selected_ids = composer.selected_reference_ids.get();
            if !composer.show_all_reference_assets.get() {
                return workspace
                    .assets
                    .with(|assets| selected_reference_assets(assets, &selected_ids));
            }
            let thread_id = workspace.current_thread_id.get();
            workspace.assets.with(|assets| {
                workspace
                    .tasks
                    .with(|tasks| thread_reference_assets(assets, tasks, &thread_id, &selected_ids))
            })
        });
        let continuation_asset = Memo::new(move |_| {
            let asset_id = composer.continuation_asset_id.get()?;
            workspace
                .assets
                .with(|assets| assets.iter().find(|asset| asset.id == asset_id).cloned())
        });
        let dimension_reference_assets = Memo::new(move |_| {
            let selected_ids = composer.selected_reference_ids.get();
            let continuation_id = composer.continuation_asset_id.get();
            workspace.assets.with(|assets| {
                let mut ordered = assets
                    .iter()
                    .filter(|asset| selected_ids.contains(&asset.id))
                    .filter(|asset| !asset.metadata.contains_key("mask_base_asset_id"))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(asset_id) = continuation_id
                    && let Some(asset) = assets
                        .iter()
                        .find(|asset| {
                            asset.id == asset_id
                                && !asset.metadata.contains_key("mask_base_asset_id")
                        })
                        .cloned()
                {
                    ordered.retain(|item| item.id != asset.id);
                    ordered.insert(0, asset);
                }
                ordered
            })
        });
        let current_reference_menu_asset = Memo::new(move |_| {
            let asset_id = composer.reference_menu_asset_id.get()?;
            workspace
                .assets
                .with(|assets| assets.iter().find(|asset| asset.id == asset_id).cloned())
        });
        let current_preview = Memo::new(move |_| {
            let preview = ui.preview_state.get()?;
            let task = workspace.tasks.with(|tasks| {
                tasks
                    .iter()
                    .find(|task| task.id == preview.task_id)
                    .cloned()
            })?;
            let asset = workspace.assets.with(|assets| {
                preview
                    .asset_id
                    .as_ref()
                    .and_then(|asset_id| assets.iter().find(|asset| asset.id == *asset_id))
                    .or_else(|| {
                        assets
                            .iter()
                            .find(|asset| asset.source_task_id.as_deref() == Some(task.id.as_str()))
                    })
                    .cloned()
            });
            Some((task, asset))
        });
        let gallery_entries = Memo::new(move |_| {
            let visible = visible_tasks.get();
            let configs = workspace.configs.get();
            workspace
                .assets
                .with(|assets| gallery_items(&visible, &configs, assets))
        });
        let favorite_folders = Memo::new(move |_| {
            normalized_favorite_folders(workspace.preferences.get().favorite_folders)
        });
        let active_favorite_folder_id = Memo::new(move |_| {
            let folders = favorite_folders.get();
            let preferred = workspace
                .preferences
                .get()
                .active_favorite_folder_id
                .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into());
            if folders.iter().any(|folder| folder.id == preferred) {
                preferred
            } else {
                folders
                    .first()
                    .map(|folder| folder.id.clone())
                    .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into())
            }
        });
        let favorite_gallery_entries = Memo::new(move |_| {
            let folder_id = active_favorite_folder_id.get();
            let mut favorite_tasks = workspace.tasks.with(|tasks| {
                tasks
                    .iter()
                    .filter(|task| task.favorite)
                    .filter(|task| {
                        task.favorite_folder_id
                            .as_deref()
                            .unwrap_or(DEFAULT_FAVORITE_FOLDER_ID)
                            == folder_id
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            });
            favorite_tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            let configs = workspace.configs.get();
            workspace
                .assets
                .with(|assets| gallery_items(&favorite_tasks, &configs, assets))
        });
        let gallery_page_count = Memo::new(move |_| {
            gallery_entries
                .get()
                .len()
                .max(1)
                .div_ceil(GALLERY_PAGE_SIZE)
        });
        let paged_gallery_entries = Memo::new(move |_| {
            let entries = gallery_entries.get();
            paged_items(
                &entries,
                ui.gallery_page.get().min(gallery_page_count.get()),
                GALLERY_PAGE_SIZE,
            )
        });
        let favorite_page_count = Memo::new(move |_| {
            favorite_gallery_entries
                .get()
                .len()
                .max(1)
                .div_ceil(FAVORITE_PAGE_SIZE)
        });
        let paged_favorite_gallery_entries = Memo::new(move |_| {
            let entries = favorite_gallery_entries.get();
            paged_items(
                &entries,
                ui.favorite_page.get().min(favorite_page_count.get()),
                FAVORITE_PAGE_SIZE,
            )
        });

        Self {
            current_config,
            visible_threads,
            archived_threads,
            reference_assets,
            continuation_asset,
            dimension_reference_assets,
            current_reference_menu_asset,
            current_preview,
            gallery_entries,
            favorite_folders,
            active_favorite_folder_id,
            favorite_gallery_entries,
            gallery_page_count,
            paged_gallery_entries,
            favorite_page_count,
            paged_favorite_gallery_entries,
        }
    }
}
