use leptos::{html, portal::Portal, prelude::*};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Blob, HtmlInputElement};

mod apply;
mod text;

use crate::{
    app::state::{ComposerState, UiState, WorkspaceState},
    image_editor::{
        DrawTool, DrawingObject, EditorDraft, EditorSession, Geometry, ImageEditorCanvas,
        load_draft, save_draft,
    },
};

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn ImageEditorOverlay() -> impl IntoView {
    let pending = expect_context::<UiState>().image_editor_thread;
    view! {
        <Portal>
            {move || pending.get().map(|thread_id| view! { <LoadEditor thread_id /> })}
        </Portal>
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorPurpose {
    Reference,
    Sketch,
}

impl EditorPurpose {
    fn from_requested_base(requested_base: Option<&str>) -> Self {
        if requested_base.is_some() {
            Self::Reference
        } else {
            Self::Sketch
        }
    }
}

#[component]
fn LoadEditor(thread_id: String) -> impl IntoView {
    let ui = expect_context::<UiState>();
    let composer = expect_context::<ComposerState>();
    let workspace = expect_context::<WorkspaceState>();
    let dimensions = if composer.resolution_mode.get_untracked() == "model_auto" {
        (1024, 1024)
    } else {
        let reference_size = workspace.assets.with_untracked(|assets| {
            composer.selected_reference_ids.with_untracked(|ids| {
                ids.iter().find_map(|id| {
                    assets
                        .iter()
                        .find(|asset| &asset.id == id)
                        .and_then(|asset| asset.width.zip(asset.height))
                })
            })
        });
        crate::app::utils::resolution::resolve_dimensions_from_reference_size(
            &composer.resolution_mode.get_untracked(),
            &composer.resolution_group.get_untracked(),
            &composer.aspect_ratio.get_untracked(),
            &composer.effective_custom_aspect_ratio.get_untracked(),
            composer.custom_width.get_untracked(),
            composer.custom_height.get_untracked(),
            reference_size,
        )
    };
    let loaded = RwSignal::new(None::<EditorDraft>);
    let error = RwSignal::new(None::<String>);
    let replacement = RwSignal::new(None::<EditorDraft>);
    let loading_dialog = NodeRef::<html::Section>::new();
    Effect::new(move |_| {
        if let Some(dialog) = loading_dialog.get() {
            let _ = dialog.focus();
        }
    });
    let requested_base = ui.image_editor_base_id.get_untracked();
    let apply_as_continuation = requested_base.is_some()
        && requested_base == composer.continuation_asset_id.get_untracked();
    // 入口用途必须在异步读取草稿前固定，避免旧草稿的最后模式改变本次入口语义。
    let purpose = EditorPurpose::from_requested_base(requested_base.as_deref());
    spawn_local(async move {
        let result = match load_draft(&thread_id).await {
            Ok(previous) => workspace.assets.with_untracked(|assets| {
                prepare_editor_draft(
                    &thread_id,
                    requested_base.as_deref(),
                    previous,
                    assets,
                    dimensions,
                    purpose,
                )
            }),
            Err(error) => Err(error),
        };
        if ui.image_editor_thread.get_untracked().as_ref() != Some(&thread_id) {
            return;
        }
        match result {
            Ok((draft, requires_confirmation)) => {
                if requires_confirmation {
                    replacement.try_set(Some(draft));
                } else {
                    loaded.try_set(Some(draft));
                }
            }
            Err(message) => {
                error.try_set(Some(message));
            }
        }
    });
    view! {
        <div class="image-editor-backdrop">
            {move || loaded.get().map(|draft| view! {
                <EditorDialog initial=draft purpose apply_as_continuation />
            })}
            <Show when=move || loaded.with(Option::is_none)>
                <section node_ref=loading_dialog tabindex="-1" class="image-editor-dialog image-editor-loading-dialog stack" role="dialog" aria-modal="true" aria-label="加载编辑草稿"
                    on:keydown=move |event: web_sys::KeyboardEvent| {
                        if event.key() == "Escape" {
                            event.prevent_default(); event.stop_propagation(); ui.image_editor_thread.set(None);
                        }
                    }>
                    <p>{move || error.get().unwrap_or_else(|| if replacement.with(Option::is_some) {
                        "请确认新的编辑工作副本。".into()
                    } else { "正在读取本地编辑草稿…".into() })}</p>
                    <p>"草稿读取失败时不会覆盖旧记录。"</p>
                    <Show when=move || replacement.with(Option::is_some)>
                        <p>"更换底图将新建编辑草稿，原草稿的操作图层不会套用到新图。是否继续？"</p>
                        <p>{move || replacement.with(|draft| draft.as_ref().and_then(|draft| {
                            draft.original_base_dimensions.map(|(width, height)| format!(
                                "原图 {width} × {height} 超出编辑工作尺寸。确认后创建 {} × {} 的等比 PNG 工作副本；原图保持不变。",
                                draft.width, draft.height
                            ))
                        }).unwrap_or_default())}</p>
                        <button class="button danger" on:click=move |_| {
                            loaded.set(replacement.get_untracked()); replacement.set(None);
                        }>"确认创建工作副本"</button>
                    </Show>
            <button class="button ghost" on:click=move |_| ui.image_editor_thread.set(None)>"关闭"</button>
                </section>
            </Show>
        </div>
    }
}

fn prepare_editor_draft(
    thread_id: &str,
    requested_base: Option<&str>,
    previous: Option<EditorDraft>,
    assets: &[mew_image_shared::ImageAssetRef],
    dimensions: (u32, u32),
    purpose: EditorPurpose,
) -> Result<(EditorDraft, bool), String> {
    let Some(base_id) = requested_base else {
        let mut draft = previous.map(Ok).unwrap_or_else(|| {
            EditorDraft::new(thread_id.into(), None, dimensions.0, dimensions.1)
        })?;
        draft.mode = crate::image_editor::EditMode::Sketch;
        draft.validate()?;
        return Ok((draft, false));
    };
    if let Some(draft) = previous
        .as_ref()
        .filter(|draft| draft.base_asset_id.as_deref() == Some(base_id))
    {
        let mut draft = draft.clone();
        if purpose == EditorPurpose::Reference
            && draft.mode == crate::image_editor::EditMode::Sketch
        {
            draft.mode = crate::image_editor::EditMode::Annotation;
        }
        draft.validate()?;
        return Ok((draft, false));
    }
    let asset = assets
        .iter()
        .find(|asset| asset.id == base_id && !mew_image_shared::is_edit_mask(asset))
        .ok_or("找不到可编辑底图，原草稿未改变。")?;
    let (width, height) = asset.width.zip(asset.height).ok_or("底图缺少尺寸信息。")?;
    let (work_width, work_height) = crate::image_editor::fitted_work_dimensions(width, height)?;
    let resized = (width, height) != (work_width, work_height);
    let mut draft = EditorDraft::new(
        thread_id.into(),
        Some(base_id.into()),
        work_width,
        work_height,
    )?;
    draft.original_base_dimensions = resized.then_some((width, height));
    draft.mode = crate::image_editor::EditMode::Annotation;
    draft.validate()?;
    Ok((draft, previous.is_some() || resized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_base_requires_confirmation_and_preserves_original_metadata() {
        let asset: mew_image_shared::ImageAssetRef = serde_json::from_value(serde_json::json!({
            "id": "large", "sha256": "hash", "mime_type": "image/jpeg", "byte_len": 200,
            "width": 8192, "height": 6144, "created_at": "now", "updated_at": "now", "metadata": {}
        }))
        .unwrap();
        let original = asset.clone();
        let (draft, confirmation) = prepare_editor_draft(
            "thread",
            Some("large"),
            None,
            std::slice::from_ref(&asset),
            (1024, 1024),
            EditorPurpose::Reference,
        )
        .unwrap();
        assert!(confirmation);
        assert_eq!((draft.width, draft.height), (4096, 3072));
        assert_eq!(draft.original_base_dimensions, Some((8192, 6144)));
        assert_eq!(asset, original);
        let (restored, confirmation) = prepare_editor_draft(
            "thread",
            Some("large"),
            Some(draft.clone()),
            &[asset],
            (1024, 1024),
            EditorPurpose::Reference,
        )
        .unwrap();
        assert!(!confirmation);
        assert_eq!(restored, draft);
    }

    #[test]
    fn changing_base_requires_confirmation_and_does_not_reuse_old_layers() {
        let asset: mew_image_shared::ImageAssetRef = serde_json::from_value(serde_json::json!({
            "id": "new", "sha256": "hash", "mime_type": "image/png", "byte_len": 20,
            "width": 64, "height": 32, "created_at": "now", "updated_at": "now", "metadata": {}
        }))
        .unwrap();
        let previous = EditorDraft::new("thread".into(), Some("old".into()), 16, 16).unwrap();
        let (new, confirm) = prepare_editor_draft(
            "thread",
            Some("new"),
            Some(previous.clone()),
            std::slice::from_ref(&asset),
            (1024, 1024),
            EditorPurpose::Reference,
        )
        .unwrap();
        assert!(confirm);
        assert_eq!((new.width, new.height), (64, 32));
        assert_eq!(new.base_asset_id.as_deref(), Some("new"));
        assert_eq!(new.mode, crate::image_editor::EditMode::Annotation);
        assert!(new.layers.iter().all(|layer| layer.objects.is_empty()));
        let (restored, confirm) = prepare_editor_draft(
            "thread",
            Some("old"),
            Some(previous.clone()),
            &[],
            (64, 64),
            EditorPurpose::Reference,
        )
        .unwrap();
        assert!(!confirm);
        assert_eq!(restored.base_asset_id, previous.base_asset_id);
        assert_eq!(restored.layers, previous.layers);
        assert_eq!(restored.mode, crate::image_editor::EditMode::Annotation);
        assert!(
            prepare_editor_draft(
                "thread",
                Some("missing"),
                None,
                &[asset],
                (64, 64),
                EditorPurpose::Reference,
            )
            .is_err()
        );
    }

    #[test]
    fn editor_entry_purpose_overrides_incompatible_restored_mode() {
        let mut previous = EditorDraft::new("thread".into(), Some("base".into()), 64, 64).unwrap();
        previous.mode = crate::image_editor::EditMode::Mask;

        let (sketch, confirmation) = prepare_editor_draft(
            "thread",
            None,
            Some(previous.clone()),
            &[],
            (1024, 1024),
            EditorPurpose::Sketch,
        )
        .unwrap();
        assert!(!confirmation);
        assert_eq!(sketch.mode, crate::image_editor::EditMode::Sketch);

        previous.mode = crate::image_editor::EditMode::Sketch;
        let (reference, confirmation) = prepare_editor_draft(
            "thread",
            Some("base"),
            Some(previous),
            &[],
            (1024, 1024),
            EditorPurpose::Reference,
        )
        .unwrap();
        assert!(!confirmation);
        assert_eq!(reference.mode, crate::image_editor::EditMode::Annotation);
    }
}

#[component]
fn EditorDialog(
    initial: EditorDraft,
    purpose: EditorPurpose,
    apply_as_continuation: bool,
) -> impl IntoView {
    let ui = expect_context::<UiState>();
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let persistence = expect_context::<crate::app::state::PersistenceState>();
    let initial_text_size =
        crate::image_editor::recommended_text_size(initial.width, initial.height);
    let session =
        StoredValue::new(EditorSession::new(initial.clone()).expect("validated loaded draft"));
    let base_request = initial.base_asset_id.clone().map(|id| {
        (
            id,
            initial.width,
            initial.height,
            initial.original_base_dimensions,
        )
    });
    let draft = RwSignal::new(initial);
    let base = RwSignal::new_local(None::<std::rc::Rc<crate::image_editor::EditorBase>>);
    let imported_mask = RwSignal::new_local(None::<std::rc::Rc<crate::image_editor::EditorMask>>);
    let erase = RwSignal::new(false);
    let tool = RwSignal::new(DrawTool::Pen);
    let selected = RwSignal::new(None::<String>);
    let text_edit = RwSignal::new(None::<text::TextEdit>);
    let color = RwSignal::new("#222222".to_string());
    let brush_width = RwSignal::new(8.0);
    let text_size = RwSignal::new(initial_text_size);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    if let Some((id, width, height, original_dimensions)) = base_request {
        busy.set(true);
        spawn_local(async move {
            let result = async {
                // 原图解码也计入预算；缩小完成后才释放原始像素，不仅按小画布估算。
                let (decode_width, decode_height) = original_dimensions.unwrap_or((width, height));
                let _budget = crate::app::utils::editor_budget::acquire_editor_budget(
                    composer,
                    decode_width,
                    decode_height,
                    || busy.try_get_untracked().is_none(),
                )
                .await?;
                let base =
                    crate::image_editor::EditorBase::load(&id, width, height, original_dimensions)
                        .await?;
                Ok::<_, String>(base)
            }
            .await;
            match result {
                Ok(image) => {
                    base.try_set(Some(std::rc::Rc::new(image)));
                }
                Err(message) => {
                    error.try_set(message);
                }
            }
            busy.try_set(false);
        });
    }
    let applying = RwSignal::new(false);
    let cancel_apply = RwSignal::new(false);
    let drawing = RwSignal::new(false);
    let reset_view = RwSignal::new(0_u64);
    let revision = RwSignal::new(0_u64);
    // 已载入的初始快照视为已保存；后续 revision 才代表尚未落盘的编辑。
    let saved_revision = RwSignal::new(Some(0_u64));
    let can_undo = RwSignal::new(false);
    let can_redo = RwSignal::new(false);
    Effect::new(move |_| {
        let requested = draft.with(|draft| draft.imported_mask_asset_id.clone());
        let loaded = imported_mask.with(|mask| mask.as_ref().map(|mask| mask.id().to_string()));
        if requested == loaded {
            return;
        }
        imported_mask.set(None);
        let Some(mask_id) = requested else { return };
        let (width, height) = draft.with_untracked(|draft| (draft.width, draft.height));
        spawn_local(async move {
            let result = async {
                let _budget = crate::app::utils::editor_budget::acquire_editor_budget(
                    composer,
                    width,
                    height,
                    || {
                        draft.try_with_untracked(|draft| {
                            draft.imported_mask_asset_id.as_deref() != Some(&mask_id)
                        }) != Some(false)
                    },
                )
                .await?;
                crate::image_editor::EditorMask::load(mask_id.clone(), width, height).await
            }
            .await;
            if draft.try_with_untracked(|draft| {
                draft.imported_mask_asset_id.as_deref() == Some(&mask_id)
            }) != Some(true)
            {
                return;
            }
            match result {
                Ok(mask) => {
                    imported_mask.try_set(Some(std::rc::Rc::new(mask)));
                }
                Err(message) => {
                    error.try_set(format!("导入遮罩恢复失败：{message}"));
                }
            }
        });
    });
    let confirm_clear = RwSignal::new(false);
    let confirm_close = RwSignal::new(false);
    let dialog = NodeRef::<html::Section>::new();
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            let _ = dialog.focus();
        }
    });
    let refresh = move || {
        session.with_value(|session| {
            draft.set(session.draft().clone());
            can_undo.set(session.can_undo());
            can_redo.set(session.can_redo());
        });
        revision.update(|revision| *revision = revision.saturating_add(1));
    };
    let import_mask = move |event: web_sys::Event| {
        let input = event_target::<HtmlInputElement>(&event);
        let file = input.files().and_then(|files| files.get(0));
        input.set_value("");
        let Some(file) = file else { return };
        if busy.get_untracked()
            || draft.with_untracked(|draft| draft.mode != crate::image_editor::EditMode::Mask)
        {
            return;
        }
        let blob = file.unchecked_ref::<Blob>().clone();
        let (width, height) = draft.with_untracked(|draft| (draft.width, draft.height));
        busy.set(true);
        error.set("正在校验遮罩 PNG…".into());
        spawn_local(async move {
            let id = format!("editor-mask-{}", uuid::Uuid::new_v4());
            let result = async {
                let _budget = crate::app::utils::editor_budget::acquire_editor_budget(
                    composer,
                    width,
                    height,
                    || busy.try_get_untracked().is_none(),
                )
                .await?;
                let loaded =
                    crate::image_editor::EditorMask::from_blob(id.clone(), &blob, width, height)
                        .await?;
                crate::storage::store_editor_draft_blob(&id, &blob).await?;
                let changed = match session
                    .try_update_value(|session| session.replace_imported_mask(id.clone()))
                {
                    Some(Ok(changed)) => changed,
                    Some(Err(message)) => {
                        crate::storage::delete_editor_draft_blobs(std::slice::from_ref(&id))
                            .await?;
                        return Err(message);
                    }
                    None => {
                        crate::storage::delete_editor_draft_blobs(std::slice::from_ref(&id))
                            .await?;
                        return Err("编辑器已关闭，遮罩未应用。".into());
                    }
                };
                if !changed {
                    crate::storage::delete_editor_draft_blobs(std::slice::from_ref(&id)).await?;
                    return Ok(None);
                }
                Ok::<_, String>(Some(loaded))
            }
            .await;
            match result {
                Ok(Some(loaded)) => {
                    imported_mask.try_set(Some(std::rc::Rc::new(loaded)));
                    if let Some(snapshot) =
                        session.try_with_value(|session| session.draft().clone())
                    {
                        draft.try_set(snapshot);
                        revision.try_update(|revision| *revision = revision.saturating_add(1));
                    }
                    error.try_set("遮罩已导入，可继续使用画笔或橡皮调整。".into());
                }
                Ok(None) => {}
                Err(message) => {
                    error.try_set(message);
                }
            }
            busy.try_set(false);
        });
    };
    let save = move |close: bool| {
        if draft.with_untracked(|draft| draft.base_asset_id.is_some())
            && base.with_untracked(Option::is_none)
        {
            // 尚未成功解码/缩小的替换草稿不得覆盖旧记录；关闭仍可取消等待预算。
            if close {
                ui.image_editor_thread.set(None);
            }
            return;
        }
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let snapshot = draft.get_untracked();
        let stale_masks = if close {
            session.with_value(|session| {
                session
                    .referenced_asset_ids()
                    .into_iter()
                    .filter(|id| snapshot.imported_mask_asset_id.as_ref() != Some(id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
        } else {
            Vec::new()
        };
        let saving_revision = revision.get_untracked();
        spawn_local(async move {
            let result = async {
                save_draft(&snapshot).await?;
                if close {
                    crate::storage::delete_editor_draft_blobs(&stale_masks).await?;
                }
                Ok::<_, String>(())
            }
            .await;
            if result.is_ok() {
                saved_revision.try_set(Some(saving_revision));
            }
            if busy.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(()) if close => ui.image_editor_thread.set(None),
                Ok(()) => error.set("草稿已保存到当前浏览器。".into()),
                Err(message) => error.set(message),
            }
        });
    };
    // 笔画结束后合并连续操作；保存和关闭共用 busy 门，旧写入不会覆盖新写入。
    Effect::new(move |_| {
        let expected_revision = revision.get();
        if drawing.get()
            || confirm_close.get()
            || (draft.with_untracked(|draft| draft.base_asset_id.is_some())
                && base.with(Option::is_none))
        {
            return;
        }
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(600).await;
            if revision.try_get_untracked() != Some(expected_revision)
                || saved_revision.try_get_untracked() == Some(Some(expected_revision))
                || busy.try_get_untracked() != Some(false)
                || drawing.try_get_untracked() != Some(false)
                || confirm_close.try_get_untracked() != Some(false)
            {
                return;
            }
            save(false);
        });
    });
    let apply = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        applying.set(true);
        cancel_apply.set(false);
        error.set("等待编辑编码预算…".into());
        let snapshot = draft.get_untracked();
        spawn_local(async move {
            match apply::apply_edit(
                snapshot,
                apply::EditorInputs {
                    base: base.get_untracked(),
                    imported_mask: imported_mask.get_untracked(),
                    apply_as_continuation,
                },
                workspace,
                composer,
                ui,
                persistence,
                cancel_apply,
            )
            .await
            {
                Ok(()) => ui.image_editor_thread.set(None),
                Err(message) => {
                    error.try_set(message);
                }
            }
            busy.try_set(false);
            applying.try_set(false);
        });
    };
    let request_close = move || {
        if busy.get_untracked() || applying.get_untracked() {
            return;
        }
        let current_revision = revision.get_untracked();
        if saved_revision.get_untracked() != Some(current_revision) {
            confirm_close.set(true);
        } else {
            save(true);
        }
    };
    let discard_and_close = move || {
        if busy.get_untracked() {
            return;
        }
        let thread_id = draft.with_untracked(|draft| draft.thread_id.clone());
        let candidates = session.with_value(|session| {
            session
                .referenced_asset_ids()
                .into_iter()
                .filter(|id| id.starts_with("editor-mask-"))
                .cloned()
                .collect::<Vec<_>>()
        });
        confirm_close.set(false);
        busy.set(true);
        spawn_local(async move {
            let result = async {
                let protected = load_draft(&thread_id)
                    .await?
                    .into_iter()
                    .flat_map(|draft| draft.input_asset_ids().cloned().collect::<Vec<_>>())
                    .collect::<std::collections::HashSet<_>>();
                let orphaned = candidates
                    .into_iter()
                    .filter(|id| !protected.contains(id))
                    .collect::<Vec<_>>();
                crate::storage::delete_editor_draft_blobs(&orphaned).await
            }
            .await;
            match result {
                Ok(()) => ui.image_editor_thread.set(None),
                Err(message) => {
                    error.try_set(format!("未保存资源清理失败：{message}"));
                }
            }
            busy.try_set(false);
        });
    };
    view! {
        <section node_ref=dialog tabindex="-1" class="image-editor-dialog stack" role="dialog" aria-modal="true" aria-label="图像编辑器"
            on:keydown=move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    event.prevent_default(); event.stop_propagation();
                    if applying.get_untracked() { cancel_apply.set(true); }
                    else if text_edit.with_untracked(Option::is_some) { text_edit.set(None); }
                    else if confirm_clear.get_untracked() { confirm_clear.set(false); }
                    else if confirm_close.get_untracked() { confirm_close.set(false); }
                    else if selected.with_untracked(Option::is_some) { selected.set(None); }
                    else { request_close(); }
                    return;
                }
                if busy.get_untracked() || drawing.get_untracked() || text_edit.with_untracked(Option::is_some) {
                    return;
                }
                let modifier = event.ctrl_key() || event.meta_key();
                let key = event.key().to_ascii_lowercase();
                if modifier && key == "z" {
                    event.prevent_default(); event.stop_propagation();
                    session.update_value(|session| {
                        if event.shift_key() { session.redo(); } else { session.undo(); }
                    });
                    refresh();
                } else if modifier && key == "y" {
                    event.prevent_default(); event.stop_propagation();
                    session.update_value(|session| { session.redo(); });
                    refresh();
                }
            }>
            <header class="row"><h2>{if purpose == EditorPurpose::Sketch { "绘制草图" } else { "编辑参考图" }}</h2><button class="button ghost" disabled=applying on:click=move |_| save(true)>"保存并关闭"</button></header>
            <fieldset disabled=busy class="image-editor-controls row">
                {match purpose {
                    EditorPurpose::Reference => vec![(crate::image_editor::EditMode::Mask, "局部修改"), (crate::image_editor::EditMode::Annotation, "标记")],
                    EditorPurpose::Sketch => vec![(crate::image_editor::EditMode::Sketch, "草图")],
                }.into_iter().map(|(mode, label)| view! {
                    <button class="button ghost" aria-pressed=move || draft.with(|draft| draft.mode == mode)
                        class:image-editor-active=move || draft.with(|draft| draft.mode == mode)
                        disabled=move || drawing.get() || (mode != crate::image_editor::EditMode::Sketch && base.with(Option::is_none))
                        on:click=move |_| {
                            let changed = draft.with_untracked(|draft| draft.mode != mode);
                            if changed {
                                session.update_value(|session| session.set_mode(mode));
                                refresh();
                            }
                            tool.set(DrawTool::Pen); erase.set(false); selected.set(None); text_edit.set(None);
                        }>{label}</button>
                }).collect_view()}
                <Show when=move || draft.with(|draft| draft.mode != crate::image_editor::EditMode::Mask)>
                    <div class="image-editor-tool-group" role="toolbar" aria-label="标记工具">
                        {[(DrawTool::Arrow, "arrow_outward", "箭头"), (DrawTool::Rectangle, "rectangle", "矩形"), (DrawTool::Ellipse, "circle", "椭圆"), (DrawTool::Text, "title", "文字"), (DrawTool::Select, "pan_tool_alt", "选择或移动")].into_iter().map(|(choice, icon, label)| view! {
                            <button class="button ghost image-editor-tool-button" disabled=drawing aria-label=label
                                aria-pressed=move || tool.get() == choice && !erase.get()
                                class:image-editor-active=move || tool.get() == choice && !erase.get()
                                title=move || format!("{label}；再次点击可返回自由画笔")
                                on:click=move |_| {
                                    let next = if tool.get_untracked() == choice && !erase.get_untracked() { DrawTool::Pen } else { choice };
                                    tool.set(next); erase.set(false);
                                }><MaterialSymbolIcon name=icon filled=false /></button>
                        }).collect_view()}
                        <button class="button ghost image-editor-tool-button" disabled=drawing aria-pressed=erase
                            class:image-editor-active=erase
                            aria-label="橡皮" title="橡皮；再次点击可返回自由画笔"
                            on:click=move |_| {
                                erase.update(|active| *active = !*active);
                                tool.set(DrawTool::Pen);
                            }><MaterialSymbolIcon name="ink_eraser" filled=false /></button>
                    </div>
                </Show>
                <Show when=move || draft.with(|draft| draft.mode == crate::image_editor::EditMode::Mask)>
                    <button class="button ghost image-editor-tool-button" disabled=drawing aria-pressed=erase
                        class:image-editor-active=erase
                        aria-label="橡皮" title="橡皮；再次点击可返回自由画笔"
                        on:click=move |_| {
                            erase.update(|active| *active = !*active);
                            tool.set(DrawTool::Pen);
                        }><MaterialSymbolIcon name="ink_eraser" filled=false /></button>
                </Show>
                <button class="button ghost image-editor-tool-button" disabled=drawing aria-label="适应窗口" title="适应窗口"
                    on:click=move |_| reset_view.update(|value| *value = value.wrapping_add(1))><MaterialSymbolIcon name="fit_screen" filled=false /></button>
                <label>"颜色 "<input type="color" prop:value=color on:input=move |event| color.set(event_target_value(&event)) /></label>
                <label>"画布背景 "<input type="color" disabled=drawing
                    prop:value=move || draft.with(|draft| draft.background.clone())
                    on:change=move |event| {
                        let mut changed = false;
                        session.update_value(|session| match session.set_background(event_target_value(&event)) {
                            Ok(value) => changed = value, Err(message) => error.set(message),
                        });
                        if changed { refresh(); }
                    } /></label>
                <Show when=move || tool.get() == DrawTool::Text && !erase.get()>
                    <label class="image-editor-value-control">"字号 "<input type="range" min="16" max="256" step="1" prop:value=text_size on:input=move |event| {
                        if let Ok(value) = event_target_value(&event).parse::<f64>() { text_size.set(value); }
                    } /><output>{move || format!("{:.0}px", text_size.get())}</output></label>
                </Show>
                <Show when=move || tool.get() != DrawTool::Text || erase.get()>
                    <label class="image-editor-value-control">"笔刷 "<input type="range" min="1" max="128" prop:value=brush_width on:input=move |event| {
                        if let Ok(value) = event_target_value(&event).parse::<f64>() { brush_width.set(value); }
                    } /><output>{move || format!("{:.0}px", brush_width.get())}</output></label>
                </Show>
                <button class="button ghost" disabled=move || !can_undo.get() on:click=move |_| { session.update_value(|session| { session.undo(); }); refresh(); }>"撤销"</button>
                <button class="button ghost" disabled=move || !can_redo.get() on:click=move |_| { session.update_value(|session| { session.redo(); }); refresh(); }>"重做"</button>
                <button class="button ghost danger" on:click=move |_| confirm_clear.set(true)>"清空"</button>
            </fieldset>
            <Show when=move || confirm_clear.get()>
                <div class="row"><span>"清空当前图层？其他模式的草稿会保留。"</span>
                    <button class="button ghost" on:click=move |_| confirm_clear.set(false)>"取消"</button>
                    <button class="button danger" disabled=busy on:click=move |_| {
                        session.update_value(|session| { session.clear_active_layer(); });
                        imported_mask.set(None); refresh(); confirm_clear.set(false);
                    }>"确认清空"</button>
                </div>
            </Show>
            <div class="image-editor-stage" style:pointer-events=move || if busy.get() { "none" } else { "auto" }>
                <Show when=move || draft.with(|draft| draft.mode == crate::image_editor::EditMode::Mask)>
                    <div class="image-editor-mask-tools">
                        <span class="image-editor-mask-hint">"蓝色区域为待修改选区；模型仍可能影响选区外内容。"</span>
                        <label class="button ghost compact-toggle"><MaterialSymbolIcon name="upload_file" filled=false />"导入 Alpha PNG"
                            <input class="visually-hidden" type="file" accept="image/png" on:change=import_mask />
                        </label>
                    </div>
                </Show>
                <Show when=move || selected.with(Option::is_some)>
                    <div class="image-editor-selection-tools" role="toolbar" aria-label="已选中对象操作">
                        <span>"已选中 · 可直接拖动"</span>
                        <button class="button ghost image-editor-tool-button" disabled=busy aria-label="修改文字" title="修改文字" on:click=move |_| {
                            if let Some(id) = selected.get_untracked() {
                                session.with_value(|session| {
                                    if let Some(object) = session.draft().active_objects().iter().find(|object| object.id == id)
                                        && let Geometry::Text { position, text } = &object.geometry {
                                        text_edit.set(Some(text::TextEdit { id: Some(id), position: *position, value: text.clone() }));
                                    } else { error.set("请先选择一个文字对象。".into()); }
                                });
                            }
                        }><MaterialSymbolIcon name="edit" filled=false /></button>
                        <button class="button ghost danger image-editor-tool-button" disabled=busy aria-label="删除对象" title="删除对象" on:click=move |_| {
                            if let Some(id) = selected.get_untracked() {
                                session.update_value(|session| { session.remove(&id); }); selected.set(None); refresh();
                            }
                        }><MaterialSymbolIcon name="delete" filled=false /></button>
                    </div>
                </Show>
                <ImageEditorCanvas draft erase color brush_width reset_view tool selected base imported_mask
                    on_text=Callback::new(move |position| text_edit.set(Some(text::TextEdit { id: None, position, value: String::new() })))
                    on_move=Callback::new(move |(id, delta): (String, crate::image_editor::Point)| {
                        if busy.get_untracked() { return; }
                        session.update_value(|session| { if let Err(message) = session.move_object(&id, delta) { error.set(message); } }); refresh();
                    })
                    on_drawing=Callback::new(move |value| drawing.set(value))
                    on_object=Callback::new(move |object| {
                        if busy.get_untracked() { return; }
                        session.update_value(|session| { if let Err(message) = session.put(object) { error.set(message); } }); refresh();
                    }) on_error=Callback::new(move |message| error.set(message)) />
            </div>
            <text::TextEditor pending=text_edit on_save=Callback::new(move |edit: text::TextEdit| {
                if busy.get_untracked() { return; }
                let mut result = Ok(());
                session.update_value(|session| {
                    result = if let Some(id) = edit.id { session.edit_text(&id, edit.value) }
                    else { session.put(DrawingObject {
                        id: uuid::Uuid::new_v4().to_string(), geometry: Geometry::Text { position: edit.position, text: edit.value },
                        color: color.get_untracked(), width: text_size.get_untracked(),
                    }) };
                });
                match result { Ok(()) => { text_edit.set(None); refresh(); }, Err(message) => error.set(message) }
            }) />
            <Show when=move || confirm_close.get()>
                <div class="image-editor-confirm-backdrop" role="presentation">
                    <section class="image-editor-confirm-dialog stack" role="alertdialog" aria-modal="true" aria-label="保存编辑确认">
                        <h3>"编辑尚未保存"</h3>
                        <p>"是否保存本次修改后退出？未保存退出会保留上一次已经落盘的草稿。"</p>
                        <div class="row">
                            <button class="button ghost" on:click=move |_| confirm_close.set(false)>"继续编辑"</button>
                            <button class="button ghost danger" on:click=move |_| discard_and_close()>"不保存退出"</button>
                            <button class="button" on:click=move |_| {
                                confirm_close.set(false);
                                save(true);
                            }>"保存并退出"</button>
                        </div>
                    </section>
                </div>
            </Show>
            <p class="status" role="status">{error}</p>
            <footer class="row"><span>"草稿自动保存在当前浏览器；应用只添加参考图，不自动生成。"</span>
                <Show when=move || applying.get()>
                    <button class="button ghost" on:click=move |_| cancel_apply.set(true)>"取消应用"</button>
                </Show>
                <button class="button" disabled=busy on:click=move |_| save(false)>"保存草稿"</button>
                <button class="button" disabled=move || busy.get() || composer.foreground_generation_task_id.get().is_some()
                    on:click=apply>"应用到工作台"</button>
            </footer>
        </section>
    }
}
