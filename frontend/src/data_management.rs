use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Read, Write},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mew_image_shared::{
    EncryptedApiConfig, LocalAppState, SyncEntityKind, apply_tombstones, merge_records,
    merge_tombstones, new_id, normalize_api_config, now_rfc3339,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const BACKUP_SCHEMA_VERSION: u32 = 1;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_UNPACKED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupAssetFile {
    path: String,
    mime_type: String,
    sha256: String,
    byte_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalBackupManifest {
    schema_version: u32,
    exported_at: String,
    app_version: String,
    workspace: LocalAppState,
    asset_files: BTreeMap<String, BackupAssetFile>,
}

pub struct ImportedBackup {
    pub state: LocalAppState,
    pub payloads: Vec<(String, String)>,
    pub imported_task_count: usize,
    pub imported_asset_count: usize,
    pub deduplicated_asset_count: usize,
}

pub fn build_backup(
    mut state: LocalAppState,
    payloads: &HashMap<String, String>,
) -> Result<Vec<u8>, String> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let stored_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut asset_files = BTreeMap::new();
    let mut written_paths = HashSet::new();
    let mut missing_payload_count = 0usize;

    for asset in &mut state.assets {
        let data_url = asset
            .data_url
            .take()
            .or_else(|| payloads.get(&asset.id).cloned());
        let Some(data_url) = data_url else {
            missing_payload_count += 1;
            continue;
        };
        let (mime_type, bytes) = decode_data_url(&data_url)?;
        let sha256 = sha256_hex(&bytes);
        let legacy_data_url_sha = sha256_hex(data_url.as_bytes());
        if !asset.sha256.is_empty()
            && !asset.sha256.eq_ignore_ascii_case(&sha256)
            && !asset.sha256.eq_ignore_ascii_case(&legacy_data_url_sha)
        {
            return Err(format!("图片 {} 的哈希校验失败，已停止导出。", asset.id));
        }
        asset.sha256 = sha256.clone();
        let path = format!("assets/{sha256}.{}", extension_from_mime(&mime_type));
        asset_files.insert(
            asset.id.clone(),
            BackupAssetFile {
                path: path.clone(),
                mime_type,
                sha256,
                byte_len: bytes.len() as u64,
            },
        );
        if written_paths.insert(path.clone()) {
            writer
                .start_file(path, stored_options)
                .map_err(|error| error.to_string())?;
            writer
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
        }
    }
    if missing_payload_count > 0 {
        return Err(format!(
            "有 {missing_payload_count} 张图片缺少可读取的本地或云端原文件，为避免生成残缺备份已停止导出。"
        ));
    }

    let manifest = LocalBackupManifest {
        schema_version: BACKUP_SCHEMA_VERSION,
        exported_at: now_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        workspace: scrub_export_payloads(state),
        asset_files,
    };
    writer
        .start_file("manifest.json", stored_options)
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    writer
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|error| error.to_string())
}

pub fn import_backup(bytes: &[u8], local: &LocalAppState) -> Result<ImportedBackup, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| "备份 ZIP 无法读取。")?;
    validate_archive_limits(&mut archive)?;
    let manifest = read_manifest(&mut archive)?;
    if manifest.schema_version != BACKUP_SCHEMA_VERSION {
        return Err(format!(
            "不支持的备份版本 {}，当前支持版本 {}。",
            manifest.schema_version, BACKUP_SCHEMA_VERSION
        ));
    }

    let imported_task_count = manifest.workspace.tasks.len();
    let imported_asset_count = manifest.workspace.assets.len();
    let (imported, payloads) = hydrate_imported_assets(&mut archive, manifest)?;
    let (state, payloads, deduplicated_asset_count) = merge_backup(local, imported, payloads);
    Ok(ImportedBackup {
        state,
        payloads,
        imported_task_count,
        imported_asset_count,
        deduplicated_asset_count,
    })
}

fn validate_archive_limits(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<(), String> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("备份文件条目过多，已拒绝导入。".into());
    }
    let mut unpacked = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        if file.enclosed_name().is_none() {
            return Err("备份包含不安全的文件路径。".into());
        }
        unpacked = unpacked
            .checked_add(file.size())
            .ok_or_else(|| "备份解压大小异常。".to_string())?;
        if unpacked > MAX_TOTAL_UNPACKED_BYTES {
            return Err("备份解压后超过 8 GiB 安全限制。".into());
        }
    }
    Ok(())
}

fn read_manifest(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<LocalBackupManifest, String> {
    let mut file = archive
        .by_name("manifest.json")
        .map_err(|_| "备份缺少 manifest.json。")?;
    if file.size() > MAX_MANIFEST_BYTES {
        return Err("备份清单过大，已拒绝导入。".into());
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("备份清单解析失败：{error}"))
}

fn hydrate_imported_assets(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    mut manifest: LocalBackupManifest,
) -> Result<(LocalAppState, HashMap<String, String>), String> {
    let mut file_cache = HashMap::<String, Vec<u8>>::new();
    let mut payloads = HashMap::new();
    for asset in &mut manifest.workspace.assets {
        asset.data_url = None;
        let Some(info) = manifest.asset_files.get(&asset.id) else {
            continue;
        };
        let bytes = if let Some(bytes) = file_cache.get(&info.path) {
            bytes.clone()
        } else {
            let mut file = archive
                .by_name(&info.path)
                .map_err(|_| format!("备份缺少图片文件 {}。", info.path))?;
            if file.size() != info.byte_len {
                return Err(format!("图片文件 {} 的长度不匹配。", info.path));
            }
            let mut bytes = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            file_cache.insert(info.path.clone(), bytes.clone());
            bytes
        };
        if sha256_hex(&bytes) != info.sha256.to_ascii_lowercase() {
            return Err(format!("图片文件 {} 的哈希校验失败。", info.path));
        }
        asset.sha256 = info.sha256.clone();
        asset.mime_type = info.mime_type.clone();
        asset.byte_len = bytes.len() as u64;
        payloads.insert(
            asset.id.clone(),
            format!("data:{};base64,{}", info.mime_type, BASE64.encode(bytes)),
        );
    }
    Ok((manifest.workspace, payloads))
}

fn merge_backup(
    local: &LocalAppState,
    mut imported: LocalAppState,
    imported_payloads: HashMap<String, String>,
) -> (LocalAppState, Vec<(String, String)>, usize) {
    let local_reference_by_sha = local
        .assets
        .iter()
        .filter(|asset| asset.source_task_id.is_none() && !asset.sha256.is_empty())
        .map(|asset| (asset.sha256.to_ascii_lowercase(), asset.id.clone()))
        .collect::<HashMap<_, _>>();
    let local_by_id = local
        .assets
        .iter()
        .map(|asset| (asset.id.clone(), asset.sha256.to_ascii_lowercase()))
        .collect::<HashMap<_, _>>();
    let mut id_remap = HashMap::new();
    let mut deduplicated_asset_ids = HashSet::new();
    let mut deduplicated = 0usize;

    for asset in &mut imported.assets {
        let original_id = asset.id.clone();
        let sha = asset.sha256.to_ascii_lowercase();
        if asset.source_task_id.is_none()
            && let Some(existing_id) = local_reference_by_sha.get(&sha)
        {
            deduplicated_asset_ids.insert(original_id.clone());
            id_remap.insert(original_id, existing_id.clone());
            deduplicated += 1;
        } else if local_by_id
            .get(&original_id)
            .map(|existing_sha| existing_sha != &sha)
            .unwrap_or(false)
        {
            asset.id = new_id();
            id_remap.insert(original_id, asset.id.clone());
        }
    }
    remap_asset_references(&mut imported, &id_remap);

    let tombstones = merge_tombstones(&local.tombstones, &imported.tombstones);
    let mut configs = apply_tombstones(
        merge_records(&local.configs, &imported.configs),
        &tombstones,
        SyncEntityKind::Config,
    );
    preserve_local_plaintext_keys(&mut configs, &local.configs);
    for config in &mut configs {
        normalize_api_config(config);
    }
    let imported_assets = imported
        .assets
        .into_iter()
        .filter(|asset| !deduplicated_asset_ids.contains(&asset.id))
        .collect::<Vec<_>>();
    let assets = apply_tombstones(
        merge_records(&local.assets, &imported_assets),
        &tombstones,
        SyncEntityKind::Asset,
    );
    let active_asset_ids = assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<HashSet<_>>();
    let payloads = imported_payloads
        .into_iter()
        .filter_map(|(id, payload)| {
            let mapped = id_remap.get(&id).cloned().unwrap_or(id);
            (active_asset_ids.contains(mapped.as_str())
                && !local.assets.iter().any(|asset| asset.id == mapped))
            .then_some((mapped, payload))
        })
        .collect();
    let imported_is_newer = imported
        .threads
        .iter()
        .map(|thread| thread.updated_at.as_str())
        .max()
        > local
            .threads
            .iter()
            .map(|thread| thread.updated_at.as_str())
            .max();

    (
        LocalAppState {
            configs,
            tasks: apply_tombstones(
                merge_records(&local.tasks, &imported.tasks),
                &tombstones,
                SyncEntityKind::Task,
            ),
            threads: apply_tombstones(
                merge_records(&local.threads, &imported.threads),
                &tombstones,
                SyncEntityKind::Thread,
            ),
            assets,
            preferences: if imported_is_newer {
                imported.preferences
            } else {
                local.preferences.clone()
            },
            checkpoint: local.checkpoint.clone(),
            tombstones,
        },
        payloads,
        deduplicated,
    )
}

fn remap_asset_references(state: &mut LocalAppState, remap: &HashMap<String, String>) {
    for task in &mut state.tasks {
        for id in &mut task.reference_asset_ids {
            if let Some(mapped) = remap.get(id) {
                *id = mapped.clone();
            }
        }
    }
}

fn preserve_local_plaintext_keys(
    configs: &mut [EncryptedApiConfig],
    local_configs: &[EncryptedApiConfig],
) {
    for config in configs {
        if config.api_key_plaintext.is_some() {
            continue;
        }
        config.api_key_plaintext = local_configs
            .iter()
            .find(|local| local.id == config.id)
            .and_then(|local| local.api_key_plaintext.clone());
    }
}

fn scrub_export_payloads(mut state: LocalAppState) -> LocalAppState {
    for config in &mut state.configs {
        config.api_key_plaintext = None;
    }
    for task in &mut state.tasks {
        let Some(result) = task.result.as_mut() else {
            continue;
        };
        for image in &mut result.images {
            image.data_url = None;
        }
        result.raw_response_json = None;
    }
    state
}

fn decode_data_url(data_url: &str) -> Result<(String, Vec<u8>), String> {
    let (header, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "图片不是有效的 data URL。".to_string())?;
    let mime_type = header
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| "图片 data URL 不是 Base64 格式。".to_string())?;
    let bytes = BASE64
        .decode(payload)
        .map_err(|error| format!("图片 Base64 解码失败：{error}"))?;
    Ok((mime_type.into(), bytes))
}

fn extension_from_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        _ => "png",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::ImageAssetRef;

    #[test]
    fn backup_round_trip_scrubs_plaintext_key() {
        let mut state = LocalAppState::default();
        state.configs.push(EncryptedApiConfig {
            api_key_plaintext: Some("secret".into()),
            ..crate::providers::default_config(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID)
        });
        let bytes = build_backup(state.clone(), &HashMap::new()).unwrap();
        let imported = import_backup(&bytes, &LocalAppState::default()).unwrap();
        assert_eq!(imported.state.configs[0].api_key_plaintext, None);
    }

    #[test]
    fn duplicate_image_reuses_existing_asset_id() {
        let bytes = b"same image";
        let sha = sha256_hex(bytes);
        let local_asset = test_asset("local-asset", &sha, bytes);
        let imported_asset = test_asset("imported-asset", &sha, bytes);
        let mut local = LocalAppState::default();
        local.assets.push(local_asset);
        let mut backup = LocalAppState::default();
        backup.assets.push(imported_asset);
        backup.tasks.push(mew_image_shared::LocalTaskRecord {
            id: "task".into(),
            thread_id: backup.threads[0].id.clone(),
            config_id: String::new(),
            prompt: "test".into(),
            requested_model: "test".into(),
            reference_asset_ids: vec!["imported-asset".into()],
            generation_settings: None,
            result: None,
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: true,
            status: mew_image_shared::TaskStatus::Failed,
            error_message: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        });
        let zip = build_backup(backup, &HashMap::new()).unwrap();
        let imported = import_backup(&zip, &local).unwrap();
        assert_eq!(imported.state.assets.len(), 1);
        assert_eq!(imported.state.tasks[0].reference_asset_ids, ["local-asset"]);
        assert!(imported.state.tasks[0].detached_from_thread);
        assert_eq!(imported.deduplicated_asset_count, 1);
    }

    fn test_asset(id: &str, sha: &str, bytes: &[u8]) -> ImageAssetRef {
        ImageAssetRef {
            id: id.into(),
            sha256: sha.into(),
            mime_type: "image/png".into(),
            byte_len: bytes.len() as u64,
            width: Some(1),
            height: Some(1),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: Some(format!("data:image/png;base64,{}", BASE64.encode(bytes))),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: BTreeMap::new(),
        }
    }
}
