use super::super::*;

#[derive(Deserialize)]
struct FetchImageResponse {
    mime_type: String,
    body_base64: String,
}

pub(crate) fn download_file_name_for_asset(asset: &ImageAssetRef) -> String {
    let date = format_shanghai_date_compact(&asset.created_at).unwrap_or_else(today_compact);
    let hash = short_download_hash(asset);
    let extension = extension_from_mime(&asset.mime_type);
    format!("mew_{date}_{hash}.{extension}")
}

pub(crate) fn short_download_hash(asset: &ImageAssetRef) -> String {
    let candidate = if asset.sha256.trim().is_empty() {
        asset.id.as_str()
    } else {
        asset.sha256.as_str()
    };
    short_hash_text(candidate)
}

pub(crate) fn short_hash_text(value: &str) -> String {
    let cleaned = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        "image".into()
    } else {
        cleaned
    }
}

pub(crate) fn extension_from_mime(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/png" => "png",
        _ => "png",
    }
}

pub(crate) fn asset_src(asset: &ImageAssetRef) -> String {
    asset
        .data_url
        .clone()
        .or_else(|| runtime_asset_object_url(&asset.id))
        .or_else(|| asset.remote_url.clone())
        .unwrap_or_default()
}

pub(crate) fn asset_full_preview_src(asset: &ImageAssetRef) -> String {
    asset
        .data_url
        .clone()
        .or_else(|| runtime_asset_object_url(&asset.id))
        .or_else(|| asset.remote_url.clone())
        .or_else(|| persisted_thumbnail_src(asset))
        .unwrap_or_default()
}

pub(crate) fn asset_display_src(asset: &ImageAssetRef) -> String {
    persisted_thumbnail_src(asset)
        .or_else(|| asset.data_url.clone())
        .or_else(|| runtime_asset_object_url(&asset.id))
        .or_else(|| asset.remote_url.clone())
        .unwrap_or_default()
}

fn persisted_thumbnail_src(asset: &ImageAssetRef) -> Option<String> {
    asset
        .metadata
        .get(THUMBNAIL_DATA_URL_KEY)
        .filter(|value| is_embedded_asset_data_url(value))
        .cloned()
}

pub(crate) fn bytes_to_data_url(bytes: &[u8], mime_type: &str) -> String {
    format!("data:{mime_type};base64,{}", BASE64.encode(bytes))
}

pub(crate) fn decode_browser_data_url(data_url: &str) -> Result<(String, Vec<u8>), String> {
    let Some((prefix, payload)) = data_url.split_once(',') else {
        return Err("浏览器数据 URL 无效".into());
    };
    let mime_type = prefix
        .trim_start_matches("data:")
        .split(';')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("image/png")
        .to_string();
    let bytes = BASE64
        .decode(payload)
        .map_err(|error| format!("浏览器数据 URL 解码失败：{error}"))?;
    Ok((mime_type, bytes))
}

pub(crate) async fn load_html_image(src: &str) -> Result<HtmlImageElement, String> {
    let image = HtmlImageElement::new().map_err(|error| format!("{error:?}"))?;
    image.set_src(src);
    JsFuture::from(image.decode())
        .await
        .map_err(|error| format!("图片载入失败：{error:?}"))?;
    Ok(image)
}

pub(crate) async fn reencode_asset_bytes(
    asset: &ImageAssetRef,
    target_mime: &str,
    quality: Option<f64>,
) -> Result<(Vec<u8>, String, u32, u32), String> {
    let source = asset_src(asset);
    if source.is_empty() {
        return Err("当前连续修改所需的图片缺少可读取数据，请先重新生成一次。".into());
    }
    let (source_bytes, source_mime) = fetch_image_bytes(&source).await?;
    let source_data_url = bytes_to_data_url(&source_bytes, &source_mime);
    let image = load_html_image(&source_data_url).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);

    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "画布元素类型错误".to_string())?;
    canvas.set_width(width);
    canvas.set_height(height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("{error:?}"))?
        .ok_or_else(|| "无法获取 2D 画布上下文".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element(&image, 0.0, 0.0)
        .map_err(|error| format!("绘制图片到画布失败：{error:?}"))?;
    let data_url = if let Some(quality) = quality {
        canvas
            .to_data_url_with_type_and_encoder_options(
                target_mime,
                &wasm_bindgen::JsValue::from_f64(quality),
            )
            .map_err(|error| format!("图片转码失败：{error:?}"))?
    } else {
        canvas
            .to_data_url_with_type(target_mime)
            .map_err(|error| format!("图片转码失败：{error:?}"))?
    };
    let (mime_type, bytes) = decode_browser_data_url(&data_url)?;
    Ok((bytes, mime_type, width, height))
}

pub(crate) async fn thumbnail_data_url_from_asset(
    asset: &ImageAssetRef,
    max_edge: u32,
) -> Result<String, String> {
    let source = asset_src(asset);
    if source.is_empty() {
        return Err("缩略图源图片不可用".into());
    }
    let image = load_html_image(&source).await?;
    let width = image.natural_width().max(1);
    let height = image.natural_height().max(1);
    let longest = width.max(height).max(1);
    // Blob/远程 URL 仅在当前页面或云端有效，不能写进可持久化的缩略图元数据。
    if longest <= max_edge && is_embedded_asset_data_url(&source) {
        return Ok(source);
    }
    let scale = (max_edge as f64 / longest as f64).min(1.0);
    let target_width = ((width as f64 * scale).round() as u32).max(1);
    let target_height = ((height as f64 * scale).round() as u32).max(1);

    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|error| format!("创建缩略图画布失败：{error:?}"))?
        .dyn_into()
        .map_err(|_| "缩略图画布元素类型错误".to_string())?;
    canvas.set_width(target_width);
    canvas.set_height(target_height);
    let context = canvas
        .get_context("2d")
        .map_err(|error| format!("{error:?}"))?
        .ok_or_else(|| "无法获取缩略图 2D 画布上下文".to_string())?
        .unchecked_into::<web_sys::CanvasRenderingContext2d>();
    context
        .draw_image_with_html_image_element_and_dw_and_dh(
            &image,
            0.0,
            0.0,
            target_width as f64,
            target_height as f64,
        )
        .map_err(|error| format!("绘制缩略图失败：{error:?}"))?;
    canvas
        .to_data_url_with_type_and_encoder_options(
            "image/webp",
            &wasm_bindgen::JsValue::from_f64(0.82),
        )
        .or_else(|_| canvas.to_data_url_with_type("image/jpeg"))
        .map_err(|error| format!("生成缩略图失败：{error:?}"))
}

pub(crate) async fn fetch_image_bytes(src: &str) -> Result<(Vec<u8>, String), String> {
    if let Some((prefix, payload)) = src.split_once(',') {
        let mime_type = prefix
            .trim_start_matches("data:")
            .split(';')
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("image/png")
            .to_string();
        let bytes = BASE64
            .decode(payload)
            .map_err(|error| format!("图片解码失败：{error}"))?;
        return Ok((bytes, mime_type));
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return fetch_remote_image_bytes(src).await;
    }
    let request_url = if src.starts_with("/api/") {
        api_url(src)
    } else {
        src.to_string()
    };
    let response = Request::get(&request_url)
        .send()
        .await
        .map_err(|error| format!("下载图片失败：{error}"))?;
    let mime_type = response
        .headers()
        .get("content-type")
        .unwrap_or_else(|| "image/png".into());
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("读取图片失败：{error}"))?;
    Ok((bytes, mime_type))
}

pub(crate) async fn fetch_remote_image_bytes(src: &str) -> Result<(Vec<u8>, String), String> {
    if let Ok(result) = fetch_image_bytes_direct(src).await {
        return Ok(result);
    }
    let response = Request::post(&api_url("/api/images/fetch"))
        .credentials(web_sys::RequestCredentials::Include)
        .json(&serde_json::json!({ "url": src }))
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| format!("代理下载图片失败：{error}"))?;
    if !response.ok() {
        return Err(response
            .text()
            .await
            .unwrap_or_else(|_| "代理下载图片失败".into()));
    }
    let payload = response
        .json::<FetchImageResponse>()
        .await
        .map_err(|error| format!("代理下载图片响应解析失败：{error}"))?;
    let bytes = BASE64
        .decode(payload.body_base64)
        .map_err(|error| format!("代理图片 Base64 解码失败：{error}"))?;
    Ok((bytes, payload.mime_type))
}

pub(crate) async fn fetch_image_bytes_direct(src: &str) -> Result<(Vec<u8>, String), String> {
    let response = Request::get(src)
        .send()
        .await
        .map_err(|error| format!("下载图片失败：{error}"))?;
    if !response.ok() {
        return Err(format!("下载图片失败：HTTP {}", response.status()));
    }
    let mime_type = response
        .headers()
        .get("content-type")
        .unwrap_or_else(|| "image/png".into());
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("读取图片失败：{error}"))?;
    Ok((bytes, mime_type))
}

pub(crate) async fn fetch_authenticated_image_bytes(
    src: &str,
) -> Result<(Vec<u8>, String), String> {
    let response = Request::get(src)
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(|error| format!("下载云端图片失败：{error}"))?;
    if !response.ok() {
        return Err(format!("下载云端图片失败：HTTP {}", response.status()));
    }
    let mime_type = response
        .headers()
        .get("content-type")
        .unwrap_or_else(|| "image/png".into());
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("读取云端图片失败：{error}"))?;
    Ok((bytes, mime_type))
}

pub(crate) fn blob_from_bytes(bytes: &[u8], mime_type: &str) -> Result<Blob, String> {
    let array = Uint8Array::from(bytes);
    let parts = Array::new();
    parts.push(&array.buffer());
    let bag = BlobPropertyBag::new();
    bag.set_type(mime_type);
    Blob::new_with_u8_array_sequence_and_options(&parts, &bag)
        .map_err(|error| format!("构建 Blob 失败：{error:?}"))
}

pub(crate) async fn copy_image_from_src(src: &str) -> Result<(), String> {
    let (bytes, mime_type) = fetch_image_bytes(src).await?;
    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let clipboard = window.navigator().clipboard();
    let blob = blob_from_bytes(&bytes, &mime_type)?;
    let item_data = Object::new();
    Reflect::set(
        &item_data,
        &wasm_bindgen::JsValue::from_str(&mime_type),
        &blob,
    )
    .map_err(|error| format!("准备剪贴板数据失败：{error:?}"))?;
    let clipboard_item = Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("ClipboardItem"),
    )
    .map_err(|_| "当前浏览器不支持 ClipboardItem".to_string())?;
    let constructor: Function = clipboard_item
        .dyn_into()
        .map_err(|_| "ClipboardItem 构造器不可用".to_string())?;
    let args = Array::new();
    args.push(&item_data);
    let item = Reflect::construct(&constructor, &args)
        .map_err(|error| format!("创建剪贴板对象失败：{error:?}"))?;
    let items = Array::new();
    items.push(&item);
    JsFuture::from(clipboard.write(&items))
        .await
        .map_err(|error| format!("写入剪贴板失败：{error:?}"))?;
    Ok(())
}

pub(crate) fn download_image_from_src(src: &str, file_name: &str) -> Result<(), String> {
    let Some(window) = web_sys::window() else {
        return Err("浏览器窗口不可用".into());
    };
    let Some(document) = window.document() else {
        return Err("浏览器文档不可用".into());
    };
    let element = document
        .create_element("a")
        .map_err(|error| format!("创建下载元素失败：{error:?}"))?;
    let anchor: HtmlAnchorElement = element
        .dyn_into()
        .map_err(|_| "下载元素类型错误".to_string())?;
    anchor.set_href(src);
    anchor.set_download(file_name);
    let _ = anchor.set_attribute("style", "display:none");
    let Some(body) = document.body() else {
        return Err("浏览器页面主体不可用".into());
    };
    body.append_child(&anchor)
        .map_err(|error| format!("挂载下载元素失败：{error:?}"))?;
    anchor.click();
    let _ = body.remove_child(&anchor);
    Ok(())
}

pub(crate) async fn collect_backup_payloads(
    state: &LocalAppState,
) -> Result<HashMap<String, String>, String> {
    let asset_ids = state
        .assets
        .iter()
        .map(|asset| asset.id.clone())
        .collect::<Vec<_>>();
    let mut payloads = load_asset_payloads(&asset_ids)
        .await
        .map_err(|error| format!("读取本地图片失败：{error}"))?;
    for asset in &state.assets {
        if payloads.contains_key(&asset.id)
            || asset
                .data_url
                .as_deref()
                .map(is_embedded_asset_data_url)
                .unwrap_or(false)
        {
            continue;
        }
        let Some(remote_url) = asset.remote_url.as_deref() else {
            continue;
        };
        let source = if remote_url.starts_with('/') {
            api_url(remote_url)
        } else {
            remote_url.to_string()
        };
        let (bytes, mime_type) = if remote_url.starts_with("/api/assets/") {
            fetch_authenticated_image_bytes(&source).await?
        } else {
            fetch_image_bytes(&source).await?
        };
        payloads.insert(asset.id.clone(), bytes_to_data_url(&bytes, &mime_type));
    }
    Ok(payloads)
}

pub(crate) fn safe_session_file_title(thread_title: &str) -> String {
    let safe_title = thread_title
        .trim()
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .take(64)
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();
    if safe_title.is_empty() {
        "session".to_string()
    } else {
        safe_title
    }
}

pub(crate) fn session_backup_file_name(thread_title: &str) -> String {
    let safe_title = safe_session_file_title(thread_title);
    format!("mew-session-{safe_title}-{}.zip", today_compact())
}

pub(crate) fn download_backup_bytes(bytes: &[u8], file_name: &str) -> Result<(), String> {
    let blob = blob_from_bytes(bytes, "application/zip")?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|error| format!("创建备份下载地址失败：{error:?}"))?;
    let result = download_image_from_src(&url, file_name);
    // 浏览器需要在点击事件之后继续读取 Blob；同步 revoke 在部分浏览器会取消下载。
    spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(0).await;
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    result
}

pub(crate) async fn import_file_list(files: FileList) -> Result<Vec<ImageAssetRef>, String> {
    let mut imported = Vec::new();
    for index in 0..files.length() {
        let Some(file) = files.get(index) else {
            continue;
        };
        let file = File::from(file);
        let bytes = read_as_bytes(&file)
            .await
            .map_err(|error| error.to_string())?;
        let data_url = read_as_data_url(&file)
            .await
            .map_err(|error| error.to_string())?;
        let (width, height) = load_image_dimensions(&data_url).await.unwrap_or((0, 0));
        imported.push(ImageAssetRef {
            id: new_id(),
            sha256: sha256_hex(&bytes),
            mime_type: file.raw_mime_type(),
            byte_len: bytes.len() as u64,
            width: (width > 0).then_some(width),
            height: (height > 0).then_some(height),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: Some(data_url),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: BTreeMap::new(),
        });
    }
    Ok(imported)
}

pub(crate) async fn load_image_dimensions(data_url: &str) -> Result<(u32, u32), String> {
    let image = load_html_image(data_url).await?;
    Ok((image.natural_width(), image.natural_height()))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_backup_filename_is_windows_safe() {
        let safe_title = safe_session_file_title(" 项目:A/B*测试? ");
        assert_eq!(safe_title, "项目_A_B_测试_");
        assert!(!safe_title.contains([':', '/', '*', '?']));
    }
}
