use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SYNC_SCHEMA_VERSION: u32 = 3;

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
            notes: Some(
                "内置 OpenAI Image 模板，用于 gpt-image 模型，支持 Images API 与 Responses API。"
                    .into(),
            ),
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
            notes: Some("Nano Banana 原生接口，可使用 Google 官方或兼容中转地址。".into()),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_model: Option<String>,
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
        normalize_responses_model(config);
        return;
    }

    if config.provider_template_id == BUILTIN_NANO_BANANA_TEMPLATE_ID {
        config.provider_kind = ProviderKind::NanoBanana;
        config.endpoint_mode = ProviderEndpointMode::CustomJson;
        if config.base_url.trim().is_empty() {
            config.base_url = "https://generativelanguage.googleapis.com".into();
        }
        if config.model.trim().is_empty() {
            config.model = "gemini-2.5-flash-image".into();
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
            normalize_responses_model(config);
            config.endpoint_mode = match config.endpoint_mode {
                ProviderEndpointMode::ResponsesApi => ProviderEndpointMode::ResponsesApi,
                _ => ProviderEndpointMode::ImagesApi,
            };
        }
        ProviderKind::NanoBanana => {
            config.provider_template_id = BUILTIN_NANO_BANANA_TEMPLATE_ID.into();
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            if config.base_url.trim().is_empty() {
                config.base_url = "https://generativelanguage.googleapis.com".into();
            }
            if config.model.trim().is_empty() {
                config.model = "gemini-2.5-flash-image".into();
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

fn normalize_responses_model(config: &mut EncryptedApiConfig) {
    let has_model = config
        .responses_model
        .as_deref()
        .map(str::trim)
        .is_some_and(|model| !model.is_empty());
    if !has_model {
        config.responses_model = Some("gpt-5.5".into());
    }
}

pub fn resolve_responses_main_model(config: &EncryptedApiConfig, request_model: &str) -> String {
    if let Some(model) = config
        .responses_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return model.to_string();
    }

    let request_model = request_model.trim();
    if request_model.starts_with("gpt-") && !request_model.starts_with("gpt-image-") {
        request_model.to_string()
    } else {
        "gpt-5.5".into()
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

pub fn is_google_official_gemini_base_url(base_url: &str) -> bool {
    let Some((scheme, remainder)) = base_url.trim().split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }

    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let host_with_port = authority.rsplit('@').next().unwrap_or_default();
    let host = host_with_port
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| host_with_port.split(':').next().unwrap_or_default())
        .to_ascii_lowercase();
    host == "generativelanguage.googleapis.com"
}

pub fn gemini_auth_header(base_url: &str, api_key: &str) -> (&'static str, String) {
    if is_google_official_gemini_base_url(base_url) {
        ("x-goog-api-key", api_key.to_string())
    } else {
        ("Authorization", format!("Bearer {api_key}"))
    }
}

pub fn gemini_generate_content_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let model = model.trim();
    if base.ends_with("/v1beta/models") {
        format!("{base}/{model}:generateContent")
    } else if base.ends_with("/v1beta") {
        format!("{base}/models/{model}:generateContent")
    } else {
        format!("{base}/v1beta/models/{model}:generateContent")
    }
}

pub fn build_gemini_generation_request<T: AsRef<str>>(
    request: &GenerationRequest,
    model: &str,
    reference_data_urls: &[T],
) -> serde_json::Value {
    let mut parts = vec![serde_json::json!({
        "text": request.prompt,
    })];
    for data_url in reference_data_urls {
        let Some((mime_type, data)) = split_data_url_payload(data_url.as_ref()) else {
            continue;
        };
        parts.push(serde_json::json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": data,
            }
        }));
    }

    let mut image_config = serde_json::json!({
        "aspectRatio": aspect_ratio_from_dimensions(request.width, request.height),
    });
    if model.trim().to_ascii_lowercase().contains("gemini-3") {
        image_config["imageSize"] = serde_json::json!(nano_banana_image_size_from_dimensions(
            request.width,
            request.height,
        ));
    }

    serde_json::json!({
        "contents": [{
            "role": "user",
            "parts": parts,
        }],
        "generationConfig": {
            "responseModalities": ["TEXT", "IMAGE"],
            "imageConfig": image_config,
        },
    })
}

fn split_data_url_payload(data_url: &str) -> Option<(&str, &str)> {
    let (prefix, payload) = data_url.split_once(',')?;
    let mime_type = prefix.strip_prefix("data:")?.strip_suffix(";base64")?;
    Some((mime_type, payload))
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
            if let Some(inline_data) = map.get("inline_data").or_else(|| map.get("inlineData")) {
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
    response_json: &serde_json::Value,
    output_format: Option<&str>,
) -> Result<GenerationResult, String> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut actual_width = Some(request.width);
    let mut actual_height = Some(request.height);
    let mut actual_quality = request.quality.clone();
    let mut revised_prompt = None::<String>;
    let fallback_mime = image_mime_from_output_format(output_format);

    let mut output_groups = Vec::new();
    collect_responses_output_groups(response_json, &mut output_groups);
    for output_items in output_groups {
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
        // 成功响应中的 Base64 已进入 images，不再重复保留整份原始载荷。
        raw_response_json: None,
    })
}

fn collect_responses_output_groups<'a>(
    value: &'a serde_json::Value,
    output_groups: &mut Vec<&'a Vec<serde_json::Value>>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(output) = map.get("output").and_then(|value| value.as_array()) {
                output_groups.push(output);
            }
            for child in map.values() {
                collect_responses_output_groups(child, output_groups);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_responses_output_groups(item, output_groups);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Default)]
pub struct OpenAiResponsesStreamAccumulator {
    buffer: Vec<u8>,
    has_data_line: bool,
    completed_response: Option<serde_json::Value>,
    completed_items: Vec<serde_json::Value>,
    latest_partial_image: Option<(u64, String)>,
}

impl OpenAiResponsesStreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 增量接收上游 SSE 字节，只在完整事件块到达后解析 JSON。
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.buffer.extend_from_slice(chunk);
        while let Some((separator_index, separator_length)) = find_sse_separator(&self.buffer) {
            let event = parse_responses_sse_block(&self.buffer[..separator_index])?;
            self.buffer
                .drain(..separator_index.saturating_add(separator_length));
            self.has_data_line |= event.had_data_line;
            if let Some(payload) = event.payload {
                self.apply_event(payload)?;
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<serde_json::Value, String> {
        if self.buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
            let event = parse_responses_sse_block(&self.buffer)?;
            self.has_data_line |= event.had_data_line;
            if let Some(payload) = event.payload {
                self.apply_event(payload)?;
            }
        }

        if !self.has_data_line {
            return Err("Responses SSE 没有返回有效的 data 事件。".into());
        }
        if let Some(response) = self.completed_response {
            return Ok(response);
        }
        if !self.completed_items.is_empty() {
            return Ok(serde_json::json!({ "output": self.completed_items }));
        }
        if let Some((_, image)) = self.latest_partial_image {
            return Ok(serde_json::json!({
                "output": [{
                    "type": "image_generation_call",
                    "status": "completed",
                    "result": image,
                }]
            }));
        }
        Err("Responses SSE 没有返回可用的最终响应。".into())
    }

    fn apply_event(&mut self, mut event: serde_json::Value) -> Result<(), String> {
        if let Some(message) = responses_stream_error_message(&event) {
            return Err(format!("Responses SSE 上游返回错误：{message}"));
        }

        match event.get("type").and_then(|value| value.as_str()) {
            Some("response.completed") => {
                let response = event
                    .get_mut("response")
                    .map(serde_json::Value::take)
                    .filter(responses_payload_has_image_result);
                if response.is_some() {
                    self.completed_response = response;
                    self.completed_items.clear();
                    self.latest_partial_image = None;
                }
            }
            Some("response.output_item.done") if self.completed_response.is_none() => {
                let item = event
                    .get_mut("item")
                    .map(serde_json::Value::take)
                    .filter(|value| {
                        value.get("type").and_then(|value| value.as_str())
                            == Some("image_generation_call")
                            && responses_payload_has_image_result(value)
                    });
                if let Some(item) = item {
                    self.completed_items.push(item);
                    self.latest_partial_image = None;
                }
            }
            Some("response.image_generation_call.partial_image")
                if self.completed_response.is_none() && self.completed_items.is_empty() =>
            {
                let image = match event
                    .get_mut("partial_image_b64")
                    .map(serde_json::Value::take)
                {
                    Some(serde_json::Value::String(image)) if !image.trim().is_empty() => image,
                    _ => return Ok(()),
                };
                let index = event
                    .get("partial_image_index")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let should_replace = match self.latest_partial_image.as_ref() {
                    Some((current_index, _)) => index >= *current_index,
                    None => true,
                };
                if !should_replace {
                    return Ok(());
                }
                self.latest_partial_image = Some((index, image));
            }
            _ => {}
        }
        Ok(())
    }
}

struct ParsedSseBlock {
    had_data_line: bool,
    payload: Option<serde_json::Value>,
}

fn find_sse_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
    }
    None
}

fn parse_responses_sse_block(block: &[u8]) -> Result<ParsedSseBlock, String> {
    let block = std::str::from_utf8(block)
        .map_err(|error| format!("Responses SSE 文本不是有效 UTF-8：{error}"))?;
    let data_lines = block
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>();
    if data_lines.is_empty() {
        return Ok(ParsedSseBlock {
            had_data_line: false,
            payload: None,
        });
    }

    let joined_data;
    let data = if data_lines.len() == 1 {
        data_lines[0].trim()
    } else {
        joined_data = data_lines.join("\n");
        joined_data.trim()
    };
    if data.is_empty() || data == "[DONE]" {
        return Ok(ParsedSseBlock {
            had_data_line: true,
            payload: None,
        });
    }

    let payload = serde_json::from_str(data)
        .map_err(|error| format!("Responses SSE 事件解析失败：{error}"))?;
    Ok(ParsedSseBlock {
        had_data_line: true,
        payload: Some(payload),
    })
}

fn responses_stream_error_message(event: &serde_json::Value) -> Option<String> {
    if let Some(error) = event.get("error") {
        if let Some(message) = error.get("message").and_then(|value| value.as_str()) {
            return Some(message.to_string());
        }
        if let Some(message) = error.as_str() {
            return Some(message.to_string());
        }
    }
    let event_type = event.get("type").and_then(|value| value.as_str())?;
    if event_type.ends_with(".failed") {
        return Some(
            event
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("流式请求失败")
                .to_string(),
        );
    }
    None
}

pub fn parse_openai_responses_event_stream(body: &str) -> Result<serde_json::Value, String> {
    let mut accumulator = OpenAiResponsesStreamAccumulator::new();
    accumulator.push_chunk(body.as_bytes())?;
    accumulator.finish()
}

fn responses_payload_has_image_result(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let is_image_call =
                map.get("type").and_then(|value| value.as_str()) == Some("image_generation_call");
            if is_image_call
                && map
                    .get("result")
                    .map(|result| match result {
                        serde_json::Value::String(value) => !value.trim().is_empty(),
                        serde_json::Value::Null => false,
                        _ => true,
                    })
                    .unwrap_or(false)
            {
                return true;
            }
            map.values().any(responses_payload_has_image_result)
        }
        serde_json::Value::Array(items) => items.iter().any(responses_payload_has_image_result),
        _ => false,
    }
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
    extract_nano_banana_result(request, response_json, output_format)
        .map_err(|error| error.replace("nano banana", "OpenAI 兼容"))
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

    if (is_root_payload && looks_like_base64_fragment(trimmed))
        || looks_like_base64_payload(trimmed)
        || (has_image_hint && looks_like_base64_fragment(trimmed))
    {
        if !is_root_payload && !has_image_hint {
            return None;
        }
        return Some(GeneratedImageResult {
            url: None,
            data_url: Some(format!(
                "data:{fallback_mime};base64,{}",
                trimmed
                    .chars()
                    .filter(|char| !char.is_whitespace())
                    .collect::<String>()
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
    let compact = value
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();
    compact.len() >= 64
        && compact
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '+' | '/' | '=' | '-' | '_'))
}

fn looks_like_base64_fragment(value: &str) -> bool {
    let compact = value
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();
    compact.len() >= 8
        && compact
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '+' | '/' | '=' | '-' | '_'))
}

fn path_indicates_image(path: &[String]) -> bool {
    path.iter().any(|segment| {
        matches!(
            segment.as_str(),
            "b64_json"
                | "base64"
                | "base64_json"
                | "image"
                | "image_url"
                | "url"
                | "data"
                | "result"
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationSettingsSnapshot {
    pub width: u32,
    pub height: u32,
    pub quality: Option<String>,
    pub count: u32,
    pub endpoint_mode: ProviderEndpointMode,
    pub output_format: Option<String>,
    pub output_compression: Option<u8>,
    pub moderation: Option<String>,
    pub responses_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalTaskRecord {
    pub id: String,
    pub thread_id: String,
    pub config_id: String,
    pub prompt: String,
    pub requested_model: String,
    pub reference_asset_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_settings: Option<GenerationSettingsSnapshot>,
    pub result: Option<GenerationResult>,
    pub favorite: bool,
    #[serde(default)]
    pub favorite_folder_id: Option<String>,
    #[serde(default)]
    pub detached_from_thread: bool,
    pub status: TaskStatus,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 成功任务的图片本体由 ImageAssetRef 管理，任务中只保留结果数量和参数快照。
pub fn strip_successful_task_payloads(tasks: &mut [LocalTaskRecord]) -> bool {
    let mut changed = false;
    for task in tasks {
        if task.status != TaskStatus::Succeeded {
            continue;
        }
        let Some(result) = task.result.as_mut() else {
            continue;
        };
        for image in &mut result.images {
            changed |= image.url.is_some() || image.data_url.is_some();
            image.url = None;
            image.data_url = None;
        }
        changed |= result.raw_response_json.is_some();
        result.raw_response_json = None;
    }
    changed
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavoriteFolderTombstone {
    pub folder_id: String,
    pub deleted_at: String,
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
    #[serde(default)]
    pub favorite_folder_tombstones: Vec<FavoriteFolderTombstone>,
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
            favorite_folder_tombstones: Vec::new(),
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
    #[serde(default)]
    pub tombstones: Vec<SyncTombstone>,
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
            tombstones: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SyncEntityKind {
    Config,
    Task,
    Thread,
    Asset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncTombstone {
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub deleted_at: String,
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
#[serde(rename_all = "snake_case")]
pub enum CloudDataClearScope {
    SyncData,
    ProviderTemplates,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudDataClearRequest {
    pub scope: CloudDataClearScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CloudDataStatsResponse {
    pub image_count: usize,
    pub image_bytes: u64,
    pub provider_template_count: usize,
    pub pending_upload_count: usize,
    pub has_sync_snapshot: bool,
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
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub password_confirm: String,
    pub admin_setup_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
    pub new_password_confirm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminBootstrapRequest {
    pub admin_setup_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsernameAvailabilityResponse {
    pub username: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSetupStatusResponse {
    pub admin_exists: bool,
    pub setup_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminUserActionRequest {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminUserSummary {
    pub id: String,
    pub username: String,
    pub role: String,
    pub status: String,
    pub image_count: usize,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminUsersResponse {
    pub users: Vec<AdminUserSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub role: String,
    pub status: String,
    pub image_count: usize,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
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
    #[serde(default)]
    pub tombstones: Vec<SyncTombstone>,
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
            tombstones: Vec::new(),
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

pub fn merge_asset_records(left: &[ImageAssetRef], right: &[ImageAssetRef]) -> Vec<ImageAssetRef> {
    let mut merged = HashMap::<String, ImageAssetRef>::new();
    for asset in left.iter().chain(right.iter()) {
        let should_replace = merged.get(&asset.id).is_none_or(|existing| {
            if existing.updated_at != asset.updated_at {
                return existing.updated_at < asset.updated_at;
            }
            asset_storage_rank(asset) > asset_storage_rank(existing)
        });
        if should_replace {
            merged.insert(asset.id.clone(), asset.clone());
        }
    }
    let mut values = merged.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
    values
}

fn asset_storage_rank(asset: &ImageAssetRef) -> u8 {
    if asset.remote_object_key.is_some() {
        2
    } else if asset.data_url.is_some() {
        1
    } else {
        0
    }
}

pub fn merge_tombstones(left: &[SyncTombstone], right: &[SyncTombstone]) -> Vec<SyncTombstone> {
    let mut merged = HashMap::<(SyncEntityKind, String), SyncTombstone>::new();
    for item in left.iter().chain(right.iter()) {
        let key = (item.entity_kind, item.entity_id.clone());
        match merged.get(&key) {
            Some(existing) if existing.deleted_at >= item.deleted_at => {}
            _ => {
                merged.insert(key, item.clone());
            }
        }
    }
    let mut values = merged.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.deleted_at
            .cmp(&right.deleted_at)
            .then_with(|| left.entity_id.cmp(&right.entity_id))
    });
    values
}

pub fn apply_tombstones<T: SyncEntity>(
    records: Vec<T>,
    tombstones: &[SyncTombstone],
    entity_kind: SyncEntityKind,
) -> Vec<T> {
    let deleted_at_by_id = tombstones
        .iter()
        .filter(|item| item.entity_kind == entity_kind)
        .map(|item| (item.entity_id.as_str(), item.deleted_at.as_str()))
        .collect::<HashMap<_, _>>();
    records
        .into_iter()
        .filter(|record| {
            deleted_at_by_id
                .get(record.sync_id())
                .map(|deleted_at| record.sync_updated_at() > *deleted_at)
                .unwrap_or(true)
        })
        .collect()
}

pub fn merge_envelopes(left: &SyncEnvelope, right: &SyncEnvelope) -> SyncEnvelope {
    let tombstones = merge_tombstones(&left.tombstones, &right.tombstones);
    let preferences = merge_preferences(
        &left.preferences,
        &right.preferences,
        left.updated_at >= right.updated_at,
    );
    SyncEnvelope {
        schema_version: left.schema_version.max(right.schema_version),
        updated_at: left.updated_at.clone().max(right.updated_at.clone()),
        configs: apply_tombstones(
            merge_records(&left.configs, &right.configs),
            &tombstones,
            SyncEntityKind::Config,
        ),
        tasks: apply_tombstones(
            merge_records(&left.tasks, &right.tasks),
            &tombstones,
            SyncEntityKind::Task,
        ),
        threads: apply_tombstones(
            merge_records(&left.threads, &right.threads),
            &tombstones,
            SyncEntityKind::Thread,
        ),
        assets: apply_tombstones(
            merge_asset_records(&left.assets, &right.assets),
            &tombstones,
            SyncEntityKind::Asset,
        ),
        preferences,
        tombstones,
    }
}

fn merge_preferences(
    left: &AppPreferences,
    right: &AppPreferences,
    prefer_left: bool,
) -> AppPreferences {
    let mut merged = if prefer_left {
        left.clone()
    } else {
        right.clone()
    };
    merged.favorite_folder_tombstones = merge_favorite_folder_tombstones(left, right);
    merged.favorite_folders = merge_favorite_folders(
        &left.favorite_folders,
        &right.favorite_folders,
        &merged.favorite_folder_tombstones,
    );
    if !merged
        .favorite_folders
        .iter()
        .any(|folder| merged.active_favorite_folder_id.as_deref() == Some(folder.id.as_str()))
    {
        merged.active_favorite_folder_id = Some(DEFAULT_FAVORITE_FOLDER_ID.into());
    }
    merged
}

fn merge_favorite_folder_tombstones(
    left: &AppPreferences,
    right: &AppPreferences,
) -> Vec<FavoriteFolderTombstone> {
    let mut merged = HashMap::<String, FavoriteFolderTombstone>::new();
    for tombstone in left
        .favorite_folder_tombstones
        .iter()
        .chain(&right.favorite_folder_tombstones)
    {
        let should_replace = merged
            .get(&tombstone.folder_id)
            .map(|existing| existing.deleted_at < tombstone.deleted_at)
            .unwrap_or(true);
        if should_replace {
            merged.insert(tombstone.folder_id.clone(), tombstone.clone());
        }
    }
    let mut values = merged.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| left.deleted_at.cmp(&right.deleted_at));
    values
}

fn merge_favorite_folders(
    left: &[FavoriteFolder],
    right: &[FavoriteFolder],
    tombstones: &[FavoriteFolderTombstone],
) -> Vec<FavoriteFolder> {
    let mut merged = HashMap::<String, FavoriteFolder>::new();
    for folder in left.iter().chain(right) {
        let should_replace = merged
            .get(&folder.id)
            .map(|existing| existing.updated_at < folder.updated_at)
            .unwrap_or(true);
        if should_replace {
            merged.insert(folder.id.clone(), folder.clone());
        }
    }
    for tombstone in tombstones {
        if tombstone.folder_id == DEFAULT_FAVORITE_FOLDER_ID {
            continue;
        }
        let should_remove = merged
            .get(&tombstone.folder_id)
            .map(|folder| folder.updated_at <= tombstone.deleted_at)
            .unwrap_or(false);
        if should_remove {
            merged.remove(&tombstone.folder_id);
        }
    }
    if !merged.contains_key(DEFAULT_FAVORITE_FOLDER_ID) {
        let default_folder = default_favorite_folders().remove(0);
        merged.insert(default_folder.id.clone(), default_folder);
    }
    let mut values = merged.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        (left.id != DEFAULT_FAVORITE_FOLDER_ID)
            .cmp(&(right.id != DEFAULT_FAVORITE_FOLDER_ID))
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    values
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

    fn gemini_request() -> GenerationRequest {
        GenerationRequest {
            prompt: "参考这张图调整配色".into(),
            model: "gemini-3.1-flash-image".into(),
            width: 3840,
            height: 2160,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::CustomJson,
            reference_assets: Vec::new(),
        }
    }

    #[test]
    fn google_official_gemini_host_is_matched_exactly() {
        assert!(is_google_official_gemini_base_url(
            "https://generativelanguage.googleapis.com"
        ));
        assert!(is_google_official_gemini_base_url(
            "https://generativelanguage.googleapis.com/v1beta"
        ));
        assert!(!is_google_official_gemini_base_url(
            "https://generativelanguage.googleapis.com.evil.example"
        ));
        assert!(!is_google_official_gemini_base_url(
            "https://img-api.apinebula.com"
        ));
    }

    #[test]
    fn gemini_auth_depends_on_official_or_custom_gateway() {
        assert_eq!(
            gemini_auth_header("https://generativelanguage.googleapis.com", "secret"),
            ("x-goog-api-key", "secret".into())
        );
        assert_eq!(
            gemini_auth_header("https://gateway.example.com", "secret"),
            ("Authorization", "Bearer secret".into())
        );
    }

    #[test]
    fn gemini_gateway_url_does_not_duplicate_v1beta() {
        let expected =
            "https://img-api.apinebula.com/v1beta/models/gemini-3.1-flash-image:generateContent";
        assert_eq!(
            gemini_generate_content_url("https://img-api.apinebula.com", "gemini-3.1-flash-image"),
            expected
        );
        assert_eq!(
            gemini_generate_content_url(
                "https://img-api.apinebula.com/v1beta",
                "gemini-3.1-flash-image"
            ),
            expected
        );
    }

    #[test]
    fn gemini_request_uses_documented_image_fields() {
        let request = gemini_request();
        let body = build_gemini_generation_request(
            &request,
            &request.model,
            &["data:image/png;base64,aGVsbG8="],
        );

        assert_eq!(
            body.pointer("/contents/0/parts/1/inlineData/mimeType"),
            Some(&serde_json::json!("image/png"))
        );
        assert_eq!(
            body.pointer("/generationConfig/imageConfig/aspectRatio"),
            Some(&serde_json::json!("16:9"))
        );
        assert_eq!(
            body.pointer("/generationConfig/imageConfig/imageSize"),
            Some(&serde_json::json!("4K"))
        );
    }

    #[test]
    fn gemini_2_5_request_omits_unsupported_image_size() {
        let request = gemini_request();
        let body =
            build_gemini_generation_request(&request, "gemini-2.5-flash-image", &[] as &[String]);
        assert!(
            body.pointer("/generationConfig/imageConfig/imageSize")
                .is_none()
        );
    }

    #[test]
    fn gemini_response_reads_documented_inline_data() {
        let request = gemini_request();
        let result = extract_gemini_generation_result(
            &request,
            serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "inlineData": {
                                "mimeType": "image/webp",
                                "data": "aGVsbG8=",
                            }
                        }]
                    }
                }]
            }),
            Some("png"),
        )
        .unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            result.images[0].data_url.as_deref(),
            Some("data:image/webp;base64,aGVsbG8=")
        );
    }

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
    fn favorite_folders_merge_by_folder_timestamp_instead_of_envelope_timestamp() {
        let stale_folder = FavoriteFolder {
            id: "folder-1".into(),
            name: "文件夹 2".into(),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let renamed_folder = FavoriteFolder {
            name: "角色收藏".into(),
            updated_at: "2026-01-03T00:00:00+00:00".into(),
            ..stale_folder.clone()
        };
        let second_folder = FavoriteFolder {
            id: "folder-2".into(),
            name: "场景收藏".into(),
            created_at: "2026-01-02T00:00:00+00:00".into(),
            updated_at: "2026-01-02T00:00:00+00:00".into(),
        };
        let mut stale_preferences = AppPreferences::default();
        stale_preferences.favorite_folders.push(stale_folder);
        let mut current_preferences = AppPreferences::default();
        current_preferences
            .favorite_folders
            .extend([renamed_folder, second_folder]);

        let merged = merge_envelopes(
            &SyncEnvelope {
                updated_at: "2026-01-04T00:00:00+00:00".into(),
                preferences: stale_preferences,
                ..SyncEnvelope::default()
            },
            &SyncEnvelope {
                updated_at: "2026-01-03T00:00:00+00:00".into(),
                preferences: current_preferences,
                ..SyncEnvelope::default()
            },
        );

        assert_eq!(merged.preferences.favorite_folders.len(), 3);
        assert_eq!(
            merged
                .preferences
                .favorite_folders
                .iter()
                .find(|folder| folder.id == "folder-1")
                .map(|folder| folder.name.as_str()),
            Some("角色收藏")
        );
        assert!(
            merged
                .preferences
                .favorite_folders
                .iter()
                .any(|folder| folder.id == "folder-2")
        );
    }

    #[test]
    fn favorite_folder_tombstone_blocks_stale_device_restore() {
        let folder = FavoriteFolder {
            id: "folder-1".into(),
            name: "待删除".into(),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let mut stale_preferences = AppPreferences::default();
        stale_preferences.favorite_folders.push(folder);
        let mut deleted_preferences = AppPreferences::default();
        deleted_preferences
            .favorite_folder_tombstones
            .push(FavoriteFolderTombstone {
                folder_id: "folder-1".into(),
                deleted_at: "2026-01-02T00:00:00+00:00".into(),
            });

        let merged = merge_envelopes(
            &SyncEnvelope {
                updated_at: "2026-01-03T00:00:00+00:00".into(),
                preferences: stale_preferences,
                ..SyncEnvelope::default()
            },
            &SyncEnvelope {
                updated_at: "2026-01-02T00:00:00+00:00".into(),
                preferences: deleted_preferences,
                ..SyncEnvelope::default()
            },
        );

        assert!(
            merged
                .preferences
                .favorite_folders
                .iter()
                .all(|folder| folder.id != "folder-1")
        );
    }

    #[test]
    fn newer_tombstone_removes_record_and_blocks_old_device_restore() {
        let task = LocalTaskRecord {
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "test".into(),
            requested_model: "test".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            status: TaskStatus::Failed,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-02T00:00:00+00:00".into(),
        };
        let server = SyncEnvelope {
            tasks: vec![task.clone()],
            ..SyncEnvelope::default()
        };
        let deleting_device = SyncEnvelope {
            tombstones: vec![SyncTombstone {
                entity_kind: SyncEntityKind::Task,
                entity_id: task.id.clone(),
                deleted_at: "2026-01-03T00:00:00+00:00".into(),
            }],
            ..SyncEnvelope::default()
        };
        let deleted = merge_envelopes(&server, &deleting_device);
        assert!(deleted.tasks.is_empty());

        let stale_device = SyncEnvelope {
            tasks: vec![task],
            ..SyncEnvelope::default()
        };
        assert!(merge_envelopes(&deleted, &stale_device).tasks.is_empty());
    }

    #[test]
    fn version_one_snapshot_without_tombstones_remains_readable() {
        let mut value = serde_json::to_value(SyncEnvelope::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("schema_version".into(), serde_json::json!(1));
        object.remove("tombstones");
        object
            .get_mut("preferences")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap()
            .remove("favorite_folder_tombstones");
        let decoded: SyncEnvelope = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.schema_version, 1);
        assert!(decoded.tombstones.is_empty());
        assert!(decoded.preferences.favorite_folder_tombstones.is_empty());
    }

    #[test]
    fn legacy_task_without_detached_flag_remains_readable() {
        let task = LocalTaskRecord {
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "test".into(),
            requested_model: "test".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: None,
            favorite: true,
            favorite_folder_id: Some(DEFAULT_FAVORITE_FOLDER_ID.into()),
            detached_from_thread: true,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        };
        let mut value = serde_json::to_value(task).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("detached_from_thread");

        let decoded: LocalTaskRecord = serde_json::from_value(value).unwrap();

        assert!(!decoded.detached_from_thread);
    }

    #[test]
    fn legacy_upload_init_without_asset_id_remains_readable() {
        let request: UploadInitRequest = serde_json::from_value(serde_json::json!({
            "file_name": "image.png",
            "mime_type": "image/png",
            "byte_len": 4,
            "sha256": "hash"
        }))
        .unwrap();

        assert_eq!(request.asset_id, None);
    }

    #[test]
    fn equal_timestamp_asset_prefers_remote_object_metadata() {
        let local = ImageAssetRef {
            id: "asset-1".into(),
            sha256: "old-hash".into(),
            mime_type: "image/png".into(),
            byte_len: 4,
            width: Some(1),
            height: Some(1),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        };
        let remote = ImageAssetRef {
            sha256: "real-hash".into(),
            remote_object_key: Some("users/user-1/assets/real-hash.bin".into()),
            remote_url: Some("/api/assets/asset-1".into()),
            ..local.clone()
        };

        let merged = merge_asset_records(&[local], &[remote.clone()]);

        assert_eq!(merged, vec![remote]);
    }

    #[test]
    fn record_newer_than_tombstone_can_be_explicitly_restored() {
        let mut task = LocalTaskRecord {
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "restored".into(),
            requested_model: "test".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            status: TaskStatus::Failed,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-04T00:00:00+00:00".into(),
        };
        let tombstone = SyncTombstone {
            entity_kind: SyncEntityKind::Task,
            entity_id: task.id.clone(),
            deleted_at: "2026-01-03T00:00:00+00:00".into(),
        };
        let merged = merge_envelopes(
            &SyncEnvelope {
                tombstones: vec![tombstone],
                ..SyncEnvelope::default()
            },
            &SyncEnvelope {
                tasks: vec![task.clone()],
                ..SyncEnvelope::default()
            },
        );
        assert_eq!(merged.tasks, vec![task.clone()]);
        task.updated_at = "2026-01-03T00:00:00+00:00".into();
        let equal_timestamp = merge_envelopes(
            &SyncEnvelope {
                tombstones: vec![SyncTombstone {
                    entity_kind: SyncEntityKind::Task,
                    entity_id: task.id.clone(),
                    deleted_at: "2026-01-03T00:00:00+00:00".into(),
                }],
                ..SyncEnvelope::default()
            },
            &SyncEnvelope {
                tasks: vec![task],
                ..SyncEnvelope::default()
            },
        );
        assert!(equal_timestamp.tasks.is_empty());
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

        let result =
            extract_openai_responses_result(&request, &response_json, Some("png")).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            result.images[0].data_url.as_deref(),
            Some("data:image/png;base64,aGVsbG8=")
        );
        assert_eq!(
            result.parameter_snapshot.actual_quality.as_deref(),
            Some("medium")
        );
        assert!(result.raw_response_json.is_none());
    }

    #[test]
    fn responses_result_reads_gateway_wrapped_output() {
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
            "data": {
                "response": {
                    "output": [{
                        "type": "image_generation_call",
                        "result": "aGVsbG8=",
                    }]
                }
            }
        });

        let result =
            extract_openai_responses_result(&request, &response_json, Some("png")).unwrap();
        assert_eq!(result.images.len(), 1);
    }

    #[test]
    fn responses_sse_prefers_completed_response() {
        let stream = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"ZG9uZQ==\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}]}}\n\n",
            "data: [DONE]\n\n",
        );
        let payload = parse_openai_responses_event_stream(stream).unwrap();
        assert_eq!(payload["output"][0]["result"], "ZmluYWw=");
    }

    #[test]
    fn responses_sse_uses_done_item_without_completed_snapshot() {
        let stream = "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}}\n\n";
        let payload = parse_openai_responses_event_stream(stream).unwrap();
        assert_eq!(payload["output"][0]["result"], "ZmluYWw=");
    }

    #[test]
    fn responses_sse_falls_back_when_completed_result_is_empty() {
        let stream = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"result\":\"\"}]}}\n\n",
        );
        let payload = parse_openai_responses_event_stream(stream).unwrap();
        assert_eq!(payload["output"][0]["result"], "ZmluYWw=");
    }

    #[test]
    fn responses_sse_uses_latest_partial_as_last_resort() {
        let stream = concat!(
            "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0,\"partial_image_b64\":\"Zmlyc3Q=\"}\n\n",
            "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":1,\"partial_image_b64\":\"bGFzdA==\"}\n\n",
        );
        let payload = parse_openai_responses_event_stream(stream).unwrap();
        assert_eq!(payload["output"][0]["result"], "bGFzdA==");
    }

    #[test]
    fn responses_sse_accumulator_handles_arbitrary_chunk_boundaries() {
        let stream = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"YWJjZGVmZw==\"}}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        );
        let mut accumulator = OpenAiResponsesStreamAccumulator::new();
        for chunk in stream.as_bytes().chunks(3) {
            accumulator.push_chunk(chunk).unwrap();
        }
        assert!(accumulator.buffer.is_empty());

        let payload = accumulator.finish().unwrap();
        assert_eq!(payload["output"][0]["result"], "YWJjZGVmZw==");
    }

    #[test]
    fn responses_sse_accumulator_supports_multiline_data() {
        let stream = concat!(
            "data: {\"type\":\"response.output_item.done\",\r\n",
            "data: \"item\":{\"type\":\"image_generation_call\",\"result\":\"bXVsdGlsaW5l\"}}\r\n\r\n",
        );
        let mut accumulator = OpenAiResponsesStreamAccumulator::new();
        for byte in stream.as_bytes() {
            accumulator.push_chunk(std::slice::from_ref(byte)).unwrap();
        }

        let payload = accumulator.finish().unwrap();
        assert_eq!(payload["output"][0]["result"], "bXVsdGlsaW5l");
    }

    #[test]
    fn responses_sse_accumulator_releases_fallback_images_after_completion() {
        let stream = concat!(
            "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0,\"partial_image_b64\":\"cGFydGlhbA==\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"ZG9uZQ==\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}]}}\n\n",
        );
        let mut accumulator = OpenAiResponsesStreamAccumulator::new();
        accumulator.push_chunk(stream.as_bytes()).unwrap();

        assert!(accumulator.latest_partial_image.is_none());
        assert!(accumulator.completed_items.is_empty());
        let payload = accumulator.finish().unwrap();
        assert_eq!(payload["output"][0]["result"], "ZmluYWw=");
    }

    #[test]
    fn responses_sse_accumulator_surfaces_upstream_errors() {
        let stream =
            "data: {\"type\":\"response.failed\",\"error\":{\"message\":\"quota exceeded\"}}\n\n";
        let mut accumulator = OpenAiResponsesStreamAccumulator::new();
        let error = accumulator.push_chunk(stream.as_bytes()).unwrap_err();
        assert!(error.contains("quota exceeded"));
    }

    #[test]
    fn successful_task_payloads_are_removed_without_losing_image_count() {
        let mut tasks = vec![LocalTaskRecord {
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "test".into(),
            requested_model: "model".into(),
            reference_asset_ids: Vec::new(),
            generation_settings: None,
            result: Some(GenerationResult {
                images: vec![GeneratedImageResult {
                    url: Some("https://example.com/image.png".into()),
                    data_url: Some("data:image/png;base64,aGVsbG8=".into()),
                }],
                parameter_snapshot: ParameterSnapshot::default(),
                raw_response_json: Some(serde_json::json!({ "result": "aGVsbG8=" })),
            }),
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        }];

        let mut failed_tasks = tasks.clone();
        failed_tasks[0].status = TaskStatus::Failed;
        assert!(!strip_successful_task_payloads(&mut failed_tasks));
        assert!(
            failed_tasks[0].result.as_ref().unwrap().images[0]
                .data_url
                .is_some()
        );

        assert!(strip_successful_task_payloads(&mut tasks));

        let result = tasks[0].result.as_ref().unwrap();
        assert_eq!(result.images.len(), 1);
        assert!(result.images[0].url.is_none());
        assert!(result.images[0].data_url.is_none());
        assert!(result.raw_response_json.is_none());
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
            responses_model: None,
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
        assert_eq!(config.responses_model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn normalize_custom_nano_banana_keeps_native_protocol() {
        let mut config = EncryptedApiConfig {
            id: "config-2".into(),
            name: "旧香蕉中转".into(),
            provider_template_id: BUILTIN_NANO_BANANA_TEMPLATE_ID.into(),
            provider_kind: ProviderKind::NanoBanana,
            endpoint_mode: ProviderEndpointMode::CustomJson,
            base_url: "https://example.com".into(),
            model: String::new(),
            responses_model: None,
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
        assert_eq!(config.provider_kind, ProviderKind::NanoBanana);
        assert_eq!(config.provider_template_id, BUILTIN_NANO_BANANA_TEMPLATE_ID);
        assert_eq!(config.model, "gemini-2.5-flash-image");
    }
}
