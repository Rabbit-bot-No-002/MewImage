use leptos::{html, prelude::*};
use wasm_bindgen::JsCast;
use web_sys::{ClipboardEvent, DragEvent, Event, FileList, HtmlInputElement};

#[component]
pub(crate) fn AssetDropZone(
    label: &'static str,
    on_files: impl Fn(FileList) + Copy + 'static,
) -> impl IntoView {
    let input_ref = NodeRef::<html::Input>::new();
    let trigger = move |_| {
        if let Some(input) = input_ref.get() {
            input.click();
        }
    };
    let handle_drop = move |event: DragEvent| {
        event.prevent_default();
        if let Some(files) = event.data_transfer().and_then(|transfer| transfer.files()) {
            on_files(files);
        }
    };
    let handle_paste = move |event: ClipboardEvent| {
        if let Some(files) = event.clipboard_data().and_then(|transfer| transfer.files()) {
            on_files(files);
        }
    };
    view! {
        <div
            class="dropzone"
            tabindex="0"
            on:click=trigger
            on:dragover=move |event: DragEvent| event.prevent_default()
            on:drop=handle_drop
            on:paste=handle_paste
        >
            <input
                node_ref=input_ref
                style="display:none"
                type="file"
                multiple
                accept="image/*"
                on:change=move |event: Event| {
                    let input: HtmlInputElement = event.target().unwrap().unchecked_into();
                    if let Some(files) = input.files() {
                        on_files(files);
                    }
                }
            />
            <strong>{label}</strong>
            <div class="muted">"支持拖拽、点击选择和 Ctrl/Cmd + V 粘贴"</div>
        </div>
    }
}
