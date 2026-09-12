//! 编辑输入独立于模板参数，遮罩不占普通参考图名额。
use crate::{GenerationRequest, ImageAssetRef};
use serde::{Deserialize, Serialize};

pub const EDIT_MASK_ROLE: &str = "image_edit_mask_v1";

pub fn is_edit_mask(asset: &ImageAssetRef) -> bool {
    asset.metadata.get("asset_role").map(String::as_str) == Some(EDIT_MASK_ROLE)
}

/// 仅在发起生成时组合提示词；任务快照仍分别保存用户原文和编辑说明。
pub fn compose_edit_prompt(prompt: String, instruction: Option<&str>) -> String {
    let Some(instruction) = instruction.map(str::trim).filter(|value| !value.is_empty()) else {
        return prompt;
    };
    format!("{prompt}\n\n图像编辑附加说明：\n{instruction}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageEditingMode {
    Mask,
    Annotation,
    Sketch,
}

/// 任务仅保存编辑资源引用，不重复保存图片正文或本地操作历史。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEditingSnapshot {
    pub mode: ImageEditingMode,
    pub base_asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

impl ImageEditingSnapshot {
    /// 复用前只检查资源元数据，不克隆尚未领取内存预算的图片正文。
    pub fn validate_resources(&self, assets: &[ImageAssetRef]) -> Result<(), String> {
        let base = assets
            .iter()
            .find(|asset| asset.id == self.base_asset_id)
            .ok_or_else(|| format!("编辑底图 {} 已丢失，无法复用。", self.base_asset_id))?;
        let mask = self
            .mask_asset_id
            .as_ref()
            .map(|id| {
                assets
                    .iter()
                    .find(|asset| &asset.id == id)
                    .ok_or_else(|| format!("编辑遮罩 {id} 已丢失，无法复用。"))
            })
            .transpose()?;
        validate_edit_assets(self.mode, base, mask, self.instruction.as_deref())
    }

    /// 从资源索引还原输入；缺失时必须报错，不能把编辑任务退化为普通生成。
    pub fn restore(&self, assets: &[ImageAssetRef]) -> Result<ImageEditingInput, String> {
        self.validate_resources(assets)?;
        let mask = self
            .mask_asset_id
            .as_ref()
            .map(|id| {
                assets
                    .iter()
                    .find(|asset| &asset.id == id && is_edit_mask(asset))
                    .cloned()
                    .ok_or_else(|| format!("编辑遮罩 {id} 已丢失或角色不符，无法复用。"))
            })
            .transpose()?;
        Ok(ImageEditingInput {
            mode: self.mode,
            base_asset_id: self.base_asset_id.clone(),
            mask,
            instruction: self.instruction.clone(),
        })
    }

    pub fn asset_ids(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.base_asset_id).chain(self.mask_asset_id.iter())
    }

    /// 导入和 SHA 去重共用映射；未映射的引用保留，交由完整性检查报告缺失。
    pub fn remap_asset_ids(&mut self, remap: &std::collections::HashMap<String, String>) {
        for id in std::iter::once(&mut self.base_asset_id).chain(self.mask_asset_id.iter_mut()) {
            if let Some(mapped) = remap.get(id) {
                *id = mapped.clone();
            }
        }
    }
}

fn validate_edit_assets(
    mode: ImageEditingMode,
    base: &ImageAssetRef,
    mask: Option<&ImageAssetRef>,
    instruction: Option<&str>,
) -> Result<(), String> {
    if base.id.is_empty()
        || base.id.len() > 128
        || is_edit_mask(base)
        || instruction.is_some_and(|text| text.chars().count() > 2048)
    {
        return Err("编辑底图引用或附加说明无效。".into());
    }
    if mode != ImageEditingMode::Mask {
        return if mask.is_none() {
            Ok(())
        } else {
            Err("草图或标记模式不能携带独立遮罩。".into())
        };
    }
    let mask = mask.ok_or("局部修改缺少独立遮罩。")?;
    if mask.id.is_empty()
        || mask.id.len() > 128
        || mask.id == base.id
        || !is_edit_mask(mask)
        || base.mime_type != "image/png"
        || mask.mime_type != "image/png"
        || !base.width.is_some_and(|size| (1..=4096).contains(&size))
        || !base.height.is_some_and(|size| (1..=4096).contains(&size))
        || mask.width != base.width
        || mask.height != base.height
        || base.byte_len == 0
        || base.byte_len > 32 * 1024 * 1024
        || mask.byte_len == 0
        || mask.byte_len > 32 * 1024 * 1024
    {
        return Err("遮罩和底图必须是不同资源的同尺寸 PNG，单文件不能超过 32 MiB。".into());
    }
    Ok(())
}

impl From<&ImageEditingInput> for ImageEditingSnapshot {
    fn from(input: &ImageEditingInput) -> Self {
        Self {
            mode: input.mode,
            base_asset_id: input.base_asset_id.clone(),
            mask_asset_id: input.mask.as_ref().map(|mask| mask.id.clone()),
            instruction: input.instruction.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageEditingInput {
    pub mode: ImageEditingMode,
    pub base_asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<ImageAssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

impl ImageEditingInput {
    /// 元数据校验不能代替上传文件的 SHA、PNG Alpha 与解码尺寸校验。
    pub fn validate(&self, request: &GenerationRequest) -> Result<(), String> {
        let base = request
            .reference_assets
            .first()
            .filter(|asset| asset.id == self.base_asset_id)
            .ok_or("编辑底图必须是第一张参考图。")?;
        validate_edit_assets(
            self.mode,
            base,
            self.mask.as_ref(),
            self.instruction.as_deref(),
        )?;
        if self.mask.as_ref().is_some_and(|mask| {
            request
                .reference_assets
                .iter()
                .any(|asset| asset.id == mask.id)
        }) {
            return Err("独立遮罩不能混入参考图列表。".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenerationRequest, ProviderEndpointMode};
    use std::collections::BTreeMap;

    #[test]
    fn edit_instruction_is_appended_without_rewriting_user_prompt() {
        let prompt = "  保留文字和构图\n只改颜色  ";
        assert_eq!(compose_edit_prompt(prompt.into(), None), prompt);
        assert_eq!(compose_edit_prompt(prompt.into(), Some(" \n ")), prompt);
        assert_eq!(
            compose_edit_prompt(prompt.into(), Some(" 不保留辅助标记 ")),
            format!("{prompt}\n\n图像编辑附加说明：\n不保留辅助标记")
        );
    }

    #[test]
    fn restored_masks_use_the_same_size_type_and_length_rules_as_requests() {
        let base = asset("base", "image/png", 64, 64);
        let mut mask = asset("mask", "image/png", 64, 64);
        mask.metadata
            .insert("asset_role".into(), EDIT_MASK_ROLE.into());
        let snapshot = ImageEditingSnapshot {
            mode: ImageEditingMode::Mask,
            base_asset_id: "base".into(),
            mask_asset_id: Some("mask".into()),
            instruction: None,
        };
        let valid = [base, mask];
        assert!(snapshot.validate_resources(&valid).is_ok());
        for changed in 0..5 {
            let mut invalid = valid.clone();
            match changed {
                0 => invalid[1].width = Some(32),
                1 => invalid[1].mime_type = "image/jpeg".into(),
                2 => invalid[0].byte_len = 32 * 1024 * 1024 + 1,
                3 => invalid[1].byte_len = 0,
                _ => invalid[0].height = Some(0),
            }
            assert!(snapshot.validate_resources(&invalid).is_err());
            assert!(snapshot.restore(&invalid).is_err());
        }
    }

    #[test]
    fn snapshot_restore_never_drops_missing_or_invalid_mask() {
        let base = asset("base", "image/png", 2, 2);
        let mut mask = asset("mask", "image/png", 2, 2);
        let snapshot = ImageEditingSnapshot {
            mode: ImageEditingMode::Mask,
            base_asset_id: base.id.clone(),
            mask_asset_id: Some(mask.id.clone()),
            instruction: Some("保留背景".into()),
        };
        assert!(snapshot.restore(std::slice::from_ref(&base)).is_err());
        assert!(snapshot.restore(&[base.clone(), mask.clone()]).is_err());
        mask.metadata
            .insert("asset_role".into(), EDIT_MASK_ROLE.into());
        assert!(snapshot.restore(std::slice::from_ref(&mask)).is_err());
        let restored = snapshot.restore(&[base, mask]).unwrap();
        assert_eq!(ImageEditingSnapshot::from(&restored), snapshot);
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("data_url")
        );
    }

    fn asset(id: &str, mime: &str, width: u32, height: u32) -> ImageAssetRef {
        ImageAssetRef {
            id: id.into(),
            sha256: "a".repeat(64),
            mime_type: mime.into(),
            byte_len: 100,
            width: Some(width),
            height: Some(height),
            created_at: "now".into(),
            updated_at: "now".into(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: BTreeMap::new(),
        }
    }

    fn request() -> GenerationRequest {
        GenerationRequest {
            editing: None,
            automatic_size: false,
            prompt: "x".into(),
            model: "gpt-image-2".into(),
            width: 1024,
            height: 1024,
            quality: None,
            count: 1,
            endpoint_mode: ProviderEndpointMode::ImagesApi,
            reference_assets: vec![asset("base", "image/png", 1024, 1024)],
        }
    }

    #[test]
    fn mask_requires_first_base_and_independent_role() {
        let mut request = request();
        let mut mask = asset("mask", "image/png", 1024, 1024);
        mask.metadata
            .insert("asset_role".into(), EDIT_MASK_ROLE.into());
        let input = ImageEditingInput {
            mode: ImageEditingMode::Mask,
            base_asset_id: "base".into(),
            mask: Some(mask),
            instruction: None,
        };
        assert!(input.validate(&request).is_ok());
        request
            .reference_assets
            .push(asset("other", "image/png", 1024, 1024));
        request.reference_assets.reverse();
        assert!(input.validate(&request).is_err());
    }

    #[test]
    fn sketch_rejects_mask_and_metadata_is_backward_compatible() {
        let mut request = request();
        let mut mask = asset("mask", "image/png", 1024, 1024);
        mask.metadata
            .insert("asset_role".into(), EDIT_MASK_ROLE.into());
        let input = ImageEditingInput {
            mode: ImageEditingMode::Sketch,
            base_asset_id: "base".into(),
            mask: Some(mask),
            instruction: None,
        };
        assert!(input.validate(&request).is_err());
        request.editing = None;
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("editing"));
    }
}
