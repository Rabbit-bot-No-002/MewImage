use super::super::*;

pub(crate) fn apply_theme(theme: ThemePreference) {
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

pub(crate) fn default_thread() -> ConversationThread {
    ConversationThread {
        id: new_id(),
        title: "新的会话".into(),
        draft_prompt: String::new(),
        task_ids: Vec::new(),
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    }
}

pub(crate) fn thread_display_name(thread: &ConversationThread) -> String {
    if thread.title.trim().is_empty() {
        "新的会话".into()
    } else {
        thread.title.clone()
    }
}

pub(crate) fn summarize_prompt(prompt: &str) -> String {
    let summary: String = prompt.chars().take(12).collect();
    if prompt.chars().count() > 12 {
        format!("{summary}…")
    } else {
        summary
    }
}

pub(crate) fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn api_error_message(raw: String, fallback: &str) -> String {
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_string))
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if raw.trim().is_empty() {
                fallback.to_string()
            } else {
                raw
            }
        })
}

pub(crate) fn mark_api_keys_for_reencryption(configs: &mut [EncryptedApiConfig]) {
    let updated_at = now_rfc3339();
    for config in configs {
        if config.api_key_plaintext.is_none() {
            continue;
        }
        config.api_key_encrypted = None;
        config.updated_at = updated_at.clone();
    }
}

pub(crate) fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn validate_frontend_password_strength(
    password: &str,
    confirm: &str,
) -> Result<(), String> {
    if password != confirm {
        return Err("两次输入的密码不一致。".into());
    }
    if password.len() < 10 {
        return Err("密码至少需要 10 个字符。".into());
    }
    let has_upper = password.chars().any(|ch| ch.is_ascii_uppercase());
    let has_lower = password.chars().any(|ch| ch.is_ascii_lowercase());
    let has_digit = password.chars().any(|ch| ch.is_ascii_digit());
    let has_symbol = password.chars().any(|ch| !ch.is_ascii_alphanumeric());
    if has_upper && has_lower && has_digit && has_symbol {
        Ok(())
    } else {
        Err("密码必须同时包含大写字母、小写字母、数字和符号。".into())
    }
}

pub(crate) fn auth_status_message(user: &UserSummary) -> String {
    match user.status.as_str() {
        "approved" => format!(
            "欢迎，{}。账号已审批，服务器当前保存 {} 张图片，可手动同步。",
            user.username, user.image_count
        ),
        "pending" => format!(
            "欢迎，{}。账号正在等待管理员审批，暂不能使用云端同步。",
            user.username
        ),
        "disabled" => format!("账号 {} 已被禁用，请联系管理员。", user.username),
        _ => format!("欢迎，{}。当前账号状态：{}。", user.username, user.status),
    }
}

pub(crate) fn is_openai_image_model(config: &EncryptedApiConfig) -> bool {
    config.provider_kind == ProviderKind::OpenAiImage
        && config.model.to_ascii_lowercase().contains("image")
}

pub(crate) fn aspect_ratio_label(width: u32, height: u32) -> String {
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

pub(crate) fn gcd(left: u32, right: u32) -> u32 {
    let mut a = left;
    let mut b = right;
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

pub(crate) fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        format!("{:.2} 秒", duration_ms as f64 / 1_000.0)
    } else {
        format!("{duration_ms} 毫秒")
    }
}

pub(crate) fn format_failure_raw_response(value: &serde_json::Value) -> String {
    let mut copy = value.clone();
    if let Some(output) = copy
        .get_mut("output")
        .and_then(|value| value.as_array_mut())
    {
        for item in output {
            if let Some(result) = item.get_mut("result") {
                if let Some(text) = result.as_str() {
                    if text.len() > 96 {
                        *result =
                            serde_json::Value::String(format!("<base64_data len={}>", text.len()));
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

pub(crate) fn redact_large_base64_values(map: &mut serde_json::Map<String, serde_json::Value>) {
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

pub(crate) fn format_shanghai_datetime(value: &str) -> String {
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

pub(crate) fn format_shanghai_date_compact(value: &str) -> Option<String> {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    let timestamp = date.get_time();
    if !timestamp.is_finite() {
        return None;
    }
    let shanghai = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        timestamp + 8.0 * 60.0 * 60.0 * 1000.0,
    ));
    Some(format!(
        "{:04}{:02}{:02}",
        shanghai.get_utc_full_year() as i32,
        shanghai.get_utc_month() + 1,
        shanghai.get_utc_date()
    ))
}

pub(crate) fn today_compact() -> String {
    let now = js_sys::Date::new_0();
    format!(
        "{:04}{:02}{:02}",
        now.get_full_year() as i32,
        now.get_month() + 1,
        now.get_date()
    )
}

pub(crate) fn format_byte_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

pub(crate) fn confirm_popover_style(anchor_x: f64, anchor_y: f64) -> String {
    let (viewport_width, viewport_height) = browser_viewport_size();
    let popover_width = 280.0_f64.min((viewport_width - 24.0).max(0.0));
    let max_left = (viewport_width - popover_width - 12.0).max(12.0);
    let left = (anchor_x + 8.0).clamp(12.0, max_left);

    if anchor_y > viewport_height / 2.0 {
        let bottom = (viewport_height - anchor_y + 8.0).max(12.0);
        format!("left: {left}px; bottom: {bottom}px;")
    } else {
        let top = (anchor_y + 8.0).max(12.0);
        format!("left: {left}px; top: {top}px;")
    }
}

pub(crate) fn favorite_folder_picker_style(anchor_x: f64, anchor_y: f64) -> String {
    let (viewport_width, viewport_height) = browser_viewport_size();
    favorite_folder_picker_style_for_viewport(anchor_x, anchor_y, viewport_width, viewport_height)
}

pub(crate) fn favorite_folder_picker_style_for_viewport(
    anchor_x: f64,
    anchor_y: f64,
    viewport_width: f64,
    viewport_height: f64,
) -> String {
    let margin = 12.0;
    let gap = 8.0;
    let popover_width = 240.0_f64.min((viewport_width - margin * 2.0).max(0.0));
    let max_left = (viewport_width - popover_width - margin).max(margin);
    let left = (anchor_x + gap).clamp(margin, max_left);

    // 菜单靠近视口下半部时向上展开，并限制可滚动高度。
    if anchor_y > viewport_height / 2.0 {
        let bottom = (viewport_height - anchor_y + gap).max(margin);
        let max_height = (viewport_height - bottom - margin).max(0.0);
        format!("left: {left}px; bottom: {bottom}px; max-height: {max_height}px;")
    } else {
        let top = (anchor_y + gap).max(margin);
        let max_height = (viewport_height - top - margin).max(0.0);
        format!("left: {left}px; top: {top}px; max-height: {max_height}px;")
    }
}

pub(crate) fn browser_viewport_size() -> (f64, f64) {
    web_sys::window()
        .map(|window| {
            let width = window
                .inner_width()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(1280.0);
            let height = window
                .inner_height()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(720.0);
            (width, height)
        })
        .unwrap_or((1280.0, 720.0))
}

pub(crate) fn local_clear_confirmation(scope: LocalDataClearScope) -> (&'static str, &'static str) {
    match scope {
        LocalDataClearScope::Workspace => (
            "清除本地历史与图片",
            "将永久删除当前浏览器中的会话、任务、收藏和图片原文件；服务商配置与云端数据保留。是否继续？",
        ),
        LocalDataClearScope::Configs => (
            "清除本地服务商配置",
            "将永久删除当前浏览器保存的服务商配置和明文 API Key，并恢复默认空配置。是否继续？",
        ),
        LocalDataClearScope::Preferences => (
            "重置本地界面偏好",
            "将重置主题、收藏文件夹和界面偏好；历史图片和服务商配置保留。是否继续？",
        ),
        LocalDataClearScope::All => (
            "清除全部本地数据",
            "将永久删除当前浏览器中的工作区、图片、配置、API Key 和界面偏好；云端数据与账号保留。是否继续？",
        ),
    }
}

pub(crate) fn cloud_clear_confirmation(
    scope: &CloudDataClearScope,
) -> (&'static str, &'static str) {
    match scope {
        CloudDataClearScope::SyncData => (
            "清除云端同步数据",
            "将永久删除当前账号的云端同步快照、服务器图片和未完成上传；当前浏览器数据保留。是否继续？",
        ),
        CloudDataClearScope::ProviderTemplates => (
            "清除云端服务商模板",
            "将永久删除当前账号保存到服务器的自定义服务商模板；本地配置保留。是否继续？",
        ),
        CloudDataClearScope::All => (
            "清除全部云端数据",
            "将永久删除当前账号的所有云端同步数据、服务器图片和自定义模板；账号与当前浏览器数据保留。是否继续？",
        ),
    }
}

pub(crate) fn mask_key(value: &str) -> String {
    if value.len() <= 6 {
        return "******".into();
    }
    format!("{}***{}", &value[..3], &value[value.len() - 3..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorite_folder_picker_stays_inside_viewport_edges() {
        assert_eq!(
            favorite_folder_picker_style_for_viewport(0.0, 0.0, 320.0, 480.0),
            "left: 12px; top: 12px; max-height: 456px;"
        );
        assert_eq!(
            favorite_folder_picker_style_for_viewport(315.0, 470.0, 320.0, 480.0),
            "left: 68px; bottom: 18px; max-height: 450px;"
        );
    }
}
