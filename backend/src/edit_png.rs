use image::{GenericImageView, ImageFormat, ImageReader, Limits};
use std::io::Cursor;

/// 限制解码尺寸和分配，逐像素读取 Alpha，不再复制整张 RGBA。
pub(super) fn validate(bytes: &[u8], expected: (u32, u32), mask: bool) -> Result<(), String> {
    if bytes.len() > 32 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("编辑输入不是有效 PNG 或超过 32 MiB。".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("编辑 PNG 解码失败：{error}"))?;
    if image.dimensions() != expected {
        return Err("编辑 PNG 实际尺寸与元数据不一致。".into());
    }
    if mask && !has_transparent_pixel(&image) {
        return Err("遮罩必须包含 Alpha 通道和完全透明的修改区域。".into());
    }
    Ok(())
}

fn has_transparent_pixel(image: &image::DynamicImage) -> bool {
    use image::DynamicImage;
    // 保留原始位深：16 位的非零 Alpha 不能因降为 8 位而被误认为编辑区域。
    match image {
        DynamicImage::ImageLumaA8(buffer) => buffer.pixels().any(|pixel| pixel.0[1] == 0),
        DynamicImage::ImageLumaA16(buffer) => buffer.pixels().any(|pixel| pixel.0[1] == 0),
        DynamicImage::ImageRgba8(buffer) => buffer.pixels().any(|pixel| pixel.0[3] == 0),
        DynamicImage::ImageRgba16(buffer) => buffer.pixels().any(|pixel| pixel.0[3] == 0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage, Rgba, RgbaImage};

    fn png(image: DynamicImage) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    #[test]
    fn requires_real_png_alpha_and_matching_dimensions() {
        let valid = png(DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2,
            2,
            Rgba([0, 0, 0, 0]),
        )));
        assert!(validate(&valid, (2, 2), true).is_ok());
        assert!(validate(&valid, (3, 2), true).is_err());
        assert!(validate(&valid[..24], (2, 2), true).is_err());
        assert!(validate(b"not png", (2, 2), true).is_err());
        for alpha in [128, 255] {
            let opaque = png(DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                2,
                2,
                Rgba([0, 0, 0, alpha]),
            )));
            assert!(validate(&opaque, (2, 2), true).is_err());
        }
        let rgb = png(DynamicImage::ImageRgb8(RgbImage::new(2, 2)));
        assert!(validate(&rgb, (2, 2), true).is_err());
        assert!(validate(&rgb, (2, 2), false).is_ok());
    }

    #[test]
    fn sixteen_bit_alpha_must_be_exactly_zero() {
        for alpha in [0_u16, 1, 127, 256, u16::MAX] {
            let rgba = png(DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
                2,
                2,
                Rgba([0, 0, 0, alpha]),
            )));
            let gray = png(DynamicImage::ImageLumaA16(image::ImageBuffer::from_pixel(
                2,
                2,
                image::LumaA([0, alpha]),
            )));
            assert_eq!(validate(&rgba, (2, 2), true).is_ok(), alpha == 0);
            assert_eq!(validate(&gray, (2, 2), true).is_ok(), alpha == 0);
        }
    }

    #[test]
    fn rejects_oversized_decoded_dimensions() {
        let bytes = png(DynamicImage::ImageRgba8(RgbaImage::new(4097, 1)));
        assert!(validate(&bytes, (4097, 1), true).is_err());
    }

    #[tokio::test]
    async fn temporary_file_is_checked_and_removed_after_use() {
        let budget = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let bytes = png(DynamicImage::ImageRgba8(RgbaImage::new(2, 2)));
        let path = std::env::temp_dir().join(format!("mew-edit-test-{}.png", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, &bytes).await.unwrap();
        let file = crate::TemporaryReferenceFile {
            path: path.clone(),
            mime_type: "image/png".into(),
            byte_len: bytes.len() as u64,
            sha256: crate::hex_sha256(&bytes),
        };
        assert!(
            crate::validate_temporary_edit_png(
                &file,
                (2, 2),
                true,
                budget.clone().acquire_owned().await.unwrap()
            )
            .await
            .is_ok()
        );
        assert!(
            crate::validate_temporary_edit_png(
                &file,
                (3, 2),
                false,
                budget.clone().acquire_owned().await.unwrap()
            )
            .await
            .is_err()
        );
        tokio::fs::write(&path, b"changed").await.unwrap();
        assert!(
            crate::validate_temporary_edit_png(
                &file,
                (2, 2),
                true,
                budget.clone().acquire_owned().await.unwrap()
            )
            .await
            .is_err()
        );
        drop(file);
        assert!(!path.exists());
    }

    fn edit_request(bytes: &[u8]) -> mew_image_shared::GenerationRequest {
        use base64::Engine;
        let asset = serde_json::json!({
            "id": "base", "sha256": crate::hex_sha256(bytes), "mime_type": "image/png",
            "byte_len": bytes.len(), "width": 2, "height": 2, "created_at": "now", "updated_at": "now",
            "metadata": {}, "data_url": null
        });
        let mut mask = asset.clone();
        mask["id"] = serde_json::json!("mask");
        mask["metadata"] = serde_json::json!({ "asset_role": mew_image_shared::EDIT_MASK_ROLE });
        mask["data_url"] = serde_json::json!(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
        serde_json::from_value(serde_json::json!({
            "prompt": "edit", "model": "gpt-image-2.5-flare", "width": 1024, "height": 1024,
            "quality": "high", "count": 1, "endpoint_mode": "images_api", "reference_assets": [asset],
            "editing": { "mode": "mask", "base_asset_id": "base", "mask": mask }
        })).unwrap()
    }

    #[tokio::test]
    async fn images_multipart_preserves_mask_bytes_and_independent_field() {
        use axum::{Json, Router, extract::Multipart, routing::post};
        async fn receive(mut multipart: Multipart) -> Json<serde_json::Value> {
            let mut fields = Vec::new();
            while let Some(field) = multipart.next_field().await.unwrap() {
                let name = field.name().unwrap().to_string();
                let filename = field.file_name().map(str::to_string);
                let mime = field.content_type().map(str::to_string);
                let bytes = field.bytes().await.unwrap();
                fields.push(
                    serde_json::json!({ "name": name, "filename": filename, "mime": mime,
                    "sha": crate::hex_sha256(&bytes), "length": bytes.len() }),
                );
            }
            Json(serde_json::json!(fields))
        }
        let bytes = png(DynamicImage::ImageRgba8(RgbaImage::new(2, 2)));
        let request = edit_request(&bytes);
        let form = reqwest::multipart::Form::new().part(
            "image[]",
            reqwest::multipart::Part::bytes(bytes.clone())
                .file_name("base.png")
                .mime_str("image/png")
                .unwrap(),
        );
        let form = crate::attach_openai_edit_mask(form, &request).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/edit", post(receive)))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        // 只访问本机临时端口，不调用真实模型，也不读取用户密钥。
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
            .post(format!("http://{address}/edit"))
            .multipart(form)
            .send()
            .await;
        let _ = shutdown.send(());
        let fields: serde_json::Value = response
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(fields.as_array().unwrap().len(), 2);
        assert_eq!(fields[0]["name"], "image[]");
        assert_eq!(fields[1]["name"], "mask");
        assert_eq!(fields[1]["filename"], "mask.png");
        assert_eq!(fields[1]["mime"], "image/png");
        assert_eq!(fields[1]["sha"], crate::hex_sha256(&bytes));
        assert_eq!(fields[1]["length"], bytes.len());
    }

    #[test]
    fn mask_constructor_rejects_missing_or_changed_upload() {
        let bytes = png(DynamicImage::ImageRgba8(RgbaImage::new(2, 2)));
        let mut request = edit_request(&bytes);
        request
            .editing
            .as_mut()
            .unwrap()
            .mask
            .as_mut()
            .unwrap()
            .sha256 = "0".repeat(64);
        assert!(crate::attach_openai_edit_mask(reqwest::multipart::Form::new(), &request).is_err());
        request
            .editing
            .as_mut()
            .unwrap()
            .mask
            .as_mut()
            .unwrap()
            .data_url = None;
        assert!(crate::attach_openai_edit_mask(reqwest::multipart::Form::new(), &request).is_err());
    }
}
