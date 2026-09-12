use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, HtmlImageElement, Url};

/// 编辑器独占底图 URL；最后一个预览/编码使用者退出时释放。
pub struct EditorBase {
    image: HtmlImageElement,
    url: String,
}

pub struct EditorMask {
    id: String,
    image: EditorBase,
}

impl EditorMask {
    pub async fn load(id: String, width: u32, height: u32) -> Result<Self, String> {
        let image = EditorBase::load_mask(&id, width, height).await?;
        Ok(Self { id, image })
    }

    pub async fn from_blob(
        id: String,
        blob: &Blob,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let image = EditorBase::validate_mask_blob(blob, width, height).await?;
        Ok(Self { id, image })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn image(&self) -> &HtmlImageElement {
        self.image.image()
    }
}

impl EditorBase {
    pub async fn load(
        asset_id: &str,
        width: u32,
        height: u32,
        original_dimensions: Option<(u32, u32)>,
    ) -> Result<Self, String> {
        let mut draft =
            super::EditorDraft::new("validate".into(), Some(asset_id.into()), width, height)?;
        draft.original_base_dimensions = original_dimensions;
        draft.validate()?;
        let blob = crate::storage::load_edit_asset_blob(asset_id).await?;
        if blob.size() > 32.0 * 1024.0 * 1024.0 {
            return Err("编辑底图超过 32 MiB，请先创建较小的工作副本。".into());
        }
        let (source_width, source_height) = original_dimensions.unwrap_or((width, height));
        let source = Self::decode_blob(&blob, source_width, source_height).await?;
        let Some(_) = original_dimensions else {
            return Ok(source);
        };
        let resized = super::RenderedCanvas::resized_base_copy(source.image(), width, height)?
            .into_png_blob()
            .await?;
        // 原始解码图只服务于缩小操作，不能在编辑会话里一直占用大像素缓冲。
        drop(source);
        drop(blob);
        Self::decode_blob(&resized, width, height).await
    }

    async fn decode_blob(blob: &web_sys::Blob, width: u32, height: u32) -> Result<Self, String> {
        let url = Url::create_object_url_with_blob(blob)
            .map_err(|error| format!("创建编辑底图 URL 失败：{error:?}"))?;
        let image = match HtmlImageElement::new() {
            Ok(image) => image,
            Err(error) => {
                let _ = Url::revoke_object_url(&url);
                return Err(format!("创建编辑底图失败：{error:?}"));
            }
        };
        let base = Self { image, url };
        base.image.set_src(&base.url);
        wasm_bindgen_futures::JsFuture::from(base.image.decode())
            .await
            .map_err(|error| format!("编辑底图解码失败：{error:?}"))?;
        if base.image.natural_width() != width || base.image.natural_height() != height {
            return Err("编辑底图尺寸与草稿不一致，请重新建立工作副本。".into());
        }
        Ok(base)
    }

    pub fn image(&self) -> &HtmlImageElement {
        &self.image
    }

    pub async fn load_mask(asset_id: &str, width: u32, height: u32) -> Result<Self, String> {
        let blob = crate::storage::load_edit_asset_blob(asset_id).await?;
        Self::validate_mask_blob(&blob, width, height).await
    }

    pub async fn validate_mask_blob(blob: &Blob, width: u32, height: u32) -> Result<Self, String> {
        if blob.type_() != "image/png" || blob.size() <= 0.0 || blob.size() > 32.0 * 1024.0 * 1024.0
        {
            return Err("遮罩必须是 32 MiB 以内的 PNG。".into());
        }
        let buffer = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|error| format!("读取遮罩失败：{error:?}"))?;
        let bytes = js_sys::Uint8Array::new(&buffer);
        let signature = [137_u8, 80, 78, 71, 13, 10, 26, 10];
        if bytes.length() < signature.len() as u32
            || signature
                .iter()
                .enumerate()
                .any(|(index, byte)| bytes.get_index(index as u32) != *byte)
        {
            return Err("遮罩文件没有有效的 PNG 魔数。".into());
        }
        drop(bytes);
        drop(buffer);
        let image = Self::decode_blob(blob, width, height).await?;
        super::RenderedCanvas::validate_mask_image(image.image(), width, height)?;
        Ok(image)
    }
}

impl Drop for EditorBase {
    fn drop(&mut self) {
        self.image.set_src("");
        let _ = Url::revoke_object_url(&self.url);
    }
}
