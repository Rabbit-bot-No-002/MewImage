use leptos::{prelude::*, task::spawn_local};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::MouseEvent;

use crate::app::{
    aspect_ratio_label, asset_src, copy_image_from_src,
    derived::AppDerived,
    download_file_name_for_asset, download_image_from_src, ensure_asset_display_sources_loaded,
    format_shanghai_datetime,
    models::ContextMenuState,
    state::{ComposerState, UiState, WorkspaceState},
};

use super::common::{FullscreenImageViewer, MaterialSymbolIcon};

async fn loaded_asset_source(
    assets: RwSignal<Vec<mew_image_shared::ImageAssetRef>>,
    asset_id: &str,
) -> Result<(String, String), String> {
    ensure_asset_display_sources_loaded(assets, &[asset_id.to_string()]).await?;
    assets
        .with_untracked(|items| {
            items
                .iter()
                .find(|asset| asset.id == asset_id)
                .map(|asset| (asset_src(asset), download_file_name_for_asset(asset)))
                .filter(|(source, _)| !source.is_empty())
        })
        .ok_or_else(|| "图片原文件不可用，请重新同步或重新生成。".to_string())
}

fn copy_asset(
    assets: RwSignal<Vec<mew_image_shared::ImageAssetRef>>,
    asset_id: String,
    status_text: RwSignal<String>,
) {
    spawn_local(async move {
        let result = match loaded_asset_source(assets, &asset_id).await {
            Ok((source, _)) => copy_image_from_src(&source).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            status_text.set(format!("复制图片失败：{error}"));
        }
    });
}

fn download_asset(
    assets: RwSignal<Vec<mew_image_shared::ImageAssetRef>>,
    asset_id: String,
    status_text: RwSignal<String>,
) {
    spawn_local(async move {
        let result = match loaded_asset_source(assets, &asset_id).await {
            Ok((source, file_name)) => download_image_from_src(&source, &file_name),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            status_text.set(format!("下载图片失败：{error}"));
        }
    });
}

#[component]
pub(crate) fn PreviewOverlay(
    close_preview: impl Fn() + Copy + Send + Sync + 'static,
    continue_from_task: impl Fn(String) + Copy + Send + Sync + 'static,
    delete_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    edit_output_asset: impl Fn(String, String) + Copy + Send + Sync + 'static,
    hide_tip: impl Fn() + Copy + Send + Sync + 'static,
    reference_tip_enabled: impl Fn() -> bool + Copy + Send + Sync + 'static,
    show_tip: impl Fn(&'static str, f64, f64, bool) + Copy + Send + Sync + 'static,
    toggle_favorite_for_task: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let composer = expect_context::<ComposerState>();
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let selected_reference_ids = composer.selected_reference_ids;
    let status_text = composer.status_text;
    let generation_runtimes = composer.generation_runtimes;
    let assets = workspace.assets;
    let preview_panel_state = ui.preview_panel_state;
    let preview_fullscreen = ui.preview_fullscreen;
    let context_menu_state = ui.context_menu_state;
    let current_preview = derived.current_preview;

    view! {
            {move || current_preview.get().zip(preview_panel_state.get()).map(|((task, asset), panel)| {
                let preview_task_id = panel.task_id.clone();
                let preview_asset_id = asset.as_ref().map(|asset| asset.id.clone());
                let favorite_task_id = panel.task_id.clone();
                let delete_task_id = panel.task_id.clone();
                let edit_task_id = panel.task_id.clone();
                let edit_asset_id = asset
                    .as_ref()
                    .map(|asset| asset.id.clone())
                    .unwrap_or_default();
                let has_asset = asset.is_some();
                let fullscreen_src = asset
                    .as_ref()
                    .map(asset_src)
                    .filter(|source| !source.is_empty())
                    .or_else(|| panel.display_src.clone())
                    .unwrap_or_default();
                let preview_image_src = fullscreen_src.clone();
                let fullscreen_image_src = fullscreen_src.clone();
                let fullscreen_image_alt = panel.prompt.clone();
                let fullscreen_download_asset_id = preview_asset_id.clone();
                let copy_asset_id = preview_asset_id.clone();
                let download_asset_id = preview_asset_id.clone();
                let prompt_text = panel.prompt.clone();
                let waiting_task_id = panel.task_id.clone();
                let waiting_detail_task_id = panel.task_id.clone();
                let reference_thumb_ids = panel
                    .reference_thumbs
                    .iter()
                    .map(|thumb| thumb.id.clone())
                    .collect::<Vec<_>>();
                view! {
                    <div
                        class="preview-overlay"
                        on:click=move |_| close_preview()
                    >
                        <div class="preview-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <button class="button ghost icon-button preview-shell-close" title="关闭详情" on:click=move |_| close_preview()><MaterialSymbolIcon name="close" filled=false /></button>
                            <section class="preview-stage">
                                <div class="preview-stage-meta">
                                    <span class="tag">{aspect_ratio_label(panel.width, panel.height)}</span>
                                    <span class="tag">{format!("{}x{}", panel.width, panel.height)}</span>
                                </div>
                                <button
                                    class="image-button preview-image-button"
                                    disabled=move || !has_asset
                                    on:click=move |_| {
                                        if !has_asset {
                                            return;
                                        }
                                        preview_fullscreen.set(true);
                                    }
                                    on:contextmenu=move |ev: MouseEvent| {
                                        let Some(asset_id) = preview_asset_id.clone() else {
                                            return;
                                        };
                                        ev.prevent_default();
                                        context_menu_state.set(Some(ContextMenuState {
                                            task_id: preview_task_id.clone(),
                                            asset_id,
                                            x: ev.client_x() as f64,
                                            y: ev.client_y() as f64,
                                        }));
                                    }
                                >
                                    {if has_asset {
                                        view! {
                                            <img
                                                class="preview-image"
                                                src=preview_image_src
                                                alt=panel.prompt.clone()
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="preview-waiting-stage">
                                                <span class="gallery-running-spinner"></span>
                                                <strong>{move || generation_runtimes.with(|items| {
                                                    items
                                                        .get(&waiting_task_id)
                                                        .map(|runtime| runtime.phase.label())
                                                        .unwrap_or_else(|| "等待生成结果".into())
                                                })}</strong>
                                                <span class="status">{move || generation_runtimes.with(|items| {
                                                    items.get(&waiting_detail_task_id).map(|runtime| {
                                                        if matches!(runtime.phase, crate::app::state::GenerationRuntimePhase::WaitingFullTaskBudget) {
                                                            format!(
                                                                "Direct 或旧版代理需要完整内存保护：预计 {}，浏览器软预算 {}；大型任务会独占处理。",
                                                                crate::app::format_byte_size(runtime.requested_bytes),
                                                                crate::app::format_byte_size(runtime.budget_bytes),
                                                            )
                                                        } else if runtime.phase.waits_for_budget() {
                                                            format!(
                                                                "预计处理 {}，浏览器软预算 {}；大型任务会独占处理。",
                                                                crate::app::format_byte_size(runtime.requested_bytes),
                                                                crate::app::format_byte_size(runtime.budget_bytes),
                                                            )
                                                        } else if matches!(runtime.phase, crate::app::state::GenerationRuntimePhase::DirectProtected | crate::app::state::GenerationRuntimePhase::LegacyProxyProtected) {
                                                            "Direct 或旧版代理无法延迟领取结果，当前任务使用完整内存保护。".into()
                                                        } else {
                                                            "任务完成后即可查看生成图片。".into()
                                                        }
                                                    }).unwrap_or_else(|| "任务完成后即可查看生成图片。".into())
                                                })}</span>
                                            </div>
                                        }.into_any()
                                    }}
                                </button>
                            </section>
                            <aside class="preview-sidebar">
                                <div class="row preview-sidebar-top">
                                    <div class="stack">
                                        <div class="row preview-prompt-head">
                                            <span class="status">"输入内容"</span>
                                            <button
                                                class="button ghost icon-button preview-copy-button"
                                                on:mouseenter=move |ev: web_sys::MouseEvent| {
                                                    let target = ev.current_target().and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok());
                                                    if let Some(target) = target {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("复制提示词", rect.left(), rect.top() + 18.0, true);
                                                    }
                                                }
                                                on:mouseleave=move |_| hide_tip()
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    let text = prompt_text.clone();
                                                    let target = ev.current_target().and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok());
                                                    if let Some(target) = target {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("提示词已复制~", rect.left(), rect.top() + 18.0, false);
                                                    }
                                                    spawn_local(async move {
                                                        let Some(window) = web_sys::window() else {
                                                            return;
                                                        };
                                                        let clipboard = window.navigator().clipboard();
                                                        let _ = JsFuture::from(clipboard.write_text(&text)).await;
                                                    });
                                                }
                                            >
                                                <MaterialSymbolIcon name="content_copy" filled=false />
                                            </button>
                                        </div>
                                        <div class="preview-prompt-box">
                                            <p class="preview-prompt">{panel.prompt.clone()}</p>
                                        </div>
                                    </div>
                                </div>
                                <div class="preview-sidebar-fixed">
                                <div class="stack">
                                    <div class="row preview-prompt-head">
                                        <span class="status">"参考图"</span>
                                        <button
                                            class="button ghost icon-button preview-copy-button"
                                            disabled=move || !reference_tip_enabled()
                                            on:mouseenter=move |ev: web_sys::MouseEvent| {
                                                if reference_tip_enabled()
                                                    && let Some(target) = ev
                                                        .current_target()
                                                        .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                                                {
                                                    let rect = target.get_bounding_client_rect();
                                                    show_tip("引用参考图", rect.left(), rect.top() + 18.0, true);
                                                }
                                            }
                                            on:mouseleave=move |_| hide_tip()
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                if !reference_tip_enabled() {
                                                    return;
                                                }
                                                selected_reference_ids.set(reference_thumb_ids.clone());
                                                if let Some(target) = ev
                                                    .current_target()
                                                    .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                                                {
                                                    let rect = target.get_bounding_client_rect();
                                                    show_tip("参考图已引用~", rect.left(), rect.top() + 18.0, false);
                                                }
                                            }
                                        >
                                            <MaterialSymbolIcon name="link" filled=false />
                                        </button>
                                    </div>
                                    <div class="preview-ref-strip">
                                        <For
                                            each=move || panel.reference_thumbs.clone()
                                            key=|item| item.id.clone()
                                            children=move |item| {
                                                view! {
                                                    <div class="preview-ref-card">
                                                        <img src=item.src alt="参考图缩略图" />
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                </div>
                                <div class="preview-details-grid">
                                    <div class="detail-card is-source is-wide">
                                        <span class="detail-label">"来源"</span>
                                        <strong class="detail-value detail-value-wrap">{format!("{} · {}", panel.source_label, panel.requested_model)}</strong>
                                    </div>
                                    <div class="detail-card is-wide">
                                        <span class="detail-label">"质量"</span>
                                        <strong class="detail-value detail-value-wrap">{format!("请求 {} / 实际 {}", panel.requested_quality_label, panel.actual_quality_label)}</strong>
                                    </div>
                                    <div class="detail-card is-inline">
                                        <span class="detail-label">"尺寸"</span>
                                        <strong class="detail-value">{format!("{}x{}", panel.width, panel.height)}</strong>
                                    </div>
                                    <div class="detail-card is-inline">
                                        <span class="detail-label">"格式"</span>
                                        <strong class="detail-value">{panel.format_label.clone()}</strong>
                                    </div>
                                    <div class="detail-card is-inline">
                                        <span class="detail-label">"审核"</span>
                                        <strong class="detail-value">{panel.moderation_label.clone()}</strong>
                                    </div>
                                </div>
                                <div class="preview-time-meta">
                                    <span>{format!("创建于 {}", format_shanghai_datetime(&panel.created_at))}</span>
                                    <span>"·"</span>
                                    <span>{format!("耗时 {}", panel.duration_label.clone())}</span>
                                </div>
                                <div class="row preview-actions preview-actions-primary">
                                    <button class="button ghost" on:click=move |_| {
                                        continue_from_task(task.id.clone());
                                        close_preview();
                                    }>"复用配置"</button>
                                    {has_asset.then(|| view! {
                                        <>
                                            <button class="button secondary" on:click=move |_| edit_output_asset(edit_task_id.clone(), edit_asset_id.clone())>"编辑输出"</button>
                                            <button class="button ghost danger" on:click=move |ev: MouseEvent| {
                                                delete_task(delete_task_id.clone(), ev.client_x() as f64, ev.client_y() as f64);
                                            }>"删除记录"</button>
                                            <button class="button ghost" on:click=move |ev: MouseEvent| {
                                                toggle_favorite_for_task(
                                                    favorite_task_id.clone(),
                                                    ev.client_x() as f64,
                                                    ev.client_y() as f64,
                                                );
                                            }>
                                                {move || if preview_panel_state.get().map(|state| state.favorite).unwrap_or(false) { "取消收藏" } else { "收藏" }}
                                            </button>
                                        </>
                                    })}
                                </div>
                                {has_asset.then(|| view! {
                                    <div class="row preview-actions">
                                        <button class="button ghost" on:click=move |_| {
                                            let Some(asset_id) = copy_asset_id.clone() else {
                                                return;
                                            };
                                            copy_asset(assets, asset_id, status_text);
                                        }>"复制"</button>
                                        <button class="button ghost" on:click=move |_| {
                                            let Some(asset_id) = download_asset_id.clone() else {
                                                return;
                                            };
                                            download_asset(assets, asset_id, status_text);
                                        }>"下载"</button>
                                    </div>
                                })}
                                </div>
                            </aside>
                        </div>
                        {move || {
                            if !has_asset || !preview_fullscreen.get() {
                                return ().into_any();
                            }
                            let download_asset_id = fullscreen_download_asset_id.clone();
                            view! {
                                <FullscreenImageViewer
                                    src=fullscreen_image_src.clone()
                                    alt=fullscreen_image_alt.clone()
                                    show_download=true
                                    close=move || preview_fullscreen.set(false)
                                    download=move || {
                                        let Some(asset_id) = download_asset_id.clone() else {
                                            return;
                                        };
                                        download_asset(assets, asset_id, status_text);
                                    }
                                />
                            }.into_any()
                        }}
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

    }
}
