use std::{collections::BTreeSet, sync::Arc};

use aes_gcm_siv::{
    Aes256GcmSiv, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mew_image_shared::{
    AccountKind, AdminUserSummary, EncryptedApiConfig, ManagedAccountCreateRequest,
    ManagedAccountCreateResponse, ManagedPasswordResetResponse, ManagedProviderAdminListResponse,
    ManagedProviderAdminView, ManagedProviderBulkCredentialsRequest, ManagedProviderConfigInput,
    ManagedProviderConfigWriteRequest, ManagedProviderEnabledRequest, ManagedProviderListResponse,
    ManagedProviderModelRequest, ManagedProviderMutationResponse, ManagedProviderSummary,
    ManagedProviderTemplateAdminView, ManagedProviderTemplateListResponse,
    ManagedProviderTemplateMutationResponse, ManagedProviderTemplateTargetsRequest,
    ManagedProviderTemplateWriteRequest, ProviderAccessMode, ProviderKind, new_id,
    normalize_api_config, now_rfc3339,
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, SqlitePool};
use tower_sessions::Session;

use crate::{
    AppError, AppState, hash_password_with_limit, require_admin, require_approved_user,
    resolve_provider_base_url, username_exists, validate_template,
};

const MANAGED_PAYLOAD_VERSION: u8 = 1;
const MAX_BULK_CONFIGS: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedSecretPayload {
    version: u8,
    config: EncryptedApiConfig,
    template: mew_image_shared::ProviderTemplate,
}

struct ManagedConfigRow {
    id: String,
    user_id: String,
    username: String,
    encrypted_payload: String,
    api_key_hint: String,
    enabled: bool,
    updated_at: String,
    updated_by: String,
    source_template_id: Option<String>,
    source_template_revision: Option<u64>,
}

struct ManagedTemplateRow {
    id: String,
    encrypted_payload: String,
    api_key_hint: String,
    revision: u64,
    enabled: bool,
    updated_at: String,
    updated_by: String,
}

struct ConfigPersistence<'a> {
    payload: &'a ManagedSecretPayload,
    encrypted: &'a str,
    key_hint: &'a str,
    enabled: bool,
    now: &'a str,
    updated_by: &'a str,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/admin/managed-users", post(create_managed_account))
        .route(
            "/api/admin/managed-users/{user_id}/reset-password",
            post(reset_managed_password),
        )
        .route(
            "/api/admin/managed-users/{user_id}/providers",
            post(add_managed_provider),
        )
        .route(
            "/api/admin/managed-providers",
            get(list_admin_managed_providers),
        )
        .route(
            "/api/admin/managed-providers/bulk-credentials",
            post(bulk_update_credentials),
        )
        .route(
            "/api/admin/managed-providers/{config_id}",
            post(update_managed_provider).delete(delete_managed_provider),
        )
        .route(
            "/api/admin/managed-providers/{config_id}/enabled",
            post(set_managed_provider_enabled),
        )
        .route(
            "/api/admin/managed-provider-templates",
            get(list_managed_provider_templates).post(create_managed_provider_template),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}",
            post(update_managed_provider_template).delete(delete_managed_provider_template),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}/enabled",
            post(set_managed_provider_template_enabled),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}/assign",
            post(assign_managed_provider_template),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}/assign-preview",
            post(preview_managed_provider_template_assignment),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}/sync",
            post(sync_managed_provider_template),
        )
        .route(
            "/api/admin/managed-provider-templates/{template_id}/sync-preview",
            post(preview_managed_provider_template_sync),
        )
        .route("/api/managed/providers", get(list_managed_providers))
        .route(
            "/api/managed/providers/{config_id}/model",
            post(select_managed_provider_model),
        )
}

pub async fn init_db(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS managed_provider_configs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL COLLATE NOCASE,
            provider_kind TEXT NOT NULL,
            endpoint_mode TEXT NOT NULL,
            encrypted_payload TEXT NOT NULL,
            api_key_hint TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            updated_by TEXT NOT NULL,
            UNIQUE(user_id, name)
        )"#,
    )
    .execute(db)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS managed_provider_configs_user ON managed_provider_configs(user_id, updated_at DESC)",
    )
    .execute(db)
    .await?;
    let columns = sqlx::query("PRAGMA table_info(managed_provider_configs)")
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<BTreeSet<_>>();
    for (name, definition) in [
        ("source_template_id", "TEXT"),
        ("source_template_revision", "INTEGER"),
    ] {
        if !columns.contains(name) {
            sqlx::query(&format!(
                "ALTER TABLE managed_provider_configs ADD COLUMN {name} {definition}"
            ))
            .execute(db)
            .await?;
        }
    }
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS managed_provider_config_source ON managed_provider_configs(user_id, source_template_id) WHERE source_template_id IS NOT NULL")
        .execute(db)
        .await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS managed_provider_templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            provider_kind TEXT NOT NULL,
            endpoint_mode TEXT NOT NULL,
            encrypted_payload TEXT NOT NULL,
            api_key_hint TEXT NOT NULL,
            revision INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            updated_by TEXT NOT NULL
        )"#,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn validate_startup_state(state: &AppState) -> anyhow::Result<()> {
    let rows = sqlx::query("SELECT id, user_id, encrypted_payload FROM managed_provider_configs")
        .fetch_all(&state.db)
        .await?;
    for row in rows {
        let id = row.get::<String, _>("id");
        let user_id = row.get::<String, _>("user_id");
        let encrypted = row.get::<String, _>("encrypted_payload");
        decrypt_payload(state, &user_id, &id, &encrypted).map_err(|error| {
            anyhow::anyhow!(
                "托管服务商配置 {id} 无法解密：{}。请恢复部署时使用的 MEW_MANAGED_PROVIDER_SECRET。",
                error.message
            )
        })?;
    }
    let rows = sqlx::query("SELECT id, encrypted_payload FROM managed_provider_templates")
        .fetch_all(&state.db)
        .await?;
    for row in rows {
        let id = row.get::<String, _>("id");
        let encrypted = row.get::<String, _>("encrypted_payload");
        decrypt_payload(state, "template", &id, &encrypted).map_err(|error| {
            anyhow::anyhow!("托管服务商模板 {id} 无法解密：{}。请恢复部署时使用的 MEW_MANAGED_PROVIDER_SECRET。", error.message)
        })?;
    }
    Ok(())
}

pub async fn resolve_generation_config(
    state: &AppState,
    user_id: &str,
    config_id: &str,
    requested_model: &str,
) -> Result<(EncryptedApiConfig, mew_image_shared::ProviderTemplate), AppError> {
    let row = load_owned_row(state, user_id, config_id).await?;
    if !row.enabled {
        return Err(AppError::bad_request("该托管服务商配置当前已停用。"));
    }
    let mut payload = decrypt_payload(state, &row.user_id, &row.id, &row.encrypted_payload)?;
    if !payload
        .config
        .available_models
        .iter()
        .any(|model| model == requested_model)
    {
        return Err(AppError::bad_request(
            "请求的模型不在管理员允许的模型列表中。",
        ));
    }
    payload.config.model = requested_model.to_string();
    payload.config.access_mode = ProviderAccessMode::Proxy;
    Ok((payload.config, payload.template))
}

async fn create_managed_account(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(request): Json<ManagedAccountCreateRequest>,
) -> Result<Json<ManagedAccountCreateResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    require_managed_secret(&state)?;
    let username = request.username.trim();
    if username.len() < 3 {
        return Err(AppError::bad_request("用户名至少 3 个字符。"));
    }
    if username_exists(&state.db, username).await? {
        return Err(AppError::bad_request("用户名已存在。"));
    }

    let temporary_password = generate_temporary_password();
    let password_hash = hash_password_with_limit(&state, temporary_password.clone()).await?;
    let user_id = new_id();
    let config_id = new_id();
    let now = now_rfc3339();
    let (payload, key_hint, source_template) = match (
        request.initial_provider,
        request
            .initial_template_id
            .filter(|id| !id.trim().is_empty()),
    ) {
        (Some(provider), None) => {
            let (payload, hint) =
                prepare_payload(&state, &user_id, &config_id, provider, None, &now)?;
            (payload, hint, None)
        }
        (None, Some(template_id)) => {
            let row = load_template_row(&state, &template_id).await?;
            if !row.enabled {
                return Err(AppError::bad_request("所选服务商模板已停用。"));
            }
            let source = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
            let payload = clone_template_payload(source, &config_id, &now);
            (
                payload,
                row.api_key_hint.clone(),
                Some((row.id, row.revision)),
            )
        }
        _ => {
            return Err(AppError::bad_request(
                "必须且只能选择手工配置或服务商模板之一。",
            ));
        }
    };
    let encrypted = encrypt_payload(&state, &user_id, &config_id, &payload)?;

    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, status, account_kind, must_change_password, password_updated_at, approved_at, approved_by, created_at) VALUES (?, ?, ?, 'user', 'approved', 'managed', 1, ?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(username)
    .bind(password_hash)
    .bind(&now)
    .bind(&now)
    .bind(&admin.id)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(map_unique_error)?;
    insert_config_row(
        &mut transaction,
        &config_id,
        &user_id,
        ConfigPersistence {
            payload: &payload,
            encrypted: &encrypted,
            key_hint: &key_hint,
            enabled: true,
            now: &now,
            updated_by: &admin.id,
        },
    )
    .await?;
    if let Some((template_id, revision)) = &source_template {
        sqlx::query("UPDATE managed_provider_configs SET source_template_id = ?, source_template_revision = ? WHERE id = ?")
            .bind(template_id)
            .bind(*revision as i64)
            .bind(&config_id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    let operation_id = new_id();
    crate::admin_console::record(
        &mut transaction,
        crate::admin_console::AuditRecord {
            operation_id: &operation_id,
            actor_user_id: &admin.id,
            actor_username: &admin.username,
            action: "managed_account.create",
            target_type: "user",
            target_id: &user_id,
            target_name: username,
            summary: "created",
        },
    )
    .await?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_provider.create",
        "managed_provider",
        &config_id,
        &payload.config.name,
        &user_id,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;

    Ok(Json(ManagedAccountCreateResponse {
        user: AdminUserSummary {
            id: user_id,
            username: username.to_string(),
            role: "user".into(),
            status: "approved".into(),
            image_count: 0,
            created_at: now.clone(),
            approved_at: Some(now),
            approved_by: Some(admin.id),
            last_login_at: None,
            last_active_at: None,
            account_kind: AccountKind::Managed,
            must_change_password: true,
            managed_provider_count: 1,
        },
        temporary_password,
    }))
}

async fn reset_managed_password(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(user_id): Path<String>,
) -> Result<Json<ManagedPasswordResetResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    ensure_managed_user(&state, &user_id).await?;
    let username = sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?;
    let temporary_password = generate_temporary_password();
    let password_hash = hash_password_with_limit(&state, temporary_password.clone()).await?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    sqlx::query(
        "UPDATE users SET password_hash = ?, password_updated_at = ?, must_change_password = 1, failed_login_count = 0, locked_until = NULL, session_version = session_version + 1 WHERE id = ? AND account_kind = 'managed'",
    )
    .bind(password_hash)
    .bind(now_rfc3339())
    .bind(&user_id)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::internal)?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_account.reset_password",
        "user",
        &user_id,
        &username,
        "temporary_password_issued",
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedPasswordResetResponse {
        user_id,
        temporary_password,
    }))
}

async fn list_admin_managed_providers(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<ManagedProviderAdminQuery>,
) -> Result<Json<ManagedProviderAdminListResponse>, AppError> {
    require_admin(&state, &session).await?;
    require_managed_secret(&state)?;
    let rows = if let Some(user_id) = query.user_id.filter(|value| !value.trim().is_empty()) {
        ensure_managed_user(&state, &user_id).await?;
        load_user_rows(&state, &user_id).await?
    } else {
        load_admin_rows(&state).await?
    };
    let mut configs = Vec::with_capacity(rows.len());
    for row in rows {
        let payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
        configs.push(admin_view(row, payload));
    }
    Ok(Json(ManagedProviderAdminListResponse { configs }))
}

#[derive(Debug, Deserialize)]
struct ManagedProviderAdminQuery {
    user_id: Option<String>,
}

async fn list_managed_providers(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<ManagedProviderListResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    if user.account_kind != AccountKind::Managed {
        return Err(AppError::bad_request("当前账号不是托管账号。"));
    }
    let rows = load_user_rows(&state, &user.id).await?;
    let mut configs = Vec::new();
    for row in rows.into_iter().filter(|row| row.enabled) {
        let payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
        configs.push(summary(&row, &payload));
    }
    Ok(Json(ManagedProviderListResponse { configs }))
}

async fn add_managed_provider(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(user_id): Path<String>,
    Json(request): Json<ManagedProviderConfigWriteRequest>,
) -> Result<Json<ManagedProviderAdminView>, AppError> {
    let admin = require_admin(&state, &session).await?;
    ensure_managed_user(&state, &user_id).await?;
    let config_id = new_id();
    let now = now_rfc3339();
    let (payload, key_hint) = prepare_payload(&state, &user_id, &config_id, request, None, &now)?;
    let encrypted = encrypt_payload(&state, &user_id, &config_id, &payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    insert_config_row(
        &mut transaction,
        &config_id,
        &user_id,
        ConfigPersistence {
            payload: &payload,
            encrypted: &encrypted,
            key_hint: &key_hint,
            enabled: true,
            now: &now,
            updated_by: &admin.id,
        },
    )
    .await?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_provider.create",
        "managed_provider",
        &config_id,
        &payload.config.name,
        &user_id,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    let row = load_owned_row(&state, &user_id, &config_id).await?;
    Ok(Json(admin_view(row, payload)))
}

async fn update_managed_provider(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(config_id): Path<String>,
    Json(request): Json<ManagedProviderConfigWriteRequest>,
) -> Result<Json<ManagedProviderAdminView>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_row_by_id(&state, &config_id).await?;
    let old = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
    let now = now_rfc3339();
    let (payload, key_hint) = prepare_payload(
        &state,
        &row.user_id,
        &row.id,
        request,
        old.config.api_key_plaintext,
        &now,
    )?;
    let encrypted = encrypt_payload(&state, &row.user_id, &row.id, &payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    update_config_on_connection(
        &mut transaction,
        &row.id,
        ConfigPersistence {
            payload: &payload,
            encrypted: &encrypted,
            key_hint: &key_hint,
            enabled: row.enabled,
            now: &now,
            updated_by: &admin.id,
        },
    )
    .await?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_provider.update",
        "managed_provider",
        &row.id,
        &payload.config.name,
        &row.user_id,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    let updated = load_row_by_id(&state, &row.id).await?;
    Ok(Json(admin_view(updated, payload)))
}

async fn set_managed_provider_enabled(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(config_id): Path<String>,
    Json(request): Json<ManagedProviderEnabledRequest>,
) -> Result<Json<ManagedProviderMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_row_by_id(&state, &config_id).await?;
    let payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    let result = sqlx::query(
        "UPDATE managed_provider_configs SET enabled = ?, updated_at = ?, updated_by = ? WHERE id = ?",
    )
    .bind(request.enabled)
    .bind(now_rfc3339())
    .bind(&admin.id)
    .bind(&config_id)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::not_found("托管服务商配置不存在。"));
    }
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_provider.enabled",
        "managed_provider",
        &config_id,
        &payload.config.name,
        if request.enabled {
            "enabled"
        } else {
            "disabled"
        },
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderMutationResponse { updated_count: 1 }))
}

async fn delete_managed_provider(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(config_id): Path<String>,
) -> Result<Json<ManagedProviderMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_row_by_id(&state, &config_id).await?;
    let payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    let result = sqlx::query("DELETE FROM managed_provider_configs WHERE id = ?")
        .bind(&config_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::not_found("托管服务商配置不存在。"));
    }
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_provider.delete",
        "managed_provider",
        &config_id,
        &payload.config.name,
        &row.user_id,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderMutationResponse { updated_count: 1 }))
}

async fn select_managed_provider_model(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(config_id): Path<String>,
    Json(request): Json<ManagedProviderModelRequest>,
) -> Result<Json<ManagedProviderSummary>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    if user.account_kind != AccountKind::Managed {
        return Err(AppError::bad_request("当前账号不是托管账号。"));
    }
    let row = load_owned_row(&state, &user.id, &config_id).await?;
    if !row.enabled {
        return Err(AppError::bad_request("该托管服务商配置当前已停用。"));
    }
    let mut payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
    let model = request.model.trim();
    if !payload
        .config
        .available_models
        .iter()
        .any(|available| available == model)
    {
        return Err(AppError::bad_request("模型不在管理员允许的模型列表中。"));
    }
    if payload.config.model != model {
        let now = now_rfc3339();
        payload.config.model = model.to_string();
        payload.config.updated_at = now.clone();
        let encrypted = encrypt_payload(&state, &row.user_id, &row.id, &payload)?;
        update_config_row(
            &state,
            &row.id,
            ConfigPersistence {
                payload: &payload,
                encrypted: &encrypted,
                key_hint: &row.api_key_hint,
                enabled: true,
                now: &now,
                updated_by: &user.id,
            },
        )
        .await?;
    }
    let updated = load_owned_row(&state, &user.id, &row.id).await?;
    Ok(Json(summary(&updated, &payload)))
}

async fn bulk_update_credentials(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(request): Json<ManagedProviderBulkCredentialsRequest>,
) -> Result<Json<ManagedProviderMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let ids = request
        .config_ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect::<BTreeSet<_>>();
    if ids.is_empty() || ids.len() > MAX_BULK_CONFIGS {
        return Err(AppError::bad_request("请选择 1–100 条托管配置。"));
    }
    let new_base_url = request.base_url.map(|value| value.trim().to_string());
    let new_api_key = request.api_key.filter(|value| !value.trim().is_empty());
    if new_base_url.as_deref().is_none_or(str::is_empty) && new_api_key.is_none() {
        return Err(AppError::bad_request("请填写新的上游地址或 API Key。"));
    }

    let mut prepared = Vec::with_capacity(ids.len());
    let mut expected_protocol = None;
    let now = now_rfc3339();
    for id in ids {
        let row = load_row_by_id(&state, &id).await?;
        let mut payload = decrypt_payload(&state, &row.user_id, &row.id, &row.encrypted_payload)?;
        let protocol = (payload.config.provider_kind, payload.config.endpoint_mode);
        if expected_protocol.is_some_and(|expected| expected != protocol) {
            return Err(AppError::bad_request(
                "批量更新只能选择相同服务商协议和接口模式的配置。",
            ));
        }
        expected_protocol = Some(protocol);
        if let Some(base_url) = &new_base_url {
            payload.config.base_url = base_url.clone();
        }
        if let Some(api_key) = &new_api_key {
            payload.config.api_key_plaintext = Some(api_key.clone());
        }
        validate_runtime_payload(&state, &mut payload, &now)?;
        let encrypted = encrypt_payload(&state, &row.user_id, &row.id, &payload)?;
        let hint = payload
            .config
            .api_key_plaintext
            .as_deref()
            .map(api_key_hint)
            .unwrap_or_default();
        prepared.push((row, payload, encrypted, hint));
    }

    let operation_id = new_id();
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    for (row, payload, encrypted, hint) in &prepared {
        update_config_on_connection(
            &mut transaction,
            &row.id,
            ConfigPersistence {
                payload,
                encrypted,
                key_hint: hint,
                enabled: row.enabled,
                now: &now,
                updated_by: &admin.id,
            },
        )
        .await?;
        crate::admin_console::record(
            &mut transaction,
            crate::admin_console::AuditRecord {
                operation_id: &operation_id,
                actor_user_id: &admin.id,
                actor_username: &admin.username,
                action: "managed_provider.bulk_credentials",
                target_type: "managed_provider",
                target_id: &row.id,
                target_name: &payload.config.name,
                summary: &row.username,
            },
        )
        .await?;
    }
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderMutationResponse {
        updated_count: prepared.len(),
    }))
}

async fn list_managed_provider_templates(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<ManagedTemplateListQuery>,
) -> Result<Json<ManagedProviderTemplateListResponse>, AppError> {
    require_admin(&state, &session).await?;
    require_managed_secret(&state)?;
    let page = query.page.unwrap_or(1).max(1);
    let limit = match query.limit.unwrap_or(20) {
        value @ (20 | 50 | 100) => value,
        _ => {
            return Err(AppError::bad_request(
                "服务商模板每页仅支持 20、50 或 100 条。",
            ));
        }
    };
    let keyword = query.q.unwrap_or_default().trim().to_string();
    if keyword.chars().count() > 100 {
        return Err(AppError::bad_request(
            "服务商模板搜索内容不能超过 100 个字符。",
        ));
    }
    let escaped = keyword
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM managed_provider_templates WHERE name LIKE ? ESCAPE '\\'",
    )
    .bind(&pattern)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?
    .max(0) as usize;
    let rows = sqlx::query(
        "SELECT t.id, t.encrypted_payload, t.api_key_hint, t.revision, t.enabled,
                t.updated_at, t.updated_by, COUNT(c.id) assigned_count,
                COALESCE(SUM(CASE WHEN c.source_template_revision < t.revision THEN 1 ELSE 0 END), 0) outdated_count
         FROM managed_provider_templates t
         LEFT JOIN managed_provider_configs c ON c.source_template_id = t.id
         WHERE t.name LIKE ? ESCAPE '\\'
         GROUP BY t.id ORDER BY t.updated_at DESC, t.id LIMIT ? OFFSET ?",
    )
    .bind(pattern)
    .bind(limit as i64)
    .bind(page.saturating_sub(1).saturating_mul(limit) as i64)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?;
    let mut templates = Vec::with_capacity(rows.len());
    for row in rows {
        let assigned_count = row.get::<i64, _>("assigned_count");
        let outdated_count = row.get::<i64, _>("outdated_count");
        let row = managed_template_row(row);
        let payload = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
        templates.push(template_admin_view(
            row,
            payload,
            assigned_count,
            outdated_count,
        ));
    }
    Ok(Json(ManagedProviderTemplateListResponse {
        templates,
        total,
        page,
        limit,
    }))
}

#[derive(Debug, Deserialize)]
struct ManagedTemplateListQuery {
    page: Option<usize>,
    limit: Option<usize>,
    q: Option<String>,
}

async fn create_managed_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(request): Json<ManagedProviderTemplateWriteRequest>,
) -> Result<Json<ManagedProviderTemplateAdminView>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let id = new_id();
    let now = now_rfc3339();
    let (payload, hint) = prepare_payload(
        &state,
        "template",
        &id,
        ManagedProviderConfigWriteRequest {
            config: request.config,
            api_key: request.api_key,
        },
        None,
        &now,
    )?;
    let encrypted = encrypt_payload(&state, "template", &id, &payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    sqlx::query("INSERT INTO managed_provider_templates (id, name, provider_kind, endpoint_mode, encrypted_payload, api_key_hint, revision, enabled, created_at, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, ?, 1, 1, ?, ?, ?)")
        .bind(&id).bind(&payload.config.name).bind(provider_kind_key(payload.config.provider_kind))
        .bind(endpoint_mode_key(payload.config.endpoint_mode)).bind(&encrypted).bind(&hint)
        .bind(&now).bind(&now).bind(&admin.id).execute(&mut *transaction).await.map_err(map_unique_error)?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_template.create",
        "managed_template",
        &id,
        &payload.config.name,
        "created",
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    let row = load_template_row(&state, &id).await?;
    Ok(Json(template_admin_view(row, payload, 0, 0)))
}

async fn update_managed_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderTemplateWriteRequest>,
) -> Result<Json<ManagedProviderTemplateAdminView>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_template_row(&state, &template_id).await?;
    let old = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
    let now = now_rfc3339();
    let (payload, hint) = prepare_payload(
        &state,
        "template",
        &row.id,
        ManagedProviderConfigWriteRequest {
            config: request.config,
            api_key: request.api_key,
        },
        old.config.api_key_plaintext,
        &now,
    )?;
    let encrypted = encrypt_payload(&state, "template", &row.id, &payload)?;
    let revision = row.revision.saturating_add(1);
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    sqlx::query("UPDATE managed_provider_templates SET name = ?, provider_kind = ?, endpoint_mode = ?, encrypted_payload = ?, api_key_hint = ?, revision = ?, updated_at = ?, updated_by = ? WHERE id = ?")
        .bind(&payload.config.name).bind(provider_kind_key(payload.config.provider_kind))
        .bind(endpoint_mode_key(payload.config.endpoint_mode)).bind(encrypted).bind(&hint)
        .bind(revision as i64).bind(&now).bind(&admin.id).bind(&row.id)
        .execute(&mut *transaction).await.map_err(map_unique_error)?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_template.update",
        "managed_template",
        &row.id,
        &payload.config.name,
        "revision_updated",
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    let updated = load_template_row(&state, &row.id).await?;
    let assigned = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM managed_provider_configs WHERE source_template_id = ?",
    )
    .bind(&row.id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    Ok(Json(template_admin_view(
        updated, payload, assigned, assigned,
    )))
}

async fn set_managed_provider_template_enabled(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderEnabledRequest>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_template_row(&state, &template_id).await?;
    let payload = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    sqlx::query("UPDATE managed_provider_templates SET enabled = ?, updated_at = ?, updated_by = ? WHERE id = ?")
        .bind(request.enabled).bind(now_rfc3339()).bind(&admin.id).bind(&row.id)
        .execute(&mut *transaction).await.map_err(AppError::internal)?;
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_template.enabled",
        "managed_template",
        &row.id,
        &payload.config.name,
        if request.enabled {
            "enabled"
        } else {
            "disabled"
        },
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: 1,
    }))
}

async fn delete_managed_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let row = load_template_row(&state, &template_id).await?;
    let payload = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    let unlinked = sqlx::query("UPDATE managed_provider_configs SET source_template_id = NULL, source_template_revision = NULL WHERE source_template_id = ?")
        .bind(&row.id).execute(&mut *transaction).await.map_err(AppError::internal)?.rows_affected();
    sqlx::query("DELETE FROM managed_provider_templates WHERE id = ?")
        .bind(&row.id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    let summary = format!("deleted; unlinked={unlinked}");
    record_managed_audit(
        &mut transaction,
        &admin,
        "managed_template.delete",
        "managed_template",
        &row.id,
        &payload.config.name,
        &summary,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: unlinked as usize,
    }))
}

async fn assign_managed_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderTemplateTargetsRequest>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let user_ids = validated_target_ids(request.user_ids)?;
    let row = load_template_row(&state, &template_id).await?;
    if !row.enabled {
        return Err(AppError::bad_request("该服务商模板已停用。"));
    }
    let source = decrypt_payload(&state, "template", &row.id, &row.encrypted_payload)?;
    let users = load_managed_user_names(&state.db, &user_ids).await?;
    ensure_template_not_assigned(&state.db, &row.id, &user_ids).await?;
    let now = now_rfc3339();
    let operation_id = new_id();
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    for (user_id, username) in &users {
        let config_id = new_id();
        let payload = clone_template_payload(source.clone(), &config_id, &now);
        let encrypted = encrypt_payload(&state, user_id, &config_id, &payload)?;
        insert_config_row(
            &mut transaction,
            &config_id,
            user_id,
            ConfigPersistence {
                payload: &payload,
                encrypted: &encrypted,
                key_hint: &row.api_key_hint,
                enabled: true,
                now: &now,
                updated_by: &admin.id,
            },
        )
        .await?;
        sqlx::query("UPDATE managed_provider_configs SET source_template_id = ?, source_template_revision = ? WHERE id = ?")
            .bind(&row.id).bind(row.revision as i64).bind(&config_id)
            .execute(&mut *transaction).await.map_err(AppError::internal)?;
        crate::admin_console::record(
            &mut transaction,
            crate::admin_console::AuditRecord {
                operation_id: &operation_id,
                actor_user_id: &admin.id,
                actor_username: &admin.username,
                action: "managed_template.assign",
                target_type: "user",
                target_id: user_id,
                target_name: username,
                summary: &payload.config.name,
            },
        )
        .await?;
    }
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: users.len(),
    }))
}

async fn preview_managed_provider_template_assignment(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderTemplateTargetsRequest>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    require_admin(&state, &session).await?;
    let user_ids = validated_target_ids(request.user_ids)?;
    let row = load_template_row(&state, &template_id).await?;
    if !row.enabled {
        return Err(AppError::bad_request("该服务商模板已停用。"));
    }
    load_managed_user_names(&state.db, &user_ids).await?;
    ensure_template_not_assigned(&state.db, &row.id, &user_ids).await?;
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: user_ids.len(),
    }))
}

async fn sync_managed_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderTemplateTargetsRequest>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    let admin = require_admin(&state, &session).await?;
    let user_ids = validated_target_ids(request.user_ids)?;
    let template_row = load_template_row(&state, &template_id).await?;
    let source = decrypt_payload(
        &state,
        "template",
        &template_row.id,
        &template_row.encrypted_payload,
    )?;
    let users = load_managed_user_names(&state.db, &user_ids).await?;
    let now = now_rfc3339();
    let operation_id = new_id();
    let mut prepared = Vec::with_capacity(users.len());
    for (user_id, username) in &users {
        let row = sqlx::query(
            "SELECT id FROM managed_provider_configs WHERE user_id = ? AND source_template_id = ?",
        )
        .bind(user_id)
        .bind(&template_row.id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::bad_request(format!("账号 {username} 未关联该模板。")))?;
        let config_row = load_owned_row(&state, user_id, &row.get::<String, _>("id")).await?;
        let old = decrypt_payload(
            &state,
            user_id,
            &config_row.id,
            &config_row.encrypted_payload,
        )?;
        let mut payload = clone_template_payload(source.clone(), &config_row.id, &now);
        payload.config.name = old.config.name;
        payload.config.created_at = old.config.created_at;
        if payload.config.available_models.contains(&old.config.model) {
            payload.config.model = old.config.model;
        }
        let encrypted = encrypt_payload(&state, user_id, &config_row.id, &payload)?;
        prepared.push((config_row, payload, encrypted, username.clone()));
    }
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    for (row, payload, encrypted, username) in &prepared {
        update_config_on_connection(
            &mut transaction,
            &row.id,
            ConfigPersistence {
                payload,
                encrypted,
                key_hint: &template_row.api_key_hint,
                enabled: row.enabled,
                now: &now,
                updated_by: &admin.id,
            },
        )
        .await?;
        sqlx::query(
            "UPDATE managed_provider_configs SET source_template_revision = ? WHERE id = ?",
        )
        .bind(template_row.revision as i64)
        .bind(&row.id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
        crate::admin_console::record(
            &mut transaction,
            crate::admin_console::AuditRecord {
                operation_id: &operation_id,
                actor_user_id: &admin.id,
                actor_username: &admin.username,
                action: "managed_template.sync",
                target_type: "user",
                target_id: &row.user_id,
                target_name: username,
                summary: &payload.config.name,
            },
        )
        .await?;
    }
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: prepared.len(),
    }))
}

async fn preview_managed_provider_template_sync(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(request): Json<ManagedProviderTemplateTargetsRequest>,
) -> Result<Json<ManagedProviderTemplateMutationResponse>, AppError> {
    require_admin(&state, &session).await?;
    let user_ids = validated_target_ids(request.user_ids)?;
    load_template_row(&state, &template_id).await?;
    let users = load_managed_user_names(&state.db, &user_ids).await?;
    if template_assignment_count(&state.db, &template_id, &user_ids).await? != users.len() {
        return Err(AppError::bad_request(
            "所选账号中包含未关联该服务商模板的账号。",
        ));
    }
    Ok(Json(ManagedProviderTemplateMutationResponse {
        affected_count: users.len(),
    }))
}

async fn ensure_template_not_assigned(
    db: &SqlitePool,
    template_id: &str,
    user_ids: &BTreeSet<String>,
) -> Result<(), AppError> {
    let assigned = template_assignment_count(db, template_id, user_ids).await?;
    if assigned > 0 {
        return Err(AppError::bad_request(
            "所选账号中已有账号分配过该服务商模板。",
        ));
    }
    Ok(())
}

async fn template_assignment_count(
    db: &SqlitePool,
    template_id: &str,
    user_ids: &BTreeSet<String>,
) -> Result<usize, AppError> {
    let mut query = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT COUNT(*) FROM managed_provider_configs WHERE source_template_id = ",
    );
    query.push_bind(template_id).push(" AND user_id IN (");
    let mut separated = query.separated(", ");
    for user_id in user_ids {
        separated.push_bind(user_id);
    }
    separated.push_unseparated(")");
    let assigned = query
        .build_query_scalar::<i64>()
        .fetch_one(db)
        .await
        .map_err(AppError::internal)?;
    Ok(assigned.max(0) as usize)
}

fn clone_template_payload(
    mut payload: ManagedSecretPayload,
    config_id: &str,
    now: &str,
) -> ManagedSecretPayload {
    payload.config.id = config_id.to_string();
    payload.config.created_at = now.to_string();
    payload.config.updated_at = now.to_string();
    payload
}

fn runtime_to_input(payload: &ManagedSecretPayload) -> ManagedProviderConfigInput {
    ManagedProviderConfigInput {
        name: payload.config.name.clone(),
        template: payload.template.clone(),
        endpoint_mode: payload.config.endpoint_mode,
        base_url: payload.config.base_url.clone(),
        available_models: payload.config.available_models.clone(),
        current_model: payload.config.model.clone(),
        responses_model: payload.config.responses_model.clone(),
        output_format: payload.config.output_format.clone(),
        output_compression: payload.config.output_compression,
        background: payload.config.background.clone(),
        moderation: payload.config.moderation.clone(),
        prompt_guard_enabled: payload.config.prompt_guard_enabled,
    }
}

fn template_admin_view(
    row: ManagedTemplateRow,
    payload: ManagedSecretPayload,
    assigned_count: i64,
    outdated_count: i64,
) -> ManagedProviderTemplateAdminView {
    ManagedProviderTemplateAdminView {
        id: row.id,
        revision: row.revision,
        enabled: row.enabled,
        assigned_count: assigned_count.max(0) as usize,
        outdated_count: outdated_count.max(0) as usize,
        config: runtime_to_input(&payload),
        api_key_hint: row.api_key_hint,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
    }
}

async fn load_template_row(
    state: &AppState,
    template_id: &str,
) -> Result<ManagedTemplateRow, AppError> {
    let row = sqlx::query("SELECT id, encrypted_payload, api_key_hint, revision, enabled, updated_at, updated_by FROM managed_provider_templates WHERE id = ?")
        .bind(template_id).fetch_optional(&state.db).await.map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("服务商模板不存在。"))?;
    Ok(managed_template_row(row))
}

fn managed_template_row(row: sqlx::sqlite::SqliteRow) -> ManagedTemplateRow {
    ManagedTemplateRow {
        id: row.get("id"),
        encrypted_payload: row.get("encrypted_payload"),
        api_key_hint: row.get("api_key_hint"),
        revision: row.get::<i64, _>("revision").max(1) as u64,
        enabled: row.get::<i64, _>("enabled") != 0,
        updated_at: row.get("updated_at"),
        updated_by: row.get("updated_by"),
    }
}

fn validated_target_ids(ids: Vec<String>) -> Result<BTreeSet<String>, AppError> {
    if ids.is_empty() || ids.len() > MAX_BULK_CONFIGS || ids.iter().any(|id| id.trim().is_empty()) {
        return Err(AppError::bad_request("请选择 1–100 个托管账号。"));
    }
    let original_len = ids.len();
    let unique = ids.into_iter().collect::<BTreeSet<_>>();
    if unique.len() != original_len {
        return Err(AppError::bad_request("托管账号 ID 不能重复。"));
    }
    if unique.len() > MAX_BULK_CONFIGS {
        return Err(AppError::bad_request("一次最多选择 100 个托管账号。"));
    }
    Ok(unique)
}

async fn load_managed_user_names(
    db: &SqlitePool,
    user_ids: &BTreeSet<String>,
) -> Result<Vec<(String, String)>, AppError> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, username FROM users WHERE account_kind = 'managed' AND id IN (",
    );
    let mut separated = query.separated(", ");
    for id in user_ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") ORDER BY username COLLATE NOCASE");
    let users = query
        .build()
        .fetch_all(db)
        .await
        .map_err(AppError::internal)?
        .into_iter()
        .map(|row| (row.get("id"), row.get("username")))
        .collect::<Vec<_>>();
    if users.len() != user_ids.len() {
        return Err(AppError::bad_request(
            "所选账号中包含非托管账号或不存在的账号。",
        ));
    }
    Ok(users)
}

async fn record_managed_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    admin: &mew_image_shared::UserSummary,
    action: &str,
    target_type: &str,
    target_id: &str,
    target_name: &str,
    summary: &str,
) -> Result<(), AppError> {
    let operation_id = new_id();
    crate::admin_console::record(
        transaction,
        crate::admin_console::AuditRecord {
            operation_id: &operation_id,
            actor_user_id: &admin.id,
            actor_username: &admin.username,
            action,
            target_type,
            target_id,
            target_name,
            summary,
        },
    )
    .await
}

fn prepare_payload(
    state: &AppState,
    user_id: &str,
    config_id: &str,
    request: ManagedProviderConfigWriteRequest,
    existing_api_key: Option<String>,
    now: &str,
) -> Result<(ManagedSecretPayload, String), AppError> {
    let api_key = request
        .api_key
        .filter(|value| !value.trim().is_empty())
        .or(existing_api_key)
        .ok_or_else(|| AppError::bad_request("新建托管服务商配置时必须填写 API Key。"))?;
    if api_key.trim().is_empty() {
        return Err(AppError::bad_request("API Key 不能为空。"));
    }
    let input = request.config;
    let mut payload = ManagedSecretPayload {
        version: MANAGED_PAYLOAD_VERSION,
        config: input_to_runtime_config(config_id, &input, api_key, now),
        template: input.template,
    };
    validate_runtime_payload(state, &mut payload, now)?;
    let hint = payload
        .config
        .api_key_plaintext
        .as_deref()
        .map(api_key_hint)
        .unwrap_or_default();
    let _ = user_id;
    Ok((payload, hint))
}

fn input_to_runtime_config(
    config_id: &str,
    input: &ManagedProviderConfigInput,
    api_key: String,
    now: &str,
) -> EncryptedApiConfig {
    EncryptedApiConfig {
        id: config_id.to_string(),
        name: input.name.trim().to_string(),
        provider_template_id: input.template.id.clone(),
        provider_kind: input.template.kind,
        endpoint_mode: input.endpoint_mode,
        base_url: input.base_url.trim().to_string(),
        model: input.current_model.trim().to_string(),
        available_models: input.available_models.clone(),
        responses_model: input.responses_model.clone(),
        access_mode: ProviderAccessMode::Proxy,
        known_requires_proxy: true,
        server_managed: false,
        output_format: input.output_format.clone(),
        output_compression: input.output_compression,
        background: input.background.clone(),
        moderation: input.moderation.clone(),
        api_key_plaintext: Some(api_key),
        api_key_encrypted: None,
        api_key_hint: None,
        prompt_guard_enabled: input.prompt_guard_enabled,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    }
}

fn validate_runtime_payload(
    state: &AppState,
    payload: &mut ManagedSecretPayload,
    now: &str,
) -> Result<(), AppError> {
    if payload.config.name.trim().is_empty() || payload.config.name.chars().count() > 80 {
        return Err(AppError::bad_request("配置名称必须为 1–80 个字符。"));
    }
    if payload.template.kind != payload.config.provider_kind {
        return Err(AppError::bad_request("服务商模板与配置协议不一致。"));
    }
    payload.config.access_mode = ProviderAccessMode::Proxy;
    payload.config.updated_at = now.to_string();
    normalize_api_config(&mut payload.config);
    if payload.config.available_models.is_empty() || payload.config.available_models.len() > 64 {
        return Err(AppError::bad_request("每条配置必须包含 1–64 个模型。"));
    }
    if payload
        .config
        .available_models
        .iter()
        .any(|model| model.chars().count() > 160)
    {
        return Err(AppError::bad_request("模型 ID 不能超过 160 个字符。"));
    }
    validate_template(
        state,
        &payload.template,
        state.config.enforce_provider_host_whitelist,
    )?;
    let _ = resolve_provider_base_url(
        state,
        payload.config.provider_kind,
        &payload.config.base_url,
    )?;
    Ok(())
}

fn summary(row: &ManagedConfigRow, payload: &ManagedSecretPayload) -> ManagedProviderSummary {
    ManagedProviderSummary {
        id: row.id.clone(),
        name: payload.config.name.clone(),
        provider_kind: payload.config.provider_kind,
        endpoint_mode: payload.config.endpoint_mode,
        available_models: payload.config.available_models.clone(),
        current_model: payload.config.model.clone(),
        responses_model: payload.config.responses_model.clone(),
        output_format: payload.config.output_format.clone(),
        output_compression: payload.config.output_compression,
        background: payload.config.background.clone(),
        moderation: payload.config.moderation.clone(),
        enabled: row.enabled,
        updated_at: row.updated_at.clone(),
    }
}

fn admin_view(row: ManagedConfigRow, payload: ManagedSecretPayload) -> ManagedProviderAdminView {
    ManagedProviderAdminView {
        user_id: row.user_id.clone(),
        username: row.username.clone(),
        summary: summary(&row, &payload),
        base_url: payload.config.base_url.clone(),
        api_key_hint: row.api_key_hint,
        responses_model: payload.config.responses_model.clone(),
        prompt_guard_enabled: payload.config.prompt_guard_enabled,
        template: payload.template,
        updated_by: row.updated_by,
        source_template_id: row.source_template_id,
        source_template_revision: row.source_template_revision,
    }
}

fn require_managed_secret(state: &AppState) -> Result<(), AppError> {
    state.config.managed_provider_key.as_ref().ok_or_else(|| {
        AppError::internal_message(
            "服务器未配置 MEW_MANAGED_PROVIDER_SECRET，托管账号功能尚未启用。",
        )
    })?;
    Ok(())
}

fn aad(user_id: &str, config_id: &str) -> String {
    format!("mew-managed-provider:v{MANAGED_PAYLOAD_VERSION}:{user_id}:{config_id}")
}

fn encrypt_payload(
    state: &AppState,
    user_id: &str,
    config_id: &str,
    payload: &ManagedSecretPayload,
) -> Result<String, AppError> {
    let key = state
        .config
        .managed_provider_key
        .as_ref()
        .ok_or_else(|| AppError::internal_message("服务器未配置托管服务商加密密钥。"))?;
    let cipher = Aes256GcmSiv::new_from_slice(key.as_bytes()).map_err(AppError::internal)?;
    let nonce_bytes = rand::random::<[u8; 12]>();
    let plaintext = serde_json::to_vec(payload).map_err(AppError::internal)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: &plaintext,
                aad: aad(user_id, config_id).as_bytes(),
            },
        )
        .map_err(|_| AppError::internal_message("托管服务商配置加密失败。"))?;
    let mut encoded = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(encoded))
}

fn decrypt_payload(
    state: &AppState,
    user_id: &str,
    config_id: &str,
    encoded: &str,
) -> Result<ManagedSecretPayload, AppError> {
    let key = state
        .config
        .managed_provider_key
        .as_ref()
        .ok_or_else(|| AppError::internal_message("服务器未配置托管服务商加密密钥。"))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| AppError::internal_message("托管服务商配置密文格式无效。"))?;
    if bytes.len() <= 12 {
        return Err(AppError::internal_message("托管服务商配置密文不完整。"));
    }
    let cipher = Aes256GcmSiv::new_from_slice(key.as_bytes()).map_err(AppError::internal)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: aad(user_id, config_id).as_bytes(),
            },
        )
        .map_err(|_| AppError::internal_message("托管服务商配置解密失败。"))?;
    let payload: ManagedSecretPayload =
        serde_json::from_slice(&plaintext).map_err(AppError::internal)?;
    if payload.version != MANAGED_PAYLOAD_VERSION {
        return Err(AppError::internal_message(
            "托管服务商配置使用了不支持的密文版本。",
        ));
    }
    Ok(payload)
}

pub(crate) fn generate_temporary_password() -> String {
    format!("Aa1!{}", Alphanumeric.sample_string(&mut rand::rng(), 20))
}

fn api_key_hint(api_key: &str) -> String {
    let suffix = api_key.chars().rev().take(4).collect::<Vec<_>>();
    let suffix = suffix.into_iter().rev().collect::<String>();
    format!("••••{suffix}")
}

fn provider_kind_key(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiImage => "openai_image",
        ProviderKind::NanoBanana => "nano_banana",
        ProviderKind::OpenAiCompatible => "openai_compatible",
        ProviderKind::CustomHttp => "custom_http",
    }
}

fn endpoint_mode_key(mode: mew_image_shared::ProviderEndpointMode) -> &'static str {
    match mode {
        mew_image_shared::ProviderEndpointMode::ImagesApi => "images_api",
        mew_image_shared::ProviderEndpointMode::ResponsesApi => "responses_api",
        mew_image_shared::ProviderEndpointMode::CustomJson => "custom_json",
    }
}

async fn ensure_managed_user(state: &AppState, user_id: &str) -> Result<(), AppError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE id = ? AND account_kind = 'managed'",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    if exists == 0 {
        return Err(AppError::bad_request("目标账号不是托管账号。"));
    }
    Ok(())
}

async fn load_owned_row(
    state: &AppState,
    user_id: &str,
    config_id: &str,
) -> Result<ManagedConfigRow, AppError> {
    let row = sqlx::query(
        "SELECT c.id, c.user_id, u.username, c.encrypted_payload, c.api_key_hint, c.enabled, c.updated_at, c.updated_by, c.source_template_id, c.source_template_revision FROM managed_provider_configs c JOIN users u ON u.id = c.user_id WHERE c.id = ? AND c.user_id = ?",
    )
    .bind(config_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("托管服务商配置不存在。"))?;
    Ok(row_from_sql(row))
}

async fn load_row_by_id(state: &AppState, config_id: &str) -> Result<ManagedConfigRow, AppError> {
    let row = sqlx::query(
        "SELECT c.id, c.user_id, u.username, c.encrypted_payload, c.api_key_hint, c.enabled, c.updated_at, c.updated_by, c.source_template_id, c.source_template_revision FROM managed_provider_configs c JOIN users u ON u.id = c.user_id WHERE c.id = ?",
    )
    .bind(config_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("托管服务商配置不存在。"))?;
    Ok(row_from_sql(row))
}

async fn load_user_rows(
    state: &AppState,
    user_id: &str,
) -> Result<Vec<ManagedConfigRow>, AppError> {
    let rows = sqlx::query(
        "SELECT c.id, c.user_id, u.username, c.encrypted_payload, c.api_key_hint, c.enabled, c.updated_at, c.updated_by, c.source_template_id, c.source_template_revision FROM managed_provider_configs c JOIN users u ON u.id = c.user_id WHERE c.user_id = ? ORDER BY c.created_at, c.id",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?;
    Ok(rows.into_iter().map(row_from_sql).collect())
}

async fn load_admin_rows(state: &AppState) -> Result<Vec<ManagedConfigRow>, AppError> {
    let rows = sqlx::query(
        "SELECT c.id, c.user_id, u.username, c.encrypted_payload, c.api_key_hint, c.enabled, c.updated_at, c.updated_by, c.source_template_id, c.source_template_revision FROM managed_provider_configs c JOIN users u ON u.id = c.user_id ORDER BY u.username COLLATE NOCASE, c.created_at, c.id",
    )
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?;
    Ok(rows.into_iter().map(row_from_sql).collect())
}

fn row_from_sql(row: sqlx::sqlite::SqliteRow) -> ManagedConfigRow {
    ManagedConfigRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        encrypted_payload: row.get("encrypted_payload"),
        api_key_hint: row.get("api_key_hint"),
        enabled: row.get::<i64, _>("enabled") != 0,
        updated_at: row.get("updated_at"),
        updated_by: row.get("updated_by"),
        source_template_id: row.get("source_template_id"),
        source_template_revision: row
            .get::<Option<i64>, _>("source_template_revision")
            .map(|value| value.max(1) as u64),
    }
}

async fn insert_config_row(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    config_id: &str,
    user_id: &str,
    persistence: ConfigPersistence<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO managed_provider_configs (id, user_id, name, provider_kind, endpoint_mode, encrypted_payload, api_key_hint, enabled, created_at, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(config_id)
    .bind(user_id)
    .bind(&persistence.payload.config.name)
    .bind(provider_kind_key(persistence.payload.config.provider_kind))
    .bind(endpoint_mode_key(persistence.payload.config.endpoint_mode))
    .bind(persistence.encrypted)
    .bind(persistence.key_hint)
    .bind(persistence.enabled)
    .bind(persistence.now)
    .bind(persistence.now)
    .bind(persistence.updated_by)
    .execute(&mut **transaction)
    .await
    .map_err(map_unique_error)?;
    Ok(())
}

async fn update_config_row(
    state: &AppState,
    config_id: &str,
    persistence: ConfigPersistence<'_>,
) -> Result<(), AppError> {
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    update_config_on_connection(&mut transaction, config_id, persistence).await?;
    transaction.commit().await.map_err(AppError::internal)
}

async fn update_config_on_connection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    config_id: &str,
    persistence: ConfigPersistence<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE managed_provider_configs SET name = ?, provider_kind = ?, endpoint_mode = ?, encrypted_payload = ?, api_key_hint = ?, enabled = ?, updated_at = ?, updated_by = ? WHERE id = ?",
    )
    .bind(&persistence.payload.config.name)
    .bind(provider_kind_key(persistence.payload.config.provider_kind))
    .bind(endpoint_mode_key(persistence.payload.config.endpoint_mode))
    .bind(persistence.encrypted)
    .bind(persistence.key_hint)
    .bind(persistence.enabled)
    .bind(persistence.now)
    .bind(persistence.updated_by)
    .bind(config_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_unique_error)?;
    Ok(())
}

fn map_unique_error(error: sqlx::Error) -> AppError {
    if error.to_string().contains("UNIQUE") {
        AppError::bad_request("用户名或同账号下的配置名称已存在。")
    } else {
        AppError::internal(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_password_meets_current_strength_rules() {
        let password = generate_temporary_password();
        assert!(password.len() >= 10);
        assert!(password.chars().any(|value| value.is_ascii_uppercase()));
        assert!(password.chars().any(|value| value.is_ascii_lowercase()));
        assert!(password.chars().any(|value| value.is_ascii_digit()));
        assert!(password.chars().any(|value| !value.is_ascii_alphanumeric()));
    }

    #[test]
    fn key_hint_only_keeps_a_short_suffix() {
        assert_eq!(api_key_hint("sk-secret-123456"), "••••3456");
    }
}
