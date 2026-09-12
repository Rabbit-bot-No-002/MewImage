//! 图片模型约束集中在共享层，避免直连与代理采用不同校验规则。

use crate::{EncryptedApiConfig, GenerationRequest, ProviderEndpointMode, ProviderKind};

pub const MAX_GENERATION_REFERENCE_IMAGES: usize = 10;
pub const AUTO_IMAGE_BUDGET_DIMENSIONS: (u32, u32) = (3840, 2160);

impl GenerationRequest {
    /// 自动尺寸的预算不能由客户端填入的小宽高绕过。
    pub fn budget_dimensions(&self) -> (u32, u32) {
        if self.automatic_size {
            AUTO_IMAGE_BUDGET_DIMENSIONS
        } else {
            (self.width, self.height)
        }
    }

    pub fn openai_size(&self) -> String {
        if self.automatic_size {
            "auto".into()
        } else {
            format!("{}x{}", self.width, self.height)
        }
    }
}
pub const GPT_IMAGE_25_MODELS: [&str; 2] = ["gpt-image-2.5-flare", "gpt-image-2.5-sunburst"];

fn matches_model(model: &str, name: &str) -> bool {
    if model.eq_ignore_ascii_case(name) {
        return true;
    }
    // 只识别正式日期快照，不能把中转站的任意后缀当作官方能力声明。
    let Some(prefix) = model.get(..name.len()) else {
        return false;
    };
    let Some(date) = model
        .get(name.len()..)
        .and_then(|suffix| suffix.strip_prefix('-'))
    else {
        return false;
    };
    prefix.eq_ignore_ascii_case(name)
        && date.len() == 10
        && date.bytes().enumerate().all(|(index, value)| {
            if index == 4 || index == 7 {
                value == b'-'
            } else {
                value.is_ascii_digit()
            }
        })
}

pub fn is_gpt_image_25(model: &str) -> bool {
    GPT_IMAGE_25_MODELS
        .iter()
        .any(|name| matches_model(model.trim(), name))
}

pub fn is_known_legacy_gpt_image(model: &str) -> bool {
    [
        "gpt-image-1",
        "gpt-image-1-mini",
        "gpt-image-1.5",
        "gpt-image-2",
    ]
    .iter()
    .any(|name| matches_model(model.trim(), name))
}

pub fn supports_extended_image_quality(model: &str) -> bool {
    !is_known_legacy_gpt_image(model)
}

/// 未知模型名保留透传能力；仅对已知模型强制应用官方尺寸边界。
pub fn validate_openai_image_options(
    config: &EncryptedApiConfig,
    model: &str,
    width: u32,
    height: u32,
    quality: Option<&str>,
) -> Result<(), String> {
    if width < 256
        || height < 256
        || !width.is_multiple_of(16)
        || !height.is_multiple_of(16)
        || u64::from(width) * u64::from(height) > 4096 * 4096
    {
        return Err(
            "显式尺寸要求宽高至少 256、为 16 的倍数，总像素不超过 4096×4096；请手动调整尺寸。"
                .into(),
        );
    }
    if config.provider_kind != ProviderKind::OpenAiImage {
        return Ok(());
    }
    if let Some(quality) = quality {
        if !["auto", "low", "medium", "high", "xhigh", "max"].contains(&quality) {
            return Err("图片质量必须为 auto、low、medium、high、xhigh 或 max。".into());
        }
        if matches!(quality, "xhigh" | "max") && !supports_extended_image_quality(model) {
            return Err(
                "当前模型不支持超高或最高质量，请手动调整质量或选择 GPT Image 2.5。".into(),
            );
        }
    }
    if config.background.as_deref() == Some("transparent")
        && crate::normalized_image_output_format(config.output_format.as_deref()) == "jpeg"
    {
        return Err("透明背景不能使用 JPEG，请选择 PNG 或 WebP。".into());
    }
    if !is_gpt_image_25(model) && !matches_model(model.trim(), "gpt-image-2") {
        return Ok(());
    }
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || !width.is_multiple_of(16)
        || !height.is_multiple_of(16)
        || width.max(height) > 3840
        || u64::from(width.max(height)) > u64::from(width.min(height)) * 3
        || !(655_360..=8_294_400).contains(&pixels)
    {
        return Err("当前模型要求宽高为 16 的倍数、单边不超过 3840、长宽比不超过 3:1、总像素为 655360–8294400；可使用 1024×1024 或 3840×2160。".into());
    }
    Ok(())
}

/// 新生成入口校验；不用于读取历史任务或导入旧备份。
pub fn validate_image_generation_request(
    config: &EncryptedApiConfig,
    request: &GenerationRequest,
) -> Result<(), String> {
    if request
        .compatibility_prompt
        .as_ref()
        .is_some_and(|prompt| prompt.chars().count() > 64_000)
    {
        return Err("连续对话兼容上下文最多 64000 个字符。".into());
    }
    if let Some(response_id) = request.previous_response_id.as_deref()
        && (config.provider_kind != ProviderKind::OpenAiImage
            || config.endpoint_mode != ProviderEndpointMode::ResponsesApi
            || request.endpoint_mode != ProviderEndpointMode::ResponsesApi
            || response_id.is_empty()
            || response_id.len() > 512
            || response_id.chars().any(char::is_control))
    {
        return Err(
            "连续会话标识仅允许用于 Responses API，且必须是不超过 512 字符的有效标识。".into(),
        );
    }
    if request.reference_assets.len() > MAX_GENERATION_REFERENCE_IMAGES {
        return Err("单次生成最多使用 10 张参考图，请先精简选择；原有图片不会删除。".into());
    }
    if let Some(editing) = &request.editing {
        editing.validate(request)?;
        if config.provider_kind != ProviderKind::OpenAiImage
            || !matches!(
                config.endpoint_mode,
                ProviderEndpointMode::ImagesApi | ProviderEndpointMode::ResponsesApi
            )
        {
            return Err("编辑输入仅支持 OpenAI Image 的 Images API 或 Responses API。".into());
        }
        if config.endpoint_mode != request.endpoint_mode {
            return Err("编辑请求的接口模式与当前服务商配置不一致，请重新提交任务。".into());
        }
    }
    if request.automatic_size && config.provider_kind != ProviderKind::OpenAiImage {
        return Err("模型自动尺寸目前仅支持 OpenAI Image，请手动选择输出尺寸。".into());
    }
    let (width, height) = request.budget_dimensions();
    validate_openai_image_options(
        config,
        &request.model,
        width,
        height,
        request.quality.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EncryptedApiConfig {
        serde_json::from_value(serde_json::json!({
            "id":"test", "name":"test", "provider_template_id":"builtin-openai-image",
            "provider_kind":"openai_image", "base_url":"https://api.openai.com",
            "model":"gpt-image-2", "endpoint_mode":"images_api", "updated_at":"",
            "created_at":"", "access_mode":"proxy", "known_requires_proxy":true,
            "prompt_guard_enabled":false
        }))
        .unwrap()
    }

    #[test]
    fn model_names_and_snapshots_do_not_capture_relay_aliases() {
        assert!(is_gpt_image_25(" GPT-IMAGE-2.5-FLARE "));
        assert!(is_gpt_image_25("gpt-image-2.5-sunburst-2026-09-08"));
        assert!(!is_gpt_image_25("gpt-image-2.5-flare-vip"));
        assert!(!supports_extended_image_quality("gpt-image-2-2026-04-21"));
        assert!(supports_extended_image_quality("vip-image"));
    }

    #[test]
    fn automatic_size_is_backward_compatible_and_reserves_maximum_pixels() {
        let mut request: GenerationRequest = serde_json::from_value(serde_json::json!({
            "prompt":"test", "model":"gpt-image-2.5-flare", "width":1024,
            "height":1024, "quality":"high", "count":1, "endpoint_mode":"images_api",
            "reference_assets":[]
        }))
        .unwrap();
        assert!(!request.automatic_size);
        assert_eq!(request.openai_size(), "1024x1024");
        request.automatic_size = true;
        request.width = 1;
        request.height = 1;
        assert_eq!(request.openai_size(), "auto");
        assert_eq!(request.budget_dimensions(), (3840, 2160));
        assert!(validate_image_generation_request(&config(), &request).is_ok());
        let mut other = config();
        other.provider_kind = ProviderKind::NanoBanana;
        assert!(validate_image_generation_request(&other, &request).is_err());
    }

    #[test]
    fn validates_known_model_quality_without_changing_it() {
        let config = config();
        for model in GPT_IMAGE_25_MODELS {
            for quality in ["auto", "low", "medium", "high", "xhigh", "max"] {
                assert!(
                    validate_openai_image_options(&config, model, 1024, 1024, Some(quality))
                        .is_ok()
                );
            }
        }
        assert!(
            validate_openai_image_options(&config, "gpt-image-2", 1024, 1024, Some("max")).is_err()
        );
    }

    #[test]
    fn validates_size_edges_and_pixel_limits() {
        let config = config();
        for (width, height) in [(1024, 640), (3840, 2160), (2160, 3840), (2880, 2880)] {
            assert!(
                validate_openai_image_options(&config, "gpt-image-2.5-flare", width, height, None)
                    .is_ok()
            );
        }
        for (width, height) in [
            (0, 1024),
            (1023, 1024),
            (512, 512),
            (4096, 1024),
            (3840, 3840),
            (3840, 640),
        ] {
            assert!(
                validate_openai_image_options(&config, "gpt-image-2.5-flare", width, height, None)
                    .is_err()
            );
        }
        assert!(validate_openai_image_options(&config, "relay-alias", 4096, 4096, None).is_ok());
    }

    #[test]
    fn rejects_transparent_jpeg_but_accepts_alpha_formats() {
        let mut config = config();
        config.background = Some("transparent".into());
        config.output_format = Some("jpeg".into());
        assert!(
            validate_openai_image_options(&config, "gpt-image-2.5-flare", 1024, 1024, None)
                .is_err()
        );
        config.output_format = Some("webp".into());
        assert!(
            validate_openai_image_options(&config, "gpt-image-2.5-flare", 1024, 1024, None).is_ok()
        );
    }

    #[test]
    fn reference_limit_is_for_new_requests_not_deserialization() {
        let mut request: GenerationRequest = serde_json::from_value(serde_json::json!({
            "prompt":"test", "model":"gpt-image-2.5-flare", "width":1024,
            "height":1024, "quality":"high", "count":1, "endpoint_mode":"images_api",
            "reference_assets":[]
        }))
        .unwrap();
        for index in 0..16 {
            request.reference_assets.push(crate::ImageAssetRef {
                id: index.to_string(),
                sha256: String::new(),
                mime_type: "image/png".into(),
                byte_len: 1,
                width: Some(1024),
                height: Some(1024),
                created_at: String::new(),
                updated_at: String::new(),
                data_url: None,
                remote_object_key: None,
                remote_url: None,
                source_task_id: None,
                metadata: Default::default(),
            });
            assert_eq!(
                validate_image_generation_request(&config(), &request).is_ok(),
                index < 10
            );
        }
        let restored: GenerationRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(restored.reference_assets.len(), 16);
    }

    #[test]
    fn editing_is_only_allowed_for_openai_image_protocols() {
        let mut request: GenerationRequest = serde_json::from_value(serde_json::json!({
            "prompt":"edit", "model":"gpt-image-2.5-flare", "width":1024,
            "height":1024, "quality":"high", "count":1, "endpoint_mode":"images_api",
            "reference_assets":[{
                "id":"base", "sha256":"hash", "mime_type":"image/png", "byte_len":10,
                "width":1024, "height":1024, "created_at":"now", "updated_at":"now", "metadata":{}
            }],
            "editing":{"mode":"annotation","base_asset_id":"base","instruction":"follow marks"}
        }))
        .unwrap();
        let mut provider = config();
        assert!(validate_image_generation_request(&provider, &request).is_ok());
        provider.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        request.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        assert!(validate_image_generation_request(&provider, &request).is_ok());
        provider.provider_kind = ProviderKind::OpenAiCompatible;
        assert!(validate_image_generation_request(&provider, &request).is_err());
        provider.provider_kind = ProviderKind::OpenAiImage;
        provider.endpoint_mode = ProviderEndpointMode::CustomJson;
        request.endpoint_mode = ProviderEndpointMode::CustomJson;
        assert!(validate_image_generation_request(&provider, &request).is_err());

        provider.endpoint_mode = ProviderEndpointMode::ImagesApi;
        request.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        assert!(validate_image_generation_request(&provider, &request).is_err());
    }
}
