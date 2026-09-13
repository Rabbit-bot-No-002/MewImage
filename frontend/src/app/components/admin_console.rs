use std::collections::HashSet;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::{ev, leptos_dom::helpers::window_event_listener, prelude::*, task::spawn_local};
use mew_image_shared::{
    AccountKind, AdminAuditResponse, AdminBatchResponse, AdminUserActionRequest,
    AdminUserBatchAction, AdminUserBatchRequest, AdminUserExportRequest, AdminUserSummary,
    AdminUsersResponse, ManagedPasswordResetResponse,
};
use wasm_bindgen::{JsCast, JsValue};

use crate::{
    api::api_url,
    app::state::{AccountState, MainView, UiState},
};

use super::{
    common::MaterialSymbolIcon,
    managed_accounts::{ManagedAccountsAdmin, ManagedProviderTemplatesAdmin},
};

fn navigate_admin(path: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_hash(path);
    }
}

fn minute_time(value: &str) -> String {
    let date = js_sys::Date::new(&JsValue::from_str(value));
    if date.get_time().is_nan() {
        return value.replace('T', " ").chars().take(16).collect();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes()
    )
}

fn csv_download_name() -> String {
    let now = js_sys::Date::new_0();
    format!(
        "mew-users-{:04}{:02}{:02}-{:02}{:02}.csv",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date(),
        now.get_hours(),
        now.get_minutes()
    )
}

#[component]
pub(crate) fn AdminConsole() -> impl IntoView {
    let account = expect_context::<AccountState>();
    let ui = expect_context::<UiState>();
    Effect::new(move |_| {
        if ui.main_view.get() == MainView::Admin
            && account.auth_checked.get()
            && account
                .auth_user
                .get()
                .is_none_or(|user| user.role != "admin")
        {
            ui.main_view.set(MainView::Workspace);
            if let Some(window) = web_sys::window()
                && let Ok(history) = window.history()
            {
                let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"));
            }
        }
    });

    view! {
        <Show when=move || account.auth_user.get().is_some_and(|user| user.role == "admin")>
            <main class="admin-console panel">
                <aside class="admin-console-nav">
                    <button class:is-active=move || ui.admin_section.get() == "users" on:click=move |_| navigate_admin("/admin/users")>
                        <MaterialSymbolIcon name="group" filled=false /><span>"用户"</span>
                    </button>
                    <button class:is-active=move || ui.admin_section.get() == "managed" on:click=move |_| navigate_admin("/admin/managed")>
                        <MaterialSymbolIcon name="manage_accounts" filled=false /><span>"托管账号"</span>
                    </button>
                    <button class:is-active=move || ui.admin_section.get() == "providers" on:click=move |_| navigate_admin("/admin/providers")>
                        <MaterialSymbolIcon name="dns" filled=false /><span>"服务商模板"</span>
                    </button>
                    <button class:is-active=move || ui.admin_section.get() == "audit" on:click=move |_| navigate_admin("/admin/audit")>
                        <MaterialSymbolIcon name="history" filled=false /><span>"审计日志"</span>
                    </button>
                </aside>
                <section class="admin-console-content">
                    <Show when=move || ui.admin_section.get() == "users"><AdminUsersPage /></Show>
                    <Show when=move || ui.admin_section.get() == "managed"><ManagedAccountsPage /></Show>
                    <Show when=move || ui.admin_section.get() == "providers"><ManagedProvidersPage /></Show>
                    <Show when=move || ui.admin_section.get() == "audit"><AdminAuditPage /></Show>
                </section>
            </main>
        </Show>
    }
}

#[component]
fn ManagedAccountsPage() -> impl IntoView {
    view! {
        <div class="admin-page stack">
            <header><h2>"托管账号"</h2></header>
            <ManagedAccountsAdmin />
        </div>
    }
}

#[component]
fn ManagedProvidersPage() -> impl IntoView {
    view! {
        <div class="admin-page stack">
            <header><h2>"服务商模板"</h2></header>
            <ManagedProviderTemplatesAdmin />
        </div>
    }
}

#[component]
fn AdminUsersPage() -> impl IntoView {
    let account = expect_context::<AccountState>();
    let query = RwSignal::new(String::new());
    let status = RwSignal::new("all".to_string());
    let role = RwSignal::new("all".to_string());
    let sort = RwSignal::new("created_at".to_string());
    let order = RwSignal::new("desc".to_string());
    let page = RwSignal::new(1usize);
    let limit = RwSignal::new(20usize);
    let total = RwSignal::new(0usize);
    let selected = RwSignal::new(HashSet::<String>::new());
    let request_sequence = RwSignal::new(0u64);
    let reload_nonce = RwSignal::new(0u64);
    let error = RwSignal::new(None::<String>);
    let delete_ids = RwSignal::new(Vec::<String>::new());
    let confirmation = RwSignal::new(String::new());
    let reset_password_user = RwSignal::new(None::<AdminUserSummary>);
    let temporary_password = RwSignal::new(None::<String>);

    let escape_listener = window_event_listener(ev::keydown, move |event| {
        if event.key() != "Escape" {
            return;
        }
        // 每次只关闭当前最上层弹窗，避免一次按键连续穿透多个确认层。
        let handled = if temporary_password.get_untracked().is_some() {
            temporary_password.set(None);
            true
        } else if reset_password_user.get_untracked().is_some() {
            reset_password_user.set(None);
            true
        } else if !delete_ids.get_untracked().is_empty() {
            delete_ids.set(Vec::new());
            confirmation.set(String::new());
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
        let _ = reload_nonce.get();
        let query_value = query.get();
        let status_value = status.get();
        let role_value = role.get();
        let sort_value = sort.get();
        let order_value = order.get();
        let page_value = page.get();
        let limit_value = limit.get();
        request_sequence.update(|value| *value += 1);
        let sequence = request_sequence.get_untracked();
        selected.set(HashSet::new());
        spawn_local(async move {
            TimeoutFuture::new(300).await;
            if request_sequence.get_untracked() != sequence {
                return;
            }
            account.loading_admin_users.set(true);
            let encoded = js_sys::encode_uri_component(query_value.trim())
                .as_string()
                .unwrap_or_default();
            let url = format!(
                "/api/admin/users?page={page_value}&limit={limit_value}&q={encoded}&status={status_value}&role={role_value}&account_kind=all&sort={sort_value}&order={order_value}"
            );
            match Request::get(&api_url(&url))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
            {
                Ok(response) if response.ok() => {
                    match response.json::<AdminUsersResponse>().await {
                        Ok(payload) if request_sequence.get_untracked() == sequence => {
                            total.set(payload.total);
                            account.admin_users.set(payload.users);
                            error.set(None);
                        }
                        Ok(_) => {}
                        Err(parse_error) => {
                            error.set(Some(format!("用户列表解析失败：{parse_error}")))
                        }
                    }
                }
                Ok(response) => error.set(Some(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "用户列表加载失败。".into()),
                )),
                Err(fetch_error) => error.set(Some(format!("用户列表加载失败：{fetch_error}"))),
            }
            account.loading_admin_users.set(false);
        });
    });

    let run_batch =
        move |action: AdminUserBatchAction, ids: Vec<String>, confirm: Option<String>| {
            let request = AdminUserBatchRequest {
                action,
                ids,
                confirmation: confirm,
            };
            spawn_local(async move {
                let Ok(builder) = Request::post(&api_url("/api/admin/users/batch"))
                    .credentials(web_sys::RequestCredentials::Include)
                    .json(&request)
                else {
                    error.set(Some("批量操作请求序列化失败。".into()));
                    return;
                };
                match builder.send().await {
                    Ok(response) if response.ok() => {
                        match response.json::<AdminBatchResponse>().await {
                            Ok(result) => {
                                error.set(Some(if result.failed.is_empty() {
                                    format!("已处理 {} 个用户。", result.updated_count)
                                } else {
                                    format!(
                                        "成功 {} 个，失败 {} 个。",
                                        result.updated_count,
                                        result.failed.len()
                                    )
                                }));
                                reload_nonce.update(|value| *value += 1);
                            }
                            Err(parse_error) => {
                                error.set(Some(format!("操作响应解析失败：{parse_error}")))
                            }
                        }
                    }
                    Ok(response) => error.set(Some(
                        response
                            .text()
                            .await
                            .unwrap_or_else(|_| "用户操作失败。".into()),
                    )),
                    Err(send_error) => error.set(Some(format!("用户操作失败：{send_error}"))),
                }
            });
        };

    let export_csv = move |_| {
        let ids = selected.get_untracked().into_iter().collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        spawn_local(async move {
            let Ok(builder) = Request::post(&api_url("/api/admin/users/export"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&AdminUserExportRequest { ids })
            else {
                return;
            };
            let Ok(response) = builder.send().await else {
                return;
            };
            if !response.ok() {
                error.set(Some(response.text().await.unwrap_or_default()));
                return;
            }
            let Ok(bytes) = response.binary().await else {
                return;
            };
            let array = js_sys::Uint8Array::from(bytes.as_slice());
            let parts = js_sys::Array::new();
            parts.push(&array);
            if let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts)
                && let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob)
                && let Some(document) = web_sys::window().and_then(|window| window.document())
                && let Ok(element) = document.create_element("a")
                && let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>()
            {
                anchor.set_href(&url);
                anchor.set_download(&csv_download_name());
                anchor.click();
                let _ = web_sys::Url::revoke_object_url(&url);
            }
        });
    };

    let reset_password = move |user_id: String| {
        let request = AdminUserActionRequest {
            user_id: user_id.clone(),
        };
        spawn_local(async move {
            let Ok(builder) = Request::post(&api_url("/api/admin/users/reset-password"))
                .credentials(web_sys::RequestCredentials::Include)
                .json(&request)
            else {
                error.set(Some("重置密码请求序列化失败。".into()));
                return;
            };
            match builder.send().await {
                Ok(response) if response.ok() => {
                    match response.json::<ManagedPasswordResetResponse>().await {
                        Ok(payload) => {
                            account.admin_users.update(|users| {
                                if let Some(target) =
                                    users.iter_mut().find(|item| item.id == payload.user_id)
                                {
                                    target.must_change_password = true;
                                }
                            });
                            temporary_password.set(Some(payload.temporary_password));
                        }
                        Err(parse_error) => {
                            error.set(Some(format!("重置密码响应解析失败：{parse_error}")))
                        }
                    }
                }
                Ok(response) => error.set(Some(
                    response
                        .text()
                        .await
                        .unwrap_or_else(|_| "重置临时密码失败。".into()),
                )),
                Err(send_error) => error.set(Some(format!("重置临时密码失败：{send_error}"))),
            }
        });
    };

    view! {
        <div class="admin-page stack">
            <header><h2>"用户"</h2><span class="tag">{move || format!("共 {} 人", total.get())}</span></header>
            <div class="admin-user-toolbar">
                <input class="text-input" placeholder="搜索用户名" prop:value=move || query.get() on:input=move |event| { query.set(event_target_value(&event)); page.set(1); } />
                <select class="select-input" on:change=move |event| { status.set(event_target_value(&event)); page.set(1); }><option value="all">"全部状态"</option><option value="pending">"待审批"</option><option value="approved">"已启用"</option><option value="disabled">"已禁用"</option></select>
                <select class="select-input" on:change=move |event| { role.set(event_target_value(&event)); page.set(1); }><option value="all">"全部角色"</option><option value="user">"用户"</option><option value="admin">"管理员"</option></select>
                <select class="select-input" on:change=move |event| { sort.set(event_target_value(&event)); page.set(1); }><option value="created_at">"注册时间"</option><option value="image_count">"图片数"</option></select>
                <button class="button ghost icon-button" title="切换排序方向" on:click=move |_| order.update(|value| *value=if value=="desc" {"asc".into()} else {"desc".into()})><MaterialSymbolIcon name=if order.get()=="desc" {"arrow_downward"} else {"arrow_upward"} filled=false /></button>
                <select class="select-input" on:change=move |event| { limit.set(event_target_value(&event).parse().unwrap_or(20)); page.set(1); }><option value="20">"20 / 页"</option><option value="50">"50 / 页"</option><option value="100">"100 / 页"</option></select>
            </div>
            <Show when=move || !selected.get().is_empty()>
                <div class="admin-batch-toolbar">
                    <strong>{move || format!("选中 {} 项", selected.get().len())}</strong>
                    <button class="button secondary" on:click=move |_| run_batch(AdminUserBatchAction::Approve, selected.get_untracked().into_iter().collect(), None)>"批量批准"</button>
                    <button class="button ghost" on:click=move |_| run_batch(AdminUserBatchAction::Disable, selected.get_untracked().into_iter().collect(), None)>"批量禁用"</button>
                    <button class="button ghost" on:click=move |_| run_batch(AdminUserBatchAction::Restore, selected.get_untracked().into_iter().collect(), None)>"批量恢复"</button>
                    <button class="button ghost" on:click=export_csv>"导出 CSV"</button>
                    <button class="button danger" on:click=move |_| { delete_ids.set(selected.get_untracked().into_iter().collect()); confirmation.set(String::new()); }>"批量删除"</button>
                </div>
            </Show>
            <div class="admin-table-scroll">
                <table class="admin-table">
                    <thead><tr>
                        <th><input type="checkbox" aria-label="全选本页"
                            prop:checked=move || {
                                let selectable_count = account.admin_users.get().into_iter().filter(|user| user.role != "admin").count();
                                selectable_count > 0 && selected.get().len() == selectable_count
                            }
                            on:change=move |_| {
                                let rows = account.admin_users.get().into_iter().filter(|user| user.role != "admin").collect::<Vec<_>>();
                                if selected.get_untracked().len() == rows.len() {
                                    selected.set(HashSet::new());
                                } else {
                                    selected.set(rows.into_iter().map(|user| user.id).collect());
                                }
                            }
                        /></th>
                        <th>"用户名"</th><th>"角色"</th><th>"状态"</th><th>"托管账号"</th>
                        <th>"服务器图片数"</th><th>"注册时间"</th><th>"最后活跃"</th><th>"操作"</th>
                    </tr></thead>
                    <tbody>
                        <For each=move || account.admin_users.get() key=|user| format!("{}:{}:{}", user.id, user.status, user.must_change_password) children=move |user| {
                            let select_id = user.id.clone();
                            let checked_id = user.id.clone();
                            let managed_id = user.id.clone();
                            let action_id = user.id.clone();
                            let delete_id = user.id.clone();
                            let password_user = user.clone();
                            let status_action = match user.status.as_str() {
                                "pending" => AdminUserBatchAction::Approve,
                                "disabled" => AdminUserBatchAction::Restore,
                                _ => AdminUserBatchAction::Disable,
                            };
                            let status_label = match user.status.as_str() {
                                "pending" => "待审批",
                                "disabled" => "已禁用",
                                _ => "已启用",
                            };
                            let (status_action_title, status_action_icon) = match user.status.as_str() {
                                "pending" => ("批准账号", "check_circle"),
                                "disabled" => ("恢复账号", "restore"),
                                _ => ("禁用账号", "block"),
                            };
                            let is_admin = user.role == "admin";
                            view! {
                                <tr>
                                    <td><input type="checkbox"
                                        disabled=is_admin
                                        prop:checked=move || selected.get().contains(&checked_id)
                                        on:change=move |_| selected.update(|ids| {
                                            if !ids.insert(select_id.clone()) { ids.remove(&select_id); }
                                        })
                                    /></td>
                                    <td><strong>{user.username}</strong></td>
                                    <td>{user.role}</td>
                                    <td><span class="tag">{status_label}</span></td>
                                    <td>{if user.account_kind == AccountKind::Managed {
                                        view! { <button class="admin-managed-link" on:click=move |_| navigate_admin(&format!("/admin/managed?user={managed_id}"))>{format!("托管 · {}", user.managed_provider_count)}</button> }.into_any()
                                    } else { view! { <span>"—"</span> }.into_any() }}</td>
                                    <td>{user.image_count}</td>
                                    <td>{minute_time(&user.created_at)}</td>
                                    <td>{user.last_active_at.as_deref().map(minute_time).unwrap_or_else(|| "从未".into())}</td>
                                    <td><div class="admin-row-actions">
                                        {if !is_admin { view! { <button class="button ghost icon-button" title=status_action_title on:click=move |_| run_batch(status_action, vec![action_id.clone()], None)><MaterialSymbolIcon name=status_action_icon filled=false /></button> }.into_any() } else { ().into_any() }}
                                        <button class="button ghost icon-button" title="重置临时密码" on:click=move |_| reset_password_user.set(Some(password_user.clone()))><MaterialSymbolIcon name="lock_reset" filled=false /></button>
                                        {if !is_admin { view! { <button class="button ghost danger icon-button" title="删除用户" on:click=move |_| { delete_ids.set(vec![delete_id.clone()]); confirmation.set(String::new()); }><MaterialSymbolIcon name="delete" filled=false /></button> }.into_any() } else { ().into_any() }}
                                    </div></td>
                                </tr>
                            }
                        } />
                    </tbody>
                </table>
            </div>
            <div class="admin-pagination">
                <button
                    class="button ghost"
                    disabled=move || { page.get() <= 1 }
                    on:click=move |_| page.update(|value| *value = value.saturating_sub(1))
                >"上一页"</button>
                <span>{move || format!("第 {} / {} 页", page.get(), total.get().div_ceil(limit.get()).max(1))}</span>
                <button
                    class="button ghost"
                    disabled=move || { page.get() >= total.get().div_ceil(limit.get()).max(1) }
                    on:click=move |_| page.update(|value| *value += 1)
                >"下一页"</button>
            </div>
            {move || error.get().map(|message| view!{<p class="form-hint">{message}</p>})}
            <Show when=move || !delete_ids.get().is_empty()>{move || {let ids=delete_ids.get();let expected=if ids.len()==1 {account.admin_users.get().into_iter().find(|user|user.id==ids[0]).map(|user|user.username).unwrap_or_default()}else{format!("删除 {} 个用户",ids.len())};view!{<div class="managed-form-backdrop"><section class="admin-delete-confirm stack"><h3>"确认删除用户"</h3><p>"删除账号、服务器图片和同步数据后无法恢复。"</p><label>{format!("请输入：{expected}")}<input class="text-input" prop:value=move || confirmation.get() on:input=move |event|confirmation.set(event_target_value(&event)) /></label><div class="row"><button class="button ghost" on:click=move |_|delete_ids.set(Vec::new())>"取消"</button><button class="button danger" disabled=move ||confirmation.get()!=expected on:click=move |_|{let ids=delete_ids.get_untracked();let text=confirmation.get_untracked();delete_ids.set(Vec::new());run_batch(AdminUserBatchAction::Delete,ids,Some(text));}>"确认删除"</button></div></section></div>}}
            }</Show>
            <Show when=move || reset_password_user.get().is_some()>
                {move || reset_password_user.get().map(|user| {
                    let user_id = user.id.clone();
                    view! {
                        <div class="managed-form-backdrop">
                            <section class="admin-delete-confirm stack" role="dialog" aria-modal="true">
                                <h3>"重置临时密码"</h3>
                                <p>{format!("确认重置用户“{}”的密码？该用户现有会话会立即失效，下次登录必须修改密码。", user.username)}</p>
                                <div class="row">
                                    <button class="button ghost" on:click=move |_| reset_password_user.set(None)>"取消"</button>
                                    <button class="button danger" on:click=move |_| {
                                        reset_password_user.set(None);
                                        reset_password(user_id.clone());
                                    }>"确认重置"</button>
                                </div>
                            </section>
                        </div>
                    }
                })}
            </Show>
            <Show when=move || temporary_password.get().is_some()>
                {move || temporary_password.get().map(|password| {
                    let copied = password.clone();
                    view! {
                        <div class="managed-form-backdrop">
                            <section class="managed-password-result stack" role="dialog" aria-modal="true">
                                <h3>"一次性临时密码"</h3>
                                <p class="status">"密码只在这里显示一次，请立即安全保存。"</p>
                                <code>{password}</code>
                                <div class="row">
                                    <button class="button secondary" on:click=move |_| {
                                        if let Some(clipboard) = web_sys::window().map(|window| window.navigator().clipboard()) {
                                            let _ = clipboard.write_text(&copied);
                                        }
                                    }>"复制密码"</button>
                                    <button class="button primary" on:click=move |_| temporary_password.set(None)>"我已保存"</button>
                                </div>
                            </section>
                        </div>
                    }
                })}
            </Show>
        </div>
    }
}

#[component]
fn AdminAuditPage() -> impl IntoView {
    let entries = RwSignal::new(Vec::new());
    let total = RwSignal::new(0usize);
    let page = RwSignal::new(1usize);
    let query = RwSignal::new(String::new());
    let action = RwSignal::new("all".to_string());
    let sequence = RwSignal::new(0u64);
    Effect::new(move |_| {
        let page_value = page.get();
        let q = query.get();
        let action_value = action.get();
        sequence.update(|value| *value += 1);
        let request_id = sequence.get_untracked();
        spawn_local(async move {
            TimeoutFuture::new(300).await;
            if sequence.get_untracked() != request_id {
                return;
            }
            let encoded = js_sys::encode_uri_component(q.trim())
                .as_string()
                .unwrap_or_default();
            let url = format!(
                "/api/admin/audit?page={page_value}&limit=20&q={encoded}&action={action_value}"
            );
            if let Ok(response) = Request::get(&api_url(&url))
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await
                && response.ok()
                && let Ok(payload) = response.json::<AdminAuditResponse>().await
            {
                entries.set(payload.entries);
                total.set(payload.total);
            }
        });
    });
    view! {
        <div class="admin-page stack">
            <header><h2>"审计日志"</h2><span class="tag">{move || format!("{} 条", total.get())}</span></header>
            <div class="admin-user-toolbar">
                <input class="text-input" placeholder="搜索操作者或目标" prop:value=move || query.get() on:input=move |event| { query.set(event_target_value(&event)); page.set(1); } />
                <select class="select-input" on:change=move |event| { action.set(event_target_value(&event)); page.set(1); }>
                    <option value="all">"全部操作"</option>
                    <option value="user.approve">"批准用户"</option>
                    <option value="user.disable">"禁用用户"</option>
                    <option value="user.restore">"恢复用户"</option>
                    <option value="user.delete">"删除用户"</option>
                    <option value="user.reset_password">"重置密码"</option>
                    <option value="managed_template.assign">"分配模板"</option>
                    <option value="managed_template.sync">"同步模板"</option>
                </select>
            </div>
            <div class="admin-table-scroll">
                <table class="admin-table">
                    <thead><tr><th>"时间"</th><th>"操作者"</th><th>"操作"</th><th>"目标"</th><th>"摘要"</th></tr></thead>
                    <tbody><For each=move || entries.get() key=|entry| entry.id.clone() children=move |entry| view! {
                        <tr><td>{minute_time(&entry.created_at)}</td><td>{entry.actor_username}</td><td><code>{entry.action}</code></td><td>{entry.target_name}</td><td>{entry.summary}</td></tr>
                    } /></tbody>
                </table>
            </div>
            <div class="admin-pagination">
                <button class="button ghost" disabled=move || { page.get() <= 1 } on:click=move |_| page.update(|value| *value = value.saturating_sub(1))>"上一页"</button>
                <span>{move || format!("第 {} / {} 页", page.get(), total.get().div_ceil(20).max(1))}</span>
                <button class="button ghost" disabled=move || { page.get() >= total.get().div_ceil(20).max(1) } on:click=move |_| page.update(|value| *value += 1)>"下一页"</button>
            </div>
        </div>
    }
}
