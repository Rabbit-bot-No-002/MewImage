use leptos::prelude::*;
use mew_image_shared::{DEFAULT_FAVORITE_FOLDER_ID, FavoriteFolder};
use web_sys::{KeyboardEvent, MouseEvent};

use crate::app::{
    confirm_popover_style,
    derived::AppDerived,
    favorite_folder_picker_style,
    models::ConfirmPopoverKind,
    state::{UiState, WorkspaceState},
};

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn GlobalPopovers(
    submit_text_popover: impl Fn() + Copy + Send + Sync + 'static,
    submit_confirm_popover: impl Fn() + Copy + Send + Sync + 'static,
    assign_favorite_folder: impl Fn(String, String) + Copy + Send + Sync + 'static,
    cancel_favorite_for_task: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let tasks = workspace.tasks;
    let text_popover = ui.text_popover;
    let text_popover_value = ui.text_popover_value;
    let confirm_popover = ui.confirm_popover;
    let favorite_folder_picker = ui.favorite_folder_picker;
    let favorite_folders = derived.favorite_folders;

    view! {
            {move || text_popover.get().map(|state| {
                let style = format!("left: {}px; top: {}px;", state.x + 8.0, state.y + 8.0);
                view! {
                    <>
                        <button class="inline-popover-dismiss" aria-label="关闭输入弹窗" on:click=move |_| text_popover.set(None)></button>
                        <div class="inline-action-popover" style=style on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <strong>{state.title}</strong>
                            <input
                                class="text-input inline-popover-input"
                                prop:value=move || text_popover_value.get()
                                on:input=move |ev| text_popover_value.set(event_target_value(&ev))
                                on:keydown=move |ev: KeyboardEvent| {
                                    match ev.key().as_str() {
                                        "Enter" => {
                                            ev.prevent_default();
                                            submit_text_popover();
                                        }
                                        "Escape" => text_popover.set(None),
                                        _ => {}
                                    }
                                }
                            />
                            <div class="row inline-popover-actions">
                                <button class="button ghost" on:click=move |_| text_popover.set(None)>"取消"</button>
                                <button class="button secondary" on:click=move |_| submit_text_popover()>"确定"</button>
                            </div>
                        </div>
                    </>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || confirm_popover.get().map(|state| {
                let style = confirm_popover_style(state.x, state.y);
                let confirm_label = if matches!(state.kind, ConfirmPopoverKind::CancelGeneration) {
                    "确认停止"
                } else {
                    "确认删除"
                };
                view! {
                    <>
                        <button class="inline-popover-dismiss" aria-label="关闭确认弹窗" on:click=move |_| confirm_popover.set(None)></button>
                        <div class="inline-action-popover confirm-popover" style=style on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <strong>{state.title}</strong>
                            <p class="muted">{state.message}</p>
                            <div class="row inline-popover-actions">
                                <button class="button ghost" on:click=move |_| confirm_popover.set(None)>"取消"</button>
                                <button class="button danger" on:click=move |_| submit_confirm_popover()>{confirm_label}</button>
                            </div>
                        </div>
                    </>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || favorite_folder_picker.get().map(|picker| {
                let style = favorite_folder_picker_style(picker.x, picker.y);
                let current_folder_id = tasks.with_untracked(|items| {
                    items
                        .iter()
                        .find(|task| task.id == picker.task_id)
                        .and_then(|task| task.favorite_folder_id.clone())
                        .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into())
                });
                let folder_options: Vec<FavoriteFolder> = favorite_folders
                    .get()
                    .into_iter()
                    .filter(|folder| !picker.is_favorite || folder.id != current_folder_id)
                    .collect();
                view! {
                    <>
                        <button class="folder-picker-dismiss" aria-label="关闭收藏文件夹选择" on:click=move |_| favorite_folder_picker.set(None)></button>
                        <div class="folder-picker-popover" style=style>
                            <strong>{if picker.is_favorite { "移动到" } else { "收藏到" }}</strong>
                            {folder_options
                                .into_iter()
                                .map(|folder| {
                                    let task_id = picker.task_id.clone();
                                    let folder_id = folder.id.clone();
                                    view! {
                                        <button class="folder-picker-item" on:click=move |_| assign_favorite_folder(task_id.clone(), folder_id.clone())>
                                            <MaterialSymbolIcon name="folder" filled=false />
                                            <span>{folder.name}</span>
                                        </button>
                                    }.into_any()
                                })
                                .collect::<Vec<_>>()}
                            {if picker.is_favorite {
                                let cancel_task_id = picker.task_id.clone();
                                view! {
                                    <button class="folder-picker-item folder-picker-cancel" on:click=move |_| cancel_favorite_for_task(cancel_task_id.clone())>
                                        <MaterialSymbolIcon name="star" filled=false />
                                        <span>"取消收藏"</span>
                                    </button>
                                }.into_any()
                            } else {
                                ().into_any()
                            }}
                        </div>
                    </>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

    }
}
