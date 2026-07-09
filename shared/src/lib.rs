use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SYNC_SCHEMA_VERSION: u32 = 1;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ProviderKind {
    #[default]
    #[serde(rename = "openai_image", alias = "open_ai_compatible")]
    OpenAiImage,
    #[serde(rename = "nano_banana", alias = "gemini")]
    NanoBanana,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "custom_http")]
    CustomHttp,
}

pub const BUILTIN_OPENAI_IMAGE_TEMPLATE_ID: &str = "builtin-openai-compatible";
pub const BUILTIN_NANO_BANANA_TEMPLATE_ID: &str = "builtin-nano-banana";
pub const BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID: &str = "builtin-openai-compatible-gateway";
pub const DEFAULT_FAVORITE_FOLDER_ID: &str = "default";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEndpointMode {
    #[default]
    ImagesApi,
    ResponsesApi,
    CustomJson,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccessMode {
    Direct,
    Proxy,
    #[default]
    Smart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Draft,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    Day,
    Night,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EncryptedSecret {
    pub salt_b64: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderTemplate {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub endpoint_path: String,
    pub method: String,
    pub auth_header: String,
    pub prompt_field: Option<String>,
    pub model_field: Option<String>,
    pub size_field: Option<String>,
    pub quality_field: Option<String>,
    pub count_field: Option<String>,
    pub reference_images_field: Option<String>,
    pub response_image_url_path: Option<String>,
    pub response_image_base64_path: Option<String>,
    pub response_revised_prompt_path: Option<String>,
    pub known_requires_proxy: bool,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl ProviderTemplate {
    pub fn builtin_openai() -> Self {
        let now = now_rfc3339();
        Self {
            id: BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into(),
            name: "OpenAI Image".into(),
            kind: ProviderKind::OpenAiImage,
            base_url: "https://api.openai.com".into(),
            endpoint_path: "/v1/images/generations".into(),
            method: "POST".into(),
            auth_header: "Authorization".into(),
            prompt_field: Some("prompt".into()),
            model_field: Some("model".into()),
            size_field: Some("size".into()),
            quality_field: Some("quality".into()),
            count_field: Some("n".into()),
            reference_images_field: Some("image".into()),
            response_image_url_path: Some("data[].url".into()),
            response_image_base64_path: Some("data[].b64_json".into()),
            response_revised_prompt_path: Some("data[0].revised_prompt".into()),
            known_requires_proxy: true,
            notes: Some("内置 OpenAI Image 模板，用于 gpt-image 模型，支持 Images API 与 Responses API。".into()),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn builtin_nano_banana() -> Self {
        let now = now_rfc3339();
        Self {
            id: BUILTIN_NANO_BANANA_TEMPLATE_ID.into(),
            name: "Nano Banana".into(),
            kind: ProviderKind::NanoBanana,
            base_url: "https://generativelanguage.googleapis.com".into(),
            endpoint_path: "/v1beta/models/gemini-2.5-flash-image:generateContent".into(),
            method: "POST".into(),
            auth_header: "x-goog-api-key".into(),
            prompt_field: Some("prompt".into()),
            model_field: Some("model".into()),
            size_field: None,
            quality_field: None,
            count_field: None,
            reference_images_field: Some("image".into()),
            response_image_url_path: None,
            response_image_base64_path: None,
            response_revised_prompt_path: None,
            known_requires_proxy: true,
            notes: Some("内置 Nano Banana 模板，使用谷歌官方图像接口。".into()),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn builtin_openai_compatible() -> Self {
        let now = now_rfc3339();
        Self {
            id: BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID.into(),
            name: "OpenAI 兼容".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: String::new(),
            endpoint_path: "/v1/images/generations".into(),
            method: "POST".into(),
            auth_header: "Authorization".into(),
            prompt_field: Some("prompt".into()),
            model_field: Some("model".into()),
            size_field: Some("aspect_ratio".into()),
            quality_field: None,
            count_field: Some("n".into()),
            reference_images_field: Some("image".into()),
            response_image_url_path: Some("data[].url".into()),
            response_image_base64_path: Some("data[].b64_json".into()),
            response_revised_prompt_path: None,
            known_requires_proxy: true,
            notes: Some("内置 OpenAI 兼容模板，用于第三方中转站图像接口。".into()),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedApiConfig {
    pub id: String,
    pub name: String,
    pub provider_template_id: String,
    pub provider_kind: ProviderKind,
    pub endpoint_mode: ProviderEndpointMode,
    pub base_url: String,
    pub model: String,
    pub access_mode: ProviderAccessMode,
    pub known_requires_proxy: bool,
    pub output_format: Option<String>,
    pub output_compression: Option<u8>,
    pub moderation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_plaintext: Option<String>,
    pub api_key_encrypted: Option<EncryptedSecret>,
    pub api_key_hint: Option<String>,
    pub prompt_guard_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageAssetRef {
    pub id: String,
    pub sha256: String,
    pub mime_type: String,
    pub byte_len: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
    pub data_url: Option<String>,
    pub remote_object_key: Option<String>,
    pub remote_url: Option<String>,
    pub source_task_id: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ParameterSnapshot {
    pub requested_width: Option<u32>,
    pub requested_height: Option<u32>,
    pub actual_width: Option<u32>,
    pub actual_height: Option<u32>,
    pub requested_quality: Option<String>,
    pub actual_quality: Option<String>,
    pub revised_prompt: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneratedImageResult {
    pub url: Option<String>,
    pub data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerationResult {
    pub images: Vec<GeneratedImageResult>,
    pub parameter_snapshot: ParameterSnapshot,
    pub raw_response_json: Option<serde_json::Value>,
}

pub fn image_mime_from_output_format(output_format: Option<&str>) -> &'static str {
    match output_format
        .unwrap_or("png")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "png" => "image/png",
        _ => "image/png",
    }
}

pub fn normalize_api_config(config: &mut EncryptedApiConfig) {
    let base_url = config.base_url.trim();
    let is_google_official = base_url.contains("generativelanguage.googleapis.com");

    if config.provider_template_id == BUILTIN_OPENAI_IMAGE_TEMPLATE_ID {
        config.provider_kind = ProviderKind::OpenAiImage;
        config.endpoint_mode = match config.endpoint_mode {
            ProviderEndpointMode::ResponsesApi => ProviderEndpointMode::ResponsesApi,
            ProviderEndpointMode::CustomJson => ProviderEndpointMode::ImagesApi,
            ProviderEndpointMode::ImagesApi => ProviderEndpointMode::ImagesApi,
        };
        if config.base_url.trim().is_empty() {
            config.base_url = "https://api.openai.com".into();
        }
        if config.model.trim().is_empty() {
            config.model = "gpt-image-2".into();
        }
        return;
    }

    if config.provider_template_id == BUILTIN_NANO_BANANA_TEMPLATE_ID {
        if is_google_official {
            config.provider_kind = ProviderKind::NanoBanana;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            if config.model.trim().is_empty() {
                config.model = "gemini-2.5-flash-image".into();
            }
        } else {
            config.provider_template_id = BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID.into();
            config.provider_kind = ProviderKind::OpenAiCompatible;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            if config.model.trim().is_empty() {
                config.model = "gemini-2.5-flash-image".into();
            }
        }
        return;
    }

    if config.provider_template_id == BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID {
        config.provider_kind = ProviderKind::OpenAiCompatible;
        config.endpoint_mode = ProviderEndpointMode::CustomJson;
        if config.model.trim().is_empty() {
            config.model = "gemini-2.5-flash-image".into();
        }
        return;
    }

    match config.provider_kind {
        ProviderKind::OpenAiImage => {
            config.provider_template_id = BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into();
            if config.base_url.trim().is_empty() {
                config.base_url = "https://api.openai.com".into();
            }
            if config.model.trim().is_empty() {
                config.model = "gpt-image-2".into();
            }
            config.endpoint_mode = match config.endpoint_mode {
                ProviderEndpointMode::ResponsesApi => ProviderEndpointMode::ResponsesApi,
                _ => ProviderEndpointMode::ImagesApi,
            };
        }
        ProviderKind::NanoBanana => {
            if is_google_official {
                config.provider_template_id = BUILTIN_NANO_BANANA_TEMPLATE_ID.into();
                config.endpoint_mode = ProviderEndpointMode::CustomJson;
                if config.model.trim().is_empty() {
                    config.model = "gemini-2.5-flash-image".into();
                }
            } else {
                config.provider_template_id = BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID.into();
                config.provider_kind = ProviderKind::OpenAiCompatible;
                config.endpoint_mode = ProviderEndpointMode::CustomJson;
                if config.model.trim().is_empty() {
                    config.model = "gemini-2.5-flash-image".into();
                }
            }
        }
        ProviderKind::OpenAiCompatible => {
            config.provider_template_id = BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID.into();
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            if config.model.trim().is_empty() {
                config.model = "gemini-2.5-flash-image".into();
            }
        }
        ProviderKind::CustomHttp => {}
    }
}

pub fn aspect_ratio_from_dimensions(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "1:1".into();
    }

    const CANDIDATES: &[(u32, u32)] = &[
        (4, 3),
        (3, 4),
        (16, 9),
        (9, 16),
        (2, 3),
        (3, 2),
        (1, 1),
        (4, 5),
        (5, 4),
        (21, 9),
    ];
    let target = width as f64 / height as f64;
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

    format!("{}:{}", best.0, best.1)
}

pub fn nano_banana_image_size_from_dimensions(width: u32, height: u32) -> String {
    let longest_edge = width.max(height);
    if longest_edge >= 3072 {
        "4K".into()
    } else if longest_edge >= 1536 {
        "2K".into()
    } else {
        "1K".into()
    }
}

pub fn extract_gemini_generation_result(
    request: &GenerationRequest,
    response_json: serde_json::Value,
    output_format: Option<&str>,
) -> Result<GenerationResult, String> {
    let fallback_mime = image_mime_from_output_format(output_format);
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let revised_prompt = find_first_string(&response_json, "text");

    collect_gemini_image_payloads(&response_json, fallback_mime, &mut images, &mut seen);

    if images.is_empty() {
        return Err("接口返回里没有解析到 Gemini 图片结果。".into());
    }

    Ok(GenerationResult {
        images,
        parameter_snapshot: ParameterSnapshot {
            requested_width: Some(request.width),
            requested_height: Some(request.height),
            actual_width: Some(request.width),
            actual_height: Some(request.height),
            requested_quality: request.quality.clone(),
            actual_quality: Some("standard".into()),
            revised_prompt,
            duration_ms: None,
        },
        raw_response_json: Some(response_json),
    })
}

fn collect_gemini_image_payloads(
    value: &serde_json::Value,
    fallback_mime: &str,
    images: &mut Vec<GeneratedImageResult>,
    seen: &mut HashSet<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(inline_data) = map
                .get("inline_data")
                .or_else(|| map.get("inlineData"))
            {
                if let Some(data) = inline_data.get("data").and_then(|value| value.as_str()) {
                    let mime_type = inline_data
                        .get("mime_type")
                        .or_else(|| inline_data.get("mimeType"))
                        .and_then(|value| value.as_str())
                        .unwrap_or(fallback_mime);
                    let data_url = format!("data:{mime_type};base64,{data}");
                    if seen.insert(data_url.clone()) {
                        images.push(GeneratedImageResult {
                            url: None,
                            data_url: Some(data_url),
                        });
                    }
                }
            }
            for child in map.values() {
                collect_gemini_image_payloads(child, fallback_mime, images, seen);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_gemini_image_payloads(item, fallback_mime, images, seen);
            }
        }
        _ => {}
    }
}

pub fn extract_openai_responses_result(
    request: &GenerationRequest,
    response_json: serde_json::Value,
    output_format: Option<&str>,
) -> Result<GenerationResult, String> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut actual_width = Some(request.width);
    let mut actual_height = Some(request.height);
    let mut actual_quality = request.quality.clone();
    let mut revised_prompt = None::<String>;
    let fallback_mime = image_mime_from_output_format(output_format);

    if let Some(output_items) = response_json
        .get("output")
        .and_then(|value| value.as_array())
    {
        for item in output_items {
            if item.get("type").and_then(|value| value.as_str()) != Some("image_generation_call") {
                continue;
            }

            if revised_prompt.is_none() {
                revised_prompt = find_first_string(item, "revised_prompt");
            }
            if let Some(size) = item.get("size").and_then(|value| value.as_str()) {
                if let Some((width, height)) = parse_size_label(size) {
                    actual_width = Some(width);
                    actual_height = Some(height);
                }
            }
            if let Some(quality) = item.get("quality").and_then(|value| value.as_str()) {
                actual_quality = Some(quality.to_string());
            }

            if let Some(result) = item.get("result") {
                collect_openai_response_images(result, fallback_mime, &mut images, &mut seen);
            }
        }
    }

    if images.is_empty() {
        return Err("接口返回里没有解析到 Responses API 图片结果。".into());
    }

    Ok(GenerationResult {
        images,
        parameter_snapshot: ParameterSnapshot {
            requested_width: Some(request.width),
            requested_height: Some(request.height),
            actual_width,
            actual_height,
            requested_quality: request.quality.clone(),
            actual_quality,
            revised_prompt,
            duration_ms: None,
        },
        raw_response_json: Some(response_json),
    })
}

pub fn extract_nano_banana_result(
    request: &GenerationRequest,
    response_json: serde_json::Value,
    output_format: Option<&str>,
) -> Result<GenerationResult, String> {
    let fallback_mime = image_mime_from_output_format(output_format);
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut actual_width = Some(request.width);
    let mut actual_height = Some(request.height);
    let mut actual_quality = Some("standard".into());
    let revised_prompt = find_first_string(&response_json, "revised_prompt");

    if let Some(items) = response_json.get("data").and_then(|value| value.as_array()) {
        for item in items {
            if let Some(url) = item.get("url").and_then(|value| value.as_str()) {
                if seen.insert(url.to_string()) {
                    images.push(GeneratedImageResult {
                        url: Some(url.to_string()),
                        data_url: None,
                    });
                }
            }
            if let Some(raw) = item.get("b64_json").and_then(|value| value.as_str()) {
                let data_url = format!("data:{fallback_mime};base64,{raw}");
                if seen.insert(data_url.clone()) {
                    images.push(GeneratedImageResult {
                        url: None,
                        data_url: Some(data_url),
                    });
                }
            }
            if let Some(size) = item.get("size").and_then(|value| value.as_str()) {
                if let Some((width, height)) = parse_size_label(size) {
                    actual_width = Some(width);
                    actual_height = Some(height);
                }
            }
            if let Some(quality) = item.get("quality").and_then(|value| value.as_str()) {
                actual_quality = Some(quality.to_string());
            }
        }
    }

    if images.is_empty() {
        return Err("接口返回里没有解析到 nano banana 图片结果。".into());
    }

    Ok(GenerationResult {
        images,
        parameter_snapshot: ParameterSnapshot {
            requested_width: Some(request.width),
            requested_height: Some(request.height),
            actual_width,
            actual_height,
            requested_quality: request.quality.clone(),
            actual_quality,
            revised_prompt,
            duration_ms: None,
        },
        raw_response_json: Some(response_json),
    })
}

pub fn extract_openai_compatible_result(
    request: &GenerationRequest,
    response_json: serde_json::Value,
    output_format: Option<&str>,
) -> Result<GenerationResult, String> {
    extract_nano_banana_result(request, response_json, output_format).map_err(|error| {
        error.replace("nano banana", "OpenAI 兼容")
    })
}

fn collect_openai_response_images(
    value: &serde_json::Value,
    fallback_mime: &str,
    images: &mut Vec<GeneratedImageResult>,
    seen: &mut HashSet<String>,
) {
    fn walk(
        value: &serde_json::Value,
        path: &mut Vec<String>,
        fallback_mime: &str,
        images: &mut Vec<GeneratedImageResult>,
        seen: &mut HashSet<String>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    path.push(key.clone());
                    walk(child, path, fallback_mime, images, seen);
                    path.pop();
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, path, fallback_mime, images, seen);
                }
            }
            serde_json::Value::String(text) => {
                if let Some(image) = normalize_openai_response_image(text, path, fallback_mime) {
                    let dedupe_key = image
                        .url
                        .as_ref()
                        .or(image.data_url.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    if !dedupe_key.is_empty() && seen.insert(dedupe_key) {
                        images.push(image);
                    }
                }
            }
            _ => {}
        }
    }

    walk(value, &mut Vec::new(), fallback_mime, images, seen);
}

fn normalize_openai_response_image(
    value: &str,
    path: &[String],
    fallback_mime: &str,
) -> Option<GeneratedImageResult> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let is_root_payload = path.is_empty();
    let has_image_hint = path_indicates_image(path);

    if let Some(data_url) = normalize_data_url(trimmed) {
        if !is_root_payload && !has_image_hint {
            return None;
        }
        return Some(GeneratedImageResult {
            url: None,
            data_url: Some(data_url),
        });
    }

    if looks_like_http_url(trimmed) {
        if !is_root_payload && !has_image_hint {
            return None;
        }
        return Some(GeneratedImageResult {
            url: Some(trimmed.to_string()),
            data_url: None,
        });
    }

    if looks_like_base64_payload(trimmed)
        || (has_image_hint && looks_like_base64_fragment(trimmed))
    {
        if !is_root_payload && !has_image_hint {
            return None;
        }
        return Some(GeneratedImageResult {
            url: None,
            data_url: Some(format!(
                "data:{fallback_mime};base64,{}",
                trimmed.chars().filter(|char| !char.is_whitespace()).collect::<String>()
            )),
        });
    }

    None
}

fn normalize_data_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.starts_with("data:image/") {
        return None;
    }
    trimmed
        .split_once(',')
        .map(|_| trimmed.to_string())
        .filter(|data_url| data_url.contains(";base64,"))
}

fn looks_like_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn looks_like_base64_payload(value: &str) -> bool {
    let compact = value.chars().filter(|char| !char.is_whitespace()).collect::<String>();
    compact.len() >= 64
        && compact.chars().all(|char| {
            char.is_ascii_alphanumeric() || matches!(char, '+' | '/' | '=' | '-' | '_')
        })
}

fn looks_like_base64_fragment(value: &str) -> bool {
    let compact = value.chars().filter(|char| !char.is_whitespace()).collect::<String>();
    compact.len() >= 8
        && compact.chars().all(|char| {
            char.is_ascii_alphanumeric() || matches!(char, '+' | '/' | '=' | '-' | '_')
        })
}

fn path_indicates_image(path: &[String]) -> bool {
    path.iter().any(|segment| {
        matches!(
            segment.as_str(),
            "b64_json" | "base64" | "base64_json" | "image" | "image_url" | "url" | "data" | "result"
        )
    })
}

fn find_first_string(value: &serde_json::Value, key: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(string) = map.get(key).and_then(|item| item.as_str()) {
                return Some(string.to_string());
            }
            for child in map.values() {
                if let Some(found) = find_first_string(child, key) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(found) = find_first_string(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_size_label(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerationRequest {
    pub prompt: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub quality: Option<String>,
    pub count: u32,
    pub endpoint_mode: ProviderEndpointMode,
    pub reference_assets: Vec<ImageAssetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerateViaProxyRequest {
    pub template: ProviderTemplate,
    pub config: EncryptedApiConfig,
    pub request: GenerationRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalTaskRecord {
    pub id: String,
    pub thread_id: String,
    pub config_id: String,
    pub prompt: String,
    pub requested_model: String,
    pub reference_asset_ids: Vec<String>,
    pub result: Option<GenerationResult>,
    pub favorite: bool,
    #[serde(default)]
    pub favorite_folder_id: Option<String>,
    pub status: TaskStatus,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationThread {
    pub id: String,
    pub title: String,
    pub draft_prompt: String,
    pub task_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavoriteFolder {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn default_favorite_folders() -> Vec<FavoriteFolder> {
    let now = now_rfc3339();
    vec![FavoriteFolder {
        id: DEFAULT_FAVORITE_FOLDER_ID.into(),
        name: "默认".into(),
        created_at: now.clone(),
        updated_at: now,
    }]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppPreferences {
    pub theme: ThemePreference,
    pub clear_prompt_after_submit: bool,
    pub preserve_draft_on_restart: bool,
    pub reuse_last_config: bool,
    #[serde(default = "default_favorite_folders")]
    pub favorite_folders: Vec<FavoriteFolder>,
    #[serde(default)]
    pub active_favorite_folder_id: Option<String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Day,
            clear_prompt_after_submit: false,
            preserve_draft_on_restart: true,
            reuse_last_config: true,
            favorite_folders: default_favorite_folders(),
            active_favorite_folder_id: Some(DEFAULT_FAVORITE_FOLDER_ID.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SyncCheckpoint {
    pub last_push_at: Option<String>,
    pub last_pull_at: Option<String>,
    pub last_merged_at: Option<String>,
    pub server_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncEnvelope {
    pub schema_version: u32,
    pub updated_at: String,
    pub configs: Vec<EncryptedApiConfig>,
    pub tasks: Vec<LocalTaskRecord>,
    pub threads: Vec<ConversationThread>,
    pub assets: Vec<ImageAssetRef>,
    pub preferences: AppPreferences,
}

impl Default for SyncEnvelope {
    fn default() -> Self {
        Self {
            schema_version: SYNC_SCHEMA_VERSION,
            updated_at: now_rfc3339(),
            configs: Vec::new(),
            tasks: Vec::new(),
            threads: Vec::new(),
            assets: Vec::new(),
            preferences: AppPreferences::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncPushRequest {
    pub client_updated_at: String,
    pub envelope: SyncEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncPullResponse {
    pub envelope: SyncEnvelope,
    pub checkpoint: SyncCheckpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergePreviewResponse {
    pub merged_updated_at: String,
    pub config_count: usize,
    pub task_count: usize,
    pub thread_count: usize,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderTemplateImportRequest {
    pub template: ProviderTemplate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthResponse {
    pub user: UserSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeResponse {
    pub user: Option<UserSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadInitRequest {
    pub file_name: String,
    pub mime_type: String,
    pub byte_len: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadInitResponse {
    pub upload_token: String,
    pub upload_url: String,
    pub asset_id: String,
    pub object_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadCompleteRequest {
    pub upload_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadCompleteResponse {
    pub asset: ImageAssetRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalAppState {
    pub configs: Vec<EncryptedApiConfig>,
    pub tasks: Vec<LocalTaskRecord>,
    pub threads: Vec<ConversationThread>,
    pub assets: Vec<ImageAssetRef>,
    pub preferences: AppPreferences,
    pub checkpoint: SyncCheckpoint,
}

impl Default for LocalAppState {
    fn default() -> Self {
        Self {
            configs: Vec::new(),
            tasks: Vec::new(),
            threads: vec![ConversationThread {
                id: new_id(),
                title: "新的会话".into(),
                draft_prompt: String::new(),
                task_ids: Vec::new(),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            }],
            assets: Vec::new(),
            preferences: AppPreferences::default(),
            checkpoint: SyncCheckpoint::default(),
        }
    }
}

pub trait SyncEntity: Clone {
    fn sync_id(&self) -> &str;
    fn sync_updated_at(&self) -> &str;
}

impl SyncEntity for EncryptedApiConfig {
    fn sync_id(&self) -> &str {
        &self.id
    }

    fn sync_updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl SyncEntity for LocalTaskRecord {
    fn sync_id(&self) -> &str {
        &self.id
    }

    fn sync_updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl SyncEntity for ConversationThread {
    fn sync_id(&self) -> &str {
        &self.id
    }

    fn sync_updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl SyncEntity for ImageAssetRef {
    fn sync_id(&self) -> &str {
        &self.id
    }

    fn sync_updated_at(&self) -> &str {
        &self.updated_at
    }
}

pub fn merge_records<T: SyncEntity>(left: &[T], right: &[T]) -> Vec<T> {
    let mut merged: HashMap<String, T> = HashMap::new();
    for item in left.iter().chain(right.iter()) {
        match merged.get(item.sync_id()) {
            Some(existing) if existing.sync_updated_at() >= item.sync_updated_at() => {}
            _ => {
                merged.insert(item.sync_id().to_string(), item.clone());
            }
        }
    }
    let mut values: Vec<T> = merged.into_values().collect();
    values.sort_by(|a, b| a.sync_updated_at().cmp(b.sync_updated_at()));
    values
}

pub fn merge_envelopes(left: &SyncEnvelope, right: &SyncEnvelope) -> SyncEnvelope {
    SyncEnvelope {
        schema_version: left.schema_version.max(right.schema_version),
        updated_at: left.updated_at.clone().max(right.updated_at.clone()),
        configs: merge_records(&left.configs, &right.configs),
        tasks: merge_records(&left.tasks, &right.tasks),
        threads: merge_records(&left.threads, &right.threads),
        assets: merge_records(&left.assets, &right.assets),
        preferences: if left.updated_at >= right.updated_at {
            left.preferences.clone()
        } else {
            right.preferences.clone()
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SizePreset {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SizeClampResult {
    pub width: u32,
    pub height: u32,
    pub adjusted: bool,
}

pub fn clamp_size(width: u32, height: u32) -> SizeClampResult {
    let max_pixels: u64 = 4096 * 4096;
    let mut safe_width = width.max(256);
    let mut safe_height = height.max(256);

    safe_width = (safe_width / 16).max(1) * 16;
    safe_height = (safe_height / 16).max(1) * 16;

    while (safe_width as u64) * (safe_height as u64) > max_pixels {
        safe_width = (safe_width.saturating_sub(16)).max(256);
        safe_height = (safe_height.saturating_sub(16)).max(256);
    }

    SizeClampResult {
        width: safe_width,
        height: safe_height,
        adjusted: safe_width != width || safe_height != height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_size_aligns_to_16() {
        let result = clamp_size(1033, 777);
        assert_eq!(result.width % 16, 0);
        assert_eq!(result.height % 16, 0);
        assert!(result.adjusted);
    }

    #[test]
    fn merge_prefers_newer_records() {
        let older = ConversationThread {
            id: "thread-1".into(),
            title: "旧标题".into(),
            draft_prompt: String::new(),
            task_ids: vec![],
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let newer = ConversationThread {
            title: "新标题".into(),
            updated_at: "2026-01-02T00:00:00+00:00".into(),
            ..older.clone()
        };
        let merged = merge_records(&[older], &[newer.clone()]);
        assert_eq!(merged, vec![newer]);
    }

    #[test]
    fn merge_envelope_keeps_latest_preferences() {
        let left = SyncEnvelope {
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            preferences: AppPreferences {
                theme: ThemePreference::Day,
                ..AppPreferences::default()
            },
            ..SyncEnvelope::default()
        };
        let right = SyncEnvelope {
            updated_at: "2026-01-02T00:00:00+00:00".into(),
            preferences: AppPreferences {
                theme: ThemePreference::Night,
                ..AppPreferences::default()
            },
            ..SyncEnvelope::default()
        };
        let merged = merge_envelopes(&left, &right);
        assert_eq!(merged.preferences.theme, ThemePreference::Night);
    }

    #[test]
    fn responses_result_only_scans_result_subtree() {
        let request = GenerationRequest {
            prompt: "test".into(),
            model: "gpt-5.5".into(),
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            reference_assets: Vec::new(),
        };
        let response_json = serde_json::json!({
            "output": [{
                "type": "image_generation_call",
                "revised_prompt": "这里不是图片",
                "result": {
                    "payload": {
                        "items": [{
                            "base64": "aGVsbG8="
                        }]
                    }
                },
                "size": "1024x1024",
                "quality": "medium"
            }]
        });

        let result = extract_openai_responses_result(&request, response_json, Some("png")).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            result.images[0].data_url.as_deref(),
            Some("data:image/png;base64,aGVsbG8=")
        );
        assert_eq!(
            result.parameter_snapshot.actual_quality.as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn normalize_legacy_openai_config_to_openai_image() {
        let mut config = EncryptedApiConfig {
            id: "config-1".into(),
            name: "旧 OpenAI".into(),
            provider_template_id: BUILTIN_OPENAI_IMAGE_TEMPLATE_ID.into(),
            provider_kind: ProviderKind::OpenAiImage,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            base_url: "https://api.openai.com".into(),
            model: "gpt-image-2".into(),
            access_mode: ProviderAccessMode::Smart,
            known_requires_proxy: true,
            output_format: Some("png".into()),
            output_compression: Some(100),
            moderation: Some("auto".into()),
            api_key_plaintext: None,
            api_key_encrypted: None,
            api_key_hint: None,
            prompt_guard_enabled: true,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };
        normalize_api_config(&mut config);
        assert_eq!(config.provider_kind, ProviderKind::OpenAiImage);
        assert_eq!(config.endpoint_mode, ProviderEndpointMode::ResponsesApi);
    }

    #[test]
    fn normalize_gateway_config_to_openai_compatible() {
        let mut config = EncryptedApiConfig {
            id: "config-2".into(),
            name: "旧香蕉中转".into(),
            provider_template_id: BUILTIN_NANO_BANANA_TEMPLATE_ID.into(),
            provider_kind: ProviderKind::NanoBanana,
            endpoint_mode: ProviderEndpointMode::CustomJson,
            base_url: "https://example.com".into(),
            model: String::new(),
            access_mode: ProviderAccessMode::Smart,
            known_requires_proxy: true,
            output_format: Some("png".into()),
            output_compression: Some(100),
            moderation: Some("auto".into()),
            api_key_plaintext: None,
            api_key_encrypted: None,
            api_key_hint: None,
            prompt_guard_enabled: true,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };
        normalize_api_config(&mut config);
        assert_eq!(config.provider_kind, ProviderKind::OpenAiCompatible);
        assert_eq!(
            config.provider_template_id,
            BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID
        );
        assert_eq!(config.model, "gemini-2.5-flash-image");
    }
}
