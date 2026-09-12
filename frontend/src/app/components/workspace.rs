use std::collections::HashSet;

use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::{
    ConversationContextMode, EncryptedApiConfig, ImageAssetRef, LocalTaskRecord, now_rfc3339,
};
use web_sys::{DragEvent, FileList, MouseEvent};

use crate::storage::save_generation_queue_mode;

use crate::app::{
    MAX_ACTIVE_GENERATION_TASKS, asset_display_src, background_mode_label, cycle_background_mode,
    derived::AppDerived,
    ensure_asset_display_sources_loaded, is_openai_image_config,
    models::{ConfirmPopoverKind, ConfirmPopoverState},
    state::{ComposerState, UiState, WorkspaceState},
    thread_display_name, transparent_background_enabled,
    utils::resolution::{
        custom_ratio_dimensions, effective_custom_ratio, preset_dimensions, resolve_dimensions,
    },
};

use super::{asset_drop_zone::AssetDropZone, common::MaterialSymbolIcon};

#[derive(Clone, PartialEq)]
struct ConversationTimelineTurn {
    index: usize,
    task: LocalTaskRecord,
    asset: Option<ImageAssetRef>,
    branched: bool,
}

#[component]
pub(crate) fn WorkspaceMain(
    commit_current_thread_draft: impl Fn() + Copy + Send + Sync + 'static,
    delete_asset: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    delete_thread: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    export_session_backup: impl Fn(String) + Copy + Send + Sync + 'static,
    generate: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    import_reference_assets: impl Fn(FileList) + Copy + Send + Sync + 'static,
    new_thread: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    open_reference_menu: impl Fn(String) + Copy + Send + Sync + 'static,
    open_preview: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
    enter_continuation_context: impl Fn(String, String) + Copy + Send + Sync + 'static,
    persist_state: impl Fn() + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    rename_thread: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    reorder_selected_references: impl Fn(String, String) + Copy + Send + Sync + 'static,
    select_thread: impl Fn(String) + Copy + Send + Sync + 'static,
    update_current_config: impl Fn(fn(&mut EncryptedApiConfig, String), String)
    + Copy
    + Send
    + Sync
    + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let configs = workspace.configs;
    let tasks = workspace.tasks;
    let assets = workspace.assets;
    let threads = workspace.threads;
    let current_thread_id = workspace.current_thread_id;
    let current_config_id = workspace.current_config_id;
    let selected_reference_ids = composer.selected_reference_ids;
    let show_all_reference_assets = composer.show_all_reference_assets;
    let dragging_reference_id = composer.dragging_reference_id;
    let drag_over_reference_id = composer.drag_over_reference_id;
    let continuation_asset_id = composer.continuation_asset_id;
    let continuation_task_id = composer.continuation_task_id;
    let conversation_rebase_requested = composer.conversation_rebase_requested;
    let draft_prompt = composer.draft_prompt;
    let draft_prompt_ref = composer.draft_prompt_ref;
    let custom_width = composer.custom_width;
    let custom_height = composer.custom_height;
    let resolution_mode = composer.resolution_mode;
    let resolution_group = composer.resolution_group;
    let aspect_ratio = composer.aspect_ratio;
    let custom_aspect_ratio_input = composer.custom_aspect_ratio_input;
    let effective_custom_aspect_ratio = composer.effective_custom_aspect_ratio;
    let quality = composer.quality;
    let count = composer.count;
    let status_text = composer.status_text;
    let queue_mode_enabled = composer.queue_mode_enabled;
    let active_generation_ids = composer.active_generation_ids;
    let foreground_generation_task_id = composer.foreground_generation_task_id;
    let generating = composer.generating;
    let show_thread_archive_menu = ui.show_thread_archive_menu;
    let data_management_busy = ui.data_management_busy;
    let confirm_popover = ui.confirm_popover;
    let show_resolution_menu = ui.show_resolution_menu;
    let show_config_switcher = ui.show_config_switcher;
    let current_config = derived.current_config;
    let visible_threads = derived.visible_threads;
    let archived_threads = derived.archived_threads;
    let reference_assets = derived.reference_assets;
    let dimension_reference_assets = derived.dimension_reference_assets;
    let conversation_turns = Memo::new(move |_| {
        let Some(anchor_task_id) = continuation_task_id.get() else {
            return Vec::new();
        };
        let current_asset_id = continuation_asset_id.get();
        tasks.with(|task_items| {
            let chain =
                crate::app::utils::conversation::conversation_chain(task_items, &anchor_task_id);
            assets.with(|asset_items| {
                chain
                    .iter()
                    .enumerate()
                    .map(|(index, task)| {
                        let selected_by_child = chain.get(index + 1).and_then(|child| {
                            child
                                .conversation
                                .as_ref()
                                .filter(|turn| turn.parent_task_id.as_deref() == Some(&task.id))
                                .and_then(|turn| turn.source_result_asset_id.as_deref())
                        });
                        let selected_id = if task.id == anchor_task_id {
                            current_asset_id.as_deref().or(selected_by_child)
                        } else {
                            selected_by_child
                        };
                        let asset = selected_id
                            .and_then(|id| asset_items.iter().find(|asset| asset.id == id))
                            .or_else(|| {
                                asset_items.iter().find(|asset| {
                                    asset.source_task_id.as_deref() == Some(task.id.as_str())
                                })
                            })
                            .cloned();
                        let child_count = task_items
                            .iter()
                            .filter(|candidate| {
                                candidate
                                    .conversation
                                    .as_ref()
                                    .and_then(|turn| turn.parent_task_id.as_deref())
                                    == Some(task.id.as_str())
                            })
                            .count();
                        ConversationTimelineTurn {
                            index,
                            task: (*task).clone(),
                            asset,
                            branched: child_count > 1,
                        }
                    })
                    .collect()
            })
        })
    });
    let conversation_reference_assets = Memo::new(move |_| {
        let selected = selected_reference_ids.get();
        let inherited = continuation_task_id
            .get()
            .and_then(|task_id| {
                tasks.with(|items| {
                    items.iter().find(|task| task.id == task_id).map(|task| {
                        task.ordinary_reference_asset_ids()
                            .cloned()
                            .collect::<HashSet<_>>()
                    })
                })
            })
            .unwrap_or_default();
        assets.with(|items| {
            selected
                .iter()
                .filter_map(|id| {
                    items
                        .iter()
                        .find(|asset| &asset.id == id)
                        .cloned()
                        .map(|asset| {
                            let is_new = !inherited.contains(id);
                            (asset, is_new)
                        })
                })
                .collect::<Vec<_>>()
        })
    });

    Effect::new(move |_| {
        let missing_source_ids = reference_assets
            .get()
            .into_iter()
            .filter(|asset| asset_display_src(asset).is_empty())
            .map(|asset| asset.id)
            .collect::<Vec<_>>();
        if missing_source_ids.is_empty() {
            return;
        }
        spawn_local(async move {
            let _ =
                ensure_asset_display_sources_loaded(workspace.assets, &missing_source_ids).await;
        });
    });

    view! {
                <div class="workspace-main">
                <section class="panel composer-panel">
                    <div class="row composer-title-row">
                        <h2>"提示词与生成"</h2>
                        <div class="composer-title-actions">
                            <button
                                type="button"
                                class="button ghost compact-toggle"
                                class:active-compact-toggle=move || queue_mode_enabled.get()
                                aria-pressed=move || queue_mode_enabled.get()
                                disabled=move || !queue_mode_enabled.get()
                                    && foreground_generation_task_id.get().is_some()
                                title="开启后可连续提交多个并发任务；与连续修改模式互斥"
                                on:click=move |_| {
                                    let next = !queue_mode_enabled.get_untracked();
                                    if next && foreground_generation_task_id.get_untracked().is_some() {
                                        status_text.set("请先等待当前普通任务完成或停止后再开启队列模式。".into());
                                        return;
                                    }
                                    queue_mode_enabled.set(next);
                                    if next {
                                        continuation_asset_id.set(None);
                                        continuation_task_id.set(None);
                                        conversation_rebase_requested.set(false);
                                        status_text.set("已开启队列模式，可以继续编辑并并发提交任务。".into());
                                    } else {
                                        status_text.set("已关闭队列模式。后台任务会继续运行。".into());
                                    }
                                    let _ = save_generation_queue_mode(next);
                                }
                            >
                                <MaterialSymbolIcon name="queue" filled=false />
                                {move || if queue_mode_enabled.get() { "队列：开" } else { "队列：关" }}
                            </button>
                            {move || {
                                let active_count = active_generation_ids.with(HashSet::len);
                                (active_count > 0).then(|| view! {
                                    <span class="tag generation-active-count">{format!("运行中 {active_count} 个")}</span>
                                    <button
                                        type="button"
                                        class="button ghost compact-toggle"
                                        on:click=move |ev: MouseEvent| {
                                            confirm_popover.set(Some(ConfirmPopoverState {
                                                kind: ConfirmPopoverKind::CancelAllGenerations,
                                                title: "停止全部生成".into(),
                                                message: "确定停止当前全部生成任务吗？已经发送到上游的请求可能仍会产生消耗。".into(),
                                                x: ev.client_x() as f64,
                                                y: ev.client_y() as f64,
                                            }));
                                        }
                                    >
                                        <MaterialSymbolIcon name="stop" filled=true />
                                        "停止全部"
                                    </button>
                                })
                            }}
                        </div>
                        <div class="config-switcher">
                            <button
                                class="tag config-switcher-button"
                                title="切换服务商配置"
                                on:click=move |_| show_config_switcher.update(|value| *value = !*value)
                            >
                                <span>
                                    {move || {
                                        current_config
                                            .get()
                                            .map(|config| format!("{} · {}", config.name, config.model))
                                            .unwrap_or_else(|| "未配置模型".into())
                                    }}
                                </span>
                                <MaterialSymbolIcon name="expand_more" filled=false />
                            </button>
                            {move || if show_config_switcher.get() {
                                view! {
                                    <div class="config-switcher-menu">
                                        <For
                                            each=move || configs.get()
                                            key=|config| config.id.clone()
                                            children=move |config| {
                                                let config_id = config.id.clone();
                                                let is_active_id = config.id.clone();
                                                let checked_id = config.id.clone();
                                                let config_name = config.name.clone();
                                                let config_model = config.model.clone();
                                                let config_title = format!("{} · {}", config_name, config_model);
                                                view! {
                                                    <button
                                                        class="config-switcher-item"
                                                        class:is-active=move || current_config_id.get() == is_active_id
                                                        title=config_title
                                                        on:click=move |_| {
                                                            current_config_id.set(config_id.clone());
                                                            show_config_switcher.set(false);
                                                        }
                                                    >
                                                        <span class="config-switcher-name">{config_name}</span>
                                                        <span class="config-switcher-model">{config_model}</span>
                                                        {move || if current_config_id.get() == checked_id {
                                                            view! { <MaterialSymbolIcon name="check" filled=false /> }.into_any()
                                                        } else {
                                                            ().into_any()
                                                        }}
                                                    </button>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                ().into_any()
                            }}
                        </div>
                    </div>

                    <div class="thread-strip">
                        <For
                            each=move || visible_threads.get()
                            key=|thread| thread.id.clone()
                            children=move |thread| {
                                let thread_id = thread.id.clone();
                                let active_thread_id = thread_id.clone();
                                let click_thread_id = thread_id.clone();
                                let label_thread_id = thread_id.clone();
                                let rename_thread_id = thread_id.clone();
                                let export_thread_id = thread_id.clone();
                                let delete_thread_id = thread_id.clone();
                                view! {
                                    <div class="thread-chip">
                                        <button
                                            class="chip-button thread-chip-button"
                                            class:active-chip=move || current_thread_id.get() == active_thread_id
                                            on:click=move |_| select_thread(click_thread_id.clone())
                                        >
                                            <span class="thread-chip-label">
                                                {move || threads.with(|items| {
                                                    items
                                                        .iter()
                                                        .find(|item| item.id == label_thread_id)
                                                        .map(thread_display_name)
                                                        .unwrap_or_else(|| "新的会话".into())
                                                })}
                                            </span>
                                        </button>
                                        <div class="thread-chip-actions">
                                            <button
                                                class="button ghost mini-action icon-action"
                                                title="导出会话"
                                                disabled=move || data_management_busy.get() || generating.get()
                                                on:click=move |_| export_session_backup(export_thread_id.clone())
                                            >
                                                <MaterialSymbolIcon name="download" filled=false />
                                            </button>
                                            <button
                                                class="button ghost mini-action icon-action"
                                                title="重命名会话"
                                                on:click=move |ev: MouseEvent| rename_thread(rename_thread_id.clone(), ev.client_x() as f64, ev.client_y() as f64)
                                            >
                                                <MaterialSymbolIcon name="edit_square" filled=false />
                                            </button>
                                            <button
                                                class="button ghost danger mini-action icon-action"
                                                title="删除会话"
                                                on:click=move |ev: MouseEvent| delete_thread(delete_thread_id.clone(), ev.client_x() as f64, ev.client_y() as f64)
                                            >
                                                <MaterialSymbolIcon name="delete" filled=false />
                                            </button>
                                        </div>
                                    </div>
                                }
                            }
                        />
                        <button class="chip-button add-chip" on:click=new_thread>"+" "新会话"</button>
                        {move || if !archived_threads.get().is_empty() {
                            view! {
                                <div class="thread-archive">
                                    <button
                                        class="button ghost icon-button thread-archive-button"
                                        title="归档会话"
                                        on:click=move |_| show_thread_archive_menu.update(|value| *value = !*value)
                                    >
                                        <MaterialSymbolIcon name="archive" filled=false />
                                    </button>
                                    {move || if show_thread_archive_menu.get() {
                                        view! {
                                            <>
                                                <button class="thread-archive-dismiss" aria-label="关闭归档会话菜单" on:click=move |_| show_thread_archive_menu.set(false)></button>
                                                <div class="thread-archive-menu">
                                                    <For
                                                        each=move || archived_threads.get()
                                                        key=|thread| thread.id.clone()
                                                        children=move |thread| {
                                                            let thread_id = thread.id.clone();
                                                            let label_thread_id = thread_id.clone();
                                                            let export_thread_id = thread_id.clone();
                                                            view! {
                                                                <div class="thread-archive-row">
                                                                    <button class="thread-archive-item" on:click=move |_| select_thread(thread_id.clone())>
                                                                        <MaterialSymbolIcon name="forum" filled=false />
                                                                        <span>
                                                                            {move || threads.with(|items| {
                                                                                items
                                                                                    .iter()
                                                                                    .find(|item| item.id == label_thread_id)
                                                                                    .map(thread_display_name)
                                                                                    .unwrap_or_else(|| "新的会话".into())
                                                                            })}
                                                                        </span>
                                                                    </button>
                                                                    <button
                                                                        class="thread-archive-export"
                                                                        title="导出会话"
                                                                        disabled=move || data_management_busy.get() || generating.get()
                                                                        on:click=move |_| export_session_backup(export_thread_id.clone())
                                                                    >
                                                                        <MaterialSymbolIcon name="download" filled=false />
                                                                    </button>
                                                                </div>
                                                            }
                                                        }
                                                    />
                                                </div>
                                            </>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }}
                                </div>
                            }.into_any()
                        } else {
                            ().into_any()
                        }}
                    </div>

                    <textarea
                        class="prompt-input"
                        prop:value=move || draft_prompt.get()
                        node_ref=draft_prompt_ref
                        placeholder="不知道做什么？去模板广场看看吧~"
                        on:input=move |ev| {
                            draft_prompt.set(event_target_value(&ev));
                        }
                        on:blur=move |_| {
                            commit_current_thread_draft();
                            persist_state();
                        }
                    />

                    <Show when=move || continuation_task_id.get().is_some()>
                        <section class="conversation-timeline-panel" aria-label="连续图像对话">
                            <div class="conversation-timeline-header">
                                <div class="conversation-mode-summary">
                                    <strong>{move || {
                                        if conversation_rebase_requested.get() {
                                            return "下轮重建";
                                        }
                                        conversation_turns
                                            .get()
                                            .last()
                                            .and_then(|turn| turn.task.conversation.as_ref())
                                            .map(|turn| match turn.mode {
                                                ConversationContextMode::NativeResponses => "原生 Responses",
                                                ConversationContextMode::Compatibility => "兼容上下文",
                                                ConversationContextMode::Rebased => "本轮重建",
                                            })
                                            .unwrap_or("连续图像对话")
                                    }}</strong>
                                    <span>{move || {
                                        if conversation_rebase_requested.get() {
                                            return "等待你手动生成；不会自动重发上一轮请求。".to_string();
                                        }
                                        conversation_turns
                                            .get()
                                            .last()
                                            .and_then(|turn| turn.task.conversation.as_ref())
                                            .map(|turn| match turn.mode {
                                                ConversationContextMode::NativeResponses => "保留上游会话，后续只发送本轮新增输入。".into(),
                                                ConversationContextMode::Compatibility => format!("携带整理后的历史要求与最新结果；实际历史 {} 轮。", turn.included_history_turns),
                                                ConversationContextMode::Rebased => format!("配置或参考图发生变化，已从当前结果重建上下文；实际历史 {} 轮。", turn.included_history_turns),
                                            })
                                            .unwrap_or_else(|| "从当前结果继续修改。".into())
                                    }}</span>
                                </div>
                                <button class="button ghost conversation-exit" on:click=move |_| {
                                    batch(|| {
                                        continuation_asset_id.set(None);
                                        continuation_task_id.set(None);
                                        conversation_rebase_requested.set(false);
                                    });
                                    status_text.set("已退出连续对话，当前普通参考图选择已保留。".into());
                                }>"退出连续对话"</button>
                                <button
                                    class="button ghost conversation-rebase"
                                    class:is-active=move || conversation_rebase_requested.get()
                                    title="下一次手动生成时不使用旧 Response ID，并从当前结果重建会话"
                                    on:click=move |_| {
                                        conversation_rebase_requested.set(true);
                                        status_text.set("已标记为下轮重建；请确认要求后手动生成，不会自动重试或重复计费。".into());
                                    }
                                >"重建上下文"</button>
                            </div>
                            <Show when=move || !conversation_reference_assets.get().is_empty()>
                                <div class="conversation-reference-row">
                                    <span>"当前参考图"</span>
                                    <For
                                        each=move || conversation_reference_assets.get()
                                        key=|(asset, _)| asset.id.clone()
                                        children=move |(asset, is_new)| {
                                            let remove_id = asset.id.clone();
                                            view! {
                                                <button
                                                    class="conversation-reference-chip"
                                                    class:is-new=is_new
                                                    title=if is_new { "本轮新增；点击移除" } else { "从父轮继承；点击移除并触发上下文重建" }
                                                    on:click=move |_| {
                                                        selected_reference_ids.update(|ids| ids.retain(|id| id != &remove_id));
                                                        status_text.set("已从下一轮输入移除该参考图；若它来自父轮，将在生成时重建上下文。".into());
                                                    }
                                                >
                                                    <img src=asset_display_src(&asset) alt="当前参考图" />
                                                    {is_new.then(|| view! { <span>"新"</span> })}
                                                </button>
                                            }
                                        }
                                    />
                                </div>
                            </Show>
                            <div class="conversation-timeline" role="list">
                                <For
                                    each=move || conversation_turns.get()
                                    key=|turn| turn.task.id.clone()
                                    children=move |turn| {
                                        let task_id = turn.task.id.clone();
                                        let current_task_id = task_id.clone();
                                        let prompt = turn.task.prompt.clone();
                                        let turn_number = turn.index + 1;
                                        let asset = turn.asset.clone();
                                        let branched = turn.branched;
                                        let preview_task_id = task_id.clone();
                                        let preview_asset_id = asset.as_ref().map(|asset| asset.id.clone());
                                        let edit_asset_id = preview_asset_id.clone();
                                        let continue_asset_id = preview_asset_id.clone();
                                        let continue_task_id = task_id.clone();
                                        view! {
                                            <article
                                                class="conversation-turn"
                                                class:is-current=move || continuation_task_id.get().as_deref() == Some(current_task_id.as_str())
                                                class:has-branch=branched
                                                role="listitem"
                                            >
                                                <div class="conversation-turn-copy">
                                                    <span class="conversation-turn-number">{format!("第 {turn_number} 轮")}</span>
                                                    <span class="conversation-turn-prompt">{prompt}</span>
                                                </div>
                                                {asset.map(|asset| {
                                                    let src = asset_display_src(&asset);
                                                    view! {
                                                        <img class="conversation-turn-thumb" src=src alt="该轮结果" />
                                                        <div class="conversation-turn-actions">
                                                            <button class="button ghost icon-action" title="查看大图" on:click=move |_| {
                                                                open_preview(preview_task_id.clone(), preview_asset_id.clone());
                                                            }><MaterialSymbolIcon name="zoom_in" filled=false /></button>
                                                            <button class="button ghost icon-action" title="编辑此图" on:click=move |_| {
                                                                ui.image_editor_base_id.set(edit_asset_id.clone());
                                                                ui.image_editor_thread.set(Some(current_thread_id.get_untracked()));
                                                            }><MaterialSymbolIcon name="edit" filled=false /></button>
                                                            <button class="button ghost icon-action" title="从此继续" on:click=move |_| {
                                                                if let Some(asset_id) = continue_asset_id.clone() {
                                                                    enter_continuation_context(continue_task_id.clone(), asset_id);
                                                                }
                                                            }><MaterialSymbolIcon name="fork_right" filled=false /></button>
                                                        </div>
                                                    }.into_any()
                                                }).unwrap_or_else(|| view! {
                                                    <span class="conversation-turn-missing">"结果不可用"</span>
                                                }.into_any())}
                                            </article>
                                        }
                                    }
                                />
                            </div>
                        </section>
                    </Show>

                    <div class="settings-inline">
                        <button
                            class="resolution-button"
                            on:click=move |_| show_resolution_menu.update(|value| *value = !*value)
                        >
                            {move || {
                                if resolution_mode.get() == "model_auto" {
                                    return "分辨率：模型自动".to_string();
                                }
                                let (width, height) = resolve_dimensions(
                                    resolution_mode.get().as_str(),
                                    resolution_group.get().as_str(),
                                    aspect_ratio.get().as_str(),
                                    effective_custom_aspect_ratio.get().as_str(),
                                    custom_width.get(),
                                    custom_height.get(),
                                    &dimension_reference_assets.get(),
                                );
                                format!("分辨率：{} × {}", width, height)
                            }}
                        </button>
                        {move || if current_config.get().map(|config| is_openai_image_config(&config)).unwrap_or(false) {
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
                                        <option value="auto">"质量：自动"</option>
                                        <option value="xhigh" disabled=move || current_config.get().is_some_and(|config| !mew_image_shared::supports_extended_image_quality(&config.model))>"质量：超高"</option>
                                        <option value="max" disabled=move || current_config.get().is_some_and(|config| !mew_image_shared::supports_extended_image_quality(&config.model))>"质量：最高"</option>
                                    </select>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || current_config.get().and_then(|config| config.output_format).unwrap_or_else(|| "png".into())
                                        on:change=move |ev| update_current_config(|config, value| config.output_format = Some(value), event_target_value(&ev))
                                    >
                                        <option value="png">"格式：PNG"</option>
                                        <option
                                            value="jpeg"
                                            disabled=move || current_config
                                                .get()
                                                .is_some_and(|config| transparent_background_enabled(&config))
                                        >"格式：JPEG"</option>
                                        <option value="webp">"格式：WEBP"</option>
                                    </select>
                                    <button
                                        type="button"
                                        class="button ghost compact-toggle"
                                        class:active-compact-toggle=move || current_config
                                            .get()
                                            .is_some_and(|config| transparent_background_enabled(&config))
                                        aria-pressed=move || current_config
                                            .get()
                                            .is_some_and(|config| transparent_background_enabled(&config))
                                        title="依次切换关闭、API 原生透明和浏览器本地去背景；透明输出仅支持 PNG 或 WebP"
                                        on:click=move |_| {
                                            configs.update(|items| {
                                                let Some(config) = items
                                                    .iter_mut()
                                                    .find(|config| config.id == current_config_id.get_untracked())
                                                else {
                                                    return;
                                                };
                                                cycle_background_mode(config);
                                                config.updated_at = now_rfc3339();
                                            });
                                            persist_ui_state();
                                        }
                                    >
                                        <MaterialSymbolIcon name="opacity" filled=false />
                                        {move || current_config
                                            .get()
                                            .as_ref()
                                            .map(background_mode_label)
                                            .unwrap_or("透明：关")}
                                    </button>
                                    <div class="compact-stepper compression-stepper" aria-label="压缩率">
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| {
                                                let value = current_config.get_untracked().and_then(|config| config.output_compression).unwrap_or(100).saturating_sub(1);
                                                configs.update(|items| {
                                                    if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                                        config.output_compression = Some(value);
                                                        config.updated_at = now_rfc3339();
                                                    }
                                                });
                                                persist_ui_state();
                                            }
                                        >"-"</button>
                                        <input
                                            class="stepper-value"
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
                                                persist_ui_state();
                                            }
                                        />
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| {
                                                let value = current_config.get_untracked().and_then(|config| config.output_compression).unwrap_or(100).saturating_add(1).min(100);
                                                configs.update(|items| {
                                                    if let Some(config) = items.iter_mut().find(|config| config.id == current_config_id.get_untracked()) {
                                                        config.output_compression = Some(value);
                                                        config.updated_at = now_rfc3339();
                                                    }
                                                });
                                                persist_ui_state();
                                            }
                                        >"+"</button>
                                    </div>
                                    <select
                                        class="select-input compact-select"
                                        prop:value=move || current_config.get().and_then(|config| config.moderation).unwrap_or_else(|| "auto".into())
                                        on:change=move |ev| update_current_config(|config, value| config.moderation = Some(value), event_target_value(&ev))
                                    >
                                        <option value="auto">"审核：自动"</option>
                                        <option value="low">"审核：宽松"</option>
                                    </select>
                                    <select
                                        class="select-input compact-select codex-compat-select"
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
                                            persist_ui_state();
                                        }
                                    >
                                        <option value="on">"Codex 兼容：开"</option>
                                        <option value="off">"Codex 兼容：关"</option>
                                    </select>
                                    <div class="compact-stepper count-stepper" aria-label="生成数量">
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| count.update(|value| *value = value.saturating_sub(1).clamp(1, 4))
                                        >"-"</button>
                                        <input
                                            class="stepper-value"
                                            type="number"
                                            min="1"
                                            max="4"
                                            prop:value=move || count.get().to_string()
                                            on:input=move |ev| count.set(event_target_value(&ev).parse().unwrap_or(1).clamp(1, 4))
                                        />
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| count.update(|value| *value = value.saturating_add(1).clamp(1, 4))
                                        >"+"</button>
                                    </div>
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
                                    <div class="compact-stepper count-stepper" aria-label="生成数量">
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| count.update(|value| *value = value.saturating_sub(1).clamp(1, 4))
                                        >"-"</button>
                                        <input
                                            class="stepper-value"
                                            type="number"
                                            min="1"
                                            max="4"
                                            prop:value=move || count.get().to_string()
                                            on:input=move |ev| count.set(event_target_value(&ev).parse().unwrap_or(1).clamp(1, 4))
                                        />
                                        <button
                                            type="button"
                                            class="stepper-button"
                                            on:click=move |_| count.update(|value| *value = value.saturating_add(1).clamp(1, 4))
                                        >"+"</button>
                                    </div>
                                </>
                            }.into_any()
                        }}
                    </div>

                    {move || if show_resolution_menu.get() {
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
                                        <span class="tag">
                                            {move || {
                                                // 只更新预览文本，避免输入宽高时重建整个弹层并丢失焦点。
                                                if resolution_mode.get() == "model_auto" {
                                                    return "由模型决定实际尺寸；按最大输出尺寸预留内存。".to_string();
                                                }
                                                let (preview_width, preview_height) = resolve_dimensions(
                                                    resolution_mode.get().as_str(),
                                                    resolution_group.get().as_str(),
                                                    aspect_ratio.get().as_str(),
                                                    effective_custom_aspect_ratio.get().as_str(),
                                                    custom_width.get(),
                                                    custom_height.get(),
                                                    &dimension_reference_assets.get(),
                                                );
                                                format!("当前预览：{} × {}", preview_width, preview_height)
                                            }}
                                        </span>
                                    </div>
                                    <div class="tag">"Responses API 使用配置中的主模型，通过 image_generation 工具调用所选图片模型。超过 2560×1440 的 GPT Image 2/2.5 输出为实验性。"</div>
                                    <div class="mode-tabs">
                                        <button class="chip-button" class:active-chip=move || resolution_mode.get() == "auto" on:click=move |_| resolution_mode.set("auto".into())>"自动"</button>
                                        <button class="chip-button" class:active-chip=move || resolution_mode.get() == "model_auto" disabled=move || !current_config.get().is_some_and(|config| config.provider_kind == mew_image_shared::ProviderKind::OpenAiImage) on:click=move |_| resolution_mode.set("model_auto".into())>"模型自动"</button>
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
                                                        <button
                                                            class="chip-button"
                                                            class:active-chip=move || aspect_ratio.get() == "custom"
                                                            on:click=move |_| aspect_ratio.set("custom".into())
                                                        >
                                                            "自定义比例"
                                                        </button>
                                                    </div>
                                                    <div
                                                        class="custom-ratio-editor"
                                                        class:custom-ratio-editor-hidden=move || aspect_ratio.get() != "custom"
                                                    >
                                                        <label for="custom-aspect-ratio">"输入自定义比例"</label>
                                                        <input
                                                            id="custom-aspect-ratio"
                                                            class="field"
                                                            class:invalid-field=move || custom_ratio_dimensions(
                                                                resolution_group.get().as_str(),
                                                                custom_aspect_ratio_input.get().as_str(),
                                                            ).is_none()
                                                            type="text"
                                                            inputmode="decimal"
                                                            placeholder="例如 5:4 / 2.39:1"
                                                            prop:value=move || custom_aspect_ratio_input.get()
                                                            on:input=move |ev| {
                                                                let value = event_target_value(&ev);
                                                                custom_aspect_ratio_input.set(value.clone());
                                                                // 输入中间态不覆盖最后一个有效比例，避免生成尺寸意外跳变。
                                                                if custom_ratio_dimensions(
                                                                    resolution_group.get_untracked().as_str(),
                                                                    &value,
                                                                ).is_some() {
                                                                    effective_custom_aspect_ratio.set(value);
                                                                }
                                                            }
                                                        />
                                                        <div class="resolution-effective-card">
                                                            {move || {
                                                                let input_ratio = custom_aspect_ratio_input.get();
                                                                if let Some((width, height)) = custom_ratio_dimensions(
                                                                    resolution_group.get().as_str(),
                                                                    &input_ratio,
                                                                ) {
                                                                    let limit_label = effective_custom_ratio(&input_ratio)
                                                                        .and_then(|(_, limit_label)| limit_label);
                                                                    if let Some(limit_label) = limit_label {
                                                                        format!(
                                                                            "目标比例超出官方 3:1 限制，按最接近的 {} 生效：{} × {}",
                                                                            limit_label, width, height,
                                                                        )
                                                                    } else {
                                                                        format!("实际生效：{} × {}", width, height)
                                                                    }
                                                                } else {
                                                                    let effective_ratio = effective_custom_aspect_ratio.get();
                                                                    let (width, height) = custom_ratio_dimensions(
                                                                        resolution_group.get().as_str(),
                                                                        &effective_ratio,
                                                                    ).unwrap_or_else(|| preset_dimensions(
                                                                        resolution_group.get().as_str(),
                                                                        "1:1",
                                                                    ));
                                                                    format!(
                                                                        "比例格式无效，暂按 {}（{} × {}）生效",
                                                                        effective_ratio, width, height,
                                                                    )
                                                                }
                                                            }}
                                                        </div>
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
                                                    <div class="resolution-effective-card">
                                                        {move || {
                                                            let input_width = custom_width.get();
                                                            let input_height = custom_height.get();
                                                            format!("请求尺寸：{input_width} × {input_height}；提交前校验，不自动调整。")
                                                        }}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="stack resolution-panel">
                                                    <div class="tag">{move || if resolution_mode.get() == "model_auto" { "模型自动模式会发送 size=auto，不沿用参考图尺寸。" } else { "自动模式会优先沿用参考图或上一轮结果的尺寸。" }}</div>
                                                    <div class="tag">{move || if resolution_mode.get() == "model_auto" { "生成后读取图片实际尺寸；预算按最大输出尺寸计算。" } else { "如果当前没有参考图，则会回落到 1024 × 1024。" }}</div>
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

                    {move || if let Some(task_id) = foreground_generation_task_id.get() {
                        view! {
                            <button
                                type="button"
                                class="button generation-cancel-button"
                                title="停止当前生成任务"
                                on:click=move |ev: MouseEvent| {
                                    confirm_popover.set(Some(ConfirmPopoverState {
                                        kind: ConfirmPopoverKind::CancelGeneration(task_id.clone()),
                                        title: "停止生成".into(),
                                        message: "确定停止当前生成任务吗？已经发送到上游的请求可能仍会产生消耗。".into(),
                                        x: ev.client_x() as f64,
                                        y: ev.client_y() as f64,
                                    }));
                                }
                            >
                                <span class="generation-cancel-spinner">
                                    <MaterialSymbolIcon name="stop" filled=true />
                                </span>
                                <span>"停止生成"</span>
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <button
                                class="button generation-submit-button"
                                disabled=move || {
                                    active_generation_ids.with(HashSet::len)
                                        >= MAX_ACTIVE_GENERATION_TASKS
                                }
                                on:click=generate
                            >
                                <span class="generation-submit-label">"开始生成"</span>
                            </button>
                        }.into_any()
                    }}
                    <span class="status">{move || status_text.get()}</span>
                </section>

                <section class="panel asset-panel">
                    <section class="stack">
                        <div class="row reference-title-row">
                            <h2>"参考图"</h2>
                            <div class="reference-title-actions">
                                <button type="button" class="button ghost compact-toggle"
                                    on:click=move |_| {
                                        ui.image_editor_base_id.set(None);
                                        ui.image_editor_thread.set(Some(current_thread_id.get_untracked()));
                                    }>
                                    <MaterialSymbolIcon name="draw" filled=false />"绘制草图"
                                </button>
                                <span class="tag">{move || {
                                    let selected = selected_reference_ids.get();
                                    let continuation = continuation_asset_id.get();
                                    let total = selected.len()
                                        + usize::from(continuation.as_ref().is_some_and(|id| !selected.contains(id)));
                                    format!("输入图片 {total}/10 · 普通参考图 {} 张", selected.len())
                                }}</span>
                                <button
                                    type="button"
                                    class="button ghost compact-toggle"
                                    class:active-compact-toggle=move || show_all_reference_assets.get()
                                    aria-pressed=move || show_all_reference_assets.get()
                                    title="显示当前会话上传过或历史任务使用过的全部参考图"
                                    on:click=move |_| show_all_reference_assets.update(|value| *value = !*value)
                                >
                                    <MaterialSymbolIcon name="collections" filled=false />
                                    {move || if show_all_reference_assets.get() { "显示全部：开" } else { "显示全部：关" }}
                                </button>
                            </div>
                        </div>
                        <div class="preview-strip">
                            <For
                                each=move || {
                                    // Object URL 存在于运行时缓存，直接追踪资产信号才能响应缓存装载。
                                    workspace.assets.track();
                                    reference_assets.get()
                                }
                                key=|asset| (asset.id.clone(), asset_display_src(asset))
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
                                                            let base = composer.continuation_asset_id.get_untracked();
                                                            let extra = usize::from(base.as_ref().is_some_and(|id| id != &toggle_reference_id && !ids.contains(id)));
                                                            if ids.len() + extra >= mew_image_shared::MAX_GENERATION_REFERENCE_IMAGES {
                                                                composer.status_text.set("最多选择 10 张参考图，请先取消部分选择。".into());
                                                                return;
                                                            }
                                                            ids.push(toggle_reference_id.clone());
                                                        }
                                                    });
                                                }>
                                                    {move || if selected_reference_ids.get().contains(&toggle_reference_label_id) { "取消参考" } else { "设为参考" }}
                                                </button>
                                                <button class="button ghost danger mini-action icon-action" title="删除参考图" on:click=move |ev: MouseEvent| delete_asset(delete_asset_id.clone(), ev.client_x() as f64, ev.client_y() as f64)><MaterialSymbolIcon name="delete" filled=false /></button>
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
    }
}
