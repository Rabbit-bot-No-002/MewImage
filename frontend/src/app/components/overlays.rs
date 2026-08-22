use leptos::{prelude::*, task::spawn_local};
use wasm_bindgen_futures::JsFuture;
use web_sys::MouseEvent;

use crate::app::state::UiState;

use super::common::MaterialSymbolIcon;
use crate::app::{
    asset_full_preview_src, asset_src, copy_image_from_src, download_file_name_for_asset,
    download_file_name_for_src, download_image_from_src,
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
    let assets = workspace.assets;

    view! {
        {move || context_menu_state.get().map(|menu| {
            let x = menu.x;
            let y = menu.y;
            let task_id = menu.task_id.clone();
            let asset_id = menu.asset_id.clone();
            let copy_src = assets.with(|items| {
                items.iter()
                    .find(|asset| asset.id == asset_id)
                    .map(asset_src)
                    .unwrap_or_default()
            });
            let download_src = copy_src.clone();
            let download_name = assets.with(|items| {
                items.iter()
                    .find(|asset| asset.id == asset_id)
                    .map(download_file_name_for_asset)
                    .unwrap_or_else(|| download_file_name_for_src(&download_src))
            });
            let edit_task_id = task_id.clone();
            let edit_asset_id = asset_id.clone();
            view! {
                <div class="context-menu-layer" on:click=move |_| context_menu_state.set(None)>
                    <div
                        class="context-menu"
                        style=format!("left: min({x}px, calc(100vw - 180px)); top: min({y}px, calc(100vh - 180px));")
                        on:click=move |ev: MouseEvent| ev.stop_propagation()
                    >
                        <button class="button ghost context-item" on:click=move |_| {
                            let src = copy_src.clone();
                            context_menu_state.set(None);
                            spawn_local(async move {
                                let _ = copy_image_from_src(&src).await;
                            });
                        }>"复制"</button>
                        <button class="button ghost context-item" on:click=move |_| {
                            let _ = download_image_from_src(&download_src, &download_name);
                            context_menu_state.set(None);
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
    let composer = expect_context::<crate::app::state::ComposerState>();
    let derived = expect_context::<crate::app::derived::AppDerived>();
    let reference_menu_asset_id = composer.reference_menu_asset_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let current_reference_menu_asset = derived.current_reference_menu_asset;

    view! {
        {move || current_reference_menu_asset.get().map(|asset| {
            let delete_asset_id = asset.id.clone();
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
                                selected_reference_ids.update(|ids| {
                                    if let Some(index) = ids.iter().position(|id| id == &toggle_reference_id) {
                                        ids.remove(index);
                                    } else {
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
