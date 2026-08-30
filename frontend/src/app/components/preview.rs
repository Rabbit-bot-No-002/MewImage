use leptos::{prelude::*, task::spawn_local};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{MouseEvent, WheelEvent};

use crate::app::{
    aspect_ratio_label, asset_src, copy_image_from_src,
    derived::AppDerived,
    download_file_name_for_asset, download_image_from_src, format_shanghai_datetime,
    models::ContextMenuState,
    state::{ComposerState, UiState},
};

use super::common::MaterialSymbolIcon;

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
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let selected_reference_ids = composer.selected_reference_ids;
    let preview_panel_state = ui.preview_panel_state;
    let preview_fullscreen = ui.preview_fullscreen;
    let preview_zoom = ui.preview_zoom;
    let preview_offset_x = ui.preview_offset_x;
    let preview_offset_y = ui.preview_offset_y;
    let preview_dragging = ui.preview_dragging;
    let preview_drag_origin_x = ui.preview_drag_origin_x;
    let preview_drag_origin_y = ui.preview_drag_origin_y;
    let preview_drag_start_x = ui.preview_drag_start_x;
    let preview_drag_start_y = ui.preview_drag_start_y;
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
                let copy_src = fullscreen_src.clone();
                let toolbar_download_src = fullscreen_src.clone();
                let download_src = fullscreen_src.clone();
                let toolbar_download_name = asset
                    .as_ref()
                    .map(download_file_name_for_asset)
                    .unwrap_or_default();
                let download_name = toolbar_download_name.clone();
                let prompt_text = panel.prompt.clone();
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
                            <section class="preview-stage" class:is-fullscreen=move || preview_fullscreen.get()>
                                <div class="preview-stage-meta">
                                    <span class="tag">{aspect_ratio_label(panel.width, panel.height)}</span>
                                    <span class="tag">{format!("{}x{}", panel.width, panel.height)}</span>
                                </div>
                                {move || {
                                    if has_asset && preview_fullscreen.get() {
                                        let toolbar_download_src = toolbar_download_src.clone();
                                        let toolbar_download_name = toolbar_download_name.clone();
                                        view! {
                                            <div class="preview-fullscreen-toolbar">
                                                <button
                                                    class="button ghost icon-button preview-toolbar-button"
                                                    title="下载原图"
                                                    on:click=move |_| {
                                                        let src = toolbar_download_src.clone();
                                                        let _ = download_image_from_src(&src, &toolbar_download_name);
                                                    }
                                                >
                                                    <MaterialSymbolIcon name="download" filled=false />
                                                </button>
                                                <button
                                                    class="button ghost icon-button preview-toolbar-button"
                                                    title="退出大图"
                                                    on:click=move |_| {
                                                        preview_fullscreen.set(false);
                                                        preview_zoom.set(1.0);
                                                        preview_offset_x.set(0.0);
                                                        preview_offset_y.set(0.0);
                                                        preview_dragging.set(false);
                                                    }
                                                >
                                                    <MaterialSymbolIcon name="close" filled=false />
                                                </button>
                                            </div>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }
                                }}
                                <button
                                    class="image-button preview-image-button"
                                    class:is-pan-enabled=move || preview_fullscreen.get()
                                    disabled=move || !has_asset
                                    on:click=move |_| {
                                        if !has_asset {
                                            return;
                                        }
                                        if !preview_fullscreen.get_untracked() {
                                            preview_fullscreen.set(true);
                                            preview_zoom.set(1.0);
                                            preview_offset_x.set(0.0);
                                            preview_offset_y.set(0.0);
                                        }
                                    }
                                    on:mousedown=move |ev: MouseEvent| {
                                        if !has_asset || !preview_fullscreen.get_untracked() {
                                            return;
                                        }
                                        ev.prevent_default();
                                        preview_dragging.set(true);
                                        preview_drag_origin_x.set(preview_offset_x.get_untracked());
                                        preview_drag_origin_y.set(preview_offset_y.get_untracked());
                                        preview_drag_start_x.set(ev.client_x() as f64);
                                        preview_drag_start_y.set(ev.client_y() as f64);
                                    }
                                    on:mousemove=move |ev: MouseEvent| {
                                        if !preview_dragging.get_untracked() {
                                            return;
                                        }
                                        let delta_x = ev.client_x() as f64 - preview_drag_start_x.get_untracked();
                                        let delta_y = ev.client_y() as f64 - preview_drag_start_y.get_untracked();
                                        preview_offset_x.set(preview_drag_origin_x.get_untracked() + delta_x);
                                        preview_offset_y.set(preview_drag_origin_y.get_untracked() + delta_y);
                                    }
                                    on:mouseup=move |_| {
                                        preview_dragging.set(false);
                                    }
                                    on:mouseleave=move |_| {
                                        preview_dragging.set(false);
                                    }
                                    on:wheel=move |ev: WheelEvent| {
                                        if !has_asset || !preview_fullscreen.get_untracked() {
                                            return;
                                        }
                                        ev.prevent_default();
                                        let current = preview_zoom.get_untracked();
                                        let delta = if ev.delta_y() < 0.0 { 0.12 } else { -0.12 };
                                        let next = (current + delta).clamp(0.4, 6.0);
                                        preview_zoom.set(next);
                                        if (next - 1.0).abs() < 0.02 {
                                            preview_zoom.set(1.0);
                                            preview_offset_x.set(0.0);
                                            preview_offset_y.set(0.0);
                                        }
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
                                                class:is-zoomed=move || preview_fullscreen.get()
                                                style=move || {
                                                    format!(
                                                        "transform: translate({:.1}px, {:.1}px) scale({:.3});",
                                                        preview_offset_x.get(),
                                                        preview_offset_y.get(),
                                                        preview_zoom.get()
                                                    )
                                                }
                                                src=preview_image_src
                                                alt=panel.prompt.clone()
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="preview-waiting-stage">
                                                <span class="gallery-running-spinner"></span>
                                                <strong>"正在等待上游结果"</strong>
                                                <span class="status">"任务完成后即可查看生成图片"</span>
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
                                <div class="stack">
                                    <div class="row preview-prompt-head">
                                        <span class="status">"参考图"</span>
                                        <button
                                            class="button ghost icon-button preview-copy-button"
                                            disabled=move || !reference_tip_enabled()
                                            on:mouseenter=move |ev: web_sys::MouseEvent| {
                                                if reference_tip_enabled() {
                                                    if let Some(target) = ev
                                                        .current_target()
                                                        .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                                                    {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("引用参考图", rect.left(), rect.top() + 18.0, true);
                                                    }
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
                                    <div class="detail-card is-source">
                                        <span class="detail-label">"来源"</span>
                                        <strong class="detail-value detail-value-wrap">{format!("{} · {}", panel.source_label, panel.requested_model)}</strong>
                                    </div>
                                    <div class="detail-card">
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
                                    <div class="detail-card is-inline">
                                        <span class="detail-label">"背景"</span>
                                        <strong class="detail-value">{panel.background_label.clone()}</strong>
                                    </div>
                                    <div class="detail-card is-inline">
                                        <span class="detail-label">"数量"</span>
                                        <strong class="detail-value">{panel.image_count.to_string()}</strong>
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
                                            let src = copy_src.clone();
                                            spawn_local(async move {
                                                let _ = copy_image_from_src(&src).await;
                                            });
                                        }>"复制"</button>
                                        <button class="button ghost" on:click=move |_| {
                                            let _ = download_image_from_src(&download_src, &download_name);
                                        }>"下载"</button>
                                    </div>
                                })}
                            </aside>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

    }
}
