use std::collections::HashSet;

use gloo_net::http::Request;
use leptos::{ev, leptos_dom::helpers::window_event_listener, prelude::*, task::spawn_local};
use mew_image_shared::{
    AdminUserSummary, AdminUsersResponse, ManagedAccountCreateRequest,
    ManagedAccountCreateResponse, ManagedPasswordResetResponse, ManagedProviderAdminListResponse,
    ManagedProviderAdminView, ManagedProviderBulkCredentialsRequest, ManagedProviderConfigInput,
    ManagedProviderConfigWriteRequest, ManagedProviderEnabledRequest,
    ManagedProviderMutationResponse, ManagedProviderTemplateAdminView,
    ManagedProviderTemplateListResponse, ManagedProviderTemplateMutationResponse,
    ManagedProviderTemplateTargetsRequest, ManagedProviderTemplateWriteRequest,
    ProviderEndpointMode, ProviderKind, ProviderTemplate,
};

use crate::{
    api::api_url,
    app::{
        components::common::MaterialSymbolIcon,
        state::{AccountState, UiState, WorkspaceState},
    },
};

async fn response_error(response: gloo_net::http::Response, fallback: &str) -> String {
    response.text().await.unwrap_or_else(|_| fallback.into())
}

async fn reload_managed_admin_configs(account: AccountState) -> Result<(), String> {
    account.loading_admin_managed_providers.set(true);
    let result = Request::get(&api_url("/api/admin/managed-providers"))
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await;
    let output = match result {
        Ok(response) if response.ok() => response
            .json::<ManagedProviderAdminListResponse>()
            .await
            .map(|payload| {
                let configs = payload.configs;
                account.admin_users.update(|users| {
                    for user in users
                        .iter_mut()
                        .filter(|user| user.account_kind == mew_image_shared::AccountKind::Managed)
                    {
                        user.managed_provider_count = configs
                            .iter()
                            .filter(|config| config.user_id == user.id)
                            .count();
                    }
                });
                account.admin_managed_providers.set(configs);
            })
            .map_err(|error| format!("托管配置响应解析失败：{error}")),
        Ok(response) => Err(response_error(response, "托管配置加载失败。").await),
        Err(error) => Err(format!("托管配置加载失败：{error}")),
    };
    account.loading_admin_managed_providers.set(false);
    output
}

fn parse_endpoint_mode(value: &str) -> ProviderEndpointMode {
    match value {
        "responses_api" => ProviderEndpointMode::ResponsesApi,
        "custom_json" => ProviderEndpointMode::CustomJson,
        _ => ProviderEndpointMode::ImagesApi,
    }
}

fn endpoint_mode_value(value: ProviderEndpointMode) -> &'static str {
    match value {
        ProviderEndpointMode::ImagesApi => "images_api",
        ProviderEndpointMode::ResponsesApi => "responses_api",
        ProviderEndpointMode::CustomJson => "custom_json",
    }
}

fn split_models(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    value
        .split([',', '，', ';', '；', '\n'])
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .filter(|model| seen.insert(model.to_string()))
        .map(str::to_string)
        .collect()
}

fn config_input(
    name: String,
    template: ProviderTemplate,
    endpoint_mode: String,
    base_url: String,
    models_text: String,
    current_model: String,
    responses_model: String,
) -> Result<ManagedProviderConfigInput, String> {
    let models = split_models(&models_text);
    let current_model = current_model.trim().to_string();
    if name.trim().is_empty() || base_url.trim().is_empty() {
        return Err("请填写配置名称和上游地址。".into());
    }
    if models.is_empty() || !models.iter().any(|model| model == &current_model) {
        return Err("模型列表不能为空，且当前模型必须位于列表中。".into());
    }
    Ok(ManagedProviderConfigInput {
        name: name.trim().into(),
        template,
        endpoint_mode: parse_endpoint_mode(&endpoint_mode),
        base_url: base_url.trim().into(),
        available_models: models,
        current_model,
        responses_model: (!responses_model.trim().is_empty())
            .then(|| responses_model.trim().to_string()),
        output_format: Some("png".into()),
        output_compression: Some(100),
        background: Some("opaque".into()),
        moderation: Some("auto".into()),
        prompt_guard_enabled: false,
    })
}

#[component]
pub(crate) fn ManagedAccountsAdmin() -> impl IntoView {
    let account = expect_context::<AccountState>();
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let message = RwSignal::new(None::<String>);
    let temporary_password = RwSignal::new(None::<String>);
    let selected_ids = RwSignal::new(HashSet::<String>::new());
    let form_mode = RwSignal::new(None::<String>);
    let username = RwSignal::new(String::new());
    let target_user_id = RwSignal::new(String::new());
    let account_template_id = RwSignal::new(String::new());
    let managed_templates = RwSignal::new(Vec::<ManagedProviderTemplateAdminView>::new());
    let managed_account_users = RwSignal::new(Vec::<AdminUserSummary>::new());
    let config_name = RwSignal::new(String::new());
    let template_id = RwSignal::new(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into());
    let endpoint_mode = RwSignal::new("images_api".to_string());
    let base_url = RwSignal::new("https://api.openai.com".to_string());
    let api_key = RwSignal::new(String::new());
    let models_text =
        RwSignal::new("gpt-image-2, gpt-image-2.5-flare, gpt-image-2.5-sunburst".to_string());
    let current_model = RwSignal::new("gpt-image-2".to_string());
    let responses_model = RwSignal::new(String::new());
    let bulk_base_url = RwSignal::new(String::new());
    let bulk_api_key = RwSignal::new(String::new());
    let pending_config_delete = RwSignal::new(None::<ManagedProviderAdminView>);
    let pending_password_reset = RwSignal::new(None::<ManagedProviderAdminView>);
    let busy = RwSignal::new(false);

    let escape_listener = window_event_listener(ev::keydown, move |event| {
        if event.key() != "Escape" {
            return;
        }
        let handled = if temporary_password.get_untracked().is_some() {
            temporary_password.set(None);
            form_mode.set(None);
            true
        } else if pending_password_reset.get_untracked().is_some() {
            pending_password_reset.set(None);
            true
        } else if pending_config_delete.get_untracked().is_some() {
            pending_config_delete.set(None);
            true
        } else if form_mode.get_untracked().is_some() && !busy.get_untracked() {
            form_mode.set(None);
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

    Effect::new(move |_| {
        if account
            .auth_user
            .get()
            .is_some_and(|user| user.role == "admin")
        {
            spawn_local(async move {
                if let Err(error) = reload_managed_admin_configs(account).await {
                    message.set(Some(error));
                }
                if let Ok(response) = Request::get(&api_url(
                    "/api/admin/managed-provider-templates?page=1&limit=100",
                ))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
                    && response.ok()
                    && let Ok(payload) =
                        response.json::<ManagedProviderTemplateListResponse>().await
                {
                    managed_templates.set(payload.templates);
                }
                if let Ok(response)=Request::get(&api_url("/api/admin/users?page=1&limit=100&status=all&role=user&account_kind=managed&sort=created_at&order=desc"))
                    .credentials(web_sys::RequestCredentials::Include).send().await
                    && response.ok() && let Ok(payload)=response.json::<AdminUsersResponse>().await
                {
                    managed_account_users.set(payload.users);
                }
            });
        }
    });
    Effect::new(move |_| {
        if let Some(user_id) = ui.admin_user_id.get() {
            target_user_id.set(user_id);
        }
    });

    let reset_form = move || {
        username.set(String::new());
        target_user_id.set(String::new());
        account_template_id.set(String::new());
        config_name.set(String::new());
        template_id.set(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into());
        endpoint_mode.set("images_api".into());
        base_url.set("https://api.openai.com".into());
        api_key.set(String::new());
        models_text.set("gpt-image-2, gpt-image-2.5-flare, gpt-image-2.5-sunburst".into());
        current_model.set("gpt-image-2".into());
        responses_model.set(String::new());
        message.set(None);
    };

    let submit_provider_form = move |_| {
        let Some(mode) = form_mode.get_untracked() else {
            return;
        };
        if mode == "create" && !account_template_id.get_untracked().is_empty() {
            if username.get_untracked().trim().is_empty() {
                message.set(Some("请填写托管账号用户名。".into()));
                return;
            }
            busy.set(true);
            spawn_local(async move {
                let request = ManagedAccountCreateRequest {
                    username: username.get_untracked(),
                    initial_provider: None,
                    initial_template_id: Some(account_template_id.get_untracked()),
                };
                let result = match Request::post(&api_url("/api/admin/managed-users"))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&request)
                {
                    Ok(builder) => builder.send().await,
                    Err(error) => {
                        message.set(Some(format!("创建请求序列化失败：{error}")));
                        busy.set(false);
                        return;
                    }
                };
                match result {
                    Ok(response) if response.ok() => {
                        match response.json::<ManagedAccountCreateResponse>().await {
                            Ok(payload) => {
                                temporary_password.set(Some(payload.temporary_password));
                                managed_account_users
                                    .update(|users| users.push(payload.user.clone()));
                                account.admin_users.update(|users| users.push(payload.user));
                                let _ = reload_managed_admin_configs(account).await;
                            }
                            Err(error) => message.set(Some(format!("创建响应解析失败：{error}"))),
                        }
                    }
                    Ok(response) => {
                        message.set(Some(response_error(response, "创建托管账号失败。").await))
                    }
                    Err(error) => message.set(Some(format!("创建托管账号失败：{error}"))),
                }
                busy.set(false);
            });
            return;
        }
        let Some(template) = workspace.templates.with_untracked(|templates| {
            templates
                .iter()
                .find(|template| template.id == template_id.get_untracked())
                .cloned()
        }) else {
            message.set(Some("所选服务商模板不存在。".into()));
            return;
        };
        let input = match config_input(
            config_name.get_untracked(),
            template,
            endpoint_mode.get_untracked(),
            base_url.get_untracked(),
            models_text.get_untracked(),
            current_model.get_untracked(),
            responses_model.get_untracked(),
        ) {
            Ok(input) => input,
            Err(error) => {
                message.set(Some(error));
                return;
            }
        };
        if mode == "create" && username.get_untracked().trim().is_empty() {
            message.set(Some("请填写托管账号用户名。".into()));
            return;
        }
        if api_key.get_untracked().trim().is_empty() && !mode.starts_with("edit:") {
            message.set(Some("新配置必须填写 API Key。".into()));
            return;
        }
        busy.set(true);
        message.set(None);
        spawn_local(async move {
            let write_request = ManagedProviderConfigWriteRequest {
                config: input,
                api_key: (!api_key.get_untracked().trim().is_empty())
                    .then(|| api_key.get_untracked()),
            };
            let result = if mode == "create" {
                let builder = Request::post(&api_url("/api/admin/managed-users"))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&ManagedAccountCreateRequest {
                        username: username.get_untracked(),
                        initial_provider: Some(write_request),
                        initial_template_id: None,
                    });
                match builder {
                    Ok(builder) => match builder.send().await {
                        Ok(response) if response.ok() => {
                            match response.json::<ManagedAccountCreateResponse>().await {
                                Ok(payload) => {
                                    temporary_password.set(Some(payload.temporary_password));
                                    managed_account_users
                                        .update(|users| users.push(payload.user.clone()));
                                    account.admin_users.update(|users| users.push(payload.user));
                                    Ok(())
                                }
                                Err(error) => Err(format!("创建响应解析失败：{error}")),
                            }
                        }
                        Ok(response) => Err(response_error(response, "创建托管账号失败。").await),
                        Err(error) => Err(format!("创建托管账号失败：{error}")),
                    },
                    Err(error) => Err(format!("创建请求序列化失败：{error}")),
                }
            } else {
                let path = if let Some(config_id) = mode.strip_prefix("edit:") {
                    format!("/api/admin/managed-providers/{config_id}")
                } else {
                    format!(
                        "/api/admin/managed-users/{}/providers",
                        target_user_id.get_untracked()
                    )
                };
                let builder = Request::post(&api_url(&path))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&write_request);
                match builder {
                    Ok(builder) => match builder.send().await {
                        Ok(response) if response.ok() => Ok(()),
                        Ok(response) => Err(response_error(response, "保存托管配置失败。").await),
                        Err(error) => Err(format!("保存托管配置失败：{error}")),
                    },
                    Err(error) => Err(format!("保存请求序列化失败：{error}")),
                }
            };
            match result {
                Ok(()) => {
                    if let Err(error) = reload_managed_admin_configs(account).await {
                        message.set(Some(error));
                    } else if temporary_password.get_untracked().is_none() {
                        form_mode.set(None);
                        reset_form();
                    }
                }
                Err(error) => message.set(Some(error)),
            }
            api_key.set(String::new());
            busy.set(false);
        });
    };

    let selected_compatible = Memo::new(move |_| {
        let selected = selected_ids.get();
        let configs = account.admin_managed_providers.get();
        let mut protocol = None;
        let mut count = 0usize;
        for config in configs
            .iter()
            .filter(|config| selected.contains(&config.summary.id))
        {
            let current = (config.summary.provider_kind, config.summary.endpoint_mode);
            if protocol.is_some_and(|expected| expected != current) {
                return None;
            }
            protocol = Some(current);
            count += 1;
        }
        (count > 0).then_some(count)
    });
    let managed_users = Memo::new(move |_| {
        managed_account_users
            .get()
            .into_iter()
            .filter(|user| user.account_kind == mew_image_shared::AccountKind::Managed)
            .collect::<Vec<_>>()
    });

    let submit_bulk = move |_| {
        let Some(count) = selected_compatible.get_untracked() else {
            message.set(Some("批量更新只能选择相同协议和接口模式的配置。".into()));
            return;
        };
        let address = bulk_base_url.get_untracked();
        let key = bulk_api_key.get_untracked();
        if address.trim().is_empty() && key.trim().is_empty() {
            message.set(Some("请至少填写新的上游地址或 API Key。".into()));
            return;
        }
        let selected = selected_ids.get_untracked();
        let selected_configs = account.admin_managed_providers.with_untracked(|configs| {
            configs
                .iter()
                .filter(|config| selected.contains(&config.summary.id))
                .cloned()
                .collect::<Vec<_>>()
        });
        let account_count = selected_configs
            .iter()
            .map(|config| config.user_id.as_str())
            .collect::<HashSet<_>>()
            .len();
        let affected_names = selected_configs
            .iter()
            .take(8)
            .map(|config| format!("{} / {}", config.username, config.summary.name))
            .collect::<Vec<_>>()
            .join("\n");
        let confirmed = web_sys::window()
            .and_then(|window| {
                window
                    .confirm_with_message(&format!(
                        "确认更新 {account_count} 个账号下的 {count} 条托管配置？\n\n{affected_names}{}",
                        if count > 8 { "\n……" } else { "" }
                    ))
                    .ok()
            })
            .unwrap_or(false);
        if !confirmed {
            return;
        }
        busy.set(true);
        let request = ManagedProviderBulkCredentialsRequest {
            config_ids: selected.into_iter().collect(),
            base_url: (!address.trim().is_empty()).then(|| address.trim().into()),
            api_key: (!key.trim().is_empty()).then_some(key),
        };
        spawn_local(async move {
            let builder = Request::post(&api_url("/api/admin/managed-providers/bulk-credentials"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request);
            let result = match builder {
                Ok(builder) => match builder.send().await {
                    Ok(response) if response.ok() => response
                        .json::<ManagedProviderMutationResponse>()
                        .await
                        .map(|payload| format!("已更新 {} 条托管配置。", payload.updated_count))
                        .map_err(|error| format!("批量更新响应解析失败：{error}")),
                    Ok(response) => Err(response_error(response, "批量更新失败。").await),
                    Err(error) => Err(format!("批量更新失败：{error}")),
                },
                Err(error) => Err(format!("批量更新请求序列化失败：{error}")),
            };
            match result {
                Ok(success) => {
                    selected_ids.set(HashSet::new());
                    bulk_base_url.set(String::new());
                    bulk_api_key.set(String::new());
                    message.set(Some(success));
                    let _ = reload_managed_admin_configs(account).await;
                }
                Err(error) => message.set(Some(error)),
            }
            busy.set(false);
        });
    };

    view! {
        <section class="stack managed-admin-panel">
            <div class="row managed-admin-toolbar">
                <div>
                    <h3>"托管账号与服务商"</h3>
                    <p class="status compact-help">"连接凭据只保存在服务器，API Key 写入后不再回显。"</p>
                </div>
                <div class="row">
                    <button class="button secondary" on:click=move |_| {
                        reset_form();
                        form_mode.set(Some("create".into()));
                    }><MaterialSymbolIcon name="person_add" filled=false />"创建托管账号"</button>
                    <button class="button ghost icon-button" title="刷新托管配置" on:click=move |_| {
                        spawn_local(async move {
                            if let Err(error) = reload_managed_admin_configs(account).await {
                                message.set(Some(error));
                            }
                        });
                    }><MaterialSymbolIcon name="refresh" filled=false /></button>
                </div>
            </div>
            <div class="row managed-add-provider-row">
                <select class="select-input" prop:value=move || target_user_id.get() on:change=move |event| target_user_id.set(event_target_value(&event))>
                    <option value="">"选择托管账号"</option>
                    <For
                        each=move || managed_users.get()
                        key=|user| user.id.clone()
                        children=move |user| view! { <option value=user.id>{user.username}</option> }
                    />
                </select>
                <button class="button ghost" disabled=move || target_user_id.get().is_empty() on:click=move |_| {
                    let selected_user_id = target_user_id.get_untracked();
                    reset_form();
                    target_user_id.set(selected_user_id);
                    form_mode.set(Some("add".into()));
                }><MaterialSymbolIcon name="add" filled=false />"为账号添加配置"</button>
            </div>

            <Show when=move || selected_compatible.get().is_some()>
                <div class="managed-bulk-bar">
                    <strong>{move || format!("已选 {} 条", selected_ids.get().len())}</strong>
                    <input class="text-input" placeholder="新上游地址（可选）" prop:value=move || bulk_base_url.get() on:input=move |event| bulk_base_url.set(event_target_value(&event)) />
                    <input class="text-input" type="password" autocomplete="new-password" placeholder="新 API Key（可选）" prop:value=move || bulk_api_key.get() on:input=move |event| bulk_api_key.set(event_target_value(&event)) />
                    <button class="button danger" disabled=move || busy.get() on:click=submit_bulk>"批量更新"</button>
                </div>
            </Show>
            <Show when=move || !selected_ids.get().is_empty() && selected_compatible.get().is_none()>
                <p class="form-error">"所选配置的协议或接口模式不同，不能批量更新凭据。"</p>
            </Show>

            <div class="managed-config-list">
                <For
                    each=move || {
                        let selected_user = ui.admin_user_id.get();
                        account.admin_managed_providers.get().into_iter().filter(|config| {
                            selected_user.as_ref().is_none_or(|user_id| config.user_id == *user_id)
                        }).collect::<Vec<_>>()
                    }
                    key=|config| format!("{}:{}:{}", config.summary.id, config.summary.enabled, config.summary.updated_at)
                    children=move |config: ManagedProviderAdminView| {
                        let select_id = config.summary.id.clone();
                        let checked_id = select_id.clone();
                        let edit_config = config.clone();
                        let enabled_config = config.clone();
                        let delete_config = config.clone();
                        let reset_user = config.clone();
                        view! {
                            <article class="managed-config-row">
                                <input type="checkbox" prop:checked=move || selected_ids.get().contains(&checked_id) on:change=move |_| {
                                    selected_ids.update(|ids| {
                                        if !ids.insert(select_id.clone()) { ids.remove(&select_id); }
                                    });
                                } />
                                <div class="managed-config-main">
                                    <strong>{format!("{} · {}", config.username, config.summary.name)}</strong>
                                    <span>{format!("{:?} · {:?} · {}", config.summary.provider_kind, config.summary.endpoint_mode, config.summary.current_model)}</span>
                                    <small>{format!("{} · Key {}", config.base_url, config.api_key_hint)}</small>
                                </div>
                                <span class="tag">{if config.summary.enabled { "已启用" } else { "已停用" }}</span>
                                <div class="row managed-config-actions">
                                    <button class="button ghost icon-button" title="编辑配置" on:click=move |_| {
                                        reset_form();
                                        config_name.set(edit_config.summary.name.clone());
                                        template_id.set(edit_config.template.id.clone());
                                        endpoint_mode.set(endpoint_mode_value(edit_config.summary.endpoint_mode).into());
                                        base_url.set(edit_config.base_url.clone());
                                        models_text.set(edit_config.summary.available_models.join(", "));
                                        current_model.set(edit_config.summary.current_model.clone());
                                        responses_model.set(edit_config.responses_model.clone().unwrap_or_default());
                                        form_mode.set(Some(format!("edit:{}", edit_config.summary.id)));
                                    }><MaterialSymbolIcon name="edit" filled=false /></button>
                                    <button class="button ghost icon-button" title=if enabled_config.summary.enabled { "停用配置" } else { "启用配置" } on:click=move |_| {
                                        let request = ManagedProviderEnabledRequest { enabled: !enabled_config.summary.enabled };
                                        let id = enabled_config.summary.id.clone();
                                        spawn_local(async move {
                                            if let Ok(builder) = Request::post(&api_url(&format!("/api/admin/managed-providers/{id}/enabled"))).credentials(web_sys::RequestCredentials::Include).json(&request) {
                                                match builder.send().await {
                                                    Ok(response) if response.ok() => { let _ = reload_managed_admin_configs(account).await; }
                                                    Ok(response) => message.set(Some(response_error(response, "启停配置失败。").await)),
                                                    Err(error) => message.set(Some(format!("启停配置失败：{error}"))),
                                                }
                                            }
                                        });
                                    }><MaterialSymbolIcon name=if enabled_config.summary.enabled { "pause_circle" } else { "play_circle" } filled=false /></button>
                                    <button class="button ghost icon-button" title="为该账号添加配置" on:click=move |_| {
                                        reset_form();
                                        target_user_id.set(config.user_id.clone());
                                        form_mode.set(Some("add".into()));
                                    }><MaterialSymbolIcon name="add" filled=false /></button>
                                    <button class="button ghost icon-button" title="重置临时密码" on:click=move |_| {
                                        pending_password_reset.set(Some(reset_user.clone()));
                                    }><MaterialSymbolIcon name="lock_reset" filled=false /></button>
                                    <button class="button ghost danger icon-button" title="删除配置" on:click=move |_| {
                                        pending_config_delete.set(Some(delete_config.clone()));
                                    }><MaterialSymbolIcon name="delete" filled=false /></button>
                                </div>
                            </article>
                        }
                    }
                />
            </div>
            <Show when=move || account.loading_admin_managed_providers.get()>
                <p class="status">"正在加载托管配置…"</p>
            </Show>
            {move || message.get().map(|value| view! { <p class="form-hint">{value}</p> })}

            <Show when=move || pending_config_delete.get().is_some()>
                {move || pending_config_delete.get().map(|config| {
                    let id = config.summary.id.clone();
                    view! {
                        <div class="managed-form-backdrop">
                            <section class="admin-delete-confirm stack" role="dialog" aria-modal="true">
                                <h3>"删除托管配置"</h3>
                                <p>{format!("确认删除 {} / {}？此操作不会删除账号。", config.username, config.summary.name)}</p>
                                <div class="row">
                                    <button class="button ghost" on:click=move |_| pending_config_delete.set(None)>"取消"</button>
                                    <button class="button danger" on:click=move |_| {
                                        let delete_id = id.clone();
                                        pending_config_delete.set(None);
                                        spawn_local(async move {
                                            match Request::delete(&api_url(&format!("/api/admin/managed-providers/{delete_id}"))).credentials(web_sys::RequestCredentials::Include).send().await {
                                                Ok(response) if response.ok() => { let _ = reload_managed_admin_configs(account).await; }
                                                Ok(response) => message.set(Some(response_error(response, "删除托管配置失败。").await)),
                                                Err(error) => message.set(Some(format!("删除托管配置失败：{error}"))),
                                            }
                                        });
                                    }>"确认删除"</button>
                                </div>
                            </section>
                        </div>
                    }
                })}
            </Show>

            <Show when=move || pending_password_reset.get().is_some()>
                {move || pending_password_reset.get().map(|config| {
                    let user_id = config.user_id.clone();
                    let username = config.username.clone();
                    view! {
                        <div class="managed-form-backdrop">
                            <section class="admin-delete-confirm stack" role="dialog" aria-modal="true">
                                <h3>"重置临时密码"</h3>
                                <p>{format!("确认重置账号“{username}”的临时密码？旧会话会立即失效，下次登录必须修改密码。")}</p>
                                <div class="row">
                                    <button class="button ghost" on:click=move |_| pending_password_reset.set(None)>"取消"</button>
                                    <button class="button danger" on:click=move |_| {
                                        let reset_user_id = user_id.clone();
                                        pending_password_reset.set(None);
                                        spawn_local(async move {
                                            match Request::post(&api_url(&format!("/api/admin/managed-users/{reset_user_id}/reset-password"))).credentials(web_sys::RequestCredentials::Include).send().await {
                                                Ok(response) if response.ok() => match response.json::<ManagedPasswordResetResponse>().await {
                                                    Ok(payload) => {
                                                        account.admin_users.update(|users| {
                                                            if let Some(user) = users.iter_mut().find(|user| user.id == payload.user_id) {
                                                                user.must_change_password = true;
                                                            }
                                                        });
                                                        temporary_password.set(Some(payload.temporary_password));
                                                    },
                                                    Err(error) => message.set(Some(format!("重置密码响应解析失败：{error}"))),
                                                },
                                                Ok(response) => message.set(Some(response_error(response, "重置临时密码失败。").await)),
                                                Err(error) => message.set(Some(format!("重置临时密码失败：{error}"))),
                                            }
                                        });
                                    }>"确认重置"</button>
                                </div>
                            </section>
                        </div>
                    }
                })}
            </Show>

            <Show when=move || form_mode.get().is_some()>
                <div class="managed-form-backdrop" on:click=move |_| form_mode.set(None)>
                    <section class="managed-form-modal stack" on:click=move |event: web_sys::MouseEvent| event.stop_propagation()>
                        <div class="row">
                            <h3>{move || match form_mode.get().as_deref() { Some("create") => "创建托管账号", Some("add") => "添加托管配置", _ => "编辑托管配置" }}</h3>
                            <button class="button ghost icon-button" title="关闭" on:click=move |_| form_mode.set(None)><MaterialSymbolIcon name="close" filled=false /></button>
                        </div>
                        <Show when=move || form_mode.get().as_deref() == Some("create")>
                            <input class="text-input" placeholder="用户名" prop:value=move || username.get() on:input=move |event| username.set(event_target_value(&event)) />
                            <select class="select-input" prop:value=move || account_template_id.get() on:change=move |event| account_template_id.set(event_target_value(&event))>
                                <option value="">"手工填写首个配置"</option>
                                <For
                                    each=move || { managed_templates.get().into_iter().filter(|item| item.enabled).collect::<Vec<_>>() }
                                    key=|item| item.id.clone()
                                    children=move |item| view! { <option value=item.id>{format!("使用模板：{}", item.config.name)}</option> }
                                />
                            </select>
                        </Show>
                        <Show when=move || form_mode.get().as_deref() != Some("create") || account_template_id.get().is_empty()>
                        <input class="text-input" placeholder="配置名称" prop:value=move || config_name.get() on:input=move |event| config_name.set(event_target_value(&event)) />
                        <select class="select-input" prop:value=move || template_id.get() on:change=move |event| {
                            let id = event_target_value(&event);
                            template_id.set(id.clone());
                            if let Some(template) = workspace.templates.with_untracked(|items| items.iter().find(|item| item.id == id).cloned()) {
                                let default_models = mew_image_shared::default_available_models(template.kind);
                                models_text.set(default_models.join(", "));
                                current_model.set(default_models.first().cloned().unwrap_or_default());
                                base_url.set(template.base_url);
                                endpoint_mode.set(if template.kind == ProviderKind::CustomHttp { "custom_json" } else { "images_api" }.into());
                            }
                        }>
                            <For each=move || workspace.templates.get() key=|template| template.id.clone() children=move |template| view! { <option value=template.id>{template.name}</option> } />
                        </select>
                        <select class="select-input" prop:value=move || endpoint_mode.get() on:change=move |event| endpoint_mode.set(event_target_value(&event))>
                            <option value="images_api">"Images API"</option>
                            <option value="responses_api">"Responses API"</option>
                            <option value="custom_json">"Custom JSON"</option>
                        </select>
                        <input class="text-input" placeholder="上游地址" prop:value=move || base_url.get() on:input=move |event| base_url.set(event_target_value(&event)) />
                        <input class="text-input" type="password" autocomplete="new-password" placeholder=move || if form_mode.get().as_deref().is_some_and(|mode| mode.starts_with("edit:")) { "新 API Key（留空则保留）" } else { "API Key" } prop:value=move || api_key.get() on:input=move |event| api_key.set(event_target_value(&event)) />
                        <textarea class="text-input" rows="2" placeholder="模型列表，可用中英文逗号或换行分隔" prop:value=move || models_text.get() on:input=move |event| models_text.set(event_target_value(&event)) />
                        <div class="row">
                            <input class="text-input" placeholder="当前模型" prop:value=move || current_model.get() on:input=move |event| current_model.set(event_target_value(&event)) />
                            <input class="text-input" placeholder="Responses 主模型（可选）" prop:value=move || responses_model.get() on:input=move |event| responses_model.set(event_target_value(&event)) />
                        </div>
                        </Show>
                        <button class="button primary" disabled=move || busy.get() on:click=submit_provider_form>{move || if busy.get() { "保存中…" } else { "保存" }}</button>
                        {move || message.get().map(|value| view! { <p class="form-error">{value}</p> })}
                    </section>
                </div>
            </Show>

            <Show when=move || temporary_password.get().is_some()>
                <div class="managed-form-backdrop">
                    <section class="managed-password-result stack" role="dialog" aria-modal="true">
                        <h3>"一次性临时密码"</h3>
                        <p class="status">"只在此处显示一次。关闭后如需再次获取，必须重置密码。"</p>
                        <code>{move || temporary_password.get().unwrap_or_default()}</code>
                        <div class="row">
                            <button class="button secondary" on:click=move |_| {
                                if let Some(value) = temporary_password.get_untracked()
                                    && let Some(clipboard) = web_sys::window().map(|window| window.navigator().clipboard())
                                {
                                    let _ = clipboard.write_text(&value);
                                }
                            }>"复制密码"</button>
                            <button class="button primary" on:click=move |_| {
                                temporary_password.set(None);
                                form_mode.set(None);
                                reset_form();
                            }>"我已保存"</button>
                        </div>
                    </section>
                </div>
            </Show>
        </section>
    }
}

#[component]
pub(crate) fn ManagedProviderTemplatesAdmin() -> impl IntoView {
    let account = expect_context::<AccountState>();
    let workspace = expect_context::<WorkspaceState>();
    let templates = RwSignal::new(Vec::<ManagedProviderTemplateAdminView>::new());
    let template_total = RwSignal::new(0usize);
    let template_page = RwSignal::new(1usize);
    let template_query = RwSignal::new(String::new());
    let template_request_sequence = RwSignal::new(0u64);
    let managed_users = RwSignal::new(Vec::<AdminUserSummary>::new());
    let form_id = RwSignal::new(None::<String>);
    let target_mode = RwSignal::new(None::<(String, String)>);
    let pending_template_delete = RwSignal::new(None::<ManagedProviderTemplateAdminView>);
    let selected_users = RwSignal::new(HashSet::<String>::new());
    let message = RwSignal::new(None::<String>);
    let name = RwSignal::new(String::new());
    let protocol_template_id =
        RwSignal::new(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into());
    let endpoint_mode = RwSignal::new("images_api".to_string());
    let base_url = RwSignal::new("https://api.openai.com".to_string());
    let api_key = RwSignal::new(String::new());
    let models =
        RwSignal::new("gpt-image-2, gpt-image-2.5-flare, gpt-image-2.5-sunburst".to_string());
    let current_model = RwSignal::new("gpt-image-2".to_string());
    let responses_model = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    let escape_listener = window_event_listener(ev::keydown, move |event| {
        if event.key() != "Escape" {
            return;
        }
        let handled = if pending_template_delete.get_untracked().is_some() {
            pending_template_delete.set(None);
            true
        } else if target_mode.get_untracked().is_some() && !busy.get_untracked() {
            target_mode.set(None);
            selected_users.set(HashSet::new());
            true
        } else if form_id.get_untracked().is_some() && !busy.get_untracked() {
            form_id.set(None);
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

    let reload = move || {
        let request_sequence = template_request_sequence.get_untracked();
        spawn_local(async move {
            let query = js_sys::encode_uri_component(template_query.get_untracked().trim())
                .as_string()
                .unwrap_or_default();
            let url = format!(
                "/api/admin/managed-provider-templates?page={}&limit=20&q={query}",
                template_page.get_untracked()
            );
            match Request::get(&api_url(&url))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                Ok(response) if response.ok() => {
                    if let Ok(payload) =
                        response.json::<ManagedProviderTemplateListResponse>().await
                    {
                        if template_request_sequence.get_untracked() != request_sequence {
                            return;
                        }
                        template_total.set(payload.total);
                        templates.set(payload.templates);
                    }
                }
                Ok(response) => {
                    message.set(Some(response_error(response, "服务商模板加载失败。").await))
                }
                Err(error) => message.set(Some(format!("服务商模板加载失败：{error}"))),
            }
            if let Ok(response)=Request::get(&api_url("/api/admin/users?page=1&limit=100&status=all&role=user&account_kind=managed&sort=created_at&order=desc"))
            .credentials(web_sys::RequestCredentials::Include).send().await
            && response.ok() && let Ok(payload)=response.json::<AdminUsersResponse>().await { managed_users.set(payload.users); }
            let _ = reload_managed_admin_configs(account).await;
        })
    };
    Effect::new(move |_| {
        let _ = template_page.get();
        let _ = template_query.get();
        template_request_sequence.update(|value| *value += 1);
        let sequence = template_request_sequence.get_untracked();
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if template_request_sequence.get_untracked() == sequence {
                reload();
            }
        });
    });
    let target_candidates = Memo::new(move |_| {
        let users = managed_users.get();
        let Some((mode, template_id)) = target_mode.get() else {
            return users;
        };
        let linked = account
            .admin_managed_providers
            .get()
            .into_iter()
            .filter(|config| config.source_template_id.as_deref() == Some(template_id.as_str()))
            .map(|config| config.user_id)
            .collect::<HashSet<_>>();
        users
            .into_iter()
            .filter(|user| {
                if mode == "sync" {
                    linked.contains(&user.id)
                } else {
                    !linked.contains(&user.id)
                }
            })
            .collect()
    });

    let reset_form = move || {
        form_id.set(None);
        name.set(String::new());
        protocol_template_id.set(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into());
        endpoint_mode.set("images_api".into());
        base_url.set("https://api.openai.com".into());
        api_key.set(String::new());
        models.set("gpt-image-2, gpt-image-2.5-flare, gpt-image-2.5-sunburst".into());
        current_model.set("gpt-image-2".into());
        responses_model.set(String::new());
    };
    let open_create = move |_| {
        reset_form();
        form_id.set(Some(String::new()));
    };
    let save = move |_| {
        let Some(protocol_template) = workspace.templates.with_untracked(|items| {
            items
                .iter()
                .find(|item| item.id == protocol_template_id.get_untracked())
                .cloned()
        }) else {
            message.set(Some("协议模板不存在。".into()));
            return;
        };
        let input = match config_input(
            name.get_untracked(),
            protocol_template,
            endpoint_mode.get_untracked(),
            base_url.get_untracked(),
            models.get_untracked(),
            current_model.get_untracked(),
            responses_model.get_untracked(),
        ) {
            Ok(value) => value,
            Err(error) => {
                message.set(Some(error));
                return;
            }
        };
        let id = form_id.get_untracked().unwrap_or_default();
        if id.is_empty() && api_key.get_untracked().trim().is_empty() {
            message.set(Some("新模板必须填写 API Key。".into()));
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let path = if id.is_empty() {
                "/api/admin/managed-provider-templates".into()
            } else {
                format!("/api/admin/managed-provider-templates/{id}")
            };
            let request = ManagedProviderTemplateWriteRequest {
                config: input,
                api_key: (!api_key.get_untracked().trim().is_empty())
                    .then(|| api_key.get_untracked()),
            };
            let result = match Request::post(&api_url(&path))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request)
            {
                Ok(builder) => builder.send().await,
                Err(error) => {
                    message.set(Some(format!("模板请求序列化失败：{error}")));
                    busy.set(false);
                    return;
                }
            };
            match result {
                Ok(response) if response.ok() => {
                    form_id.set(None);
                    api_key.set(String::new());
                    reload();
                }
                Ok(response) => {
                    message.set(Some(response_error(response, "保存服务商模板失败。").await))
                }
                Err(error) => message.set(Some(format!("保存服务商模板失败：{error}"))),
            }
            busy.set(false);
        });
    };
    let submit_targets = move |_| {
        let Some((mode, id)) = target_mode.get_untracked() else {
            return;
        };
        let request = ManagedProviderTemplateTargetsRequest {
            user_ids: selected_users.get_untracked().into_iter().collect(),
        };
        if request.user_ids.is_empty() {
            message.set(Some("请至少选择一个托管账号。".into()));
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let preview_path = format!("/api/admin/managed-provider-templates/{id}/{mode}-preview");
            let preview = match Request::post(&api_url(&preview_path))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request)
            {
                Ok(builder) => builder.send().await,
                Err(error) => {
                    message.set(Some(format!("预检请求序列化失败：{error}")));
                    busy.set(false);
                    return;
                }
            };
            match preview {
                Ok(response) if response.ok() => {}
                Ok(response) => {
                    message.set(Some(response_error(response, "模板操作预检失败。").await));
                    busy.set(false);
                    return;
                }
                Err(error) => {
                    message.set(Some(format!("模板操作预检失败：{error}")));
                    busy.set(false);
                    return;
                }
            }
            let path = format!("/api/admin/managed-provider-templates/{id}/{mode}");
            let result = match Request::post(&api_url(&path))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request)
            {
                Ok(builder) => builder.send().await,
                Err(error) => {
                    message.set(Some(format!("请求序列化失败：{error}")));
                    busy.set(false);
                    return;
                }
            };
            match result {
                Ok(response) if response.ok() => match response
                    .json::<ManagedProviderTemplateMutationResponse>()
                    .await
                {
                    Ok(payload) => {
                        message.set(Some(format!("已处理 {} 个账号。", payload.affected_count)));
                        target_mode.set(None);
                        selected_users.set(HashSet::new());
                        reload();
                    }
                    Err(error) => message.set(Some(format!("响应解析失败：{error}"))),
                },
                Ok(response) => {
                    message.set(Some(response_error(response, "模板分配或同步失败。").await))
                }
                Err(error) => message.set(Some(format!("模板分配或同步失败：{error}"))),
            }
            busy.set(false);
        });
    };

    view! {<section class="stack managed-template-admin"><div class="row managed-admin-toolbar"><div><h3>"服务商模板"</h3><p class="status compact-help">"完整连接配置加密保存在服务器；分配后为账号创建独立副本。"</p></div><button class="button secondary" on:click=open_create><MaterialSymbolIcon name="add" filled=false />"新建模板"</button></div>
        <div class="admin-user-toolbar"><input class="text-input" placeholder="搜索模板名称" prop:value=move||template_query.get() on:input=move|event|{template_query.set(event_target_value(&event));template_page.set(1)} /><span class="tag">{move||format!("共 {} 个",template_total.get())}</span></div>
        <div class="managed-config-list"><For each=move||templates.get() key=|item|format!("{}:{}:{}:{}",item.id,item.revision,item.enabled,item.assigned_count) children=move|item|{let edit=item.clone();let assign_id=item.id.clone();let sync_id=item.id.clone();let toggle=item.clone();let delete=item.clone();view!{<article class="managed-config-row"><div class="managed-config-main"><strong>{item.config.name.clone()}</strong><span>{format!("{:?} · {:?} · {}",item.config.template.kind,item.config.endpoint_mode,item.config.current_model)}</span><small>{format!("修订 {} · 已分配 {} · 待同步 {} · Key {}",item.revision,item.assigned_count,item.outdated_count,item.api_key_hint)}</small></div><span class="tag">{if item.enabled{"已启用"}else{"已停用"}}</span><div class="row managed-config-actions"><button class="button ghost icon-button" title="编辑模板" on:click=move |_|{name.set(edit.config.name.clone());protocol_template_id.set(edit.config.template.id.clone());endpoint_mode.set(endpoint_mode_value(edit.config.endpoint_mode).into());base_url.set(edit.config.base_url.clone());models.set(edit.config.available_models.join(", "));current_model.set(edit.config.current_model.clone());responses_model.set(edit.config.responses_model.clone().unwrap_or_default());api_key.set(String::new());form_id.set(Some(edit.id.clone()));}><MaterialSymbolIcon name="edit" filled=false /></button><button class="button ghost icon-button" title="分配账号" disabled=move||!item.enabled on:click=move |_|{selected_users.set(HashSet::new());target_mode.set(Some(("assign".into(),assign_id.clone())));}><MaterialSymbolIcon name="person_add" filled=false /></button><button class="button ghost icon-button" title="同步关联账号" disabled=move||item.assigned_count==0 on:click=move |_|{let linked=account.admin_managed_providers.get().into_iter().filter(|config|config.source_template_id.as_deref()==Some(sync_id.as_str())).map(|config|config.user_id).collect();selected_users.set(linked);target_mode.set(Some(("sync".into(),sync_id.clone())));}><MaterialSymbolIcon name="sync" filled=false /></button><button class="button ghost icon-button" title=if toggle.enabled{"停用模板"}else{"启用模板"} on:click=move |_|{let id=toggle.id.clone();let request=ManagedProviderEnabledRequest{enabled:!toggle.enabled};spawn_local(async move{if let Ok(builder)=Request::post(&api_url(&format!("/api/admin/managed-provider-templates/{id}/enabled"))).credentials(web_sys::RequestCredentials::Include).json(&request){let _=builder.send().await;reload();}})}><MaterialSymbolIcon name=if item.enabled{"pause_circle"}else{"play_circle"} filled=false /></button><button class="button ghost danger icon-button" title="删除模板" on:click=move |_|pending_template_delete.set(Some(delete.clone()))><MaterialSymbolIcon name="delete" filled=false /></button></div></article>}}/></div>
        {move||message.get().map(|value|view!{<p class="form-hint">{value}</p>})}
        <Show when=move||form_id.get().is_some()><div class="managed-form-backdrop"><section class="managed-form-modal stack"><div class="row"><h3>{move||if form_id.get().as_deref()==Some(""){"新建服务商模板"}else{"编辑服务商模板"}}</h3><button class="button ghost icon-button" on:click=move |_|form_id.set(None)><MaterialSymbolIcon name="close" filled=false /></button></div><input class="text-input" placeholder="模板名称" prop:value=move||name.get() on:input=move|event|name.set(event_target_value(&event)) /><select class="select-input" prop:value=move||protocol_template_id.get() on:change=move|event|protocol_template_id.set(event_target_value(&event))><For each=move||workspace.templates.get() key=|item|item.id.clone() children=move|item|view!{<option value=item.id>{item.name}</option>}/></select><select class="select-input" prop:value=move||endpoint_mode.get() on:change=move|event|endpoint_mode.set(event_target_value(&event))><option value="images_api">"Images API"</option><option value="responses_api">"Responses API"</option><option value="custom_json">"Custom JSON"</option></select><input class="text-input" placeholder="上游地址" prop:value=move||base_url.get() on:input=move|event|base_url.set(event_target_value(&event)) /><input class="text-input" type="password" placeholder=move||if form_id.get().as_deref()==Some(""){"API Key"}else{"新 API Key（留空保留）"} prop:value=move||api_key.get() on:input=move|event|api_key.set(event_target_value(&event)) /><textarea class="text-input" rows="2" placeholder="模型列表" prop:value=move||models.get() on:input=move|event|models.set(event_target_value(&event)) /><div class="row"><input class="text-input" placeholder="默认模型" prop:value=move||current_model.get() on:input=move|event|current_model.set(event_target_value(&event)) /><input class="text-input" placeholder="Responses 主模型" prop:value=move||responses_model.get() on:input=move|event|responses_model.set(event_target_value(&event)) /></div><button class="button primary" disabled=move||busy.get() on:click=save>"保存"</button></section></div></Show>
        <Show when=move||target_mode.get().is_some()><div class="managed-form-backdrop"><section class="managed-form-modal stack"><div class="row"><h3>{move||if target_mode.get().as_ref().is_some_and(|(mode,_)|mode=="sync"){"同步到关联账号"}else{"分配模板"}}</h3><button class="button ghost icon-button" on:click=move |_|target_mode.set(None)><MaterialSymbolIcon name="close" filled=false /></button></div><label class="row"><input type="checkbox" on:change=move|_|{let candidates=target_candidates.get();if selected_users.get_untracked().len()==candidates.len(){selected_users.set(HashSet::new())}else{selected_users.set(candidates.into_iter().map(|user|user.id).collect())}} />"全选账号"</label><div class="managed-target-list"><For each=move||target_candidates.get() key=|user|user.id.clone() children=move|user|{let id=user.id.clone();let checked=id.clone();view!{<label><input type="checkbox" prop:checked=move||selected_users.get().contains(&checked) on:change=move|_|selected_users.update(|items|{if !items.insert(id.clone()){items.remove(&id);}}) />{user.username}</label>}}/></div><button class="button primary" disabled=move||busy.get()||selected_users.get().is_empty() on:click=submit_targets>"确认"</button></section></div></Show>
        <Show when=move||pending_template_delete.get().is_some()>{move||pending_template_delete.get().map(|template|{let id=template.id.clone();view!{<div class="managed-form-backdrop"><section class="admin-delete-confirm stack" role="dialog" aria-modal="true"><h3>"删除服务商模板"</h3><p>{format!("删除后，{} 条现有账号配置会保留并解除模板关联。",template.assigned_count)}</p><div class="row"><button class="button ghost" on:click=move |_|pending_template_delete.set(None)>"取消"</button><button class="button danger" on:click=move |_|{let delete_id=id.clone();pending_template_delete.set(None);spawn_local(async move{let _=Request::delete(&api_url(&format!("/api/admin/managed-provider-templates/{delete_id}"))).credentials(web_sys::RequestCredentials::Include).send().await;reload();});}>"确认删除"</button></div></section></div>}})}</Show>
        <div class="admin-pagination">
            <button class="button ghost" disabled=move || { template_page.get() <= 1 } on:click=move |_| template_page.update(|value| *value = value.saturating_sub(1))>"上一页"</button>
            <span>{move || format!("第 {} / {} 页", template_page.get(), template_total.get().div_ceil(20).max(1))}</span>
            <button class="button ghost" disabled=move || { template_page.get() >= template_total.get().div_ceil(20).max(1) } on:click=move |_| template_page.update(|value| *value += 1)>"下一页"</button>
        </div>
    </section>}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_input_supports_chinese_separators_and_deduplication() {
        assert_eq!(split_models("a，b；a\nc"), ["a", "b", "c"]);
    }
}
