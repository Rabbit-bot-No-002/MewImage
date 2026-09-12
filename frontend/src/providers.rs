use std::{cell::Cell, future::Future, pin::Pin, rc::Rc};

use gloo_net::http::Request;
use mew_image_shared::{
    BUILTIN_NANO_BANANA_TEMPLATE_ID, BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID,
    BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, EncryptedApiConfig, GenerateViaProxyRequest,
    GenerationRequest, GenerationResult, ImageAssetRef, LocalAppState, ProviderAccessMode,
    ProviderEndpointMode, ProviderKind, ProviderTemplate, ProxyGenerationJobAccepted,
    ProxyGenerationJobResponse, ProxyGenerationJobStatus, SyncCheckpoint, SyncEnvelope,
    aspect_ratio_from_dimensions, build_gemini_generation_request,
    extract_gemini_generation_result, extract_openai_compatible_result,
    extract_openai_responses_result, gemini_auth_header, gemini_generate_content_url,
    is_google_official_gemini_base_url, merge_envelopes, nano_banana_image_size_from_dimensions,
    new_id, normalize_api_config, normalized_image_output_format, normalized_openai_background,
    now_rfc3339, openai_output_compression, parse_openai_responses_event_stream,
    resolve_responses_main_model,
};
use serde_json::json;

use crate::api::{api_candidates, api_url};
use crate::app::{
    blob_from_bytes, decode_browser_data_url, reencode_asset_bytes, sha256_hex, strip_task_payloads,
};
use crate::crypto::{decrypt_secret, encrypt_secret};

const PROMPT_REWRITE_GUARD_PREFIX: &str =
    "Use the following text as the complete prompt. Do not rewrite it:";
const PROXY_GENERATION_POLL_INTERVAL_MS: u32 = 1_500;
const MAX_PROXY_POLL_NETWORK_FAILURES: u8 = 5;

type GenerationBudgetFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyGenerationPhase {
    LegacyProtected,
    ServerQueued { release_budget: bool },
    AwaitingUpstream,
    ResultReady,
    ReceivingResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyBudgetRequest {
    LegacyFullTask,
    Result { response_bytes: u64 },
}

#[derive(Clone)]
pub(crate) struct GenerationLifecycle {
    on_proxy_phase: Rc<dyn Fn(ProxyGenerationPhase)>,
    wait_for_budget: Rc<dyn Fn(ProxyBudgetRequest) -> GenerationBudgetFuture>,
    has_accumulated_results: Rc<Cell<bool>>,
    accumulated_response_bytes: Rc<Cell<u64>>,
}

impl GenerationLifecycle {
    pub(crate) fn new(
        on_proxy_phase: impl Fn(ProxyGenerationPhase) + 'static,
        wait_for_budget: impl Fn(ProxyBudgetRequest) -> GenerationBudgetFuture + 'static,
    ) -> Self {
        Self {
            on_proxy_phase: Rc::new(on_proxy_phase),
            wait_for_budget: Rc::new(wait_for_budget),
            has_accumulated_results: Rc::new(Cell::new(false)),
            accumulated_response_bytes: Rc::new(Cell::new(0)),
        }
    }

    fn set_proxy_phase(&self, phase: ProxyGenerationPhase) {
        (self.on_proxy_phase)(phase);
    }

    async fn reserve(&self, request: ProxyBudgetRequest) -> Result<(), String> {
        (self.wait_for_budget)(request).await
    }

    fn retain_accumulated_results(&self) {
        self.has_accumulated_results.set(true);
    }

    fn accumulate_response_bytes(&self, response_bytes: u64) -> u64 {
        let total = self
            .accumulated_response_bytes
            .get()
            .saturating_add(response_bytes);
        self.accumulated_response_bytes.set(total);
        total
    }
}

#[derive(Clone)]
struct TransportAsset {
    meta: ImageAssetRef,
    bytes: Vec<u8>,
    mime_type: String,
}

struct ProxyGenerationEndpoint {
    submit_url: String,
    supports_status_only: bool,
    supports_options_v2: bool,
    supports_image_editing: bool,
    supports_image_conversation: bool,
}

#[derive(Default, serde::Deserialize)]
struct ProxyHealthCapabilities {
    #[serde(default)]
    image_editing_v1: bool,
    #[serde(default)]
    image_generation_options_v2: bool,
    #[serde(default)]
    proxy_generation_status_only: bool,
    #[serde(default)]
    image_conversation_v1: bool,
}

#[derive(Default, serde::Deserialize)]
struct ProxyHealthResponse {
    #[serde(default)]
    capabilities: ProxyHealthCapabilities,
}

pub(crate) fn sync_requires_image_editing_capability(state: &LocalAppState) -> bool {
    state.tasks.iter().any(|task| task.editing.is_some())
}

pub(crate) async fn ensure_image_editing_sync_capability(
    state: &LocalAppState,
) -> Result<(), String> {
    if !sync_requires_image_editing_capability(state) {
        return Ok(());
    }

    let health_url = api_url("/api/health");
    let response = Request::get(&health_url)
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(|error| format!("检查同步服务编辑能力失败：{error}"))?;
    if !response.ok() {
        return Err(format!(
            "检查同步服务编辑能力失败：HTTP {}。",
            response.status()
        ));
    }
    let health = response
        .json::<ProxyHealthResponse>()
        .await
        .map_err(|error| format!("同步服务健康响应无法解析：{error}"))?;
    if !health.capabilities.image_editing_v1 {
        return Err("当前后端不支持图像编辑数据同步，请先将前后端同时升级后再同步。".into());
    }
    Ok(())
}

enum ProxyGenerationSubmission {
    Completed(GenerationResult),
    Accepted(String),
}

pub(crate) struct GenerationExecutionResult {
    pub(crate) result: GenerationResult,
    pub(crate) used_proxy: bool,
    pub(crate) pending_proxy_poll_urls: Vec<String>,
}

impl GenerationExecutionResult {
    fn direct(result: GenerationResult) -> Self {
        Self {
            result,
            used_proxy: false,
            pending_proxy_poll_urls: Vec::new(),
        }
    }

    fn proxied(result: GenerationResult, pending_poll_url: Option<String>) -> Self {
        Self {
            result,
            used_proxy: true,
            pending_proxy_poll_urls: pending_poll_url.into_iter().collect(),
        }
    }
}

#[derive(Default)]
struct GenerationResultAccumulator {
    images: Vec<mew_image_shared::GeneratedImageResult>,
    first_parameter_snapshot: Option<mew_image_shared::ParameterSnapshot>,
    first_upstream_response_id: Option<String>,
    used_proxy: bool,
    pending_proxy_poll_urls: Vec<String>,
}

impl GenerationResultAccumulator {
    fn push(&mut self, mut execution: GenerationExecutionResult) -> usize {
        self.used_proxy |= execution.used_proxy;
        if self.first_parameter_snapshot.is_none() {
            self.first_parameter_snapshot = Some(execution.result.parameter_snapshot);
            self.first_upstream_response_id = execution.result.upstream_response_id.take();
        }
        self.pending_proxy_poll_urls
            .append(&mut execution.pending_proxy_poll_urls);
        let produced_count = execution.result.images.len();
        compact_embedded_images_to_blobs(&mut execution.result.images);
        self.images.append(&mut execution.result.images);
        produced_count
    }

    fn finish(self) -> GenerationExecutionResult {
        GenerationExecutionResult {
            result: GenerationResult {
                images: self.images,
                parameter_snapshot: self.first_parameter_snapshot.unwrap_or_default(),
                upstream_response_id: self.first_upstream_response_id,
                raw_response_json: None,
            },
            used_proxy: self.used_proxy,
            pending_proxy_poll_urls: self.pending_proxy_poll_urls,
        }
    }
}

fn compact_embedded_images_to_blobs(images: &mut [mew_image_shared::GeneratedImageResult]) {
    for image in images {
        let Some(data_url) = image.data_url.as_deref() else {
            continue;
        };
        let Ok((mime_type, bytes)) = decode_browser_data_url(data_url) else {
            continue;
        };
        let Ok(blob) = blob_from_bytes(&bytes, &mime_type) else {
            continue;
        };
        let Ok(object_url) = web_sys::Url::create_object_url_with_blob(&blob) else {
            continue;
        };
        image.url = Some(object_url);
        image.data_url = None;
    }
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
        responses_model: None,
        access_mode: ProviderAccessMode::Smart,
        known_requires_proxy: true,
        output_format: Some("png".into()),
        output_compression: Some(100),
        background: None,
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
            config.model = "gpt-image-2".into();
        }
        _ => {
            config.provider_kind = ProviderKind::CustomHttp;
            config.endpoint_mode = ProviderEndpointMode::CustomJson;
        }
    }
    normalize_api_config(&mut config);
    config
}

pub(crate) fn generation_uses_proxy(
    config: &EncryptedApiConfig,
    has_reference_assets: bool,
) -> bool {
    if config.provider_kind == ProviderKind::NanoBanana {
        return config.access_mode == ProviderAccessMode::Proxy;
    }
    if config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        return config.access_mode != ProviderAccessMode::Direct;
    }
    has_reference_assets
        || config.access_mode == ProviderAccessMode::Proxy
        || (config.access_mode == ProviderAccessMode::Smart && config.known_requires_proxy)
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
    sync_api_keys: bool,
) -> Result<SyncEnvelope, String> {
    let encrypted_at = now_rfc3339();
    let mut configs = Vec::with_capacity(state.configs.len());
    for config in &state.configs {
        let mut config = config.clone();
        if sync_api_keys {
            if let (Some(secret), Some(plaintext)) = (sync_secret, config.api_key_plaintext.clone())
            {
                let encrypted_matches = config
                    .api_key_encrypted
                    .as_ref()
                    .and_then(|encrypted| decrypt_secret(secret, encrypted).ok())
                    .map(|decrypted| decrypted == plaintext)
                    .unwrap_or(false);
                if !encrypted_matches {
                    config.api_key_encrypted = Some(encrypt_secret(secret, &plaintext)?);
                    config.updated_at = encrypted_at.clone();
                }
                config.api_key_hint = Some(mask_key(&plaintext));
            }
        } else if config.api_key_encrypted.take().is_some() {
            config.api_key_hint = None;
            config.updated_at = encrypted_at.clone();
        }
        config.api_key_plaintext = None;
        configs.push(config);
    }
    let mut tasks = state.tasks.clone();
    strip_task_payloads(&mut tasks);
    Ok(SyncEnvelope {
        schema_version: mew_image_shared::SYNC_SCHEMA_VERSION,
        updated_at: now_rfc3339(),
        configs,
        tasks,
        threads: state.threads.clone(),
        assets: state
            .assets
            .iter()
            .filter(|asset| !asset.metadata.contains_key("mask_base_asset_id"))
            .cloned()
            .collect(),
        preferences: state.preferences.clone(),
        tombstones: state.tombstones.clone(),
    })
}

pub fn hydrate_local_state(
    local: &LocalAppState,
    remote: SyncEnvelope,
    checkpoint: SyncCheckpoint,
    sync_secret: Option<&str>,
    legacy_sync_secret: Option<&str>,
) -> LocalAppState {
    let local_envelope = SyncEnvelope {
        schema_version: mew_image_shared::SYNC_SCHEMA_VERSION,
        updated_at: now_rfc3339(),
        configs: local.configs.clone(),
        tasks: local.tasks.clone(),
        threads: local.threads.clone(),
        assets: local.assets.clone(),
        preferences: local.preferences.clone(),
        tombstones: local.tombstones.clone(),
    };
    let merged = merge_envelopes(&local_envelope, &remote);
    let mut configs = merged.configs.clone();
    for config in &mut configs {
        normalize_api_config(config);
        if config.api_key_plaintext.is_some() {
            continue;
        }
        if let Some(encrypted) = config.api_key_encrypted.clone() {
            let primary_plaintext = sync_secret
                .and_then(|secret| decrypt_secret(secret, &encrypted).ok())
                .map(|plaintext| (plaintext, false));
            let recovered = primary_plaintext.or_else(|| {
                legacy_sync_secret
                    .and_then(|secret| decrypt_secret(secret, &encrypted).ok())
                    .map(|plaintext| (plaintext, true))
            });
            if let Some((plaintext, used_legacy_secret)) = recovered {
                config.api_key_plaintext = Some(plaintext.clone());
                config.api_key_hint = Some(mask_key(&plaintext));
                if used_legacy_secret {
                    // 旧版本直接使用登录密码加密，下次同步时升级为可信设备密钥。
                    config.api_key_encrypted = None;
                    config.updated_at = now_rfc3339();
                }
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

    let mut assets = merged.assets.clone();
    for asset in &mut assets {
        if asset.data_url.is_some() {
            continue;
        }
        if let Some(local_asset) = local.assets.iter().find(|item| item.id == asset.id)
            && local_asset.data_url.is_some()
        {
            asset.data_url = local_asset.data_url.clone();
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
        tombstones: merged.tombstones,
    }
}

pub async fn generate_with_strategy(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
    abort_signal: Option<&web_sys::AbortSignal>,
    lifecycle: &GenerationLifecycle,
) -> Result<GenerationExecutionResult, String> {
    mew_image_shared::validate_image_generation_request(config, request)?;
    let requested_count = request.count.max(1);
    if requested_count <= 1 {
        return generate_once_with_strategy(template, config, request, abort_signal, lifecycle)
            .await;
    }

    let mut accumulated = GenerationResultAccumulator::default();
    let mut last_error = None;

    for _ in 0..requested_count {
        let remaining = requested_count.saturating_sub(accumulated.images.len() as u32);
        if remaining == 0 {
            break;
        }

        let mut next_request = request.clone();
        next_request.count = if config.provider_kind == ProviderKind::NanoBanana
            || config.endpoint_mode == ProviderEndpointMode::ResponsesApi
        {
            1
        } else {
            remaining
        };

        match generate_once_with_strategy(template, config, &next_request, abort_signal, lifecycle)
            .await
        {
            Ok(execution) => {
                let produced_count = accumulated.push(execution);
                lifecycle.retain_accumulated_results();
                if produced_count == 0 {
                    break;
                }
            }
            Err(error) => {
                last_error = Some(error);
                break;
            }
        }
    }

    if accumulated.images.is_empty() {
        // 没有可保存结果时无需保留已完成的代理任务，避免等到 TTL 才回收。
        remove_proxy_generation_jobs(accumulated.pending_proxy_poll_urls);
        return Err(last_error.unwrap_or_else(|| "上游没有返回任何可用图片结果。".into()));
    }

    Ok(accumulated.finish())
}

async fn generate_once_with_strategy(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
    abort_signal: Option<&web_sys::AbortSignal>,
    lifecycle: &GenerationLifecycle,
) -> Result<GenerationExecutionResult, String> {
    if config.provider_kind == ProviderKind::NanoBanana {
        return match config.access_mode {
            ProviderAccessMode::Proxy => {
                proxy_generate(template, config, request, abort_signal, lifecycle).await
            }
            ProviderAccessMode::Direct => direct_generate(template, config, request, abort_signal)
                .await
                .map(GenerationExecutionResult::direct),
            // Smart 只在发出请求前选择链路。请求一旦发送，失败后不能自动换链路，
            // 否则响应丢失时可能重复生成并产生二次计费。
            ProviderAccessMode::Smart => direct_generate(template, config, request, abort_signal)
                .await
                .map(GenerationExecutionResult::direct),
        };
    }
    if config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        return match config.access_mode {
            ProviderAccessMode::Direct => direct_generate(template, config, request, abort_signal)
                .await
                .map(GenerationExecutionResult::direct),
            ProviderAccessMode::Proxy | ProviderAccessMode::Smart => {
                proxy_generate(template, config, request, abort_signal, lifecycle).await
            }
        };
    }
    if !request.reference_assets.is_empty() {
        return proxy_generate(template, config, request, abort_signal, lifecycle).await;
    }
    if matches!(config.access_mode, ProviderAccessMode::Smart) && config.known_requires_proxy {
        return proxy_generate(template, config, request, abort_signal, lifecycle).await;
    }
    match config.access_mode {
        ProviderAccessMode::Proxy => {
            proxy_generate(template, config, request, abort_signal, lifecycle).await
        }
        ProviderAccessMode::Direct => direct_generate(template, config, request, abort_signal)
            .await
            .map(GenerationExecutionResult::direct),
        ProviderAccessMode::Smart => direct_generate(template, config, request, abort_signal)
            .await
            .map(GenerationExecutionResult::direct),
    }
}

async fn direct_generate(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
    abort_signal: Option<&web_sys::AbortSignal>,
) -> Result<GenerationResult, String> {
    let api_key = config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| "请先填写 API Key。".to_string())?;
    let gemini_model = if config.provider_kind == ProviderKind::NanoBanana {
        let model = if is_google_official_gemini_base_url(&config.base_url) {
            normalize_google_image_model(&request.model)
        } else {
            request.model.trim().to_string()
        };
        if model.is_empty() {
            return Err("当前配置缺少 Gemini 模型名称。".into());
        }
        Some(model)
    } else {
        None
    };
    let url = if config.provider_kind == ProviderKind::NanoBanana {
        gemini_generate_content_url(
            &config.base_url,
            gemini_model.as_deref().unwrap_or_default(),
        )
    } else {
        join_api_url(
            &config.base_url,
            direct_endpoint_path(template, config, request),
        )
    };
    let response = if config.provider_kind == ProviderKind::NanoBanana {
        let body = build_gemini_json(request, gemini_model.as_deref().unwrap_or_default());
        let (auth_header, auth_value) = gemini_auth_header(&config.base_url, &api_key);
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(auth_header, &auth_value)
            .json(&body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?
    } else if config.provider_kind == ProviderKind::OpenAiCompatible {
        if request.reference_assets.is_empty() {
            Request::post(&url)
                .abort_signal(abort_signal)
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
            form.append_with_str("n", &request.count.to_string())
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
                .abort_signal(abort_signal)
                .header("Authorization", &format!("Bearer {api_key}"))
                .header("Accept", "application/json")
                .body(form)
                .map_err(|error| error.to_string())?
                .send()
                .await
                .map_err(|error| error.to_string())?
        }
    } else {
        if config.provider_kind == ProviderKind::OpenAiImage
            && config.endpoint_mode == ProviderEndpointMode::ResponsesApi
            && let Some(mask) = request
                .editing
                .as_ref()
                .and_then(|editing| editing.mask.as_ref())
        {
            validated_responses_mask_data_url(mask)?;
        }
        let body = match config.provider_kind {
            ProviderKind::OpenAiImage => build_openai_json(config, request),
            ProviderKind::CustomHttp => build_custom_json(template, request),
            ProviderKind::NanoBanana | ProviderKind::OpenAiCompatible => {
                unreachable!("该服务商类型在上游分支已提前处理")
            }
        };

        let builder = Request::post(&url)
            .abort_signal(abort_signal)
            .header("Content-Type", "application/json");
        let builder = builder.header("Authorization", &format!("Bearer {api_key}"));
        builder
            .json(&body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?
    };

    if !response.ok() {
        let status = response.status();
        let request_id = response.headers().get("x-request-id");
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "上游请求失败".into());
        if config.provider_kind == ProviderKind::OpenAiImage {
            let request_id = request_id
                .as_deref()
                .map(|value| format!("，request_id={value}"))
                .unwrap_or_default();
            return Err(format!(
                "OpenAI 上游请求失败：HTTP {status}{request_id}，{body}"
            ));
        }
        return Err(body);
    }
    let value = if config.provider_kind == ProviderKind::OpenAiImage
        && config.endpoint_mode == ProviderEndpointMode::ResponsesApi
    {
        let is_event_stream = response
            .headers()
            .get("content-type")
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        let body = response.text().await.map_err(|error| error.to_string())?;
        if is_event_stream || body.trim_start().starts_with("data:") {
            parse_openai_responses_event_stream(&body)?
        } else {
            serde_json::from_str(&body).map_err(|error| error.to_string())?
        }
    } else {
        response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| error.to_string())?
    };
    extract_result(template, config, request, value)
}

async fn proxy_generate(
    template: &ProviderTemplate,
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
    abort_signal: Option<&web_sys::AbortSignal>,
    lifecycle: &GenerationLifecycle,
) -> Result<GenerationExecutionResult, String> {
    let config = config.clone();
    if config.api_key_plaintext.is_none() {
        return Err("代理模式也需要当前浏览器里已有 API Key。".into());
    }
    let endpoint = select_proxy_generation_endpoint(abort_signal).await?;
    let mut compatibility_request;
    let request = if request.previous_response_id.is_some() && !endpoint.supports_image_conversation
    {
        compatibility_request = request.clone();
        if let Some(prompt) = compatibility_request.compatibility_prompt.take() {
            compatibility_request.prompt = prompt;
        }
        compatibility_request.previous_response_id = None;
        &compatibility_request
    } else {
        request
    };
    if request.editing.is_some() && !endpoint.supports_image_editing {
        return Err(
            "后端尚不支持图片编辑协议，请同步更新前后端；未上传图片或发送生成请求。".into(),
        );
    }
    if (request.automatic_size || config.endpoint_mode == ProviderEndpointMode::ResponsesApi)
        && !endpoint.supports_options_v2
    {
        return Err("后端尚不支持新的图片参数协议，请更新后端后使用模型自动尺寸或 Responses 图片模型选择；未发送生成请求。".into());
    }
    if !endpoint.supports_status_only {
        lifecycle.set_proxy_phase(ProxyGenerationPhase::LegacyProtected);
        lifecycle
            .reserve(ProxyBudgetRequest::LegacyFullTask)
            .await?;
    }
    let mut reference_assets = Vec::with_capacity(request.reference_assets.len());
    let mut exact_blobs = std::collections::HashMap::new();
    let mut upload_bytes = 0;
    // 遮罩与普通参考图共用总量上限，先检查元数据，再按实际传输文件逐项复核。
    for asset in request
        .reference_assets
        .iter()
        .chain(request.editing.as_ref().and_then(|edit| edit.mask.as_ref()))
    {
        upload_bytes = checked_proxy_upload_bytes(upload_bytes, asset.byte_len)?;
    }
    upload_bytes = 0;
    for asset in &request.reference_assets {
        if request
            .editing
            .as_ref()
            .is_some_and(|edit| edit.mask.is_some() && edit.base_asset_id == asset.id)
        {
            let blob = validated_edit_blob(asset).await?;
            upload_bytes = checked_proxy_upload_bytes(upload_bytes, blob.size() as u64)?;
            exact_blobs.insert(asset.id.clone(), blob);
            let mut meta = asset.clone();
            meta.data_url = None;
            meta.remote_url = None;
            meta.remote_object_key = None;
            reference_assets.push(TransportAsset {
                meta,
                bytes: Vec::new(),
                mime_type: "image/png".into(),
            });
        } else {
            let prepared = prepare_transport_asset(asset).await?;
            upload_bytes = checked_proxy_upload_bytes(upload_bytes, prepared.bytes.len() as u64)?;
            reference_assets.push(prepared);
        }
    }
    let mask_blob = match request.editing.as_ref().and_then(|edit| edit.mask.as_ref()) {
        Some(mask) => {
            let blob = validated_edit_blob(mask).await?;
            checked_proxy_upload_bytes(upload_bytes, blob.size() as u64)?;
            Some(blob)
        }
        None => None,
    };
    let mut request_payload = request.clone();
    request_payload.reference_assets = Vec::new();
    if let Some(mask) = request_payload
        .editing
        .as_mut()
        .and_then(|edit| edit.mask.as_mut())
    {
        mask.data_url = None;
        mask.remote_url = None;
        mask.remote_object_key = None;
    }
    let payload = GenerateViaProxyRequest {
        template: template.clone(),
        config,
        request: request_payload,
    };
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
        let blob = match exact_blobs.remove(&asset.meta.id) {
            Some(blob) => blob,
            None => blob_from_bytes(&asset.bytes, &asset.mime_type)?,
        };
        form.append_with_blob_and_filename(
            "reference_asset_files",
            &blob,
            &format!("{}.{}", asset.meta.id, mime_extension(&asset.mime_type)),
        )
        .map_err(|error| format!("{error:?}"))?;
    }
    if let Some(mask) = mask_blob {
        form.append_with_blob_and_filename("mask_file", &mask, "mask.png")
            .map_err(|error| format!("遮罩加入上传失败：{error:?}"))?;
    }
    let response = Request::post(&endpoint.submit_url)
        .abort_signal(abort_signal)
        .credentials(web_sys::RequestCredentials::Include)
        .body(form)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| {
            format!(
                "代理生成请求发送后未收到响应：{error}。为避免重复生成和计费，本次不会自动切换端点重试。"
            )
        })?;
    // multipart 已被浏览器接管，尽早释放转码后的参考图字节。
    drop(reference_assets);
    if !response.ok() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "代理生成失败".into());
        return Err(proxy_error_message(&body, "代理生成失败"));
    }
    let body = response.text().await.map_err(|error| error.to_string())?;
    let synchronous_result = serde_json::from_str::<ProxyGenerationJobAccepted>(&body).is_err();
    if synchronous_result && endpoint.supports_status_only {
        // 升级期间若同源后端仍同步返回完整结果，也要在 JSON 解码前补领结果预算。
        lifecycle.set_proxy_phase(ProxyGenerationPhase::ResultReady);
        let response_bytes = lifecycle.accumulate_response_bytes(body.len() as u64);
        lifecycle
            .reserve(ProxyBudgetRequest::Result { response_bytes })
            .await?;
        lifecycle.set_proxy_phase(ProxyGenerationPhase::ReceivingResult);
    }
    match parse_proxy_generation_submission(&body)? {
        ProxyGenerationSubmission::Completed(result) => {
            Ok(GenerationExecutionResult::proxied(result, None))
        }
        ProxyGenerationSubmission::Accepted(job_id) => {
            if endpoint.supports_status_only {
                lifecycle.set_proxy_phase(ProxyGenerationPhase::ServerQueued {
                    release_budget: !lifecycle.has_accumulated_results.get(),
                });
            }
            poll_proxy_generation(&endpoint, &job_id, abort_signal, lifecycle).await
        }
    }
}

fn proxy_health_url(submit_url: &str) -> String {
    submit_url
        .strip_suffix("/api/providers/generate")
        .map(|prefix| format!("{prefix}/api/health"))
        .unwrap_or_else(|| "/api/health".into())
}

async fn select_proxy_generation_endpoint(
    abort_signal: Option<&web_sys::AbortSignal>,
) -> Result<ProxyGenerationEndpoint, String> {
    let mut errors = Vec::new();
    for submit_url in api_candidates("/api/providers/generate") {
        if abort_signal.is_some_and(web_sys::AbortSignal::aborted) {
            return Err("当前生成任务已停止。".into());
        }
        let health_url = proxy_health_url(&submit_url);
        match Request::get(&health_url)
            .abort_signal(abort_signal)
            .credentials(web_sys::RequestCredentials::Include)
            .send()
            .await
        {
            Ok(response) if response.ok() => {
                let capabilities = response
                    .json::<ProxyHealthResponse>()
                    .await
                    .map(|health| health.capabilities)
                    .unwrap_or_default();
                return Ok(ProxyGenerationEndpoint {
                    submit_url,
                    supports_status_only: capabilities.proxy_generation_status_only,
                    supports_options_v2: capabilities.image_generation_options_v2,
                    supports_image_editing: capabilities.image_editing_v1,
                    supports_image_conversation: capabilities.image_conversation_v1,
                });
            }
            Ok(response) => errors.push(format!("{health_url} -> HTTP {}", response.status())),
            Err(error) => errors.push(format!("{health_url} -> {error}")),
        }
    }
    Err(format!(
        "代理不可用。请先启动 Rust 后端：`cargo run -p mew-image-backend`，并优先通过 http://127.0.0.1:3000 访问页面。游客可使用通过公网地址校验的 HTTPS 标准图像上游；若部署者主动开启了域名白名单，请确认中转站域名已获允许。健康检查记录：{}",
        if errors.is_empty() {
            "未知网络错误".into()
        } else {
            errors.join(" | ")
        }
    ))
}

fn parse_proxy_generation_submission(body: &str) -> Result<ProxyGenerationSubmission, String> {
    // 兼容升级期间仍返回同步结果的旧版后端。
    if let Ok(result) = serde_json::from_str::<GenerationResult>(body) {
        return Ok(ProxyGenerationSubmission::Completed(result));
    }
    serde_json::from_str::<ProxyGenerationJobAccepted>(body)
        .map(|accepted| ProxyGenerationSubmission::Accepted(accepted.job_id))
        .map_err(|error| format!("代理任务响应解析失败：{error}"))
}

async fn poll_proxy_generation(
    endpoint: &ProxyGenerationEndpoint,
    job_id: &str,
    abort_signal: Option<&web_sys::AbortSignal>,
    lifecycle: &GenerationLifecycle,
) -> Result<GenerationExecutionResult, String> {
    let poll_url = format!("{}/{}", endpoint.submit_url.trim_end_matches('/'), job_id);
    let status_url = if endpoint.supports_status_only {
        format!("{poll_url}?status_only=true")
    } else {
        poll_url.clone()
    };
    let mut consecutive_network_failures = 0_u8;
    loop {
        if abort_signal.is_some_and(web_sys::AbortSignal::aborted) {
            remove_proxy_generation_job(poll_url);
            return Err("当前生成任务已停止。".into());
        }
        gloo_timers::future::TimeoutFuture::new(PROXY_GENERATION_POLL_INTERVAL_MS).await;

        let response = Request::get(&status_url)
            .abort_signal(abort_signal)
            .credentials(web_sys::RequestCredentials::Include)
            .send()
            .await;
        let response = match response {
            Ok(response) => {
                consecutive_network_failures = 0;
                response
            }
            Err(error) => {
                if abort_signal.is_some_and(web_sys::AbortSignal::aborted) {
                    remove_proxy_generation_job(poll_url);
                    return Err("当前生成任务已停止。".into());
                }
                consecutive_network_failures = consecutive_network_failures.saturating_add(1);
                if consecutive_network_failures < MAX_PROXY_POLL_NETWORK_FAILURES {
                    continue;
                }
                remove_proxy_generation_job(poll_url);
                return Err(format!("代理任务状态查询失败：{error}"));
            }
        };
        if !response.ok() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "代理任务状态查询失败".into());
            remove_proxy_generation_job(poll_url);
            return Err(proxy_error_message(&body, "代理任务状态查询失败"));
        }

        let job = match response.json::<ProxyGenerationJobResponse>().await {
            Ok(job) => job,
            Err(error) => {
                remove_proxy_generation_job(poll_url);
                return Err(format!("代理任务状态解析失败：{error}"));
            }
        };
        match job.status {
            ProxyGenerationJobStatus::Queued => {
                if endpoint.supports_status_only {
                    lifecycle.set_proxy_phase(ProxyGenerationPhase::ServerQueued {
                        release_budget: !lifecycle.has_accumulated_results.get(),
                    });
                }
                continue;
            }
            ProxyGenerationJobStatus::Running => {
                if endpoint.supports_status_only {
                    lifecycle.set_proxy_phase(ProxyGenerationPhase::AwaitingUpstream);
                }
                continue;
            }
            ProxyGenerationJobStatus::Succeeded => {
                let result = if endpoint.supports_status_only {
                    lifecycle.set_proxy_phase(ProxyGenerationPhase::ResultReady);
                    let response_bytes = lifecycle
                        .accumulate_response_bytes(job.result_byte_len.unwrap_or_default());
                    if let Err(error) = lifecycle
                        .reserve(ProxyBudgetRequest::Result { response_bytes })
                        .await
                    {
                        remove_proxy_generation_job(poll_url);
                        return Err(error);
                    }
                    lifecycle.set_proxy_phase(ProxyGenerationPhase::ReceivingResult);
                    match fetch_proxy_generation_result(&poll_url, abort_signal).await {
                        Ok(result) => result,
                        Err(error) => {
                            remove_proxy_generation_job(poll_url);
                            return Err(error);
                        }
                    }
                } else {
                    job.result
                        .ok_or_else(|| "代理任务缺少生成结果。".to_string())?
                };
                // 成功结果由调用方完成 IndexedDB 与任务状态持久化后再确认删除。
                return Ok(GenerationExecutionResult::proxied(result, Some(poll_url)));
            }
            ProxyGenerationJobStatus::Failed => {
                let error = job.error.unwrap_or_else(|| "代理生成失败。".into());
                remove_proxy_generation_job(poll_url);
                return Err(error);
            }
        }
    }
}

async fn fetch_proxy_generation_result(
    poll_url: &str,
    abort_signal: Option<&web_sys::AbortSignal>,
) -> Result<GenerationResult, String> {
    let response = Request::get(poll_url)
        .abort_signal(abort_signal)
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(|error| format!("领取代理生成结果失败：{error}"))?;
    if !response.ok() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "领取代理生成结果失败".into());
        return Err(proxy_error_message(&body, "领取代理生成结果失败"));
    }
    let job = response
        .json::<ProxyGenerationJobResponse>()
        .await
        .map_err(|error| format!("代理生成结果解析失败：{error}"))?;
    if job.status != ProxyGenerationJobStatus::Succeeded {
        return Err(job.error.unwrap_or_else(|| "代理生成结果尚未就绪。".into()));
    }
    job.result.ok_or_else(|| "代理任务缺少生成结果。".into())
}

fn remove_proxy_generation_job(poll_url: String) {
    remove_proxy_generation_jobs(vec![poll_url]);
}

pub(crate) fn remove_proxy_generation_jobs(poll_urls: Vec<String>) {
    if poll_urls.is_empty() {
        return;
    }

    // DELETE 只是结果确认/回收信号，不应让失败、取消或完成收尾被网络状态阻塞。
    wasm_bindgen_futures::spawn_local(async move {
        for poll_url in poll_urls {
            let _ = Request::delete(&poll_url)
                .credentials(web_sys::RequestCredentials::Include)
                .send()
                .await;
        }
    });
}

fn proxy_error_message(body: &str, fallback: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_string))
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            let message = body.trim();
            if message.is_empty() {
                fallback.into()
            } else {
                message.into()
            }
        })
}

fn checked_proxy_upload_bytes(current: u64, file_bytes: u64) -> Result<u64, String> {
    if file_bytes > 32 * 1024 * 1024 {
        return Err("单张参考图或遮罩不能超过 32 MiB。".into());
    }
    let total = current
        .checked_add(file_bytes)
        .filter(|total| *total <= 160 * 1024 * 1024)
        .ok_or_else(|| "参考图与遮罩的总大小不能超过 160 MiB。".to_string())?;
    Ok(total)
}

async fn validated_edit_blob(asset: &ImageAssetRef) -> Result<web_sys::Blob, String> {
    let blob = crate::storage::load_edit_asset_blob(&asset.id).await?;
    if blob.size() != asset.byte_len as f64
        || blob.size() > 32.0 * 1024.0 * 1024.0
        || blob.type_() != "image/png"
        || asset.mime_type != "image/png"
    {
        return Err("编辑输入 PNG 类型或大小与记录不一致。".into());
    }
    let buffer = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
        .await
        .map_err(|error| format!("读取编辑原图失败：{error:?}"))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || sha256_hex(&bytes) != asset.sha256 {
        return Err("编辑原图 PNG 魔数或 SHA 校验失败。".into());
    }
    Ok(blob)
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
    let meta = transport_asset_meta(asset, &bytes, &mime_type, width, height);
    Ok(TransportAsset {
        meta,
        bytes,
        mime_type,
    })
}

fn transport_asset_meta(
    asset: &ImageAssetRef,
    bytes: &[u8],
    mime_type: &str,
    width: u32,
    height: u32,
) -> ImageAssetRef {
    let mut meta = asset.clone();
    // 参考图在传输前会重新编码，完整性元数据必须对应实际发送的字节。
    meta.sha256 = sha256_hex(bytes);
    meta.mime_type = mime_type.to_string();
    meta.byte_len = bytes.len() as u64;
    meta.width = Some(width);
    meta.height = Some(height);
    meta.data_url = None;
    meta.remote_object_key = None;
    meta.remote_url = None;
    meta
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
        _ => {
            let mut body = json!({
                "prompt": request.prompt,
                "model": request.model,
                "size": request.openai_size(),
                "quality": request.quality,
                "n": request.count,
                "output_format": normalized_image_output_format(config.output_format.as_deref()),
                "background": normalized_openai_background(config.background.as_deref()),
                "moderation": config.moderation,
            });
            if let Some(compression) = openai_output_compression(
                config.output_format.as_deref(),
                config.output_compression,
            ) {
                body["output_compression"] = json!(compression);
            }
            body
        }
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
        "size": request.openai_size(),
        "n": request.count,
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

fn transport_image_data_url(asset: &ImageAssetRef) -> Option<&str> {
    asset.data_url.as_deref().filter(|value| {
        let Some((header, payload)) = value.trim().split_once(',') else {
            return false;
        };
        header.starts_with("data:image/")
            && header.ends_with(";base64")
            && !payload.trim().is_empty()
    })
}

fn validated_responses_mask_data_url(mask: &ImageAssetRef) -> Result<&str, String> {
    let data_url =
        transport_image_data_url(mask).ok_or("Responses 编辑遮罩原图尚未加载，未发送请求。")?;
    let (mime_type, bytes) = decode_browser_data_url(data_url)?;
    if mime_type != "image/png"
        || bytes.len() as u64 != mask.byte_len
        || sha256_hex(&bytes) != mask.sha256
    {
        return Err("Responses 编辑遮罩发送前校验失败，未发送请求。".into());
    }
    Ok(data_url)
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
            if let Some(data_url) = transport_image_data_url(asset) {
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
        "model": request.model,
        "action": if request.previous_response_id.is_some() || !request.reference_assets.is_empty() {
            "edit"
        } else {
            "generate"
        },
        "size": request.openai_size(),
        "output_format": normalized_image_output_format(config.output_format.as_deref()),
        "background": normalized_openai_background(config.background.as_deref()),
        "moderation": config.moderation.clone().unwrap_or_else(|| "auto".into()),
        "partial_images": 1,
    });

    if let Some(editing) = &request.editing
        && let Some(mask) = &editing.mask
        && let Some(data_url) = transport_image_data_url(mask)
    {
        tool["input_image_mask"] = json!({ "image_url": data_url });
        tool["action"] = json!("edit");
    }

    if let Some(quality) = &request.quality {
        tool["quality"] = json!(quality);
    }
    if let Some(compression) =
        openai_output_compression(config.output_format.as_deref(), config.output_compression)
    {
        tool["output_compression"] = json!(compression);
    }

    let mut body = json!({
        "model": resolve_responses_main_model(config, &request.model),
        "input": input,
        "tools": [tool],
        "tool_choice": "required",
        "stream": true,
    });
    if let Some(response_id) = &request.previous_response_id {
        body["previous_response_id"] = json!(response_id);
    }
    body
}

fn build_gemini_json(request: &GenerationRequest, model: &str) -> serde_json::Value {
    let data_urls = request
        .reference_assets
        .iter()
        .filter_map(transport_image_data_url)
        .collect::<Vec<_>>();
    build_gemini_generation_request(request, model, &data_urls)
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
        json!(request.openai_size()),
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
            &response_json,
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
            requested_width: (!request.automatic_size).then_some(request.width),
            requested_height: (!request.automatic_size).then_some(request.height),
            actual_width: (!request.automatic_size).then_some(request.width),
            actual_height: (!request.automatic_size).then_some(request.height),
            requested_quality: request.quality.clone(),
            actual_quality: request.quality.clone(),
            revised_prompt: template
                .response_revised_prompt_path
                .as_deref()
                .and_then(|path| collect_json_path(&response_json, path).into_iter().next())
                .and_then(|value| value.as_str().map(str::to_string)),
            duration_ms: None,
        },
        upstream_response_id: None,
        // 图片已提取到 images，避免成功任务重复持有完整 Base64 JSON。
        raw_response_json: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{
        ImageEditingMode, ImageEditingSnapshot, LocalTaskRecord, ParameterSnapshot, SyncEntityKind,
        SyncTombstone, TaskStatus,
    };

    fn test_reference_asset(data_url: Option<&str>, remote_url: Option<&str>) -> ImageAssetRef {
        ImageAssetRef {
            id: "reference-1".into(),
            sha256: "hash".into(),
            mime_type: "image/png".into(),
            byte_len: 1,
            width: Some(1),
            height: Some(1),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: data_url.map(str::to_string),
            remote_object_key: None,
            remote_url: remote_url.map(str::to_string),
            source_task_id: None,
            metadata: Default::default(),
        }
    }

    fn test_task(editing: Option<ImageEditingSnapshot>) -> LocalTaskRecord {
        LocalTaskRecord {
            editing,
            id: "task-1".into(),
            thread_id: "thread-1".into(),
            config_id: "config-1".into(),
            prompt: "prompt".into(),
            requested_model: "gpt-image-2.5-flare".into(),
            reference_asset_ids: Vec::new(),
            conversation: None,
            generation_settings: None,
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            source_gallery_template_id: None,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    #[test]
    fn proxy_submission_parser_accepts_job_and_legacy_result() {
        let accepted = parse_proxy_generation_submission(r#"{"job_id":"job-1"}"#).unwrap();
        assert!(matches!(
            accepted,
            ProxyGenerationSubmission::Accepted(job_id) if job_id == "job-1"
        ));

        let expected = GenerationResult {
            images: Vec::new(),
            parameter_snapshot: ParameterSnapshot::default(),
            upstream_response_id: None,
            raw_response_json: None,
        };
        let body = serde_json::to_string(&expected).unwrap();
        let completed = parse_proxy_generation_submission(&body).unwrap();
        assert!(matches!(
            completed,
            ProxyGenerationSubmission::Completed(result) if result == expected
        ));
    }

    #[test]
    fn transport_metadata_hashes_the_reencoded_bytes() {
        let source = test_reference_asset(
            Some("data:image/png;base64,AA=="),
            Some("https://example.test/reference.png"),
        );
        let bytes = b"reencoded-webp";

        let meta = transport_asset_meta(&source, bytes, "image/webp", 640, 480);

        assert_eq!(meta.id, source.id);
        assert_eq!(meta.sha256, sha256_hex(bytes));
        assert_eq!(meta.mime_type, "image/webp");
        assert_eq!(meta.byte_len, bytes.len() as u64);
        assert_eq!((meta.width, meta.height), (Some(640), Some(480)));
        assert!(meta.data_url.is_none());
        assert!(meta.remote_object_key.is_none());
        assert!(meta.remote_url.is_none());
    }

    #[test]
    fn proxy_health_probe_preserves_submit_endpoint_origin() {
        assert_eq!(
            proxy_health_url("http://127.0.0.1:3000/api/providers/generate"),
            "http://127.0.0.1:3000/api/health"
        );
        assert_eq!(proxy_health_url("/api/providers/generate"), "/api/health");
    }

    #[test]
    fn proxy_capability_defaults_off_for_old_health_responses() {
        let old: ProxyHealthResponse = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        let current: ProxyHealthResponse = serde_json::from_str(
            r#"{"ok":true,"capabilities":{"proxy_generation_status_only":true,"image_generation_options_v2":true,"image_editing_v1":true,"image_conversation_v1":true}}"#,
        )
        .unwrap();

        assert!(!old.capabilities.proxy_generation_status_only);
        assert!(!old.capabilities.image_generation_options_v2);
        assert!(!old.capabilities.image_editing_v1);
        assert!(!old.capabilities.image_conversation_v1);
        assert!(current.capabilities.image_editing_v1);
        assert!(current.capabilities.image_generation_options_v2);
        assert!(current.capabilities.proxy_generation_status_only);
        assert!(current.capabilities.image_conversation_v1);
    }

    #[test]
    fn sync_only_requires_editing_capability_for_persisted_edit_tasks() {
        let mut state = LocalAppState::default();
        state.tasks.push(test_task(None));
        assert!(!sync_requires_image_editing_capability(&state));

        state.tasks.push(test_task(Some(ImageEditingSnapshot {
            mode: ImageEditingMode::Mask,
            base_asset_id: "base-1".into(),
            mask_asset_id: Some("mask-1".into()),
            instruction: None,
        })));
        assert!(sync_requires_image_editing_capability(&state));
    }

    #[test]
    fn proxy_upload_budget_includes_mask_and_rejects_overflow() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(
            checked_proxy_upload_bytes(128 * MIB, 32 * MIB).unwrap(),
            160 * MIB
        );
        assert!(checked_proxy_upload_bytes(160 * MIB, 1).is_err());
        assert!(checked_proxy_upload_bytes(0, 32 * MIB + 1).is_err());
        assert!(checked_proxy_upload_bytes(u64::MAX, 1).is_err());
        let mut total = 0;
        for _ in 0..10 {
            total = checked_proxy_upload_bytes(total, 16 * MIB).unwrap();
        }
        assert!(checked_proxy_upload_bytes(total, 1024).is_err());
    }

    #[test]
    fn generation_route_prediction_preserves_direct_and_proxy_modes() {
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.access_mode = ProviderAccessMode::Direct;
        config.known_requires_proxy = false;
        assert!(!generation_uses_proxy(&config, false));
        assert!(generation_uses_proxy(&config, true));

        config.access_mode = ProviderAccessMode::Proxy;
        assert!(generation_uses_proxy(&config, false));

        config.access_mode = ProviderAccessMode::Smart;
        config.known_requires_proxy = true;
        assert!(generation_uses_proxy(&config, false));

        config.provider_kind = ProviderKind::NanoBanana;
        config.access_mode = ProviderAccessMode::Smart;
        assert!(!generation_uses_proxy(&config, true));

        config.provider_kind = ProviderKind::OpenAiImage;
        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        assert!(generation_uses_proxy(&config, false));
    }

    #[test]
    fn lifecycle_accumulates_multi_batch_response_bytes() {
        let lifecycle = GenerationLifecycle::new(|_| {}, |_| Box::pin(async { Ok(()) }));

        assert_eq!(lifecycle.accumulate_response_bytes(10), 10);
        assert_eq!(lifecycle.accumulate_response_bytes(25), 35);
    }

    #[test]
    fn generation_result_accumulator_keeps_all_pending_proxy_confirmations() {
        let result = |marker: u32| GenerationResult {
            images: vec![mew_image_shared::GeneratedImageResult {
                url: Some(format!("https://example.test/{marker}.png")),
                data_url: None,
            }],
            parameter_snapshot: mew_image_shared::ParameterSnapshot {
                requested_width: Some(marker),
                ..Default::default()
            },
            upstream_response_id: None,
            raw_response_json: Some(serde_json::json!({ "large": marker })),
        };
        let direct = GenerationExecutionResult::direct(result(1));
        assert!(!direct.used_proxy);
        assert!(direct.pending_proxy_poll_urls.is_empty());

        let mut accumulated = GenerationResultAccumulator::default();
        assert_eq!(accumulated.push(direct), 1);
        assert_eq!(
            accumulated.push(GenerationExecutionResult::proxied(
                result(2),
                Some("/api/providers/generate/job-2".into()),
            )),
            1
        );
        assert_eq!(
            accumulated.push(GenerationExecutionResult::proxied(
                result(3),
                Some("/api/providers/generate/job-3".into()),
            )),
            1
        );

        let execution = accumulated.finish();
        assert!(execution.used_proxy);
        assert_eq!(execution.result.images.len(), 3);
        assert_eq!(execution.result.parameter_snapshot.requested_width, Some(1));
        assert!(execution.result.raw_response_json.is_none());
        assert_eq!(
            execution.pending_proxy_poll_urls,
            [
                "/api/providers/generate/job-2",
                "/api/providers/generate/job-3"
            ]
        );
    }

    #[test]
    fn proxy_error_prefers_backend_json_message() {
        assert_eq!(
            proxy_error_message(r#"{"error":"任务队列已满"}"#, "代理失败"),
            "任务队列已满"
        );
        assert_eq!(
            proxy_error_message("openresty 504", "代理失败"),
            "openresty 504"
        );
    }

    #[test]
    fn responses_request_keeps_quality_with_prompt_guard() {
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        config.prompt_guard_enabled = true;
        config.responses_model = Some("gpt-5.6".into());
        config.background = Some("transparent".into());
        let request = GenerationRequest {
            editing: None,
            automatic_size: false,
            prompt: "test".into(),
            compatibility_prompt: None,
            model: "gpt-image-2".into(),
            width: 3840,
            height: 2160,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            previous_response_id: None,
            reference_assets: Vec::new(),
        };

        let body = build_openai_responses_json(&config, &request);
        assert_eq!(body["model"], "gpt-5.6");
        assert_eq!(body["tools"][0]["size"], "3840x2160");
        assert_eq!(body["tools"][0]["quality"], "high");
        assert_eq!(body["tools"][0]["model"], "gpt-image-2");
        assert_eq!(body["tools"][0]["background"], "transparent");
        assert!(body["tools"][0].get("output_compression").is_none());
        let mut continued_request = request.clone();
        continued_request.previous_response_id = Some("resp_previous".into());
        let continued = build_openai_responses_json(&config, &continued_request);
        assert_eq!(continued["previous_response_id"], "resp_previous");
        assert_eq!(continued["tools"][0]["action"], "edit");
        let mut automatic_request = request;
        automatic_request.automatic_size = true;
        automatic_request.model = "gpt-image-2.5-sunburst".into();
        automatic_request.quality = Some("max".into());
        let automatic = build_openai_responses_json(&config, &automatic_request);
        assert_eq!(automatic["tools"][0]["size"], "auto");
        assert_eq!(automatic["tools"][0]["model"], "gpt-image-2.5-sunburst");
        assert_eq!(automatic["tools"][0]["quality"], "max");
        assert_eq!(automatic["model"], "gpt-5.6");
        config.endpoint_mode = ProviderEndpointMode::ImagesApi;
        let images = build_openai_json(&config, &automatic_request);
        assert_eq!(images["size"], "auto");
        assert!(images.get("response_format").is_none());
    }

    #[test]
    fn responses_request_never_sends_runtime_blob_url_upstream() {
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        let request = GenerationRequest {
            editing: None,
            automatic_size: false,
            prompt: "test".into(),
            compatibility_prompt: None,
            model: "gpt-image-2".into(),
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            previous_response_id: None,
            reference_assets: vec![test_reference_asset(
                Some("blob:http://127.0.0.1/runtime-only"),
                Some("https://example.test/reference.png"),
            )],
        };

        let body = build_openai_responses_json(&config, &request);
        let serialized = body.to_string();
        assert!(!serialized.contains("blob:"));
        assert!(serialized.contains("https://example.test/reference.png"));
    }

    #[test]
    fn direct_responses_edit_keeps_mask_separate_from_input_images() {
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        config.responses_model = Some("gpt-5.6".into());
        let base = test_reference_asset(Some("data:image/png;base64,YmFzZQ=="), None);
        let mut mask = test_reference_asset(Some("data:image/png;base64,bWFzaw=="), None);
        mask.id = "mask".into();
        mask.byte_len = 4;
        mask.sha256 = sha256_hex(b"mask");
        mask.metadata
            .insert("asset_role".into(), mew_image_shared::EDIT_MASK_ROLE.into());
        let request = GenerationRequest {
            editing: Some(mew_image_shared::ImageEditingInput {
                mode: mew_image_shared::ImageEditingMode::Mask,
                base_asset_id: base.id.clone(),
                mask: Some(mask),
                instruction: None,
            }),
            automatic_size: false,
            prompt: "edit".into(),
            compatibility_prompt: None,
            model: "gpt-image-2.5-flare".into(),
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            previous_response_id: None,
            reference_assets: vec![base],
        };
        let body = build_openai_responses_json(&config, &request);
        assert_eq!(body["model"], "gpt-5.6");
        assert_eq!(body["tools"][0]["model"], "gpt-image-2.5-flare");
        assert_eq!(body["tools"][0]["action"], "edit");
        assert_eq!(
            body["tools"][0]["input_image_mask"]["image_url"],
            "data:image/png;base64,bWFzaw=="
        );
        assert_eq!(body["input"][0]["content"].as_array().unwrap().len(), 2);
        assert!(!body["input"].to_string().contains("bWFzaw=="));
        assert_eq!(
            validated_responses_mask_data_url(
                request.editing.as_ref().unwrap().mask.as_ref().unwrap()
            )
            .unwrap(),
            "data:image/png;base64,bWFzaw=="
        );
        let mut invalid = request.editing.unwrap().mask.unwrap();
        invalid.sha256 = "changed".into();
        assert!(validated_responses_mask_data_url(&invalid).is_err());
    }

    #[test]
    fn images_request_omits_png_compression_and_keeps_webp_compression() {
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.background = Some("transparent".into());
        let request = GenerationRequest {
            editing: None,
            automatic_size: false,
            prompt: "test".into(),
            compatibility_prompt: None,
            model: "gpt-image2-vip".into(),
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ImagesApi,
            previous_response_id: None,
            reference_assets: Vec::new(),
        };

        let png_body = build_openai_json(&config, &request);
        assert_eq!(png_body["background"], "transparent");
        assert!(png_body.get("output_compression").is_none());

        config.output_format = Some("webp".into());
        config.output_compression = Some(82);
        let webp_body = build_openai_json(&config, &request);
        assert_eq!(webp_body["output_compression"], 82);

        config.background = Some("local".into());
        let local_body = build_openai_json(&config, &request);
        assert_eq!(local_body["background"], "auto");

        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        let local_responses_body = build_openai_json(&config, &request);
        assert_eq!(local_responses_body["tools"][0]["background"], "auto");
    }

    #[test]
    fn hydrate_does_not_restore_local_asset_removed_by_remote_tombstone() {
        let mut local = LocalAppState::default();
        local.assets.push(ImageAssetRef {
            id: "asset-1".into(),
            sha256: "hash".into(),
            mime_type: "image/png".into(),
            byte_len: 1,
            width: None,
            height: None,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            data_url: Some("data:image/png;base64,AA==".into()),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        });
        let hydrated = hydrate_local_state(
            &local,
            SyncEnvelope {
                tombstones: vec![SyncTombstone {
                    entity_kind: SyncEntityKind::Asset,
                    entity_id: "asset-1".into(),
                    deleted_at: "2026-01-02T00:00:00+00:00".into(),
                }],
                ..SyncEnvelope::default()
            },
            SyncCheckpoint::default(),
            None,
            None,
        );
        assert!(hydrated.assets.is_empty());
    }

    #[test]
    fn sync_envelope_encrypts_api_key_without_exposing_plaintext() {
        let mut state = LocalAppState::default();
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.api_key_plaintext = Some("sk-example".into());
        state.configs.push(config);

        let envelope = prepare_sync_envelope(&state, Some("trusted-secret"), true).unwrap();
        let synced = &envelope.configs[0];
        assert!(synced.api_key_plaintext.is_none());
        assert!(synced.api_key_encrypted.is_some());
        assert_eq!(
            decrypt_secret("trusted-secret", synced.api_key_encrypted.as_ref().unwrap()).unwrap(),
            "sk-example"
        );
    }

    #[test]
    fn disabling_api_key_sync_removes_ciphertext() {
        let mut state = LocalAppState::default();
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.api_key_plaintext = Some("sk-example".into());
        config.api_key_encrypted = Some(encrypt_secret("trusted-secret", "sk-example").unwrap());
        state.configs.push(config);

        let envelope = prepare_sync_envelope(&state, None, false).unwrap();
        assert!(envelope.configs[0].api_key_plaintext.is_none());
        assert!(envelope.configs[0].api_key_encrypted.is_none());
    }

    #[test]
    fn legacy_password_ciphertext_is_recovered_for_migration() {
        let local = LocalAppState::default();
        let mut config = default_config(BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.api_key_encrypted = Some(encrypt_secret("old-password", "sk-example").unwrap());
        let remote = SyncEnvelope {
            configs: vec![config],
            ..SyncEnvelope::default()
        };

        let hydrated = hydrate_local_state(
            &local,
            remote,
            SyncCheckpoint::default(),
            Some("trusted-secret"),
            Some("old-password"),
        );
        let recovered = &hydrated.configs[0];
        assert_eq!(recovered.api_key_plaintext.as_deref(), Some("sk-example"));
        assert!(recovered.api_key_encrypted.is_none());
    }
}
