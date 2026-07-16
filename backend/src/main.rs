mod state;

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    path::{Component, Path as FsPath, PathBuf},
    str::FromStr,
    sync::Arc,
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use aws_config::BehaviorVersion;
use aws_sdk_s3::{Client as S3Client, config::Region, primitives::ByteStream};
use axum::{
    Json, Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration, Utc};
use mew_image_shared::{
    AdminBootstrapRequest, AdminSetupStatusResponse, AdminUserActionRequest, AdminUserSummary,
    AdminUsersResponse, AuthRequest, AuthResponse, ChangePasswordRequest, CloudDataClearRequest,
    CloudDataClearScope, CloudDataStatsResponse, GenerateViaProxyRequest, GeneratedImageResult,
    GenerationResult, ImageAssetRef, MeResponse, MergePreviewResponse, ParameterSnapshot,
    ProviderEndpointMode, ProviderKind, ProviderTemplate, ProviderTemplateImportRequest,
    RegisterRequest, SyncEntityKind, SyncEnvelope, SyncPullResponse, SyncPushRequest,
    UploadCompleteRequest, UploadCompleteResponse, UploadInitRequest, UploadInitResponse,
    UserSummary, UsernameAvailabilityResponse, aspect_ratio_from_dimensions,
    extract_gemini_generation_result, extract_openai_compatible_result,
    extract_openai_responses_result, merge_envelopes, nano_banana_image_size_from_dimensions,
    new_id, now_rfc3339, parse_openai_responses_event_stream,
};
use rand::distr::{Alphanumeric, SampleString};
use reqwest::Url;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use state::{AppConfig, AppState, AssetStoreKind};
use tokio::net::TcpListener;
use tower_http::{
    cors::{AllowHeaders, AllowOrigin, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tower_sessions::{ExpiredDeletion, Session, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;
use tower_sessions_sqlx_store::sqlx::sqlite::SqlitePool as SessionSqlitePool;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mew_image_backend=debug,tower_http=info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    ensure_sqlite_parent_dir(&config.database_url)?;
    ensure_asset_store_ready(&config)?;
    let db_options = SqliteConnectOptions::from_str(&config.database_url)?.create_if_missing(true);
    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(db_options)
        .await?;
    init_db(&db).await?;

    let s3 = build_s3_client(&config).await?;
    let builtins = vec![
        ProviderTemplate::builtin_openai(),
        ProviderTemplate::builtin_nano_banana(),
        ProviderTemplate::builtin_openai_compatible(),
    ];

    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        s3,
        http: reqwest::Client::builder().build()?,
        provider_builtins: builtins,
    });

    let session_pool = SessionSqlitePool::connect(&config.database_url).await?;
    let session_store = SqliteStore::new(session_pool)
        .with_table_name("mew_image_sessions")
        .map_err(anyhow::Error::msg)?;
    session_store.migrate().await?;
    tokio::spawn(
        session_store
            .clone()
            .continuously_delete_expired(std::time::Duration::from_secs(900)),
    );

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(config.session_secure)
        .with_same_site(tower_sessions::cookie::SameSite::Lax);

    let cors_layer = build_cors_layer(&config)?;

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/register", post(register))
        .route("/api/auth/check-username", get(check_username))
        .route("/api/auth/setup-status", get(admin_setup_status))
        .route("/api/auth/bootstrap-admin", post(bootstrap_admin))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/admin/users", get(admin_list_users))
        .route("/api/admin/users/approve", post(admin_approve_user))
        .route("/api/admin/users/disable", post(admin_disable_user))
        .route("/api/admin/users/restore", post(admin_restore_user))
        .route("/api/admin/users/delete", post(admin_delete_user))
        .route("/api/sync/push", post(sync_push))
        .route("/api/sync/pull", get(sync_pull))
        .route("/api/sync/merge-preview", post(sync_merge_preview))
        .route("/api/data/stats", get(cloud_data_stats))
        .route("/api/data/clear", post(clear_cloud_data))
        .route(
            "/api/providers/templates",
            get(list_provider_templates).post(import_provider_template),
        )
        .route("/api/providers/generate", post(generate_via_proxy))
        .route("/api/assets/upload-init", post(upload_init))
        .route("/api/assets/upload/{token}", put(upload_bytes))
        .route("/api/assets/complete", post(upload_complete))
        .route("/api/assets/{asset_id}", get(get_asset))
        .route("/api/images/fetch", post(fetch_image_via_proxy))
        .fallback_service(
            ServeDir::new(&config.frontend_dist).append_index_html_on_directories(true),
        )
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer)
        .layer(session_layer)
        .with_state(state);

    let addr: SocketAddr = config.listen_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!("backend listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn ensure_sqlite_parent_dir(database_url: &str) -> anyhow::Result<()> {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    let path = path.split('?').next().unwrap_or(path);
    let path = path.strip_prefix("./").unwrap_or(path);
    let path = std::path::Path::new(path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn ensure_asset_store_ready(config: &AppConfig) -> anyhow::Result<()> {
    if config.asset_store == AssetStoreKind::Local {
        std::fs::create_dir_all(&config.local_asset_dir)?;
    }
    Ok(())
}

async fn build_s3_client(config: &AppConfig) -> anyhow::Result<Option<S3Client>> {
    if config.asset_store != AssetStoreKind::S3 || config.s3_bucket.is_empty() {
        return Ok(None);
    }

    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(config.s3_region.clone()));
    if let Some(endpoint) = config.s3_endpoint.clone() {
        loader = loader.endpoint_url(endpoint);
    }
    if let (Some(access_key), Some(secret_key)) =
        (config.s3_access_key.clone(), config.s3_secret_key.clone())
    {
        let creds =
            aws_sdk_s3::config::Credentials::new(access_key, secret_key, None, None, "mew-image");
        loader = loader.credentials_provider(creds);
    }
    let shared_config = loader.load().await;
    Ok(Some(S3Client::new(&shared_config)))
}

async fn init_db(db: &SqlitePool) -> anyhow::Result<()> {
    for statement in [
        r#"CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'user',
            status TEXT NOT NULL DEFAULT 'approved',
            password_updated_at TEXT,
            approved_at TEXT,
            approved_by TEXT,
            last_login_at TEXT,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS sync_snapshots (
            user_id TEXT PRIMARY KEY,
            payload TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS provider_templates (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS assets (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            object_key TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            byte_len INTEGER NOT NULL,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS upload_tokens (
            token TEXT PRIMARY KEY,
            asset_id TEXT NOT NULL,
            user_id TEXT,
            object_key TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            byte_len INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            expires_at TEXT NOT NULL
        )"#,
    ] {
        sqlx::query(statement).execute(db).await?;
    }
    migrate_users_table(db).await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS users_single_admin ON users(role) WHERE role = 'admin'",
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn migrate_users_table(db: &SqlitePool) -> anyhow::Result<()> {
    let rows = sqlx::query("PRAGMA table_info(users)")
        .fetch_all(db)
        .await?;
    let columns = rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<BTreeSet<_>>();
    for (name, definition) in [
        ("role", "TEXT NOT NULL DEFAULT 'user'"),
        ("status", "TEXT NOT NULL DEFAULT 'approved'"),
        ("password_updated_at", "TEXT"),
        ("approved_at", "TEXT"),
        ("approved_by", "TEXT"),
        ("last_login_at", "TEXT"),
    ] {
        if !columns.contains(name) {
            sqlx::query(&format!("ALTER TABLE users ADD COLUMN {name} {definition}"))
                .execute(db)
                .await?;
        }
    }
    sqlx::query("UPDATE users SET role = 'user' WHERE role IS NULL OR role = ''")
        .execute(db)
        .await?;
    sqlx::query("UPDATE users SET status = 'approved' WHERE status IS NULL OR status = ''")
        .execute(db)
        .await?;
    Ok(())
}

fn build_cors_layer(config: &AppConfig) -> anyhow::Result<CorsLayer> {
    let origins = if config.allowed_web_origins.is_empty() {
        vec![
            "http://127.0.0.1:3000".to_string(),
            "http://localhost:3000".to_string(),
            "http://127.0.0.1:8080".to_string(),
            "http://localhost:8080".to_string(),
        ]
    } else {
        config.allowed_web_origins.clone()
    };

    let origin_headers = origins
        .into_iter()
        .map(|origin| HeaderValue::from_str(&origin))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CorsLayer::new()
        .allow_credentials(true)
        .allow_headers(AllowHeaders::mirror_request())
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
        .allow_origin(AllowOrigin::list(origin_headers)))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

#[derive(Debug, serde::Deserialize)]
struct UsernameAvailabilityQuery {
    username: String,
}

async fn check_username(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsernameAvailabilityQuery>,
) -> Result<Json<UsernameAvailabilityResponse>, AppError> {
    let username = query.username.trim().to_string();
    if username.len() < 3 {
        return Ok(Json(UsernameAvailabilityResponse {
            username,
            available: false,
        }));
    }
    Ok(Json(UsernameAvailabilityResponse {
        available: !username_exists(&state.db, &username).await?,
        username,
    }))
}

async fn admin_setup_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AdminSetupStatusResponse>, AppError> {
    let admin_exists = user_role_exists(&state.db, "admin").await?;
    Ok(Json(AdminSetupStatusResponse {
        admin_exists,
        setup_allowed: state.config.allow_first_admin_setup
            && !admin_exists
            && state.config.admin_setup_token.is_some(),
    }))
}

async fn register(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    validate_registration(&payload)?;

    let password_hash = hash_password(&payload.password)?;
    let has_admin = user_role_exists(&state.db, "admin").await?;
    let can_bootstrap_admin = state.config.allow_first_admin_setup
        && !has_admin
        && payload
            .admin_setup_token
            .as_deref()
            .zip(state.config.admin_setup_token.as_deref())
            .map(|(provided, expected)| provided == expected)
            .unwrap_or(false);
    let role = if can_bootstrap_admin { "admin" } else { "user" };
    let status = if can_bootstrap_admin {
        "approved"
    } else {
        "pending"
    };
    let now = now_rfc3339();
    let user = UserSummary {
        id: new_id(),
        username: payload.username.trim().to_string(),
        role: role.into(),
        status: status.into(),
        image_count: 0,
        created_at: now.clone(),
    };

    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, status, password_updated_at, approved_at, approved_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.username)
    .bind(&password_hash)
    .bind(role)
    .bind(status)
    .bind(&now)
    .bind(if can_bootstrap_admin { Some(now.clone()) } else { None })
    .bind(if can_bootstrap_admin { Some(user.id.clone()) } else { None })
    .bind(&now)
    .execute(&state.db)
    .await;

    if let Err(error) = result {
        if error.to_string().contains("UNIQUE") {
            return Err(AppError::bad_request("用户名已存在"));
        }
        return Err(AppError::internal(error));
    }

    session
        .insert("user_id", &user.id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(AuthResponse { user }))
}

async fn bootstrap_admin(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AdminBootstrapRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let user = require_user(&state, &session).await?;
    if user.role == "admin" {
        return Ok(Json(AuthResponse { user }));
    }
    if !state.config.allow_first_admin_setup || user_role_exists(&state.db, "admin").await? {
        return Err(AppError::unauthorized(
            "系统已存在管理员，不能再使用初始化口令升级账号。",
        ));
    }
    let expected = state
        .config
        .admin_setup_token
        .as_deref()
        .ok_or_else(|| AppError::unauthorized("服务器未配置管理员初始化口令。"))?;
    if payload.admin_setup_token.trim() != expected {
        return Err(AppError::unauthorized("管理员初始化口令不正确。"));
    }

    let now = now_rfc3339();
    let result = sqlx::query(
        "UPDATE users
         SET role = 'admin', status = 'approved', approved_at = ?, approved_by = ?
         WHERE id = ? AND NOT EXISTS (SELECT 1 FROM users WHERE role = 'admin')",
    )
    .bind(&now)
    .bind(&user.id)
    .bind(&user.id)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::unauthorized(
            "系统已存在管理员，不能再使用初始化口令升级账号。",
        ));
    }

    let upgraded = UserSummary {
        role: "admin".into(),
        status: "approved".into(),
        image_count: user_image_count(&state.db, &user.id).await?,
        ..user
    };
    Ok(Json(AuthResponse { user: upgraded }))
}

async fn login(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    validate_login_credentials(&payload)?;

    let row =
        sqlx::query("SELECT id, username, password_hash, role, status, created_at FROM users WHERE username = ?")
            .bind(payload.username.trim())
            .fetch_optional(&state.db)
            .await
            .map_err(AppError::internal)?;

    let Some(row) = row else {
        return Err(AppError::unauthorized("用户名或密码错误"));
    };

    let password_hash = row.get::<String, _>("password_hash");
    verify_password(&payload.password, &password_hash)?;
    if row.get::<String, _>("status") == "disabled" {
        return Err(AppError::unauthorized("账号已被禁用，请联系管理员。"));
    }

    let user = UserSummary {
        id: row.get("id"),
        username: row.get("username"),
        role: row.get("role"),
        status: row.get("status"),
        image_count: user_image_count(&state.db, row.get::<String, _>("id").as_str()).await?,
        created_at: row.get("created_at"),
    };
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    session
        .insert("user_id", &user.id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(AuthResponse { user }))
}

async fn logout(session: Session) -> Result<StatusCode, AppError> {
    session.delete().await.map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<MeResponse>, AppError> {
    let user = current_user(&state, &session).await?;
    Ok(Json(MeResponse { user }))
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    let user = require_user(&state, &session).await?;
    validate_strong_password(&payload.new_password, &payload.new_password_confirm)?;

    let row = sqlx::query("SELECT password_hash FROM users WHERE id = ?")
        .bind(&user.id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::unauthorized("登录状态已失效，请重新登录。"))?;

    verify_password(
        &payload.old_password,
        &row.get::<String, _>("password_hash"),
    )?;
    let password_hash = hash_password(&payload.new_password)?;
    sqlx::query("UPDATE users SET password_hash = ?, password_updated_at = ? WHERE id = ?")
        .bind(password_hash)
        .bind(now_rfc3339())
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_list_users(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<AdminUsersResponse>, AppError> {
    require_admin(&state, &session).await?;
    let rows = sqlx::query(
        "SELECT id, username, role, status, created_at, approved_at, approved_by, last_login_at
         FROM users
         ORDER BY CASE status WHEN 'pending' THEN 0 WHEN 'approved' THEN 1 ELSE 2 END, created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?;

    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.get::<String, _>("id");
        users.push(AdminUserSummary {
            image_count: user_image_count(&state.db, &id).await?,
            id,
            username: row.get("username"),
            role: row.get("role"),
            status: row.get("status"),
            created_at: row.get("created_at"),
            approved_at: row.get("approved_at"),
            approved_by: row.get("approved_by"),
            last_login_at: row.get("last_login_at"),
        });
    }
    Ok(Json(AdminUsersResponse { users }))
}

async fn admin_approve_user(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AdminUserActionRequest>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &session).await?;
    update_user_status(
        &state,
        &payload.user_id,
        "approved",
        Some(admin.id.as_str()),
    )
    .await
}

async fn admin_disable_user(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AdminUserActionRequest>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &session).await?;
    if payload.user_id == admin.id {
        return Err(AppError::bad_request("不能禁用当前登录的管理员账号。"));
    }
    update_user_status(&state, &payload.user_id, "disabled", None).await
}

async fn admin_restore_user(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AdminUserActionRequest>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &session).await?;
    update_user_status(
        &state,
        &payload.user_id,
        "approved",
        Some(admin.id.as_str()),
    )
    .await
}

async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AdminUserActionRequest>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &session).await?;
    if payload.user_id == admin.id {
        return Err(AppError::bad_request("不能删除当前登录的管理员账号。"));
    }

    let role = sqlx::query_scalar::<_, String>("SELECT role FROM users WHERE id = ?")
        .bind(&payload.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("用户不存在"))?;
    if role == "admin" {
        return Err(AppError::bad_request(
            "管理员账号不能通过用户管理页面删除。",
        ));
    }

    let mut object_keys = BTreeSet::new();
    for table in ["assets", "upload_tokens"] {
        let query = format!("SELECT object_key FROM {table} WHERE user_id = ?");
        let rows = sqlx::query(&query)
            .bind(&payload.user_id)
            .fetch_all(&state.db)
            .await
            .map_err(AppError::internal)?;
        object_keys.extend(
            rows.into_iter()
                .map(|row| row.get::<String, _>("object_key")),
        );
    }
    for object_key in object_keys {
        delete_object(&state, &object_key).await?;
    }
    delete_user_object_namespace(&state, &payload.user_id).await?;

    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    for table in [
        "upload_tokens",
        "assets",
        "sync_snapshots",
        "provider_templates",
    ] {
        let query = format!("DELETE FROM {table} WHERE user_id = ?");
        sqlx::query(&query)
            .bind(&payload.user_id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&payload.user_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_user_status(
    state: &AppState,
    user_id: &str,
    status: &str,
    approved_by: Option<&str>,
) -> Result<StatusCode, AppError> {
    let approved_at = if status == "approved" {
        Some(now_rfc3339())
    } else {
        None
    };
    let result = sqlx::query(
        "UPDATE users
         SET status = ?, approved_at = COALESCE(?, approved_at), approved_by = COALESCE(?, approved_by)
         WHERE id = ?",
    )
    .bind(status)
    .bind(approved_at)
    .bind(approved_by)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;

    if result.rows_affected() == 0 {
        return Err(AppError::not_found("用户不存在"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_push(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<SyncPushRequest>,
) -> Result<Json<SyncPullResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let existing = load_sync_envelope(&state.db, &user.id).await?;
    let normalized = normalize_envelope_assets(&state, &user.id, payload.envelope).await?;
    let merged = merge_envelopes(&existing, &normalized);
    let updated_at = now_rfc3339();

    sqlx::query(
        "INSERT INTO sync_snapshots (user_id, payload, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
    )
    .bind(&user.id)
    .bind(serde_json::to_string(&merged).map_err(AppError::internal)?)
    .bind(&updated_at)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;

    cleanup_tombstoned_assets(&state, &user.id, &merged).await?;

    Ok(Json(SyncPullResponse {
        envelope: merged,
        checkpoint: mew_image_shared::SyncCheckpoint {
            last_push_at: Some(updated_at.clone()),
            last_pull_at: Some(updated_at.clone()),
            last_merged_at: Some(updated_at.clone()),
            server_cursor: Some(updated_at),
        },
    }))
}

async fn cleanup_tombstoned_assets(
    state: &AppState,
    user_id: &str,
    envelope: &SyncEnvelope,
) -> Result<(), AppError> {
    let active_asset_ids = envelope
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<BTreeSet<_>>();
    let deleted_asset_ids = envelope
        .tombstones
        .iter()
        .filter(|item| item.entity_kind == SyncEntityKind::Asset)
        .map(|item| item.entity_id.as_str())
        .filter(|asset_id| !active_asset_ids.contains(asset_id))
        .collect::<BTreeSet<_>>();

    for asset_id in deleted_asset_ids {
        let row = sqlx::query("SELECT object_key FROM assets WHERE id = ? AND user_id = ?")
            .bind(asset_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(AppError::internal)?;
        let Some(row) = row else {
            continue;
        };
        let object_key = row.get::<String, _>("object_key");
        let other_references = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM assets WHERE user_id = ? AND object_key = ? AND id != ?",
        )
        .bind(user_id)
        .bind(&object_key)
        .bind(asset_id)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?;
        if other_references == 0 {
            // 对象删除是幂等的，先删文件可确保失败后仍能通过资产行重试。
            delete_object(state, &object_key).await?;
        }
        let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
        sqlx::query("DELETE FROM upload_tokens WHERE asset_id = ? AND user_id = ?")
            .bind(asset_id)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM assets WHERE id = ? AND user_id = ?")
            .bind(asset_id)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        transaction.commit().await.map_err(AppError::internal)?;
    }
    Ok(())
}

async fn sync_pull(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<SyncPullResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let envelope = load_sync_envelope(&state.db, &user.id).await?;
    let now = now_rfc3339();
    Ok(Json(SyncPullResponse {
        envelope,
        checkpoint: mew_image_shared::SyncCheckpoint {
            last_pull_at: Some(now.clone()),
            server_cursor: Some(now),
            ..Default::default()
        },
    }))
}

async fn cloud_data_stats(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<CloudDataStatsResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    Ok(Json(load_cloud_data_stats(&state.db, &user.id).await?))
}

async fn clear_cloud_data(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<CloudDataClearRequest>,
) -> Result<Json<CloudDataStatsResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    match payload.scope {
        CloudDataClearScope::SyncData => clear_user_sync_data(&state, &user.id).await?,
        CloudDataClearScope::ProviderTemplates => {
            delete_user_rows(&state.db, &user.id, &["provider_templates"]).await?
        }
        CloudDataClearScope::All => {
            clear_user_sync_data(&state, &user.id).await?;
            delete_user_rows(&state.db, &user.id, &["provider_templates"]).await?;
        }
    }
    Ok(Json(load_cloud_data_stats(&state.db, &user.id).await?))
}

async fn clear_user_sync_data(state: &AppState, user_id: &str) -> Result<(), AppError> {
    // 先清理对象命名空间，再删除索引，避免数据库成功后遗留无法定位的云端文件。
    delete_user_object_namespace(state, user_id).await?;
    delete_user_rows(
        &state.db,
        user_id,
        &["upload_tokens", "assets", "sync_snapshots"],
    )
    .await
}

async fn delete_user_rows(db: &SqlitePool, user_id: &str, tables: &[&str]) -> Result<(), AppError> {
    let mut transaction = db.begin().await.map_err(AppError::internal)?;
    for table in tables {
        let query = format!("DELETE FROM {table} WHERE user_id = ?");
        sqlx::query(&query)
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    transaction.commit().await.map_err(AppError::internal)
}

async fn load_cloud_data_stats(
    db: &SqlitePool,
    user_id: &str,
) -> Result<CloudDataStatsResponse, AppError> {
    let (image_count, image_bytes) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM assets WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    .map_err(AppError::internal)?;
    let provider_template_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM provider_templates WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(AppError::internal)?;
    let pending_upload_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM upload_tokens WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(AppError::internal)?;
    let has_sync_snapshot =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sync_snapshots WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(AppError::internal)?
            > 0;

    Ok(CloudDataStatsResponse {
        image_count: image_count.max(0) as usize,
        image_bytes: image_bytes.max(0) as u64,
        provider_template_count: provider_template_count.max(0) as usize,
        pending_upload_count: pending_upload_count.max(0) as usize,
        has_sync_snapshot,
    })
}

async fn sync_merge_preview(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<SyncPushRequest>,
) -> Result<Json<MergePreviewResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let existing = load_sync_envelope(&state.db, &user.id).await?;
    let merged = merge_envelopes(&existing, &payload.envelope);
    Ok(Json(MergePreviewResponse {
        merged_updated_at: merged.updated_at.clone(),
        config_count: merged.configs.len(),
        task_count: merged.tasks.len(),
        thread_count: merged.threads.len(),
        asset_count: merged.assets.len(),
    }))
}

async fn list_provider_templates(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<Vec<ProviderTemplate>>, AppError> {
    let user = current_user(&state, &session).await?;
    let mut templates = state.provider_builtins.clone();
    if let Some(user) = user {
        let rows = sqlx::query(
            "SELECT payload FROM provider_templates WHERE user_id = ? ORDER BY updated_at DESC",
        )
        .bind(user.id)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::internal)?;
        for row in rows {
            let payload = row.get::<String, _>("payload");
            if let Ok(template) = serde_json::from_str::<ProviderTemplate>(&payload) {
                templates.push(template);
            }
        }
    }
    Ok(Json(templates))
}

async fn import_provider_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<ProviderTemplateImportRequest>,
) -> Result<Json<ProviderTemplate>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    validate_template(&state, &payload.template, true)?;

    let serialized = serde_json::to_string(&payload.template).map_err(AppError::internal)?;
    sqlx::query(
        "INSERT INTO provider_templates (id, user_id, payload, created_at, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
    )
    .bind(&payload.template.id)
    .bind(&user.id)
    .bind(serialized)
    .bind(&payload.template.created_at)
    .bind(&payload.template.updated_at)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;

    Ok(Json(payload.template))
}

async fn upload_init(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<UploadInitRequest>,
) -> Result<Json<UploadInitResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state.db).await?;
    let asset_id = new_id();
    let token = random_token();
    let object_key = format!(
        "users/{}/assets/{}-{}",
        user.id,
        payload.sha256,
        sanitize_file_name(&payload.file_name)
    );
    let expires_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();

    sqlx::query(
        "INSERT INTO upload_tokens (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(&asset_id)
    .bind(&user.id)
    .bind(&object_key)
    .bind(&payload.mime_type)
    .bind(payload.byte_len as i64)
    .bind(&payload.sha256)
    .bind(&expires_at)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;

    Ok(Json(UploadInitResponse {
        upload_token: token.clone(),
        upload_url: format!("/api/assets/upload/{token}"),
        asset_id,
        object_key,
    }))
}

async fn upload_bytes(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(token): Path<String>,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let user = require_approved_user(&state, &session).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state.db).await?;
    let row = sqlx::query(
        "SELECT asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at FROM upload_tokens WHERE token = ?",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?;
    let Some(row) = row else {
        return Err(AppError::not_found("上传凭证不存在"));
    };
    let owner_id = row.get::<Option<String>, _>("user_id");
    if owner_id.as_deref() != Some(user.id.as_str()) {
        return Err(AppError::unauthorized("上传凭证不属于当前登录用户"));
    }
    ensure_upload_token_not_expired(&row)?;

    let expected_len = row.get::<i64, _>("byte_len") as usize;
    if expected_len != body.len() {
        return Err(AppError::bad_request("上传大小与预期不一致"));
    }

    let hash = hex_sha256(&body);
    let expected_hash = row.get::<String, _>("sha256");
    if hash != expected_hash {
        return Err(AppError::bad_request("文件哈希校验失败"));
    }

    put_object(
        &state,
        row.get::<String, _>("object_key").as_str(),
        row.get::<String, _>("mime_type").as_str(),
        body.to_vec(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

async fn upload_complete(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<UploadCompleteRequest>,
) -> Result<Json<UploadCompleteResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state.db).await?;
    let row = sqlx::query(
        "SELECT asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at FROM upload_tokens WHERE token = ?",
    )
    .bind(&payload.upload_token)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?;
    let Some(row) = row else {
        return Err(AppError::not_found("上传凭证不存在"));
    };
    let owner_id = row.get::<Option<String>, _>("user_id");
    if owner_id.as_deref() != Some(user.id.as_str()) {
        return Err(AppError::unauthorized("上传凭证不属于当前登录用户"));
    }
    ensure_upload_token_not_expired(&row)?;

    let created_at = now_rfc3339();
    sqlx::query(
        "INSERT INTO assets (id, user_id, object_key, mime_type, sha256, byte_len, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET object_key = excluded.object_key, mime_type = excluded.mime_type, sha256 = excluded.sha256, byte_len = excluded.byte_len",
    )
    .bind(row.get::<String, _>("asset_id"))
    .bind(row.get::<Option<String>, _>("user_id"))
    .bind(row.get::<String, _>("object_key"))
    .bind(row.get::<String, _>("mime_type"))
    .bind(row.get::<String, _>("sha256"))
    .bind(row.get::<i64, _>("byte_len"))
    .bind(&created_at)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;
    sqlx::query("DELETE FROM upload_tokens WHERE token = ?")
        .bind(&payload.upload_token)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;

    let asset = ImageAssetRef {
        id: row.get("asset_id"),
        sha256: row.get("sha256"),
        mime_type: row.get("mime_type"),
        byte_len: row.get::<i64, _>("byte_len") as u64,
        width: None,
        height: None,
        created_at: created_at.clone(),
        updated_at: created_at,
        data_url: None,
        remote_object_key: Some(row.get("object_key")),
        remote_url: Some(format!("/api/assets/{}", row.get::<String, _>("asset_id"))),
        source_task_id: None,
        metadata: Default::default(),
    };
    Ok(Json(UploadCompleteResponse { asset }))
}

async fn get_asset(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(asset_id): Path<String>,
) -> Result<Response, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let row = sqlx::query("SELECT object_key, mime_type, user_id FROM assets WHERE id = ?")
        .bind(asset_id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?;
    let Some(row) = row else {
        return Err(AppError::not_found("资源不存在"));
    };
    let owner_id = row.get::<Option<String>, _>("user_id");
    if owner_id.as_deref() != Some(user.id.as_str()) {
        return Err(AppError::unauthorized("当前登录用户无权访问该资源"));
    }

    let bytes = get_object_bytes(&state, &row.get::<String, _>("object_key")).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&row.get::<String, _>("mime_type")).map_err(AppError::internal)?,
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn fetch_image_via_proxy(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<FetchImageRequest>,
) -> Result<Json<FetchImageResponse>, AppError> {
    if !state.config.enable_guest_proxy {
        return Err(AppError::unauthorized("当前部署已关闭游客代理。"));
    }
    let (mime_type, bytes) = fetch_remote_image_bytes(&state, &payload.url).await?;
    Ok(Json(FetchImageResponse {
        mime_type,
        body_base64: BASE64.encode(bytes),
    }))
}

async fn generate_via_proxy(
    State(state): State<Arc<AppState>>,
    session: Session,
    multipart: Multipart,
) -> Result<Json<GenerationResult>, AppError> {
    let payload = parse_generate_multipart(multipart).await?;
    let user = current_user(&state, &session).await?;
    validate_generate_request(&state, user.as_ref(), &payload)?;
    let started_at = Utc::now();
    let response_json = match payload.template.kind {
        ProviderKind::OpenAiImage => invoke_openai_image(&state, &payload).await?,
        ProviderKind::NanoBanana => invoke_nano_banana_google(&state, &payload).await?,
        ProviderKind::OpenAiCompatible => invoke_openai_compatible_image(&state, &payload).await?,
        ProviderKind::CustomHttp => invoke_custom_http(&state, &payload).await?,
    };

    let duration_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;
    let result = extract_generation_result(
        &payload.template,
        payload.config.output_format.as_deref(),
        &payload.request,
        response_json,
        duration_ms,
    );
    let result = hydrate_proxy_result_images(&state, result).await?;
    Ok(Json(result))
}

async fn parse_generate_multipart(
    mut multipart: Multipart,
) -> Result<GenerateViaProxyRequest, AppError> {
    let mut payload = None;
    let mut reference_assets_meta = None;
    let mut reference_assets_files = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(AppError::internal)? {
        let name = field.name().unwrap_or_default().to_string();
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        match name.as_str() {
            "payload" => {
                let text = field.text().await.map_err(AppError::internal)?;
                payload = Some(
                    serde_json::from_str::<GenerateViaProxyRequest>(&text).map_err(|error| {
                        AppError::bad_request(format!("生成请求解析失败：{error}"))
                    })?,
                );
            }
            "reference_assets_meta" => {
                reference_assets_meta = Some(field.text().await.map_err(AppError::internal)?);
            }
            "reference_asset_files" => {
                let bytes = field.bytes().await.map_err(AppError::internal)?;
                reference_assets_files.push((content_type, bytes.to_vec()));
            }
            _ => {}
        }
    }

    let mut payload = payload.ok_or_else(|| AppError::bad_request("缺少生成请求主体"))?;
    let reference_assets_meta: Vec<ReferenceAssetMeta> = reference_assets_meta
        .map(|text| {
            serde_json::from_str(&text)
                .map_err(|error| AppError::bad_request(format!("参考图元数据解析失败：{error}")))
        })
        .transpose()?
        .unwrap_or_default();
    if reference_assets_meta.len() != reference_assets_files.len() {
        return Err(AppError::bad_request("参考图文件数量与元数据数量不一致"));
    }

    let mut reference_assets = Vec::with_capacity(reference_assets_meta.len());
    for (asset, (mime_type, bytes)) in reference_assets_meta
        .into_iter()
        .zip(reference_assets_files.into_iter())
    {
        reference_assets.push(ImageAssetRef {
            id: asset.id,
            sha256: asset.sha256,
            mime_type: mime_type.clone(),
            byte_len: bytes.len() as u64,
            width: asset.width,
            height: asset.height,
            created_at: asset.created_at,
            updated_at: asset.updated_at,
            data_url: Some(format!("data:{mime_type};base64,{}", BASE64.encode(&bytes))),
            remote_object_key: None,
            remote_url: None,
            source_task_id: asset.source_task_id,
            metadata: asset.metadata,
        });
    }

    payload.request.reference_assets = reference_assets;
    Ok(payload)
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ReferenceAssetMeta {
    id: String,
    sha256: String,
    width: Option<u32>,
    height: Option<u32>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    source_task_id: Option<String>,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, String>,
}

async fn current_user(
    state: &AppState,
    session: &Session,
) -> Result<Option<UserSummary>, AppError> {
    let Some(user_id) = session
        .get::<String>("user_id")
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT id, username, role, status, created_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?;

    if let Some(row) = row {
        let id = row.get::<String, _>("id");
        let image_count = user_image_count(&state.db, &id).await?;
        Ok(Some(UserSummary {
            id,
            username: row.get("username"),
            role: row.get("role"),
            status: row.get("status"),
            image_count,
            created_at: row.get("created_at"),
        }))
    } else {
        Ok(None)
    }
}

async fn require_user(state: &AppState, session: &Session) -> Result<UserSummary, AppError> {
    current_user(state, session)
        .await?
        .ok_or_else(|| AppError::unauthorized("请先登录以启用云端同步"))
}

async fn require_approved_user(
    state: &AppState,
    session: &Session,
) -> Result<UserSummary, AppError> {
    let user = require_user(state, session).await?;
    if user.status != "approved" {
        return Err(AppError::unauthorized(
            "账号待管理员审批，暂不能使用云端同步和服务器资源存储。",
        ));
    }
    Ok(user)
}

async fn require_admin(state: &AppState, session: &Session) -> Result<UserSummary, AppError> {
    let user = require_approved_user(state, session).await?;
    if user.role != "admin" {
        return Err(AppError::unauthorized("需要管理员权限。"));
    }
    Ok(user)
}

async fn load_sync_envelope(db: &SqlitePool, user_id: &str) -> Result<SyncEnvelope, AppError> {
    let row = sqlx::query("SELECT payload FROM sync_snapshots WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(AppError::internal)?;

    match row {
        Some(row) => {
            serde_json::from_str(&row.get::<String, _>("payload")).map_err(AppError::internal)
        }
        None => Ok(SyncEnvelope::default()),
    }
}

async fn normalize_envelope_assets(
    state: &AppState,
    user_id: &str,
    mut envelope: SyncEnvelope,
) -> Result<SyncEnvelope, AppError> {
    for config in &mut envelope.configs {
        config.api_key_plaintext = None;
    }
    for asset in &mut envelope.assets {
        if asset.remote_object_key.is_some() {
            asset.remote_url = Some(format!("/api/assets/{}", asset.id));
            asset.data_url = None;
            continue;
        }
        let Some(data_url) = asset.data_url.clone() else {
            continue;
        };
        let (mime_type, bytes) = decode_data_url(&data_url)?;
        let object_key = format!("users/{user_id}/assets/{}.bin", asset.sha256);
        put_object(state, &object_key, &mime_type, bytes).await?;
        asset.remote_object_key = Some(object_key);
        asset.remote_url = Some(format!("/api/assets/{}", asset.id));
        asset.data_url = None;

        sqlx::query(
            "INSERT INTO assets (id, user_id, object_key, mime_type, sha256, byte_len, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET object_key = excluded.object_key, mime_type = excluded.mime_type, sha256 = excluded.sha256, byte_len = excluded.byte_len",
        )
        .bind(&asset.id)
        .bind(user_id)
        .bind(asset.remote_object_key.as_deref().unwrap_or_default())
        .bind(&mime_type)
        .bind(&asset.sha256)
        .bind(asset.byte_len as i64)
        .bind(&asset.created_at)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    }
    envelope.updated_at = now_rfc3339();
    Ok(envelope)
}

async fn put_object(
    state: &AppState,
    object_key: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> Result<(), AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(AppError::internal)?;
            }
            tokio::fs::write(path, bytes)
                .await
                .map_err(AppError::internal)?;
            Ok(())
        }
        AssetStoreKind::S3 => {
            let client = state.s3.as_ref().ok_or_else(|| {
                AppError::bad_request("服务器未启用远程资源存储，当前操作不可用。")
            })?;
            client
                .put_object()
                .bucket(&state.config.s3_bucket)
                .key(object_key)
                .content_type(mime_type)
                .body(ByteStream::from(bytes))
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok(())
        }
        AssetStoreKind::Disabled => Err(AppError::bad_request(
            "服务器未启用远程资源存储，当前操作不可用。",
        )),
    }
}

async fn get_object_bytes(state: &AppState, object_key: &str) -> Result<Vec<u8>, AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            tokio::fs::read(path).await.map_err(AppError::internal)
        }
        AssetStoreKind::S3 => {
            let client = state.s3.as_ref().ok_or_else(|| {
                AppError::bad_request("服务器未启用远程资源存储，当前资源无法读取。")
            })?;
            let output = client
                .get_object()
                .bucket(&state.config.s3_bucket)
                .key(object_key)
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok(output
                .body
                .collect()
                .await
                .map_err(AppError::internal)?
                .into_bytes()
                .to_vec())
        }
        AssetStoreKind::Disabled => Err(AppError::bad_request(
            "服务器未启用远程资源存储，当前资源无法读取。",
        )),
    }
}

async fn delete_object(state: &AppState, object_key: &str) -> Result<(), AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            match tokio::fs::remove_file(path).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(AppError::internal(error)),
            }
        }
        AssetStoreKind::S3 => {
            let client = state.s3.as_ref().ok_or_else(|| {
                AppError::bad_request("服务器未启用远程资源存储，无法删除用户图片。")
            })?;
            client
                .delete_object()
                .bucket(&state.config.s3_bucket)
                .key(object_key)
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok(())
        }
        AssetStoreKind::Disabled => Ok(()),
    }
}

async fn delete_user_object_namespace(state: &AppState, user_id: &str) -> Result<(), AppError> {
    let prefix = format!("users/{user_id}/");
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, &prefix)?;
            match tokio::fs::remove_dir_all(path).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(AppError::internal(error)),
            }
        }
        AssetStoreKind::S3 => {
            let client = state.s3.as_ref().ok_or_else(|| {
                AppError::bad_request("服务器未启用远程资源存储，无法删除用户图片。")
            })?;
            let mut continuation_token = None;
            loop {
                let mut request = client
                    .list_objects_v2()
                    .bucket(&state.config.s3_bucket)
                    .prefix(&prefix);
                if let Some(token) = continuation_token.as_deref() {
                    request = request.continuation_token(token);
                }
                let response = request.send().await.map_err(AppError::internal)?;
                for object in response.contents() {
                    if let Some(key) = object.key() {
                        client
                            .delete_object()
                            .bucket(&state.config.s3_bucket)
                            .key(key)
                            .send()
                            .await
                            .map_err(AppError::internal)?;
                    }
                }
                continuation_token = response.next_continuation_token().map(str::to_string);
                if continuation_token.is_none() {
                    break;
                }
            }
            Ok(())
        }
        AssetStoreKind::Disabled => Ok(()),
    }
}

async fn user_role_exists(db: &SqlitePool, role: &str) -> Result<bool, AppError> {
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE role = ?")
        .bind(role)
        .fetch_one(db)
        .await
        .map_err(AppError::internal)?;
    Ok(count > 0)
}

async fn username_exists(db: &SqlitePool, username: &str) -> Result<bool, AppError> {
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = ?")
        .bind(username.trim())
        .fetch_one(db)
        .await
        .map_err(AppError::internal)?;
    Ok(count > 0)
}

async fn user_image_count(db: &SqlitePool, user_id: &str) -> Result<usize, AppError> {
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(db)
        .await
        .map_err(AppError::internal)?;
    Ok(count.max(0) as usize)
}

fn validate_login_credentials(payload: &AuthRequest) -> Result<(), AppError> {
    if payload.username.trim().len() < 3 {
        return Err(AppError::bad_request("用户名至少 3 个字符"));
    }
    if payload.password.len() < 8 {
        return Err(AppError::bad_request("密码至少 8 个字符"));
    }
    Ok(())
}

fn validate_registration(payload: &RegisterRequest) -> Result<(), AppError> {
    if payload.username.trim().len() < 3 {
        return Err(AppError::bad_request("用户名至少 3 个字符"));
    }
    validate_strong_password(&payload.password, &payload.password_confirm)
}

fn validate_strong_password(password: &str, confirm: &str) -> Result<(), AppError> {
    if password != confirm {
        return Err(AppError::bad_request("两次输入的密码不一致"));
    }
    if password.len() < 10 {
        return Err(AppError::bad_request("密码至少 10 个字符"));
    }
    let has_upper = password.chars().any(|ch| ch.is_ascii_uppercase());
    let has_lower = password.chars().any(|ch| ch.is_ascii_lowercase());
    let has_digit = password.chars().any(|ch| ch.is_ascii_digit());
    let has_symbol = password.chars().any(|ch| !ch.is_ascii_alphanumeric());
    if !(has_upper && has_lower && has_digit && has_symbol) {
        return Err(AppError::bad_request(
            "密码必须包含大写字母、小写字母、数字和符号",
        ));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::internal_message(format!("密码哈希失败：{error}")))
}

fn verify_password(password: &str, password_hash: &str) -> Result<(), AppError> {
    let parsed = PasswordHash::new(password_hash)
        .map_err(|error| AppError::internal_message(format!("密码哈希格式无效：{error}")))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AppError::unauthorized("用户名或密码错误"))
}

#[derive(Debug, Clone)]
struct ResolvedUpstreamTarget {
    base_url: String,
}

fn validate_template(
    state: &AppState,
    template: &ProviderTemplate,
    require_custom_host_whitelist: bool,
) -> Result<(), AppError> {
    if template.name.trim().is_empty() {
        return Err(AppError::bad_request("模板名称不能为空"));
    }
    if template.base_url.trim().is_empty() {
        return Err(AppError::bad_request("模板基础地址不能为空"));
    }
    if !template.base_url.starts_with("http://") && !template.base_url.starts_with("https://") {
        return Err(AppError::bad_request("模板基础地址必须是 http/https"));
    }
    if require_custom_host_whitelist {
        resolve_upstream_target(
            state,
            template.kind,
            &template.base_url,
            true,
            true,
            require_custom_host_whitelist,
        )?;
    }
    Ok(())
}

fn validate_generate_request(
    state: &AppState,
    user: Option<&UserSummary>,
    payload: &GenerateViaProxyRequest,
) -> Result<(), AppError> {
    if user.is_none() && !state.config.enable_guest_proxy {
        return Err(AppError::unauthorized(
            "当前部署已关闭游客代理，请登录后再试。",
        ));
    }

    let require_custom_login = matches!(payload.template.kind, ProviderKind::CustomHttp)
        && state.config.require_login_for_custom_provider;
    if require_custom_login && user.is_none() {
        return Err(AppError::unauthorized(
            "自定义服务商仅对登录用户开放，请先登录。",
        ));
    }

    validate_template(
        state,
        &payload.template,
        state.config.enforce_provider_host_whitelist
            && matches!(
                payload.template.kind,
                ProviderKind::OpenAiCompatible | ProviderKind::CustomHttp
            ),
    )?;
    let _ = resolve_upstream_target(
        state,
        payload.template.kind,
        &payload.config.base_url,
        user.is_some(),
        false,
        state.config.enforce_provider_host_whitelist
            && matches!(
                payload.template.kind,
                ProviderKind::OpenAiCompatible | ProviderKind::CustomHttp
            ),
    )?;
    Ok(())
}

fn resolve_provider_base_url(
    state: &AppState,
    kind: ProviderKind,
    configured_base_url: &str,
    user_present: bool,
) -> Result<String, AppError> {
    let default_base_url = match kind {
        ProviderKind::OpenAiImage => Some("https://api.openai.com"),
        ProviderKind::NanoBanana => Some("https://generativelanguage.googleapis.com"),
        ProviderKind::OpenAiCompatible | ProviderKind::CustomHttp => None,
    };
    let base_url = if configured_base_url.trim().is_empty() {
        default_base_url.unwrap_or_default().to_string()
    } else {
        configured_base_url.trim().to_string()
    };
    if base_url.is_empty() {
        return Err(AppError::bad_request("当前配置缺少 Base URL。"));
    }

    let target = resolve_upstream_target(
        state,
        kind,
        &base_url,
        user_present,
        false,
        state.config.enforce_provider_host_whitelist
            && matches!(
                kind,
                ProviderKind::OpenAiCompatible | ProviderKind::CustomHttp
            ),
    )?;
    Ok(target.base_url)
}

fn resolve_upstream_target(
    state: &AppState,
    kind: ProviderKind,
    base_url: &str,
    user_present: bool,
    require_https: bool,
    enforce_custom_whitelist: bool,
) -> Result<ResolvedUpstreamTarget, AppError> {
    let url =
        Url::parse(base_url).map_err(|_| AppError::bad_request("当前配置的 Base URL 无效。"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::provider_target_blocked(
            "仅允许 http/https 上游地址。",
        ));
    }
    if require_https && url.scheme() != "https" {
        return Err(AppError::provider_target_blocked(
            "当前上游仅允许 HTTPS 地址。",
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| AppError::provider_target_blocked("当前上游地址缺少主机名。"))?
        .to_ascii_lowercase();
    reject_unsafe_host(&host)?;

    let mut allowed_hosts = BTreeSet::new();
    match kind {
        ProviderKind::OpenAiImage => {
            allowed_hosts.insert("api.openai.com".to_string());
        }
        ProviderKind::NanoBanana => {
            allowed_hosts.insert("generativelanguage.googleapis.com".to_string());
        }
        ProviderKind::OpenAiCompatible | ProviderKind::CustomHttp => {}
    }
    for host in &state.config.trusted_provider_hosts {
        allowed_hosts.insert(host.to_ascii_lowercase());
    }

    let requires_trusted_host = enforce_custom_whitelist
        || (state.config.enforce_provider_host_whitelist
            && (!matches!(kind, ProviderKind::OpenAiImage | ProviderKind::NanoBanana)
                || user_present));
    if requires_trusted_host && !host_matches_allowlist(&host, &allowed_hosts) {
        return Err(AppError::provider_target_blocked(format!(
            "上游 `{host}` 不在受信任白名单中；可关闭 `MEW_ENFORCE_HOST_WHITELIST`，或将该域名加入 `MEW_TRUSTED_HOSTS`。"
        )));
    }

    Ok(ResolvedUpstreamTarget {
        base_url: url.to_string().trim_end_matches('/').to_string(),
    })
}

fn reject_unsafe_host(host: &str) -> Result<(), AppError> {
    if matches!(host, "localhost" | "localhost.localdomain") {
        return Err(AppError::provider_target_blocked(
            "不允许访问本机或内网地址。",
        ));
    }
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(AppError::provider_target_blocked(
            "不允许访问本地或内部网络地址。",
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(AppError::provider_target_blocked(
                "不允许访问本机、私网或链路本地 IP。",
            ));
        }
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_private()
                || ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
                || ipv4.octets()[0] == 0
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || ipv6.is_unique_local()
                || ipv6.is_unicast_link_local()
                || ipv6.segments()[0] == 0x2001 && ipv6.segments()[1] == 0x0db8
        }
    }
}

fn host_matches_allowlist(host: &str, allowed_hosts: &BTreeSet<String>) -> bool {
    allowed_hosts.iter().any(|candidate| {
        host == candidate
            || host
                .strip_suffix(candidate)
                .map(|prefix| prefix.ends_with('.'))
                .unwrap_or(false)
    })
}

fn ensure_object_storage_ready(state: &AppState) -> Result<(), AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => Ok(()),
        AssetStoreKind::S3 if state.s3.is_some() => Ok(()),
        _ => Err(AppError::bad_request(
            "服务器未启用远程资源存储，请登录前确认资源存储配置完整。",
        )),
    }
}

fn ensure_upload_token_not_expired(row: &sqlx::sqlite::SqliteRow) -> Result<(), AppError> {
    let expires_at = row.get::<String, _>("expires_at");
    let expires_at = chrono::DateTime::parse_from_rfc3339(&expires_at).map_err(|error| {
        AppError::internal_message(format!("上传凭证过期时间解析失败：{error}"))
    })?;
    if expires_at.with_timezone(&Utc) <= Utc::now() {
        return Err(AppError::bad_request("上传凭证已过期，请重新发起上传。"));
    }
    Ok(())
}

async fn cleanup_expired_upload_tokens(db: &SqlitePool) -> Result<(), AppError> {
    sqlx::query("DELETE FROM upload_tokens WHERE expires_at <= ?")
        .bind(now_rfc3339())
        .execute(db)
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

fn decode_data_url(data_url: &str) -> Result<(String, Vec<u8>), AppError> {
    let Some((meta, data)) = data_url.split_once(',') else {
        return Err(AppError::bad_request("无效的数据 URL"));
    };
    let mime_type = meta
        .trim_start_matches("data:")
        .trim_end_matches(";base64")
        .to_string();
    let bytes = BASE64
        .decode(data)
        .map_err(|_| AppError::bad_request("资源 Base64 无效"))?;
    Ok((mime_type, bytes))
}

fn local_object_path(base_dir: &str, object_key: &str) -> Result<PathBuf, AppError> {
    let mut path = PathBuf::from(base_dir);
    for component in FsPath::new(object_key).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return Err(AppError::bad_request("资源路径无效")),
        }
    }
    Ok(path)
}

fn sanitize_file_name(file_name: &str) -> String {
    let sanitized = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('_').trim_matches('.');
    if sanitized.is_empty() {
        "asset.bin".into()
    } else {
        sanitized.chars().take(120).collect()
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn random_token() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 32)
}

fn openai_images_endpoint(request: &mew_image_shared::GenerationRequest) -> &'static str {
    if !request.reference_assets.is_empty() {
        "/v1/images/edits"
    } else {
        "/v1/images/generations"
    }
}

fn join_api_url(base_url: &str, endpoint_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let endpoint = endpoint_path.trim_start_matches('/');
    let base = if base.ends_with("/v1") && endpoint.starts_with("v1/") {
        base.trim_end_matches("/v1")
    } else {
        base
    };
    format!("{base}/{endpoint}")
}

fn openai_compatible_endpoint(request: &mew_image_shared::GenerationRequest) -> &'static str {
    if request.reference_assets.is_empty() {
        "/v1/images/generations"
    } else {
        "/v1/images/edits"
    }
}

fn openai_compatible_response_format(
    request: &mew_image_shared::GenerationRequest,
) -> &'static str {
    if request.reference_assets.is_empty() {
        "url"
    } else {
        // 编辑接口优先请求 base64，兼容中转站直接返回图像数据。
        "b64_json"
    }
}

fn google_image_size_value(
    model: &str,
    request: &mew_image_shared::GenerationRequest,
) -> Option<String> {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.contains("gemini-3") {
        Some(nano_banana_image_size_from_dimensions(
            request.width,
            request.height,
        ))
    } else {
        None
    }
}

fn normalize_google_image_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return "gemini-2.5-flash-image".into();
    }
    if trimmed.starts_with("gemini-3.1-flash-image") && !trimmed.ends_with("-preview") {
        return format!("{trimmed}-preview");
    }
    if trimmed.starts_with("gemini-3-pro-image") && !trimmed.ends_with("-preview") {
        return format!("{trimmed}-preview");
    }
    trimmed.to_string()
}

fn split_data_url_payload(data_url: &str) -> Option<(&str, &str)> {
    let (prefix, payload) = data_url.split_once(',')?;
    let mime_type = prefix.strip_prefix("data:")?.strip_suffix(";base64")?;
    Some((mime_type, payload))
}

async fn invoke_openai_image(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let base_url = resolve_provider_base_url(
        state,
        ProviderKind::OpenAiImage,
        &payload.config.base_url,
        true,
    )?;
    let url = join_api_url(
        &base_url,
        match payload.config.endpoint_mode {
            ProviderEndpointMode::ImagesApi => openai_images_endpoint(&payload.request),
            ProviderEndpointMode::ResponsesApi => "/v1/responses",
            ProviderEndpointMode::CustomJson => payload.template.endpoint_path.as_str(),
        },
    );

    let request = state.http.post(url).bearer_auth(api_key);

    let response = if payload.config.endpoint_mode == ProviderEndpointMode::ImagesApi
        && !payload.request.reference_assets.is_empty()
    {
        let mut form = reqwest::multipart::Form::new()
            .text("prompt", payload.request.prompt.clone())
            .text("model", payload.request.model.clone())
            .text(
                "size",
                format!("{}x{}", payload.request.width, payload.request.height),
            )
            .text("n", payload.request.count.to_string());
        if let Some(quality) = &payload.request.quality {
            form = form.text("quality", quality.clone());
        }
        if let Some(format) = &payload.config.output_format {
            form = form.text("output_format", format.clone());
        }
        if let Some(compression) = payload.config.output_compression {
            form = form.text("output_compression", compression.to_string());
        }
        if let Some(moderation) = &payload.config.moderation {
            form = form.text("moderation", moderation.clone());
        }
        for asset in &payload.request.reference_assets {
            let (mime, bytes) = resolve_asset_bytes(state, asset).await?;
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("{}.png", asset.id))
                .mime_str(&mime)
                .map_err(AppError::internal)?;
            form = form.part("image[]", part);
        }
        if payload
            .request
            .model
            .to_ascii_lowercase()
            .contains("gpt-image-1")
        {
            form = form.text("input_fidelity", "high");
        }
        request.multipart(form).send().await.map_err(|error| {
            warn!("openai image multipart request failed: {}", error);
            AppError::bad_gateway("OpenAI-Image 图像编辑请求失败")
        })?
    } else if payload.config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        let mut content = vec![json!({
            "type": "input_text",
            "text": if payload.config.prompt_guard_enabled {
                format!(
                    "Use the following text as the complete prompt. Do not rewrite it:\n{}",
                    payload.request.prompt
                )
            } else {
                payload.request.prompt.clone()
            },
        })];
        if !payload.request.reference_assets.is_empty() {
            let images = gather_data_urls(state, &payload.request.reference_assets).await?;
            for data_url in images {
                content.push(json!({
                    "type": "input_image",
                    "image_url": data_url,
                }));
            }
        }
        let mut tool = json!({
            "type": "image_generation",
            "action": if payload.request.reference_assets.is_empty() { "generate" } else { "edit" },
            "size": format!("{}x{}", payload.request.width, payload.request.height),
            "output_format": payload.config.output_format.clone().unwrap_or_else(|| "png".into()),
            "moderation": payload.config.moderation.clone().unwrap_or_else(|| "auto".into()),
            "partial_images": 1,
        });
        if !payload.config.prompt_guard_enabled {
            if let Some(quality) = &payload.request.quality {
                tool["quality"] = json!(quality);
            }
        }
        if payload.config.output_format.as_deref() != Some("png") {
            if let Some(compression) = payload.config.output_compression {
                tool["output_compression"] = json!(compression);
            }
        }
        let body = json!({
            "model": if payload.request.model.trim().starts_with("gpt-") && !payload.request.model.trim().starts_with("gpt-image-") {
                payload.request.model.clone()
            } else {
                "gpt-5.5".to_string()
            },
            "input": if payload.request.reference_assets.is_empty() {
                content[0]["text"].clone()
            } else {
                json!([{
                    "role": "user",
                    "content": content,
                }])
            },
            "tools": [tool],
            "tool_choice": "required",
            "stream": true,
        });
        request.json(&body).send().await.map_err(|error| {
            warn!("openai image responses request failed: {}", error);
            AppError::bad_gateway("Responses API 请求失败")
        })?
    } else {
        let body = json!({
            "prompt": payload.request.prompt,
            "model": payload.request.model,
            "size": format!("{}x{}", payload.request.width, payload.request.height),
            "quality": payload.request.quality,
            "n": payload.request.count,
            "output_format": payload.config.output_format,
            "output_compression": payload.config.output_compression,
            "moderation": payload.config.moderation,
        });
        request.json(&body).send().await.map_err(|error| {
            warn!("openai image json request failed: {}", error);
            AppError::bad_gateway("Images API 请求失败")
        })?
    };

    if payload.config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        let status = response.status();
        let is_event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        let body = response.text().await.map_err(AppError::internal)?;
        if !status.is_success() {
            return Err(AppError::bad_gateway(format!(
                "Responses API 上游请求失败：HTTP {status}，{body}"
            )));
        }
        if is_event_stream || body.trim_start().starts_with("data:") {
            return parse_openai_responses_event_stream(&body).map_err(AppError::bad_gateway);
        }
        return serde_json::from_str(&body).map_err(AppError::internal);
    }

    response.json().await.map_err(AppError::internal)
}

async fn invoke_openai_compatible_image(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let base_url = resolve_provider_base_url(
        state,
        ProviderKind::OpenAiCompatible,
        &payload.config.base_url,
        true,
    )?;
    let url = join_api_url(&base_url, openai_compatible_endpoint(&payload.request));

    let response = if payload.request.reference_assets.is_empty() {
        state
            .http
            .post(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .json(&json!({
                "model": payload.request.model,
                "prompt": payload.request.prompt,
                "aspect_ratio": aspect_ratio_from_dimensions(payload.request.width, payload.request.height),
                "response_format": "url",
                "image_size": nano_banana_image_size_from_dimensions(payload.request.width, payload.request.height),
                "size": format!("{}x{}", payload.request.width, payload.request.height),
                "n": payload.request.count,
            }))
            .send()
            .await
            .map_err(|error| {
                warn!("openai compatible request failed: {}", error);
                AppError::bad_gateway("OpenAI 兼容请求失败")
            })?
    } else {
        let mut form = reqwest::multipart::Form::new()
            .text("model", payload.request.model.clone())
            .text("prompt", payload.request.prompt.clone())
            .text(
                "aspect_ratio",
                aspect_ratio_from_dimensions(payload.request.width, payload.request.height),
            )
            .text(
                "response_format",
                openai_compatible_response_format(&payload.request),
            )
            .text(
                "image_size",
                nano_banana_image_size_from_dimensions(
                    payload.request.width,
                    payload.request.height,
                ),
            )
            .text("n", payload.request.count.to_string());
        for asset in &payload.request.reference_assets {
            let (mime, bytes) = resolve_asset_bytes(state, asset).await?;
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("{}.{}", asset.id, mime_extension(&mime)))
                .mime_str(&mime)
                .map_err(AppError::internal)?;
            form = form.part("image", part);
        }
        state
            .http
            .post(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .await
            .map_err(|error| {
                warn!("openai compatible multipart request failed: {}", error);
                AppError::bad_gateway("OpenAI 兼容请求失败")
            })?
    };

    response.json().await.map_err(AppError::internal)
}

async fn invoke_nano_banana_google(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let model = normalize_google_image_model(&payload.request.model);
    let base_url = resolve_provider_base_url(
        state,
        ProviderKind::NanoBanana,
        &payload.config.base_url,
        true,
    )?;
    let url = join_api_url(
        &base_url,
        &format!("/v1beta/models/{model}:generateContent"),
    );
    let body = build_gemini_payload(state, payload).await?;
    let response = state
        .http
        .post(url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            warn!("nano banana request failed: {}", error);
            AppError::bad_gateway("Nano Banana 请求失败")
        })?;

    response.json().await.map_err(AppError::internal)
}

async fn invoke_custom_http(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let base_url = resolve_provider_base_url(
        state,
        ProviderKind::CustomHttp,
        &payload.config.base_url,
        true,
    )?;
    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        payload.template.endpoint_path
    );
    let mut body = json!({});
    set_json_path(
        &mut body,
        payload.template.prompt_field.as_deref().unwrap_or("prompt"),
        json!(payload.request.prompt),
    );
    set_json_path(
        &mut body,
        payload.template.model_field.as_deref().unwrap_or("model"),
        json!(payload.request.model),
    );
    set_json_path(
        &mut body,
        payload.template.size_field.as_deref().unwrap_or("size"),
        json!(format!(
            "{}x{}",
            payload.request.width, payload.request.height
        )),
    );
    set_json_path(
        &mut body,
        payload.template.count_field.as_deref().unwrap_or("n"),
        json!(payload.request.count),
    );
    if let Some(quality) = &payload.request.quality {
        if let Some(path) = payload.template.quality_field.as_deref() {
            set_json_path(&mut body, path, json!(quality));
        }
    }

    let response = state
        .http
        .request(
            payload
                .template
                .method
                .parse()
                .map_err(|_| AppError::bad_request("自定义模板 HTTP 方法无效"))?,
            url,
        )
        .header(&payload.template.auth_header, format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            warn!("custom provider request failed: {}", error);
            AppError::bad_gateway("自定义服务商请求失败")
        })?;

    response.json().await.map_err(AppError::internal)
}

fn mime_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/webp" => "webp",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "bin",
    }
}

async fn hydrate_proxy_result_images(
    state: &AppState,
    mut result: GenerationResult,
) -> Result<GenerationResult, AppError> {
    for image in &mut result.images {
        if image.data_url.is_some() {
            continue;
        }
        let Some(url) = image.url.clone() else {
            continue;
        };
        if let Ok((mime_type, bytes)) = fetch_remote_image_bytes(state, &url).await {
            image.data_url = Some(format!("data:{mime_type};base64,{}", BASE64.encode(bytes)));
        }
    }
    Ok(result)
}

async fn fetch_remote_image_bytes(
    state: &AppState,
    image_url: &str,
) -> Result<(String, Vec<u8>), AppError> {
    let parsed =
        Url::parse(image_url).map_err(|_| AppError::bad_request("上游返回了无效的图片地址。"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::provider_target_blocked("上游返回的图片地址缺少主机名。"))?
        .to_ascii_lowercase();
    reject_unsafe_host(&host)?;

    let mut allowed_hosts = BTreeSet::new();
    for value in &state.config.trusted_provider_hosts {
        allowed_hosts.insert(value.to_ascii_lowercase());
    }
    allowed_hosts.insert("api.openai.com".into());
    allowed_hosts.insert("oaidalleapiprodscus.blob.core.windows.net".into());
    allowed_hosts.insert("generativelanguage.googleapis.com".into());
    if state.config.enforce_provider_host_whitelist
        && !host_matches_allowlist(&host, &allowed_hosts)
    {
        return Err(AppError::provider_target_blocked(format!(
            "上游返回的图片地址 `{host}` 不在允许的下载白名单中；可关闭 `MEW_ENFORCE_HOST_WHITELIST`，或将该域名加入 `MEW_TRUSTED_HOSTS`。"
        )));
    }

    let response = state.http.get(parsed).send().await.map_err(|error| {
        warn!("remote image fetch failed: {}", error);
        AppError::bad_gateway("下载上游图片失败")
    })?;
    if !response.status().is_success() {
        return Err(AppError::bad_gateway(format!(
            "下载上游图片失败：HTTP {}",
            response.status()
        )));
    }
    let mime_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/png")
        .to_string();
    let bytes = response.bytes().await.map_err(AppError::internal)?.to_vec();
    Ok((mime_type, bytes))
}

#[derive(serde::Deserialize)]
struct FetchImageRequest {
    url: String,
}

#[derive(serde::Serialize)]
struct FetchImageResponse {
    mime_type: String,
    body_base64: String,
}

fn extract_generation_result(
    template: &ProviderTemplate,
    output_format: Option<&str>,
    request: &mew_image_shared::GenerationRequest,
    response_json: serde_json::Value,
    duration_ms: u64,
) -> GenerationResult {
    if template.kind == ProviderKind::NanoBanana {
        let mut result = extract_gemini_generation_result(request, response_json, output_format)
            .unwrap_or_else(|error| GenerationResult {
                images: Vec::new(),
                parameter_snapshot: ParameterSnapshot {
                    requested_width: Some(request.width),
                    requested_height: Some(request.height),
                    actual_width: Some(request.width),
                    actual_height: Some(request.height),
                    requested_quality: request.quality.clone(),
                    actual_quality: Some("standard".into()),
                    revised_prompt: None,
                    duration_ms: Some(duration_ms),
                },
                raw_response_json: Some(serde_json::json!({ "error": error })),
            });
        result.parameter_snapshot.duration_ms = Some(duration_ms);
        return result;
    }
    if template.kind == ProviderKind::OpenAiCompatible {
        let mut result = extract_openai_compatible_result(request, response_json, output_format)
            .unwrap_or_else(|error| GenerationResult {
                images: Vec::new(),
                parameter_snapshot: ParameterSnapshot {
                    requested_width: Some(request.width),
                    requested_height: Some(request.height),
                    actual_width: Some(request.width),
                    actual_height: Some(request.height),
                    requested_quality: request.quality.clone(),
                    actual_quality: Some("standard".into()),
                    revised_prompt: None,
                    duration_ms: Some(duration_ms),
                },
                raw_response_json: Some(serde_json::json!({ "error": error })),
            });
        result.parameter_snapshot.duration_ms = Some(duration_ms);
        return result;
    }
    if request.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        let upstream_response = response_json.clone();
        let mut result = extract_openai_responses_result(request, response_json, output_format)
            .unwrap_or_else(|error| GenerationResult {
                images: Vec::new(),
                parameter_snapshot: ParameterSnapshot {
                    requested_width: Some(request.width),
                    requested_height: Some(request.height),
                    actual_width: Some(request.width),
                    actual_height: Some(request.height),
                    requested_quality: request.quality.clone(),
                    actual_quality: request.quality.clone(),
                    revised_prompt: None,
                    duration_ms: Some(duration_ms),
                },
                raw_response_json: Some(serde_json::json!({
                    "parse_error": error,
                    "upstream_response": upstream_response,
                })),
            });
        result.parameter_snapshot.duration_ms = Some(duration_ms);
        return result;
    }
    let urls = template
        .response_image_url_path
        .as_deref()
        .map(|path| collect_json_path(&response_json, path))
        .unwrap_or_default();
    let base64_images = template
        .response_image_base64_path
        .as_deref()
        .map(|path| collect_json_path(&response_json, path))
        .unwrap_or_default();

    let mut images = Vec::new();
    for value in urls {
        if let Some(url) = value.as_str() {
            images.push(GeneratedImageResult {
                url: Some(url.to_string()),
                data_url: None,
            });
        }
    }
    for value in base64_images {
        if let Some(raw) = value.as_str() {
            images.push(GeneratedImageResult {
                url: None,
                data_url: Some(format!("data:image/png;base64,{raw}")),
            });
        }
    }

    let revised_prompt = template
        .response_revised_prompt_path
        .as_deref()
        .and_then(|path| collect_json_path(&response_json, path).into_iter().next())
        .and_then(|value| value.as_str().map(str::to_string));

    GenerationResult {
        images,
        parameter_snapshot: ParameterSnapshot {
            requested_width: Some(request.width),
            requested_height: Some(request.height),
            actual_width: Some(request.width),
            actual_height: Some(request.height),
            requested_quality: request.quality.clone(),
            actual_quality: request.quality.clone(),
            revised_prompt,
            duration_ms: Some(duration_ms),
        },
        raw_response_json: Some(response_json),
    }
}

async fn gather_data_urls(
    state: &AppState,
    assets: &[ImageAssetRef],
) -> Result<Vec<String>, AppError> {
    let mut results = Vec::with_capacity(assets.len());
    for asset in assets {
        if let Some(data_url) = &asset.data_url {
            results.push(data_url.clone());
            continue;
        }
        let (mime, bytes) = resolve_asset_bytes(state, asset).await?;
        results.push(format!("data:{mime};base64,{}", BASE64.encode(bytes)));
    }
    Ok(results)
}

async fn build_gemini_payload(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let model = normalize_google_image_model(&payload.request.model);
    let mut parts = vec![json!({
        "text": payload.request.prompt.clone(),
    })];
    let data_urls = gather_data_urls(state, &payload.request.reference_assets).await?;
    for data_url in data_urls {
        if let Some((mime_type, data)) = split_data_url_payload(&data_url) {
            parts.push(json!({
                "inline_data": {
                    "mime_type": mime_type,
                    "data": data,
                }
            }));
        }
    }

    let mut generation_config = json!({
        "responseModalities": ["TEXT", "IMAGE"],
        "imageConfig": {
            "aspectRatio": aspect_ratio_from_dimensions(
                payload.request.width,
                payload.request.height,
            ),
        }
    });
    if let Some(image_size) = google_image_size_value(&model, &payload.request) {
        generation_config["imageConfig"]["imageSize"] = json!(image_size);
    }

    Ok(json!({
        "contents": [{
            "role": "user",
            "parts": parts,
        }],
        "generationConfig": generation_config,
    }))
}

async fn resolve_asset_bytes(
    state: &AppState,
    asset: &ImageAssetRef,
) -> Result<(String, Vec<u8>), AppError> {
    if let Some(data_url) = &asset.data_url {
        return decode_data_url(data_url);
    }
    let object_key = asset
        .remote_object_key
        .as_ref()
        .ok_or_else(|| AppError::bad_request("资源缺少可读取的图像数据"))?;
    let bytes = get_object_bytes(state, object_key).await?;
    Ok((asset.mime_type.clone(), bytes))
}

fn set_json_path(target: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let mut current = target;
    let segments: Vec<&str> = path.split('.').collect();
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index == segments.len() - 1;
        if is_last {
            if let Some(object) = current.as_object_mut() {
                object.insert((*segment).to_string(), value.clone());
            }
            return;
        }
        if current.get(segment).is_none() {
            current[segment] = json!({});
        }
        current = &mut current[segment];
    }
}

fn collect_json_path(value: &serde_json::Value, path: &str) -> Vec<serde_json::Value> {
    fn walk(current: &serde_json::Value, parts: &[&str], output: &mut Vec<serde_json::Value>) {
        if parts.is_empty() {
            output.push(current.clone());
            return;
        }
        let part = parts[0];
        if let Some(key) = part.strip_suffix("[]") {
            if let Some(array) = current.get(key).and_then(|value| value.as_array()) {
                for item in array {
                    walk(item, &parts[1..], output);
                }
            }
            return;
        }
        if let Some((key, raw_index)) = part.split_once('[') {
            let index = raw_index
                .trim_end_matches(']')
                .parse::<usize>()
                .unwrap_or(0);
            if let Some(item) = current
                .get(key)
                .and_then(|value| value.as_array())
                .and_then(|array| array.get(index))
            {
                walk(item, &parts[1..], output);
            }
            return;
        }
        if let Some(next) = current.get(part) {
            walk(next, &parts[1..], output);
        }
    }

    let mut values = Vec::new();
    walk(value, &path.split('.').collect::<Vec<_>>(), &mut values);
    values
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            code: None,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            code: None,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            code: None,
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            code: None,
        }
    }

    fn provider_target_blocked(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            code: Some("provider_target_blocked"),
        }
    }

    fn internal_message(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            code: None,
        }
    }

    fn internal(error: impl std::error::Error) -> Self {
        error!("internal error: {}", error);
        Self::internal_message("服务器内部错误")
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
                "code": self.code,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{GenerationRequest, ProviderEndpointMode, SyncTombstone};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn data_url_can_be_decoded() {
        let (mime, bytes) = decode_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(mime, "text/plain");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn private_hosts_are_blocked() {
        assert!(reject_unsafe_host("127.0.0.1").is_err());
        assert!(reject_unsafe_host("10.0.0.8").is_err());
        assert!(reject_unsafe_host("localhost").is_err());
        assert!(reject_unsafe_host("service.internal").is_err());
        assert!(reject_unsafe_host("api.openai.com").is_ok());
    }

    #[test]
    fn allowlist_matches_exact_host_and_subdomain() {
        let allowed = BTreeSet::from(["api.openai.com".to_string(), "example.com".to_string()]);
        assert!(host_matches_allowlist("api.openai.com", &allowed));
        assert!(host_matches_allowlist("cdn.example.com", &allowed));
        assert!(!host_matches_allowlist("evil-example.com", &allowed));
    }

    #[test]
    fn public_gateway_host_is_allowed_by_basic_safety_policy() {
        assert!(reject_unsafe_host("api.cphone.vip").is_ok());
        assert!(reject_unsafe_host("cdnoss.jounery.vip").is_ok());
    }

    #[test]
    fn private_ip_detection_covers_v4_and_v6() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_private_ip(IpAddr::V6(
            "fd00::1".parse::<Ipv6Addr>().unwrap()
        )));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn responses_result_can_find_nested_base64() {
        let request = GenerationRequest {
            prompt: "test".into(),
            model: "gpt-5.5".into(),
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: ProviderEndpointMode::ResponsesApi,
            reference_assets: Vec::new(),
        };
        let response_json = serde_json::json!({
            "output": [{
                "type": "image_generation_call",
                "result": {
                    "payload": {
                        "items": [{
                            "base64": "aGVsbG8="
                        }]
                    }
                },
                "revised_prompt": "better prompt",
                "size": "1024x1024",
                "quality": "high"
            }]
        });

        let result = extract_openai_responses_result(&request, response_json, Some("png")).unwrap();
        assert_eq!(result.images.len(), 1);
        assert!(
            result.images[0]
                .data_url
                .as_ref()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert_eq!(
            result.parameter_snapshot.revised_prompt.as_deref(),
            Some("better prompt")
        );
    }

    #[tokio::test]
    async fn tombstone_cleanup_keeps_shared_object_until_last_reference_is_deleted() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        init_db(&db).await.unwrap();
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let config = AppConfig {
            listen_addr: "127.0.0.1:0".into(),
            database_url: "sqlite::memory:".into(),
            frontend_dist: String::new(),
            session_secure: false,
            allowed_web_origins: Vec::new(),
            trusted_provider_hosts: Vec::new(),
            enforce_provider_host_whitelist: false,
            enable_guest_proxy: true,
            require_login_for_custom_provider: true,
            admin_setup_token: None,
            allow_first_admin_setup: false,
            asset_store: AssetStoreKind::Local,
            local_asset_dir: asset_dir.to_string_lossy().into_owned(),
            s3_bucket: String::new(),
            s3_region: "auto".into(),
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
        };
        let state = AppState {
            config,
            db,
            s3: None,
            http: reqwest::Client::new(),
            provider_builtins: Vec::new(),
        };
        let object_key = "users/user-1/assets/shared.bin";
        put_object(&state, object_key, "image/png", b"image".to_vec())
            .await
            .unwrap();
        for asset_id in ["asset-1", "asset-2"] {
            sqlx::query(
                "INSERT INTO assets (id, user_id, object_key, mime_type, sha256, byte_len, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(asset_id)
            .bind("user-1")
            .bind(object_key)
            .bind("image/png")
            .bind("shared")
            .bind(5_i64)
            .bind(now_rfc3339())
            .execute(&state.db)
            .await
            .unwrap();
        }

        let asset_2 = ImageAssetRef {
            id: "asset-2".into(),
            sha256: "shared".into(),
            mime_type: "image/png".into(),
            byte_len: 5,
            width: None,
            height: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: None,
            remote_object_key: Some(object_key.into()),
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        };
        let first_delete = SyncEnvelope {
            assets: vec![asset_2],
            tombstones: vec![SyncTombstone {
                entity_kind: SyncEntityKind::Asset,
                entity_id: "asset-1".into(),
                deleted_at: now_rfc3339(),
            }],
            ..SyncEnvelope::default()
        };
        cleanup_tombstoned_assets(&state, "user-1", &first_delete)
            .await
            .unwrap();
        assert!(
            local_object_path(&state.config.local_asset_dir, object_key)
                .unwrap()
                .exists()
        );

        let final_delete = SyncEnvelope {
            tombstones: vec![SyncTombstone {
                entity_kind: SyncEntityKind::Asset,
                entity_id: "asset-2".into(),
                deleted_at: now_rfc3339(),
            }],
            ..SyncEnvelope::default()
        };
        cleanup_tombstoned_assets(&state, "user-1", &final_delete)
            .await
            .unwrap();
        assert!(
            !local_object_path(&state.config.local_asset_dir, object_key)
                .unwrap()
                .exists()
        );
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }
}
