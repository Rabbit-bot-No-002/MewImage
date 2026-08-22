use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Read, Write},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mew_image_shared::{
    AppPreferences, EncryptedApiConfig, LocalAppState, SyncCheckpoint, SyncEntityKind,
    apply_tombstones, merge_asset_records, merge_records, merge_tombstones, new_id,
    normalize_api_config, now_rfc3339,
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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    #[default]
    Workspace,
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalBackupManifest {
    schema_version: u32,
    exported_at: String,
    app_version: String,
    #[serde(default)]
    backup_kind: BackupKind,
    workspace: LocalAppState,
    asset_files: BTreeMap<String, BackupAssetFile>,
}

pub struct ImportedBackup {
    pub state: LocalAppState,
    pub payloads: Vec<(String, String)>,
    pub imported_task_count: usize,
    pub imported_asset_count: usize,
    pub deduplicated_asset_count: usize,
    pub backup_kind: BackupKind,
    pub imported_thread_id: Option<String>,
    pub imported_thread_title: Option<String>,
}

pub fn build_backup(
    state: LocalAppState,
    payloads: &HashMap<String, String>,
) -> Result<Vec<u8>, String> {
    build_archive(state, payloads, BackupKind::Workspace)
}

pub fn build_session_backup(
    state: LocalAppState,
    payloads: &HashMap<String, String>,
) -> Result<Vec<u8>, String> {
    build_archive(state, payloads, BackupKind::Session)
}

pub fn prepare_session_backup(
    state: &LocalAppState,
    thread_id: &str,
) -> Result<LocalAppState, String> {
    let mut thread = state
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .cloned()
        .ok_or_else(|| "未找到需要导出的会话。".to_string())?;
    let tasks = state
        .tasks
        .iter()
        .filter(|task| task.thread_id == thread_id && !task.detached_from_thread)
        .cloned()
        .collect::<Vec<_>>();
    let task_ids = tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let mut asset_ids = tasks
        .iter()
        .flat_map(|task| task.reference_asset_ids.iter().cloned())
        .collect::<HashSet<_>>();
    let available_asset_ids = state
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<HashSet<_>>();
    if let Some(missing_id) = asset_ids
        .iter()
        .find(|asset_id| !available_asset_ids.contains(asset_id.as_str()))
    {
        return Err(format!(
            "会话引用的图片 {missing_id} 已丢失，无法生成完整项目包。"
        ));
    }
    asset_ids.extend(
        state
            .assets
            .iter()
            .filter(|asset| {
                asset
                    .source_task_id
                    .as_ref()
                    .map(|task_id| task_ids.contains(task_id))
                    .unwrap_or(false)
                    || asset.metadata.get("thread_id").map(String::as_str) == Some(thread_id)
            })
            .map(|asset| asset.id.clone()),
    );
    let mut assets = state
        .assets
        .iter()
        .filter(|asset| asset_ids.contains(&asset.id))
        .cloned()
        .collect::<Vec<_>>();
    for asset in &mut assets {
        if asset
            .source_task_id
            .as_ref()
            .map(|task_id| !task_ids.contains(task_id))
            .unwrap_or(false)
        {
            // 跨会话引用只作为参考图导出，避免连带依赖外部任务。
            asset.source_task_id = None;
        }
    }
    thread.task_ids.retain(|task_id| task_ids.contains(task_id));
    for task in &tasks {
        if !thread.task_ids.contains(&task.id) {
            thread.task_ids.push(task.id.clone());
        }
    }

    Ok(LocalAppState {
        configs: Vec::new(),
        tasks,
        threads: vec![thread],
        assets,
        preferences: AppPreferences::default(),
        checkpoint: SyncCheckpoint::default(),
        tombstones: Vec::new(),
    })
}

fn build_archive(
    mut state: LocalAppState,
    payloads: &HashMap<String, String>,
    backup_kind: BackupKind,
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
        backup_kind,
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
    let backup_kind = manifest.backup_kind;
    let (imported, payloads) = hydrate_imported_assets(&mut archive, manifest)?;
    let (state, payloads, deduplicated_asset_count, imported_thread) = match backup_kind {
        BackupKind::Workspace => {
            let (state, payloads, deduplicated) = merge_backup(local, imported, payloads);
            (state, payloads, deduplicated, None)
        }
        BackupKind::Session => {
            let (state, payloads, thread_id, thread_title) =
                import_session_backup(local, imported, payloads)?;
            (state, payloads, 0, Some((thread_id, thread_title)))
        }
    };
    Ok(ImportedBackup {
        state,
        payloads,
        imported_task_count,
        imported_asset_count,
        deduplicated_asset_count,
        backup_kind,
        imported_thread_id: imported_thread.as_ref().map(|(id, _)| id.clone()),
        imported_thread_title: imported_thread.map(|(_, title)| title),
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
        merge_asset_records(&local.assets, &imported_assets),
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

fn import_session_backup(
    local: &LocalAppState,
    mut imported: LocalAppState,
    imported_payloads: HashMap<String, String>,
) -> Result<(LocalAppState, Vec<(String, String)>, String, String), String> {
    if imported.threads.len() != 1 {
        return Err("会话项目包必须且只能包含一个会话。".into());
    }
    let task_ids = imported
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    let asset_ids = imported
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<HashSet<_>>();
    if task_ids.len() != imported.tasks.len() || asset_ids.len() != imported.assets.len() {
        return Err("会话项目包包含重复的任务或图片 ID。".into());
    }
    if let Some(missing_asset_id) = imported
        .tasks
        .iter()
        .flat_map(|task| task.reference_asset_ids.iter())
        .find(|asset_id| !asset_ids.contains(asset_id.as_str()))
    {
        return Err(format!("会话项目包缺少引用图片 {missing_asset_id}。"));
    }
    if let Some(missing_payload_id) = imported
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .find(|asset_id| !imported_payloads.contains_key(*asset_id))
    {
        return Err(format!("会话项目包缺少图片文件 {missing_payload_id}。"));
    }
    let mut thread = imported.threads.remove(0);
    let mut reserved_ids = local
        .threads
        .iter()
        .map(|item| item.id.clone())
        .chain(local.tasks.iter().map(|item| item.id.clone()))
        .chain(local.assets.iter().map(|item| item.id.clone()))
        .collect::<HashSet<_>>();
    let new_thread_id = fresh_backup_id(&mut reserved_ids);
    let task_id_remap = imported
        .tasks
        .iter()
        .map(|task| (task.id.clone(), fresh_backup_id(&mut reserved_ids)))
        .collect::<HashMap<_, _>>();
    let asset_id_remap = imported
        .assets
        .iter()
        .map(|asset| (asset.id.clone(), fresh_backup_id(&mut reserved_ids)))
        .collect::<HashMap<_, _>>();
    let imported_at = now_rfc3339();

    for task in &mut imported.tasks {
        task.id = task_id_remap
            .get(&task.id)
            .cloned()
            .ok_or_else(|| "会话任务 ID 重映射失败。".to_string())?;
        task.thread_id = new_thread_id.clone();
        task.reference_asset_ids = task
            .reference_asset_ids
            .iter()
            .map(|asset_id| {
                asset_id_remap
                    .get(asset_id)
                    .cloned()
                    .ok_or_else(|| "会话参考图 ID 重映射失败。".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        task.favorite = false;
        task.favorite_folder_id = None;
        task.detached_from_thread = false;
    }

    for asset in &mut imported.assets {
        let original_id = asset.id.clone();
        asset.id = asset_id_remap
            .get(&original_id)
            .cloned()
            .ok_or_else(|| "会话图片 ID 重映射失败。".to_string())?;
        asset.source_task_id = asset
            .source_task_id
            .as_ref()
            .and_then(|task_id| task_id_remap.get(task_id).cloned());
        asset.remote_object_key = None;
        asset.remote_url = None;
        if asset.source_task_id.is_none() {
            asset
                .metadata
                .insert("thread_id".into(), new_thread_id.clone());
        } else {
            asset.metadata.remove("thread_id");
        }
        if let Some(mask_base_id) = asset.metadata.get("mask_base_asset_id").cloned() {
            if let Some(mapped) = asset_id_remap.get(&mask_base_id) {
                asset
                    .metadata
                    .insert("mask_base_asset_id".into(), mapped.clone());
            } else {
                asset.metadata.remove("mask_base_asset_id");
            }
        }
    }

    let mut ordered_task_ids = thread
        .task_ids
        .iter()
        .filter_map(|task_id| task_id_remap.get(task_id).cloned())
        .collect::<Vec<_>>();
    for task in &imported.tasks {
        if !ordered_task_ids.contains(&task.id) {
            ordered_task_ids.push(task.id.clone());
        }
    }
    thread.id = new_thread_id.clone();
    thread.title = unique_imported_thread_title(&thread.title, &local.threads);
    thread.task_ids = ordered_task_ids;
    thread.created_at = imported_at.clone();
    thread.updated_at = imported_at;

    let payloads = imported_payloads
        .into_iter()
        .filter_map(|(asset_id, payload)| {
            asset_id_remap
                .get(&asset_id)
                .cloned()
                .map(|mapped| (mapped, payload))
        })
        .collect::<Vec<_>>();
    let imported_thread_title = thread.title.clone();
    let mut state = local.clone();
    state.tasks.extend(imported.tasks);
    state.assets.extend(imported.assets);
    state.threads.push(thread);
    Ok((state, payloads, new_thread_id, imported_thread_title))
}

fn fresh_backup_id(reserved_ids: &mut HashSet<String>) -> String {
    loop {
        let candidate = new_id();
        if reserved_ids.insert(candidate.clone()) {
            return candidate;
        }
    }
}

fn unique_imported_thread_title(
    original_title: &str,
    local_threads: &[mew_image_shared::ConversationThread],
) -> String {
    let base_title = if original_title.trim().is_empty() {
        "新的会话"
    } else {
        original_title.trim()
    };
    let existing_titles = local_threads
        .iter()
        .map(|thread| thread.title.as_str())
        .collect::<HashSet<_>>();
    if !existing_titles.contains(base_title) {
        return base_title.to_string();
    }
    let first_copy = format!("{base_title}（导入）");
    if !existing_titles.contains(first_copy.as_str()) {
        return first_copy;
    }
    for suffix in 2.. {
        let candidate = format!("{base_title}（导入 {suffix}）");
        if !existing_titles.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!()
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
    use mew_image_shared::{
        ConversationThread, ImageAssetRef, LocalTaskRecord, SyncTombstone, TaskStatus,
    };

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
        assert_eq!(imported.backup_kind, BackupKind::Workspace);
    }

    #[test]
    fn legacy_manifest_without_backup_kind_defaults_to_workspace() {
        let value = serde_json::json!({
            "schema_version": BACKUP_SCHEMA_VERSION,
            "exported_at": now_rfc3339(),
            "app_version": "legacy",
            "workspace": LocalAppState::default(),
            "asset_files": {},
        });
        let manifest: LocalBackupManifest = serde_json::from_value(value).unwrap();
        assert_eq!(manifest.backup_kind, BackupKind::Workspace);
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

    #[test]
    fn session_backup_only_contains_selected_thread_dependencies() {
        let mut source = LocalAppState::default();
        source.threads = vec![
            test_thread("project", "项目 A"),
            test_thread("other", "其他"),
        ];
        source.tasks = vec![
            test_task("project-task", "project", &["cross-reference"], true),
            test_task("other-task", "other", &[], false),
        ];
        source.assets = vec![
            test_scoped_asset("project-output", b"project", Some("project-task"), None),
            test_scoped_asset("unused-reference", b"unused", None, Some("project")),
            test_scoped_asset("cross-reference", b"cross", Some("other-task"), None),
            test_scoped_asset("other-output", b"other", Some("other-task"), None),
        ];
        source.configs.push(EncryptedApiConfig {
            api_key_plaintext: Some("secret".into()),
            ..crate::providers::default_config(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID)
        });
        source.tombstones.push(SyncTombstone {
            entity_kind: SyncEntityKind::Thread,
            entity_id: "deleted".into(),
            deleted_at: now_rfc3339(),
        });

        let prepared = prepare_session_backup(&source, "project").unwrap();
        let asset_ids = prepared
            .assets
            .iter()
            .map(|asset| asset.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(prepared.threads.len(), 1);
        assert_eq!(prepared.tasks.len(), 1);
        assert!(prepared.configs.is_empty());
        assert!(prepared.tombstones.is_empty());
        assert_eq!(
            asset_ids,
            HashSet::from(["project-output", "unused-reference", "cross-reference"])
        );
        assert_eq!(
            prepared
                .assets
                .iter()
                .find(|asset| asset.id == "cross-reference")
                .and_then(|asset| asset.source_task_id.as_deref()),
            None
        );
    }

    #[test]
    fn session_import_creates_independent_copy_and_can_repeat() {
        let mut source = LocalAppState::default();
        source.threads = vec![test_thread("project", "项目 A")];
        source.tasks = vec![test_task("project-task", "project", &["reference"], true)];
        source.assets = vec![
            test_scoped_asset("reference", b"reference", None, Some("project")),
            test_scoped_asset("output", b"output", Some("project-task"), None),
        ];
        source.assets[0].remote_object_key = Some("users/old/reference".into());
        source.assets[0].remote_url = Some("/api/assets/reference".into());
        let prepared = prepare_session_backup(&source, "project").unwrap();
        let zip = build_session_backup(prepared, &HashMap::new()).unwrap();

        let mut local = LocalAppState::default();
        local.threads = vec![test_thread("project", "项目 A")];
        local.configs.push(EncryptedApiConfig {
            api_key_plaintext: Some("local-secret".into()),
            ..crate::providers::default_config(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID)
        });
        local.preferences.clear_prompt_after_submit = true;
        local.tombstones.push(SyncTombstone {
            entity_kind: SyncEntityKind::Thread,
            entity_id: "project".into(),
            deleted_at: now_rfc3339(),
        });
        let first = import_backup(&zip, &local).unwrap();
        let imported_thread_id = first.imported_thread_id.clone().unwrap();
        assert_eq!(first.backup_kind, BackupKind::Session);
        assert_ne!(imported_thread_id, "project");
        assert_eq!(
            first.imported_thread_title.as_deref(),
            Some("项目 A（导入）")
        );
        assert_eq!(first.state.threads.len(), 2);
        assert_eq!(first.state.configs, local.configs);
        assert_eq!(first.state.preferences, local.preferences);
        assert_eq!(first.state.tombstones, local.tombstones);
        assert_eq!(first.state.tasks.len(), 1);
        assert_eq!(first.state.assets.len(), 2);
        assert!(first.state.tasks[0].reference_asset_ids[0] != "reference");
        assert!(!first.state.tasks[0].favorite);
        assert_eq!(first.state.tasks[0].favorite_folder_id, None);
        assert_eq!(first.state.tasks[0].thread_id, imported_thread_id);
        assert!(
            first
                .state
                .assets
                .iter()
                .all(|asset| asset.remote_object_key.is_none() && asset.remote_url.is_none())
        );
        assert_eq!(first.payloads.len(), 2);

        let second = import_backup(&zip, &first.state).unwrap();
        assert_eq!(second.state.threads.len(), 3);
        assert_eq!(
            second.imported_thread_title.as_deref(),
            Some("项目 A（导入 2）")
        );
        assert_ne!(second.imported_thread_id, first.imported_thread_id);
    }

    fn test_thread(id: &str, title: &str) -> ConversationThread {
        ConversationThread {
            id: id.into(),
            title: title.into(),
            draft_prompt: format!("draft-{id}"),
            task_ids: vec![format!("{id}-task")],
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        }
    }

    fn test_task(
        id: &str,
        thread_id: &str,
        reference_asset_ids: &[&str],
        favorite: bool,
    ) -> LocalTaskRecord {
        LocalTaskRecord {
            id: id.into(),
            thread_id: thread_id.into(),
            config_id: "config".into(),
            prompt: "test".into(),
            requested_model: "test".into(),
            reference_asset_ids: reference_asset_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
            generation_settings: None,
            result: None,
            favorite,
            favorite_folder_id: favorite.then(|| "folder".into()),
            detached_from_thread: false,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        }
    }

    fn test_scoped_asset(
        id: &str,
        bytes: &[u8],
        source_task_id: Option<&str>,
        thread_id: Option<&str>,
    ) -> ImageAssetRef {
        let mut asset = test_asset(id, &sha256_hex(bytes), bytes);
        asset.source_task_id = source_task_id.map(str::to_string);
        if let Some(thread_id) = thread_id {
            asset.metadata.insert("thread_id".into(), thread_id.into());
        }
        asset
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
