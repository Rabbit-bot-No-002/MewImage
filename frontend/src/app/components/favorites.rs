use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::DEFAULT_FAVORITE_FOLDER_ID;
use web_sys::MouseEvent;

use crate::app::{
    derived::AppDerived,
    ensure_asset_display_sources_loaded, first_displayable_generated_asset,
    state::{UiState, WorkspaceState},
};

use super::common::{MaterialSymbolIcon, PaginationControls};

#[component]
pub(crate) fn FavoritesOverlay(
    select_favorite_folder: impl Fn(String) + Copy + Send + Sync + 'static,
    add_favorite_folder: impl Fn(f64, f64) + Copy + Send + Sync + 'static,
    rename_favorite_folder: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    delete_favorite_folder: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    open_preview: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
    enter_continuation_context: impl Fn(String, String) + Copy + Send + Sync + 'static,
    rerun_task: impl Fn(String) + Copy + Send + Sync + 'static,
    toggle_favorite_for_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    open_failure_log: impl Fn(String) + Copy + Send + Sync + 'static,
    delete_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let tasks = workspace.tasks;
    let assets = workspace.assets;
    let show_favorites_panel = ui.show_favorites_panel;
    let favorite_page = ui.favorite_page;
    let favorite_folders = derived.favorite_folders;
    let active_favorite_folder_id = derived.active_favorite_folder_id;
    let paged_favorite_gallery_entries = derived.paged_favorite_gallery_entries;
    let favorite_gallery_entries = derived.favorite_gallery_entries;
    let favorite_page_count = derived.favorite_page_count;

    Effect::new(move |_| {
        if !show_favorites_panel.get() {
            return;
        }
        let missing_source_ids = paged_favorite_gallery_entries
            .get()
            .into_iter()
            .filter(|item| item.src.as_deref().unwrap_or_default().is_empty())
            .filter_map(|item| item.asset_id)
            .collect::<Vec<_>>();
        if missing_source_ids.is_empty() {
            return;
        }
        spawn_local(async move {
            let _ = ensure_asset_display_sources_loaded(assets, &missing_source_ids).await;
        });
    });

    view! {
            {move || if show_favorites_panel.get() {
                view! {
                    <div class="favorites-overlay" on:click=move |_| show_favorites_panel.set(false)>
                        <div class="favorites-popover" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <div class="favorites-header">
                                <div class="row">
                                    <MaterialSymbolIcon name="star" filled=true />
                                    <h2>"全局收藏"</h2>
                                    <span class="tag">{move || format!("{} 张", favorite_gallery_entries.get().len())}</span>
                                </div>
                                <button class="button ghost icon-button" title="关闭收藏夹" on:click=move |_| show_favorites_panel.set(false)>
                                    <MaterialSymbolIcon name="close" filled=false />
                                </button>
                            </div>
                            <div class="favorite-folder-tabs">
                                {move || favorite_folders
                                    .get()
                                    .into_iter()
                                    .map(|folder| {
                                        let folder_id = folder.id.clone();
                                        let active_folder_id = folder.id.clone();
                                        let rename_folder_id = folder.id.clone();
                                        let delete_folder_id = folder.id.clone();
                                        let can_delete_folder = folder.id != DEFAULT_FAVORITE_FOLDER_ID;
                                        view! {
                                            <div
                                                class="favorite-folder-tab"
                                                class:is-active=move || active_favorite_folder_id.get() == active_folder_id
                                            >
                                                <button class="favorite-folder-tab-main" on:click=move |_| select_favorite_folder(folder_id.clone())>
                                                    <MaterialSymbolIcon name="folder" filled=false />
                                                    <span>{folder.name}</span>
                                                </button>
                                                <button class="favorite-folder-rename" title="重命名收藏文件夹" on:click=move |ev: MouseEvent| rename_favorite_folder(rename_folder_id.clone(), ev.client_x() as f64, ev.client_y() as f64)>
                                                    <MaterialSymbolIcon name="edit_square" filled=false />
                                                </button>
                                                {if can_delete_folder {
                                                    view! {
                                                        <button class="favorite-folder-rename danger" title="删除收藏文件夹" on:click=move |ev: MouseEvent| delete_favorite_folder(delete_folder_id.clone(), ev.client_x() as f64, ev.client_y() as f64)>
                                                            <MaterialSymbolIcon name="delete" filled=false />
                                                        </button>
                                                    }.into_any()
                                                } else {
                                                    ().into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    })
                                    .collect::<Vec<_>>()}
                                <button class="favorite-folder-add" title="新增收藏文件夹" on:click=move |ev: MouseEvent| add_favorite_folder(ev.client_x() as f64, ev.client_y() as f64)>
                                    <MaterialSymbolIcon name="add" filled=false />
                                </button>
                            </div>
                            <div class="favorite-gallery-grid">
                                {move || {
                                    let entries = paged_favorite_gallery_entries.get();
                                    if entries.is_empty() {
                                        return vec![view! {
                                            <div class="favorite-empty">
                                                <MaterialSymbolIcon name="star" filled=false />
                                                <strong>"这个文件夹还没有收藏~"</strong>
                                                <span class="muted">"点击画廊卡片里的星星就能加入这里。"</span>
                                            </div>
                                        }.into_any()];
                                    }
                                    entries
                                        .into_iter()
                                        .map(|item| {
                                            let task_id = item.task_id.clone();
                                            let asset_id = item.asset_id.clone();
                                            let prompt = item.prompt.clone();
                                            let show_failure_log = tasks.with_untracked(|items| {
                                                items
                                                    .iter()
                                                    .find(|task| task.id == task_id)
                                                    .and_then(|task| task.error_message.clone())
                                                    .is_some()
                                            });
                                            let rerun_task_id = task_id.clone();
                                            let continue_task_id = task_id.clone();
                                            let favorite_task_id = task_id.clone();
                                            let delete_task_id = task_id.clone();
                                            let open_task_id = task_id.clone();
                                            let ratio_label = item.ratio_label.clone();
                                            let size_label = item.size_label.clone();
                                            view! {
                                                <article class="card gallery-card-compact favorite-gallery-card">
                                                    {item.src.clone().map(|src| {
                                                        let open_asset_id = asset_id.clone();
                                                        let open_task_id = open_task_id.clone();
                                                        let ratio_label = ratio_label.clone();
                                                        let size_label = size_label.clone();
                                                        view! {
                                                            <button class="image-button compact-preview-button" on:click=move |_| {
                                                                if let Some(asset_id) = open_asset_id.clone() {
                                                                    open_preview(open_task_id.clone(), Some(asset_id));
                                                                }
                                                            }>
                                                                <div class="gallery-image-overlay">
                                                                    <span class="gallery-corner-badge">{ratio_label}</span>
                                                                    <span class="gallery-corner-badge">{size_label}</span>
                                                                </div>
                                                                <img class="compact-preview-image" src=src alt=prompt.clone() />
                                                            </button>
                                                        }.into_any()
                                                    }).unwrap_or_else(|| view! {
                                                        <div class="compact-preview-fallback favorite-empty-thumb">"无预览"</div>
                                                    }.into_any())}
                                                    <div class="card-body stack compact-card-body">
                                                        <p class="gallery-card-title">{item.prompt.clone()}</p>
                                                        <div class="gallery-meta">
                                                            <span class="gallery-badge" title=format!("{} · {}", item.config_name, item.model)>
                                                                {format!("{} · {}", item.config_name, item.model)}
                                                            </span>
                                                        </div>
                                                        <div class="row compact-actions">
                                                            <button class="button ghost mini-action icon-action" title="重新生成" on:click=move |_| rerun_task(rerun_task_id.clone())>
                                                                <MaterialSymbolIcon name="restart_alt" filled=false />
                                                            </button>
                                                            <button class="button ghost mini-action icon-action" title="继续修改" on:click=move |_| {
                                                                if let Some(first_asset) = assets.with_untracked(|items| first_displayable_generated_asset(items, &continue_task_id)) {
                                                                    enter_continuation_context(continue_task_id.clone(), first_asset.id);
                                                                    show_favorites_panel.set(false);
                                                                }
                                                            }>
                                                                <MaterialSymbolIcon name="edit_square" filled=false />
                                                            </button>
                                                            <button
                                                                class="button ghost mini-action icon-action"
                                                                title="移动或取消收藏"
                                                                on:click=move |ev: MouseEvent| {
                                                                    toggle_favorite_for_task(
                                                                        favorite_task_id.clone(),
                                                                        ev.client_x() as f64,
                                                                        ev.client_y() as f64,
                                                                    );
                                                                }
                                                            >
                                                                <MaterialSymbolIcon name="star" filled=true />
                                                            </button>
                                                            {if show_failure_log {
                                                                view! {
                                                                    <button class="button ghost mini-action icon-action" title="查看失败日志" on:click=move |_| open_failure_log(task_id.clone())>
                                                                        <MaterialSymbolIcon name="receipt_long" filled=false />
                                                                    </button>
                                                                }.into_any()
                                                            } else {
                                                                ().into_any()
                                                            }}
                                                            <button class="button ghost danger mini-action icon-action" title="删除记录" on:click=move |ev: MouseEvent| delete_task(delete_task_id.clone(), ev.client_x() as f64, ev.client_y() as f64)>
                                                                <MaterialSymbolIcon name="delete" filled=false />
                                                            </button>
                                                        </div>
                                                    </div>
                                                </article>
                                            }.into_any()
                                        })
                                        .collect::<Vec<_>>()
                                }}
                            </div>
                            <PaginationControls
                                page=favorite_page
                                page_count=favorite_page_count
                                favorite=true
                            />
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

    }
}
