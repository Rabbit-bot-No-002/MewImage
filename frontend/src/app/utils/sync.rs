use super::super::*;

const ASSET_PRESENCE_BATCH_SIZE: usize = 500;

pub(crate) async fn missing_remote_asset_ids(
    asset_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut missing_asset_ids = Vec::new();
    for batch in asset_ids.chunks(ASSET_PRESENCE_BATCH_SIZE) {
        missing_asset_ids.extend(check_remote_asset_batch(batch.to_vec()).await?);
    }
    Ok(missing_asset_ids)
}

async fn check_remote_asset_batch(asset_ids: Vec<String>) -> Result<Vec<String>, String> {
    let builder = Request::post(&api_url("/api/assets/presence"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(&AssetPresenceRequest { asset_ids })
        .map_err(|error| format!("图片完整性检查序列化失败：{error}"))?;
    let response = builder
        .send()
        .await
        .map_err(|error| format!("图片完整性检查失败：{error}"))?;
    if !response.ok() {
        let raw = response
            .text()
            .await
            .unwrap_or_else(|_| "图片完整性检查失败".into());
        return Err(api_error_message(raw, "图片完整性检查失败"));
    }
    response
        .json::<AssetPresenceResponse>()
        .await
        .map(|result| result.missing_asset_ids)
        .map_err(|error| format!("图片完整性检查响应解析失败：{error}"))
}

pub(crate) async fn upload_asset_for_sync(
    asset: &ImageAssetRef,
    data_url: &str,
) -> Result<ImageAssetRef, String> {
    let (mime_type, bytes) = decode_browser_data_url(data_url)?;
    let sha256 = sha256_hex(&bytes);
    let extension = extension_from_mime(&mime_type);
    let init_request = UploadInitRequest {
        asset_id: Some(asset.id.clone()),
        file_name: format!("{}.{}", sha256, extension),
        mime_type: mime_type.clone(),
        byte_len: bytes.len() as u64,
        sha256,
    };
    let init_builder = Request::post(&api_url("/api/assets/upload-init"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(&init_request)
        .map_err(|error| format!("上传初始化序列化失败：{error}"))?;
    let init_response = init_builder
        .send()
        .await
        .map_err(|error| format!("上传初始化失败：{error}"))?;
    if !init_response.ok() {
        let raw = init_response
            .text()
            .await
            .unwrap_or_else(|_| "上传初始化失败".into());
        return Err(api_error_message(raw, "上传初始化失败"));
    }
    let initialized = init_response
        .json::<UploadInitResponse>()
        .await
        .map_err(|error| format!("上传初始化响应解析失败：{error}"))?;

    let upload_builder = Request::put(&api_url(&initialized.upload_url))
        .credentials(web_sys::RequestCredentials::Include)
        .header("Content-Type", &mime_type)
        .body(bytes)
        .map_err(|error| format!("图片上传请求构建失败：{error}"))?;
    let upload_response = upload_builder
        .send()
        .await
        .map_err(|error| format!("图片上传失败：{error}"))?;
    if !upload_response.ok() {
        let raw = upload_response
            .text()
            .await
            .unwrap_or_else(|_| "图片上传失败".into());
        return Err(api_error_message(raw, "图片上传失败"));
    }

    let complete_builder = Request::post(&api_url("/api/assets/complete"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(&UploadCompleteRequest {
            upload_token: initialized.upload_token,
        })
        .map_err(|error| format!("上传确认序列化失败：{error}"))?;
    let complete_response = complete_builder
        .send()
        .await
        .map_err(|error| format!("上传确认失败：{error}"))?;
    if !complete_response.ok() {
        let raw = complete_response
            .text()
            .await
            .unwrap_or_else(|_| "上传确认失败".into());
        return Err(api_error_message(raw, "上传确认失败"));
    }
    complete_response
        .json::<UploadCompleteResponse>()
        .await
        .map(|response| response.asset)
        .map_err(|error| format!("上传确认响应解析失败：{error}"))
}
