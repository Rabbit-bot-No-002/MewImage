use leptos::prelude::*;
use mew_image_shared::ThemePreference;
use wasm_bindgen::JsValue;

use crate::app::{
    resolved_night_mode,
    state::{MainView, UiState, WorkspaceState},
};

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn TopBar(persist_ui_state: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let preferences = workspace.preferences;
    let show_favorites_panel = ui.show_favorites_panel;
    let show_settings_menu = ui.show_settings_menu;
    let main_view = ui.main_view;

    let switch_view = move |view: MainView| {
        main_view.set(view);
        let url = match view {
            MainView::Workspace => "/",
            MainView::TemplatePlaza => "/?view=templates",
        };
        if let Some(window) = web_sys::window()
            && let Ok(history) = window.history()
        {
            let _ = history.push_state_with_url(&JsValue::NULL, "", Some(url));
        }
    };

    view! {
            <header class="panel topbar">
                <div class="topbar-left">
                    <button
                        class="button ghost icon-button favorite-top-button"
                        title="收藏夹"
                        on:click=move |_| show_favorites_panel.update(|value| *value = !*value)
                    >
                        <MaterialSymbolIcon name="star" filled=true />
                    </button>
                    <nav class="main-view-switcher" aria-label="主视图">
                        <button
                            class="main-view-tab"
                            class:is-active=move || main_view.get() == MainView::Workspace
                            aria-pressed=move || main_view.get() == MainView::Workspace
                            on:click=move |_| switch_view(MainView::Workspace)
                        >
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M4 4h6v6H4V4Zm10 0h6v6h-6V4ZM4 14h6v6H4v-6Zm10 0h6v6h-6v-6Z" />
                            </svg>
                            <span>"工作台"</span>
                        </button>
                        <button
                            class="main-view-tab"
                            class:is-active=move || main_view.get() == MainView::TemplatePlaza
                            aria-pressed=move || main_view.get() == MainView::TemplatePlaza
                            on:click=move |_| switch_view(MainView::TemplatePlaza)
                        >
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="m12 2 2.15 5.4L20 8l-4.45 3.8L17 18l-5-3.25L7 18l1.45-6.2L4 8l5.85-.6L12 2Zm7 11 1.1 2.7L23 16l-2.2 1.9.7 3.1-2.5-1.65L16.5 21l.7-3.1L15 16l2.9-.3L19 13Z" />
                            </svg>
                            <span>"模板广场"</span>
                        </button>
                    </nav>
                </div>
                <div class="brand topbar-brand-centered">
                    <img class="brand-logo" src="/favicon/MewImage04.svg" alt="MewImage" />
                    <h1>"MewImage"</h1>
                </div>
                <div class="row topbar-actions">
                    <button class="button ghost" on:click=move |_| {
                        let night = resolved_night_mode(
                            preferences.get_untracked().theme,
                            ui.system_dark.get_untracked(),
                        );
                        preferences.update(|value| {
                            value.theme = if night { ThemePreference::Day } else { ThemePreference::Night };
                        });
                        persist_ui_state();
                    }>
                        {move || if resolved_night_mode(preferences.get().theme, ui.system_dark.get()) { "白天模式" } else { "夜间模式" }}
                    </button>
                    <button class="button secondary" on:click=move |_| show_settings_menu.update(|value| *value = !*value)>
                        {move || if show_settings_menu.get() { "收起设置" } else { "打开设置" }}
                    </button>
                </div>
            </header>

    }
}
