use std::collections::BTreeMap;

use gloo_file::{File, futures::read_as_bytes};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::{prelude::*, task::spawn_local};
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
use web_sys::{
    Blob, BlobPropertyBag, Event, HtmlAnchorElement, HtmlCanvasElement, HtmlInputElement,
};

use crate::app::{
    FAVORITE_ARCHIVE_ASSET_KEY, asset_src, bytes_to_data_url, ensure_asset_display_sources_loaded,
    load_html_image, sha256_hex,
    state::{AccountState, ComposerState, MainView, UiState, WorkspaceState},
};
use crate::{api::api_url, storage::apply_asset_payload_changes};

use super::common::{MaterialSymbolIcon, PaginationControls};

const PREVIEW_MAX_EDGE: u32 = 2_048;
const REFERENCE_MAX_EDGE: u32 = 4_096;
const TEMPLATE_IMAGE_QUALITY: f64 = 0.9;
const UNCATEGORIZED_TAG_CATEGORY: &str = "未分类";

#[derive(Clone, Debug, PartialEq, Eq)]
struct GalleryTagGroup {
    name: String,
    tags: Vec<GalleryTagSummary>,
}

#[derive(Clone)]
struct TemplateEditorDraft {
    id: Option<String>,
    title: String,
    prompt: String,
    description: String,
    tags: String,
    recommended_provider_kind: ProviderKind,
    recommended_model: String,
    generation_settings: GenerationSettingsSnapshot,
    preview_assets: Vec<GalleryAsset>,
    reference_assets: Vec<GalleryAsset>,
    status: GalleryTemplateStatus,
}

impl TemplateEditorDraft {
    fn from_template(template: GalleryTemplate) -> Self {
        Self {
            id: Some(template.id),
            title: template.title,
            prompt: template.prompt,
            description: template.description,
            tags: template.tags.join(", "),
            recommended_provider_kind: template.recommended_provider_kind,
            recommended_model: template.recommended_model,
            generation_settings: template.generation_settings,
            preview_assets: template.preview_assets,
            reference_assets: template.reference_assets,
            status: template.status,
        }
    }
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
    let available_tags = RwSignal::new(Vec::<GalleryTagSummary>::new());
    let selected_tags = RwSignal::new(Vec::<String>::new());
    let search = RwSignal::new(String::new());
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
    let request_revision = RwSignal::new(0u64);
    let reload_trigger = RwSignal::new(0u64);
    let editor = RwSignal::new(None::<TemplateEditorDraft>);
    let editor_busy = RwSignal::new(false);
    let import_input = NodeRef::<leptos::html::Input>::new();
    let export_confirm = RwSignal::new(false);
    let replace_confirm_stage = RwSignal::new(0u8);

    let is_admin = Memo::new(move |_| {
        account.auth_user.with(|user| {
            user.as_ref()
                .is_some_and(|user| user.status == "approved" && user.role == "admin")
        })
    });

    Effect::new(move |_| {
        let _ = reload_trigger.get();
        let query = search.get();
        let tags = selected_tags.get();
        let sort_value = sort.get();
        let requested_page = page.get();
        let admin = is_admin.get();
        let revision = request_revision.get_untracked().saturating_add(1);
        request_revision.set(revision);
        loading.set(true);
        spawn_local(async move {
            if !query.trim().is_empty() {
                TimeoutFuture::new(300).await;
            }
            if request_revision.get_untracked() != revision {
                return;
            }
            let mut url = format!(
                "/api/gallery/templates?page={requested_page}&page_size=24&sort={sort_value}"
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
                    total_pages.set(usize::try_from(pages).unwrap_or(usize::MAX));
                    templates.set(response.items);
                    loading.set(false);
                }
                Err(error) if request_revision.get_untracked() == revision => {
                    message.set(Some(error));
                    loading.set(false);
                }
                _ => {}
            }
        });
    });

    Effect::new(move |_| {
        let _ = reload_trigger.get();
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
                    Ok(template) => selected_template.set(Some(template)),
                    Err(error) => message.set(Some(error)),
                }
            }
        });
    });

    Effect::new(move |_| {
        let Some(task_id) = ui.gallery_template_draft_task_id.get() else {
            return;
        };
        ui.gallery_template_draft_task_id.set(None);
        if !is_admin.get_untracked() {
            return;
        }
        open_editor_from_task(task_id, workspace, composer, editor, editor_busy, message);
    });

    let open_template = move |template: GalleryTemplate| {
        update_template_url(Some(&template.id));
        selected_template.set(Some(template));
    };
    let close_template = move |_| {
        selected_template.set(None);
        update_template_url(None);
    };
    let toggle_tag = move |tag: String| {
        selected_tags.update(|items| {
            if let Some(index) = items.iter().position(|item| item == &tag) {
                items.remove(index);
            } else {
                items.push(tag);
            }
        });
        page.set(1);
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
        move |template: GalleryTemplate| {
            let existing_task_id = workspace.tasks.with_untracked(|tasks| {
                tasks
                    .iter()
                    .find(|task| {
                        task.source_gallery_template_id.as_deref() == Some(template.id.as_str())
                    })
                    .map(|task| task.id.clone())
            });
            if let Some(existing_task_id) = existing_task_id {
                let favorite_folder_id = workspace.preferences.with_untracked(|preferences| {
                    preferences
                        .active_favorite_folder_id
                        .clone()
                        .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into())
                });
                workspace.tasks.update(|tasks| {
                    if let Some(task) = tasks.iter_mut().find(|task| task.id == existing_task_id) {
                        task.favorite = true;
                        task.favorite_folder_id = Some(favorite_folder_id);
                        task.updated_at = now_rfc3339();
                    }
                });
                persist_state();
                message.set(Some("该模板的本地快照已恢复到收藏夹。".into()));
                return;
            }
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
                let favorite_folder_id = workspace.preferences.with_untracked(|preferences| {
                    preferences
                        .active_favorite_folder_id
                        .clone()
                        .unwrap_or_else(|| DEFAULT_FAVORITE_FOLDER_ID.into())
                });
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
                message.set(Some(format!(
                    "已将“{}”收藏为本地独立快照。",
                    template.title
                )));
            });
        };

    let new_editor = move |_| {
        editor.set(Some(default_editor_draft(workspace, composer)));
    };
    let edit_template = move |template: GalleryTemplate| {
        editor.set(Some(TemplateEditorDraft::from_template(template)));
    };
    let export_templates = move || {
        spawn_local(async move {
            message.set(Some("正在导出模板广场……".into()));
            match Request::get(&api_url("/api/admin/gallery/export"))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                Ok(response) if response.ok() => match response.binary().await {
                    Ok(bytes) => {
                        if let Err(error) =
                            download_bytes(&bytes, "application/zip", "mew-gallery.zip")
                        {
                            message.set(Some(error));
                        } else {
                            message.set(Some("模板广场已导出。".into()));
                        }
                    }
                    Err(error) => message.set(Some(error.to_string())),
                },
                Ok(response) => message.set(Some(response_error(response).await)),
                Err(error) => message.set(Some(error.to_string())),
            }
        });
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
        spawn_local(async move {
            editor_busy.set(true);
            let path = match mode {
                GalleryImportMode::Merge => "/api/admin/gallery/import?mode=merge",
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
                                "已导入 {} 个模板和 {} 个资源。",
                                result.imported_template_count, result.imported_asset_count
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
                            <button class="button ghost icon-button template-admin-export" title="导出全部模板" aria-label="导出全部模板" on:click=move |_| export_confirm.set(true)>
                                <MaterialSymbolIcon name="download" filled=false />
                            </button>
                            <button class="button ghost icon-button template-admin-import" title="合并导入模板" aria-label="合并导入模板" on:click=move |_| {
                                replace_confirm_stage.set(0);
                                if let Some(input) = import_input.get() { input.click(); }
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
                    <label class="template-search">
                        <MaterialSymbolIcon name="search" filled=false />
                        <input type="search" placeholder="搜索标题、提示词或标签" prop:value=move || search.get()
                            on:input=move |event| { search.set(event_target_value(&event)); page.set(1); } />
                    </label>
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
                                        on:click=move |_| { selected_tags.set(Vec::new()); page.set(1); }>
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
                <For each=move || templates.get() key=|template| (
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
                                    <button class="button ghost icon-button" title="收藏到工作台" aria-label="收藏到工作台" on:click=move |_| favorite_template(favorite_value.clone())><MaterialSymbolIcon name="star" filled=false /></button>
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
            <PaginationControls page=page page_count=page_count favorite=false />
        </main>

        {move || message.get().map(|notice| view! {
            <div class="template-plaza-notice" role="status" aria-live="polite">
                <span>{notice}</span>
                <button class="button ghost icon-button" title="关闭提示" aria-label="关闭提示" on:click=move |_| message.set(None)>
                    <MaterialSymbolIcon name="close" filled=false />
                </button>
            </div>
        })}

        {move || selected_template.get().map(|template| {
            let like_id = template.id.clone(); let liked = template.liked_by_viewer;
            let use_value = template.clone(); let favorite_value = template.clone(); let prompt = template.prompt.clone();
            view! { <div class="modal-backdrop template-detail-backdrop" on:click=close_template>
                <article class="panel template-detail" on:click=move |event| event.stop_propagation()>
                    <button class="button ghost icon-button template-modal-close" on:click=close_template><MaterialSymbolIcon name="close" filled=false /></button>
                    <div class="template-detail-gallery">{template.preview_assets.iter().map(|asset| view! { <img src=gallery_asset_url(asset) alt=template.title.clone() /> }).collect_view()}</div>
                    <div class="template-detail-content stack"><div><span class="template-plaza-kicker">"GALLERY TEMPLATE"</span><h2>{template.title.clone()}</h2></div>
                        <p>{template.description.clone()}</p>
                        <div class="template-card-tags">{template.tags.iter().map(|tag| view! {
                            <span class="tag" title=tag.clone()>{gallery_tag_label(tag).to_string()}</span>
                        }).collect_view()}</div>
                        <div class="template-prompt-box"><strong>"提示词"</strong><p>{template.prompt.clone()}</p>
                            <button class="button ghost" on:click=move |_| copy_text(prompt.clone(), message)><MaterialSymbolIcon name="content_copy" filled=false />"复制"</button>
                        </div>
                        {if template.reference_assets.is_empty() {
                            ().into_any()
                        } else {
                            view! { <div class="template-reference-strip"><strong>"参考图"</strong><div>{template.reference_assets.iter().map(|asset| view! { <img src=gallery_thumbnail_url(asset) alt="模板参考图" loading="lazy" /> }).collect_view()}</div></div> }.into_any()
                        }}
                        <dl class="template-parameters"><div><dt>"推荐模型"</dt><dd>{template.recommended_model.clone()}</dd></div><div><dt>"尺寸"</dt><dd>{format!("{} × {}", template.generation_settings.width, template.generation_settings.height)}</dd></div><div><dt>"质量"</dt><dd>{template.generation_settings.quality.clone().unwrap_or_else(|| "自动".into())}</dd></div><div><dt>"参考图"</dt><dd>{format!("{} 张", template.reference_assets.len())}</dd></div></dl>
                        <div class="template-detail-actions"><button class="button ghost" class:is-active=liked on:click=move |_| toggle_like(like_id.clone(), liked)><MaterialSymbolIcon name="favorite" filled=liked />{template.like_count}</button>
                            <button class="button secondary" on:click=move |_| share_template(&template.id, message)><MaterialSymbolIcon name="share" filled=false />"复制分享链接"</button>
                            <button class="button secondary" on:click=move |_| favorite_template(favorite_value.clone())><MaterialSymbolIcon name="star" filled=false />"收藏到工作台"</button>
                            <button class="button primary" on:click=move |_| use_template(use_value.clone())>"使用模板"</button></div>
                    </div>
                </article>
            </div> }
        })}

        {move || editor.get().map(|draft| view! {
            <TemplateEditor draft editor editor_busy message templates reload_trigger />
        })}

        <Show when=move || export_confirm.get()>
            <div class="modal-backdrop" on:click=move |_| export_confirm.set(false)>
                <div class="panel confirm-dialog" on:click=move |event| event.stop_propagation()>
                    <h3>"导出全部模板"</h3>
                    <p>"将导出模板广场中的草稿、已发布和已归档模板，以及关联的预览图和参考图。是否继续？"</p>
                    <div class="row">
                        <button class="button ghost" on:click=move |_| export_confirm.set(false)>"取消"</button>
                        <button class="button primary" on:click=move |_| {
                            export_confirm.set(false);
                            export_templates();
                        }>"确认导出"</button>
                    </div>
                </div>
            </div>
        </Show>

        <Show when=move || { replace_confirm_stage.get() > 0 }>
            <div class="modal-backdrop" on:click=move |_| replace_confirm_stage.set(0)>
                <div class="panel confirm-dialog" on:click=move |event| event.stop_propagation()>
                    <h3>"全量替换模板广场"</h3>
                    <p>{move || if replace_confirm_stage.get() == 1 { "导入成功后，包外模板及全部点赞会被清除。请再次确认。" } else { "这是最后一步确认。现有模板资源将在完整校验成功后被替换。" }}</p>
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
    editor_busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
    templates: RwSignal<Vec<GalleryTemplate>>,
    reload_trigger: RwSignal<u64>,
) -> impl IntoView {
    let delete_confirm = RwSignal::new(false);
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
            <div class="row"><div><span class="template-plaza-kicker">"ADMIN EDITOR"</span><h2>{if draft.id.is_some() { "编辑模板" } else { "新建模板" }}</h2></div>
                <button class="button ghost icon-button" on:click=move |_| editor.set(None)><MaterialSymbolIcon name="close" filled=false /></button></div>
            <label>"标题"<input class="text-input" prop:value=draft.title on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.title = event_target_value(&event) }) /></label>
            <label>"提示词"<textarea class="text-input template-editor-prompt" prop:value=draft.prompt on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.prompt = event_target_value(&event) }) /></label>
            <label>"说明"<textarea class="text-input" prop:value=draft.description on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.description = event_target_value(&event) }) /></label>
            <label>"标签（逗号分隔）"<input class="text-input" placeholder="例如：风格/赛博朋克，构图/特写" prop:value=draft.tags on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.tags = event_target_value(&event) }) />
                <small class="muted">"使用“分类/标签”归类；没有分类路径的旧标签会显示在“未分类”。"</small>
            </label>
            <div class="template-editor-fields">
                <label>"推荐服务商"<select class="select-input" prop:value=provider_kind_value(draft.recommended_provider_kind) on:change=move |event| editor.update(|draft| if let Some(draft) = draft { draft.recommended_provider_kind = parse_provider_kind(&event_target_value(&event)) })><option value="openai_image">"OpenAI Images"</option><option value="nano_banana">"Nano Banana"</option><option value="openai_compatible">"OpenAI 兼容"</option></select></label>
                <label>"推荐模型"<input class="text-input" prop:value=draft.recommended_model on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.recommended_model = event_target_value(&event) }) /></label>
                <label>"状态"<select class="select-input" prop:value=status_value(draft.status) on:change=move |event| editor.update(|draft| if let Some(draft) = draft { draft.status = parse_status(&event_target_value(&event)) })><option value="draft">"草稿"</option><option value="published">"已发布"</option><option value="archived">"已归档"</option></select></label>
                <label>"宽度"<input class="text-input" type="number" min="1" prop:value=draft.generation_settings.width on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.generation_settings.width = event_target_value(&event).parse().unwrap_or(1024) }) /></label>
                <label>"高度"<input class="text-input" type="number" min="1" prop:value=draft.generation_settings.height on:input=move |event| editor.update(|draft| if let Some(draft) = draft { draft.generation_settings.height = event_target_value(&event).parse().unwrap_or(1024) }) /></label>
            </div>
            <EditorAssets title="预览图（最多 6 张）" assets=draft.preview_assets editor role=GalleryAssetRole::Preview max=6 max_edge=PREVIEW_MAX_EDGE editor_busy message />
            <EditorAssets title="参考图（最多 16 张）" assets=draft.reference_assets editor role=GalleryAssetRole::Reference max=16 max_edge=REFERENCE_MAX_EDGE editor_busy message />
            <div class="row template-editor-actions">
                {delete_id.get_value().map(|_| view! { <button class="button danger" disabled=move || editor_busy.get() on:click=move |_| delete_confirm.set(true)><MaterialSymbolIcon name="delete" filled=false />"删除模板"</button> })}
                <span class="spacer"></span><button class="button ghost" on:click=move |_| editor.set(None)>"取消"</button><button class="button primary" disabled=move || editor_busy.get() on:click=save>"保存模板"</button>
            </div>
            <Show when=move || delete_confirm.get()>
                <div class="template-inline-confirm">
                    <p>"确定删除这个模板吗？不再被其他模板引用的广场图片也会一并清理。"</p>
                    <div class="row"><button class="button ghost" on:click=move |_| delete_confirm.set(false)>"取消"</button>
                        {move || delete_id.get_value().map(|target_id| {
                            view! { <button class="button danger" on:click=move |_| delete_admin_template(target_id.clone(), editor, templates, message, editor_busy, reload_trigger)>"确认删除"</button> }
                        })}
                    </div>
                </div>
            </Show>
        </section>
    </div> }
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
        tags: String::new(),
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
        tags: draft
            .tags
            .split([',', '，'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect(),
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
fn provider_kind_value(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiImage => "openai_image",
        ProviderKind::NanoBanana => "nano_banana",
        ProviderKind::OpenAiCompatible => "openai_compatible",
        ProviderKind::CustomHttp => "openai_compatible",
    }
}
fn parse_provider_kind(value: &str) -> ProviderKind {
    match value {
        "nano_banana" => ProviderKind::NanoBanana,
        "openai_compatible" => ProviderKind::OpenAiCompatible,
        _ => ProviderKind::OpenAiImage,
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

fn download_bytes(bytes: &[u8], mime: &str, file_name: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用。".to_string())?;
    let document = window
        .document()
        .ok_or_else(|| "浏览器文档不可用。".to_string())?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let options = BlobPropertyBag::new();
    options.set_type(mime);
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|error| format!("创建下载文件失败：{error:?}"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|error| format!("创建下载地址失败：{error:?}"))?;
    let anchor: HtmlAnchorElement = document
        .create_element("a")
        .map_err(|error| format!("创建下载按钮失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "下载按钮类型错误。".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
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
    fn like_count_uses_truncated_three_digit_abbreviations() {
        assert_eq!(format_compact_like_count(999), "999");
        assert_eq!(format_compact_like_count(1_200), "1K2");
        assert_eq!(format_compact_like_count(1_280), "1K2");
        assert_eq!(format_compact_like_count(1_300), "1K3");
        assert_eq!(format_compact_like_count(12_000), "1W2");
        assert_eq!(format_compact_like_count(999_999), "99W9");
        assert_eq!(format_compact_like_count(1_280_000), "128W");
    }
}
