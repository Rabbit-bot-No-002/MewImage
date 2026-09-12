use crate::app::state::UiState;
use leptos::{html, portal::Portal, prelude::*};
use mew_image_shared::MAX_GENERATION_REFERENCE_IMAGES;
use std::{collections::HashSet, sync::Arc};

pub(crate) fn choose_task_references(
    ui: UiState,
    assets: RwSignal<Vec<mew_image_shared::ImageAssetRef>>,
    task: mew_image_shared::LocalTaskRecord,
    continuation: Option<String>,
    accept: impl Fn(mew_image_shared::LocalTaskRecord) + Send + Sync + 'static,
) {
    let workspace = expect_context::<crate::app::state::WorkspaceState>();
    let composer = expect_context::<crate::app::state::ComposerState>();
    if let Some(editing) = &task.editing
        && continuation
            .as_ref()
            .is_some_and(|id| id != &editing.base_asset_id)
    {
        composer
            .status_text
            .set("更换编辑底图需要重新建立遮罩，不能沿用原图的选区。".into());
        return;
    }
    let original_thread = workspace.current_thread_id.get_untracked();
    let choices = continuation
        .iter()
        .chain(task.reference_asset_ids.iter())
        .map(|id| ReferenceChoice {
            id: id.clone(),
            preview: assets.with_untracked(|assets| {
                assets
                    .iter()
                    .find(|asset| asset.id == *id)
                    .map(crate::app::asset_display_src)
                    .unwrap_or_default()
            }),
            required: continuation.as_ref() == Some(id)
                || task
                    .editing
                    .as_ref()
                    .is_some_and(|editing| &editing.base_asset_id == id),
        })
        .collect();
    choose_references(ui, choices, move |ids| {
        if workspace.current_thread_id.get_untracked() != original_thread
            || composer
                .foreground_generation_task_id
                .get_untracked()
                .is_some()
        {
            composer
                .status_text
                .set("会话已切换或前台正在生成，请稍后重新复用。".into());
            return;
        }
        let mut selected = task.clone();
        selected.reference_asset_ids.retain(|id| ids.contains(id));
        if let Some(editing) = &selected.editing
            && let Err(error) = assets.with_untracked(|items| editing.validate_resources(items))
        {
            composer.status_text.set(error);
            return;
        }
        let target_thread = workspace.threads.with_untracked(|threads| {
            crate::app::task_target_thread_id(&selected, threads, &original_thread)
        });
        composer.editing_by_thread.update(|items| {
            if let Some(editing) = selected.editing.clone() {
                items.insert(target_thread, editing);
            } else {
                items.remove(&target_thread);
            }
        });
        accept(selected);
    });
}

#[derive(Clone)]
pub(crate) struct ReferenceChoice {
    pub id: String,
    pub preview: String,
    pub required: bool,
}

#[derive(Clone)]
pub(crate) struct ReferenceSelection {
    pub choices: Vec<ReferenceChoice>,
    pub accept: Arc<dyn Fn(Vec<String>) + Send + Sync>,
}

/// 超额时只保存待应用的输入；确认之前不能修改会话或读取原图。
pub(crate) fn choose_references(
    ui: UiState,
    choices: Vec<ReferenceChoice>,
    accept: impl Fn(Vec<String>) + Send + Sync + 'static,
) {
    let mut seen = HashSet::new();
    let choices: Vec<_> = choices
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect();
    if choices.len() <= MAX_GENERATION_REFERENCE_IMAGES {
        accept(choices.into_iter().map(|item| item.id).collect());
        return;
    }
    ui.reference_selection.set(Some(ReferenceSelection {
        choices,
        accept: Arc::new(accept),
    }));
}

#[component]
pub(crate) fn ReferenceSelectionOverlay() -> impl IntoView {
    let pending = expect_context::<UiState>().reference_selection;
    view! {
        <Portal>
        {move || pending.get().map(|request| {
            let selected = RwSignal::new(request.choices.iter().map(|item| item.id.clone()).collect::<Vec<_>>());
            let accept = request.accept.clone();
            let original_choices = StoredValue::new(request.choices.clone());
            let required_ids: Vec<_> = request.choices.iter().filter(|item| item.required).map(|item| item.id.clone()).collect();
            let dialog_ref = NodeRef::<html::Section>::new();
            Effect::new(move |_| {
                if let Some(dialog) = dialog_ref.get() { let _ = dialog.focus(); }
            });
            view! {
                    <div class="reference-selection-backdrop" on:click=move |_| pending.set(None)>
                        <section node_ref=dialog_ref tabindex="-1" class="reference-selection-dialog stack" role="dialog" aria-modal="true" aria-label="精简参考图" on:click=move |event| event.stop_propagation()>
                            <h3>"精简参考图"</h3>
                            <p>"旧图片会完整保留。请选择不超过 10 张后再应用；取消不会改变工作台。"</p>
                            <span>{move || format!("已选 {} / 10", selected.with(Vec::len))}</span>
                            <div class="reference-selection-grid">
                                {request.choices.into_iter().enumerate().map(|(index, choice)| {
                                    let id = choice.id;
                                    let check_id = id.clone();
                                    view! {
                                        <button type="button" class="reference-selection-item" disabled=choice.required class:is-selected=move || selected.with(|items| items.contains(&check_id)) on:click=move |_| selected.update(|items| {
                                            if let Some(index) = items.iter().position(|item| item == &id) { items.remove(index); }
                                            else if items.len() < MAX_GENERATION_REFERENCE_IMAGES { items.push(id.clone()); }
                                        })>
                                            <img src=choice.preview alt=format!("参考图 {}", index + 1) loading="lazy" />
                                            <span>{if choice.required { "编辑底图（固定保留）".into() } else { format!("参考图 {}", index + 1) }}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            <div class="row">
                                <button class="button ghost" on:click=move |_| selected.set(required_ids.clone())>"清空可选项"</button>
                                <button class="button ghost" on:click=move |_| pending.set(None)>"取消"</button>
                                <button class="button" disabled=move || selected.with(Vec::len) > MAX_GENERATION_REFERENCE_IMAGES on:click=move |_| {
                                    let ids = original_choices.with_value(|choices| confirmed_selection(choices, &selected.get_untracked()));
                                    if let Some(ids) = ids {
                                        pending.set(None);
                                        accept(ids);
                                    }
                                }>"确认并应用"</button>
                            </div>
                        </section>
                    </div>
            }
        })}
        </Portal>
    }
}

fn confirmed_selection(choices: &[ReferenceChoice], selected: &[String]) -> Option<Vec<String>> {
    if choices
        .iter()
        .any(|choice| choice.required && !selected.contains(&choice.id))
    {
        return None;
    }
    let ids: Vec<_> = choices
        .iter()
        .filter(|choice| selected.contains(&choice.id))
        .map(|choice| choice.id.clone())
        .collect();
    (ids.len() <= MAX_GENERATION_REFERENCE_IMAGES).then_some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_keeps_original_order_and_requires_continuation_base() {
        let choices: Vec<_> = (0..16)
            .map(|index| ReferenceChoice {
                id: index.to_string(),
                preview: String::new(),
                required: index == 0,
            })
            .collect();
        let all: Vec<_> = choices.iter().map(|choice| choice.id.clone()).collect();
        assert!(confirmed_selection(&choices, &all).is_none());
        assert!(confirmed_selection(&choices, &["1".into()]).is_none());
        assert_eq!(
            confirmed_selection(&choices, &["2".into(), "0".into()]),
            Some(vec!["0".into(), "2".into()])
        );
        assert_eq!(confirmed_selection(&choices, &all[..10]).unwrap().len(), 10);
        assert_eq!(choices.len(), 16);
    }
}
