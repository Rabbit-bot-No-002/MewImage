use leptos::prelude::*;
use mew_image_shared::{
    AppearancePreferences, BackgroundFit, BackgroundLayer, BackgroundPosition, DecorationLevel,
    ThemePreference, VisualTheme,
};
use web_sys::{Event, FileList, MouseEvent};

use crate::app::{
    background_position_css,
    state::{UiState, WorkspaceState},
};

use super::common::MaterialSymbolIcon;

#[component]
pub(crate) fn ThemeBackdrop() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    view! {
        <div class="theme-backdrop" aria-hidden="true">
            {move || {
                let preferences = workspace.preferences.get();
                let background = preferences.appearance.custom_background;
                let source = ui.background_display_src.get();
                if !background.enabled || source.is_none() {
                    return ().into_any();
                }
                let fit = if background.fit == BackgroundFit::Contain { "contain" } else { "cover" };
                let style = format!(
                    "object-fit:{fit};object-position:{};opacity:{};filter:blur({}px);",
                    background_position_css(background.position),
                    f64::from(background.opacity) / 100.0,
                    background.blur_px,
                );
                view! {
                    <img class="theme-background-image" src=source.unwrap_or_default() style=style alt="" />
                    <div
                        class="theme-background-overlay"
                        style=format!("opacity:{};", f64::from(background.overlay_strength) / 100.0)
                    ></div>
                }.into_any()
            }}
        </div>
    }
}

#[component]
pub(crate) fn AppearanceSettings(
    import_theme_background: impl Fn(FileList) + Copy + Send + Sync + 'static,
    persist_ui_state: impl Fn() + Copy + Send + Sync + 'static,
    request_delete_theme_background: impl Fn(MouseEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let ui = expect_context::<UiState>();
    let preferences = workspace.preferences;
    let background_input = ui.background_file_input;

    let choose_background = move |_| {
        if !ui.background_processing.get_untracked()
            && let Some(input) = background_input.get()
        {
            input.click();
        }
    };
    let on_background_file = move |event: Event| {
        let input = event_target::<web_sys::HtmlInputElement>(&event);
        if let Some(files) = input.files() {
            import_theme_background(files);
        }
        input.set_value("");
    };

    view! {
        <section class="stack appearance-settings">
            <div>
                <h2>"外观"</h2>
                <p class="status">"主题决定特征色与装饰，明暗模式负责背景、面板和文字对比度。"</p>
            </div>

            <div class="appearance-section stack">
                <h3>"主题风格"</h3>
                <div class="theme-card-grid">
                    <button
                        class="theme-choice-card theme-choice-classic"
                        class:is-active=move || preferences.get().appearance.visual_theme == VisualTheme::Classic
                        aria-pressed=move || preferences.get().appearance.visual_theme == VisualTheme::Classic
                        on:click=move |_| {
                            preferences.update(|value| value.appearance.visual_theme = VisualTheme::Classic);
                            persist_ui_state();
                        }
                    >
                        <span class="theme-choice-preview"></span>
                        <strong>"经典 Mew"</strong>
                        <small>"柔和粉蓝与轻量点状纹理"</small>
                    </button>
                    <button
                        class="theme-choice-card theme-choice-aurora"
                        class:is-active=move || preferences.get().appearance.visual_theme == VisualTheme::Aurora
                        aria-pressed=move || preferences.get().appearance.visual_theme == VisualTheme::Aurora
                        on:click=move |_| {
                            preferences.update(|value| value.appearance.visual_theme = VisualTheme::Aurora);
                            persist_ui_state();
                        }
                    >
                        <span class="theme-choice-preview"></span>
                        <strong>"极光星轨"</strong>
                        <small>"冰蓝星图与深靛天文仪轨道"</small>
                    </button>
                    <button
                        class="theme-choice-card theme-choice-liquid-glass"
                        class:is-active=move || preferences.get().appearance.visual_theme == VisualTheme::LiquidGlass
                        aria-pressed=move || preferences.get().appearance.visual_theme == VisualTheme::LiquidGlass
                        on:click=move |_| {
                            preferences.update(|value| value.appearance.visual_theme = VisualTheme::LiquidGlass);
                            persist_ui_state();
                        }
                    >
                        <span class="theme-choice-preview"></span>
                        <strong>"液态玻璃"</strong>
                        <small>"低模糊、高透明与柔和折射光纹"</small>
                    </button>
                </div>
            </div>

            <div class="appearance-section stack">
                <h3>"明暗模式"</h3>
                <div class="appearance-segmented">
                    {[
                        (ThemePreference::Day, "light_mode", "日间"),
                        (ThemePreference::Night, "dark_mode", "夜间"),
                        (ThemePreference::System, "contrast", "跟随系统"),
                    ].into_iter().map(|(mode, icon, label)| view! {
                        <button
                            class="button ghost"
                            class:active-compact-toggle=move || preferences.get().theme == mode
                            aria-pressed=move || preferences.get().theme == mode
                            on:click=move |_| {
                                preferences.update(|value| value.theme = mode);
                                persist_ui_state();
                            }
                        >
                            <MaterialSymbolIcon name=icon filled=false />
                            <span>{label}</span>
                        </button>
                    }).collect_view()}
                </div>
                <AppearanceRange label="卡片不透明度" value=move || preferences.get().appearance.panel_opacity min=20 max=100 on_change=move |value| {
                    preferences.update(|preferences| preferences.appearance.panel_opacity = value);
                    persist_ui_state();
                } />
                <p class="status compact-help">"100% 为主题默认效果；降低后可让自定义背景透过页面卡片，最低保留 20% 以维持内容边界。"</p>
            </div>

            <div class="appearance-section stack">
                <h3>"页面装饰"</h3>
                <div class="appearance-segmented">
                    {[
                        (DecorationLevel::Off, "关闭"),
                        (DecorationLevel::Subtle, "柔和"),
                        (DecorationLevel::Standard, "标准"),
                    ].into_iter().map(|(level, label)| view! {
                        <button
                            class="button ghost"
                            class:active-compact-toggle=move || preferences.get().appearance.decoration_level == level
                            aria-pressed=move || preferences.get().appearance.decoration_level == level
                            on:click=move |_| {
                                preferences.update(|value| value.appearance.decoration_level = level);
                                persist_ui_state();
                            }
                        >{label}</button>
                    }).collect_view()}
                </div>
            </div>

            <div class="appearance-section stack">
                <div class="row appearance-section-heading">
                    <div>
                        <h3>"自定义背景"</h3>
                        <p class="status compact-help">"原图最大 15 MiB，最长边压缩至 4096px，并统一转换为质量 0.8 的 WebP。"</p>
                    </div>
                    <div class="row">
                        <input
                            class="visually-hidden"
                            node_ref=background_input
                            type="file"
                            accept="image/png,image/jpeg,image/webp"
                            on:change=on_background_file
                        />
                        <button class="button secondary" on:click=choose_background disabled=move || ui.background_processing.get()>
                            {move || if ui.background_processing.get() { "处理中…" } else if preferences.get().appearance.custom_background.asset_id.is_some() { "替换背景" } else { "上传背景" }}
                        </button>
                        <button
                            class="button ghost danger"
                            on:click=request_delete_theme_background
                            disabled=move || preferences.get().appearance.custom_background.asset_id.is_none()
                        >"删除"</button>
                    </div>
                </div>

                {move || ui.background_display_src.get().map(|source| view! {
                    <div class="theme-background-preview"><img src=source alt="自定义背景预览" /></div>
                })}

                {move || if preferences.get().appearance.custom_background.asset_id.is_some() {
                    view! {
                        <label class="sync-key-toggle-row">
                            <input
                                type="checkbox"
                                prop:checked=move || preferences.get().appearance.custom_background.enabled
                                on:change=move |event| {
                                    let enabled = event_target_checked(&event);
                                    preferences.update(|value| value.appearance.custom_background.enabled = enabled);
                                    persist_ui_state();
                                }
                            />
                            <span>"启用自定义背景"</span>
                        </label>

                        <div class="appearance-control-grid">
                            <label class="stack compact-field">
                                <span>"填充方式"</span>
                                <select class="select-input" on:change=move |event| {
                                    let fit = if event_target_value(&event) == "contain" { BackgroundFit::Contain } else { BackgroundFit::Cover };
                                    preferences.update(|value| value.appearance.custom_background.fit = fit);
                                    persist_ui_state();
                                }>
                                    <option value="cover" selected=move || preferences.get().appearance.custom_background.fit == BackgroundFit::Cover>"覆盖"</option>
                                    <option value="contain" selected=move || preferences.get().appearance.custom_background.fit == BackgroundFit::Contain>"完整显示"</option>
                                </select>
                            </label>
                            <label class="stack compact-field">
                                <span>"背景图层"</span>
                                <select class="select-input" on:change=move |event| {
                                    let layer = if event_target_value(&event) == "above_decorations" {
                                        BackgroundLayer::AboveDecorations
                                    } else {
                                        BackgroundLayer::BelowDecorations
                                    };
                                    preferences.update(|value| value.appearance.custom_background.layer = layer);
                                    persist_ui_state();
                                }>
                                    <option value="below_decorations" selected=move || preferences.get().appearance.custom_background.layer == BackgroundLayer::BelowDecorations>"位于主题装饰下方"</option>
                                    <option value="above_decorations" selected=move || preferences.get().appearance.custom_background.layer == BackgroundLayer::AboveDecorations>"覆盖主题装饰"</option>
                                </select>
                            </label>
                            <AppearanceRange label="透明度" value=move || preferences.get().appearance.custom_background.opacity max=100 on_change=move |value| {
                                preferences.update(|preferences| preferences.appearance.custom_background.opacity = value);
                                persist_ui_state();
                            } />
                            <AppearanceRange label="模糊" value=move || preferences.get().appearance.custom_background.blur_px max=24 suffix=" px" on_change=move |value| {
                                preferences.update(|preferences| preferences.appearance.custom_background.blur_px = value);
                                persist_ui_state();
                            } />
                            <AppearanceRange label="遮罩" value=move || preferences.get().appearance.custom_background.overlay_strength max=90 on_change=move |value| {
                                preferences.update(|preferences| preferences.appearance.custom_background.overlay_strength = value);
                                persist_ui_state();
                            } />
                        </div>

                        <div class="background-position-picker" aria-label="背景位置">
                            {[
                                BackgroundPosition::TopLeft, BackgroundPosition::Top, BackgroundPosition::TopRight,
                                BackgroundPosition::Left, BackgroundPosition::Center, BackgroundPosition::Right,
                                BackgroundPosition::BottomLeft, BackgroundPosition::Bottom, BackgroundPosition::BottomRight,
                            ].into_iter().map(|position| view! {
                                <button
                                    type="button"
                                    class="background-position-button"
                                    class:is-active=move || preferences.get().appearance.custom_background.position == position
                                    aria-pressed=move || preferences.get().appearance.custom_background.position == position
                                    title=background_position_label(position)
                                    on:click=move |_| {
                                        preferences.update(|value| value.appearance.custom_background.position = position);
                                        persist_ui_state();
                                    }
                                ></button>
                            }).collect_view()}
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}

                {move || ui.appearance_message.get().map(|message| view! { <p class="status">{message}</p> })}
            </div>

            <button class="button ghost appearance-reset" on:click=move |_| {
                let asset_id = preferences.get_untracked().appearance.custom_background.asset_id;
                preferences.update(|value| {
                    value.theme = ThemePreference::Day;
                    value.appearance = AppearancePreferences::default();
                    value.appearance.custom_background.asset_id = asset_id;
                    value.appearance.custom_background.enabled = false;
                });
                persist_ui_state();
            }>"恢复外观默认值"</button>
        </section>
    }
}

#[component]
fn AppearanceRange(
    label: &'static str,
    value: impl Fn() -> u8 + Copy + Send + Sync + 'static,
    #[prop(default = 0)] min: u8,
    max: u8,
    on_change: impl Fn(u8) + Copy + Send + Sync + 'static,
    #[prop(default = "%")] suffix: &'static str,
) -> impl IntoView {
    view! {
        <label class="stack compact-field appearance-range">
            <span>{label}" · "{move || format!("{}{}", value(), suffix)}</span>
            <input type="range" min=min.to_string() max=max.to_string() prop:value=move || value().to_string() on:input=move |event| {
                if let Ok(value) = event_target_value(&event).parse::<u8>() { on_change(value.clamp(min, max)); }
            } />
        </label>
    }
}

fn background_position_label(position: BackgroundPosition) -> &'static str {
    match position {
        BackgroundPosition::TopLeft => "左上",
        BackgroundPosition::Top => "上方",
        BackgroundPosition::TopRight => "右上",
        BackgroundPosition::Left => "左侧",
        BackgroundPosition::Center => "居中",
        BackgroundPosition::Right => "右侧",
        BackgroundPosition::BottomLeft => "左下",
        BackgroundPosition::Bottom => "下方",
        BackgroundPosition::BottomRight => "右下",
    }
}
