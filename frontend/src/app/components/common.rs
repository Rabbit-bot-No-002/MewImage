use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, KeyboardEvent, MouseEvent, WheelEvent};

#[component]
pub(crate) fn MaterialSymbolIcon(name: &'static str, filled: bool) -> impl IntoView {
    view! {
        <span
            class="material-symbols-rounded app-icon"
            class:is-filled=filled
            aria-hidden="true"
        >
            {name}
        </span>
    }
}

#[component]
pub(crate) fn GitHubIcon() -> impl IntoView {
    view! {
        <svg class="github-brand-icon" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 .7a11.5 11.5 0 0 0-3.64 22.41c.58.1.79-.25.79-.56v-2.23c-3.22.7-3.9-1.37-3.9-1.37-.52-1.34-1.28-1.69-1.28-1.69-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.7 1.26 3.36.97.1-.75.4-1.26.73-1.55-2.57-.29-5.27-1.28-5.27-5.68 0-1.26.45-2.28 1.19-3.09-.12-.29-.52-1.47.11-3.05 0 0 .97-.31 3.16 1.18a10.98 10.98 0 0 1 5.76 0c2.2-1.49 3.16-1.18 3.16-1.18.63 1.58.23 2.76.11 3.05.74.81 1.19 1.83 1.19 3.09 0 4.41-2.71 5.38-5.29 5.67.42.36.78 1.06.78 2.14v3.18c0 .31.21.67.8.56A11.5 11.5 0 0 0 12 .7Z" />
        </svg>
    }
}

#[component]
pub(crate) fn FullscreenImageViewer(
    src: String,
    alt: String,
    close: impl Fn() + Send + Sync + 'static,
    download: impl Fn() + Send + Sync + 'static,
    show_download: bool,
) -> impl IntoView {
    let zoom = RwSignal::new(1.0_f64);
    let offset_x = RwSignal::new(0.0_f64);
    let offset_y = RwSignal::new(0.0_f64);
    let dragging = RwSignal::new(false);
    let drag_start_x = RwSignal::new(0.0_f64);
    let drag_start_y = RwSignal::new(0.0_f64);
    let drag_origin_x = RwSignal::new(0.0_f64);
    let drag_origin_y = RwSignal::new(0.0_f64);

    let reset_view = move || {
        zoom.set(1.0);
        offset_x.set(0.0);
        offset_y.set(0.0);
        dragging.set(false);
    };
    let step_zoom = move |factor: f64| {
        let next = (zoom.get_untracked() * factor).clamp(0.5, 8.0);
        zoom.set(next);
        if (next - 1.0).abs() < 0.02 {
            reset_view();
        }
    };

    view! {
        <div
            class="fullscreen-image-viewer"
            class:is-dragging=move || dragging.get()
            role="dialog"
            aria-modal="true"
            aria-label="全屏查看图片"
            on:click=move |event: MouseEvent| event.stop_propagation()
            on:mousedown=move |event: MouseEvent| {
                if event.button() != 0 {
                    return;
                }
                event.prevent_default();
                dragging.set(true);
                drag_start_x.set(event.client_x() as f64);
                drag_start_y.set(event.client_y() as f64);
                drag_origin_x.set(offset_x.get_untracked());
                drag_origin_y.set(offset_y.get_untracked());
            }
            on:mousemove=move |event: MouseEvent| {
                if !dragging.get_untracked() {
                    return;
                }
                offset_x.set(
                    drag_origin_x.get_untracked()
                        + event.client_x() as f64
                        - drag_start_x.get_untracked(),
                );
                offset_y.set(
                    drag_origin_y.get_untracked()
                        + event.client_y() as f64
                        - drag_start_y.get_untracked(),
                );
            }
            on:mouseup=move |_| dragging.set(false)
            on:mouseleave=move |_| dragging.set(false)
            on:wheel=move |event: WheelEvent| {
                event.prevent_default();
                let current = zoom.get_untracked();
                let factor = (-event.delta_y() * 0.0015).exp().clamp(0.75, 1.25);
                let next = (current * factor).clamp(0.5, 8.0);
                if (next - current).abs() < f64::EPSILON {
                    return;
                }

                // 以鼠标指针为缩放中心，避免放大后目标区域从视野中跳走。
                if let Some(element) = event
                    .current_target()
                    .and_then(|target| target.dyn_into::<Element>().ok())
                {
                    let rect = element.get_bounding_client_rect();
                    let pointer_x = event.client_x() as f64 - rect.left() - rect.width() / 2.0;
                    let pointer_y = event.client_y() as f64 - rect.top() - rect.height() / 2.0;
                    let image_x = (pointer_x - offset_x.get_untracked()) / current;
                    let image_y = (pointer_y - offset_y.get_untracked()) / current;
                    offset_x.set(pointer_x - image_x * next);
                    offset_y.set(pointer_y - image_y * next);
                }
                zoom.set(next);
            }
        >
            <img
                class="fullscreen-image-content"
                class:is-dragging=move || dragging.get()
                src=src
                alt=alt
                draggable="false"
                style=move || format!(
                    "transform: translate3d({:.1}px, {:.1}px, 0) scale({:.4});",
                    offset_x.get(),
                    offset_y.get(),
                    zoom.get(),
                )
            />
            <div
                class="fullscreen-image-toolbar"
                on:mousedown=move |event: MouseEvent| event.stop_propagation()
            >
                <button class="button ghost icon-button" title="缩小" on:click=move |_| step_zoom(1.0 / 1.2)>
                    <MaterialSymbolIcon name="zoom_out" filled=false />
                </button>
                <span class="fullscreen-image-scale">{move || format!("{:.0}%", zoom.get() * 100.0)}</span>
                <button class="button ghost icon-button" title="放大" on:click=move |_| step_zoom(1.2)>
                    <MaterialSymbolIcon name="zoom_in" filled=false />
                </button>
                <button class="button ghost icon-button" title="恢复原始视图" on:click=move |_| reset_view()>
                    <MaterialSymbolIcon name="fit_screen" filled=false />
                </button>
                {show_download.then(|| view! {
                    <button class="button ghost icon-button" title="下载原图" on:click=move |_| download()>
                        <MaterialSymbolIcon name="download" filled=false />
                    </button>
                })}
                <button class="button ghost icon-button" title="退出大图" on:click=move |_| close()>
                    <MaterialSymbolIcon name="close" filled=false />
                </button>
            </div>
            <div class="fullscreen-image-hint">"滚轮缩放 · 左键拖拽 · Esc 退出"</div>
        </div>
    }
}

#[component]
pub(crate) fn PaginationControls(
    page: RwSignal<usize>,
    page_count: Memo<usize>,
    favorite: bool,
) -> impl IntoView {
    let show_picker = RwSignal::new(false);
    let candidate = RwSignal::new(1usize);
    let page_label = Memo::new(move |_| format!("{}/{}", page.get(), page_count.get()));
    let picker_rows = Memo::new(move |_| {
        let total = page_count.get().max(1);
        let current = candidate.get().clamp(1, total) as isize;
        (-2..=2)
            .filter_map(|offset| {
                let page = current + offset;
                (1..=total as isize)
                    .contains(&page)
                    .then_some((page as usize, offset))
            })
            .collect::<Vec<_>>()
    });
    let can_prev = Memo::new(move |_| page.get() > 1);
    let can_next = Memo::new(move |_| page.get() < page_count.get());
    let step_candidate = move |delta: isize| {
        let total = page_count.get_untracked().max(1);
        let current = candidate.get_untracked().clamp(1, total) as isize;
        candidate.set((current + delta).clamp(1, total as isize) as usize);
    };
    let submit = move || {
        let total = page_count.get_untracked().max(1);
        page.set(candidate.get_untracked().clamp(1, total));
        show_picker.set(false);
    };

    Effect::new(move |_| {
        let total = page_count.get().max(1);
        if page.get() > total {
            page.set(total);
        }
    });

    view! {
        <div
            class="gallery-pagination-footer"
            class:favorite-pagination-footer=favorite
        >
            <div class="gallery-pagination-anchor">
                {move || if show_picker.get() {
                    view! {
                        <>
                            <button
                                class="gallery-page-dismiss-layer"
                                aria-label="关闭页码选择"
                                on:click=move |_| show_picker.set(false)
                            ></button>
                            <div class="gallery-page-popover-layer">
                                <div
                                    class="gallery-page-popover"
                                    on:wheel=move |ev: WheelEvent| {
                                        ev.prevent_default();
                                        if ev.delta_y() < 0.0 {
                                            step_candidate(-1);
                                        } else if ev.delta_y() > 0.0 {
                                            step_candidate(1);
                                        }
                                    }
                                >
                                    <div class="gallery-page-popover-body">
                                        <div
                                            class="gallery-page-wheel"
                                            tabindex="0"
                                            on:keydown=move |ev: KeyboardEvent| {
                                                match ev.key().as_str() {
                                                    "ArrowUp" => {
                                                        ev.prevent_default();
                                                        step_candidate(-1);
                                                    }
                                                    "ArrowDown" => {
                                                        ev.prevent_default();
                                                        step_candidate(1);
                                                    }
                                                    "Enter" => submit(),
                                                    "Escape" => show_picker.set(false),
                                                    _ => {}
                                                }
                                            }
                                        >
                                            <For
                                                each=move || picker_rows.get()
                                                key=|(page, offset)| format!("{page}-{offset}")
                                                children=move |(target_page, offset)| {
                                                    let is_focused = offset == 0;
                                                    let is_near = offset.abs() == 1;
                                                    let is_far = offset.abs() >= 2;
                                                    view! {
                                                        <button
                                                            class="gallery-page-wheel-item"
                                                            class:is-focused=is_focused
                                                            class:is-near=is_near
                                                            class:is-far=is_far
                                                            on:click=move |_| candidate.set(target_page)
                                                            on:wheel=move |ev: WheelEvent| {
                                                                ev.prevent_default();
                                                                if ev.delta_y() < 0.0 {
                                                                    step_candidate(-1);
                                                                } else if ev.delta_y() > 0.0 {
                                                                    step_candidate(1);
                                                                }
                                                            }
                                                        >
                                                            {target_page.to_string()}
                                                        </button>
                                                    }
                                                }
                                            />
                                        </div>
                                        <button
                                            class="button secondary gallery-page-confirm"
                                            on:click=move |_| submit()
                                        >
                                            <MaterialSymbolIcon name="check" filled=false />
                                        </button>
                                    </div>
                                </div>
                            </div>
                        </>
                    }.into_any()
                } else {
                    ().into_any()
                }}
                <div class="gallery-pagination-cluster">
                    <button
                        class="button ghost icon-button pagination-icon-button"
                        title="上一页"
                        disabled=move || !can_prev.get()
                        on:click=move |_| page.update(|value| *value = value.saturating_sub(1).max(1))
                    >
                        <MaterialSymbolIcon name="chevron_left" filled=false />
                    </button>
                    <button
                        class="button ghost pagination-page-button"
                        title="跳转页码"
                        on:click=move |_| {
                            if show_picker.get_untracked() {
                                show_picker.set(false);
                            } else {
                                candidate.set(page.get_untracked().max(1));
                                show_picker.set(true);
                            }
                        }
                    >
                        {move || page_label.get()}
                    </button>
                    <button
                        class="button ghost icon-button pagination-icon-button"
                        title="下一页"
                        disabled=move || !can_next.get()
                        on:click=move |_| {
                            let total = page_count.get_untracked().max(1);
                            page.update(|value| *value = (*value + 1).min(total));
                        }
                    >
                        <MaterialSymbolIcon name="chevron_right" filled=false />
                    </button>
                </div>
            </div>
        </div>
    }
}
