use web_sys::{Blob, HtmlImageElement};

use super::{EditMode, EditorDraft, RenderedCanvas};

/// 仅最终压缩输入；不携带画布、操作历史或预览 UI。
pub struct EncodedEdit {
    pub mode: EditMode,
    pub image: Blob,
    pub mask: Option<Blob>,
    pub width: u32,
    pub height: u32,
}

/// 在调用方持有编辑编码内存预算时执行；全部编码成功前不更新工作台。
pub async fn encode_edit(
    draft: &EditorDraft,
    base: Option<&HtmlImageElement>,
    imported_mask: Option<&HtmlImageElement>,
) -> Result<EncodedEdit, String> {
    draft.validate_size()?;
    let maximum_edge = draft.width.max(draft.height);
    if draft.mode != EditMode::Mask {
        let image = RenderedCanvas::render(draft, base, None, maximum_edge)?
            .into_png_blob()
            .await?;
        return Ok(EncodedEdit {
            mode: draft.mode,
            image,
            mask: None,
            width: draft.width,
            height: draft.height,
        });
    }
    let base = base.ok_or("局部修改缺少底图。")?;
    if draft.base_asset_id.is_none() {
        return Err("局部修改缺少底图资源引用。".into());
    }
    // 先校验选区并释放遮罩画布，再编码底图，避免同时持有两份工作 RGBA。
    let mask = RenderedCanvas::render(draft, Some(base), imported_mask, maximum_edge)?
        .into_png_blob()
        .await?;
    let image = RenderedCanvas::base_copy(base, draft.width, draft.height)?
        .into_png_blob()
        .await?;
    Ok(EncodedEdit {
        mode: draft.mode,
        image,
        mask: Some(mask),
        width: draft.width,
        height: draft.height,
    })
}
