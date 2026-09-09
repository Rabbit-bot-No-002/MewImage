use super::*;
use mew_image_shared::{GalleryExportFilter, GalleryExportPreview, GalleryExportScope};

pub(super) fn supports_import_conflict(health: &serde_json::Value) -> bool {
    health["capabilities"]["gallery_import_conflict"].as_bool() == Some(true)
}

fn valid_selection(filter: &GalleryExportFilter) -> bool {
    filter.scope == GalleryExportScope::Backup
        || (!filter.statuses.is_empty()
            && (filter.uncategorized || !filter.categories.is_empty() || !filter.tags.is_empty()))
}

fn category_selected(filter: &GalleryExportFilter, category: &str) -> bool {
    if category == UNCATEGORIZED_TAG_CATEGORY {
        filter.uncategorized
    } else {
        filter.categories.iter().any(|value| value == category)
    }
}

fn toggle_value<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if let Some(index) = values.iter().position(|current| *current == value) {
        values.remove(index);
    } else {
        values.push(value);
    }
}

fn toggle_category(filter: &mut GalleryExportFilter, category: &str) {
    if category == UNCATEGORIZED_TAG_CATEGORY {
        filter.uncategorized = !filter.uncategorized;
    } else {
        toggle_value(&mut filter.categories, category.to_string());
    }
    // 全分类代替零散标签，取消全分类时不会暗中恢复旧标签选择。
    filter
        .tags
        .retain(|tag| gallery_tag_parts(tag).0 != category);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_backend_cannot_silently_ignore_keep_local() {
        assert!(!supports_import_conflict(&serde_json::json!({"ok": true})));
        assert!(!supports_import_conflict(
            &serde_json::json!({"capabilities": {"gallery_import_conflict": false}})
        ));
        assert!(supports_import_conflict(
            &serde_json::json!({"capabilities": {"gallery_import_conflict": true}})
        ));
    }

    #[test]
    fn category_selection_absorbs_tags_without_restoring_them_on_removal() {
        let mut filter = GalleryExportFilter {
            tags: vec!["海报/商业".into(), "用途/动漫".into()],
            ..Default::default()
        };
        toggle_category(&mut filter, "海报");
        assert!(category_selected(&filter, "海报"));
        assert_eq!(filter.tags, ["用途/动漫"]);
        toggle_category(&mut filter, "海报");
        assert!(!category_selected(&filter, "海报"));
        assert_eq!(filter.tags, ["用途/动漫"]);
    }

    #[test]
    fn sharing_requires_scope_and_status_while_backup_is_explicit() {
        let mut filter = GalleryExportFilter::default();
        assert!(!valid_selection(&filter));
        toggle_category(&mut filter, UNCATEGORIZED_TAG_CATEGORY);
        assert!(valid_selection(&filter));
        filter.statuses.clear();
        assert!(!valid_selection(&filter));
        filter.scope = GalleryExportScope::Backup;
        assert!(valid_selection(&filter));
    }
}

async fn export_preview(filter: &GalleryExportFilter) -> Result<GalleryExportPreview, String> {
    let response = Request::post(&api_url("/api/admin/gallery/export-preview"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(filter)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}

async fn download_selection(filter: &GalleryExportFilter) -> Result<(), String> {
    let response = Request::post(&api_url("/api/admin/gallery/export"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(filter)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let response: web_sys::Response = response.into();
    let blob = JsFuture::from(response.blob().map_err(|_| "无法读取模板包")?)
        .await
        .map_err(|_| "模板包下载中断")?
        .dyn_into::<Blob>()
        .map_err(|_| "模板包格式异常")?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| "无法创建模板包下载链接".to_string())?;
    let result = (|| {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("无法访问页面")?;
        let anchor = document
            .create_element("a")
            .map_err(|_| "无法创建下载入口")?
            .dyn_into::<HtmlAnchorElement>()
            .map_err(|_| "无法创建下载入口")?;
        let purpose = if filter.scope == GalleryExportScope::Backup {
            "backup"
        } else {
            "share"
        };
        anchor.set_href(&url);
        anchor.set_download(&format!(
            "mew-gallery-{purpose}-{}.zip",
            now_rfc3339()
                .chars()
                .take(10)
                .filter(|ch| *ch != '-')
                .collect::<String>()
        ));
        let body = document.body().ok_or("无法访问页面")?;
        body.append_child(&anchor).map_err(|_| "无法创建下载入口")?;
        anchor.click();
        anchor.remove();
        Ok::<_, String>(())
    })();
    // 延迟释放，给浏览器下载进程留出领取 Blob 的时间。
    spawn_local(async move {
        TimeoutFuture::new(30_000).await;
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    result
}

#[component]
pub(super) fn TemplateExportDialog(
    open: RwSignal<bool>,
    busy: RwSignal<bool>,
    tags: RwSignal<Vec<GalleryTagSummary>>,
    tags_loading: RwSignal<bool>,
    tags_error: RwSignal<Option<String>>,
    reload: RwSignal<u64>,
    message: RwSignal<Option<String>>,
) -> impl IntoView {
    let filter = RwSignal::new(GalleryExportFilter::default());
    let category = RwSignal::new(UNCATEGORIZED_TAG_CATEGORY.to_string());
    let search = RwSignal::new(String::new());
    let preview = RwSignal::new(None::<GalleryExportPreview>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let revision = RwSignal::new(0u64);
    let retry = RwSignal::new(0u64);
    Effect::new(move |_| {
        let selection = filter.get();
        let _ = retry.get();
        let current_revision = revision.get_untracked().wrapping_add(1);
        revision.set(current_revision);
        preview.set(None);
        error.set(None);
        loading.set(valid_selection(&selection));
        if !valid_selection(&selection) {
            return;
        }
        spawn_local(async move {
            TimeoutFuture::new(300).await;
            if revision.try_get_untracked() != Some(current_revision) {
                return;
            }
            let result = export_preview(&selection).await;
            if revision.try_get_untracked() != Some(current_revision) {
                return;
            }
            match result {
                Ok(value) => preview.set(Some(value)),
                Err(value) => error.set(Some(value)),
            }
            loading.set(false);
        });
    });
    let ready = Memo::new(move |_| {
        valid_selection(&filter.get())
            && !loading.get()
            && !busy.get()
            && preview.with(|value| value.as_ref().is_some_and(|value| value.template_count > 0))
            && (filter.with(|value| value.scope == GalleryExportScope::Backup)
                || (!tags_loading.get() && tags_error.with(Option::is_none)))
    });
    let submit = move |_| {
        if !ready.get_untracked() {
            return;
        }
        busy.set(true);
        let selection = filter.get_untracked();
        spawn_local(async move {
            let result = download_selection(&selection).await;
            match result {
                Ok(()) => {
                    message.set(Some("模板包已导出。".into()));
                    open.set(false);
                }
                Err(value) => {
                    let _ = error.try_set(Some(value));
                }
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <div class="modal-backdrop template-transfer-backdrop" on:click=move |_| { if !busy.get_untracked() { open.set(false); } }>
            <section class="template-editor-tag-dialog template-export-dialog" role="dialog" aria-modal="true" aria-labelledby="gallery-export-title" on:click=move |event| event.stop_propagation()>
                <header class="template-editor-tag-dialog-header"><h3 id="gallery-export-title">"导出模板"</h3>
                    <button class="button ghost icon-button" aria-label="关闭导出" disabled=move || busy.get() on:click=move |_| open.set(false)><MaterialSymbolIcon name="close" filled=false /></button>
                </header>
                <fieldset disabled=move || busy.get() class="template-transfer-fields">
                    <div class="row">
                        <button class="button secondary" class:is-active=move || filter.with(|value| value.scope == GalleryExportScope::Share) on:click=move |_| filter.update(|value| value.scope = GalleryExportScope::Share)>"分类分享"</button>
                        <button class="button secondary" class:is-active=move || filter.with(|value| value.scope == GalleryExportScope::Backup) on:click=move |_| filter.update(|value| value.scope = GalleryExportScope::Backup)>"完整备份"</button>
                    </div>
                    <Show when=move || filter.with(|value| value.scope == GalleryExportScope::Share) fallback=|| view! { <p>"包含全部分类的草稿、已发布和已归档模板，以及完整图片资源。"</p> }>
                        <div class="row template-export-statuses">
                            {[GalleryTemplateStatus::Published, GalleryTemplateStatus::Draft, GalleryTemplateStatus::Archived].into_iter().map(|status| view! {
                                <label><input type="checkbox" prop:checked=move || filter.with(|value| value.statuses.contains(&status)) on:change=move |_| filter.update(|value| toggle_value(&mut value.statuses, status)) />{match status { GalleryTemplateStatus::Published => "已发布", GalleryTemplateStatus::Draft => "草稿", GalleryTemplateStatus::Archived => "已归档" }}</label>
                            }).collect_view()}
                        </div>
                        <label class="template-tag-search"><MaterialSymbolIcon name="search" filled=false /><input type="search" placeholder="搜索分类或当前分类中的标签" prop:value=move || search.get() on:input=move |event| search.set(event_target_value(&event)) /></label>
                        <div class="template-tag-browser template-editor-tag-browser">
                            <nav class="template-tag-categories" aria-label="导出分类">
                                <For each=move || {
                                    let query = search.get().trim().to_lowercase();
                                    editor_tag_groups(&tags.get(), &[]).into_iter().filter(|group| query.is_empty() || group.name.to_lowercase().contains(&query) || group.tags.iter().any(|tag| tag.name.to_lowercase().contains(&query))).collect::<Vec<_>>()
                                } key=|group| group.name.clone() children=move |group| {
                                    let select_name = group.name.clone();
                                    let check_name = group.name.clone();
                                    let active_name = group.name.clone();
                                    let toggle_name = group.name.clone();
                                    view! { <div class="template-export-category-row">
                                        <input type="checkbox" aria-label=format!("导出整个{}分类", group.name) prop:checked=move || filter.with(|value| category_selected(value, &check_name)) on:change=move |_| filter.update(|value| toggle_category(value, &toggle_name)) />
                                        <button class="template-tag-category" class:is-active=move || category.get() == active_name on:click=move |_| category.set(select_name.clone())>{group.name}</button>
                                    </div> }
                                } />
                            </nav>
                            <div class="template-tag-options">
                                <For each=move || visible_gallery_tags(&tags.get(), Some(&category.get()), &search.get()) key=|tag| tag.name.clone() children=move |tag| {
                                    let selected = tag.name.clone();
                                    let disabled = tag.name.clone();
                                    let name = tag.name.clone();
                                    view! { <button class="template-tag-option" class:is-active=move || filter.with(|value| value.tags.contains(&selected) || category_selected(value, gallery_tag_parts(&selected).0)) disabled=move || filter.with(|value| category_selected(value, gallery_tag_parts(&disabled).0)) on:click=move |_| filter.update(|value| toggle_value(&mut value.tags, name.clone()))><span>{gallery_tag_label(&tag.name).to_string()}</span><small>{tag.template_count}</small></button> }
                                } />
                                <Show when=move || visible_gallery_tags(&tags.get(), Some(&category.get()), &search.get()).is_empty()><p class="template-tag-empty">"没有匹配的标签；未分类也包含没有标签的模板。"</p></Show>
                            </div>
                        </div>
                        <div class="template-export-selection">
                            {move || {
                                let value = filter.get();
                                let mut names = value.categories.iter().map(|name| format!("{name} · 全分类")).collect::<Vec<_>>();
                                if value.uncategorized { names.push("未分类 · 全分类".into()); }
                                names.extend(value.tags.iter().map(|tag| gallery_tag_breadcrumb(tag)));
                                if names.is_empty() { "请选择分类或标签".into() } else { names.join("、") }
                            }}
                        </div>
                        <Show when=move || tags_loading.get()><p>"正在加载管理员标签目录…"</p></Show>
                        {move || tags_error.get().map(|value| view! { <p class="template-editor-tag-note is-error">{value}<button class="button ghost" on:click=move |_| reload.update(|value| *value = value.wrapping_add(1))>"重试目录"</button></p> })}
                    </Show>
                </fieldset>
                <div class="template-export-summary" aria-live="polite">
                    {move || preview.get().map(|value| format!("匹配 {} 个模板 · {} 个原图资源 · 原图合计 {:.2} MiB（非 ZIP 大小）", value.template_count, value.asset_count, value.asset_byte_len as f64 / 1_048_576.0))}
                    <Show when=move || loading.get()>"正在计算导出范围…"</Show>
                    {move || error.get().map(|value| view! { <p class="template-editor-tag-note is-error">{value}<button class="button ghost" disabled=move || busy.get() on:click=move |_| retry.update(|value| *value = value.wrapping_add(1))>"重试"</button></p> })}
                </div>
                <footer class="template-editor-tag-dialog-actions"><span>"匹配任意所选分类或标签；图片和全部参数一同打包。"</span><button class="button primary" disabled=move || !ready.get() on:click=submit>{move || if busy.get() { "正在打包…" } else { "确认导出" }}</button></footer>
            </section>
        </div>
    }
}
