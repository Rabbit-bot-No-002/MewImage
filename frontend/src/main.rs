mod api;
mod crypto;
mod providers;
mod storage;

use std::collections::{BTreeMap, HashMap};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gloo_file::{File, futures::read_as_bytes, futures::read_as_data_url};
use gloo_net::http::Request;
use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use leptos::{html, prelude::*, task::spawn_local};
use mew_image_shared::{
    AppPreferences, AuthRequest, AuthResponse, BUILTIN_OPENAI_IMAGE_TEMPLATE_ID,
    ConversationThread, EncryptedApiConfig, ImageAssetRef, LocalAppState, LocalTaskRecord,
    MeResponse, ProviderAccessMode, ProviderEndpointMode, ProviderKind, ProviderTemplate,
    ProxyInvokeRequest, ProxyInvokeResponse, SyncCheckpoint, SyncPullResponse, TaskStatus,
    ThemePreference, UserSummary, clamp_size, new_id, normalize_api_config, now_rfc3339,
};
use providers::{
    default_config, generate_with_strategy, hydrate_local_state, load_templates,
    prepare_sync_envelope,
};
use sha2::{Digest, Sha256};
use storage::{load_snapshot, save_snapshot};
use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, ClipboardEvent, DragEvent, Event, FileList, HtmlAnchorElement,
    HtmlCanvasElement, HtmlImageElement, HtmlInputElement, HtmlTextAreaElement, KeyboardEvent,
    MouseEvent, WheelEvent,
};

use crate::api::api_url;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[derive(Clone, PartialEq, Eq)]
struct PreviewState {
    task_id: String,
    asset_id: String,
}

#[derive(Clone, PartialEq, Eq)]
struct PreviewReferenceThumb {
    id: String,
    src: String,
}

#[derive(Clone, PartialEq)]
struct FailureLogState {
    task_id: String,
    title: String,
    summary: String,
    details: String,
}

#[derive(Clone, PartialEq, Eq)]
struct PreviewPanelState {
    task_id: String,
    asset_id: String,
    prompt: String,
    display_src: String,
    width: u32,
    height: u32,
    source_label: String,
    requested_model: String,
    moderation_label: String,
    requested_quality_label: String,
    actual_quality_label: String,
    format_label: String,
    image_count: usize,
    created_at: String,
    duration_label: String,
    favorite: bool,
    reference_thumbs: Vec<PreviewReferenceThumb>,
}

#[derive(Clone, PartialEq)]
struct ContextMenuState {
    task_id: String,
    asset_id: String,
    x: f64,
    y: f64,
}

#[derive(Clone, PartialEq)]
struct FloatingTipState {
    text: String,
    x: f64,
    y: f64,
    token: u64,
    persistent: bool,
}

#[component]
fn MaterialSymbolIcon(name: &'static str, filled: bool) -> impl IntoView {
    view! {
        <span
            class="material-symbols-rounded app-icon"
            class:is-filled=filled
            aria-hidden="true"
        >
            {name}
        </span>
    }
}

const THUMBNAIL_DATA_URL_KEY: &str = "thumbnail_data_url";
const THUMBNAIL_MAX_EDGE: u32 = 320;

#[component]
fn App() -> impl IntoView {
    let configs = RwSignal::new(Vec::<EncryptedApiConfig>::new());
    let tasks = RwSignal::new(Vec::<LocalTaskRecord>::new());
    let threads = RwSignal::new(vec![default_thread()]);
    let assets = RwSignal::new(Vec::<ImageAssetRef>::new());
    let preferences = RwSignal::new(AppPreferences::default());
    let checkpoint = RwSignal::new(SyncCheckpoint::default());
    let templates = RwSignal::new(vec![
        ProviderTemplate::builtin_openai(),
        ProviderTemplate::builtin_nano_banana(),
        ProviderTemplate::builtin_openai_compatible(),
    ]);
    let auth_user = RwSignal::new(None::<UserSummary>);
    let login_username = RwSignal::new(String::new());
    let login_password = RwSignal::new(String::new());
    let sync_secret = RwSignal::new(String::new());
    let current_thread_id = RwSignal::new(String::new());
    let current_config_id = RwSignal::new(String::new());
    let selected_reference_ids = RwSignal::new(Vec::<String>::new());
    let dragging_reference_id = RwSignal::new(None::<String>);
    let drag_over_reference_id = RwSignal::new(None::<String>);
    let reference_menu_asset_id = RwSignal::new(None::<String>);
    let continuation_asset_id = RwSignal::new(None::<String>);
    let draft_prompt = RwSignal::new(String::new());
    let draft_prompt_ref = NodeRef::<html::Textarea>::new();
    let custom_width = RwSignal::new(1024u32);
    let custom_height = RwSignal::new(1024u32);
    let resolution_mode = RwSignal::new("auto".to_string());
    let resolution_group = RwSignal::new("1k".to_string());
    let aspect_ratio = RwSignal::new("1:1".to_string());
    let quality = RwSignal::new("high".to_string());
    let count = RwSignal::new(1u32);
    let status_text = RwSignal::new("准备就绪，默认会优先使用代理兜底跨域问题。".to_string());
    let generating = RwSignal::new(false);
    let syncing = RwSignal::new(false);
    let show_favorites_only = RwSignal::new(false);
    let show_settings = RwSignal::new(false);
    let show_resolution_menu = RwSignal::new(false);
    let preview_state = RwSignal::new(None::<PreviewState>);
    let preview_panel_state = RwSignal::new(None::<PreviewPanelState>);
    let preview_fullscreen = RwSignal::new(false);
    let preview_zoom = RwSignal::new(1.0f64);
    let preview_offset_x = RwSignal::new(0.0f64);
    let preview_offset_y = RwSignal::new(0.0f64);
    let preview_dragging = RwSignal::new(false);
    let preview_drag_origin_x = RwSignal::new(0.0f64);
    let preview_drag_origin_y = RwSignal::new(0.0f64);
    let preview_drag_start_x = RwSignal::new(0.0f64);
    let preview_drag_start_y = RwSignal::new(0.0f64);
    let context_menu_state = RwSignal::new(None::<ContextMenuState>);
    let failure_log_state = RwSignal::new(None::<FailureLogState>);
    let floating_tip_state = RwSignal::new(None::<FloatingTipState>);
    let floating_tip_token = RwSignal::new(0u64);
    let persist_timer_id = RwSignal::new(None::<i32>);
    let persist_state = {
        let configs = configs;
        let tasks = tasks;
        let threads = threads;
        let assets = assets;
        let preferences = preferences;
        let checkpoint = checkpoint;
        move || {
            if persist_timer_id.get_untracked().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let callback = Closure::<dyn FnMut()>::once(move || {
                persist_timer_id.set(None);
                let snapshot =
                    snapshot_local_state(configs, tasks, threads, assets, preferences, checkpoint);
                spawn_local(async move {
                    let _ = save_snapshot(&snapshot).await;
                });
            });
            if let Ok(timer_id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                480,
            ) {
                persist_timer_id.set(Some(timer_id));
                callback.forget();
            }
        }
    };

    Effect::new(move |_| {
        apply_theme(preferences.get().theme);
    });

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let on_keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
            if ev.key() != "Escape" || preview_state.get_untracked().is_none() {
                return;
            }
            if preview_fullscreen.get_untracked() {
                preview_fullscreen.set(false);
                preview_zoom.set(1.0);
                preview_offset_x.set(0.0);
                preview_offset_y.set(0.0);
                preview_dragging.set(false);
            } else {
                preview_state.set(None);
                preview_panel_state.set(None);
                preview_fullscreen.set(false);
                preview_zoom.set(1.0);
                preview_offset_x.set(0.0);
                preview_offset_y.set(0.0);
                preview_dragging.set(false);
                context_menu_state.set(None);
            }
        });
        let _ =
            window.add_event_listener_with_callback("keydown", on_keydown.as_ref().unchecked_ref());
        on_keydown.forget();
    });

    Effect::new(move |_| {
        if let Some(tip) = floating_tip_state.get() {
            if tip.persistent {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let token = tip.token;
            let callback = Closure::<dyn FnMut()>::once(move || {
                if floating_tip_state
                    .get_untracked()
                    .map(|current| current.token == token)
                    .unwrap_or(false)
                {
                    floating_tip_state.set(None);
                }
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                1400,
            );
            callback.forget();
        }
    });

    let open_failure_log = move |task_id: String| {
        let Some(task) = tasks.with_untracked(|items| items.iter().find(|task| task.id == task_id).cloned()) else {
            return;
        };
        let raw_response = task
            .result
            .as_ref()
            .and_then(|result| result.raw_response_json.as_ref())
            .map(|value| format_failure_raw_response(value))
            .unwrap_or_else(|| "无原始响应 JSON".into());
        let details = format!(
            "任务ID: {}\n状态: {:?}\n错误: {}\n创建时间: {}\n更新时间: {}\n\n原始响应:\n{}",
            task.id,
            task.status,
            task.error_message.clone().unwrap_or_else(|| "无错误信息".into()),
            task.created_at,
            task.updated_at,
            raw_response,
        );
        failure_log_state.set(Some(FailureLogState {
            task_id: task.id.clone(),
            title: format!("失败日志：{}", task.prompt),
            summary: task.error_message.unwrap_or_else(|| "生成失败".into()),
            details,
        }));
    };

    spawn_local({
        let configs = configs;
        let tasks = tasks;
        let threads = threads;
        let assets = assets;
        let preferences = preferences;
        let checkpoint = checkpoint;
        let templates = templates;
        let auth_user = auth_user;
        let current_thread_id = current_thread_id;
        let current_config_id = current_config_id;
        let draft_prompt = draft_prompt;
        let status_text = status_text;
        async move {
            let state = load_snapshot().await.unwrap_or_else(|_| {
                let mut next = LocalAppState::default();
                next.threads = vec![default_thread()];
                next.configs
                    .push(default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID));
                next
            });
            let mut state = state;
            if state.threads.is_empty() {
                state.threads.push(default_thread());
            }
            if state.configs.is_empty() {
                state
                    .configs
                    .push(default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID));
            }
            for config in &mut state.configs {
                normalize_api_config(config);
            }
            state
                .assets
                .retain(|asset| !asset.metadata.contains_key("mask_base_asset_id"));
            for asset in &mut state.assets {
                if asset.metadata.contains_key(THUMBNAIL_DATA_URL_KEY) {
                    continue;
                }
                if let Ok(thumbnail) =
                    thumbnail_data_url_from_asset(asset, THUMBNAIL_MAX_EDGE).await
                {
                    asset
                        .metadata
                        .insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                }
            }
            reconcile_task_integrity(&mut state.tasks, &state.assets, true);
            current_thread_id.set(
                state
                    .threads
                    .first()
                    .map(|thread| thread.id.clone())
                    .unwrap_or_default(),
            );
            current_config_id.set(
                state
                    .configs
                    .first()
                    .map(|config| config.id.clone())
                    .unwrap_or_default(),
            );
            draft_prompt.set(
                state
                    .threads
                    .first()
                    .map(|thread| thread.draft_prompt.clone())
                    .unwrap_or_default(),
            );
            apply_local_state(
                state.clone(),
                configs,
                tasks,
                threads,
                assets,
                preferences,
                checkpoint,
            );
            let _ = save_snapshot(&state).await;

            if let Ok(remote_templates) = load_templates().await {
                if !remote_templates.is_empty() {
                    templates.set(remote_templates);
                }
            }

            if let Ok(response) = Request::get(&api_url("/api/auth/me"))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                if let Ok(me) = response.json::<MeResponse>().await {
                    auth_user.set(me.user);
                }
            }

            status_text.set("本地工作台已恢复，可以直接开始生成或继续修改。".into());
        }
    });

    let current_config = Memo::new(move |_| {
        configs
            .get()
            .into_iter()
            .find(|config| config.id == current_config_id.get())
    });

    Effect::new(move |_| {
        let value = draft_prompt.get();
        if let Some(textarea) = draft_prompt_ref.get() {
            if textarea.value() != value {
                textarea.set_value(&value);
            }
        }
    });

    let visible_tasks = Memo::new(move |_| {
        let thread_id = current_thread_id.get();
        let show_favorites = show_favorites_only.get();
        let mut visible: Vec<LocalTaskRecord> = tasks.with(|task_list| {
            task_list
                .iter()
                .filter(|task| task.thread_id == thread_id)
                .filter(|task| !show_favorites || task.favorite)
                .cloned()
                .collect()
        });
        visible.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        visible
    });

    let reference_assets = Memo::new(move |_| {
        let thread_id = current_thread_id.get();
        let selected_ids = selected_reference_ids.get();
        assets.with(|asset_list| {
            selected_thread_reference_assets(asset_list, &thread_id, &selected_ids)
        })
    });

    let continuation_asset = Memo::new(move |_| {
        let Some(asset_id) = continuation_asset_id.get() else {
            return None;
        };
        assets.with(|asset_list| {
            asset_list
                .iter()
                .find(|asset| asset.id == asset_id)
                .cloned()
        })
    });

    let dimension_reference_assets = Memo::new(move |_| {
        let selected_ids = selected_reference_ids.get();
        let continuation_id = continuation_asset_id.get();
        assets.with(|asset_list| {
            let mut ordered: Vec<ImageAssetRef> = asset_list
                .iter()
                .filter(|asset| selected_ids.contains(&asset.id))
                .filter(|asset| !asset.metadata.contains_key("mask_base_asset_id"))
                .cloned()
                .collect();
            if let Some(asset_id) = continuation_id.clone() {
                if let Some(asset) = asset_list
                    .iter()
                    .find(|asset| {
                        asset.id == asset_id && !asset.metadata.contains_key("mask_base_asset_id")
                    })
                    .cloned()
                {
                    ordered.retain(|item| item.id != asset.id);
                    ordered.insert(0, asset);
                }
            }
            ordered
        })
    });

    let current_reference_menu_asset = Memo::new(move |_| {
        let Some(asset_id) = reference_menu_asset_id.get() else {
            return None;
        };
        assets.with(|asset_list| {
            asset_list
                .iter()
                .find(|asset| asset.id == asset_id)
                .cloned()
        })
    });

    let current_preview = Memo::new(move |_| {
        let Some(preview) = preview_state.get() else {
            return None;
        };
        let task = tasks.with(|task_list| {
            task_list
                .iter()
                .find(|task| task.id == preview.task_id)
                .cloned()
        })?;
        let asset = assets.with(|asset_list| {
            asset_list
                .iter()
                .find(|asset| asset.id == preview.asset_id)
                .cloned()
        })?;
        Some((task, asset))
    });

    let build_preview_panel_state = move |task_id: &str, asset_id: &str| {
        let task =
            tasks.with_untracked(|items| items.iter().find(|task| task.id == task_id).cloned())?;
        let asset = assets
            .with_untracked(|items| items.iter().find(|asset| asset.id == asset_id).cloned())?;
        let preview_config = configs.with_untracked(|items| {
            items
                .iter()
                .find(|config| config.id == task.config_id)
                .cloned()
        });
        let moderation_label = preview_config
            .as_ref()
            .and_then(|config| config.moderation.clone())
            .unwrap_or_else(|| "auto".into());
        let source_label = preview_config
            .as_ref()
            .map(|config| config.name.clone())
            .unwrap_or_else(|| "默认配置".into());
        let requested_quality_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.requested_quality.clone())
            .unwrap_or_else(|| "未设置".into());
        let actual_quality_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.actual_quality.clone())
            .unwrap_or_else(|| "medium".into());
        let duration_label = task
            .result
            .as_ref()
            .and_then(|result| result.parameter_snapshot.duration_ms)
            .map(format_duration_ms)
            .unwrap_or_else(|| "未记录".into());
        let reference_thumbs = assets.with_untracked(|items| {
            task.reference_asset_ids
                .iter()
                .filter_map(|id| {
                    items
                        .iter()
                        .find(|asset| asset.id == *id)
                        .map(|asset| PreviewReferenceThumb {
                            id: id.clone(),
                            src: asset_display_src(asset),
                        })
                })
                .collect::<Vec<_>>()
        });
        Some(PreviewPanelState {
            task_id: task.id.clone(),
            asset_id: asset.id.clone(),
            prompt: task.prompt.clone(),
            display_src: asset_display_src(&asset),
            width: asset.width.unwrap_or(0),
            height: asset.height.unwrap_or(0),
            source_label,
            requested_model: task.requested_model.clone(),
            moderation_label,
            requested_quality_label,
            actual_quality_label,
            format_label: asset.mime_type.replace("image/", ""),
            image_count: task
                .result
                .as_ref()
                .map(|result| result.images.len())
                .unwrap_or(1),
            created_at: task.created_at.clone(),
            duration_label,
            favorite: task.favorite,
            reference_thumbs,
        })
    };

    let gallery_entries = Memo::new(move |_| {
        let visible = visible_tasks.get();
        assets.with(|asset_list| gallery_items(&visible, asset_list))
    });

    let update_current_config = move |updater: fn(&mut EncryptedApiConfig, String),
                                      value: String| {
        configs.update(|items| {
            if let Some(config) = items
                .iter_mut()
                .find(|config| config.id == current_config_id.get_untracked())
            {
                updater(config, value.clone());
                config.updated_at = now_rfc3339();
            }
        });
        persist_state();
    };

    let commit_current_thread_draft = move || {
        let thread_id = current_thread_id.get_untracked();
        if thread_id.is_empty() {
            return;
        }
        let value = draft_prompt_ref
            .get()
            .map(|textarea: HtmlTextAreaElement| textarea.value())
            .unwrap_or_else(|| draft_prompt.get_untracked());
        draft_prompt.set(value.clone());
        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                if thread.draft_prompt != value {
                    thread.draft_prompt = value;
                    thread.updated_at = now_rfc3339();
                }
            }
        });
    };

    let sync_action = move || {
        let Some(user) = auth_user.get_untracked() else {
            status_text.set("登录后才会启用跨设备同步。".into());
            return;
        };
        syncing.set(true);
        status_text.set("正在同步本地记录到云端……".into());
        let state = snapshot_local_state(configs, tasks, threads, assets, preferences, checkpoint);
        let secret = sync_secret.get_untracked();
        let status_signal = status_text;
        let syncing_signal = syncing;
        let persist = persist_state;
        let configs_signal = configs;
        let tasks_signal = tasks;
        let threads_signal = threads;
        let assets_signal = assets;
        let preferences_signal = preferences;
        let checkpoint_signal = checkpoint;
        spawn_local(async move {
            let envelope = match prepare_sync_envelope(
                &state,
                if secret.is_empty() {
                    None
                } else {
                    Some(secret.as_str())
                },
            ) {
                Ok(envelope) => envelope,
                Err(error) => {
                    syncing_signal.set(false);
                    status_signal.set(format!("同步前加密失败：{error}"));
                    return;
                }
            };
            let request = mew_image_shared::SyncPushRequest {
                client_updated_at: now_rfc3339(),
                envelope,
            };
            let response = Request::post(&api_url("/api/sync/push"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request);
            let Ok(builder) = response else {
                syncing_signal.set(false);
                status_signal.set("同步请求序列化失败。".into());
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<SyncPullResponse>().await {
                    Ok(pulled) => {
                        let hydrated = hydrate_local_state(
                            &state,
                            pulled.envelope,
                            pulled.checkpoint,
                            if secret.is_empty() {
                                None
                            } else {
                                Some(secret.as_str())
                            },
                        );
                        let mut hydrated = hydrated;
                        reconcile_task_integrity(&mut hydrated.tasks, &hydrated.assets, true);
                        apply_local_state(
                            hydrated,
                            configs_signal,
                            tasks_signal,
                            threads_signal,
                            assets_signal,
                            preferences_signal,
                            checkpoint_signal,
                        );
                        persist();
                        status_signal.set(format!("已完成与 {} 的云端同步。", user.username));
                    }
                    Err(error) => status_signal.set(format!("同步响应解析失败：{error}")),
                },
                Ok(response) => {
                    status_signal.set(response.text().await.unwrap_or_else(|_| "同步失败".into()));
                }
                Err(error) => status_signal.set(format!("同步失败：{error}")),
            }
            syncing_signal.set(false);
        });
    };

    let submit_auth = move |mode: &'static str| {
        let username = login_username.get_untracked();
        let password = login_password.get_untracked();
        if username.trim().is_empty() || password.is_empty() {
            status_text.set("请先填写用户名和密码。".into());
            return;
        }
        status_text.set("正在处理账号状态……".into());
        let auth_user = auth_user;
        let sync_secret = sync_secret;
        let status_text = status_text;
        spawn_local(async move {
            let request = Request::post(&api_url(if mode == "register" {
                "/api/auth/register"
            } else {
                "/api/auth/login"
            }))
            .credentials(web_sys::RequestCredentials::Include)
            .json(&AuthRequest {
                username,
                password: password.clone(),
            });
            let Ok(builder) = request else {
                status_text.set("认证请求序列化失败。".into());
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => match response.json::<AuthResponse>().await {
                    Ok(auth) => {
                        sync_secret.set(password);
                        auth_user.set(Some(auth.user.clone()));
                        status_text.set(format!(
                            "欢迎，{}。现在可以进行跨设备同步。",
                            auth.user.username
                        ));
                    }
                    Err(error) => status_text.set(format!("认证响应解析失败：{error}")),
                },
                Ok(response) => {
                    status_text.set(response.text().await.unwrap_or_else(|_| "认证失败".into()));
                }
                Err(error) => status_text.set(format!("认证失败：{error}")),
            }
        });
    };

    let add_config = move |_| {
        let template = templates
            .get_untracked()
            .first()
            .cloned()
            .unwrap_or_else(ProviderTemplate::builtin_openai);
        configs.update(|items| {
            let mut config = default_config(&template.id);
            config.name = format!("{} 配置", template.name);
            config.base_url = template.base_url.clone();
            config.provider_kind = template.kind;
            config.known_requires_proxy = template.known_requires_proxy;
            normalize_api_config(&mut config);
            items.push(config);
            if let Some(last) = items.last() {
                current_config_id.set(last.id.clone());
            }
        });
        persist_state();
    };

    let new_thread = move |_| {
        commit_current_thread_draft();
        let thread = default_thread();
        current_thread_id.set(thread.id.clone());
        draft_prompt.set(String::new());
        selected_reference_ids.set(Vec::new());
        reference_menu_asset_id.set(None);
        continuation_asset_id.set(None);
        threads.update(|items| items.push(thread));
        persist_state();
        status_text.set("已新建会话，可以开始新的连续修改。".into());
    };

    let rename_thread = move |thread_id: String| {
        let current_name = threads
            .get_untracked()
            .iter()
            .find(|thread| thread.id == thread_id)
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| "新的会话".into());
        let next_name = web_sys::window()
            .and_then(|window| {
                window
                    .prompt_with_message_and_default("重命名会话", &current_name)
                    .ok()
                    .flatten()
            })
            .unwrap_or_default();
        if next_name.trim().is_empty() {
            return;
        }
        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                thread.title = next_name.trim().to_string();
                thread.updated_at = now_rfc3339();
            }
        });
        persist_state();
    };

    let delete_thread = move |thread_id: String| {
        if !confirm_action("删除会话后，会连同该会话的记录与参考图一起移除，是否继续？")
        {
            return;
        }
        let removed_task_ids: Vec<String> = tasks
            .get_untracked()
            .iter()
            .filter(|task| task.thread_id == thread_id)
            .map(|task| task.id.clone())
            .collect();
        let removed_asset_ids: Vec<String> = assets
            .get_untracked()
            .iter()
            .filter(|asset| {
                asset
                    .metadata
                    .get("thread_id")
                    .map(|id| id == &thread_id)
                    .unwrap_or(false)
                    || asset
                        .source_task_id
                        .as_ref()
                        .map(|task_id| removed_task_ids.contains(task_id))
                        .unwrap_or(false)
            })
            .map(|asset| asset.id.clone())
            .collect();
        tasks.update(|items| items.retain(|task| task.thread_id != thread_id));
        assets.update(|items| {
            items.retain(|asset| {
                !(asset
                    .metadata
                    .get("thread_id")
                    .map(|id| id == &thread_id)
                    .unwrap_or(false)
                    || asset
                        .source_task_id
                        .as_ref()
                        .map(|task_id| removed_task_ids.contains(task_id))
                        .unwrap_or(false))
            });
        });
        threads.update(|items| {
            items.retain(|thread| thread.id != thread_id);
            if items.is_empty() {
                items.push(default_thread());
            }
        });
        selected_reference_ids.update(|ids| ids.retain(|id| !removed_asset_ids.contains(id)));
        if continuation_asset_id
            .get_untracked()
            .as_ref()
            .map(|id| removed_asset_ids.contains(id))
            .unwrap_or(false)
        {
            continuation_asset_id.set(None);
        }
        if current_thread_id.get_untracked() == thread_id {
            let fallback = threads
                .get_untracked()
                .first()
                .cloned()
                .unwrap_or_else(default_thread);
            current_thread_id.set(fallback.id.clone());
            draft_prompt.set(fallback.draft_prompt);
            selected_reference_ids.set(Vec::new());
            reference_menu_asset_id.set(None);
            continuation_asset_id.set(None);
        }
        persist_state();
        status_text.set("会话已删除。".into());
    };

    let import_reference_assets = move |files: FileList| {
        let assets_signal = assets;
        let selected_reference_ids = selected_reference_ids;
        let status_text = status_text;
        let persist = persist_state;
        let thread_id = current_thread_id.get_untracked();
        spawn_local(async move {
            match import_file_list(files).await {
                Ok(mut imported) => {
                    let existing_hashes = assets_signal.with_untracked(|items| {
                        items
                            .iter()
                            .map(|asset| asset.sha256.clone())
                            .collect::<std::collections::HashSet<_>>()
                    });
                    imported.retain(|asset| !existing_hashes.contains(&asset.sha256));
                    if imported.is_empty() {
                        status_text.set("已自动去重，未导入重复参考图。".into());
                        return;
                    }
                    for asset in &mut imported {
                        asset.metadata.insert("thread_id".into(), thread_id.clone());
                        if let Ok(thumbnail) =
                            thumbnail_data_url_from_asset(asset, THUMBNAIL_MAX_EDGE).await
                        {
                            asset
                                .metadata
                                .insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                        }
                    }
                    let ids: Vec<String> = imported.iter().map(|asset| asset.id.clone()).collect();
                    assets_signal.update(|items| items.extend(imported));
                    selected_reference_ids.update(|current| {
                        for id in ids {
                            if !current.contains(&id) {
                                current.push(id);
                            }
                        }
                    });
                    persist();
                    status_text.set("图片已导入，可点击缩略图打开参考图操作菜单。".into());
                }
                Err(error) => status_text.set(format!("导入图片失败：{error}")),
            }
        });
    };

    let open_reference_menu = move |asset_id: String| {
        reference_menu_asset_id.set(Some(asset_id));
    };

    let reorder_selected_references = move |dragged_id: String, target_id: String| {
        if dragged_id == target_id {
            return;
        }
        selected_reference_ids.update(|ids| {
            let Some(from_index) = ids.iter().position(|id| id == &dragged_id) else {
                return;
            };
            let Some(to_index) = ids.iter().position(|id| id == &target_id) else {
                return;
            };
            let item = ids.remove(from_index);
            ids.insert(to_index, item);
        });
        persist_state();
    };

    let delete_asset = move |asset_id: String| {
        if !confirm_action(
            "删除后将从当前浏览器移除这张参考图，并让所有引用它的结果失效，是否继续？",
        )
        {
            return;
        }
        assets.update(|items| items.retain(|asset| asset.id != asset_id));
        selected_reference_ids.update(|ids| ids.retain(|id| id != &asset_id));
        if dragging_reference_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            dragging_reference_id.set(None);
        }
        if reference_menu_asset_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            reference_menu_asset_id.set(None);
        }
        if continuation_asset_id.get_untracked().as_deref() == Some(asset_id.as_str()) {
            continuation_asset_id.set(None);
        }
        persist_state();
        status_text.set("参考图已删除。".into());
    };

    let continue_from_task = move |task_id: String| {
        let task_list = tasks.get_untracked();
        let Some(task) = task_list.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        selected_reference_ids.set(task.reference_asset_ids.clone());
        reference_menu_asset_id.set(None);
        current_thread_id.set(task.thread_id.clone());
        draft_prompt.set(task.prompt.clone());
        continuation_asset_id.set(None);
        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == task.thread_id) {
                thread.draft_prompt = task.prompt.clone();
                thread.updated_at = now_rfc3339();
            }
        });
        persist_state();
        status_text.set("已复用配置，下一次会继续沿用该提示词和参考图。".into());
    };

    let enter_continuation_context = move |task_id: String, asset_id: String| {
        let task_list = tasks.get_untracked();
        let Some(task) = task_list.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        current_thread_id.set(task.thread_id.clone());
        draft_prompt.set(task.prompt.clone());
        selected_reference_ids.set(task.reference_asset_ids.clone());
        continuation_asset_id.set(Some(asset_id));
        reference_menu_asset_id.set(None);
        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == task.thread_id) {
                thread.draft_prompt = task.prompt.clone();
                thread.updated_at = now_rfc3339();
            }
        });
        persist_state();
        status_text.set("已进入连续修改模式。".into());
    };

    let delete_task = move |task_id: String| {
        if !confirm_action("删除后会从当前浏览器移除这条生成记录和对应图片，是否继续？")
        {
            return;
        }
        let removed_asset_ids: Vec<String> = assets
            .get_untracked()
            .iter()
            .filter(|asset| asset.source_task_id.as_deref() == Some(task_id.as_str()))
            .map(|asset| asset.id.clone())
            .collect();
        assets.update(|items| {
            items.retain(|asset| asset.source_task_id.as_deref() != Some(task_id.as_str()));
        });
        tasks.update(|items| items.retain(|task| task.id != task_id));
        threads.update(|items| {
            for thread in items {
                thread.task_ids.retain(|id| id != &task_id);
            }
        });
        selected_reference_ids.update(|ids| ids.retain(|id| !removed_asset_ids.contains(id)));
        if let Some(asset_id) = continuation_asset_id.get_untracked() {
            if removed_asset_ids.contains(&asset_id) {
                continuation_asset_id.set(None);
            }
        }
        persist_state();
        status_text.set("历史记录已删除。".into());
    };

    let open_preview = move |task_id: String, asset_id: String| {
        preview_panel_state.set(build_preview_panel_state(&task_id, &asset_id));
        preview_state.set(Some(PreviewState { task_id, asset_id }));
        preview_fullscreen.set(false);
        preview_zoom.set(1.0);
        preview_offset_x.set(0.0);
        preview_offset_y.set(0.0);
        preview_dragging.set(false);
        context_menu_state.set(None);
    };

    let close_preview = move || {
        preview_state.set(None);
        preview_panel_state.set(None);
        preview_fullscreen.set(false);
        preview_zoom.set(1.0);
        preview_offset_x.set(0.0);
        preview_offset_y.set(0.0);
        preview_dragging.set(false);
        context_menu_state.set(None);
    };

    let edit_output_asset = move |task_id: String, asset_id: String| {
        enter_continuation_context(task_id, asset_id);
        close_preview();
        show_settings.set(false);
    };

    let show_tip = move |text: &str, x: f64, y: f64, persistent: bool| {
        let token = floating_tip_token.get_untracked().saturating_add(1);
        floating_tip_token.set(token);
        floating_tip_state.set(Some(FloatingTipState {
            text: text.into(),
            x,
            y,
            token,
            persistent,
        }));
    };

    let hide_tip = move || {
        floating_tip_state.set(None);
    };

    let reference_tip_enabled = move || !preview_panel_state
        .get()
        .map(|panel| panel.reference_thumbs.is_empty())
        .unwrap_or(true);

    let run_generation = move || {
        if generating.get_untracked() {
            return;
        }
        let Some(config) = current_config.get_untracked() else {
            status_text.set("请先在设置中准备一个服务商配置。".into());
            return;
        };
        if config
            .api_key_plaintext
            .clone()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            status_text.set("请先在设置中填写 API Key。".into());
            show_settings.set(true);
            return;
        }
        let prompt = draft_prompt.get_untracked();
        let prompt = draft_prompt_ref
            .get()
            .map(|textarea: HtmlTextAreaElement| textarea.value())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(prompt);
        if prompt.trim().is_empty() {
            status_text.set("请输入提示词后再开始生成。".into());
            return;
        }
        let thread_id = current_thread_id.get_untracked();
        let template = templates
            .get_untracked()
            .into_iter()
            .find(|template| template.id == config.provider_template_id)
            .unwrap_or_else(ProviderTemplate::builtin_openai);
        commit_current_thread_draft();
        let selected_ids = selected_reference_ids.get_untracked();
        let mut references =
            selected_thread_reference_assets(&assets.get_untracked(), &thread_id, &selected_ids);
        references.truncate(16);
        if let Some(asset_id) = continuation_asset_id.get_untracked() {
            if let Some(asset) = assets
                .get_untracked()
                .iter()
                .find(|asset| {
                    asset.id == asset_id && !asset.metadata.contains_key("mask_base_asset_id")
                })
                .cloned()
            {
                references.retain(|item| item.id != asset.id);
                references.insert(0, asset);
                references.truncate(16);
            }
        }
        let (resolved_width, resolved_height) = resolve_dimensions(
            resolution_mode.get_untracked().as_str(),
            resolution_group.get_untracked().as_str(),
            aspect_ratio.get_untracked().as_str(),
            custom_width.get_untracked(),
            custom_height.get_untracked(),
            &references,
        );
        custom_width.set(resolved_width);
        custom_height.set(resolved_height);

        let task_id = new_id();
        generating.set(true);
        status_text.set("正在发起生成请求，已优先启用更稳的代理策略……".into());

        threads.update(|items| {
            if let Some(thread) = items.iter_mut().find(|thread| thread.id == thread_id) {
                thread.draft_prompt = prompt.clone();
                thread.updated_at = now_rfc3339();
                if !thread.task_ids.contains(&task_id) {
                    thread.task_ids.push(task_id.clone());
                }
                if thread.title == "新的会话" {
                    thread.title = summarize_prompt(&prompt);
                }
            }
        });
        tasks.update(|items| {
            items.push(LocalTaskRecord {
                id: task_id.clone(),
                thread_id: thread_id.clone(),
                config_id: config.id.clone(),
                prompt: prompt.clone(),
                requested_model: config.model.clone(),
                reference_asset_ids: selected_ids.clone(),
                result: None,
                favorite: false,
                status: TaskStatus::Running,
                error_message: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            });
        });
        persist_state();

        let tasks_signal = tasks;
        let assets_signal = assets;
        let status_signal = status_text;
        let generating_signal = generating;
        let continuation_signal = continuation_asset_id;
        let persist = persist_state;
        let quality_value = quality.get_untracked();
        let count_value = count.get_untracked();
        spawn_local(async move {
            let request = mew_image_shared::GenerationRequest {
                prompt: prompt.clone(),
                model: config.model.clone(),
                width: resolved_width,
                height: resolved_height,
                quality: Some(quality_value),
                count: count_value,
                endpoint_mode: config.endpoint_mode,
                reference_assets: references,
            };
            match generate_with_strategy(&template, &config, &request).await {
                Ok((result, used_proxy)) => {
                    let mut produced_assets = Vec::new();
                    let mut asset_build_errors = Vec::new();
                    for (index, image) in result.images.iter().enumerate() {
                        let asset_payload = match (image.data_url.clone(), image.url.clone()) {
                            (Some(data_url), _) => {
                                let byte_len = data_url.len() as u64;
                                let sha256 = sha256_hex(data_url.as_bytes());
                                Some((
                                    Some(data_url),
                                    None,
                                    byte_len,
                                    sha256,
                                    "image/png".to_string(),
                                ))
                            }
                            (None, Some(url)) => match fetch_image_bytes(&url).await {
                                Ok((bytes, mime_type)) => {
                                    let data_url = bytes_to_data_url(&bytes, &mime_type);
                                    let byte_len = bytes.len() as u64;
                                    let sha256 = sha256_hex(&bytes);
                                    Some((Some(data_url), Some(url), byte_len, sha256, mime_type))
                                }
                                Err(error) => {
                                    asset_build_errors.push(format!(
                                        "第 {} 张结果下载失败：{}",
                                        index + 1,
                                        error
                                    ));
                                    None
                                }
                            },
                            (None, None) => {
                                asset_build_errors
                                    .push(format!("第 {} 张结果缺少图像数据。", index + 1));
                                None
                            }
                        };
                        let Some((data_url, remote_url, byte_len, sha256, mime_type)) =
                            asset_payload
                        else {
                            continue;
                        };
                        let (actual_width, actual_height) = match data_url.as_deref() {
                            Some(data_url) => load_image_dimensions(data_url)
                                .await
                                .unwrap_or((resolved_width, resolved_height)),
                            None => (resolved_width, resolved_height),
                        };
                        let mut metadata = BTreeMap::new();
                        let thumbnail_source = ImageAssetRef {
                            id: String::new(),
                            sha256: sha256.clone(),
                            mime_type: mime_type.clone(),
                            byte_len,
                            width: Some(actual_width),
                            height: Some(actual_height),
                            created_at: String::new(),
                            updated_at: String::new(),
                            data_url: data_url.clone(),
                            remote_object_key: None,
                            remote_url: remote_url.clone(),
                            source_task_id: None,
                            metadata: BTreeMap::new(),
                        };
                        if let Ok(thumbnail) =
                            thumbnail_data_url_from_asset(&thumbnail_source, THUMBNAIL_MAX_EDGE)
                                .await
                        {
                            metadata.insert(THUMBNAIL_DATA_URL_KEY.into(), thumbnail);
                        }
                        produced_assets.push(ImageAssetRef {
                            id: new_id(),
                            sha256,
                            mime_type,
                            byte_len,
                            width: Some(actual_width),
                            height: Some(actual_height),
                            created_at: now_rfc3339(),
                            updated_at: now_rfc3339(),
                            data_url,
                            remote_object_key: None,
                            remote_url,
                            source_task_id: Some(task_id.clone()),
                            metadata,
                        });
                    }
                    if produced_assets.is_empty() {
                        let upstream_count = result
                            .images
                            .iter()
                            .filter(|image| {
                                image
                                    .data_url
                                    .as_deref()
                                    .map(|value| !value.trim().is_empty())
                                    .unwrap_or(false)
                                    || image
                                        .url
                                        .as_deref()
                                        .map(|value| !value.trim().is_empty())
                                        .unwrap_or(false)
                            })
                            .count();
                        let error = if upstream_count == 0 {
                            "上游没有返回任何可用图片结果。".to_string()
                        } else if !asset_build_errors.is_empty() {
                            format!(
                                "上游返回了结果，但没有任何图片成功写入本地。{}",
                                asset_build_errors.join("；")
                            )
                        } else {
                            "上游结果未能落成本地可用图片，可能是网络、尺寸或响应异常导致。"
                                .to_string()
                        };
                        tasks_signal.update(|items| {
                            if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                                task.status = TaskStatus::Failed;
                                task.updated_at = now_rfc3339();
                                task.result = Some(result.clone());
                                task.error_message = Some(error.clone());
                            }
                        });
                        persist();
                        status_signal.set(format!("生成失败：{error}"));
                        generating_signal.set(false);
                        return;
                    }
                    let (actual_width, actual_height) = produced_assets
                        .iter()
                        .find_map(|asset| asset.width.zip(asset.height))
                        .unwrap_or((resolved_width, resolved_height));
                    let first_generated_id = produced_assets.first().map(|asset| asset.id.clone());
                    assets_signal.update(|items| {
                        items.extend(produced_assets);
                    });
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            let mut result = result;
                            result.parameter_snapshot.actual_width = Some(actual_width);
                            result.parameter_snapshot.actual_height = Some(actual_height);
                            task.status = TaskStatus::Succeeded;
                            task.updated_at = now_rfc3339();
                            task.result = Some(result);
                        }
                    });
                    continuation_signal.set(first_generated_id);
                    persist();
                    status_signal.set(if !asset_build_errors.is_empty() {
                        format!(
                            "生成完成，但有 {} 张结果未能保存到本地。{}",
                            asset_build_errors.len(),
                            asset_build_errors.join("；")
                        )
                    } else if used_proxy {
                        "生成完成，已自动进入连续修改模式，并通过同源代理绕过跨域限制。".into()
                    } else {
                        "生成完成，已自动进入连续修改模式，结果已写入当前会话。".into()
                    });
                }
                Err(error) => {
                    tasks_signal.update(|items| {
                        if let Some(task) = items.iter_mut().find(|task| task.id == task_id) {
                            task.status = TaskStatus::Failed;
                            task.updated_at = now_rfc3339();
                            task.error_message = Some(error.clone());
                        }
                    });
                    persist();
                    status_signal.set(format!("生成失败：{error}"));
                }
            }
            generating_signal.set(false);
        });
    };

    let rerun_task = move |task_id: String| {
        continue_from_task(task_id);
        run_generation();
    };

    let generate = move |_| run_generation();

    view! {
        <div class="shell shell-single">
            <header class="panel topbar">
                <div class="brand brand-inline">
                    <h1>"MewImage"</h1>
                    <span class="muted">"本地优先、同步增强、代理兜底"</span>
                </div>
                <div class="row topbar-actions">
                    <button class="button ghost" on:click=move |_| {
                        preferences.update(|value| {
                            value.theme = if value.theme == ThemePreference::Day {
                                ThemePreference::Night
                            } else {
                                ThemePreference::Day
                            };
                        });
                        persist_state();
                    }>
                        {move || if preferences.get().theme == ThemePreference::Day { "夜间模式" } else { "白天模式" }}
                    </button>
                    <button class="button secondary" on:click=move |_| show_settings.update(|value| *value = !*value)>
                        {move || if show_settings.get() { "收起设置" } else { "打开设置" }}
                    </button>
                </div>
            </header>

            {move || if show_settings.get() {
                view! {
                    <section class="panel settings-panel">
                        <div class="settings-grid">
                            <section class="stack">
                                <div class="row">
                                    <h2>"账号与同步"</h2>
                                    <span class="tag">{move || auth_user.get().map(|user| user.username).unwrap_or_else(|| "游客模式".into())}</span>
                                </div>
                                <input
                                    class="text-input"
                                    placeholder="用户名"
                                    prop:value=move || login_username.get()
                                    on:input=move |ev| login_username.set(event_target_value(&ev))
                                />
                                <input
                                    class="text-input"
                                    type="password"
                                    placeholder="密码 / 同步口令"
                                    prop:value=move || login_password.get()
                                    on:input=move |ev| login_password.set(event_target_value(&ev))
                                />
                                <div class="row">
                                    <button class="button" on:click=move |_| submit_auth("login")>"登录"</button>
                                    <button class="button secondary" on:click=move |_| submit_auth("register")>"注册"</button>
                                    <button class="button ghost" on:click=move |_| sync_action() disabled=move || syncing.get()>
                                        {move || if syncing.get() { "同步中…" } else { "立即同步" }}
                                    </button>
                                </div>
                                <span class="status">
                                    {move || auth_user.get().map(|user| format!("当前账号：{}", user.username)).unwrap_or_else(|| "未登录时所有数据仍保存在当前浏览器。".into())}
                                </span>
                            </section>

                            <section class="stack">
                                <div class="row">
                                    <h2>"服务商配置"</h2>
                                    <button class="button ghost" on:click=add_config>"新增配置"</button>
                                </div>
                                <select
                                    class="select-input"
                                    prop:value=move || current_config_id.get()
                                    on:change=move |ev| current_config_id.set(event_target_value(&ev))
                                >
                                    <For
                                        each=move || configs.get()
                                        key=|config| config.id.clone()
                                        children=move |config| view! {
                                            <option value=config.id.clone()>{config.name}</option>
                                        }
                                    />
                                </select>
                                <ConfigEditor
                                    configs=configs
                                    current_config_id=current_config_id
                                    templates=templates
                                    persist=persist_state
                                />
                            </section>
                        </div>
                    </section>
                }.into_any()
            } else {
                ().into_any()
            }}

            <main class="workspace-layout">
                <aside class="panel gallery-sidebar">
                    <div class="row">
                        <h2>"结果画廊"</h2>
                        <div class="row">
                            <button class="button ghost" on:click=move |_| show_favorites_only.update(|value| *value = !*value)>
                                {move || if show_favorites_only.get() { "显示全部" } else { "仅收藏" }}
                            </button>
                            <span class="tag">{move || format!("{} 条任务", visible_tasks.get().len())}</span>
                        </div>
                    </div>
                    <div class="gallery sidebar-gallery">
                        <For
                            each=move || gallery_entries.get()
                            key=|item| item.key.clone()
                                children=move |item| {
                                    let asset_id = item.asset_id.clone();
                                    let task_id = item.task_id.clone();
                                    let show_failure_log = tasks.with_untracked(|items| {
                                        items
                                            .iter()
                                            .find(|task| task.id == task_id)
                                            .and_then(|task| task.error_message.clone())
                                            .is_some()
                                    });
                                    let rerun_task_id = task_id.clone();
                                    let continue_task_id = task_id.clone();
                                    let delete_task_id = task_id.clone();
                                    let favorite_task_id = task_id.clone();
                                    let favorite_title_task_id = task_id.clone();
                                    let favorite_icon_task_id = task_id.clone();
                                    let favorite_fill_task_id = task_id.clone();
                                    let preview_task_id = task_id.clone();
                                    let preview_asset_id = asset_id.clone();
                                    let context_task_id = task_id.clone();
                                    let context_asset_id = asset_id.clone();
                                    view! {
                                        <article class="card gallery-card-compact">
                                            {item.src.clone().map(|src| {
                                                let preview_src = src.clone();
                                                let ratio_label = item.ratio_label.clone();
                                                let size_label = item.size_label.clone();
                                                view! {
                                                    <button
                                                        class="image-button compact-preview-button"
                                                        on:click=move |_| {
                                                            if let Some(asset_id) = preview_asset_id.clone() {
                                                                open_preview(preview_task_id.clone(), asset_id);
                                                            }
                                                        }
                                                        on:contextmenu=move |ev: MouseEvent| {
                                                            ev.prevent_default();
                                                            if let Some(asset_id) = context_asset_id.clone() {
                                                                context_menu_state.set(Some(ContextMenuState {
                                                                    task_id: context_task_id.clone(),
                                                                    asset_id,
                                                                    x: ev.client_x() as f64,
                                                                    y: ev.client_y() as f64,
                                                                }));
                                                            }
                                                        }
                                                    >
                                                        <div class="gallery-image-overlay">
                                                            <span class="gallery-corner-badge">{ratio_label}</span>
                                                            <span class="gallery-corner-badge">{size_label}</span>
                                                        </div>
                                                        <img class="compact-preview-image" src=preview_src alt=item.prompt.clone() />
                                                    </button>
                                                }.into_any()
                                            }).unwrap_or_else(|| view! { <div class="compact-preview-fallback muted">"无预览"</div> }.into_any())}
                                            <div class="card-body stack compact-card-body">
                                                <p class="gallery-card-title">{item.prompt.clone()}</p>
                                                <div class="gallery-meta">
                                                    <span class="gallery-badge">{item.model.clone()}</span>
                                                </div>
                                                <div class="row compact-actions">
                                                    <button class="button ghost mini-action icon-action" title="重新生成" on:click=move |_| rerun_task(rerun_task_id.clone())><MaterialSymbolIcon name="restart_alt" filled=false /></button>
                                                    <button class="button ghost mini-action icon-action" title="继续修改" on:click=move |_| {
                                                        if let Some(first_asset) = assets.with_untracked(|items| {
                                                            items.iter().find(|asset| asset.source_task_id.as_deref() == Some(continue_task_id.as_str())).cloned()
                                                        }) {
                                                            enter_continuation_context(continue_task_id.clone(), first_asset.id);
                                                        }
                                                    }><MaterialSymbolIcon name="edit_square" filled=false /></button>
                                                    <button class="button ghost mini-action icon-action" on:click=move |_| {
                                                        tasks.update(|items| {
                                                            if let Some(found) = items.iter_mut().find(|task| task.id == favorite_task_id) {
                                                                found.favorite = !found.favorite;
                                                            }
                                                        });
                                                        persist_state();
                                                    } title=move || {
                                                        if tasks.with(|items| {
                                                            items.iter()
                                                                .find(|task| task.id == favorite_title_task_id)
                                                                .map(|task| task.favorite)
                                                                .unwrap_or(item.favorite)
                                                        }) {
                                                            "取消收藏"
                                                        } else {
                                                            "收藏"
                                                        }
                                                    }>
                                                        <span
                                                            class="material-symbols-rounded app-icon"
                                                            class:is-filled=move || {
                                                                tasks.with(|items| {
                                                                    items.iter()
                                                                        .find(|task| task.id == favorite_fill_task_id)
                                                                        .map(|task| task.favorite)
                                                                        .unwrap_or(item.favorite)
                                                                })
                                                            }
                                                            aria-hidden="true"
                                                        >
                                                            {move || {
                                                                if tasks.with(|items| {
                                                                    items.iter()
                                                                        .find(|task| task.id == favorite_icon_task_id)
                                                                        .map(|task| task.favorite)
                                                                        .unwrap_or(item.favorite)
                                                                }) {
                                                                    "star"
                                                                } else {
                                                                    "star_outline"
                                                                }
                                                            }}
                                                        </span>
                                                    </button>
                                                    {if show_failure_log {
                                                        view! {
                                                            <button class="button ghost mini-action icon-action" title="查看失败日志" on:click=move |_| open_failure_log(task_id.clone())>
                                                                <MaterialSymbolIcon name="receipt_long" filled=false />
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    <button class="button ghost danger mini-action icon-action" title="删除记录" on:click=move |_| delete_task(delete_task_id.clone())><MaterialSymbolIcon name="delete" filled=false /></button>
                                                </div>
                                            </div>
                                        </article>
                                    }
                                }
                            />
                    </div>
                </aside>

                <div class="workspace-main">
                <section class="panel composer-panel">
                    <div class="row">
                        <h2>"提示词与生成"</h2>
                        <span class="tag">
                            {move || {
                                current_config
                                    .get()
                                    .map(|config| format!("{} · {}", config.name, config.model))
                                    .unwrap_or_else(|| "未配置模型".into())
                            }}
                        </span>
                    </div>

                    <div class="thread-strip">
                        <For
                            each=move || threads.get()
                            key=|thread| thread.id.clone()
                            children=move |thread| {
                                let thread_id = thread.id.clone();
                                let active_thread_id = thread_id.clone();
                                let click_thread_id = thread_id.clone();
                                let rename_thread_id = thread_id.clone();
                                let delete_thread_id = thread_id.clone();
                                view! {
                                    <div class="thread-chip">
                                        <button
                                            class="chip-button thread-chip-button"
                                            class:active-chip=move || current_thread_id.get() == active_thread_id
                                        on:click=move |_| {
                                            if current_thread_id.get_untracked() == click_thread_id {
                                                return;
                                            }
                                            commit_current_thread_draft();
                                            current_thread_id.set(click_thread_id.clone());
                                            if let Some(selected_thread) = threads.with_untracked(|items| {
                                                items.iter().find(|item| item.id == click_thread_id).cloned()
                                            }) {
                                                draft_prompt.set(selected_thread.draft_prompt.clone());
                                            }
                                            selected_reference_ids.set(Vec::new());
                                            reference_menu_asset_id.set(None);
                                            continuation_asset_id.set(None);
                                        }
                                    >
                                            <span class="thread-chip-label">
                                                {move || {
                                                    thread_display_name(&thread)
                                                }}
                                            </span>
                                    </button>
                                        <div class="thread-chip-actions">
                                            <button
                                                class="button ghost mini-action icon-action"
                                                title="重命名会话"
                                                on:click=move |_| rename_thread(rename_thread_id.clone())
                                            >
                                                <MaterialSymbolIcon name="edit_square" filled=false />
                                            </button>
                                            <button
                                                class="button ghost danger mini-action icon-action"
                                                title="删除会话"
                                                on:click=move |_| delete_thread(delete_thread_id.clone())
                                            >
                                                <MaterialSymbolIcon name="delete" filled=false />
                                            </button>
                                        </div>
                                    </div>
                                }
                            }
                        />
                        <button class="chip-button add-chip" on:click=new_thread>"+" "新会话"</button>
                    </div>

                    <textarea
                        class="prompt-input"
                        prop:value=move || draft_prompt.get()
                        node_ref=draft_prompt_ref
                        placeholder="输入你想要的画面，例如：软萌猫耳少女，奶油色光影，樱花飘落"
                        on:input=move |ev| {
                            draft_prompt.set(event_target_value(&ev));
                        }
                        on:blur=move |_| {
                            commit_current_thread_draft();
                            persist_state();
                        }
                    />

                    {move || continuation_asset.get().map(|asset| {
                        let clear_asset = asset.id.clone();
                        view! {
                            <div class="continuation-banner">
                                <div class="row">
                                    <div class="row">
                                        <img class="continuation-thumb" src=asset_src(&asset) alt="连续修改底图" />
                                        <div class="stack">
                                            <strong>"连续修改模式"</strong>
                                            <span class="status">"下一次会基于上一张输出继续生成，不会加入参考图队列。"</span>
                                        </div>
                                    </div>
                                    <button class="button ghost" on:click=move |_| {
                                        if continuation_asset_id.get_untracked().as_deref() == Some(clear_asset.as_str()) {
                                            continuation_asset_id.set(None);
                                            selected_reference_ids.set(Vec::new());
                                            status_text.set("已退出连续修改模式。".into());
                                        }
                                    }>"清除上下文"</button>
                                </div>
                            </div>
                        }.into_any()
                    }).unwrap_or_else(|| ().into_any())}

                    <div class="settings-inline">
                        <button
                            class="resolution-button"
                            on:click=move |_| show_resolution_menu.update(|value| *value = !*value)
                        >
                            {move || {
                                let (width, height) = resolve_dimensions(
                                    resolution_mode.get().as_str(),
                                    resolution_group.get().as_str(),
                                    aspect_ratio.get().as_str(),
                                    custom_width.get(),
                                    custom_height.get(),
                                    &dimension_reference_assets.get(),
                                );
                                format!("分辨率：{} × {}", width, height)
                            }}
                        </button>
                        {move || if current_config.get().map(|config| is_openai_image_model(&config)).unwrap_or(false) {
                            view! {
                                <>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || quality.get()
                                        on:change=move |ev| quality.set(event_target_value(&ev))
                                    >
                                        <option value="low">"质量：低"</option>
                                        <option value="medium">"质量：中"</option>
                                        <option value="high">"质量：高"</option>
                                    </select>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || current_config.get().and_then(|config| config.output_format).unwrap_or_else(|| "png".into())
                                        on:change=move |ev| update_current_config(|config, value| config.output_format = Some(value), event_target_value(&ev))
                                    >
                                        <option value="png">"格式：PNG"</option>
                                        <option value="jpeg">"格式：JPEG"</option>
                                        <option value="webp">"格式：WEBP"</option>
                                    </select>
                                    <input
                                        class="field compact-field"
                                        type="number"
                                        min="0"
                                        max="100"
                                        prop:value=move || current_config.get().and_then(|config| config.output_compression).unwrap_or(100).to_string()
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev).parse::<u8>().unwrap_or(100).clamp(0, 100);
                                            configs.update(|items| {
                                                if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                                    config.output_compression = Some(value);
                                                    config.updated_at = now_rfc3339();
                                                }
                                            });
                                            persist_state();
                                        }
                                    />
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || current_config.get().and_then(|config| config.moderation).unwrap_or_else(|| "auto".into())
                                        on:change=move |ev| update_current_config(|config, value| config.moderation = Some(value), event_target_value(&ev))
                                    >
                                        <option value="auto">"审核：自动"</option>
                                        <option value="low">"审核：宽松"</option>
                                    </select>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || {
                                            current_config
                                                .get()
                                                .map(|config| if config.prompt_guard_enabled { "on".to_string() } else { "off".to_string() })
                                                .unwrap_or_else(|| "off".to_string())
                                        }
                                        on:change=move |ev| {
                                            let value = event_target_value(&ev);
                                            configs.update(|items| {
                                                if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                                    config.prompt_guard_enabled = value == "on";
                                                    config.updated_at = now_rfc3339();
                                                }
                                            });
                                            persist_state();
                                        }
                                    >
                                        <option value="on">"Codex 兼容：开"</option>
                                        <option value="off">"Codex 兼容：关"</option>
                                    </select>
                                    <input
                                        class="field compact-field"
                                        type="number"
                                        min="1"
                                        max="4"
                                        prop:value=move || count.get().to_string()
                                        on:input=move |ev| count.set(event_target_value(&ev).parse().unwrap_or(1).clamp(1, 4))
                                    />
                                </>
                            }.into_any()
                        } else {
                            view! {
                                <>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || quality.get()
                                        on:change=move |ev| quality.set(event_target_value(&ev))
                                    >
                                        <option value="low">"质量：低"</option>
                                        <option value="medium">"质量：中"</option>
                                        <option value="high">"质量：高"</option>
                                    </select>
                                    <input
                                        class="field compact-field"
                                        type="number"
                                        min="1"
                                        max="4"
                                        prop:value=move || count.get().to_string()
                                        on:input=move |ev| count.set(event_target_value(&ev).parse().unwrap_or(1).clamp(1, 4))
                                    />
                                </>
                            }.into_any()
                        }}
                    </div>

                    {move || if show_resolution_menu.get() {
                        let (preview_width, preview_height) = resolve_dimensions(
                            resolution_mode.get().as_str(),
                            resolution_group.get().as_str(),
                            aspect_ratio.get().as_str(),
                            custom_width.get(),
                            custom_height.get(),
                            &dimension_reference_assets.get(),
                        );
                        view! {
                            <div class="resolution-modal" on:click=move |_| show_resolution_menu.set(false)>
                                <div class="resolution-sheet" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                                    <button
                                        class="button ghost icon-button resolution-close-button"
                                        title="关闭分辨率设置"
                                        on:click=move |_| show_resolution_menu.set(false)
                                    >
                                        <MaterialSymbolIcon name="close" filled=false />
                                    </button>
                                    <div class="row">
                                        <h3>"分辨率设置"</h3>
                                    </div>
                                    <div class="resolution-preview">
                                        <span class="tag">{format!("当前预览：{} × {}", preview_width, preview_height)}</span>
                                    </div>
                                    <div class="tag">"Responses API 会自动切换到 gpt-5.5 兼容模型，并通过 image_generation 工具产图。"</div>
                                    <div class="mode-tabs">
                                        <button class="chip-button" class:active-chip=move || resolution_mode.get() == "auto" on:click=move |_| resolution_mode.set("auto".into())>"自动"</button>
                                        <button class="chip-button" class:active-chip=move || resolution_mode.get() == "preset" on:click=move |_| resolution_mode.set("preset".into())>"按比例"</button>
                                        <button class="chip-button" class:active-chip=move || resolution_mode.get() == "custom" on:click=move |_| resolution_mode.set("custom".into())>"自定义"</button>
                                    </div>
                                    <div class="resolution-content">
                                        {move || if resolution_mode.get() == "preset" {
                                            view! {
                                                <div class="stack resolution-panel">
                                                    <div class="tag">"先选清晰度等级，再选构图比例"</div>
                                                    <div class="mode-tabs">
                                                        <button class="chip-button" class:active-chip=move || resolution_group.get() == "1k" on:click=move |_| resolution_group.set("1k".into())>"1K"</button>
                                                        <button class="chip-button" class:active-chip=move || resolution_group.get() == "2k" on:click=move |_| resolution_group.set("2k".into())>"2K"</button>
                                                        <button class="chip-button" class:active-chip=move || resolution_group.get() == "4k" on:click=move |_| resolution_group.set("4k".into())>"4K"</button>
                                                    </div>
                                                    <div class="mode-tabs">
                                                        <For
                                                            each=move || vec!["1:1", "3:2", "2:3", "16:9", "9:16"]
                                                            key=|item| item.to_string()
                                                            children=move |ratio| view! {
                                                                <button
                                                                    class="chip-button"
                                                                    class:active-chip=move || aspect_ratio.get() == ratio
                                                                    on:click=move |_| aspect_ratio.set(ratio.to_string())
                                                                >
                                                                    {ratio}
                                                                </button>
                                                            }
                                                        />
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else if resolution_mode.get() == "custom" {
                                            view! {
                                                <div class="stack resolution-panel">
                                                    <div class="tag">"自定义分辨率会自动按 16 的倍数和像素上限规整"</div>
                                                    <div class="custom-dimension-row">
                                                        <input
                                                            class="field"
                                                            type="number"
                                                            min="256"
                                                            step="16"
                                                            prop:value=move || custom_width.get().to_string()
                                                            on:input=move |ev| custom_width.set(event_target_value(&ev).parse().unwrap_or(1024))
                                                        />
                                                        <span class="custom-dimension-separator">"x"</span>
                                                        <input
                                                            class="field"
                                                            type="number"
                                                            min="256"
                                                            step="16"
                                                            prop:value=move || custom_height.get().to_string()
                                                            on:input=move |ev| custom_height.set(event_target_value(&ev).parse().unwrap_or(1024))
                                                        />
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="stack resolution-panel">
                                                    <div class="tag">"自动模式会优先沿用参考图或上一轮结果的尺寸。"</div>
                                                    <div class="tag">"如果当前没有参考图，则会回落到 1024 × 1024。"</div>
                                                </div>
                                            }.into_any()
                                        }}
                                    </div>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        ().into_any()
                    }}

                    <button class="button" on:click=generate disabled=move || generating.get()>
                        {move || if generating.get() { "生成中…" } else { "开始生成" }}
                    </button>
                    <span class="status">{move || status_text.get()}</span>
                </section>

                <section class="panel asset-panel">
                    <section class="stack">
                        <div class="row">
                            <h2>"参考图"</h2>
                            <span class="tag">{move || format!("已选参考图 {} 张", selected_reference_ids.get().len())}</span>
                        </div>
                        <div class="preview-strip">
                            <For
                                each=move || reference_assets.get()
                                key=|asset| asset.id.clone()
                                children=move |asset| {
                                    let asset_id = asset.id.clone();
                                    let src = asset_display_src(&asset);
                                    let menu_asset_id = asset_id.clone();
                                    let toggle_reference_id = asset_id.clone();
                                    let toggle_reference_label_id = asset_id.clone();
                                    let delete_asset_id = asset_id.clone();
                                    let drag_asset_id = asset_id.clone();
                                    let drag_over_asset_id = asset_id.clone();
                                    let drop_target_asset_id = asset_id.clone();
                                    let badge_asset_id = asset_id.clone();
                                    let selected_asset_id = asset_id.clone();
                                    let placeholder_asset_id = asset_id.clone();
                                    view! {
                                        <article
                                            class="thumb-card"
                                            class:is-reference-selected=move || selected_reference_ids.get().contains(&selected_asset_id)
                                            class:is-drag-placeholder=move || drag_over_reference_id.get().as_deref() == Some(placeholder_asset_id.as_str())
                                            draggable="true"
                                            on:dragstart=move |_| {
                                                dragging_reference_id.set(Some(drag_asset_id.clone()));
                                                drag_over_reference_id.set(Some(drag_asset_id.clone()));
                                            }
                                            on:dragover=move |ev: DragEvent| {
                                                ev.prevent_default();
                                                drag_over_reference_id.set(Some(drag_over_asset_id.clone()));
                                            }
                                            on:drop=move |ev: DragEvent| {
                                                ev.prevent_default();
                                                if let Some(dragged_id) = dragging_reference_id.get_untracked() {
                                                    reorder_selected_references(dragged_id, drop_target_asset_id.clone());
                                                }
                                                dragging_reference_id.set(None);
                                                drag_over_reference_id.set(None);
                                            }
                                            on:dragend=move |_| {
                                                dragging_reference_id.set(None);
                                                drag_over_reference_id.set(None);
                                            }
                                        >
                                            <button class="image-button" on:click=move |_| open_reference_menu(menu_asset_id.clone())>
                                                <div class="thumb-drag-handle" title="拖动调整参考顺序">"⋮⋮"</div>
                                                <div class="thumb-order-badge-slot">
                                                    {move || {
                                                        selected_reference_ids
                                                            .get()
                                                            .iter()
                                                            .position(|id| id == &badge_asset_id)
                                                            .map(|index| view! {
                                                                <span class="gallery-corner-badge reference-order-badge">{format!("图{}", index + 1)}</span>
                                                            }.into_any())
                                                            .unwrap_or_else(|| ().into_any())
                                                    }}
                                                </div>
                                                <img src=src.clone() alt="参考图" />
                                            </button>
                                            <div class="row thumb-actions">
                                                <button class="button ghost reference-toggle-button" on:click=move |_| {
                                                    selected_reference_ids.update(|ids| {
                                                        if let Some(index) = ids.iter().position(|id| id == &toggle_reference_id) {
                                                            ids.remove(index);
                                                        } else {
                                                            ids.push(toggle_reference_id.clone());
                                                        }
                                                    });
                                                    persist_state();
                                                }>
                                                    {move || if selected_reference_ids.get().contains(&toggle_reference_label_id) { "取消参考" } else { "设为参考" }}
                                                </button>
                                                <button class="button ghost danger mini-action icon-action" title="删除参考图" on:click=move |_| delete_asset(delete_asset_id.clone())><MaterialSymbolIcon name="delete" filled=false /></button>
                                            </div>
                                        </article>
                                    }
                                }
                            />
                        </div>
                        <AssetDropZone
                            label="拖拽、点击或粘贴图片。点击缩略图可打开参考图操作菜单。"
                            on_files=move |files| import_reference_assets(files)
                        />
                    </section>
                </section>
                </div>
            </main>

            {move || current_reference_menu_asset.get().map(|asset| {
                let delete_asset_id = asset.id.clone();
                let toggle_reference_id = asset.id.clone();
                let toggle_reference_label_id = asset.id.clone();
                view! {
                    <div class="preview-overlay" on:click=move |_| reference_menu_asset_id.set(None)>
                        <div class="reference-menu-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <div class="row reference-menu-top">
                                <div class="stack">
                                    <h3>"参考图操作"</h3>
                                    <span class="status">"可设为参考、复制、下载或删除。"</span>
                                </div>
                                <button class="button ghost icon-button" on:click=move |_| reference_menu_asset_id.set(None)><MaterialSymbolIcon name="close" filled=false /></button>
                            </div>
                            <div class="reference-menu-preview">
                                <img src=asset_src(&asset) alt="参考图预览" />
                            </div>
                            <div class="row reference-menu-actions">
                                <button class="button ghost" on:click=move |_| {
                                    selected_reference_ids.update(|ids| {
                                        if let Some(index) = ids.iter().position(|id| id == &toggle_reference_id) {
                                            ids.remove(index);
                                        } else {
                                            ids.push(toggle_reference_id.clone());
                                        }
                                    });
                                }>
                                    {move || if selected_reference_ids.get().contains(&toggle_reference_label_id) { "取消参考" } else { "设为参考" }}
                                </button>
                                <button class="button ghost danger" on:click=move |_| {
                                    reference_menu_asset_id.set(None);
                                    delete_asset(delete_asset_id.clone());
                                }>"删除图片"</button>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || current_preview.get().zip(preview_panel_state.get()).map(|((task, asset), panel)| {
                let preview_task_id = panel.task_id.clone();
                let preview_asset_id = panel.asset_id.clone();
                let favorite_task_id = panel.task_id.clone();
                let delete_task_id = panel.task_id.clone();
                let edit_task_id = panel.task_id.clone();
                let edit_asset_id = panel.asset_id.clone();
                let fullscreen_src = asset_src(&asset);
                let preview_image_src = fullscreen_src.clone();
                let copy_src = fullscreen_src.clone();
                let toolbar_download_src = fullscreen_src.clone();
                let download_src = fullscreen_src.clone();
                let prompt_text = panel.prompt.clone();
                let reference_thumb_ids = panel
                    .reference_thumbs
                    .iter()
                    .map(|thumb| thumb.id.clone())
                    .collect::<Vec<_>>();
                view! {
                    <div
                        class="preview-overlay"
                        on:click=move |_| close_preview()
                    >
                        <div class="preview-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <button class="button ghost icon-button preview-shell-close" title="关闭详情" on:click=move |_| close_preview()><MaterialSymbolIcon name="close" filled=false /></button>
                            <section class="preview-stage" class:is-fullscreen=move || preview_fullscreen.get()>
                                <div class="preview-stage-meta">
                                    <span class="tag">{aspect_ratio_label(panel.width, panel.height)}</span>
                                    <span class="tag">{format!("{}x{}", panel.width, panel.height)}</span>
                                </div>
                                {move || {
                                    if preview_fullscreen.get() {
                                        let toolbar_download_src = toolbar_download_src.clone();
                                        view! {
                                            <div class="preview-fullscreen-toolbar">
                                                <button
                                                    class="button ghost icon-button preview-toolbar-button"
                                                    title="下载原图"
                                                    on:click=move |_| {
                                                        let src = toolbar_download_src.clone();
                                                        let _ = download_image_from_src(&src, "mew-image-output");
                                                    }
                                                >
                                                    <MaterialSymbolIcon name="download" filled=false />
                                                </button>
                                                <button
                                                    class="button ghost icon-button preview-toolbar-button"
                                                    title="退出大图"
                                                    on:click=move |_| {
                                                        preview_fullscreen.set(false);
                                                        preview_zoom.set(1.0);
                                                        preview_offset_x.set(0.0);
                                                        preview_offset_y.set(0.0);
                                                        preview_dragging.set(false);
                                                    }
                                                >
                                                    <MaterialSymbolIcon name="close" filled=false />
                                                </button>
                                            </div>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }
                                }}
                                <button
                                    class="image-button preview-image-button"
                                    class:is-pan-enabled=move || preview_fullscreen.get()
                                    on:click=move |_| {
                                        if !preview_fullscreen.get_untracked() {
                                            preview_fullscreen.set(true);
                                            preview_zoom.set(1.0);
                                            preview_offset_x.set(0.0);
                                            preview_offset_y.set(0.0);
                                        }
                                    }
                                    on:mousedown=move |ev: MouseEvent| {
                                        if !preview_fullscreen.get_untracked() {
                                            return;
                                        }
                                        ev.prevent_default();
                                        preview_dragging.set(true);
                                        preview_drag_origin_x.set(preview_offset_x.get_untracked());
                                        preview_drag_origin_y.set(preview_offset_y.get_untracked());
                                        preview_drag_start_x.set(ev.client_x() as f64);
                                        preview_drag_start_y.set(ev.client_y() as f64);
                                    }
                                    on:mousemove=move |ev: MouseEvent| {
                                        if !preview_dragging.get_untracked() {
                                            return;
                                        }
                                        let delta_x = ev.client_x() as f64 - preview_drag_start_x.get_untracked();
                                        let delta_y = ev.client_y() as f64 - preview_drag_start_y.get_untracked();
                                        preview_offset_x.set(preview_drag_origin_x.get_untracked() + delta_x);
                                        preview_offset_y.set(preview_drag_origin_y.get_untracked() + delta_y);
                                    }
                                    on:mouseup=move |_| {
                                        preview_dragging.set(false);
                                    }
                                    on:mouseleave=move |_| {
                                        preview_dragging.set(false);
                                    }
                                    on:wheel=move |ev: WheelEvent| {
                                        if !preview_fullscreen.get_untracked() {
                                            return;
                                        }
                                        ev.prevent_default();
                                        let current = preview_zoom.get_untracked();
                                        let delta = if ev.delta_y() < 0.0 { 0.12 } else { -0.12 };
                                        let next = (current + delta).clamp(0.4, 6.0);
                                        preview_zoom.set(next);
                                        if (next - 1.0).abs() < 0.02 {
                                            preview_zoom.set(1.0);
                                            preview_offset_x.set(0.0);
                                            preview_offset_y.set(0.0);
                                        }
                                    }
                                    on:contextmenu=move |ev: MouseEvent| {
                                        ev.prevent_default();
                                        context_menu_state.set(Some(ContextMenuState {
                                            task_id: preview_task_id.clone(),
                                            asset_id: preview_asset_id.clone(),
                                            x: ev.client_x() as f64,
                                            y: ev.client_y() as f64,
                                        }));
                                    }
                                >
                                    <img
                                        class="preview-image"
                                        class:is-zoomed=move || preview_fullscreen.get()
                                        style=move || {
                                            format!(
                                                "transform: translate({:.1}px, {:.1}px) scale({:.3});",
                                                preview_offset_x.get(),
                                                preview_offset_y.get(),
                                                preview_zoom.get()
                                            )
                                        }
                                        src=preview_image_src
                                        alt=panel.prompt.clone()
                                    />
                                </button>
                            </section>
                            <aside class="preview-sidebar">
                                <div class="row preview-sidebar-top">
                                    <div class="stack">
                                        <div class="row preview-prompt-head">
                                            <span class="status">"输入内容"</span>
                                            <button
                                                class="button ghost icon-button preview-copy-button"
                                                on:mouseenter=move |ev: web_sys::MouseEvent| {
                                                    let target = ev.current_target().and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok());
                                                    if let Some(target) = target {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("复制提示词", rect.left(), rect.top() + 18.0, true);
                                                    }
                                                }
                                                on:mouseleave=move |_| hide_tip()
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    let text = prompt_text.clone();
                                                    let target = ev.current_target().and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok());
                                                    if let Some(target) = target {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("提示词已复制~", rect.left(), rect.top() + 18.0, false);
                                                    }
                                                    spawn_local(async move {
                                                        let Some(window) = web_sys::window() else {
                                                            return;
                                                        };
                                                        let clipboard = window.navigator().clipboard();
                                                        let _ = JsFuture::from(clipboard.write_text(&text)).await;
                                                    });
                                                }
                                            >
                                                <MaterialSymbolIcon name="content_copy" filled=false />
                                            </button>
                                        </div>
                                        <p class="preview-prompt">{panel.prompt.clone()}</p>
                                    </div>
                                </div>
                                <div class="stack">
                                    <div class="row preview-prompt-head">
                                        <span class="status">"参考图"</span>
                                        <button
                                            class="button ghost icon-button preview-copy-button"
                                            disabled=move || !reference_tip_enabled()
                                            on:mouseenter=move |ev: web_sys::MouseEvent| {
                                                if reference_tip_enabled() {
                                                    if let Some(target) = ev
                                                        .current_target()
                                                        .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                                                    {
                                                        let rect = target.get_bounding_client_rect();
                                                        show_tip("引用参考图", rect.left(), rect.top() + 18.0, true);
                                                    }
                                                }
                                            }
                                            on:mouseleave=move |_| hide_tip()
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                if !reference_tip_enabled() {
                                                    return;
                                                }
                                                selected_reference_ids.set(reference_thumb_ids.clone());
                                                if let Some(target) = ev
                                                    .current_target()
                                                    .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                                                {
                                                    let rect = target.get_bounding_client_rect();
                                                    show_tip("参考图已引用~", rect.left(), rect.top() + 18.0, false);
                                                }
                                            }
                                        >
                                            <MaterialSymbolIcon name="link" filled=false />
                                        </button>
                                    </div>
                                    <div class="preview-ref-strip">
                                        <For
                                            each=move || panel.reference_thumbs.clone()
                                            key=|item| item.id.clone()
                                            children=move |item| {
                                                view! {
                                                    <div class="preview-ref-card">
                                                        <img src=item.src alt="参考图缩略图" />
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                </div>
                                <div class="preview-details-grid">
                                    <div class="detail-card">
                                        <span class="status">"来源"</span>
                                        <strong>{format!("{} · {}", panel.source_label, panel.requested_model)}</strong>
                                    </div>
                                    <div class="detail-card">
                                        <span class="status">"尺寸"</span>
                                        <strong>{format!("{}x{}", panel.width, panel.height)}</strong>
                                    </div>
                                    <div class="detail-card">
                                        <span class="status">"质量"</span>
                                        <strong>{format!("请求 {} / 实际 {}", panel.requested_quality_label, panel.actual_quality_label)}</strong>
                                    </div>
                                    <div class="detail-card">
                                        <span class="status">"格式"</span>
                                        <strong>{panel.format_label.clone()}</strong>
                                    </div>
                                    <div class="detail-card">
                                        <span class="status">"审核"</span>
                                        <strong>{panel.moderation_label.clone()}</strong>
                                    </div>
                                    <div class="detail-card">
                                        <span class="status">"数量"</span>
                                        <strong>{panel.image_count.to_string()}</strong>
                                    </div>
                                </div>
                                <div class="preview-time-meta">
                                    <span>{format!("创建于 {}", format_shanghai_datetime(&panel.created_at))}</span>
                                    <span>"·"</span>
                                    <span>{format!("耗时 {}", panel.duration_label.clone())}</span>
                                </div>
                                <div class="row preview-actions">
                                    <button class="button ghost" on:click=move |_| {
                                        continue_from_task(task.id.clone());
                                        close_preview();
                                    }>"复用配置"</button>
                                    <button class="button secondary" on:click=move |_| edit_output_asset(edit_task_id.clone(), edit_asset_id.clone())>"编辑输出"</button>
                                    <button class="button ghost danger" on:click=move |_| {
                                        delete_task(delete_task_id.clone());
                                        close_preview();
                                    }>"删除记录"</button>
                                    <button class="button ghost" on:click=move |_| {
                                        tasks.update(|items| {
                                            if let Some(found) = items.iter_mut().find(|item| item.id == favorite_task_id) {
                                                found.favorite = !found.favorite;
                                            }
                                        });
                                        preview_panel_state.update(|state| {
                                            if let Some(state) = state.as_mut() {
                                                state.favorite = !state.favorite;
                                            }
                                        });
                                        persist_state();
                                    }>
                                        {move || if preview_panel_state.get().map(|state| state.favorite).unwrap_or(false) { "取消收藏" } else { "收藏" }}
                                    </button>
                                </div>
                                <div class="row preview-actions">
                                    <button class="button ghost" on:click=move |_| {
                                        let src = copy_src.clone();
                                        spawn_local(async move {
                                            let _ = copy_image_from_src(&src).await;
                                        });
                                    }>"复制"</button>
                                    <button class="button ghost" on:click=move |_| {
                                        let _ = download_image_from_src(&download_src, "mew-image-output");
                                    }>"下载"</button>
                                </div>
                            </aside>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || context_menu_state.get().map(|menu| {
                let x = menu.x;
                let y = menu.y;
                let task_id = menu.task_id.clone();
                let asset_id = menu.asset_id.clone();
                let copy_src = assets.with(|items| {
                    items.iter()
                        .find(|asset| asset.id == asset_id)
                        .map(asset_src)
                        .unwrap_or_default()
                });
                let download_src = copy_src.clone();
                let edit_task_id = task_id.clone();
                let edit_asset_id = asset_id.clone();
                view! {
                    <div class="context-menu-layer" on:click=move |_| context_menu_state.set(None)>
                        <div
                            class="context-menu"
                            style=format!("left: min({x}px, calc(100vw - 180px)); top: min({y}px, calc(100vh - 180px));")
                            on:click=move |ev: MouseEvent| ev.stop_propagation()
                        >
                            <button class="button ghost context-item" on:click=move |_| {
                                let src = copy_src.clone();
                                context_menu_state.set(None);
                                spawn_local(async move {
                                    let _ = copy_image_from_src(&src).await;
                                });
                            }>"复制"</button>
                            <button class="button ghost context-item" on:click=move |_| {
                                let _ = download_image_from_src(&download_src, "mew-image-output");
                                context_menu_state.set(None);
                            }>"下载"</button>
                            <button class="button ghost context-item" on:click=move |_| {
                                edit_output_asset(edit_task_id.clone(), edit_asset_id.clone());
                                context_menu_state.set(None);
                            }>"编辑"</button>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || failure_log_state.get().map(|log| {
                let copy_text = log.details.clone();
                let delete_task_id = log.task_id.clone();
                view! {
                    <div class="preview-overlay" on:click=move |_| failure_log_state.set(None)>
                        <div class="preview-shell failure-log-shell" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <button class="button ghost icon-button preview-shell-close" title="关闭日志" on:click=move |_| failure_log_state.set(None)>
                                <MaterialSymbolIcon name="close" filled=false />
                            </button>
                            <div class="stack failure-log-top">
                                <h3>{log.title.clone()}</h3>
                                <span class="status">{log.summary.clone()}</span>
                            </div>
                            <pre class="failure-log-text">{log.details.clone()}</pre>
                            <div class="row preview-actions">
                                <button class="button ghost" on:click=move |_| {
                                    let text = copy_text.clone();
                                    spawn_local(async move {
                                        let Some(window) = web_sys::window() else {
                                            return;
                                        };
                                        let _ = JsFuture::from(window.navigator().clipboard().write_text(&text)).await;
                                    });
                                }>
                                    <MaterialSymbolIcon name="content_copy" filled=false />
                                    "复制"
                                </button>
                                <button class="button ghost danger" on:click=move |_| {
                                    delete_task(delete_task_id.clone());
                                    failure_log_state.set(None);
                                }>
                                    <MaterialSymbolIcon name="delete" filled=false />
                                    "删除任务"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}

            {move || floating_tip_state.get().map(|tip| {
                view! {
                    <div
                        class="floating-tip"
                        style=format!("left: {}px; top: {}px;", tip.x, tip.y)
                    >
                        {tip.text.clone()}
                    </div>
                }.into_any()
            }).unwrap_or_else(|| ().into_any())}
        </div>
    }
}

#[component]
fn ConfigEditor(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    current_config_id: RwSignal<String>,
    templates: RwSignal<Vec<ProviderTemplate>>,
    persist: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let current_config = Memo::new(move |_| {
        configs
            .get()
            .into_iter()
            .find(|config| config.id == current_config_id.get())
    });
    let name_draft = RwSignal::new(String::new());
    let base_url_draft = RwSignal::new(String::new());
    let model_draft = RwSignal::new(String::new());
    let api_key_draft = RwSignal::new(String::new());
    let has_pending_changes = RwSignal::new(false);

    Effect::new(move |_| {
        if let Some(config) = current_config.get() {
            name_draft.set(config.name);
            base_url_draft.set(config.base_url);
            model_draft.set(config.model);
            api_key_draft.set(config.api_key_plaintext.unwrap_or_default());
        }
    });

    let update_current = move |mutator: fn(&mut EncryptedApiConfig, String), value: String| {
        configs.update(|items| {
            if let Some(config) = items
                .iter_mut()
                .find(|config| config.id == current_config_id.get_untracked())
            {
                mutator(config, value.clone());
                config.updated_at = now_rfc3339();
            }
        });
    };

    let flush_changes = move || {
        if has_pending_changes.get_untracked() {
            persist();
            has_pending_changes.set(false);
        }
    };

    let commit_name = move || {
        update_current(
            |config, value| config.name = value,
            name_draft.get_untracked(),
        );
        has_pending_changes.set(true);
    };
    let commit_base_url = move || {
        update_current(
            |config, value| config.base_url = value,
            base_url_draft.get_untracked(),
        );
        has_pending_changes.set(true);
    };
    let commit_model = move || {
        update_current(
            |config, value| config.model = value,
            model_draft.get_untracked(),
        );
        has_pending_changes.set(true);
    };
    let commit_api_key = move || {
        update_current(
            |config, value| {
                config.api_key_plaintext = Some(value.clone());
                config.api_key_hint = Some(mask_key(&value));
            },
            api_key_draft.get_untracked(),
        );
        has_pending_changes.set(true);
    };

    view! {
        <div class="stack" on:focusout=move |_| flush_changes()>
            <input
                class="text-input"
                placeholder="配置名称"
                prop:value=move || name_draft.get()
                on:input=move |ev| name_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_name()
            />
            <select
                class="select-input"
                prop:value=move || current_config.get().map(|config| config.provider_template_id).unwrap_or_default()
                on:change=move |ev| {
                    let value = event_target_value(&ev);
                    let template = templates.get_untracked().into_iter().find(|template| template.id == value);
                    configs.update(|items| {
                        if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                            config.provider_template_id = value.clone();
                            if let Some(template) = template.clone() {
                                config.provider_kind = template.kind;
                                config.base_url = template.base_url;
                                config.known_requires_proxy = template.known_requires_proxy;
                                match template.kind {
                                    ProviderKind::OpenAiImage => {
                                        config.endpoint_mode = ProviderEndpointMode::ImagesApi;
                                        config.model = "gpt-image-1".into();
                                    }
                                    ProviderKind::NanoBanana | ProviderKind::OpenAiCompatible => {
                                        config.endpoint_mode = ProviderEndpointMode::CustomJson;
                                        config.model = "gemini-2.5-flash-image".into();
                                    }
                                    ProviderKind::CustomHttp => {
                                        config.endpoint_mode = ProviderEndpointMode::CustomJson;
                                    }
                                }
                                normalize_api_config(config);
                            }
                            config.updated_at = now_rfc3339();
                        }
                    });
                    has_pending_changes.set(true);
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
                prop:value=move || model_draft.get()
                on:input=move |ev| model_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_model()
            />
            <input
                class="text-input"
                type="password"
                placeholder="API Key"
                prop:value=move || api_key_draft.get()
                on:input=move |ev| api_key_draft.set(event_target_value(&ev))
                on:blur=move |_| commit_api_key()
            />
            <div class="grid compact-grid">
                <select
                    class="select-input"
                    prop:value=move || current_config.get().map(|config| format!("{:?}", config.access_mode)).unwrap_or_default()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        configs.update(|items| {
                            if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                config.access_mode = match value.as_str() {
                                    "Proxy" => ProviderAccessMode::Proxy,
                                    "Direct" => ProviderAccessMode::Direct,
                                    _ => ProviderAccessMode::Smart,
                                };
                                config.updated_at = now_rfc3339();
                            }
                        });
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
                                prop:value=move || current_config.get().map(|config| format!("{:?}", config.endpoint_mode)).unwrap_or_default()
                                on:change=move |ev| {
                                    let value = event_target_value(&ev);
                                    configs.update(|items| {
                                        if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                            config.endpoint_mode = match value.as_str() {
                                                "ResponsesApi" => ProviderEndpointMode::ResponsesApi,
                                                _ => ProviderEndpointMode::ImagesApi,
                                            };
                                            config.updated_at = now_rfc3339();
                                        }
                                    });
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
            </div>
        </div>
    }
}

#[component]
fn AssetDropZone(
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

#[derive(Clone, PartialEq)]
struct GalleryItem {
    key: String,
    task_id: String,
    asset_id: Option<String>,
    prompt: String,
    src: Option<String>,
    model: String,
    size_label: String,
    ratio_label: String,
    favorite: bool,
}

fn gallery_items(tasks: &[LocalTaskRecord], assets: &[ImageAssetRef]) -> Vec<GalleryItem> {
    let mut assets_by_task: HashMap<&str, Vec<&ImageAssetRef>> = HashMap::new();
    for asset in assets {
        if let Some(task_id) = asset.source_task_id.as_deref() {
            assets_by_task.entry(task_id).or_default().push(asset);
        }
    }
    let mut items = Vec::new();
    for task in tasks {
        if let Some(generated_assets) = assets_by_task.get(task.id.as_str()) {
            for asset in generated_assets {
                items.push(GalleryItem {
                    key: format!("{}-{}", task.id, asset.id),
                    task_id: task.id.clone(),
                    asset_id: Some(asset.id.clone()),
                    prompt: task.prompt.clone(),
                    src: Some(asset_display_src(asset)),
                    model: task.requested_model.clone(),
                    size_label: format!(
                        "{}x{}",
                        asset.width.unwrap_or(0),
                        asset.height.unwrap_or(0)
                    ),
                    ratio_label: aspect_ratio_label(
                        asset.width.unwrap_or(0),
                        asset.height.unwrap_or(0),
                    ),
                    favorite: task.favorite,
                });
            }
        } else if let Some(error) = &task.error_message {
            items.push(GalleryItem {
                key: format!("{}-error", task.id),
                task_id: task.id.clone(),
                asset_id: None,
                prompt: format!("失败：{error}"),
                src: None,
                model: task.requested_model.clone(),
                size_label: "-".into(),
                ratio_label: "失败".into(),
                favorite: task.favorite,
            });
        }
    }
    items
}

fn reconcile_task_integrity(
    tasks: &mut [LocalTaskRecord],
    assets: &[ImageAssetRef],
    repair_running_tasks: bool,
) -> bool {
    let mut asset_count_by_task: HashMap<&str, usize> = HashMap::new();
    for asset in assets {
        if let Some(task_id) = asset.source_task_id.as_deref() {
            *asset_count_by_task.entry(task_id).or_default() += 1;
        }
    }

    let mut changed = false;
    for task in tasks {
        let produced_count = asset_count_by_task
            .get(task.id.as_str())
            .copied()
            .unwrap_or(0);
        match task.status {
            TaskStatus::Succeeded if produced_count == 0 => {
                task.status = TaskStatus::Failed;
                if task
                    .error_message
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    let upstream_count = task
                        .result
                        .as_ref()
                        .map(|result| {
                            result
                                .images
                                .iter()
                                .filter(|image| {
                                    image
                                        .data_url
                                        .as_deref()
                                        .map(|value| !value.trim().is_empty())
                                        .unwrap_or(false)
                                        || image
                                            .url
                                            .as_deref()
                                            .map(|value| !value.trim().is_empty())
                                            .unwrap_or(false)
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    task.error_message = Some(if upstream_count == 0 {
                        "上游没有返回任何可用图片结果，任务已改判为失败。".into()
                    } else {
                        "上游结果未能落成本地可用图片，可能是网络、尺寸或响应异常导致。".into()
                    });
                }
                changed = true;
            }
            TaskStatus::Running if repair_running_tasks && produced_count == 0 => {
                task.status = TaskStatus::Failed;
                if task
                    .error_message
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    task.error_message = Some("上次生成未正常结束，已自动标记为失败。".into());
                }
                changed = true;
            }
            _ => {}
        }
    }

    changed
}

fn selected_thread_reference_assets(
    assets: &[ImageAssetRef],
    thread_id: &str,
    selected_reference_ids: &[String],
) -> Vec<ImageAssetRef> {
    let mut selected_assets = Vec::new();
    for selected_id in selected_reference_ids {
        if let Some(asset) = assets.iter().find(|asset| {
            asset.id == *selected_id
                && asset.source_task_id.is_none()
                && !asset.metadata.contains_key("mask_base_asset_id")
                && asset
                    .metadata
                    .get("thread_id")
                    .map(|value| value == thread_id)
                    .unwrap_or(false)
        }) {
            selected_assets.push(asset.clone());
        }
    }
    selected_assets
}

fn snapshot_local_state(
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    preferences: RwSignal<AppPreferences>,
    checkpoint: RwSignal<SyncCheckpoint>,
) -> LocalAppState {
    LocalAppState {
        configs: configs.get_untracked(),
        tasks: tasks.get_untracked(),
        threads: threads.get_untracked(),
        assets: assets.get_untracked(),
        preferences: preferences.get_untracked(),
        checkpoint: checkpoint.get_untracked(),
    }
}

fn apply_local_state(
    mut state: LocalAppState,
    configs: RwSignal<Vec<EncryptedApiConfig>>,
    tasks: RwSignal<Vec<LocalTaskRecord>>,
    threads: RwSignal<Vec<ConversationThread>>,
    assets: RwSignal<Vec<ImageAssetRef>>,
    preferences: RwSignal<AppPreferences>,
    checkpoint: RwSignal<SyncCheckpoint>,
) {
    for config in &mut state.configs {
        normalize_api_config(config);
    }
    configs.set(state.configs);
    tasks.set(state.tasks);
    threads.set(state.threads);
    assets.set(state.assets);
    preferences.set(state.preferences);
    checkpoint.set(state.checkpoint);
}

fn resolve_dimensions(
    mode: &str,
    group: &str,
    ratio: &str,
    custom_width: u32,
    custom_height: u32,
    references: &[ImageAssetRef],
) -> (u32, u32) {
    if mode == "custom" {
        let result = clamp_size(custom_width, custom_height);
        return (result.width, result.height);
    }
    if mode == "auto" {
        if let Some(reference) = references
            .iter()
            .find(|asset| asset.width.is_some() && asset.height.is_some())
        {
            let result = clamp_size(
                reference.width.unwrap_or(1024),
                reference.height.unwrap_or(1024),
            );
            return (result.width, result.height);
        }
        return (1024, 1024);
    }
    preset_dimensions(group, ratio)
}

fn preset_dimensions(group: &str, ratio: &str) -> (u32, u32) {
    let size = match (group, ratio) {
        ("1k", "3:2") => (1216, 832),
        ("1k", "2:3") => (832, 1216),
        ("1k", "16:9") => (1344, 768),
        ("1k", "9:16") => (768, 1344),
        ("2k", "3:2") => (1792, 1216),
        ("2k", "2:3") => (1216, 1792),
        ("2k", "16:9") => (1920, 1088),
        ("2k", "9:16") => (1088, 1920),
        ("4k", "3:2") => (3840, 2560),
        ("4k", "2:3") => (2560, 3840),
        ("4k", "16:9") => (3840, 2160),
        ("4k", "9:16") => (2160, 3840),
        ("2k", _) => (1536, 1536),
        ("4k", _) => (4096, 4096),
        _ => (1024, 1024),
    };
    let result = clamp_size(size.0, size.1);
    (result.width, result.height)
}

fn apply_theme(theme: ThemePreference) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    if let Some(body) = document.body() {
        let _ = body.set_attribute(
            "data-theme",
            if theme == ThemePreference::Night {
                "night"
            } else {
                "day"
            },
        );
    }
}

fn default_thread() -> ConversationThread {
    ConversationThread {
        id: new_id(),
        title: "新的会话".into(),
        draft_prompt: String::new(),
        task_ids: Vec::new(),
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    }
}

fn thread_display_name(thread: &ConversationThread) -> String {
    if thread.title.trim().is_empty() {
        "新的会话".into()
    } else {
        thread.title.clone()
    }
}

fn summarize_prompt(prompt: &str) -> String {
    let summary: String = prompt.chars().take(12).collect();
    if prompt.chars().count() > 12 {
        format!("{summary}…")
    } else {
        summary
    }
}

fn is_openai_image_model(config: &EncryptedApiConfig) -> bool {
    config.provider_kind == ProviderKind::OpenAiImage
        && config.model.to_ascii_lowercase().contains("image")
}

fn aspect_ratio_label(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "未知比例".into();
    }
    let width = width as f64;
    let height = height as f64;
    let target = width / height;
    const CANDIDATES: &[(u32, u32)] = &[(1, 1), (4, 3), (3, 4), (3, 2), (2, 3), (16, 9), (9, 16)];
    let mut best = (1, 1);
    let mut best_error = f64::MAX;
    for &(candidate_width, candidate_height) in CANDIDATES {
        let ratio = candidate_width as f64 / candidate_height as f64;
        let error = (target - ratio).abs();
        if error < best_error {
            best = (candidate_width, candidate_height);
            best_error = error;
        }
    }
    if best_error <= 0.08 {
        format!("{}:{}", best.0, best.1)
    } else {
        let divisor = gcd(width.round() as u32, height.round() as u32).max(1);
        format!(
            "{}:{}",
            width.round() as u32 / divisor,
            height.round() as u32 / divisor
        )
    }
}

fn gcd(left: u32, right: u32) -> u32 {
    let mut a = left;
    let mut b = right;
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn confirm_action(message: &str) -> bool {
    web_sys::window()
        .and_then(|window| window.confirm_with_message(message).ok())
        .unwrap_or(false)
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        format!("{:.2} 秒", duration_ms as f64 / 1_000.0)
    } else {
        format!("{duration_ms} 毫秒")
    }
}

fn format_failure_raw_response(value: &serde_json::Value) -> String {
    let mut copy = value.clone();
    if let Some(output) = copy.get_mut("output").and_then(|value| value.as_array_mut()) {
        for item in output {
            if let Some(result) = item.get_mut("result") {
                if let Some(text) = result.as_str() {
                    if text.len() > 96 {
                        *result = serde_json::Value::String(format!(
                            "<base64_data len={}>",
                            text.len()
                        ));
                    }
                } else if let Some(object) = result.as_object_mut() {
                    redact_large_base64_values(object);
                }
            }
        }
    }
    if let Some(tools) = copy.get_mut("tools").and_then(|value| value.as_array_mut()) {
        for tool in tools {
            if let Some(object) = tool.as_object_mut() {
                redact_large_base64_values(object);
            }
        }
    }
    serde_json::to_string_pretty(&copy).unwrap_or_else(|_| copy.to_string())
}

fn redact_large_base64_values(map: &mut serde_json::Map<String, serde_json::Value>) {
    let keys = ["result", "data", "b64_json", "base64", "image", "image_url"];
    for key in keys {
        if let Some(value) = map.get_mut(key) {
            match value {
                serde_json::Value::String(text) if text.len() > 96 => {
                    *value = serde_json::Value::String(format!("<base64_data len={}>", text.len()));
                }
                serde_json::Value::Object(object) => {
                    redact_large_base64_values(object);
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        if let Some(object) = item.as_object_mut() {
                            redact_large_base64_values(object);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn format_shanghai_datetime(value: &str) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    let timestamp = date.get_time();
    if !timestamp.is_finite() {
        return value.to_string();
    }
    let shanghai = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        timestamp + 8.0 * 60.0 * 60.0 * 1000.0,
    ));
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        shanghai.get_utc_full_year() as i32,
        shanghai.get_utc_month() + 1,
        shanghai.get_utc_date(),
        shanghai.get_utc_hours(),
        shanghai.get_utc_minutes(),
        shanghai.get_utc_seconds()
    )
}

pub(crate) fn asset_src(asset: &ImageAssetRef) -> String {
    asset
        .data_url
        .clone()
        .or_else(|| asset.remote_url.clone())
        .unwrap_or_default()
}

fn asset_display_src(asset: &ImageAssetRef) -> String {
    asset
        .metadata
        .get(THUMBNAIL_DATA_URL_KEY)
        .cloned()
        .or_else(|| asset.data_url.clone())
        .or_else(|| asset.remote_url.clone())
        .unwrap_or_default()
}

pub(crate) fn bytes_to_data_url(bytes: &[u8], mime_type: &str) -> String {
    format!("data:{mime_type};base64,{}", BASE64.encode(bytes))
}

fn decode_browser_data_url(data_url: &str) -> Result<(String, Vec<u8>), String> {
    let Some((prefix, payload)) = data_url.split_once(',') else {
        return Err("浏览器数据 URL 无效".into());
    };
    let mime_type = prefix
        .trim_start_matches("data:")
        .split(';')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("image/png")
        .to_string();
    let bytes = BASE64
        .decode(payload)
        .map_err(|error| format!("浏览器数据 URL 解码失败：{error}"))?;
    Ok((mime_type, bytes))
}

async fn load_html_image(src: &str) -> Result<HtmlImageElement, String> {
    let image = HtmlImageElement::new().map_err(|error| format!("{error:?}"))?;
    let image_for_promise = image.clone();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let onload = Closure::<dyn FnMut()>::once(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
        let onerror = Closure::<dyn FnMut()>::once(move || {
            let _ = reject.call1(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from_str("图片载入失败"),
            );
        });
        image_for_promise.set_onload(Some(onload.as_ref().unchecked_ref()));
        image_for_promise.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onload.forget();
        onerror.forget();
    });
    image.set_src(src);
    JsFuture::from(promise)
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(image)
}

pub(crate) async fn reencode_asset_bytes(
    asset: &ImageAssetRef,
    target_mime: &str,
    quality: Option<f64>,
) -> Result<(Vec<u8>, String, u32, u32), String> {
    let source = asset_src(asset);
    if source.is_empty() {
        return Err("当前连续修改所需的图片缺少可读取数据，请先重新生成一次。".into());
    }
    let (source_bytes, source_mime) = fetch_image_bytes(&source).await?;
    let source_data_url = bytes_to_data_url(&source_bytes, &source_mime);
    let image = load_html_image(&source_data_url).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);

    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "画布元素类型错误".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("{error:?}"))?
        .ok_or_else(|| "无法获取 2D 画布上下文".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element(&image, 0.0, 0.0)
        .map_err(|error| format!("绘制图片到画布失败：{error:?}"))?;
    let data_url = if let Some(quality) = quality {
        canvas
            .to_data_url_with_type_and_encoder_options(
                target_mime,
                &wasm_bindgen::JsValue::from_f64(quality),
            )
            .map_err(|error| format!("图片转码失败：{error:?}"))?
    } else {
        canvas
            .to_data_url_with_type(target_mime)
            .map_err(|error| format!("图片转码失败：{error:?}"))?
    };
    let (mime_type, bytes) = decode_browser_data_url(&data_url)?;
    Ok((bytes, mime_type, width, height))
}

async fn thumbnail_data_url_from_asset(
    asset: &ImageAssetRef,
    max_edge: u32,
) -> Result<String, String> {
    let source = asset_src(asset);
    if source.is_empty() {
        return Err("缩略图源图片不可用".into());
    }
    let image = load_html_image(&source).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);
    let longest = width.max(height).max(1);
    if longest <= max_edge {
        return Ok(source);
    }
    let scale = max_edge as f64 / longest as f64;
    let target_width = ((width as f64 * scale).round() as u32).max(1);
    let target_height = ((height as f64 * scale).round() as u32).max(1);

    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建缩略图画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "缩略图画布元素类型错误".to_string())?;
    canvas.set_width(target_width);
    canvas.set_height(target_height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("{error:?}"))?
        .ok_or_else(|| "无法获取缩略图 2D 画布上下文".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element_and_dw_and_dh(
            &image,
            0.0,
            0.0,
            target_width as f64,
            target_height as f64,
        )
        .map_err(|error| format!("绘制缩略图失败：{error:?}"))?;
    canvas
        .to_data_url_with_type_and_encoder_options(
            "image/webp",
            &wasm_bindgen::JsValue::from_f64(0.82),
        )
        .or_else(|_| canvas.to_data_url_with_type("image/jpeg"))
        .map_err(|error| format!("生成缩略图失败：{error:?}"))
}

async fn fetch_image_bytes_via_proxy(src: &str) -> Result<(Vec<u8>, String), String> {
    let mut headers = BTreeMap::new();
    headers.insert("Accept".into(), "image/*".into());
    let response = Request::post(&api_url("/api/proxy/forward"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(&ProxyInvokeRequest {
            url: src.to_string(),
            method: "GET".into(),
            headers,
            body_base64: None,
        })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| format!("代理下载图片失败：{error}"))?;
    if !response.ok() {
        return Err(response
            .text()
            .await
            .unwrap_or_else(|_| "代理下载图片失败".into()));
    }
    let payload = response
        .json::<ProxyInvokeResponse>()
        .await
        .map_err(|error| format!("代理下载图片响应解析失败：{error}"))?;
    if payload.status >= 400 {
        return Err(format!("代理下载图片失败：上游返回 {}", payload.status));
    }
    let bytes = BASE64
        .decode(payload.body_base64)
        .map_err(|error| format!("代理图片 Base64 解码失败：{error}"))?;
    Ok((
        bytes,
        payload.content_type.unwrap_or_else(|| "image/png".into()),
    ))
}

pub(crate) async fn fetch_image_bytes(src: &str) -> Result<(Vec<u8>, String), String> {
    if let Some((prefix, payload)) = src.split_once(',') {
        let mime_type = prefix
            .trim_start_matches("data:")
            .split(';')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("image/png")
            .to_string();
        let bytes = BASE64
            .decode(payload)
            .map_err(|error| format!("图片解码失败：{error}"))?;
        return Ok((bytes, mime_type));
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return fetch_image_bytes_via_proxy(src).await;
    }
    let request_url = if src.starts_with("/api/") {
        api_url(src)
    } else {
        src.to_string()
    };
    let response = Request::get(&request_url)
        .send()
        .await
        .map_err(|error| format!("下载图片失败：{error}"))?;
    let mime_type = response
        .headers()
        .get("content-type")
        .unwrap_or_else(|| "image/png".into());
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("读取图片失败：{error}"))?;
    Ok((bytes, mime_type))
}

pub(crate) fn blob_from_bytes(bytes: &[u8], mime_type: &str) -> Result<Blob, String> {
    let array = Uint8Array::from(bytes);
    let parts = Array::new();
    parts.push(&array.buffer());
    let bag = BlobPropertyBag::new();
    bag.set_type(mime_type);
    Blob::new_with_u8_array_sequence_and_options(&parts, &bag)
        .map_err(|error| format!("构建 Blob 失败：{error:?}"))
}

async fn copy_image_from_src(src: &str) -> Result<(), String> {
    let (bytes, mime_type) = fetch_image_bytes(src).await?;
    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let clipboard = window.navigator().clipboard();
    let blob = blob_from_bytes(&bytes, &mime_type)?;
    let item_data = Object::new();
    Reflect::set(
        &item_data,
        &wasm_bindgen::JsValue::from_str(&mime_type),
        &blob,
    )
    .map_err(|error| format!("准备剪贴板数据失败：{error:?}"))?;
    let clipboard_item = Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("ClipboardItem"),
    )
    .map_err(|_| "当前浏览器不支持 ClipboardItem".to_string())?;
    let constructor: Function = clipboard_item
        .dyn_into()
        .map_err(|_| "ClipboardItem 构造器不可用".to_string())?;
    let args = Array::new();
    args.push(&item_data);
    let item = Reflect::construct(&constructor, &args)
        .map_err(|error| format!("创建剪贴板对象失败：{error:?}"))?;
    let items = Array::new();
    items.push(&item);
    JsFuture::from(clipboard.write(&items))
        .await
        .map_err(|error| format!("写入剪贴板失败：{error:?}"))?;
    Ok(())
}

fn download_image_from_src(src: &str, base_name: &str) -> Result<(), String> {
    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let element = document
        .create_element("a")
        .map_err(|error| format!("创建下载元素失败：{error:?}"))?;
    let anchor: HtmlAnchorElement = element
        .dyn_into()
        .map_err(|_| "下载元素类型错误".to_string())?;
    anchor.set_href(src);
    anchor.set_download(&format!("{base_name}.png"));
    let _ = anchor.set_attribute("style", "display:none");
    let Some(body) = document.body() else {
        return Err("浏览器页面主体不可用".into());
    };
    body.append_child(&anchor)
        .map_err(|error| format!("挂载下载元素失败：{error:?}"))?;
    anchor.click();
    let _ = body.remove_child(&anchor);
    Ok(())
}

async fn import_file_list(files: FileList) -> Result<Vec<ImageAssetRef>, String> {
    let mut imported = Vec::new();
    for index in 0..files.length() {
        let Some(file) = files.get(index) else {
            continue;
        };
        let file = File::from(file);
        let bytes = read_as_bytes(&file)
            .await
            .map_err(|error| error.to_string())?;
        let data_url = read_as_data_url(&file)
            .await
            .map_err(|error| error.to_string())?;
        let (width, height) = load_image_dimensions(&data_url).await.unwrap_or((0, 0));
        imported.push(ImageAssetRef {
            id: new_id(),
            sha256: sha256_hex(&bytes),
            mime_type: file.raw_mime_type(),
            byte_len: bytes.len() as u64,
            width: (width > 0).then_some(width),
            height: (height > 0).then_some(height),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: Some(data_url),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: BTreeMap::new(),
        });
    }
    Ok(imported)
}

async fn load_image_dimensions(data_url: &str) -> Result<(u32, u32), String> {
    let image = HtmlImageElement::new().map_err(|error| format!("{error:?}"))?;
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let image_for_load = image.clone();
        let onload = Closure::<dyn FnMut()>::once(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
        let onerror = Closure::<dyn FnMut()>::once(move || {
            let _ = reject.call1(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from_str("图片尺寸读取失败"),
            );
        });
        image_for_load.set_onload(Some(onload.as_ref().unchecked_ref()));
        image_for_load.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onload.forget();
        onerror.forget();
    });
    image.set_src(data_url);
    JsFuture::from(promise)
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok((image.natural_width(), image.natural_height()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn mask_key(value: &str) -> String {
    if value.len() <= 6 {
        return "******".into();
    }
    format!("{}***{}", &value[..3], &value[value.len() - 3..])
}
