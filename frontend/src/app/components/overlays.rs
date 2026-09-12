use leptos::{prelude::*, task::spawn_local};
use wasm_bindgen_futures::JsFuture;
use web_sys::MouseEvent;

use crate::app::state::UiState;

use super::common::MaterialSymbolIcon;
use crate::app::{
    asset_full_preview_src, asset_src, copy_image_from_src, download_file_name_for_asset,
    download_image_from_src, ensure_asset_display_sources_loaded,
};

#[component]
pub(crate) fn FailureLogOverlay(
    delete_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let failure_log_state = expect_context::<UiState>().failure_log_state;

    view! {
            {move || failure_log_state.get().map(|log| {
                let copy_text = log.details.clone();
                let delete_task_id = log.task_id.clone();
                view! {
                    <div class="preview-overlay" on:click=move |_| failure_log_state.set(None)>
                        <div class="preview-shell failure-log-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <button class="button ghost icon-button preview-shell-close" title="关闭日志" on:click=move |_| failure_log_state.set(None)>
                                <MaterialSymbolIcon name="close" filled=false />
                            </button>
                            <div class="stack failure-log-top">
                                <h3>{log.title.clone()}</h3>
                                <span class="status">{log.summary.clone()}</span>
                            </div>
                            <pre class="failure-log-text">{log.details.clone()}</pre>
                            <div class="row preview-actions">
                                <button class="button ghost" on:click=move |_| {
                                    let text = copy_text.clone();
                                    spawn_local(async move {
                                        let Some(window) = web_sys::window() else {
                                            return;
                                        };
                                        let _ = JsFuture::from(window.navigator().clipboard().write_text(&text)).await;
                                    });
                                }>
                                    <MaterialSymbolIcon name="content_copy" filled=false />
                                    "复制"
                                </button>
                                <button class="button ghost danger" on:click=move |ev: MouseEvent| {
                                    delete_task(delete_task_id.clone(), ev.client_x() as f64, ev.client_y() as f64);
                                    failure_log_state.set(None);
                                }>
                                    <MaterialSymbolIcon name="delete" filled=false />
                                    "删除任务"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}
    }
}

#[component]
pub(crate) fn FloatingTipOverlay() -> impl IntoView {
    let floating_tip_state = expect_context::<UiState>().floating_tip_state;

    view! {
            {move || floating_tip_state.get().map(|tip| {
                view! {
                    <div
                        class="floating-tip"
                        style=format!("left: {}px; top: {}px;", tip.x, tip.y)
                    >
                        {tip.text.clone()}
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}
    }
}

#[component]
pub(crate) fn ContextMenuOverlay(
    edit_output_asset: impl Fn(String, String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<crate::app::state::WorkspaceState>();
    let context_menu_state = expect_context::<UiState>().context_menu_state;
    let status_text = expect_context::<crate::app::state::ComposerState>().status_text;
    let assets = workspace.assets;

    view! {
        {move || context_menu_state.get().map(|menu| {
            let x = menu.x;
            let y = menu.y;
            let task_id = menu.task_id.clone();
            let asset_id = menu.asset_id.clone();
            let edit_task_id = task_id.clone();
            let edit_asset_id = asset_id.clone();
            let copy_asset_id = asset_id.clone();
            let download_asset_id = asset_id.clone();
            view! {
                <div class="context-menu-layer" on:click=move |_| context_menu_state.set(None)>
                    <div
                        class="context-menu"
                        style=format!("left: min({x}px, calc(100vw - 180px)); top: min({y}px, calc(100vh - 180px));")
                        on:click=move |ev: MouseEvent| ev.stop_propagation()
                    >
                        <button class="button ghost context-item" on:click=move |_| {
                            context_menu_state.set(None);
                            let asset_id = copy_asset_id.clone();
                            spawn_local(async move {
                                if let Err(error) = ensure_asset_display_sources_loaded(
                                    assets,
                                    std::slice::from_ref(&asset_id),
                                )
                                .await
                                {
                                    status_text.set(format!("复制图片失败：{error}"));
                                    return;
                                }
                                let src = assets.with_untracked(|items| {
                                    items
                                        .iter()
                                        .find(|asset| asset.id == asset_id)
                                        .map(asset_src)
                                        .unwrap_or_default()
                                });
                                if !src.is_empty()
                                    && let Err(error) = copy_image_from_src(&src).await
                                {
                                    status_text.set(format!("复制图片失败：{error}"));
                                }
                            });
                        }>"复制"</button>
                        <button class="button ghost context-item" on:click=move |_| {
                            context_menu_state.set(None);
                            let asset_id = download_asset_id.clone();
                            spawn_local(async move {
                                if let Err(error) = ensure_asset_display_sources_loaded(
                                    assets,
                                    std::slice::from_ref(&asset_id),
                                )
                                .await
                                {
                                    status_text.set(format!("下载图片失败：{error}"));
                                    return;
                                }
                                let source = assets.with_untracked(|items| {
                                    items
                                        .iter()
                                        .find(|asset| asset.id == asset_id)
                                        .map(|asset| {
                                            (asset_src(asset), download_file_name_for_asset(asset))
                                        })
                                });
                                if let Some((src, file_name)) = source.filter(|(src, _)| !src.is_empty())
                                    && let Err(error) = download_image_from_src(&src, &file_name)
                                {
                                    status_text.set(format!("下载图片失败：{error}"));
                                }
                            });
                        }>"下载"</button>
                        <button class="button ghost context-item" on:click=move |_| {
                            edit_output_asset(edit_task_id.clone(), edit_asset_id.clone());
                            context_menu_state.set(None);
                        }>"编辑"</button>
                    </div>
                </div>
            }.into_any()
        }).unwrap_or_else(|| ().into_any())}
    }
}

#[component]
pub(crate) fn ReferenceMenuOverlay(
    delete_asset: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let ui = expect_context::<crate::app::state::UiState>();
    let composer = expect_context::<crate::app::state::ComposerState>();
    let workspace = expect_context::<crate::app::state::WorkspaceState>();
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let assets = workspace.assets;

    view! {
        {move || reference_menu_asset_id.get().and_then(|asset_id| {
            assets.with(|items| items.iter().find(|asset| asset.id == asset_id).cloned())
        }).map(|asset| {
            let delete_asset_id = asset.id.clone();
            let edit_asset_id = asset.id.clone();
            let toggle_reference_id = asset.id.clone();
            let toggle_reference_label_id = asset.id.clone();
            view! {
                <div class="preview-overlay" on:click=move |_| reference_menu_asset_id.set(None)>
                    <div class="reference-menu-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                        <div class="row reference-menu-top">
                            <div class="stack">
                                <h3>"参考图操作"</h3>
                                <span class="status">"可设为参考、复制、下载或删除。"</span>
                            </div>
                            <button class="button ghost icon-button" on:click=move |_| reference_menu_asset_id.set(None)><MaterialSymbolIcon name="close" filled=false /></button>
                        </div>
                        <div class="reference-menu-preview">
                            <img src=asset_full_preview_src(&asset) alt="参考图预览" />
                        </div>
                        <div class="row reference-menu-actions">
                            <button class="button ghost" on:click=move |_| {
                                reference_menu_asset_id.set(None);
                                ui.image_editor_base_id.set(Some(edit_asset_id.clone()));
                                ui.image_editor_thread.set(Some(workspace.current_thread_id.get_untracked()));
                            }>"编辑"</button>
                            <button class="button ghost" on:click=move |_| {
                                selected_reference_ids.update(|ids| {
                                    if let Some(index) = ids.iter().position(|id| id == &toggle_reference_id) {
                                        ids.remove(index);
                                    } else {
                                        let base = composer.continuation_asset_id.get_untracked();
                                        let extra = usize::from(base.as_ref().is_some_and(|id| id != &toggle_reference_id && !ids.contains(id)));
                                        if ids.len() + extra >= mew_image_shared::MAX_GENERATION_REFERENCE_IMAGES {
                                            composer.status_text.set("最多选择 10 张参考图，请先取消部分选择。".into());
                                            return;
                                        }
                                        ids.push(toggle_reference_id.clone());
                                    }
                                });
                            }>
                                {move || if selected_reference_ids.get().contains(&toggle_reference_label_id) { "取消参考" } else { "设为参考" }}
                            </button>
                            <button class="button ghost danger" on:click=move |ev: MouseEvent| {
                                reference_menu_asset_id.set(None);
                                delete_asset(
                                    delete_asset_id.clone(),
                                    ev.client_x() as f64,
                                    ev.client_y() as f64,
                                );
                            }>"删除图片"</button>
                        </div>
                    </div>
                </div>
            }.into_any()
        }).unwrap_or_else(|| ().into_any())}
    }
}
