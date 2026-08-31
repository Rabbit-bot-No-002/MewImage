use leptos::prelude::*;
use mew_image_shared::ThemePreference;

use crate::app::{
    resolved_night_mode,
    state::{UiState, WorkspaceState},
};

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn TopBar(persist_ui_state: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let preferences = workspace.preferences;
    let show_favorites_panel = ui.show_favorites_panel;
    let show_settings_menu = ui.show_settings_menu;

    view! {
            <header class="panel topbar">
                <div class="brand brand-inline">
                    <button
                        class="button ghost icon-button favorite-top-button"
                        title="收藏夹"
                        on:click=move |_| show_favorites_panel.update(|value| *value = !*value)
                    >
                        <MaterialSymbolIcon name="star" filled=true />
                    </button>
                    <img class="brand-logo" src="/favicon/MewImage04.svg" alt="MewImage" />
                    <h1>"MewImage"</h1>
                    <span class="muted">"默认本地模式、登录手动同步~"</span>
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
