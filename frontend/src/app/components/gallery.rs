use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::TaskStatus;
use web_sys::MouseEvent;

use crate::app::{
    derived::AppDerived,
    ensure_asset_display_sources_loaded, first_displayable_generated_asset,
    models::{ConfirmPopoverKind, ConfirmPopoverState, ContextMenuState},
    state::{ComposerState, UiState, WorkspaceState},
};

use super::common::{MaterialSymbolIcon, PaginationControls};

#[component]
pub(crate) fn GallerySidebar(
    open_preview: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
    enter_continuation_context: impl Fn(String, String) + Copy + Send + Sync + 'static,
    rerun_task: impl Fn(String) + Copy + Send + Sync + 'static,
    toggle_favorite_for_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    open_failure_log: impl Fn(String) + Copy + Send + Sync + 'static,
    delete_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let tasks = workspace.tasks;
    let assets = workspace.assets;
    let gallery_page = ui.gallery_page;
    let context_menu_state = ui.context_menu_state;
    let confirm_popover = ui.confirm_popover;
    let generation_runtimes = composer.generation_runtimes;
    let gallery_entries = derived.gallery_entries;
    let paged_gallery_entries = derived.paged_gallery_entries;
    let gallery_page_count = derived.gallery_page_count;

    Effect::new(move |_| {
        let missing_source_ids = paged_gallery_entries
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
                <aside class="panel gallery-sidebar">
                    <div class="row">
                        <h2>"结果画廊"</h2>
                        <div class="row gallery-title-actions">
                            <span class="tag gallery-count-tag">{move || {
                                let entries = gallery_entries.get();
                                let completed = entries
                                    .iter()
                                    .filter(|item| item.status == TaskStatus::Succeeded)
                                    .count();
                                let running = entries
                                    .iter()
                                    .filter(|item| item.status == TaskStatus::Running)
                                    .count();
                                let failed = entries
                                    .iter()
                                    .filter(|item| item.status == TaskStatus::Failed)
                                    .count();
                                let mut summary = vec![format!("{completed} 张")];
                                if running > 0 {
                                    summary.push(format!("{running} 个进行中"));
                                }
                                if failed > 0 {
                                    summary.push(format!("{failed} 个失败"));
                                }
                                summary.join(" · ")
                            }}</span>
                        </div>
                    </div>
                    <div class="gallery sidebar-gallery">
                        {move || {
                            paged_gallery_entries
                                .get()
                                .into_iter()
                                .map(|item| {
                                    let asset_id = item.asset_id.clone();
                                    let task_id = item.task_id.clone();
                                    let show_failure_log = tasks.with_untracked(|items| {
                                        items
                                            .iter()
                                            .find(|task| task.id == task_id)
                                            .and_then(|task| task.error_message.clone())
                                            .is_some()
                                    });
                                    let rerun_task_id = task_id.clone();
                                    let continue_task_id = task_id.clone();
                                    let delete_task_id = task_id.clone();
                                    let favorite_task_id = task_id.clone();
                                    let favorite_title_task_id = task_id.clone();
                                    let favorite_icon_task_id = task_id.clone();
                                    let favorite_fill_task_id = task_id.clone();
                                    let preview_task_id = task_id.clone();
                                    let running_preview_task_id = task_id.clone();
                                    let preview_asset_id = asset_id.clone();
                                    let context_task_id = task_id.clone();
                                    let context_asset_id = asset_id.clone();
                                    let cancel_task_id = task_id.clone();
                                    let item_status = item.status;
                                    let error_message = item.error_message.clone();
                                    let progress_label = generation_runtimes.with(|items| {
                                        items
                                            .get(&task_id)
                                            .map(|runtime| runtime.progress_label.clone())
                                            .unwrap_or_else(|| "等待结果".into())
                                    });
                                    view! {
                                        <article
                                            class="card gallery-card-compact"
                                            class:is-failed=item_status == TaskStatus::Failed
                                        >
                                            {if item_status == TaskStatus::Running {
                                                view! {
                                                    <button
                                                        class="image-button compact-preview-button gallery-running-preview"
                                                        title="查看任务详情"
                                                        on:click=move |_| open_preview(running_preview_task_id.clone(), None)
                                                    >
                                                        <span class="gallery-running-spinner"></span>
                                                        <strong>{progress_label}</strong>
                                                    </button>
                                                }.into_any()
                                            } else {
                                                item.src.clone().map(|src| {
                                                let preview_src = src.clone();
                                                let ratio_label = item.ratio_label.clone();
                                                let size_label = item.size_label.clone();
                                                view! {
                                                    <button
                                                        class="image-button compact-preview-button"
                                                        on:click=move |_| {
                                                            if let Some(asset_id) = preview_asset_id.clone() {
                                                                open_preview(preview_task_id.clone(), Some(asset_id));
                                                            }
                                                        }
                                                        on:contextmenu=move |ev: MouseEvent| {
                                                            ev.prevent_default();
                                                            if let Some(asset_id) = context_asset_id.clone() {
                                                                let assets_signal = assets;
                                                                let preload_asset_id = asset_id.clone();
                                                                spawn_local(async move {
                                                                    let _ = ensure_asset_display_sources_loaded(
                                                                        assets_signal,
                                                                        &[preload_asset_id],
                                                                    )
                                                                    .await;
                                                                });
                                                                context_menu_state.set(Some(ContextMenuState {
                                                                    task_id: context_task_id.clone(),
                                                                    asset_id,
                                                                    x: ev.client_x() as f64,
                                                                    y: ev.client_y() as f64,
                                                                }));
                                                            }
                                                        }
                                                    >
                                                        <div class="gallery-image-overlay">
                                                            <span class="gallery-corner-badge">{ratio_label}</span>
                                                            <span class="gallery-corner-badge">{size_label}</span>
                                                        </div>
                                                        <img class="compact-preview-image" src=preview_src alt=item.prompt.clone() />
                                                    </button>
                                                }.into_any()
                                            }).unwrap_or_else(|| view! { <div class="compact-preview-fallback muted">"无预览"</div> }.into_any())
                                            }}
                                            <div class="card-body stack compact-card-body">
                                                <p class="gallery-card-title">{item.prompt.clone()}</p>
                                                {error_message.map(|error| view! {
                                                    <span
                                                        class="status gallery-failure-summary"
                                                        title=error.clone()
                                                    >
                                                        {format!("失败：{error}")}
                                                    </span>
                                                })}
                                                {
                                                    let meta_label =
                                                        format!("{} · {}", item.config_name, item.model);
                                                    view! {
                                                        <div class="gallery-meta">
                                                            <span class="gallery-badge" title=meta_label.clone()>{meta_label.clone()}</span>
                                                        </div>
                                                    }
                                                }
                                                {if item_status == TaskStatus::Running {
                                                    view! {
                                                        <div class="row compact-actions">
                                                            <button
                                                                class="button ghost danger mini-action gallery-stop-action"
                                                                title="停止这个生成任务"
                                                                on:click=move |ev: MouseEvent| {
                                                                    confirm_popover.set(Some(ConfirmPopoverState {
                                                                        kind: ConfirmPopoverKind::CancelGeneration(cancel_task_id.clone()),
                                                                        title: "停止生成".into(),
                                                                        message: "确定停止这个生成任务吗？已经发送到上游的请求可能仍会产生消耗。".into(),
                                                                        x: ev.client_x() as f64,
                                                                        y: ev.client_y() as f64,
                                                                    }));
                                                                }
                                                            >
                                                                <MaterialSymbolIcon name="stop" filled=true />
                                                                <span>"停止"</span>
                                                            </button>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! { <div class="row compact-actions">
                                                    <button class="button ghost mini-action icon-action" title="重新生成" on:click=move |_| rerun_task(rerun_task_id.clone())><MaterialSymbolIcon name="restart_alt" filled=false /></button>
                                                    <button class="button ghost mini-action icon-action" title="继续修改" on:click=move |_| {
                                                        if let Some(first_asset) = assets.with_untracked(|items| first_displayable_generated_asset(items, &continue_task_id)) {
                                                            enter_continuation_context(continue_task_id.clone(), first_asset.id);
                                                        }
                                                    }><MaterialSymbolIcon name="edit_square" filled=false /></button>
                                                    <button class="button ghost mini-action icon-action" on:click=move |ev: MouseEvent| {
                                                        toggle_favorite_for_task(
                                                            favorite_task_id.clone(),
                                                            ev.client_x() as f64,
                                                            ev.client_y() as f64,
                                                        );
                                                    } title=move || {
                                                        if tasks.with(|items| {
                                                            items.iter()
                                                                .find(|task| task.id == favorite_title_task_id)
                                                                .map(|task| task.favorite)
                                                                .unwrap_or(item.favorite)
                                                        }) {
                                                            "取消收藏"
                                                        } else {
                                                            "收藏"
                                                        }
                                                    }>
                                                        <span
                                                            class="material-symbols-rounded app-icon"
                                                            class:is-filled=move || {
                                                                tasks.with(|items| {
                                                                    items.iter()
                                                                        .find(|task| task.id == favorite_fill_task_id)
                                                                        .map(|task| task.favorite)
                                                                        .unwrap_or(item.favorite)
                                                                })
                                                            }
                                                            aria-hidden="true"
                                                        >
                                                            {move || {
                                                                if tasks.with(|items| {
                                                                    items.iter()
                                                                        .find(|task| task.id == favorite_icon_task_id)
                                                                        .map(|task| task.favorite)
                                                                        .unwrap_or(item.favorite)
                                                                }) {
                                                                    "star"
                                                                } else {
                                                                    "star_outline"
                                                                }
                                                            }}
                                                        </span>
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
                                                    <button class="button ghost danger mini-action icon-action" title="删除记录" on:click=move |ev: MouseEvent| delete_task(delete_task_id.clone(), ev.client_x() as f64, ev.client_y() as f64)><MaterialSymbolIcon name="delete" filled=false /></button>
                                                    </div> }.into_any()
                                                }}
                                            </div>
                                        </article>
                                    }.into_any()
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <PaginationControls
                        page=gallery_page
                        page_count=gallery_page_count
                        favorite=false
                    />
                </aside>

    }
}
