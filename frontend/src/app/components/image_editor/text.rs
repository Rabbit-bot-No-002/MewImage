use crate::image_editor::Point;
use leptos::{html, prelude::*};

#[derive(Clone)]
pub(super) struct TextEdit {
    pub id: Option<String>,
    pub position: Point,
    pub value: String,
}

fn confirms_text(key: &str, shift: bool, composing: bool) -> bool {
    key == "Enter" && !shift && !composing
}

#[component]
pub(super) fn TextEditor(
    pending: RwSignal<Option<TextEdit>>,
    on_save: Callback<TextEdit>,
) -> impl IntoView {
    let textarea = NodeRef::<html::Textarea>::new();
    let text = RwSignal::new(String::new());
    Effect::new(move |_| {
        if let Some(edit) = pending.get() {
            text.set(edit.value);
            if let Some(textarea) = textarea.get() {
                let _ = textarea.focus();
            }
        }
    });
    let submit = move || {
        let value = text.get_untracked();
        if value.is_empty() || value.chars().count() > 2048 {
            return;
        }
        if let Some(mut edit) = pending.get_untracked() {
            edit.value = value;
            on_save.run(edit);
        }
    };
    view! {
        <Show when=move || pending.with(Option::is_some)>
            <div class="image-editor-text-backdrop" on:click=move |_| pending.set(None)>
                <section class="image-editor-text-dialog stack" role="dialog" aria-modal="true" aria-label="编辑标记文字"
                    on:click=move |event| event.stop_propagation()
                    on:keydown=move |event: web_sys::KeyboardEvent| {
                        event.stop_propagation();
                        if event.is_composing() { return; }
                        if event.key() == "Escape" { event.prevent_default(); pending.set(None); }
                        else if confirms_text(&event.key(), event.shift_key(), event.is_composing()) { event.prevent_default(); submit(); }
                    }>
                    <h3>"标记文字"</h3>
                    <textarea node_ref=textarea rows="5" prop:value=text aria-label="标记内容"
                        on:input=move |event| text.set(event_target_value(&event)) />
                    <span>"Enter 确认，Shift+Enter 换行；最多 2048 个字符。"</span>
                    <div class="row">
                        <button class="button ghost" on:click=move |_| pending.set(None)>"取消"</button>
                        <button class="button" disabled=move || text.with(|text| text.is_empty() || text.chars().count() > 2048)
                            on:click=move |_| submit()>"确认文字"</button>
                    </div>
                </section>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_does_not_submit_during_composition_or_with_shift() {
        assert!(confirms_text("Enter", false, false));
        assert!(!confirms_text("Enter", false, true));
        assert!(!confirms_text("Enter", true, false));
        assert!(!confirms_text("Process", false, true));
        assert!(!confirms_text(" ", false, false));
    }
}
