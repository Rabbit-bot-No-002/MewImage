use std::collections::{BTreeMap, HashSet};

use gloo_file::{File, futures::read_as_bytes};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::{
    ev, leptos_dom::helpers::window_event_listener, portal::Portal, prelude::*, task::spawn_local,
};
use mew_image_shared::{
    DEFAULT_FAVORITE_FOLDER_ID, GalleryAsset, GalleryAssetRole, GalleryImportMode,
    GalleryImportResponse, GalleryLikeResponse, GalleryTagSummary, GalleryTemplate,
    GalleryTemplateListResponse, GalleryTemplateStatus, GalleryTemplateUpsertRequest,
    GeneratedImageResult, GenerationResult, GenerationSettingsSnapshot, ImageAssetRef,
    LocalTaskRecord, ParameterSnapshot, ProviderEndpointMode, ProviderKind, TaskStatus, clamp_size,
    new_id, now_rfc3339,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, Event, HtmlAnchorElement, HtmlCanvasElement, HtmlInputElement, MouseEvent};

use crate::app::{
    FAVORITE_ARCHIVE_ASSET_KEY, asset_src, bytes_to_data_url, ensure_asset_display_sources_loaded,
    favorite_folder_picker_style, load_html_image, normalized_favorite_folders, sha256_hex,
    state::{AccountState, ComposerState, MainView, UiState, WorkspaceState},
};
use crate::{api::api_url, storage::apply_asset_payload_changes};

use super::common::{FullscreenImageViewer, MaterialSymbolIcon, PaginationControls};

#[path = "template_transfer.rs"]
mod transfer;
use transfer::TemplateExportDialog;

const PREVIEW_MAX_EDGE: u32 = 2_048;
const REFERENCE_MAX_EDGE: u32 = 4_096;
const TEMPLATE_IMAGE_QUALITY: f64 = 0.9;
const TEMPLATE_PAGE_SIZE: usize = 24;
const TEMPLATE_BATCH_SIZE: usize = 8;
const TEMPLATE_SCROLL_PREFETCH_PX: f64 = 480.0;
const UNCATEGORIZED_TAG_CATEGORY: &str = "未分类";
const MAX_TEMPLATE_TAGS: usize = 12;
const MAX_TEMPLATE_TAG_CHARS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
struct GalleryTagGroup {
    name: String,
    tags: Vec<GalleryTagSummary>,
}

#[derive(Clone)]
struct TemplateFavoritePickerState {
    template: GalleryTemplate,
    x: f64,
    y: f64,
}

#[derive(Clone)]
struct TemplateEditorDraft {
    id: Option<String>,
    title: String,
    prompt: String,
    description: String,
    tags: Vec<String>,
    recommended_provider_kind: ProviderKind,
    recommended_model: String,
    generation_settings: GenerationSettingsSnapshot,
    preview_assets: Vec<GalleryAsset>,
    reference_assets: Vec<GalleryAsset>,
    status: GalleryTemplateStatus,
}

#[derive(Clone, Debug)]
struct TemplateEditorTagUiState {
    selected_category: String,
    search: String,
    new_category: String,
    new_tag_input: String,
    feedback: Option<String>,
}

impl TemplateEditorTagUiState {
    fn for_tags(tags: &[String]) -> Self {
        let selected_category = tags
            .first()
            .map(|tag| gallery_tag_parts(tag).0.to_string())
            .unwrap_or_else(|| UNCATEGORIZED_TAG_CATEGORY.to_string());
        let new_category = if selected_category == UNCATEGORIZED_TAG_CATEGORY {
            String::new()
        } else {
            selected_category.clone()
        };
        Self {
            selected_category,
            search: String::new(),
            new_category,
            new_tag_input: String::new(),
            feedback: None,
        }
    }
}

impl TemplateEditorDraft {
    fn from_template(template: GalleryTemplate) -> Self {
        Self {
            id: Some(template.id),
            title: template.title,
            prompt: template.prompt,
            description: template.description,
            tags: template.tags,
            recommended_provider_kind: template.recommended_provider_kind,
            recommended_model: template.recommended_model,
            generation_settings: template.generation_settings,
            preview_assets: template.preview_assets,
            reference_assets: template.reference_assets,
            status: template.status,
        }
    }
}

fn normalized_gallery_search_filters(query: &str, tags: &[String]) -> (String, Vec<String>) {
    let mut normalized_tags = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    normalized_tags.sort_unstable();
    normalized_tags.dedup();
    (query.trim().to_string(), normalized_tags)
}

fn next_template_visible_count(current: usize, total: usize) -> usize {
    current.saturating_add(TEMPLATE_BATCH_SIZE).min(total)
}

fn reveal_next_template_batch(
    templates: RwSignal<Vec<GalleryTemplate>>,
    visible_count: RwSignal<usize>,
    loading: RwSignal<bool>,
) {
    if loading.get_untracked() {
        return;
    }
    let total = templates.with_untracked(|items| items.len());
    let current = visible_count.get_untracked().min(total);
    let next = next_template_visible_count(current, total);
    if next > current {
        visible_count.set(next);
    }
}

fn viewport_near_document_end(prefetch_px: f64) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Some(document) = window.document() else {
        return false;
    };
    let viewport_height = window
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or_default();
    let scroll_y = window.scroll_y().unwrap_or_default();
    let root_height = document
        .document_element()
        .map(|element| f64::from(element.scroll_height()))
        .unwrap_or_default();
    let body_height = document
        .body()
        .map(|element| f64::from(element.scroll_height()))
        .unwrap_or_default();
    scroll_y + viewport_height + prefetch_px >= root_height.max(body_height)
}

#[component]
pub(crate) fn TemplatePlaza(
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();

    let templates = RwSignal::new(Vec::<GalleryTemplate>::new());
    let visible_template_count = RwSignal::new(TEMPLATE_BATCH_SIZE);
    let available_tags = RwSignal::new(Vec::<GalleryTagSummary>::new());
    let admin_available_tags = RwSignal::new(Vec::<GalleryTagSummary>::new());
    let admin_tags_loading = RwSignal::new(false);
    let admin_tags_error = RwSignal::new(None::<String>);
    let selected_tags = RwSignal::new(Vec::<String>::new());
    let search = RwSignal::new(String::new());
    let applied_filters = RwSignal::new((String::new(), Vec::<String>::new()));
    let sort = RwSignal::new("latest".to_string());
    let page = RwSignal::new(1usize);
    let total_pages = RwSignal::new(1usize);
    let page_count = Memo::new(move |_| total_pages.get());
    let loading = RwSignal::new(false);
    let message = RwSignal::new(None::<String>);
    let show_tag_picker = RwSignal::new(false);
    let show_sort_picker = RwSignal::new(false);
    let tag_search = RwSignal::new(String::new());
    let selected_tag_category = RwSignal::new(None::<String>);
    let selected_template = RwSignal::new(None::<GalleryTemplate>);
    let detail_preview_index = RwSignal::new(0usize);
    let detail_image_fullscreen = RwSignal::new(false);
    let template_favorite_picker = RwSignal::new(None::<TemplateFavoritePickerState>);
    let pending_template_favorites = RwSignal::new(HashSet::<String>::new());
    let request_revision = RwSignal::new(0u64);
    let admin_tags_request_revision = RwSignal::new(0u64);
    let reload_trigger = RwSignal::new(0u64);
    let editor = RwSignal::new(None::<TemplateEditorDraft>);
    let editor_tag_picker_open = RwSignal::new(false);
    let editor_tag_ui = RwSignal::new(TemplateEditorTagUiState::for_tags(&[]));
    let editor_delete_confirm = RwSignal::new(false);
    let editor_busy = RwSignal::new(false);
    let import_input = NodeRef::<leptos::html::Input>::new();
    let export_confirm = RwSignal::new(false);
    let export_busy = RwSignal::new(false);
    let import_confirm = RwSignal::new(false);
    let import_overwrite = RwSignal::new(false);
    let replace_confirm_stage = RwSignal::new(0u8);

    let is_admin = Memo::new(move |_| {
        account.auth_user.with(|user| {
            user.as_ref()
                .is_some_and(|user| user.status == "approved" && user.role == "admin")
        })
    });
    let search_filters_dirty = Memo::new(move |_| {
        normalized_gallery_search_filters(&search.get(), &selected_tags.get())
            != applied_filters.get()
    });

    Effect::new(move |_| {
        let _ = reload_trigger.get();
        let (query, tags) = applied_filters.get();
        let sort_value = sort.get();
        let requested_page = page.get();
        let admin = is_admin.get();
        let revision = request_revision.get_untracked().saturating_add(1);
        request_revision.set(revision);
        loading.set(true);
        spawn_local(async move {
            if request_revision.get_untracked() != revision {
                return;
            }
            let mut url = format!(
                "/api/gallery/templates?page={requested_page}&page_size={TEMPLATE_PAGE_SIZE}&sort={sort_value}"
            );
            if !query.trim().is_empty() {
                url.push_str("&q=");
                url.push_str(&crate::app::percent_encode_query_value(query.trim()));
            }
            if !tags.is_empty() {
                url.push_str("&tags=");
                url.push_str(&crate::app::percent_encode_query_value(&tags.join(",")));
            }
            if admin {
                url.push_str("&admin=true");
            }
            match fetch_json::<GalleryTemplateListResponse>(&url).await {
                Ok(response) if request_revision.get_untracked() == revision => {
                    let pages = response
                        .total
                        .div_ceil(u64::from(response.page_size))
                        .max(1);
                    let initial_count = TEMPLATE_BATCH_SIZE.min(response.items.len());
                    batch(move || {
                        total_pages.set(usize::try_from(pages).unwrap_or(usize::MAX));
                        visible_template_count.set(initial_count);
                        templates.set(response.items);
                        loading.set(false);
                    });
                }
                Err(error) if request_revision.get_untracked() == revision => {
                    message.set(Some(error));
                    loading.set(false);
                }
                _ => {}
            }
        });
    });

    // 首批内容不足一屏时继续补齐；正常页面仍保持首批只挂载 8 张卡片。
    Effect::new(move |_| {
        let total = templates.with(|items| items.len());
        let visible = visible_template_count.get();
        if loading.get() || visible >= total {
            return;
        }
        spawn_local(async move {
            TimeoutFuture::new(0).await;
            if viewport_near_document_end(0.0) {
                reveal_next_template_batch(templates, visible_template_count, loading);
            }
        });
    });

    Effect::new(move |_| {
        let _ = reload_trigger.get();
        // 登录态或模板数据变化时让旧请求失效，避免退出登录后的迟到响应重新填充管理员缓存。
        let admin_tags_revision = admin_tags_request_revision
            .get_untracked()
            .saturating_add(1);
        admin_tags_request_revision.set(admin_tags_revision);
        if is_admin.get() {
            admin_tags_loading.set(true);
            admin_tags_error.set(None);
            spawn_local(async move {
                match fetch_json::<Vec<GalleryTagSummary>>("/api/admin/gallery/tags").await {
                    Ok(tags)
                        if admin_tags_request_revision.get_untracked() == admin_tags_revision
                            && is_admin.get_untracked() =>
                    {
                        admin_available_tags.set(tags);
                    }
                    Err(error)
                        if admin_tags_request_revision.get_untracked() == admin_tags_revision
                            && is_admin.get_untracked() =>
                    {
                        admin_tags_error
                            .set(Some(format!("已有标签加载失败，仍可继续新建标签：{error}")));
                    }
                    _ => return,
                }
                if admin_tags_request_revision.get_untracked() == admin_tags_revision {
                    admin_tags_loading.set(false);
                }
            });
        } else {
            admin_available_tags.set(Vec::new());
            admin_tags_error.set(None);
            admin_tags_loading.set(false);
        }
        spawn_local(async move {
            if let Ok(tags) = fetch_json::<Vec<GalleryTagSummary>>("/api/gallery/tags").await {
                if selected_tag_category.get_untracked().is_none() {
                    selected_tag_category.set(
                        group_gallery_tags(&tags)
                            .first()
                            .map(|group| group.name.clone()),
                    );
                }
                available_tags.set(tags);
            }
            if let Some(template_id) = template_id_from_location() {
                match fetch_json::<GalleryTemplate>(&format!(
                    "/api/gallery/templates/{template_id}"
                ))
                .await
                {
                    Ok(template) => {
                        detail_image_fullscreen.set(false);
                        detail_preview_index.set(0);
                        selected_template.set(Some(template));
                    }
                    Err(error) => message.set(Some(error)),
                }
            }
        });
    });

    let escape_listener = window_event_listener(ev::keydown, move |event| {
        if event.key() != "Escape" {
            return;
        }

        // 只关闭当前最上层界面，避免一次 Escape 同时穿透多个弹层。
        let handled = if detail_image_fullscreen.get_untracked() {
            detail_image_fullscreen.set(false);
            true
        } else if editor_delete_confirm.get_untracked() {
            editor_delete_confirm.set(false);
            true
        } else if template_favorite_picker.get_untracked().is_some() {
            template_favorite_picker.set(None);
            true
        } else if replace_confirm_stage.get_untracked() > 0 {
            replace_confirm_stage.set(0);
            true
        } else if export_confirm.get_untracked() {
            if !export_busy.get_untracked() {
                export_confirm.set(false);
            }
            true
        } else if import_confirm.get_untracked() {
            if !editor_busy.get_untracked() {
                import_confirm.set(false);
            }
            true
        } else if editor_tag_picker_open.get_untracked() {
            editor_tag_picker_open.set(false);
            true
        } else if editor.get_untracked().is_some() {
            editor.set(None);
            true
        } else if selected_template.get_untracked().is_some() {
            selected_template.set(None);
            detail_preview_index.set(0);
            update_template_url(None);
            true
        } else if show_tag_picker.get_untracked() {
            show_tag_picker.set(false);
            true
        } else if show_sort_picker.get_untracked() {
            show_sort_picker.set(false);
            true
        } else if message.get_untracked().is_some() {
            message.set(None);
            true
        } else {
            false
        };

        if handled {
            event.prevent_default();
            event.stop_propagation();
            event.stop_immediate_propagation();
        }
    });
    on_cleanup(move || escape_listener.remove());

    // 滚动检查限制为约 10 Hz，避免连续滚动时反复读取页面尺寸。
    let last_template_scroll_check = RwSignal::new(0.0_f64);
    let template_scroll_listener = window_event_listener(ev::scroll, move |_| {
        let now = js_sys::Date::now();
        if now - last_template_scroll_check.get_untracked() < 100.0 {
            return;
        }
        last_template_scroll_check.set(now);
        if viewport_near_document_end(TEMPLATE_SCROLL_PREFETCH_PX) {
            reveal_next_template_batch(templates, visible_template_count, loading);
        }
    });
    let template_resize_listener = window_event_listener(ev::resize, move |_| {
        if viewport_near_document_end(0.0) {
            reveal_next_template_batch(templates, visible_template_count, loading);
        }
    });
    on_cleanup(move || {
        template_scroll_listener.remove();
        template_resize_listener.remove();
    });

    Effect::new(move |_| {
        let Some(task_id) = ui.gallery_template_draft_task_id.get() else {
            return;
        };
        ui.gallery_template_draft_task_id.set(None);
        if !is_admin.get_untracked() {
            return;
        }
        editor_tag_picker_open.set(false);
        editor_tag_ui.set(TemplateEditorTagUiState::for_tags(&[]));
        open_editor_from_task(task_id, workspace, composer, editor, editor_busy, message);
    });

    let open_template = move |template: GalleryTemplate| {
        update_template_url(Some(&template.id));
        template_favorite_picker.set(None);
        detail_image_fullscreen.set(false);
        detail_preview_index.set(0);
        selected_template.set(Some(template));
    };
    let close_template = move |_| {
        template_favorite_picker.set(None);
        detail_image_fullscreen.set(false);
        selected_template.set(None);
        detail_preview_index.set(0);
        update_template_url(None);
    };
    let submit_search = move || {
        let filters = normalized_gallery_search_filters(
            &search.get_untracked(),
            &selected_tags.get_untracked(),
        );
        show_tag_picker.set(false);
        show_sort_picker.set(false);
        if filters == applied_filters.get_untracked() && page.get_untracked() == 1 {
            return;
        }
        batch(move || {
            page.set(1);
            applied_filters.set(filters);
        });
    };
    let toggle_tag = move |tag: String| {
        selected_tags.update(|items| {
            if let Some(index) = items.iter().position(|item| item == &tag) {
                items.remove(index);
            } else {
                items.push(tag);
            }
        });
    };

    let toggle_like = move |template_id: String, liked: bool| {
        spawn_local(async move {
            let builder = if liked {
                Request::delete(&api_url(&format!(
                    "/api/gallery/templates/{template_id}/like"
                )))
            } else {
                Request::post(&api_url(&format!(
                    "/api/gallery/templates/{template_id}/like"
                )))
            }
            .credentials(web_sys::RequestCredentials::Include);
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<GalleryLikeResponse>().await
                {
                    Ok(result) => {
                        update_like_state(templates, selected_template, &result);
                    }
                    Err(error) => message.set(Some(format!("读取点赞结果失败：{error}"))),
                },
                Ok(response) => message.set(Some(response_error(response).await)),
                Err(error) => message.set(Some(format!("点赞请求失败：{error}"))),
            }
        });
    };

    let use_template = move |template: GalleryTemplate| {
        if composer
            .foreground_generation_task_id
            .get_untracked()
            .is_some()
        {
            message.set(Some(
                "前台生成任务运行时暂不能应用模板，请等待任务完成。".into(),
            ));
            return;
        }
        spawn_local(async move {
            message.set(Some("正在校验并载入模板参考图……".into()));
            match prepare_local_assets(
                &template.reference_assets,
                None,
                &workspace.current_thread_id.get_untracked(),
                false,
            )
            .await
            {
                Ok((local_assets, payloads)) => {
                    let ids = local_assets
                        .iter()
                        .map(|asset| asset.id.clone())
                        .collect::<Vec<_>>();
                    if let Err(error) = apply_asset_payload_changes(&payloads, &[]).await {
                        // 分批写入可能已有前序批次成功，失败时清掉本次新 ID，避免留下孤儿 Blob。
                        let _ = apply_asset_payload_changes(&[], &ids).await;
                        message.set(Some(format!("模板参考图保存失败：{error}")));
                        return;
                    }
                    workspace
                        .assets
                        .update(|assets| assets.extend(local_assets));
                    composer.selected_reference_ids.set(ids);
                    composer.continuation_asset_id.set(None);
                    composer.draft_prompt.set(template.prompt.clone());
                    workspace.threads.update(|threads| {
                        if let Some(thread) = threads
                            .iter_mut()
                            .find(|thread| thread.id == workspace.current_thread_id.get_untracked())
                        {
                            thread.draft_prompt = template.prompt.clone();
                            thread.updated_at = now_rfc3339();
                        }
                    });
                    let size = clamp_size(
                        template.generation_settings.width,
                        template.generation_settings.height,
                    );
                    composer.custom_width.set(size.width);
                    composer.custom_height.set(size.height);
                    composer.resolution_mode.set("custom".into());
                    composer.quality.set(
                        template
                            .generation_settings
                            .quality
                            .clone()
                            .unwrap_or_else(|| "high".into()),
                    );
                    composer
                        .count
                        .set(template.generation_settings.count.clamp(1, 4));
                    let current_kind = workspace.configs.with_untracked(|configs| {
                        configs
                            .iter()
                            .find(|config| config.id == workspace.current_config_id.get_untracked())
                            .map(|config| config.provider_kind)
                    });
                    if current_kind == Some(template.recommended_provider_kind) {
                        workspace.configs.update(|configs| {
                            if let Some(config) = configs.iter_mut().find(|config| {
                                config.id == workspace.current_config_id.get_untracked()
                            }) {
                                config.output_format =
                                    template.generation_settings.output_format.clone();
                                config.output_compression =
                                    template.generation_settings.output_compression;
                                config.background = template.generation_settings.background.clone();
                                config.moderation = template.generation_settings.moderation.clone();
                                config.responses_model =
                                    template.generation_settings.responses_model.clone();
                                config.updated_at = now_rfc3339();
                            }
                        });
                        persist_ui_state();
                    }
                    persist_state();
                    ui.main_view.set(MainView::Workspace);
                    update_main_view_url();
                    composer.status_text.set(if current_kind == Some(template.recommended_provider_kind) {
                        format!("已应用模板“{}”。推荐模型：{}；请确认后手动生成。", template.title, template.recommended_model)
                    } else {
                        format!("已应用模板“{}”的提示词和通用参数。当前服务商与推荐的 {:?} / {} 不同，已保留当前服务商。", template.title, template.recommended_provider_kind, template.recommended_model)
                    });
                }
                Err(error) => message.set(Some(error)),
            }
        });
    };

    let favorite_template =
        move |template: GalleryTemplate, favorite_folder_id: String| {
            let favorite_folder_name =
                normalized_favorite_folders(workspace.preferences.get_untracked().favorite_folders)
                    .into_iter()
                    .find(|folder| folder.id == favorite_folder_id)
                    .map(|folder| folder.name)
                    .unwrap_or_else(|| "默认收藏夹".into());
            let existing_task_id = workspace.tasks.with_untracked(|tasks| {
                tasks
                    .iter()
                    .find(|task| {
                        task.source_gallery_template_id.as_deref() == Some(template.id.as_str())
                    })
                    .map(|task| task.id.clone())
            });
            if let Some(existing_task_id) = existing_task_id {
                workspace.tasks.update(|tasks| {
                    if let Some(task) = tasks.iter_mut().find(|task| task.id == existing_task_id) {
                        task.favorite = true;
                        task.favorite_folder_id = Some(favorite_folder_id);
                        task.detached_from_thread = true;
                        task.updated_at = now_rfc3339();
                    }
                });
                workspace.threads.update(|threads| {
                    for thread in threads {
                        let previous_len = thread.task_ids.len();
                        thread
                            .task_ids
                            .retain(|task_id| task_id != &existing_task_id);
                        if thread.task_ids.len() != previous_len {
                            thread.updated_at = now_rfc3339();
                        }
                    }
                });
                persist_state();
                message.set(Some(format!(
                    "该模板的本地快照已收藏到“{favorite_folder_name}”。"
                )));
                return;
            }
            if pending_template_favorites
                .with_untracked(|template_ids| template_ids.contains(&template.id))
            {
                message.set(Some("该模板正在保存到收藏夹，请稍候。".into()));
                return;
            }
            let pending_template_id = template.id.clone();
            pending_template_favorites.update(|template_ids| {
                template_ids.insert(pending_template_id.clone());
            });
            spawn_local(async move {
                message.set(Some("正在创建可离线使用的收藏快照……".into()));
                let task_id = new_id();
                let thread_id = workspace.current_thread_id.get_untracked();
                let (mut preview_assets, mut preview_payloads) = match prepare_local_assets(
                    &template.preview_assets,
                    Some(&task_id),
                    &thread_id,
                    false,
                )
                .await
                {
                    Ok(value) => value,
                    Err(error) => {
                        pending_template_favorites.update(|template_ids| {
                            template_ids.remove(&pending_template_id);
                        });
                        message.set(Some(error));
                        return;
                    }
                };
                let (reference_assets, reference_payloads) =
                    match prepare_local_assets(&template.reference_assets, None, &thread_id, true)
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            pending_template_favorites.update(|template_ids| {
                                template_ids.remove(&pending_template_id);
                            });
                            message.set(Some(error));
                            return;
                        }
                    };
                preview_payloads.extend(reference_payloads);
                let payload_ids = preview_payloads
                    .iter()
                    .map(|(asset_id, _)| asset_id.clone())
                    .collect::<Vec<_>>();
                if let Err(error) = apply_asset_payload_changes(&preview_payloads, &[]).await {
                    let _ = apply_asset_payload_changes(&[], &payload_ids).await;
                    pending_template_favorites.update(|template_ids| {
                        template_ids.remove(&pending_template_id);
                    });
                    message.set(Some(format!("收藏图片保存失败：{error}")));
                    return;
                }
                let reference_ids = reference_assets
                    .iter()
                    .map(|asset| asset.id.clone())
                    .collect::<Vec<_>>();
                let result_count = preview_assets.len();
                preview_assets.extend(reference_assets);
                workspace
                    .assets
                    .update(|assets| assets.extend(preview_assets));
                let now = now_rfc3339();
                workspace.tasks.update(|tasks| {
                    tasks.push(LocalTaskRecord {
                        id: task_id,
                        thread_id,
                        config_id: workspace.current_config_id.get_untracked(),
                        prompt: template.prompt.clone(),
                        requested_model: template.recommended_model.clone(),
                        reference_asset_ids: reference_ids,
                        generation_settings: Some(template.generation_settings.clone()),
                        result: Some(GenerationResult {
                            images: (0..result_count)
                                .map(|_| GeneratedImageResult {
                                    url: None,
                                    data_url: None,
                                })
                                .collect(),
                            parameter_snapshot: ParameterSnapshot {
                                requested_width: Some(template.generation_settings.width),
                                requested_height: Some(template.generation_settings.height),
                                requested_quality: template.generation_settings.quality.clone(),
                                ..Default::default()
                            },
                            raw_response_json: None,
                        }),
                        favorite: true,
                        favorite_folder_id: Some(favorite_folder_id),
                        detached_from_thread: true,
                        source_gallery_template_id: Some(template.id.clone()),
                        status: TaskStatus::Succeeded,
                        error_message: None,
                        created_at: now.clone(),
                        updated_at: now,
                    })
                });
                persist_state();
                pending_template_favorites.update(|template_ids| {
                    template_ids.remove(&pending_template_id);
                });
                message.set(Some(format!(
                    "已将“{}”收藏到“{favorite_folder_name}”。",
                    template.title,
                )));
            });
        };

    let cancel_template_favorite = move |template_id: String, template_title: String| {
        let mut cancelled_task_id = None;
        workspace.tasks.update(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| {
                task.favorite
                    && task.source_gallery_template_id.as_deref() == Some(template_id.as_str())
            }) else {
                return;
            };
            task.favorite = false;
            task.favorite_folder_id = None;
            // 模板快照不能像普通生成任务一样回到当前会话，否则会污染结果画廊。
            task.detached_from_thread = true;
            task.updated_at = now_rfc3339();
            cancelled_task_id = Some(task.id.clone());
        });
        if let Some(cancelled_task_id) = cancelled_task_id {
            workspace.threads.update(|threads| {
                for thread in threads {
                    let previous_len = thread.task_ids.len();
                    thread
                        .task_ids
                        .retain(|task_id| task_id != &cancelled_task_id);
                    if thread.task_ids.len() != previous_len {
                        thread.updated_at = now_rfc3339();
                    }
                }
            });
            persist_state();
            message.set(Some(format!("已取消收藏“{template_title}”。")));
        }
        template_favorite_picker.set(None);
    };

    let open_template_favorite_picker = move |template: GalleryTemplate, event: MouseEvent| {
        event.stop_propagation();
        ui.favorite_folder_picker.set(None);
        template_favorite_picker.set(Some(TemplateFavoritePickerState {
            template,
            x: f64::from(event.client_x()),
            y: f64::from(event.client_y()),
        }));
    };

    let new_editor = move |_| {
        editor_delete_confirm.set(false);
        editor_tag_picker_open.set(false);
        let draft = default_editor_draft(workspace, composer);
        editor_tag_ui.set(TemplateEditorTagUiState::for_tags(&draft.tags));
        editor.set(Some(draft));
    };
    let edit_template = move |template: GalleryTemplate| {
        editor_delete_confirm.set(false);
        editor_tag_picker_open.set(false);
        let draft = TemplateEditorDraft::from_template(template);
        editor_tag_ui.set(TemplateEditorTagUiState::for_tags(&draft.tags));
        editor.set(Some(draft));
    };

    let import_archive = move |event: Event| {
        let input = event_target::<HtmlInputElement>(&event);
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        input.set_value("");
        let mode = if replace_confirm_stage.get_untracked() > 0 {
            GalleryImportMode::Replace
        } else {
            GalleryImportMode::Merge
        };
        let overwrite = import_overwrite.get_untracked();
        spawn_local(async move {
            editor_busy.set(true);
            if mode == GalleryImportMode::Merge {
                match fetch_json::<serde_json::Value>("/api/health").await {
                    Ok(health) if transfer::supports_import_conflict(&health) => {}
                    _ => {
                        message.set(Some(
                            "当前后端不支持导入冲突策略，请更新并重启后端后重试。".into(),
                        ));
                        editor_busy.set(false);
                        return;
                    }
                }
            }
            let path = match mode {
                GalleryImportMode::Merge if overwrite => {
                    "/api/admin/gallery/import?mode=merge&conflict=overwrite"
                }
                GalleryImportMode::Merge => {
                    "/api/admin/gallery/import?mode=merge&conflict=keep_local"
                }
                GalleryImportMode::Replace => "/api/admin/gallery/import?mode=replace",
            };
            let request = match Request::post(&api_url(path))
                .credentials(web_sys::RequestCredentials::Include)
                .body(file)
            {
                Ok(request) => request,
                Err(error) => {
                    message.set(Some(error.to_string()));
                    editor_busy.set(false);
                    return;
                }
            };
            match request.send().await {
                Ok(response) if response.ok() => {
                    match response.json::<GalleryImportResponse>().await {
                        Ok(result) => {
                            message.set(Some(format!(
                                "已新增 {} 个、覆盖 {} 个、跳过 {} 个模板。",
                                result.added_template_count,
                                result.overwritten_template_count,
                                result.skipped_template_count
                            )));
                            page.set(1);
                            reload_trigger.update(|value| *value = value.saturating_add(1));
                        }
                        Err(error) => message.set(Some(error.to_string())),
                    }
                }
                Ok(response) => message.set(Some(response_error(response).await)),
                Err(error) => message.set(Some(error.to_string())),
            }
            replace_confirm_stage.set(0);
            import_confirm.set(false);
            editor_busy.set(false);
        });
    };

    view! {
        <main class="template-plaza stack">
            <section class="panel template-plaza-hero">
                <div class="template-plaza-heading">
                    <h2>"模板广场"</h2>
                    <Show when=move || is_admin.get()>
                        <div class="template-admin-toolbar" aria-label="模板广场管理">
                            <button class="button ghost icon-button template-admin-create" title="新建模板" aria-label="新建模板" on:click=new_editor>
                                <MaterialSymbolIcon name="add" filled=false />
                            </button>
                            <button class="button ghost icon-button template-admin-export" title="导出模板" aria-label="导出模板" disabled=move || export_busy.get() on:click=move |_| export_confirm.set(true)>
                                <MaterialSymbolIcon name="download" filled=false />
                            </button>
                            <button class="button ghost icon-button template-admin-import" title="合并导入模板" aria-label="合并导入模板" on:click=move |_| {
                                replace_confirm_stage.set(0);
                                import_overwrite.set(false);
                                import_confirm.set(true);
                            }>
                                <MaterialSymbolIcon name="upload" filled=false />
                            </button>
                            <button class="button ghost danger icon-button" title="全量替换模板" aria-label="全量替换模板" on:click=move |_| replace_confirm_stage.set(1)>
                                <MaterialSymbolIcon name="sync" filled=false />
                            </button>
                            <input class="visually-hidden" node_ref=import_input type="file" accept=".zip,application/zip" on:change=import_archive />
                        </div>
                    </Show>
                </div>
                <div class="template-plaza-tools">
                    <div class="template-search">
                        <input
                            type="search"
                            aria-label="搜索模板标题、提示词或标签"
                            placeholder="搜索标题、提示词或标签"
                            prop:value=move || search.get()
                            on:input=move |event| search.set(event_target_value(&event))
                            on:keydown=move |event: web_sys::KeyboardEvent| {
                                if event.key() == "Enter" {
                                    event.prevent_default();
                                    submit_search();
                                }
                            }
                        />
                        <button
                            class="button primary icon-button template-search-submit"
                            class:is-dirty=move || search_filters_dirty.get()
                            title="搜索"
                            aria-label="按当前文字和所选标签搜索"
                            on:click=move |_| submit_search()
                        >
                            <MaterialSymbolIcon name="search" filled=false />
                        </button>
                    </div>
                    <div class="template-sort-filter">
                        <button
                            class="button secondary icon-button template-sort-button"
                            title=move || if sort.get() == "popular" { "当前按最多点赞排序" } else { "当前按最新发布排序" }
                            aria-label=move || if sort.get() == "popular" { "排序：最多点赞" } else { "排序：最新发布" }
                            aria-expanded=move || show_sort_picker.get()
                            on:click=move |_| {
                                show_tag_picker.set(false);
                                show_sort_picker.update(|value| *value = !*value);
                            }
                        >
                            <MaterialSymbolIcon name="sort" filled=false />
                        </button>
                        <Show when=move || show_sort_picker.get()>
                            <div class="template-sort-menu">
                                <button class="template-sort-option" class:is-active=move || sort.get() == "latest"
                                    on:click=move |_| { sort.set("latest".into()); page.set(1); show_sort_picker.set(false); }>
                                    <MaterialSymbolIcon name="schedule" filled=false />
                                    <span><strong>"最新发布"</strong><small>"优先查看最近更新的模板"</small></span>
                                    <Show when=move || sort.get() == "latest"><MaterialSymbolIcon name="check" filled=false /></Show>
                                </button>
                                <button class="template-sort-option" class:is-active=move || sort.get() == "popular"
                                    on:click=move |_| { sort.set("popular".into()); page.set(1); show_sort_picker.set(false); }>
                                    <MaterialSymbolIcon name="favorite" filled=false />
                                    <span><strong>"最多点赞"</strong><small>"优先查看大家喜欢的模板"</small></span>
                                    <Show when=move || sort.get() == "popular"><MaterialSymbolIcon name="check" filled=false /></Show>
                                </button>
                            </div>
                        </Show>
                    </div>
                    <div class="template-tag-filter">
                        <button class="button secondary" aria-expanded=move || show_tag_picker.get() on:click=move |_| {
                            show_sort_picker.set(false);
                            show_tag_picker.update(|value| *value = !*value);
                        }>
                            <MaterialSymbolIcon name="filter_alt" filled=false />
                            {move || if selected_tags.get().is_empty() { "标签筛选".into() } else { format!("已选 {} 项", selected_tags.get().len()) }}
                        </button>
                        <Show when=move || show_tag_picker.get()>
                            <div class="template-tag-menu">
                                <div class="template-tag-menu-header">
                                    <label class="template-tag-search">
                                        <MaterialSymbolIcon name="search" filled=false />
                                        <input type="search" placeholder="搜索当前分类中的标签" prop:value=move || tag_search.get()
                                            on:input=move |event| tag_search.set(event_target_value(&event)) />
                                    </label>
                                    <button class="button ghost" disabled=move || selected_tags.get().is_empty()
                                        on:click=move |_| selected_tags.set(Vec::new())>
                                        "清空"
                                    </button>
                                </div>
                                <div class="template-tag-browser">
                                    <nav class="template-tag-categories" aria-label="标签分类">
                                        <For each=move || group_gallery_tags(&available_tags.get()) key=|group| group.name.clone() children=move |group| {
                                            let category_name = group.name.clone();
                                            let checked_category = group.name.clone();
                                            view! {
                                                <button class="template-tag-category" class:is-active=move || selected_tag_category.get().as_deref() == Some(checked_category.as_str())
                                                    on:click=move |_| { selected_tag_category.set(Some(category_name.clone())); tag_search.set(String::new()); }>
                                                    <span>{group.name}</span><small>{group.tags.len()}</small>
                                                </button>
                                            }
                                        } />
                                    </nav>
                                    <div class="template-tag-options">
                                        <For each=move || visible_gallery_tags(
                                            &available_tags.get(),
                                            selected_tag_category.get().as_deref(),
                                            &tag_search.get(),
                                        ) key=|tag| (tag.name.clone(), tag.template_count) children=move |tag| {
                                            let tag_name = tag.name.clone();
                                            let checked_name = tag.name.clone();
                                            view! { <button class="template-tag-option" class:is-active=move || selected_tags.get().contains(&checked_name)
                                                on:click=move |_| toggle_tag(tag_name.clone())>
                                                <span>{gallery_tag_label(&tag.name).to_string()}</span><small>{tag.template_count}</small>
                                            </button> }
                                        } />
                                        <Show when=move || visible_gallery_tags(
                                            &available_tags.get(),
                                            selected_tag_category.get().as_deref(),
                                            &tag_search.get(),
                                        ).is_empty()>
                                            <p class="template-tag-empty">"当前分类中没有匹配的标签"</p>
                                        </Show>
                                    </div>
                                </div>
                                <div class="template-tag-menu-actions">
                                    <button class="button primary" on:click=move |_| submit_search()>
                                        <MaterialSymbolIcon name="search" filled=false />
                                        "按所选标签搜索"
                                    </button>
                                </div>
                            </div>
                        </Show>
                    </div>
                </div>
                <Show when=move || !selected_tags.get().is_empty()>
                    <div class="template-selected-tags">
                        <For each=move || selected_tags.get() key=|tag| tag.clone() children=move |tag| {
                            let remove_tag = tag.clone();
                            view! { <button class="tag is-selected" title=tag.clone() on:click=move |_| toggle_tag(remove_tag.clone())>{format!("{} ×", gallery_tag_breadcrumb(&tag))}</button> }
                        } />
                    </div>
                </Show>
            </section>

            <Show when=move || loading.get()>
                <div class="template-loading"><span class="gallery-running-spinner"></span>"正在整理灵感星图……"</div>
            </Show>
            <section class="template-card-grid">
                <For each=move || {
                    let visible = visible_template_count.get();
                    templates.with(|items| items.iter().take(visible).cloned().collect::<Vec<_>>())
                } key=|template| (
                    template.id.clone(),
                    template.updated_at.clone(),
                    template.like_count,
                    template.liked_by_viewer,
                ) children=move |template| {
                    let open_value = template.clone();
                    let like_id = template.id.clone();
                    let liked = template.liked_by_viewer;
                    let copy_prompt = template.prompt.clone();
                    let share_id = template.id.clone();
                    let use_value = template.clone();
                    let favorite_value = template.clone();
                    let edit_value = template.clone();
                    let favorite_template_id = template.id.clone();
                    let pending_template_id = template.id.clone();
                    let is_favorite = Memo::new(move |_| {
                        workspace.tasks.with(|tasks| {
                            is_gallery_template_favorite(tasks, &favorite_template_id)
                        })
                    });
                    let favorite_pending = Memo::new(move |_| {
                        pending_template_favorites
                            .with(|template_ids| template_ids.contains(&pending_template_id))
                    });
                    let like_label = format_compact_like_count(template.like_count);
                    let like_title = format!("点赞（{}）", template.like_count);
                    view! {
                        <article class="panel template-card">
                            <button class="template-card-preview" on:click=move |_| open_template(open_value.clone())>
                                {template.preview_assets.first().map(|asset| view! {
                                    <img src=gallery_thumbnail_url(asset) alt=template.title.clone() loading="lazy" />
                                }.into_any()).unwrap_or_else(|| view! { <div class="template-empty-preview"><MaterialSymbolIcon name="image" filled=false /></div> }.into_any())}
                                <span class="template-card-status">{status_label(template.status)}</span>
                            </button>
                            {move || if is_admin.get() {
                                let edit_target = edit_value.clone();
                                view! { <button class="button ghost icon-button template-card-edit" title="编辑模板" aria-label="编辑模板" on:click=move |_| edit_template(edit_target.clone())>
                                    <MaterialSymbolIcon name="edit" filled=false />
                                </button> }.into_any()
                            } else { ().into_any() }}
                            <div class="template-card-body">
                                <div class="template-card-title-row"><h3>{template.title.clone()}</h3><span class="tag">{template.recommended_model.clone()}</span></div>
                                <p>{prompt_excerpt(&template.prompt)}</p>
                                <div class="template-card-tags">{template.tags.iter().map(|tag| view! {
                                    <span class="tag" title=tag.clone()>{gallery_tag_label(tag).to_string()}</span>
                                }).collect_view()}</div>
                                <div class="template-card-actions">
                                    <button class="button ghost template-like-button" title=like_title aria-label=format!("点赞，当前 {} 赞", template.like_count) class:is-active=liked on:click=move |_| toggle_like(like_id.clone(), liked)>
                                        <MaterialSymbolIcon name="favorite" filled=liked /><span>{like_label}</span>
                                    </button>
                                    <button class="button ghost icon-button" title="复制提示词" on:click=move |_| copy_text(copy_prompt.clone(), message)>
                                        <MaterialSymbolIcon name="content_copy" filled=false />
                                    </button>
                                    <button class="button ghost icon-button" title="复制分享链接" on:click=move |_| share_template(&share_id, message)>
                                        <MaterialSymbolIcon name="share" filled=false />
                                    </button>
                                    <button
                                        class="button ghost icon-button template-favorite-button"
                                        class:is-active=move || is_favorite.get()
                                        title=move || if is_favorite.get() { "管理收藏" } else { "选择收藏夹" }
                                        aria-label=move || if is_favorite.get() { "管理收藏" } else { "选择收藏夹" }
                                        aria-pressed=move || is_favorite.get()
                                        disabled=move || favorite_pending.get()
                                        on:click=move |event: MouseEvent| open_template_favorite_picker(favorite_value.clone(), event)
                                    >
                                        {move || view! { <MaterialSymbolIcon name="star" filled=is_favorite.get() /> }}
                                    </button>
                                    <button class="button primary" on:click=move |_| use_template(use_value.clone())>"使用模板"</button>
                                </div>
                            </div>
                        </article>
                    }
                } />
            </section>
            <Show when=move || !loading.get() && templates.get().is_empty()>
                <div class="panel template-empty-state"><MaterialSymbolIcon name="travel_explore" filled=false /><h3>"还没有匹配的模板"</h3><p>"换个关键词或减少标签试试看。"</p></div>
            </Show>
            <Show when=move || !loading.get() && templates.with(|items| {
                !items.is_empty() && visible_template_count.get() < items.len()
            })>
                <div class="template-progressive-loader" role="status">
                    <span class="gallery-running-spinner"></span>
                    <span>{move || format!(
                        "继续向下浏览 · 已显示 {}/{}",
                        visible_template_count.get(),
                        templates.with(|items| items.len()),
                    )}</span>
                </div>
            </Show>
            <Show when=move || !loading.get() && templates.with(|items| {
                !items.is_empty() && visible_template_count.get() >= items.len()
            })>
                <PaginationControls page=page page_count=page_count favorite=false />
            </Show>
        </main>

        {move || message.get().map(|notice| view! {
            <div class="template-plaza-notice" role="status" aria-live="polite">
                <span>{notice}</span>
                <button class="button ghost icon-button" title="关闭提示" aria-label="关闭提示" on:click=move |_| message.set(None)>
                    <MaterialSymbolIcon name="close" filled=false />
                </button>
            </div>
        })}

        {move || template_favorite_picker.get().map(|picker| {
            let style = favorite_folder_picker_style(picker.x, picker.y);
            let current_folder_id = workspace.tasks.with_untracked(|tasks| {
                tasks
                    .iter()
                    .find(|task| {
                        task.favorite
                            && task.source_gallery_template_id.as_deref()
                                == Some(picker.template.id.as_str())
                    })
                    .map(|task| {
                        task.favorite_folder_id
                            .clone()
                            .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into())
                    })
            });
            let picker_title = if current_folder_id.is_some() { "移动到" } else { "收藏到" };
            let folders = normalized_favorite_folders(
                workspace.preferences.get_untracked().favorite_folders,
            );
            let is_favorite = current_folder_id.is_some();
            let cancel_template_id = picker.template.id.clone();
            let cancel_template_title = picker.template.title.clone();
            view! {
                <>
                    <button class="folder-picker-dismiss" aria-label="关闭收藏文件夹选择" on:click=move |_| template_favorite_picker.set(None)></button>
                    <div class="folder-picker-popover template-folder-picker" style=style>
                        <strong>{picker_title}</strong>
                        {folders.into_iter().filter(|folder| {
                            !is_favorite || current_folder_id.as_deref() != Some(folder.id.as_str())
                        }).map(|folder| {
                            let folder_id = folder.id;
                            let target_template = picker.template.clone();
                            view! {
                                <button
                                    class="folder-picker-item"
                                    on:click=move |_| {
                                        template_favorite_picker.set(None);
                                        favorite_template(target_template.clone(), folder_id.clone());
                                    }
                                >
                                    <MaterialSymbolIcon name="folder" filled=false />
                                    <span>{folder.name}</span>
                                </button>
                            }
                        }).collect_view()}
                        {if is_favorite {
                            view! {
                                <button class="folder-picker-item folder-picker-cancel" on:click=move |_| {
                                    cancel_template_favorite(cancel_template_id.clone(), cancel_template_title.clone());
                                }>
                                    <MaterialSymbolIcon name="star" filled=false />
                                    <span>"取消收藏"</span>
                                </button>
                            }.into_any()
                        } else {
                            ().into_any()
                        }}
                    </div>
                </>
            }
        })}

        {move || selected_template.get().map(|template| {
            let like_id = template.id.clone(); let liked = template.liked_by_viewer;
            let use_value = template.clone(); let favorite_value = template.clone(); let prompt = template.prompt.clone();
            let like_label = format_compact_like_count(template.like_count);
            let like_title = format!("点赞（{}）", template.like_count);
            let is_favorite = workspace.tasks.with(|tasks| {
                is_gallery_template_favorite(tasks, &template.id)
            });
            let favorite_pending = pending_template_favorites
                .with(|template_ids| template_ids.contains(&template.id));
            let preview_count = template.preview_assets.len();
            let active_preview_index = if preview_count == 0 {
                0
            } else {
                detail_preview_index.get().min(preview_count - 1)
            };
            let active_preview_asset = template.preview_assets.get(active_preview_index).cloned();
            let fullscreen_preview_src = active_preview_asset.as_ref().map(gallery_asset_url);
            let fullscreen_preview_alt = format!("{} 的结果预览", template.title);
            view! { <div class="modal-backdrop template-detail-backdrop" on:click=close_template>
                <article class="panel template-detail" role="dialog" aria-modal="true" aria-labelledby="template-detail-title" on:click=move |event| event.stop_propagation()>
                    <section class="template-detail-visual" aria-label="模板结果预览">
                        <div class="template-detail-stage">
                            {active_preview_asset.as_ref().map(|asset| view! {
                                <button
                                    class="image-button template-detail-image-button"
                                    title="全屏查看结果图"
                                    aria-label="全屏查看结果图"
                                    on:click=move |_| detail_image_fullscreen.set(true)
                                >
                                    <img src=gallery_asset_url(asset) alt=format!("{} 的结果预览", template.title) />
                                </button>
                            }.into_any()).unwrap_or_else(|| view! {
                                <div class="template-detail-empty-preview"><MaterialSymbolIcon name="image" filled=false /><span>"暂无结果预览"</span></div>
                            }.into_any())}
                            <Show when={move || preview_count > 1}>
                                <span class="template-detail-preview-count">{format!("{} / {}", active_preview_index + 1, preview_count)}</span>
                            </Show>
                        </div>
                        <Show when={move || preview_count > 1}>
                            <div class="template-detail-thumbnails" role="tablist" aria-label="切换结果预览">
                                {template.preview_assets.iter().enumerate().map(|(index, asset)| view! {
                                    <button
                                        class="template-detail-thumbnail"
                                        class:is-active=move || detail_preview_index.get() == index
                                        aria-label=format!("查看第 {} 张结果图", index + 1)
                                        aria-pressed=move || detail_preview_index.get() == index
                                        on:click=move |_| detail_preview_index.set(index)
                                    >
                                        <img src=gallery_thumbnail_url(asset) alt="" loading="lazy" />
                                    </button>
                                }).collect_view()}
                            </div>
                        </Show>
                    </section>
                    <aside class="template-detail-content">
                        <button class="button ghost icon-button template-modal-close" title="关闭详情" aria-label="关闭详情" on:click=close_template><MaterialSymbolIcon name="close" filled=false /></button>
                        <header class="template-detail-header stack">
                            <div><span class="template-plaza-kicker">"GALLERY TEMPLATE"</span><h2 id="template-detail-title">{template.title.clone()}</h2></div>
                            <p>{template.description.clone()}</p>
                            <div class="template-card-tags">{template.tags.iter().map(|tag| view! {
                                <span class="tag" title=tag.clone()>{gallery_tag_label(tag).to_string()}</span>
                            }).collect_view()}</div>
                        </header>
                        <div class="template-prompt-box"><strong>"提示词"</strong><p>{template.prompt.clone()}</p>
                            <button class="button ghost template-prompt-copy" title="复制提示词" aria-label="复制提示词" on:click=move |_| copy_text(prompt.clone(), message)><MaterialSymbolIcon name="content_copy" filled=false />"复制"</button>
                        </div>
                        <div class="template-detail-fixed-info">
                            {if template.reference_assets.is_empty() {
                                ().into_any()
                            } else {
                                view! { <div class="template-reference-strip"><strong>"参考图"</strong><div>{template.reference_assets.iter().map(|asset| view! { <img src=gallery_thumbnail_url(asset) alt="模板参考图" loading="lazy" /> }).collect_view()}</div></div> }.into_any()
                            }}
                            <dl class="template-parameters"><div><dt>"推荐模型"</dt><dd>{template.recommended_model.clone()}</dd></div><div><dt>"尺寸"</dt><dd>{format!("{} × {}", template.generation_settings.width, template.generation_settings.height)}</dd></div><div><dt>"质量"</dt><dd>{template.generation_settings.quality.clone().unwrap_or_else(|| "自动".into())}</dd></div><div><dt>"参考图"</dt><dd>{format!("{} 张", template.reference_assets.len())}</dd></div></dl>
                        </div>
                        <div class="template-detail-actions template-card-actions">
                            <button class="button ghost template-like-button" title=like_title aria-label=format!("点赞，当前 {} 赞", template.like_count) class:is-active=liked on:click=move |_| toggle_like(like_id.clone(), liked)><MaterialSymbolIcon name="favorite" filled=liked /><span>{like_label}</span></button>
                            <button class="button ghost icon-button" title="复制分享链接" aria-label="复制分享链接" on:click=move |_| share_template(&template.id, message)><MaterialSymbolIcon name="share" filled=false /></button>
                            <button class="button ghost icon-button template-favorite-button" class:is-active=is_favorite title=if is_favorite { "管理收藏" } else { "选择收藏夹" } aria-label=if is_favorite { "管理收藏" } else { "选择收藏夹" } aria-pressed=is_favorite disabled=favorite_pending on:click=move |event: MouseEvent| open_template_favorite_picker(favorite_value.clone(), event)><MaterialSymbolIcon name="star" filled=is_favorite /></button>
                            <button class="button primary" on:click=move |_| use_template(use_value.clone())>"使用模板"</button></div>
                    </aside>
                </article>
                {move || {
                    if !detail_image_fullscreen.get() {
                        return ().into_any();
                    }
                    let Some(src) = fullscreen_preview_src.clone() else {
                        return ().into_any();
                    };
                    view! {
                        <FullscreenImageViewer
                            src=src
                            alt=fullscreen_preview_alt.clone()
                            show_download=false
                            close=move || detail_image_fullscreen.set(false)
                            download=move || {}
                        />
                    }.into_any()
                }}
            </div> }
        })}

        {move || editor.get().map(|draft| view! {
            <TemplateEditor
                draft
                editor
                delete_confirm=editor_delete_confirm
                tag_picker_open=editor_tag_picker_open
                tag_ui=editor_tag_ui
                available_tags=admin_available_tags
                tags_loading=admin_tags_loading
                tags_error=admin_tags_error
                editor_busy
                message
                templates
                reload_trigger
            />
        })}

        <Show when=move || export_confirm.get()>
            <TemplateExportDialog open=export_confirm busy=export_busy tags=admin_available_tags tags_loading=admin_tags_loading tags_error=admin_tags_error reload=reload_trigger message />
        </Show>
        <Show when=move || import_confirm.get()>
            <div class="modal-backdrop" on:click=move |_| { if !editor_busy.get_untracked() { import_confirm.set(false); } }>
                <section class="panel confirm-dialog" on:click=move |event| event.stop_propagation()>
                    <h3>"合并导入模板"</h3>
                    <p>"新增模板正常导入；遇到相同 UUID 的模板时："</p>
                    <label><input type="radio" name="gallery-conflict" prop:checked=move || !import_overwrite.get() disabled=move || editor_busy.get() on:change=move |_| import_overwrite.set(false) />"保留本地版本（跳过包内同 ID 模板）"</label>
                    <label><input type="radio" name="gallery-conflict" prop:checked=move || import_overwrite.get() disabled=move || editor_busy.get() on:change=move |_| import_overwrite.set(true) />"使用包内版本（覆盖本地同 ID 模板）"</label>
                    <p>"包外模板和现有点赞保留；图片及生成参数一同导入。"</p>
                    <div class="row">
                        <button class="button ghost" disabled=move || editor_busy.get() on:click=move |_| import_confirm.set(false)>"取消"</button>
                        <button class="button primary" disabled=move || editor_busy.get() on:click=move |_| { if let Some(input) = import_input.get() { input.click(); } }>{move || if editor_busy.get() { "正在导入…" } else { "选择 ZIP 并导入" }}</button>
                    </div>
                </section>
            </div>
        </Show>

        <Show when=move || { replace_confirm_stage.get() > 0 }>
            <div class="modal-backdrop" on:click=move |_| replace_confirm_stage.set(0)>
                <div class="panel confirm-dialog" on:click=move |event| event.stop_propagation()>
                    <h3>"全量替换模板广场"</h3>
                    <p>{move || if replace_confirm_stage.get() == 1 { "即使导入的是分类包，也会替换整个广场，包外模板及全部点赞会被清除。请再次确认。" } else { "这是最后一步确认。现有模板资源将在完整校验成功后被替换。" }}</p>
                    <div class="row"><button class="button ghost" on:click=move |_| replace_confirm_stage.set(0)>"取消"</button>
                    <button class="button danger" on:click=move |_| {
                        if replace_confirm_stage.get_untracked() == 1 { replace_confirm_stage.set(2); }
                        else if let Some(input) = import_input.get() { input.click(); }
                    }>{move || if replace_confirm_stage.get() == 1 { "继续" } else { "选择模板包并替换" }}</button></div>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn TemplateEditor(
    draft: TemplateEditorDraft,
    editor: RwSignal<Option<TemplateEditorDraft>>,
    delete_confirm: RwSignal<bool>,
    tag_picker_open: RwSignal<bool>,
    tag_ui: RwSignal<TemplateEditorTagUiState>,
    available_tags: RwSignal<Vec<GalleryTagSummary>>,
    tags_loading: RwSignal<bool>,
    tags_error: RwSignal<Option<String>>,
    editor_busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
    templates: RwSignal<Vec<GalleryTemplate>>,
    reload_trigger: RwSignal<u64>,
) -> impl IntoView {
    let save = move |_| {
        let Some(draft) = editor.get_untracked() else {
            return;
        };
        editor_busy.set(true);
        spawn_local(async move {
            let payload = editor_request(&draft);
            let (method, path) = draft
                .id
                .as_ref()
                .map(|id| ("put", format!("/api/admin/gallery/templates/{id}")))
                .unwrap_or_else(|| ("post", "/api/admin/gallery/templates".into()));
            let builder = if method == "put" {
                Request::put(&api_url(&path))
            } else {
                Request::post(&api_url(&path))
            };
            let request = match builder
                .credentials(web_sys::RequestCredentials::Include)
                .json(&payload)
            {
                Ok(request) => request,
                Err(error) => {
                    message.set(Some(error.to_string()));
                    editor_busy.set(false);
                    return;
                }
            };
            match request.send().await {
                Ok(response) if response.ok() => match response.json::<GalleryTemplate>().await {
                    Ok(saved) => {
                        templates.update(|items| {
                            if let Some(item) = items.iter_mut().find(|item| item.id == saved.id) {
                                *item = saved.clone();
                            } else {
                                items.insert(0, saved);
                            }
                        });
                        reload_trigger.update(|value| *value = value.saturating_add(1));
                        delete_confirm.set(false);
                        tag_picker_open.set(false);
                        editor.set(None);
                    }
                    Err(error) => message.set(Some(error.to_string())),
                },
                Ok(response) => message.set(Some(response_error(response).await)),
                Err(error) => message.set(Some(error.to_string())),
            }
            editor_busy.set(false);
        });
    };
    let delete_id = StoredValue::new(draft.id.clone());
    view! { <div class="modal-backdrop template-editor-backdrop">
        <section class="panel template-editor stack">
            <header class="template-editor-header"><span class="template-plaza-kicker">"ADMIN EDITOR"</span><h2>{if draft.id.is_some() { "编辑模板" } else { "新建模板" }}</h2></header>
            <button class="button ghost icon-button template-editor-close" title="关闭编辑器" aria-label="关闭编辑器" on:click=move |_| { delete_confirm.set(false); tag_picker_open.set(false); editor.set(None); }><MaterialSymbolIcon name="close" filled=false /></button>
            <label>"标题"<input class="text-input" prop:value=draft.title on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.title = event_target_value(&event) }) /></label>
            <label>"提示词"<textarea class="text-input template-editor-prompt" prop:value=draft.prompt on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.prompt = event_target_value(&event) }) /></label>
            <label>"说明"<textarea class="text-input" prop:value=draft.description on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.description = event_target_value(&event) }) /></label>
            <TemplateEditorTags editor picker_open=tag_picker_open ui_state=tag_ui available_tags loading=tags_loading load_error=tags_error />
            <div class="template-editor-fields">
                <label class="template-editor-model-field">"推荐模型"<input class="text-input" prop:value=draft.recommended_model on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.recommended_model = event_target_value(&event) }) /></label>
                <div class="template-editor-field template-editor-size-field">
                    <span>"尺寸"</span>
                    <div class="template-editor-size-inputs">
                        <input class="text-input" type="number" min="1" aria-label="模板宽度" prop:value=draft.generation_settings.width on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.generation_settings.width = event_target_value(&event).parse().unwrap_or(1024) }) />
                        <span aria-hidden="true">"×"</span>
                        <input class="text-input" type="number" min="1" aria-label="模板高度" prop:value=draft.generation_settings.height on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.generation_settings.height = event_target_value(&event).parse().unwrap_or(1024) }) />
                    </div>
                </div>
                <label class="template-editor-status-field">"状态"<select class="select-input" prop:value=status_value(draft.status) on:change=move |event| editor.update(|draft| if let Some(draft) = draft { draft.status = parse_status(&event_target_value(&event)) })><option value="draft">"草稿"</option><option value="published">"已发布"</option><option value="archived">"已归档"</option></select></label>
            </div>
            <EditorAssets title="预览图（最多 6 张）" assets=draft.preview_assets editor role=GalleryAssetRole::Preview max=6 max_edge=PREVIEW_MAX_EDGE editor_busy message />
            <EditorAssets title="参考图（最多 16 张）" assets=draft.reference_assets editor role=GalleryAssetRole::Reference max=16 max_edge=REFERENCE_MAX_EDGE editor_busy message />
            <div class="row template-editor-actions">
                {delete_id.get_value().map(|_| view! { <button class="button danger" disabled=move || editor_busy.get() on:click=move |_| { tag_picker_open.set(false); delete_confirm.set(true); }><MaterialSymbolIcon name="delete" filled=false />"删除模板"</button> })}
                <span class="spacer"></span><button class="button ghost" on:click=move |_| { delete_confirm.set(false); tag_picker_open.set(false); editor.set(None); }>"取消"</button><button class="button primary" disabled=move || editor_busy.get() on:click=save>"保存模板"</button>
            </div>
            <Show when=move || delete_confirm.get()>
                <div class="template-inline-confirm">
                    <p>"确定删除这个模板吗？不再被其他模板引用的广场图片也会一并清理。"</p>
                    <div class="row"><button class="button ghost" on:click=move |_| delete_confirm.set(false)>"取消"</button>
                        {move || delete_id.get_value().map(|target_id| {
                            view! { <button class="button danger" on:click=move |_| { delete_confirm.set(false); delete_admin_template(target_id.clone(), editor, templates, message, editor_busy, reload_trigger); }>"确认删除"</button> }
                        })}
                    </div>
                </div>
            </Show>
        </section>
    </div> }
}

#[component]
fn TemplateEditorTags(
    editor: RwSignal<Option<TemplateEditorDraft>>,
    picker_open: RwSignal<bool>,
    ui_state: RwSignal<TemplateEditorTagUiState>,
    available_tags: RwSignal<Vec<GalleryTagSummary>>,
    loading: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let search_input = NodeRef::<leptos::html::Input>::new();

    view! {
        <div class="template-editor-tags" aria-labelledby="template-editor-tags-label">
            <div class="template-editor-tags-header">
                <strong id="template-editor-tags-label">"标签"</strong>
                <small>{move || format!("已选 {} / {MAX_TEMPLATE_TAGS}", editor_tag_count(editor))}</small>
            </div>
            <div class="template-editor-tag-chips">
                <Show when=move || current_editor_tags(editor).is_empty()>
                    <span class="template-editor-tags-empty">"尚未添加标签"</span>
                </Show>
                <For
                    each=move || current_editor_tags(editor)
                    key=|tag| tag.clone()
                    children=move |tag| {
                        let remove_tag = tag.clone();
                        view! {
                            <button
                                class="template-editor-tag-chip"
                                title=format!("移除标签：{}", gallery_tag_breadcrumb(&tag))
                                aria-label=format!("移除标签：{}", gallery_tag_breadcrumb(&tag))
                                on:click=move |_| remove_editor_tag(editor, &remove_tag)
                            >
                                <span>{gallery_tag_breadcrumb(&tag)}</span>
                                <MaterialSymbolIcon name="close" filled=false />
                            </button>
                        }
                    }
                />
                <button class="button secondary template-editor-add-tag" on:click=move |_| {
                    ui_state.update(|state| {
                        state.feedback = None;
                        state.search.clear();
                    });
                    picker_open.set(true);
                    spawn_local(async move {
                        TimeoutFuture::new(0).await;
                        if let Some(input) = search_input.get() {
                            let _ = input.focus();
                        }
                    });
                }>
                    <MaterialSymbolIcon name="add" filled=false />
                    "添加标签"
                </button>
            </div>
        </div>

        <Show when=move || picker_open.get()>
            <Portal>
                <div class="modal-backdrop template-editor-tag-backdrop" on:click=move |_| picker_open.set(false)>
                    <section
                        class="template-editor-tag-dialog"
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="template-editor-tag-dialog-title"
                        on:click=move |event: MouseEvent| event.stop_propagation()
                    >
                        <header class="template-editor-tag-dialog-header">
                            <div>
                                <span class="template-plaza-kicker">"TAG EDITOR"</span>
                                <h3 id="template-editor-tag-dialog-title">"选择或新建标签"</h3>
                            </div>
                            <button class="button ghost icon-button" title="关闭标签选择" aria-label="关闭标签选择" on:click=move |_| picker_open.set(false)>
                                <MaterialSymbolIcon name="close" filled=false />
                            </button>
                        </header>

                        <label class="template-tag-search template-editor-tag-search">
                            <MaterialSymbolIcon name="search" filled=false />
                            <input
                                node_ref=search_input
                                type="search"
                                placeholder="搜索当前分类中的标签"
                                prop:value=move || ui_state.with(|state| state.search.clone())
                                on:input=move |event| ui_state.update(|state| state.search = event_target_value(&event))
                            />
                        </label>

                        <div class="template-tag-browser template-editor-tag-browser">
                            <nav class="template-tag-categories" aria-label="标签分类">
                                <For
                                    each=move || editor_tag_groups(&available_tags.get(), &current_editor_tags(editor))
                                    key=|group| group.name.clone()
                                    children=move |group| {
                                        let category_name = group.name.clone();
                                        let checked_category = group.name.clone();
                                        let input_category = if group.name == UNCATEGORIZED_TAG_CATEGORY {
                                            String::new()
                                        } else {
                                            group.name.clone()
                                        };
                                        view! {
                                            <button
                                                class="template-tag-category"
                                                class:is-active=move || ui_state.with(|state| state.selected_category == checked_category)
                                                on:click=move |_| {
                                                    ui_state.update(|state| {
                                                        state.selected_category = category_name.clone();
                                                        state.new_category = input_category.clone();
                                                        state.search.clear();
                                                        state.feedback = None;
                                                    });
                                                }
                                            >
                                                <span>{group.name}</span>
                                                <small>{group.tags.len()}</small>
                                            </button>
                                        }
                                    }
                                />
                            </nav>
                            <div class="template-tag-options">
                                <For
                                    each=move || visible_gallery_tags(
                                        &merged_editor_tag_summaries(&available_tags.get(), &current_editor_tags(editor)),
                                        Some(ui_state.with(|state| state.selected_category.clone()).as_str()),
                                        &ui_state.with(|state| state.search.clone()),
                                    )
                                    key=|tag| tag.name.clone()
                                    children=move |tag| {
                                        let tag_name = tag.name.clone();
                                        let selected_for_class = tag.name.clone();
                                        let selected_for_disabled = tag.name.clone();
                                        let usage = if tag.template_count == 0 {
                                            "新".to_string()
                                        } else {
                                            tag.template_count.to_string()
                                        };
                                        view! {
                                            <button
                                                class="template-tag-option"
                                                class:is-active=move || editor_has_tag(editor, &selected_for_class)
                                                disabled=move || {
                                                    editor_tag_count(editor) >= MAX_TEMPLATE_TAGS
                                                        && !editor_has_tag(editor, &selected_for_disabled)
                                                }
                                                on:click=move |_| {
                                                    let result = toggle_editor_tag(editor, &tag_name).err();
                                                    ui_state.update(|state| state.feedback = result);
                                                }
                                            >
                                                <span>{gallery_tag_label(&tag.name).to_string()}</span>
                                                <small>{usage}</small>
                                            </button>
                                        }
                                    }
                                />
                                <Show when=move || visible_gallery_tags(
                                    &merged_editor_tag_summaries(&available_tags.get(), &current_editor_tags(editor)),
                                    Some(ui_state.with(|state| state.selected_category.clone()).as_str()),
                                    &ui_state.with(|state| state.search.clone()),
                                ).is_empty()>
                                    <p class="template-tag-empty">"当前分类中没有匹配的标签"</p>
                                </Show>
                            </div>
                        </div>

                        <div class="template-editor-new-tag">
                            <label>
                                <span>"分类（可选）"</span>
                                <input
                                    class="text-input"
                                    placeholder="留空即未分类"
                                    prop:value=move || ui_state.with(|state| state.new_category.clone())
                                    on:input=move |event| ui_state.update(|state| state.new_category = event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"新标签"</span>
                                <textarea
                                    class="text-input"
                                    rows="1"
                                    placeholder="输入标签；可用逗号、顿号、分号或换行批量添加"
                                    prop:value=move || ui_state.with(|state| state.new_tag_input.clone())
                                    on:input=move |event| ui_state.update(|state| state.new_tag_input = event_target_value(&event))
                                    on:keydown=move |event: web_sys::KeyboardEvent| {
                                        if should_submit_editor_tag(
                                            &event.key(),
                                            event.shift_key(),
                                            event.is_composing(),
                                        ) {
                                            event.prevent_default();
                                            submit_editor_tag_input(editor, ui_state);
                                        }
                                    }
                                ></textarea>
                            </label>
                            <button
                                class="button primary template-editor-new-tag-submit"
                                disabled=move || {
                                    ui_state.with(|state| state.new_tag_input.trim().is_empty())
                                        || editor_tag_count(editor) >= MAX_TEMPLATE_TAGS
                                }
                                on:click=move |_| submit_editor_tag_input(editor, ui_state)
                            >
                                <MaterialSymbolIcon name="add" filled=false />
                                "添加"
                            </button>
                        </div>

                        <Show when=move || loading.get()>
                            <p class="template-editor-tag-note">"正在加载已有标签……"</p>
                        </Show>
                        {move || load_error.get().map(|error| view! {
                            <p class="template-editor-tag-note is-error">{error}</p>
                        })}
                        {move || ui_state.with(|state| state.feedback.clone()).map(|notice| view! {
                            <p class="template-editor-tag-note is-error">{notice}</p>
                        })}

                        <footer class="template-editor-tag-dialog-actions">
                            <span>{move || format!("已选 {} / {MAX_TEMPLATE_TAGS}", editor_tag_count(editor))}</span>
                            <button class="button primary" on:click=move |_| picker_open.set(false)>"完成"</button>
                        </footer>
                    </section>
                </div>
            </Portal>
        </Show>
    }
}

fn current_editor_tags(editor: RwSignal<Option<TemplateEditorDraft>>) -> Vec<String> {
    editor.with(|draft| {
        draft
            .as_ref()
            .map(|draft| draft.tags.clone())
            .unwrap_or_default()
    })
}

fn editor_tag_count(editor: RwSignal<Option<TemplateEditorDraft>>) -> usize {
    editor.with(|draft| draft.as_ref().map_or(0, |draft| draft.tags.len()))
}

fn editor_tag_key(tag: &str) -> String {
    tag.trim().to_lowercase()
}

fn editor_has_tag(editor: RwSignal<Option<TemplateEditorDraft>>, tag: &str) -> bool {
    let expected = editor_tag_key(tag);
    editor.with(|draft| {
        draft.as_ref().is_some_and(|draft| {
            draft
                .tags
                .iter()
                .any(|current| editor_tag_key(current) == expected)
        })
    })
}

fn merged_editor_tag_summaries(
    available_tags: &[GalleryTagSummary],
    selected_tags: &[String],
) -> Vec<GalleryTagSummary> {
    let mut merged = BTreeMap::<String, GalleryTagSummary>::new();
    for tag in available_tags {
        merged.insert(editor_tag_key(&tag.name), tag.clone());
    }
    for tag in selected_tags {
        merged
            .entry(editor_tag_key(tag))
            .or_insert_with(|| GalleryTagSummary {
                name: tag.clone(),
                template_count: 0,
            });
    }
    merged.into_values().collect()
}

fn editor_tag_groups(
    available_tags: &[GalleryTagSummary],
    selected_tags: &[String],
) -> Vec<GalleryTagGroup> {
    let mut groups =
        group_gallery_tags(&merged_editor_tag_summaries(available_tags, selected_tags));
    let uncategorized_index = groups
        .iter()
        .position(|group| group.name == UNCATEGORIZED_TAG_CATEGORY);
    let uncategorized = uncategorized_index
        .map(|index| groups.remove(index))
        .unwrap_or_else(|| GalleryTagGroup {
            name: UNCATEGORIZED_TAG_CATEGORY.to_string(),
            tags: Vec::new(),
        });
    groups.insert(0, uncategorized);
    groups
}

fn toggle_editor_tag(
    editor: RwSignal<Option<TemplateEditorDraft>>,
    tag: &str,
) -> Result<(), String> {
    let expected = editor_tag_key(tag);
    let mut result = Ok(());
    editor.update(|draft| {
        let Some(draft) = draft else {
            result = Err("模板编辑器已关闭。".to_string());
            return;
        };
        if let Some(index) = draft
            .tags
            .iter()
            .position(|current| editor_tag_key(current) == expected)
        {
            draft.tags.remove(index);
            return;
        }
        if draft.tags.len() >= MAX_TEMPLATE_TAGS {
            result = Err(format!("每个模板最多使用 {MAX_TEMPLATE_TAGS} 个标签。"));
            return;
        }
        draft.tags.push(tag.to_string());
    });
    result
}

fn remove_editor_tag(editor: RwSignal<Option<TemplateEditorDraft>>, tag: &str) {
    let expected = editor_tag_key(tag);
    editor.update(|draft| {
        if let Some(draft) = draft {
            draft
                .tags
                .retain(|current| editor_tag_key(current) != expected);
        }
    });
}

fn submit_editor_tag_input(
    editor: RwSignal<Option<TemplateEditorDraft>>,
    ui_state: RwSignal<TemplateEditorTagUiState>,
) {
    let current = current_editor_tags(editor);
    let (new_category, new_tag_input) =
        ui_state.with_untracked(|state| (state.new_category.clone(), state.new_tag_input.clone()));
    let result = build_editor_tags(&new_category, &new_tag_input, &current);
    let additions = match result {
        Ok(additions) => additions,
        Err(error) => {
            ui_state.update(|state| state.feedback = Some(error));
            return;
        }
    };
    let first_category = gallery_tag_parts(&additions[0]).0.to_string();
    editor.update(|draft| {
        if let Some(draft) = draft {
            draft.tags.extend(additions);
        }
    });
    ui_state.update(|state| {
        state.selected_category = first_category.clone();
        state.new_category = if first_category == UNCATEGORIZED_TAG_CATEGORY {
            String::new()
        } else {
            first_category
        };
        state.new_tag_input.clear();
        state.feedback = None;
    });
}

fn build_editor_tags(
    category_input: &str,
    tag_input: &str,
    existing_tags: &[String],
) -> Result<Vec<String>, String> {
    let default_category = normalize_editor_category(category_input)?;
    let mut known = existing_tags
        .iter()
        .map(|tag| editor_tag_key(tag))
        .collect::<HashSet<_>>();
    let mut additions = Vec::new();
    for value in tag_input.split(is_editor_tag_batch_separator) {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (category, label) = parse_editor_tag_path(value, default_category)?;
        let path = category
            .map(|category| format!("{category}/{label}"))
            .unwrap_or_else(|| label.to_string())
            .to_lowercase();
        if path.chars().count() > MAX_TEMPLATE_TAG_CHARS {
            return Err(format!(
                "标签“{path}”超过 {MAX_TEMPLATE_TAG_CHARS} 个字符。"
            ));
        }
        if known.insert(editor_tag_key(&path)) {
            additions.push(path);
        }
    }
    if additions.is_empty() {
        return Err(if tag_input.trim().is_empty() {
            "请输入至少一个标签。".to_string()
        } else {
            "输入的标签已经全部添加。".to_string()
        });
    }
    if existing_tags.len().saturating_add(additions.len()) > MAX_TEMPLATE_TAGS {
        return Err(format!(
            "添加后将超过每个模板 {MAX_TEMPLATE_TAGS} 个标签的限制。"
        ));
    }
    Ok(additions)
}

fn normalize_editor_category(value: &str) -> Result<Option<&str>, String> {
    let value = value.trim();
    if value.is_empty() || value == UNCATEGORIZED_TAG_CATEGORY {
        return Ok(None);
    }
    validate_editor_tag_component("分类", value)?;
    Ok(Some(value))
}

fn parse_editor_tag_path<'a>(
    value: &'a str,
    default_category: Option<&'a str>,
) -> Result<(Option<&'a str>, &'a str), String> {
    let separator_count = value.chars().filter(|ch| matches!(ch, '/' | '／')).count();
    if separator_count > 1 {
        return Err(format!("标签“{value}”只能包含一级分类。"));
    }
    if separator_count == 1 {
        let separator = value.find(['/', '／']).unwrap_or_default();
        let category = value[..separator].trim();
        let label = value[separator..].trim_start_matches(['/', '／']).trim();
        if category.is_empty() || label.is_empty() {
            return Err(format!("标签路径“{value}”不完整。"));
        }
        validate_editor_tag_component("分类", category)?;
        validate_editor_tag_component("标签", label)?;
        return Ok((Some(category), label));
    }
    validate_editor_tag_component("标签", value)?;
    Ok((default_category, value))
}

fn validate_editor_tag_component(label: &str, value: &str) -> Result<(), String> {
    if value.chars().any(|ch| {
        matches!(
            ch,
            '/' | '／' | ',' | '，' | '、' | ';' | '；' | '\r' | '\n'
        )
    }) {
        return Err(format!("{label}不能包含斜杠、逗号、顿号、分号或换行。"));
    }
    Ok(())
}

fn is_editor_tag_batch_separator(ch: char) -> bool {
    matches!(ch, ',' | '，' | '、' | ';' | '；' | '\r' | '\n')
}

fn should_submit_editor_tag(key: &str, shift_key: bool, is_composing: bool) -> bool {
    key == "Enter" && !shift_key && !is_composing
}

#[component]
fn EditorAssets(
    title: &'static str,
    assets: Vec<GalleryAsset>,
    editor: RwSignal<Option<TemplateEditorDraft>>,
    role: GalleryAssetRole,
    max: usize,
    max_edge: u32,
    editor_busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
) -> impl IntoView {
    let asset_count = assets.len();
    view! { <div class="template-editor-assets"><div class="row"><strong>{title}</strong><label class="button secondary template-file-button"><MaterialSymbolIcon name="add_photo_alternate" filled=false />"上传"
        <input type="file" multiple accept="image/png,image/jpeg,image/webp" on:change=move |event: Event| {
            let input = event_target::<HtmlInputElement>(&event); let Some(files) = input.files() else { return; }; input.set_value("");
            upload_editor_files(files, role, max, max_edge, editor, editor_busy, message);
        } /></label></div>
        <div class="template-editor-asset-grid">
            {assets.into_iter().enumerate().map(|(index, asset)| {
                let remove_id = asset.id.clone();
                let move_previous_id = asset.id.clone();
                let move_next_id = asset.id.clone();
                view! {
                    <div>
                        <img src=gallery_thumbnail_url(&asset) alt="模板资源" />
                        <div class="template-editor-asset-actions">
                            <button class="button ghost icon-button" title="向前移动" disabled={index == 0}
                                on:click=move |_| move_editor_asset(editor, role, &move_previous_id, -1)>
                                <MaterialSymbolIcon name="chevron_left" filled=false />
                            </button>
                            <button class="button ghost icon-button" title="向后移动" disabled={index + 1 >= asset_count}
                                on:click=move |_| move_editor_asset(editor, role, &move_next_id, 1)>
                                <MaterialSymbolIcon name="chevron_right" filled=false />
                            </button>
                            <button class="button danger icon-button" title="移除" on:click=move |_| {
                                editor.update(|draft| {
                                    if let Some(draft) = draft {
                                        let items = editor_assets_mut(draft, role);
                                        items.retain(|item| item.id != remove_id);
                                    }
                                });
                            }>
                                <MaterialSymbolIcon name="close" filled=false />
                            </button>
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    </div> }
}

fn editor_assets_mut(
    draft: &mut TemplateEditorDraft,
    role: GalleryAssetRole,
) -> &mut Vec<GalleryAsset> {
    match role {
        GalleryAssetRole::Preview => &mut draft.preview_assets,
        GalleryAssetRole::Reference => &mut draft.reference_assets,
    }
}

fn move_editor_asset(
    editor: RwSignal<Option<TemplateEditorDraft>>,
    role: GalleryAssetRole,
    asset_id: &str,
    offset: isize,
) {
    editor.update(|draft| {
        let Some(draft) = draft else {
            return;
        };
        let assets = editor_assets_mut(draft, role);
        let Some(index) = assets.iter().position(|asset| asset.id == asset_id) else {
            return;
        };
        let Some(target) = index.checked_add_signed(offset) else {
            return;
        };
        if target < assets.len() {
            assets.swap(index, target);
        }
    });
}

fn default_editor_draft(workspace: WorkspaceState, composer: ComposerState) -> TemplateEditorDraft {
    let config = workspace.configs.with_untracked(|items| {
        items
            .iter()
            .find(|item| item.id == workspace.current_config_id.get_untracked())
            .cloned()
    });
    TemplateEditorDraft {
        id: None,
        title: String::new(),
        prompt: composer.draft_prompt.get_untracked(),
        description: String::new(),
        tags: Vec::new(),
        recommended_provider_kind: config
            .as_ref()
            .map(|config| config.provider_kind)
            .unwrap_or_default(),
        recommended_model: config
            .as_ref()
            .map(|config| config.model.clone())
            .unwrap_or_default(),
        generation_settings: GenerationSettingsSnapshot {
            width: composer.custom_width.get_untracked(),
            height: composer.custom_height.get_untracked(),
            quality: Some(composer.quality.get_untracked()),
            count: composer.count.get_untracked(),
            endpoint_mode: config
                .as_ref()
                .map(|config| config.endpoint_mode)
                .unwrap_or(ProviderEndpointMode::ImagesApi),
            output_format: config
                .as_ref()
                .and_then(|config| config.output_format.clone()),
            output_compression: config.as_ref().and_then(|config| config.output_compression),
            background: config.as_ref().and_then(|config| config.background.clone()),
            moderation: config.as_ref().and_then(|config| config.moderation.clone()),
            responses_model: config.and_then(|config| config.responses_model),
        },
        preview_assets: Vec::new(),
        reference_assets: Vec::new(),
        status: GalleryTemplateStatus::Draft,
    }
}

fn editor_request(draft: &TemplateEditorDraft) -> GalleryTemplateUpsertRequest {
    let mut generation_settings = draft.generation_settings.clone();
    let size = clamp_size(generation_settings.width, generation_settings.height);
    generation_settings.width = size.width;
    generation_settings.height = size.height;
    generation_settings.count = generation_settings.count.clamp(1, 4);
    GalleryTemplateUpsertRequest {
        id: draft.id.clone(),
        title: draft.title.clone(),
        prompt: draft.prompt.clone(),
        description: draft.description.clone(),
        tags: draft.tags.clone(),
        generation_settings,
        recommended_provider_kind: draft.recommended_provider_kind,
        recommended_model: draft.recommended_model.clone(),
        preview_asset_ids: draft
            .preview_assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
        reference_asset_ids: draft
            .reference_assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
        status: draft.status,
    }
}

fn upload_editor_files(
    files: web_sys::FileList,
    role: GalleryAssetRole,
    max: usize,
    max_edge: u32,
    editor: RwSignal<Option<TemplateEditorDraft>>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
) {
    let current = editor.with_untracked(|draft| {
        draft
            .as_ref()
            .map(|draft| {
                if role == GalleryAssetRole::Preview {
                    draft.preview_assets.len()
                } else {
                    draft.reference_assets.len()
                }
            })
            .unwrap_or(0)
    });
    if current + files.length() as usize > max {
        message.set(Some(format!("图片数量不能超过 {max} 张。")));
        return;
    }
    busy.set(true);
    spawn_local(async move {
        for index in 0..files.length() {
            let Some(file) = files.get(index) else {
                continue;
            };
            match process_and_upload_file(file, role, max_edge).await {
                Ok(asset) => editor.update(|draft| {
                    if let Some(draft) = draft {
                        if role == GalleryAssetRole::Preview {
                            draft.preview_assets.push(asset);
                        } else {
                            draft.reference_assets.push(asset);
                        }
                    }
                }),
                Err(error) => {
                    message.set(Some(error));
                    busy.set(false);
                    return;
                }
            }
            TimeoutFuture::new(0).await;
        }
        message.set(Some("模板图片已上传。".into()));
        busy.set(false);
    });
}

async fn process_and_upload_file(
    file: web_sys::File,
    role: GalleryAssetRole,
    max_edge: u32,
) -> Result<GalleryAsset, String> {
    if file.size() > 64.0 * 1024.0 * 1024.0 {
        return Err("单张模板图片不能超过 64 MiB。".into());
    }
    let bytes = read_as_bytes(&File::from(file))
        .await
        .map_err(|error| error.to_string())?;
    let mime = sniff_input_image(&bytes)?;
    let source = bytes_to_data_url(&bytes, mime);
    let encoded = reencode_gallery_source(&source, max_edge).await?;
    upload_gallery_webp(encoded, role).await
}

async fn upload_gallery_webp(
    bytes: Vec<u8>,
    role: GalleryAssetRole,
) -> Result<GalleryAsset, String> {
    let role_value = match role {
        GalleryAssetRole::Preview => "preview",
        GalleryAssetRole::Reference => "reference",
    };
    let request = Request::post(&api_url(&format!(
        "/api/admin/gallery/assets?role={role_value}"
    )))
    .credentials(web_sys::RequestCredentials::Include)
    .header("content-type", "image/webp")
    .body(bytes)
    .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let mut asset = response
        .json::<GalleryAsset>()
        .await
        .map_err(|error| error.to_string())?;
    asset.role = role;
    Ok(asset)
}

async fn reencode_gallery_source(source: &str, max_edge: u32) -> Result<Vec<u8>, String> {
    let image = load_html_image(source).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);
    let scale = (f64::from(max_edge) / f64::from(width.max(height))).min(1.0);
    let target_width = (f64::from(width) * scale).round().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).round().max(1.0) as u32;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "浏览器文档不可用。".to_string())?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "画布类型错误。".to_string())?;
    canvas.set_width(target_width);
    canvas.set_height(target_height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("读取画布失败：{error:?}"))?
        .ok_or_else(|| "浏览器不支持 2D 画布。".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element_and_dw_and_dh(
            &image,
            0.0,
            0.0,
            f64::from(target_width),
            f64::from(target_height),
        )
        .map_err(|error| format!("绘制图片失败：{error:?}"))?;
    let data_url = canvas
        .to_data_url_with_type_and_encoder_options(
            "image/webp",
            &JsValue::from_f64(TEMPLATE_IMAGE_QUALITY),
        )
        .map_err(|error| format!("WebP 编码失败：{error:?}"))?;
    let (mime, bytes) = crate::app::decode_browser_data_url(&data_url)?;
    if mime != "image/webp" {
        return Err("当前浏览器不支持 WebP 编码。".into());
    }
    Ok(bytes)
}

fn sniff_input_image(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        if bytes.windows(4).any(|window| window == b"ANIM") {
            return Err("暂不支持动画 WebP。".into());
        }
        return Ok("image/webp");
    }
    Err("只支持 PNG、JPEG 或静态 WebP。".into())
}

async fn prepare_local_assets(
    source_assets: &[GalleryAsset],
    source_task_id: Option<&str>,
    thread_id: &str,
    archive_only: bool,
) -> Result<(Vec<ImageAssetRef>, Vec<(String, String)>), String> {
    let mut assets = Vec::with_capacity(source_assets.len());
    let mut payloads = Vec::with_capacity(source_assets.len());
    for source in source_assets {
        let (bytes, mime) =
            crate::app::fetch_authenticated_image_bytes(&gallery_asset_url(source)).await?;
        if mime.split(';').next().unwrap_or_default() != source.mime_type
            || bytes.len() as u64 != source.byte_len
            || sha256_hex(&bytes) != source.sha256
        {
            return Err(format!("模板图片 `{}` 校验失败。", source.id));
        }
        let id = new_id();
        let now = now_rfc3339();
        let data_url = bytes_to_data_url(&bytes, &source.mime_type);
        let mut metadata = BTreeMap::new();
        metadata.insert("thread_id".into(), thread_id.into());
        if archive_only {
            metadata.insert(FAVORITE_ARCHIVE_ASSET_KEY.into(), "true".into());
        }
        assets.push(ImageAssetRef {
            id: id.clone(),
            sha256: source.sha256.clone(),
            mime_type: source.mime_type.clone(),
            byte_len: source.byte_len,
            width: Some(source.width),
            height: Some(source.height),
            created_at: now.clone(),
            updated_at: now,
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: source_task_id.map(str::to_string),
            metadata,
        });
        payloads.push((id, data_url));
        TimeoutFuture::new(0).await;
    }
    Ok((assets, payloads))
}

fn open_editor_from_task(
    task_id: String,
    workspace: WorkspaceState,
    composer: ComposerState,
    editor: RwSignal<Option<TemplateEditorDraft>>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
) {
    let Some(task) = workspace
        .tasks
        .with_untracked(|tasks| tasks.iter().find(|task| task.id == task_id).cloned())
    else {
        message.set(Some("找不到用于发布的任务。".into()));
        return;
    };
    let mut draft = default_editor_draft(workspace, composer);
    draft.prompt = task.prompt.clone();
    draft.recommended_model = task.requested_model.clone();
    if let Some(settings) = task.generation_settings.clone() {
        draft.generation_settings = settings;
    }
    editor.set(Some(draft));
    busy.set(true);
    spawn_local(async move {
        let result_ids = workspace.assets.with_untracked(|assets| {
            assets
                .iter()
                .filter(|asset| asset.source_task_id.as_deref() == Some(task.id.as_str()))
                .take(6)
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>()
        });
        let mut all_ids = result_ids.clone();
        all_ids.extend(task.reference_asset_ids.iter().cloned());
        let _ = ensure_asset_display_sources_loaded(workspace.assets, &all_ids).await;
        for (ids, role, edge) in [
            (result_ids, GalleryAssetRole::Preview, PREVIEW_MAX_EDGE),
            (
                task.reference_asset_ids,
                GalleryAssetRole::Reference,
                REFERENCE_MAX_EDGE,
            ),
        ] {
            for id in ids {
                let Some(asset) = workspace
                    .assets
                    .with_untracked(|assets| assets.iter().find(|asset| asset.id == id).cloned())
                else {
                    continue;
                };
                let source = asset_src(&asset);
                if source.is_empty() {
                    continue;
                }
                let encoded = match reencode_gallery_source(&source, edge).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        message.set(Some(error));
                        busy.set(false);
                        return;
                    }
                };
                match upload_gallery_webp(encoded, role).await {
                    Ok(uploaded) => editor.update(|draft| {
                        if let Some(draft) = draft {
                            if role == GalleryAssetRole::Preview {
                                draft.preview_assets.push(uploaded);
                            } else {
                                draft.reference_assets.push(uploaded);
                            }
                        }
                    }),
                    Err(error) => {
                        message.set(Some(error));
                        busy.set(false);
                        return;
                    }
                }
            }
        }
        busy.set(false);
        message.set(Some("已从生成结果预填模板草稿，请检查后保存。".into()));
    });
}

fn delete_admin_template(
    id: String,
    editor: RwSignal<Option<TemplateEditorDraft>>,
    templates: RwSignal<Vec<GalleryTemplate>>,
    message: RwSignal<Option<String>>,
    busy: RwSignal<bool>,
    reload_trigger: RwSignal<u64>,
) {
    busy.set(true);
    spawn_local(async move {
        match Request::delete(&api_url(&format!("/api/admin/gallery/templates/{id}")))
            .credentials(web_sys::RequestCredentials::Include)
            .send()
            .await
        {
            Ok(response) if response.ok() => {
                templates.update(|items| items.retain(|item| item.id != id));
                reload_trigger.update(|value| *value = value.saturating_add(1));
                editor.set(None);
                message.set(Some("模板已删除。".into()));
            }
            Ok(response) => message.set(Some(response_error(response).await)),
            Err(error) => message.set(Some(error.to_string())),
        }
        busy.set(false);
    });
}

async fn fetch_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let response = Request::get(&api_url(path))
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}
async fn response_error(response: gloo_net::http::Response) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("请求失败：HTTP {status} {text}"))
}
fn update_like_state(
    templates: RwSignal<Vec<GalleryTemplate>>,
    selected: RwSignal<Option<GalleryTemplate>>,
    result: &GalleryLikeResponse,
) {
    templates.update(|items| {
        if let Some(item) = items.iter_mut().find(|item| item.id == result.template_id) {
            item.like_count = result.like_count;
            item.liked_by_viewer = result.liked;
        }
    });
    selected.update(|item| {
        if let Some(item) = item.as_mut().filter(|item| item.id == result.template_id) {
            item.like_count = result.like_count;
            item.liked_by_viewer = result.liked;
        }
    });
}
fn gallery_asset_url(asset: &GalleryAsset) -> String {
    api_url(&format!(
        "/api/gallery/assets/{}?sha={}",
        asset.id, asset.sha256
    ))
}

fn gallery_thumbnail_url(asset: &GalleryAsset) -> String {
    api_url(&format!(
        "/api/gallery/assets/{}/thumbnail?sha={}",
        asset.id, asset.sha256
    ))
}

fn gallery_tag_parts(value: &str) -> (&str, &str) {
    let Some((category, label)) = value.split_once('/') else {
        return (UNCATEGORIZED_TAG_CATEGORY, value.trim());
    };
    let category = category.trim();
    let label = label.trim();
    if category.is_empty() || label.is_empty() {
        (UNCATEGORIZED_TAG_CATEGORY, value.trim())
    } else {
        (category, label)
    }
}

fn gallery_tag_label(value: &str) -> &str {
    gallery_tag_parts(value).1
}

fn gallery_tag_breadcrumb(value: &str) -> String {
    let (category, label) = gallery_tag_parts(value);
    if category == UNCATEGORIZED_TAG_CATEGORY {
        label.to_string()
    } else {
        format!("{category} · {label}")
    }
}

fn group_gallery_tags(tags: &[GalleryTagSummary]) -> Vec<GalleryTagGroup> {
    let mut grouped = BTreeMap::<String, Vec<GalleryTagSummary>>::new();
    for tag in tags {
        grouped
            .entry(gallery_tag_parts(&tag.name).0.to_string())
            .or_default()
            .push(tag.clone());
    }
    let mut groups = grouped
        .into_iter()
        .map(|(name, mut tags)| {
            tags.sort_by(|left, right| {
                right
                    .template_count
                    .cmp(&left.template_count)
                    .then_with(|| gallery_tag_label(&left.name).cmp(gallery_tag_label(&right.name)))
            });
            GalleryTagGroup { name, tags }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        (left.name == UNCATEGORIZED_TAG_CATEGORY)
            .cmp(&(right.name == UNCATEGORIZED_TAG_CATEGORY))
            .then_with(|| right.tags.len().cmp(&left.tags.len()))
            .then_with(|| left.name.cmp(&right.name))
    });
    groups
}

fn visible_gallery_tags(
    tags: &[GalleryTagSummary],
    category: Option<&str>,
    query: &str,
) -> Vec<GalleryTagSummary> {
    let Some(category) = category else {
        return Vec::new();
    };
    let query = query.trim().to_lowercase();
    let mut visible = tags
        .iter()
        .filter(|tag| gallery_tag_parts(&tag.name).0 == category)
        .filter(|tag| {
            query.is_empty()
                || tag.name.to_lowercase().contains(&query)
                || gallery_tag_label(&tag.name).to_lowercase().contains(&query)
        })
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| {
        right
            .template_count
            .cmp(&left.template_count)
            .then_with(|| gallery_tag_label(&left.name).cmp(gallery_tag_label(&right.name)))
    });
    visible
}

fn format_compact_like_count(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    if count < 10_000 {
        return format_count_with_decimal_unit(count, 1_000, "K");
    }
    if count < 1_000_000 {
        return format_count_with_decimal_unit(count, 10_000, "W");
    }
    if count < 10_000_000 {
        return format!("{}W", count / 10_000);
    }
    "999W+".into()
}

fn format_count_with_decimal_unit(count: u64, unit: u64, suffix: &str) -> String {
    let whole = count / unit;
    let decimal = count % unit / (unit / 10);
    if decimal == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}{suffix}{decimal}")
    }
}

fn is_gallery_template_favorite(tasks: &[LocalTaskRecord], template_id: &str) -> bool {
    tasks.iter().any(|task| {
        task.favorite && task.source_gallery_template_id.as_deref() == Some(template_id)
    })
}

fn prompt_excerpt(prompt: &str) -> String {
    let mut value = prompt.chars().take(110).collect::<String>();
    if prompt.chars().count() > 110 {
        value.push('…');
    }
    value
}
fn status_label(status: GalleryTemplateStatus) -> &'static str {
    match status {
        GalleryTemplateStatus::Draft => "草稿",
        GalleryTemplateStatus::Published => "已发布",
        GalleryTemplateStatus::Archived => "已归档",
    }
}
fn status_value(status: GalleryTemplateStatus) -> &'static str {
    match status {
        GalleryTemplateStatus::Draft => "draft",
        GalleryTemplateStatus::Published => "published",
        GalleryTemplateStatus::Archived => "archived",
    }
}
fn parse_status(value: &str) -> GalleryTemplateStatus {
    match value {
        "published" => GalleryTemplateStatus::Published,
        "archived" => GalleryTemplateStatus::Archived,
        _ => GalleryTemplateStatus::Draft,
    }
}

fn copy_text(text: String, message: RwSignal<Option<String>>) {
    spawn_local(async move {
        let result = if let Some(window) = web_sys::window() {
            JsFuture::from(window.navigator().clipboard().write_text(&text))
                .await
                .map(|_| ())
        } else {
            Err(JsValue::from_str("window unavailable"))
        };
        message.set(Some(if result.is_ok() {
            "已复制到剪贴板。".into()
        } else {
            "复制失败，请手动选择文本。".into()
        }));
    });
}
fn share_template(id: &str, message: RwSignal<Option<String>>) {
    let link = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .map(|origin| format!("{origin}/?view=templates&template={id}"))
        .unwrap_or_else(|| format!("/?view=templates&template={id}"));
    copy_text(link, message);
}
fn update_template_url(id: Option<&str>) {
    let url = id
        .map(|id| format!("/?view=templates&template={id}"))
        .unwrap_or_else(|| "/?view=templates".into());
    if let Some(window) = web_sys::window()
        && let Ok(history) = window.history()
    {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
    }
}
fn update_main_view_url() {
    if let Some(window) = web_sys::window()
        && let Ok(history) = window.history()
    {
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some("/"));
    }
}
fn template_id_from_location() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|part| part.strip_prefix("template="))
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, template_count: u64) -> GalleryTagSummary {
        GalleryTagSummary {
            name: name.into(),
            template_count,
        }
    }

    fn template_with_tags(tags: Vec<String>) -> GalleryTemplate {
        GalleryTemplate {
            id: "template".into(),
            title: "标题".into(),
            prompt: "提示词".into(),
            description: String::new(),
            tags,
            generation_settings: GenerationSettingsSnapshot {
                width: 1024,
                height: 1024,
                quality: None,
                count: 1,
                endpoint_mode: ProviderEndpointMode::ImagesApi,
                output_format: Some("png".into()),
                output_compression: None,
                background: None,
                moderation: None,
                responses_model: None,
            },
            recommended_provider_kind: ProviderKind::OpenAiImage,
            recommended_model: "gpt-image-2".into(),
            preview_assets: Vec::new(),
            reference_assets: Vec::new(),
            status: GalleryTemplateStatus::Draft,
            like_count: 0,
            liked_by_viewer: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn gallery_tags_are_grouped_by_category_path() {
        let groups = group_gallery_tags(&[
            tag("风格/赛博朋克", 3),
            tag("构图/特写", 2),
            tag("人像", 1),
            tag("风格/写实", 5),
        ]);

        assert_eq!(groups[0].name, "风格");
        assert_eq!(groups[0].tags[0].name, "风格/写实");
        assert_eq!(
            groups.last().map(|group| group.name.as_str()),
            Some("未分类")
        );
        assert_eq!(gallery_tag_breadcrumb("风格/赛博朋克"), "风格 · 赛博朋克");
    }

    #[test]
    fn tag_search_stays_inside_the_selected_category() {
        let tags = [
            tag("风格/写实", 4),
            tag("构图/写实光影", 3),
            tag("风格/水彩", 2),
        ];

        let visible = visible_gallery_tags(&tags, Some("风格"), "写实");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "风格/写实");
    }

    #[test]
    fn editor_tags_support_categories_chinese_separators_and_spaces() {
        let tags =
            build_editor_tags("风格", "二次元，concept art、厚涂；\n风格／水彩", &[]).unwrap();

        assert_eq!(
            tags,
            ["风格/二次元", "风格/concept art", "风格/厚涂", "风格/水彩"]
        );
    }

    #[test]
    fn editor_tags_keep_uncategorized_values_and_ignore_duplicates() {
        let tags = build_editor_tags(
            "",
            "人物 立绘，风格/写实，构图／特写",
            &["风格/写实".into()],
        )
        .unwrap();

        assert_eq!(tags, ["人物 立绘", "构图/特写"]);
    }

    #[test]
    fn editor_tag_validation_matches_backend_limits() {
        let existing = (0..MAX_TEMPLATE_TAGS)
            .map(|index| format!("标签{index}"))
            .collect::<Vec<_>>();
        assert!(build_editor_tags("", "新增", &existing).is_err());
        assert!(build_editor_tags("分类", &"字".repeat(30), &[]).is_err());
        assert!(build_editor_tags("错误/分类", "标签", &[]).is_err());
        assert!(build_editor_tags("", "分类/多/层", &[]).is_err());
    }

    #[test]
    fn editor_tag_groups_include_current_draft_and_uncategorized() {
        let groups =
            editor_tag_groups(&[tag("风格/写实", 2)], &["构图/特写".into(), "人物".into()]);

        assert_eq!(groups[0].name, UNCATEGORIZED_TAG_CATEGORY);
        assert!(groups.iter().any(|group| group.name == "构图"));
    }

    #[test]
    fn editor_tag_enter_ignores_ime_composition_and_shift_enter() {
        assert!(should_submit_editor_tag("Enter", false, false));
        assert!(!should_submit_editor_tag("Enter", false, true));
        assert!(!should_submit_editor_tag("Enter", true, false));
        assert!(!should_submit_editor_tag("Space", false, false));
    }

    #[test]
    fn editor_request_preserves_existing_tag_order_without_reparsing() {
        let original_tags = vec!["风格/写实".into(), "人物 立绘".into(), "旧/多/层".into()];
        let draft = TemplateEditorDraft::from_template(template_with_tags(original_tags.clone()));

        assert_eq!(editor_request(&draft).tags, original_tags);
    }

    #[test]
    fn submitted_search_filters_trim_and_keep_multiple_unique_tags() {
        let filters = normalized_gallery_search_filters(
            "  星空少女  ",
            &["风格/写实".into(), " 构图/特写 ".into(), "风格/写实".into()],
        );

        assert_eq!(filters.0, "星空少女");
        assert_eq!(filters.1, ["构图/特写", "风格/写实"]);
    }

    #[test]
    fn template_batches_reveal_eight_items_until_page_is_complete() {
        assert_eq!(next_template_visible_count(8, 24), 16);
        assert_eq!(next_template_visible_count(16, 24), 24);
        assert_eq!(next_template_visible_count(24, 24), 24);
        assert_eq!(next_template_visible_count(8, 13), 13);
    }

    #[test]
    fn like_count_uses_truncated_three_digit_abbreviations() {
        assert_eq!(format_compact_like_count(999), "999");
        assert_eq!(format_compact_like_count(1_200), "1K2");
        assert_eq!(format_compact_like_count(1_280), "1K2");
        assert_eq!(format_compact_like_count(1_300), "1K3");
        assert_eq!(format_compact_like_count(12_000), "1W2");
        assert_eq!(format_compact_like_count(999_999), "99W9");
        assert_eq!(format_compact_like_count(1_280_000), "128W");
    }

    #[test]
    fn gallery_template_favorite_state_requires_matching_active_snapshot() {
        let mut task = LocalTaskRecord {
            id: "snapshot".into(),
            thread_id: "thread".into(),
            config_id: "config".into(),
            prompt: "prompt".into(),
            requested_model: "model".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: None,
            favorite: true,
            favorite_folder_id: Some(DEFAULT_FAVORITE_FOLDER_ID.into()),
            detached_from_thread: true,
            source_gallery_template_id: Some("template-1".into()),
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };

        assert!(is_gallery_template_favorite(
            std::slice::from_ref(&task),
            "template-1"
        ));
        assert!(!is_gallery_template_favorite(
            std::slice::from_ref(&task),
            "template-2"
        ));
        task.favorite = false;
        assert!(!is_gallery_template_favorite(&[task], "template-1"));
    }
}
