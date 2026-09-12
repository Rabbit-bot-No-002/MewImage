use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::{
    BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, EncryptedApiConfig, ProviderAccessMode, ProviderEndpointMode,
    ProviderKind, ProviderTemplate, normalize_api_config, now_rfc3339,
};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::JsFuture;

use crate::app::mask_key;

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn ConfigEditor(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    current_config_id: RwSignal<String>,
    templates: RwSignal<Vec<ProviderTemplate>>,
    current_config_snapshot: Memo<Option<EncryptedApiConfig>>,
    save_configs_only: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let current_config = current_config_snapshot;
    let template_id_draft = RwSignal::new(String::new());
    let name_draft = RwSignal::new(String::new());
    let base_url_draft = RwSignal::new(String::new());
    let model_draft = RwSignal::new(String::new());
    let responses_model_draft = RwSignal::new(String::new());
    let api_key_draft = RwSignal::new(String::new());
    let api_key_visible = RwSignal::new(false);
    let api_key_copy_feedback = RwSignal::new(None::<String>);
    let access_mode_draft = RwSignal::new(String::from("Smart"));
    let endpoint_mode_draft = RwSignal::new(String::from("ImagesApi"));
    let has_pending_changes = RwSignal::new(false);
    let save_feedback = RwSignal::new(false);
    let loaded_config_id = RwSignal::new(String::new());

    Effect::new(move |_| {
        if let Some(config) = current_config_snapshot.get() {
            let should_reset_feedback = loaded_config_id.get_untracked() != config.id;
            loaded_config_id.set(config.id.clone());
            template_id_draft.set(config.provider_template_id);
            name_draft.set(config.name);
            base_url_draft.set(config.base_url);
            model_draft.set(config.model);
            responses_model_draft.set(config.responses_model.unwrap_or_else(|| "gpt-5.5".into()));
            api_key_draft.set(config.api_key_plaintext.unwrap_or_default());
            api_key_visible.set(false);
            api_key_copy_feedback.set(None);
            access_mode_draft.set(format!("{:?}", config.access_mode));
            endpoint_mode_draft.set(format!("{:?}", config.endpoint_mode));
            has_pending_changes.set(false);
            if should_reset_feedback {
                save_feedback.set(false);
            }
        }
    });

    let commit_name = move || {
        has_pending_changes.set(true);
    };
    let commit_base_url = move || {
        has_pending_changes.set(true);
    };
    let commit_model = move || {
        has_pending_changes.set(true);
    };
    let commit_responses_model = move || {
        has_pending_changes.set(true);
    };
    let commit_api_key = move || {
        has_pending_changes.set(true);
    };

    let save_config = move |_| {
        let current_id = current_config_id.get_untracked();
        if current_id.is_empty() {
            return;
        }
        let template_id = template_id_draft.get_untracked();
        let selected_template = templates
            .get_untracked()
            .into_iter()
            .find(|template| template.id == template_id);
        configs.update(|items| {
            if let Some(config) = items.iter_mut().find(|config| config.id == current_id) {
                config.provider_template_id = template_id.clone();
                if let Some(template) = selected_template.clone() {
                    config.provider_kind = template.kind;
                    config.known_requires_proxy = template.known_requires_proxy;
                }
                config.name = name_draft.get_untracked().trim().to_string();
                config.base_url = base_url_draft.get_untracked().trim().to_string();
                config.model = model_draft.get_untracked().trim().to_string();
                config.responses_model =
                    Some(responses_model_draft.get_untracked().trim().to_string());
                config.access_mode = match access_mode_draft.get_untracked().as_str() {
                    "Proxy" => ProviderAccessMode::Proxy,
                    "Direct" => ProviderAccessMode::Direct,
                    _ => ProviderAccessMode::Smart,
                };
                config.endpoint_mode = match endpoint_mode_draft.get_untracked().as_str() {
                    "ResponsesApi" => ProviderEndpointMode::ResponsesApi,
                    "CustomJson" => ProviderEndpointMode::CustomJson,
                    _ => ProviderEndpointMode::ImagesApi,
                };
                let api_key = api_key_draft.get_untracked().trim().to_string();
                let api_key_changed = config.api_key_plaintext.as_deref()
                    != (!api_key.is_empty()).then_some(api_key.as_str());
                if api_key_changed {
                    config.api_key_encrypted = None;
                }
                if api_key.is_empty() {
                    config.api_key_plaintext = None;
                    config.api_key_hint = None;
                } else {
                    config.api_key_plaintext = Some(api_key.clone());
                    config.api_key_hint = Some(mask_key(&api_key));
                }
                normalize_api_config(config);
                config.updated_at = now_rfc3339();
            }
        });
        has_pending_changes.set(false);
        save_feedback.set(true);
        save_configs_only();
        if let Some(window) = web_sys::window() {
            let callback = Closure::<dyn FnMut()>::once(move || {
                save_feedback.set(false);
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                1200,
            );
            callback.forget();
        }
    };

    let copy_api_key = move |_| {
        let value = api_key_draft.get_untracked();
        if value.trim().is_empty() {
            api_key_copy_feedback.set(Some("当前没有可复制的 API Key。".into()));
            return;
        }
        spawn_local(async move {
            let result = match web_sys::window() {
                Some(window) => JsFuture::from(window.navigator().clipboard().write_text(&value))
                    .await
                    .map(|_| ()),
                None => Err(wasm_bindgen::JsValue::from_str("浏览器窗口不可用")),
            };
            api_key_copy_feedback.set(Some(if result.is_ok() {
                "API Key 已复制到剪贴板。".into()
            } else {
                "复制失败，请检查浏览器剪贴板权限。".into()
            }));
        });
    };

    view! {
        <div class="stack">
            <input
                class="text-input"
                placeholder="配置名称"
                prop:value=move || name_draft.get()
                on:input=move |ev| name_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_name()
            />
            <select
                class="select-input"
                prop:value=move || template_id_draft.get()
                on:change=move |ev| {
                    let value = event_target_value(&ev);
                    let template = templates.get_untracked().into_iter().find(|template| template.id == value);
                    template_id_draft.set(value.clone());
                    has_pending_changes.set(true);
                    if let Some(template) = template {
                        base_url_draft.set(template.base_url.clone());
                        access_mode_draft.set("Smart".into());
                        endpoint_mode_draft.set(match template.kind {
                            ProviderKind::OpenAiImage => "ImagesApi".into(),
                            ProviderKind::NanoBanana | ProviderKind::OpenAiCompatible => {
                                "CustomJson".into()
                            }
                            ProviderKind::CustomHttp => "CustomJson".into(),
                        });
                        model_draft.set(match template.kind {
                            ProviderKind::OpenAiImage => "gpt-image-2".into(),
                            ProviderKind::NanoBanana | ProviderKind::OpenAiCompatible => {
                                "gemini-2.5-flash-image".into()
                            }
                            ProviderKind::CustomHttp => String::new(),
                        });
                        responses_model_draft.set(if template.kind == ProviderKind::OpenAiImage {
                            "gpt-5.5".into()
                        } else {
                            String::new()
                        });
                    }
                }
            >
                <For
                    each=move || templates.get()
                    key=|template| template.id.clone()
                    children=move |template| view! {
                        <option value=template.id.clone()>{template.name}</option>
                    }
                />
            </select>
            <input
                class="text-input"
                placeholder="Base URL"
                prop:value=move || base_url_draft.get()
                on:input=move |ev| base_url_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_base_url()
            />
            <input
                class="text-input"
                placeholder="模型名"
                list="image-model-suggestions"
                prop:value=move || model_draft.get()
                on:input=move |ev| model_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_model()
            />
            <datalist id="image-model-suggestions">
                <option value="gpt-image-2.5-flare">"GPT Image 2.5 · Flare"</option>
                <option value="gpt-image-2.5-sunburst">"GPT Image 2.5 · Sunburst"</option>
            </datalist>
            {move || {
                let show_responses_model = template_id_draft.get()
                    == BUILTIN_OPENAI_IMAGE_TEMPLATE_ID
                    && endpoint_mode_draft.get() == "ResponsesApi";
                if show_responses_model {
                    view! {
                        <input
                            class="text-input"
                            placeholder="Responses 主模型（例如 gpt-5.5）"
                            prop:value=move || responses_model_draft.get()
                            on:input=move |ev| responses_model_draft.set(event_target_value(&ev))
                            on:blur=move |_| commit_responses_model()
                        />
                    }
                    .into_any()
                } else {
                    ().into_any()
                }
            }}
            <div class="api-key-input-row">
                <input
                    class="text-input"
                    type=move || if api_key_visible.get() { "text" } else { "password" }
                    autocomplete="off"
                    placeholder="API Key"
                    prop:value=move || api_key_draft.get()
                    on:input=move |ev| {
                        api_key_draft.set(event_target_value(&ev));
                        api_key_copy_feedback.set(None);
                    }
                    on:blur=move |_| commit_api_key()
                />
                <button
                    class="button ghost icon-button api-key-tool-button"
                    title=move || if api_key_visible.get() { "隐藏 API Key" } else { "显示 API Key" }
                    on:click=move |_| api_key_visible.update(|visible| *visible = !*visible)
                >
                    {move || if api_key_visible.get() {
                        view! { <MaterialSymbolIcon name="visibility_off" filled=false /> }.into_any()
                    } else {
                        view! { <MaterialSymbolIcon name="visibility" filled=false /> }.into_any()
                    }}
                </button>
                <button
                    class="button ghost icon-button api-key-tool-button"
                    title="复制 API Key"
                    on:click=copy_api_key
                >
                    <MaterialSymbolIcon name="content_copy" filled=false />
                </button>
            </div>
            {move || api_key_copy_feedback.get().map(|message| view! {
                <p class="form-hint api-key-copy-feedback">{message}</p>
            })}
            <div class="row settings-config-actions">
                <select
                    class="select-input"
                    prop:value=move || access_mode_draft.get()
                    on:change=move |ev| {
                        access_mode_draft.set(event_target_value(&ev));
                        has_pending_changes.set(true);
                    }
                >
                    <option value="Smart">"智能切换"</option>
                    <option value="Direct">"优先直连"</option>
                    <option value="Proxy">"固定代理"</option>
                </select>
                {move || {
                    let is_openai_image = current_config
                        .get()
                        .map(|config| config.provider_kind == ProviderKind::OpenAiImage)
                        .unwrap_or(false);
                    if is_openai_image {
                        view! {
                            <select
                                class="select-input"
                                prop:value=move || endpoint_mode_draft.get()
                                on:change=move |ev| {
                                    endpoint_mode_draft.set(event_target_value(&ev));
                                    has_pending_changes.set(true);
                                }
                            >
                                <option value="ImagesApi">"Images API"</option>
                                <option value="ResponsesApi">"Responses API"</option>
                            </select>
                        }
                        .into_any()
                    } else {
                        ().into_any()
                    }
                }}
                <button
                    class="button secondary"
                    class:save-success=move || save_feedback.get()
                    on:click=save_config
                    disabled=move || !has_pending_changes.get()
                >
                    {move || if save_feedback.get() { "已保存" } else { "保存" }}
                </button>
            </div>
        </div>
    }
}
