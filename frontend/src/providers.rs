use gloo_net::http::Request;
use mew_image_shared::{
    BUILTIN_NANO_BANANA_TEMPLATE_ID, BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID,
    BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, EncryptedApiConfig, GenerateViaProxyRequest,
    GenerationRequest, GenerationResult, ImageAssetRef, LocalAppState, ProviderAccessMode,
    ProviderEndpointMode, ProviderKind, ProviderTemplate, SyncCheckpoint, SyncEnvelope,
    aspect_ratio_from_dimensions, extract_gemini_generation_result,
    extract_openai_compatible_result, extract_openai_responses_result, merge_envelopes,
    merge_records, nano_banana_image_size_from_dimensions, new_id, normalize_api_config,
    now_rfc3339,
};
use serde_json::json;

use crate::api::api_candidates;
use crate::crypto::{decrypt_secret, encrypt_secret};
use crate::{blob_from_bytes, reencode_asset_bytes};

const PROMPT_REWRITE_GUARD_PREFIX: &str =
    "Use the following text as the complete prompt. Do not rewrite it:";

#[derive(Clone)]
struct TransportAsset {
    meta: ImageAssetRef,
    bytes: Vec<u8>,
    mime_type: String,
}

pub fn default_config(template_id: &str) -> EncryptedApiConfig {
    let mut config = EncryptedApiConfig {
        id: new_id(),
        name: "默认配置".into(),
        provider_template_id: template_id.into(),
        provider_kind: ProviderKind::OpenAiImage,
        endpoint_mode: ProviderEndpointMode::ImagesApi,
        base_url: String::new(),
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
    match template_id {
        BUILTIN_NANO_BANANA_TEMPLATE_ID => {
            config.provider_kind = ProviderKind::NanoBanana;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            config.base_url = "https://generativelanguage.googleapis.com".into();
            config.model = "gemini-2.5-flash-image".into();
        }
        BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID => {
            config.provider_kind = ProviderKind::OpenAiCompatible;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
            config.base_url = String::new();
            config.model = "gemini-2.5-flash-image".into();
        }
        BUILTIN_OPENAI_IMAGE_TEMPLATE_ID => {
            config.provider_kind = ProviderKind::OpenAiImage;
            config.endpoint_mode = ProviderEndpointMode::ImagesApi;
            config.base_url = "https://api.openai.com".into();
            config.model = "gpt-image-1".into();
        }
        _ => {
            config.provider_kind = ProviderKind::CustomHttp;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
        }
    }
    normalize_api_config(&mut config);
    config
}

pub async fn load_templates() -> Result<Vec<ProviderTemplate>, String> {
    for url in api_candidates("/api/providers/templates") {
        match Request::get(&url)
            .credentials(web_sys::RequestCredentials::Include)
            .send()
            .await
        {
            Ok(response) if response.ok() => {
                return response.json().await.map_err(|error| error.to_string());
            }
            _ => {}
        }
    }
    Ok(vec![
        ProviderTemplate::builtin_openai(),
        ProviderTemplate::builtin_nano_banana(),
        ProviderTemplate::builtin_openai_compatible(),
    ])
}

pub fn prepare_sync_envelope(
    state: &LocalAppState,
    sync_secret: Option<&str>,
) -> Result<SyncEnvelope, String> {
    let mut configs = Vec::with_capacity(state.configs.len());
    for config in &state.configs {
        let mut config = config.clone();
        if let (Some(secret), Some(plaintext)) = (sync_secret, config.api_key_plaintext.clone()) {
            config.api_key_encrypted = Some(encrypt_secret(secret, &plaintext)?);
            config.api_key_hint = Some(mask_key(&plaintext));
        }
        config.api_key_plaintext = None;
        configs.push(config);
    }
    Ok(SyncEnvelope {
        schema_version: mew_image_shared::SYNC_SCHEMA_VERSION,
        updated_at: now_rfc3339(),
        configs,
        tasks: state.tasks.clone(),
        threads: state.threads.clone(),
        assets: state
            .assets
            .iter()
            .filter(|asset| !asset.metadata.contains_key("mask_base_asset_id"))
            .cloned()
            .collect(),
        preferences: state.preferences.clone(),
    })
}

pub fn hydrate_local_state(
    local: &LocalAppState,
    remote: SyncEnvelope,
    checkpoint: SyncCheckpoint,
    sync_secret: Option<&str>,
) -> LocalAppState {
    let local_envelope = SyncEnvelope {
        schema_version: mew_image_shared::SYNC_SCHEMA_VERSION,
        updated_at: now_rfc3339(),
        configs: local.configs.clone(),
        tasks: local.tasks.clone(),
        threads: local.threads.clone(),
        assets: local.assets.clone(),
        preferences: local.preferences.clone(),
    };
    let merged = merge_envelopes(&local_envelope, &remote);
    let mut configs = merge_records(&local.configs, &merged.configs);
    for config in &mut configs {
        normalize_api_config(config);
        if config.api_key_plaintext.is_some() {
            continue;
        }
        if let (Some(secret), Some(encrypted)) = (sync_secret, config.api_key_encrypted.clone()) {
            if let Ok(plaintext) = decrypt_secret(secret, &encrypted) {
                config.api_key_plaintext = Some(plaintext.clone());
                config.api_key_hint = Some(mask_key(&plaintext));
            }
        }
        if let Some(local_plaintext) = local
            .configs
            .iter()
            .find(|item| item.id == config.id)
            .and_then(|item| item.api_key_plaintext.clone())
        {
            config.api_key_plaintext = Some(local_plaintext.clone());
            config.api_key_hint = Some(mask_key(&local_plaintext));
        }
    }

    let mut assets = merge_records(&local.assets, &merged.assets);
    for asset in &mut assets {
        if asset.data_url.is_some() {
            continue;
        }
        if let Some(local_asset) = local.assets.iter().find(|item| item.id == asset.id) {
            if local_asset.data_url.is_some() {
                asset.data_url = local_asset.data_url.clone();
            }
        }
    }
    assets.retain(|asset| !asset.metadata.contains_key("mask_base_asset_id"));

    LocalAppState {
        configs,
        tasks: merged.tasks,
        threads: merged.threads,
        assets,
        preferences: merged.preferences,
        checkpoint,
    }
}

pub async fn generate_with_strategy(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> Result<(GenerationResult, bool), String> {
    if config.provider_kind == ProviderKind::NanoBanana {
        return match config.access_mode {
            ProviderAccessMode::Proxy => proxy_generate(template, config, request)
                .await
                .map(|result| (result, true)),
            ProviderAccessMode::Direct => direct_generate(template, config, request)
                .await
                .map(|result| (result, false)),
            ProviderAccessMode::Smart => match direct_generate(template, config, request).await {
                Ok(result) => Ok((result, false)),
                Err(_) => proxy_generate(template, config, request)
                    .await
                    .map(|result| (result, true)),
            },
        };
    }
    if config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        return match config.access_mode {
            ProviderAccessMode::Direct => direct_generate(template, config, request)
                .await
                .map(|result| (result, false)),
            ProviderAccessMode::Proxy => proxy_generate(template, config, request)
                .await
                .map(|result| (result, true)),
            ProviderAccessMode::Smart => match proxy_generate(template, config, request).await {
                Ok(result) => Ok((result, true)),
                Err(_) => direct_generate(template, config, request)
                    .await
                    .map(|result| (result, false)),
            },
        };
    }
    if !request.reference_assets.is_empty() {
        return proxy_generate(template, config, request)
            .await
            .map(|result| (result, true));
    }
    if matches!(config.access_mode, ProviderAccessMode::Smart) && config.known_requires_proxy {
        return proxy_generate(template, config, request)
            .await
            .map(|result| (result, true));
    }
    match config.access_mode {
        ProviderAccessMode::Proxy => proxy_generate(template, config, request)
            .await
            .map(|result| (result, true)),
        ProviderAccessMode::Direct => direct_generate(template, config, request)
            .await
            .map(|result| (result, false)),
        ProviderAccessMode::Smart => match direct_generate(template, config, request).await {
            Ok(result) => Ok((result, false)),
            Err(_) => proxy_generate(template, config, request)
                .await
                .map(|result| (result, true)),
        },
    }
}

async fn direct_generate(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> Result<GenerationResult, String> {
    let api_key = config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| "请先填写 API Key。".to_string())?;
    let url = if config.provider_kind == ProviderKind::NanoBanana {
        let model = normalize_google_image_model(&request.model);
        join_api_url(&config.base_url, &format!("/v1beta/models/{model}:generateContent"))
    } else {
        join_api_url(
            &config.base_url,
            direct_endpoint_path(template, config, request),
        )
    };
    let response = if config.provider_kind == ProviderKind::NanoBanana {
        let body = build_gemini_json(config, request);
        Request::post(&url)
            .header("x-goog-api-key", &api_key)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?
    } else if config.provider_kind == ProviderKind::OpenAiCompatible {
        if request.reference_assets.is_empty() {
            Request::post(&url)
                .header("Authorization", &format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .json(&build_openai_compatible_json(config, request))
                .map_err(|error| error.to_string())?
                .send()
                .await
                .map_err(|error| error.to_string())?
        } else {
            let prepared_assets = prepare_transport_assets(&request.reference_assets).await?;
            let form = web_sys::FormData::new().map_err(|error| format!("{error:?}"))?;
            form.append_with_str("prompt", &request.prompt)
                .map_err(|error| format!("{error:?}"))?;
            form.append_with_str("model", &request.model)
                .map_err(|error| format!("{error:?}"))?;
            form.append_with_str(
                "aspect_ratio",
                &aspect_ratio_from_dimensions(request.width, request.height),
            )
            .map_err(|error| format!("{error:?}"))?;
            form.append_with_str(
                "response_format",
                openai_compatible_response_format(request),
            )
            .map_err(|error| format!("{error:?}"))?;
            form.append_with_str(
                "image_size",
                &nano_banana_image_size_from_dimensions(request.width, request.height),
            )
            .map_err(|error| format!("{error:?}"))?;
            for asset in &prepared_assets {
                let blob = blob_from_bytes(&asset.bytes, &asset.mime_type)?;
                form.append_with_blob_and_filename(
                    "image",
                    &blob,
                    &format!("{}.{}", asset.meta.id, mime_extension(&asset.mime_type)),
                )
                .map_err(|error| format!("{error:?}"))?;
            }
            Request::post(&url)
                .header("Authorization", &format!("Bearer {api_key}"))
                .header("Accept", "application/json")
                .body(form)
                .map_err(|error| error.to_string())?
                .send()
                .await
                .map_err(|error| error.to_string())?
        }
    } else {
        let body = match config.provider_kind {
            ProviderKind::OpenAiImage => build_openai_json(config, request),
            ProviderKind::CustomHttp => build_custom_json(template, request),
            ProviderKind::NanoBanana | ProviderKind::OpenAiCompatible => {
                unreachable!("该服务商类型在上游分支已提前处理")
            }
        };

        let builder = Request::post(&url).header("Content-Type", "application/json");
        let builder = builder.header("Authorization", &format!("Bearer {api_key}"));
        builder
            .json(&body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?
    };

    if !response.ok() {
        return Err(response
            .text()
            .await
            .unwrap_or_else(|_| "上游请求失败".into()));
    }
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| error.to_string())?;
    extract_result(template, config, request, value)
}

async fn proxy_generate(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> Result<GenerationResult, String> {
    let config = config.clone();
    if config.api_key_plaintext.is_none() {
        return Err("代理模式也需要当前浏览器里已有 API Key。".into());
    }
    let reference_assets = prepare_transport_assets(&request.reference_assets).await?;
    let mut request_payload = request.clone();
    request_payload.reference_assets = Vec::new();
    let payload = GenerateViaProxyRequest {
        template: template.clone(),
        config,
        request: request_payload,
    };
    let mut errors = Vec::new();
    for url in api_candidates("/api/providers/generate") {
        let form = web_sys::FormData::new().map_err(|error| format!("{error:?}"))?;
        form.append_with_str(
            "payload",
            &serde_json::to_string(&payload).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("{error:?}"))?;
        form.append_with_str(
            "reference_assets_meta",
            &serde_json::to_string(
                &reference_assets
                    .iter()
                    .map(|asset| asset.meta.clone())
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("{error:?}"))?;
        for asset in &reference_assets {
            let blob = blob_from_bytes(&asset.bytes, &asset.mime_type)?;
            form.append_with_blob_and_filename(
                "reference_asset_files",
                &blob,
                &format!("{}.{}", asset.meta.id, mime_extension(&asset.mime_type)),
            )
            .map_err(|error| format!("{error:?}"))?;
        }
        let builder = Request::post(&url)
            .credentials(web_sys::RequestCredentials::Include)
            .body(form)
            .map_err(|error| error.to_string())?;
        match builder.send().await {
            Ok(response) if response.ok() => {
                return response.json().await.map_err(|error| error.to_string());
            }
            Ok(response) => {
                return Err(response
                    .text()
                    .await
                    .unwrap_or_else(|_| "代理生成失败".into()));
            }
            Err(error) => {
                errors.push(format!("{url} -> {error}"));
            }
        }
    }
    Err(format!(
        "代理不可用。带参考图生成必须经过 Rust 后端代理，请先启动后端：`cargo run -p mew-image-backend`。默认地址是 http://127.0.0.1:3000 。如果你当前只开了 `trunk serve` 或直接打开静态文件，那么纯文本生图还能直连，但带参考图的请求一定会失败。请求尝试记录：{}",
        if errors.is_empty() {
            "未知网络错误".into()
        } else {
            errors.join(" | ")
        }
    ))
}

async fn prepare_transport_assets(assets: &[ImageAssetRef]) -> Result<Vec<TransportAsset>, String> {
    let mut prepared = Vec::with_capacity(assets.len());
    for asset in assets {
        prepared.push(prepare_transport_asset(asset).await?);
    }
    Ok(prepared)
}

async fn prepare_transport_asset(asset: &ImageAssetRef) -> Result<TransportAsset, String> {
    let (bytes, mime_type, width, height) =
        reencode_asset_bytes(asset, "image/webp", Some(0.9)).await?;
    let mut meta = asset.clone();
    meta.mime_type = mime_type.clone();
    meta.byte_len = bytes.len() as u64;
    meta.width = Some(width);
    meta.height = Some(height);
    meta.data_url = None;
    meta.remote_object_key = None;
    meta.remote_url = None;
    Ok(TransportAsset {
        meta,
        bytes,
        mime_type,
    })
}

fn mime_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/webp" => "webp",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "bin",
    }
}

fn build_openai_json(
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> serde_json::Value {
    match config.endpoint_mode {
        ProviderEndpointMode::ResponsesApi => build_openai_responses_json(config, request),
        _ => json!({
            "prompt": request.prompt,
            "model": request.model,
            "size": format!("{}x{}", request.width, request.height),
            "quality": request.quality,
            "n": request.count,
            "output_format": config.output_format,
            "output_compression": config.output_compression,
            "moderation": config.moderation,
        }),
    }
}

fn build_openai_compatible_json(
    _config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> serde_json::Value {
    json!({
        "model": request.model,
        "prompt": request.prompt,
        "aspect_ratio": aspect_ratio_from_dimensions(request.width, request.height),
        "response_format": "url",
        "image_size": nano_banana_image_size_from_dimensions(request.width, request.height),
    })
}

fn openai_compatible_response_format(request: &GenerationRequest) -> &'static str {
    if request.reference_assets.is_empty() {
        "url"
    } else {
        // 中转站编辑接口实测更稳定地返回 base64，前端和后端都能直接解析。
        "b64_json"
    }
}

fn normalize_google_image_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return "gemini-2.5-flash-image".into();
    }
    if trimmed.starts_with("gemini-3.1-flash-image") && !trimmed.ends_with("-preview") {
        return format!("{trimmed}-preview");
    }
    if trimmed.starts_with("gemini-3-pro-image") && !trimmed.ends_with("-preview") {
        return format!("{trimmed}-preview");
    }
    trimmed.to_string()
}

fn google_image_size_value(model: &str, request: &GenerationRequest) -> Option<String> {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.contains("gemini-3") {
        Some(nano_banana_image_size_from_dimensions(
            request.width,
            request.height,
        ))
    } else {
        None
    }
}

fn build_openai_responses_json(
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> serde_json::Value {
    let prompt_text = if config.prompt_guard_enabled {
        format!("{PROMPT_REWRITE_GUARD_PREFIX}\n{}", request.prompt)
    } else {
        request.prompt.clone()
    };

    let input = if request.reference_assets.is_empty() {
        json!(prompt_text)
    } else {
        let mut content = vec![json!({
            "type": "input_text",
            "text": prompt_text,
        })];
        for asset in &request.reference_assets {
            if let Some(data_url) = asset.data_url.as_deref() {
                content.push(json!({
                    "type": "input_image",
                    "image_url": data_url,
                }));
            } else if let Some(url) = asset.remote_url.as_deref() {
                content.push(json!({
                    "type": "input_image",
                    "image_url": url,
                }));
            }
        }
        json!([{
            "role": "user",
            "content": content,
        }])
    };

    let mut tool = json!({
        "type": "image_generation",
        "action": if request.reference_assets.is_empty() { "generate" } else { "edit" },
        "size": format!("{}x{}", request.width, request.height),
        "output_format": config.output_format.clone().unwrap_or_else(|| "png".into()),
    });

    if !config.prompt_guard_enabled {
        if let Some(quality) = &request.quality {
            tool["quality"] = json!(quality);
        }
    }
    if config.output_format.as_deref() != Some("png") {
        if let Some(compression) = config.output_compression {
            tool["output_compression"] = json!(compression);
        }
    }

    json!({
        "model": resolve_responses_model(&request.model),
        "input": input,
        "tools": [tool],
        "tool_choice": "required",
    })
}

fn build_gemini_json(
    _config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> serde_json::Value {
    let model = normalize_google_image_model(&request.model);
    let mut parts = vec![json!({
        "text": request.prompt,
    })];
    for asset in &request.reference_assets {
        if let Some(data_url) = asset.data_url.as_deref() {
            if let Some((mime_type, data)) = split_data_url_payload(data_url) {
                parts.push(json!({
                    "inline_data": {
                        "mime_type": mime_type,
                        "data": data,
                    }
                }));
            }
        }
    }

    let mut generation_config = json!({
        "responseModalities": ["TEXT", "IMAGE"],
        "imageConfig": {
            "aspectRatio": aspect_ratio_from_dimensions(request.width, request.height),
        }
    });
    if let Some(image_size) = google_image_size_value(&model, request) {
        generation_config["imageConfig"]["imageSize"] = json!(image_size);
    }

    json!({
        "contents": [{
            "role": "user",
            "parts": parts,
        }],
        "generationConfig": generation_config,
    })
}

fn resolve_responses_model(request_model: &str) -> String {
    let trimmed = request_model.trim();
    if trimmed.is_empty() {
        return "gpt-5.5".into();
    }
    if trimmed.starts_with("gpt-image-") {
        return "gpt-5.5".into();
    }
    trimmed.to_string()
}

fn openai_images_endpoint(request: &GenerationRequest) -> &'static str {
    if !request.reference_assets.is_empty() {
        "/v1/images/edits"
    } else {
        "/v1/images/generations"
    }
}

fn join_api_url(base_url: &str, endpoint_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let endpoint = endpoint_path.trim_start_matches('/');
    let base = if base.ends_with("/v1") && endpoint.starts_with("v1/") {
        base.trim_end_matches("/v1")
    } else {
        base
    };
    format!("{base}/{endpoint}")
}

fn build_custom_json(
    template: &ProviderTemplate,
    request: &GenerationRequest,
) -> serde_json::Value {
    let mut body = json!({});
    set_json_path(
        &mut body,
        template.prompt_field.as_deref().unwrap_or("prompt"),
        json!(request.prompt),
    );
    set_json_path(
        &mut body,
        template.model_field.as_deref().unwrap_or("model"),
        json!(request.model),
    );
    set_json_path(
        &mut body,
        template.size_field.as_deref().unwrap_or("size"),
        json!(format!("{}x{}", request.width, request.height)),
    );
    body
}

fn extract_result(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
    response_json: serde_json::Value,
) -> Result<GenerationResult, String> {
    if config.provider_kind == ProviderKind::NanoBanana {
        return extract_gemini_generation_result(
            request,
            response_json,
            config.output_format.as_deref(),
        );
    }
    if config.provider_kind == ProviderKind::OpenAiCompatible {
        return extract_openai_compatible_result(
            request,
            response_json,
            config.output_format.as_deref(),
        );
    }
    if config.provider_kind == ProviderKind::OpenAiImage
        && request.endpoint_mode == ProviderEndpointMode::ResponsesApi
    {
        return extract_openai_responses_result(
            request,
            response_json,
            config.output_format.as_deref(),
        );
    }
    let urls = template
        .response_image_url_path
        .as_deref()
        .map(|path| collect_json_path(&response_json, path))
        .unwrap_or_default();
    let base64_images = template
        .response_image_base64_path
        .as_deref()
        .map(|path| collect_json_path(&response_json, path))
        .unwrap_or_default();

    let mut images = Vec::new();
    for value in urls {
        if let Some(url) = value.as_str() {
            images.push(mew_image_shared::GeneratedImageResult {
                url: Some(url.to_string()),
                data_url: None,
            });
        }
    }
    for value in base64_images {
        if let Some(raw) = value.as_str() {
            images.push(mew_image_shared::GeneratedImageResult {
                url: None,
                data_url: Some(format!("data:image/png;base64,{raw}")),
            });
        }
    }
    if images.is_empty() {
        return Err("接口返回里没有解析到图片结果，请检查模板路径。".into());
    }

    Ok(GenerationResult {
        images,
        parameter_snapshot: mew_image_shared::ParameterSnapshot {
            requested_width: Some(request.width),
            requested_height: Some(request.height),
            actual_width: Some(request.width),
            actual_height: Some(request.height),
            requested_quality: request.quality.clone(),
            actual_quality: request.quality.clone(),
            revised_prompt: template
                .response_revised_prompt_path
                .as_deref()
                .and_then(|path| collect_json_path(&response_json, path).into_iter().next())
                .and_then(|value| value.as_str().map(str::to_string)),
            duration_ms: None,
        },
        raw_response_json: Some(response_json),
    })
}

fn direct_endpoint_path<'a>(
    template: &'a ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> &'a str {
    match config.provider_kind {
        ProviderKind::OpenAiImage => match config.endpoint_mode {
            ProviderEndpointMode::ImagesApi => openai_images_endpoint(request),
            ProviderEndpointMode::ResponsesApi => "/v1/responses",
            ProviderEndpointMode::CustomJson => template.endpoint_path.as_str(),
        },
        ProviderKind::NanoBanana => {
            let _ = request;
            template.endpoint_path.as_str()
        }
        ProviderKind::OpenAiCompatible => {
            if request.reference_assets.is_empty() {
                "/v1/images/generations"
            } else {
                "/v1/images/edits"
            }
        }
        ProviderKind::CustomHttp => template.endpoint_path.as_str(),
    }
}

fn split_data_url_payload(data_url: &str) -> Option<(&str, &str)> {
    let (prefix, payload) = data_url.split_once(',')?;
    let mime_type = prefix
        .strip_prefix("data:")?
        .strip_suffix(";base64")?;
    Some((mime_type, payload))
}

fn set_json_path(target: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let mut current = target;
    let segments: Vec<&str> = path.split('.').collect();
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index == segments.len() - 1;
        if is_last {
            if let Some(object) = current.as_object_mut() {
                object.insert((*segment).to_string(), value.clone());
            }
            return;
        }
        if current.get(segment).is_none() {
            current[segment] = json!({});
        }
        current = &mut current[segment];
    }
}

fn collect_json_path(value: &serde_json::Value, path: &str) -> Vec<serde_json::Value> {
    fn walk(current: &serde_json::Value, parts: &[&str], output: &mut Vec<serde_json::Value>) {
        if parts.is_empty() {
            output.push(current.clone());
            return;
        }
        let part = parts[0];
        if let Some(key) = part.strip_suffix("[]") {
            if let Some(array) = current.get(key).and_then(|value| value.as_array()) {
                for item in array {
                    walk(item, &parts[1..], output);
                }
            }
            return;
        }
        if let Some((key, raw_index)) = part.split_once('[') {
            let index = raw_index
                .trim_end_matches(']')
                .parse::<usize>()
                .unwrap_or(0);
            if let Some(item) = current
                .get(key)
                .and_then(|value| value.as_array())
                .and_then(|array| array.get(index))
            {
                walk(item, &parts[1..], output);
            }
            return;
        }
        if let Some(next) = current.get(part) {
            walk(next, &parts[1..], output);
        }
    }

    let mut values = Vec::new();
    walk(value, &path.split('.').collect::<Vec<_>>(), &mut values);
    values
}

fn mask_key(value: &str) -> String {
    if value.len() <= 6 {
        return "******".into();
    }
    format!("{}***{}", &value[..3], &value[value.len() - 3..])
}
