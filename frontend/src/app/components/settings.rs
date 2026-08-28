use leptos::prelude::*;
use mew_image_shared::CloudDataClearScope;
use web_sys::{Event, MouseEvent};

use crate::app::{
    components::{
        common::{GitHubIcon, MaterialSymbolIcon},
        config_editor::ConfigEditor,
    },
    derived::AppDerived,
    format_byte_size,
    models::LocalDataClearScope,
    state::{AccountState, ComposerState, UiState, WorkspaceState},
    thread_display_name,
};

#[component]
pub(crate) fn SettingsOverlay(
    add_config: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    admin_user_action: impl Fn(&'static str, String) + Copy + Send + Sync + 'static,
    bootstrap_current_user_as_admin: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    change_password: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    check_username_availability: impl Fn() + Copy + Send + Sync + 'static,
    confirm_cloud_clear: impl Fn(CloudDataClearScope, MouseEvent) + Copy + Send + Sync + 'static,
    confirm_local_clear: impl Fn(LocalDataClearScope, MouseEvent) + Copy + Send + Sync + 'static,
    delete_config: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    delete_managed_user: impl Fn(String, f64, f64) + Copy + Send + Sync + 'static,
    export_local_backup: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    export_session_backup: impl Fn(String) + Copy + Send + Sync + 'static,
    import_local_backup: impl Fn(Event) + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    refresh_admin_users: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
    refresh_cloud_data_stats: impl Fn() + Copy + Send + Sync + 'static,
    submit_auth: impl Fn(&'static str) + Copy + Send + Sync + 'static,
    sync_action: impl Fn() + Copy + Send + Sync + 'static,
    toggle_api_key_sync: impl Fn(Event) + Copy + Send + Sync + 'static,
    unlock_api_key_sync: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let account = expect_context::<AccountState>();
    let composer = expect_context::<ComposerState>();
    let ui = expect_context::<UiState>();
    let derived = expect_context::<AppDerived>();
    let configs = workspace.configs;
    let tasks = workspace.tasks;
    let threads = workspace.threads;
    let assets = workspace.assets;
    let templates = workspace.templates;
    let current_config_id = workspace.current_config_id;
    let current_thread_id = workspace.current_thread_id;
    let generating = composer.generating;
    let auth_user = account.auth_user;
    let login_username = account.login_username;
    let login_password = account.login_password;
    let auth_mode = account.auth_mode;
    let register_password_confirm = account.register_password_confirm;
    let admin_setup_token = account.admin_setup_token;
    let show_admin_setup_token = account.show_admin_setup_token;
    let admin_setup_allowed = account.admin_setup_allowed;
    let auth_form_message = account.auth_form_message;
    let username_check_message = account.username_check_message;
    let change_old_password = account.change_old_password;
    let change_new_password = account.change_new_password;
    let change_new_password_confirm = account.change_new_password_confirm;
    let password_form_message = account.password_form_message;
    let admin_users = account.admin_users;
    let loading_admin_users = account.loading_admin_users;
    let sync_secret = account.sync_secret;
    let sync_api_keys_enabled = account.sync_api_keys_enabled;
    let sync_unlock_password = account.sync_unlock_password;
    let sync_unlocking = account.sync_unlocking;
    let sync_status_text = account.sync_status_text;
    let syncing = account.syncing;
    let show_settings_menu = ui.show_settings_menu;
    let settings_tab = ui.settings_tab;
    let data_management_tab = ui.data_management_tab;
    let data_management_busy = ui.data_management_busy;
    let data_management_message = ui.data_management_message;
    let session_export_thread_id = ui.session_export_thread_id;
    let cloud_data_stats = ui.cloud_data_stats;
    let backup_file_input = ui.backup_file_input;
    let current_config = derived.current_config;

    view! {
            {move || if show_settings_menu.get() {
                view! {
                    <div class="settings-overlay" on:click=move |_| show_settings_menu.set(false)>
                        <div class="settings-popover" on:click=move |ev: MouseEvent| ev.stop_propagation()>
                            <div class="settings-shell">
                                <aside class="settings-sidebar">
                                    <div class="settings-sidebar-main">
                                        <button
                                            class="settings-nav-button"
                                            class:is-active=move || settings_tab.get() == "providers"
                                            on:click=move |_| settings_tab.set("providers".into())
                                        >
                                            <MaterialSymbolIcon name="tune" filled=false />
                                            <span>"服务商配置"</span>
                                        </button>
                                        <button
                                            class="settings-nav-button"
                                            class:is-active=move || settings_tab.get() == "account"
                                            on:click=move |_| settings_tab.set("account".into())
                                        >
                                            <MaterialSymbolIcon name="cloud_sync" filled=false />
                                            <span>"账号与同步"</span>
                                        </button>
                                        {move || if auth_user.get().is_some() {
                                            view! {
                                                <button
                                                    class="settings-nav-button"
                                                    class:is-active=move || settings_tab.get() == "password"
                                                    on:click=move |_| settings_tab.set("password".into())
                                                >
                                                    <MaterialSymbolIcon name="lock_reset" filled=false />
                                                    <span>"密码更改"</span>
                                                </button>
                                            }.into_any()
                                        } else {
                                            ().into_any()
                                        }}
                                        {move || if auth_user
                                            .get()
                                            .map(|user| user.role == "admin")
                                            .unwrap_or(false)
                                        {
                                            view! {
                                                <button
                                                    class="settings-nav-button"
                                                    class:is-active=move || settings_tab.get() == "admin"
                                                    on:click=move |_| settings_tab.set("admin".into())
                                                >
                                                    <MaterialSymbolIcon name="admin_panel_settings" filled=false />
                                                    <span>"用户管理"</span>
                                                </button>
                                            }.into_any()
                                        } else {
                                            ().into_any()
                                        }}
                                        <button
                                            class="settings-nav-button"
                                            class:is-active=move || settings_tab.get() == "data"
                                            on:click=move |_| settings_tab.set("data".into())
                                        >
                                            <MaterialSymbolIcon name="database" filled=false />
                                            <span>"数据管理"</span>
                                        </button>
                                    </div>
                                    <button
                                        class="settings-nav-button settings-about-button"
                                        class:is-active=move || settings_tab.get() == "about"
                                        on:click=move |_| settings_tab.set("about".into())
                                    >
                                        <MaterialSymbolIcon name="info" filled=false />
                                        <span>"关于"</span>
                                    </button>
                                </aside>
                                <div class="settings-content">
                                    {move || match settings_tab.get().as_str() {
                                        "account" => view! {
                                            <section class="stack">
                                                <div class="row">
                                                    <h2>"账号与同步"</h2>
                                                    <span class="tag">{move || auth_user
                                                        .get()
                                                        .map(|user| format!("{} · {} · 服务器图片 {} 张", user.username, user.status, user.image_count))
                                                        .unwrap_or_else(|| "游客本地 + 受限代理模式".into())}</span>
                                                </div>
                                                <p class="status">
                                                    {move || {
                                                        match auth_user.get() {
                                                            Some(user) if user.status == "approved" => {
                                                                "已登录且审批通过：本地继续优先，只有点击“立即同步”才会上云。".to_string()
                                                            }
                                                            Some(_) => {
                                                                "已登录但账号待审批：可以继续本地使用，暂不能使用云端同步和服务器资源存储。".to_string()
                                                            }
                                                            None => {
                                                                "未登录：会话、历史、参考图和配置都保存在当前浏览器；代理仅临时中转请求，不写入云端同步或对象存储。".to_string()
                                                            }
                                                        }
                                                    }}
                                                </p>
                                                <div class="settings-form-card">
                                                    <div class="auth-mode-tabs">
                                                        <button
                                                            class="auth-mode-button"
                                                            class:is-active=move || auth_mode.get() == "login"
                                                            on:click=move |_| {
                                                                auth_mode.set("login".into());
                                                                username_check_message.set(None);
                                                                auth_form_message.set(None);
                                                            }
                                                        >
                                                            "登录"
                                                        </button>
                                                        <button
                                                            class="auth-mode-button"
                                                            class:is-active=move || auth_mode.get() == "register"
                                                            on:click=move |_| {
                                                                auth_mode.set("register".into());
                                                                username_check_message.set(None);
                                                                auth_form_message.set(None);
                                                            }
                                                        >
                                                            "注册"
                                                        </button>
                                                    </div>
                                                    <input
                                                        class="text-input"
                                                        placeholder="用户名"
                                                        prop:value=move || login_username.get()
                                                        on:input=move |ev| {
                                                            login_username.set(event_target_value(&ev));
                                                            username_check_message.set(None);
                                                            auth_form_message.set(None);
                                                        }
                                                        on:blur=move |_| {
                                                            if auth_mode.get_untracked() == "register" {
                                                                check_username_availability();
                                                            }
                                                        }
                                                    />
                                                    {move || if auth_mode.get() == "register" {
                                                        username_check_message.get().map(|message| view! {
                                                            <p class="form-hint">{message}</p>
                                                        }).into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    <input
                                                        class="text-input"
                                                        type="password"
                                                        placeholder="密码"
                                                        prop:value=move || login_password.get()
                                                        on:input=move |ev| {
                                                            login_password.set(event_target_value(&ev));
                                                            auth_form_message.set(None);
                                                        }
                                                    />
                                                    {move || if auth_mode.get() == "register" {
                                                        view! {
                                                            <input
                                                                class="text-input"
                                                                type="password"
                                                                placeholder="确认密码"
                                                                prop:value=move || register_password_confirm.get()
                                                                on:input=move |ev| {
                                                                    register_password_confirm.set(event_target_value(&ev));
                                                                    auth_form_message.set(None);
                                                                }
                                                            />
                                                        }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    {move || if admin_setup_allowed.get()
                                                        && auth_user.get().map(|user| user.role != "admin").unwrap_or(true)
                                                    {
                                                        view! {
                                                            <div class="stack admin-bootstrap-block">
                                                                <button
                                                                    class="button ghost"
                                                                    on:click=move |_| show_admin_setup_token.update(|value| *value = !*value)
                                                                >
                                                                    {move || if show_admin_setup_token.get() { "隐藏管理员初始化" } else { "使用管理员初始化口令" }}
                                                                </button>
                                                                {move || if show_admin_setup_token.get() {
                                                                    view! {
                                                                        <div class="stack">
                                                                            <input
                                                                                class="text-input"
                                                                                type="password"
                                                                                placeholder="管理员初始化口令"
                                                                                prop:value=move || admin_setup_token.get()
                                                                                on:input=move |ev| {
                                                                                    admin_setup_token.set(event_target_value(&ev));
                                                                                    auth_form_message.set(None);
                                                                                }
                                                                            />
                                                                            {move || if auth_user.get().is_some() {
                                                                                view! {
                                                                                    <button class="button secondary" on:click=bootstrap_current_user_as_admin>
                                                                                        "将当前账号初始化为管理员"
                                                                                    </button>
                                                                                }.into_any()
                                                                            } else {
                                                                                view! {
                                                                                    <p class="status compact-help">"首次管理员注册时填写；已有管理员后此入口会自动隐藏。"</p>
                                                                                }.into_any()
                                                                            }}
                                                                        </div>
                                                                    }.into_any()
                                                                } else {
                                                                    ().into_any()
                                                                }}
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    {move || auth_form_message.get().map(|message| view! {
                                                        <p class="form-error">{message}</p>
                                                    })}
                                                    {move || if auth_mode.get() == "register" {
                                                        view! {
                                                            <p class="status compact-help">"注册密码需至少 10 位，并包含大写、小写、数字和符号。普通注册账号需管理员审批后才能同步。"</p>
                                                        }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    {move || if auth_user.get().is_some() {
                                                        view! {
                                                            <div class="sync-key-settings">
                                                                <label class="sync-key-toggle-row">
                                                                    <input
                                                                        type="checkbox"
                                                                        prop:checked=move || sync_api_keys_enabled.get()
                                                                        on:change=toggle_api_key_sync
                                                                    />
                                                                    <span>"同步 API Key（客户端加密）"</span>
                                                                    <span class="tag sync-key-state">
                                                                        {move || if !sync_api_keys_enabled.get() {
                                                                            "已关闭"
                                                                        } else if sync_secret.get().is_empty() {
                                                                            "未解锁"
                                                                        } else {
                                                                            "可信设备已解锁"
                                                                        }}
                                                                    </span>
                                                                </label>
                                                                <p class="status compact-help">
                                                                    "服务器只保存密文；代理生成时 Key 仍会在后端内存中瞬时转发。"
                                                                </p>
                                                                {move || if sync_api_keys_enabled.get() && sync_secret.get().is_empty() {
                                                                    view! {
                                                                        <div class="row sync-key-unlock-row">
                                                                            <input
                                                                                class="text-input"
                                                                                type="password"
                                                                                autocomplete="current-password"
                                                                                placeholder="输入账号密码解锁当前设备"
                                                                                prop:value=move || sync_unlock_password.get()
                                                                                on:input=move |event| sync_unlock_password.set(event_target_value(&event))
                                                                            />
                                                                            <button
                                                                                class="button secondary"
                                                                                on:click=unlock_api_key_sync
                                                                                disabled=move || sync_unlocking.get()
                                                                            >
                                                                                {move || if sync_unlocking.get() { "解锁中…" } else { "解锁" }}
                                                                            </button>
                                                                        </div>
                                                                    }.into_any()
                                                                } else {
                                                                    ().into_any()
                                                                }}
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        ().into_any()
                                                    }}
                                                    <div class="row auth-action-row">
                                                        <button
                                                            class="button"
                                                            on:click=move |_| {
                                                                if auth_mode.get_untracked() == "register" {
                                                                    submit_auth("register");
                                                                } else {
                                                                    submit_auth("login");
                                                                }
                                                            }
                                                        >
                                                            {move || if auth_mode.get() == "register" { "创建账号" } else { "登录" }}
                                                        </button>
                                                        <div class="sync-action-control">
                                                            <button
                                                                class="button ghost"
                                                                on:click=move |_| sync_action()
                                                                disabled=move || syncing.get()
                                                                    || auth_user.get().map(|user| user.status != "approved").unwrap_or(true)
                                                            >
                                                                {move || if syncing.get() { "同步中…" } else { "立即同步" }}
                                                            </button>
                                                            {move || sync_status_text.get().map(|message| view! {
                                                                <p class="form-hint sync-status-message">{message}</p>
                                                            })}
                                                        </div>
                                                    </div>
                                                </div>
                                            </section>
                                        }.into_any(),
                                        "password" => view! {
                                            <section class="stack">
                                                <h2>"密码更改"</h2>
                                                {move || if auth_user.get().is_some() {
                                                    view! {
                                                        <div class="account-security-card">
                                                            <input
                                                                class="text-input"
                                                                type="password"
                                                                placeholder="当前密码"
                                                                prop:value=move || change_old_password.get()
                                                                on:input=move |ev| {
                                                                    change_old_password.set(event_target_value(&ev));
                                                                    password_form_message.set(None);
                                                                }
                                                            />
                                                            <input
                                                                class="text-input"
                                                                type="password"
                                                                placeholder="新密码"
                                                                prop:value=move || change_new_password.get()
                                                                on:input=move |ev| {
                                                                    change_new_password.set(event_target_value(&ev));
                                                                    password_form_message.set(None);
                                                                }
                                                            />
                                                            <input
                                                                class="text-input"
                                                                type="password"
                                                                placeholder="再次输入新密码"
                                                                prop:value=move || change_new_password_confirm.get()
                                                                on:input=move |ev| {
                                                                    change_new_password_confirm.set(event_target_value(&ev));
                                                                    password_form_message.set(None);
                                                                }
                                                            />
                                                            {move || password_form_message.get().map(|message| view! {
                                                                <p class="form-error">{message}</p>
                                                            })}
                                                            <p class="status compact-help">"新密码至少 10 位，并包含大写、小写、数字和符号。"</p>
                                                            <button class="button secondary" on:click=change_password>"更新密码"</button>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <div class="admin-empty">"登录后才能修改密码。"</div>
                                                    }.into_any()
                                                }}
                                            </section>
                                        }.into_any(),
                                        "admin" => view! {
                                            <section class="stack admin-panel">
                                                <div class="row admin-panel-header">
                                                    <h2>"用户管理"</h2>
                                                    <button class="button ghost" on:click=refresh_admin_users disabled=move || loading_admin_users.get()>
                                                        {move || if loading_admin_users.get() { "刷新中…" } else { "刷新列表" }}
                                                    </button>
                                                </div>
                                                <div class="admin-user-list">
                                                    {move || {
                                                        let rows = admin_users.get();
                                                        if rows.is_empty() {
                                                            return vec![view! {
                                                                <div class="admin-empty">
                                                                    "还没有加载用户列表。点击“刷新列表”查看注册申请。"
                                                                </div>
                                                            }.into_any()];
                                                        }
                                                        rows.into_iter().map(|user| {
                                                            let approve_id = user.id.clone();
                                                            let disable_id = user.id.clone();
                                                            let restore_id = user.id.clone();
                                                            let delete_id = user.id.clone();
                                                            let can_delete = user.role != "admin";
                                                            view! {
                                                                <article class="admin-user-row">
                                                                    <div class="admin-user-main">
                                                                        <strong>{user.username}</strong>
                                                                        <span class="muted">{format!("{} · {}", user.role, user.status)}</span>
                                                                    </div>
                                                                    <span class="tag">{format!("服务器图片 {} 张", user.image_count)}</span>
                                                                    <span class="muted admin-user-date">{format!("注册 {}", user.created_at)}</span>
                                                                    <div class="row admin-user-actions">
                                                                        {if user.status == "pending" {
                                                                            view! {
                                                                                <button class="button secondary" on:click=move |_| admin_user_action("/api/admin/users/approve", approve_id.clone())>
                                                                                    "批准"
                                                                                </button>
                                                                            }.into_any()
                                                                        } else if user.status == "disabled" {
                                                                            view! {
                                                                                <button class="button secondary" on:click=move |_| admin_user_action("/api/admin/users/restore", restore_id.clone())>
                                                                                    "恢复"
                                                                                </button>
                                                                            }.into_any()
                                                                        } else {
                                                                            view! {
                                                                                <button class="button ghost danger" on:click=move |_| admin_user_action("/api/admin/users/disable", disable_id.clone())>
                                                                                    "禁用"
                                                                                </button>
                                                                            }.into_any()
                                                                        }}
                                                                        {if can_delete {
                                                                            view! {
                                                                                <button
                                                                                    class="button ghost danger"
                                                                                    on:click=move |event: MouseEvent| delete_managed_user(
                                                                                        delete_id.clone(),
                                                                                        event.client_x() as f64,
                                                                                        event.client_y() as f64,
                                                                                    )
                                                                                >
                                                                                    "删除"
                                                                                </button>
                                                                            }.into_any()
                                                                        } else {
                                                                            ().into_any()
                                                                        }}
                                                                    </div>
                                                                </article>
                                                            }.into_any()
                                                        }).collect::<Vec<_>>()
                                                    }}
                                                </div>
                                            </section>
                                        }.into_any(),
                                        "data" => view! {
                                            <section class="stack data-management-panel">
                                                <div class="row data-management-header">
                                                    <div>
                                                        <h2>"数据管理"</h2>
                                                        <p class="status compact-help">"本地与云端完全分开管理，任何一侧的清除都不会自动影响另一侧。"</p>
                                                    </div>
                                                    <span class="tag">"版本化 ZIP 备份"</span>
                                                </div>
                                                <div class="auth-mode-tabs data-scope-tabs">
                                                    <button
                                                        class="auth-mode-button"
                                                        class:is-active=move || data_management_tab.get() == "local"
                                                        on:click=move |_| data_management_tab.set("local".into())
                                                    >
                                                        "本地数据"
                                                    </button>
                                                    <button
                                                        class="auth-mode-button"
                                                        class:is-active=move || data_management_tab.get() == "cloud"
                                                        on:click=move |_| {
                                                            data_management_tab.set("cloud".into());
                                                            refresh_cloud_data_stats();
                                                        }
                                                    >
                                                        "云端数据"
                                                    </button>
                                                </div>
                                                {move || if data_management_tab.get() == "local" {
                                                    let task_count = tasks.with(|items| items.len());
                                                    let thread_count = threads.with(|items| items.len());
                                                    let asset_count = assets.with(|items| items.len());
                                                    view! {
                                                        <div class="stack data-scope-content">
                                                            <div class="data-stat-grid">
                                                                <div class="data-stat-card"><span>"会话"</span><strong>{thread_count}</strong></div>
                                                                <div class="data-stat-card"><span>"生成任务"</span><strong>{task_count}</strong></div>
                                                                <div class="data-stat-card"><span>"本地图片"</span><strong>{asset_count}</strong></div>
                                                            </div>
                                                            <article class="data-action-card">
                                                                <div>
                                                                    <h3>"备份与恢复"</h3>
                                                                    <p class="status compact-help">"导出会包含工作区和图片原文件，但不会导出明文 API Key。导入默认按 ID、更新时间和 SHA-256 合并。"</p>
                                                                </div>
                                                                <div class="row data-action-buttons">
                                                                    <button class="button secondary" on:click=export_local_backup disabled=move || data_management_busy.get() || generating.get()>
                                                                        <MaterialSymbolIcon name="download" filled=false />
                                                                        "导出 ZIP"
                                                                    </button>
                                                                    <button class="button ghost" on:click=move |_| {
                                                                        if let Some(input) = backup_file_input.get() {
                                                                            input.click();
                                                                        }
                                                                    } disabled=move || data_management_busy.get() || generating.get()>
                                                                        <MaterialSymbolIcon name="upload" filled=false />
                                                                        "合并导入"
                                                                    </button>
                                                                    <input
                                                                        node_ref=backup_file_input
                                                                        class="visually-hidden-file-input"
                                                                        type="file"
                                                                        accept=".zip,application/zip"
                                                                        on:change=import_local_backup
                                                                    />
                                                                </div>
                                                            </article>
                                                            <article class="data-action-card session-export-card">
                                                                <div>
                                                                    <h3>"单会话项目包"</h3>
                                                                    <p class="status compact-help">"只导出所选会话、任务、结果图和参考图；不包含服务商配置、密钥或收藏状态。导入后会创建独立副本。"</p>
                                                                </div>
                                                                <div class="session-export-controls">
                                                                    <select
                                                                        class="select-input session-export-select"
                                                                        prop:value=move || {
                                                                            let selected = session_export_thread_id.get();
                                                                            if threads.with(|items| items.iter().any(|thread| thread.id == selected)) {
                                                                                selected
                                                                            } else {
                                                                                current_thread_id.get()
                                                                            }
                                                                        }
                                                                        on:change=move |ev| session_export_thread_id.set(event_target_value(&ev))
                                                                    >
                                                                        <For
                                                                            each=move || {
                                                                                let mut items = threads.get();
                                                                                items.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                                                                                items
                                                                            }
                                                                            key=|thread| thread.id.clone()
                                                                            children=move |thread| view! {
                                                                                <option value=thread.id.clone()>{thread_display_name(&thread)}</option>
                                                                            }
                                                                        />
                                                                    </select>
                                                                    <button
                                                                        class="button secondary"
                                                                        disabled=move || data_management_busy.get() || generating.get() || threads.get().is_empty()
                                                                        on:click=move |_| {
                                                                            let selected = session_export_thread_id.get_untracked();
                                                                            let thread_id = if threads.with_untracked(|items| items.iter().any(|thread| thread.id == selected)) {
                                                                                selected
                                                                            } else {
                                                                                current_thread_id.get_untracked()
                                                                            };
                                                                            export_session_backup(thread_id);
                                                                        }
                                                                    >
                                                                        <MaterialSymbolIcon name="download" filled=false />
                                                                        "导出所选会话"
                                                                    </button>
                                                                </div>
                                                            </article>
                                                            <article class="data-action-card danger-zone">
                                                                <div>
                                                                    <h3>"清除本地数据"</h3>
                                                                    <p class="status compact-help">"仅清除当前浏览器中的数据；服务器备份和账号不会受影响。"</p>
                                                                </div>
                                                                <div class="row data-action-buttons">
                                                                    <button class="button ghost danger" on:click=move |event| confirm_local_clear(LocalDataClearScope::Workspace, event)>"历史与图片"</button>
                                                                    <button class="button ghost danger" on:click=move |event| confirm_local_clear(LocalDataClearScope::Configs, event)>"服务商配置"</button>
                                                                    <button class="button ghost danger" on:click=move |event| confirm_local_clear(LocalDataClearScope::Preferences, event)>"界面偏好"</button>
                                                                    <button class="button danger" on:click=move |event| confirm_local_clear(LocalDataClearScope::All, event)>"全部本地数据"</button>
                                                                </div>
                                                            </article>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    let approved = auth_user.get().map(|user| user.status == "approved").unwrap_or(false);
                                                    if !approved {
                                                        view! {
                                                            <div class="admin-empty data-cloud-locked">
                                                                <MaterialSymbolIcon name="cloud_off" filled=false />
                                                                <strong>"云端数据暂不可用"</strong>
                                                                <span>"登录且账号审批通过后，可查看和清除自己的云端同步数据。游客本地数据不会上传。"</span>
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        let stats = cloud_data_stats.get().unwrap_or_default();
                                                        view! {
                                                            <div class="stack data-scope-content">
                                                                <div class="row cloud-stat-toolbar">
                                                                    <div class="data-stat-grid cloud-stat-grid">
                                                                        <div class="data-stat-card"><span>"云端图片"</span><strong>{stats.image_count}</strong></div>
                                                                        <div class="data-stat-card"><span>"占用空间"</span><strong>{format_byte_size(stats.image_bytes)}</strong></div>
                                                                        <div class="data-stat-card"><span>"服务商模板"</span><strong>{stats.provider_template_count}</strong></div>
                                                                        <div class="data-stat-card"><span>"同步快照"</span><strong>{if stats.has_sync_snapshot { "有" } else { "无" }}</strong></div>
                                                                    </div>
                                                                    <button class="button ghost icon-button" title="刷新云端统计" on:click=move |_| refresh_cloud_data_stats() disabled=move || data_management_busy.get()>
                                                                        <MaterialSymbolIcon name="refresh" filled=false />
                                                                    </button>
                                                                </div>
                                                                <article class="data-action-card danger-zone">
                                                                    <div>
                                                                        <h3>"清除云端数据"</h3>
                                                                        <p class="status compact-help">"只删除当前账号的服务器数据；当前浏览器工作区和账号本身都会保留。"</p>
                                                                    </div>
                                                                    <div class="row data-action-buttons">
                                                                        <button class="button ghost danger" on:click=move |event| confirm_cloud_clear(CloudDataClearScope::SyncData, event)>"同步数据与图片"</button>
                                                                        <button class="button ghost danger" on:click=move |event| confirm_cloud_clear(CloudDataClearScope::ProviderTemplates, event)>"服务商模板"</button>
                                                                        <button class="button danger" on:click=move |event| confirm_cloud_clear(CloudDataClearScope::All, event)>"全部云端数据"</button>
                                                                    </div>
                                                                </article>
                                                            </div>
                                                        }.into_any()
                                                    }
                                                }}
                                                {move || data_management_message.get().map(|message| view! {
                                                    <p class="data-management-message">{message}</p>
                                                })}
                                            </section>
                                        }.into_any(),
                                        "about" => view! {
                                            <section class="stack">
                                                <div class="settings-about-card">
                                                    <img class="settings-about-logo" src="/favicon/MewImage04.svg" alt="MewImage" />
                                                    <div class="stack">
                                                        <div class="row settings-about-title">
                                                            <h2>"关于 MewImage"</h2>
                                                            <span class="tag settings-version-tag">"v1.0.5"</span>
                                                            <a
                                                                class="button ghost icon-button settings-github-button"
                                                                href="https://github.com/Rabbit-bot-No-002/MewImage"
                                                                target="_blank"
                                                                rel="noopener noreferrer"
                                                                aria-label="打开 MewImage GitHub 项目"
                                                                title="Rabbit-bot-No-002/MewImage"
                                                            >
                                                                <GitHubIcon />
                                                            </a>
                                                        </div>
                                                        <p class="status">"一个本地优先、登录后手动同步的可爱图片生成工作台。游客数据默认留在浏览器，服务器只承担代理、账号和可选同步职责。"</p>
                                                        <span class="tag">"Rust · Leptos · Axum · SQLite"</span>
                                                    </div>
                                                </div>
                                            </section>
                                        }.into_any(),
                                        _ => view! {
                                            <section class="stack">
                                                <div class="row">
                                                    <h2>"服务商配置"</h2>
                                                    <div class="row">
                                                        <button class="button ghost" on:click=add_config>"新增配置"</button>
                                                        <button class="button ghost danger" on:click=delete_config>"删除配置"</button>
                                                    </div>
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
                                                    current_config_snapshot=current_config
                                                    templates=templates
                                                    save_configs_only=move || persist_ui_state()
                                                />
                                            </section>
                                        }.into_any(),
                                    }}
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

    }
}
