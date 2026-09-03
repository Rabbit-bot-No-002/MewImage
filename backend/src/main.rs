mod migrations;
mod security_headers;
mod state;
mod sync_store;

use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, SocketAddr},
    path::{Component, Path as FsPath, PathBuf},
    str::FromStr,
    sync::{Arc, OnceLock},
    time::{Duration as StdDuration, Instant},
};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use aws_config::{BehaviorVersion, timeout::TimeoutConfig};
use aws_sdk_s3::{Client as S3Client, config::Region, primitives::ByteStream};
use axum::{
    Json, Router,
    body::{Bytes, to_bytes},
    extract::DefaultBodyLimit,
    extract::{ConnectInfo, Multipart, Path, Query, Request, State, multipart::Field},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration, Utc};
use mew_image_shared::{
    AdminBootstrapRequest, AdminSetupStatusResponse, AdminUserActionRequest, AdminUserSummary,
    AdminUsersResponse, AssetPresenceRequest, AssetPresenceResponse, AuthRequest, AuthResponse,
    BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID, ChangePasswordRequest, CloudDataClearRequest,
    CloudDataClearScope, CloudDataStatsResponse, GenerateViaProxyRequest, GeneratedImageResult,
    GenerationResult, ImageAssetRef, MeResponse, MergePreviewResponse,
    OpenAiResponsesStreamAccumulator, ParameterSnapshot, ProviderEndpointMode, ProviderKind,
    ProviderTemplate, ProviderTemplateImportRequest, ProxyGenerationJobAccepted,
    ProxyGenerationJobResponse, ProxyGenerationJobStatus, RegisterRequest, SyncEntityKind,
    SyncEnvelope, SyncPullResponse, SyncPushRequest, UploadCompleteRequest, UploadCompleteResponse,
    UploadInitRequest, UploadInitResponse, UserSummary, UsernameAvailabilityResponse,
    aspect_ratio_from_dimensions, build_gemini_generation_request,
    extract_gemini_generation_result, extract_openai_compatible_result,
    extract_openai_responses_result, gemini_auth_header, gemini_generate_content_url,
    is_google_official_gemini_base_url, merge_envelopes, nano_banana_image_size_from_dimensions,
    new_id, normalized_image_output_format, normalized_openai_background, now_rfc3339,
    openai_output_compression, parse_openai_responses_event_stream, resolve_responses_main_model,
    strip_successful_task_payloads,
};
use rand::distr::{Alphanumeric, SampleString};
use reqwest::Url;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use state::{
    AppConfig, AppState, AssetStoreKind, GuestLimitRejection, GuestProxyLimits,
    GuestProxyOperation, GuestProxyPermit, ProxyGenerationJob, ProxyGenerationJobState,
};
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tower_cookies::{
    Cookie, CookieManagerLayer, Cookies,
    cookie::{SameSite, time::Duration as CookieDuration},
};
use tower_http::{
    cors::{AllowHeaders, AllowOrigin, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tower_sessions::{ExpiredDeletion, Session, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;
use tracing::{error, info, warn};

const MAX_ACTIVE_PROXY_GENERATION_JOBS: usize = 20;
const MAX_STORED_PROXY_GENERATION_JOBS: usize = 32;
const PROXY_GENERATION_JOB_TIMEOUT: StdDuration = StdDuration::from_secs(30 * 60);
const PROXY_GENERATION_RESULT_TTL: StdDuration = StdDuration::from_secs(10 * 60);
const MAX_ASSET_PRESENCE_CHECKS: usize = 10_000;
const MAX_SYNC_ASSETS: usize = 10_000;
const DEFAULT_JSON_BODY_LIMIT: usize = 2 * 1024 * 1024;
const AUTH_BODY_LIMIT: usize = 64 * 1024;
const SYNC_BODY_LIMIT: usize = 32 * 1024 * 1024;
const GENERATION_BODY_LIMIT: usize = 192 * 1024 * 1024;
const IMAGE_FETCH_BODY_LIMIT: usize = 32 * 1024;
const MAX_GENERATION_REFERENCE_COUNT: usize = 16;
const MAX_GENERATION_REFERENCE_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GENERATION_REFERENCE_TOTAL_BYTES: usize = 160 * 1024 * 1024;
const MAX_GENERATION_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_REMOTE_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REMOTE_IMAGE_REDIRECTS: usize = 3;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 256 * 1024 * 1024;
const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;
const USER_DATA_WRITE_LOCK_SHARDS: usize = 256;
const PROXY_TEMP_FILE_TTL: StdDuration = StdDuration::from_secs(45 * 60);
const S3_CONNECT_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const S3_OPERATION_ATTEMPT_TIMEOUT: StdDuration = StdDuration::from_secs(2 * 60);
const S3_OPERATION_TIMEOUT: StdDuration = StdDuration::from_secs(5 * 60);
const DOCKER_DATA_PERMISSION_HINT: &str = "Docker Compose 默认以 UID:GID 10001:10001 运行；请在部署目录停止容器后执行 `sudo chown -R 10001:10001 ./data`，并确认该目录允许所有者读写。";
const REGISTRATION_DEVICE_COOKIE: &str = "mew_registration_device";
const OPENAI_EDIT_IMAGE_FIELD: &str = "image[]";
static PROXY_TEMP_DIR: OnceLock<PathBuf> = OnceLock::new();
#[cfg(all(target_os = "linux", target_env = "gnu"))]
static MALLOC_TRIM_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct GenerationMemoryTrimGuard;

#[derive(Debug, Clone, Copy)]
enum UpstreamRequestKind {
    Generation,
    Image,
}

struct PreparedUpstreamRequest {
    client: reqwest::Client,
    url: Url,
}

#[derive(Clone)]
struct ResponseMemoryPermit {
    _permit: Arc<tokio::sync::OwnedSemaphorePermit>,
}

struct ParsedGeneratePayload {
    payload: GenerateViaProxyRequest,
    reference_files: Vec<TemporaryReferenceFile>,
}

struct TemporaryReferenceFile {
    path: PathBuf,
    mime_type: String,
    byte_len: u64,
    sha256: String,
}

impl Drop for TemporaryReferenceFile {
    fn drop(&mut self) {
        // 任务完成、取消或超时时都会随请求快照释放，避免参考图滞留临时目录。
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for GenerationMemoryTrimGuard {
    fn drop(&mut self) {
        trim_process_heap();
    }
}

fn trim_process_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        if MALLOC_TRIM_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // 只在大型代理请求结束时整理 glibc 堆，避免并发触发全局内存扫描。
        unsafe {
            libc::malloc_trim(0);
        }
        MALLOC_TRIM_IN_PROGRESS.store(false, Ordering::Release);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("healthcheck")) {
        return run_healthcheck().await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mew_image_backend=debug,tower_http=info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    prepare_proxy_temp_dir().await?;
    ensure_sqlite_parent_dir(&config.database_url)?;
    ensure_asset_store_ready(&config)?;
    let db_options = SqliteConnectOptions::from_str(&config.database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(StdDuration::from_secs(10));
    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(db_options)
        .await
        .with_context(|| {
            format!(
                "无法打开 SQLite 数据库 `{}`。{DOCKER_DATA_PERMISSION_HINT}",
                config.database_url
            )
        })?;
    init_db(&db).await.with_context(|| {
        format!(
            "初始化 SQLite 数据结构失败。数据库位置：`{}`。{DOCKER_DATA_PERMISSION_HINT}",
            config.database_url
        )
    })?;

    let s3 = build_s3_client(&config).await?;
    let dummy_password_hash = hash_password("MewImage dummy password verification")
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let builtins = vec![
        ProviderTemplate::builtin_openai(),
        ProviderTemplate::builtin_nano_banana(),
        ProviderTemplate::builtin_openai_compatible(),
    ];

    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        s3,
        provider_builtins: builtins,
        generation_job_slots: Arc::new(tokio::sync::Semaphore::new(
            MAX_ACTIVE_PROXY_GENERATION_JOBS,
        )),
        generation_temp_budget: Arc::new(tokio::sync::Semaphore::new(
            config.proxy_memory_budget_mib,
        )),
        generation_memory_budget: Arc::new(tokio::sync::Semaphore::new(
            config.proxy_memory_budget_mib,
        )),
        generation_jobs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        user_data_write_locks: Arc::new(
            (0..USER_DATA_WRITE_LOCK_SHARDS)
                .map(|_| tokio::sync::Mutex::new(()))
                .collect(),
        ),
        auth_hash_semaphore: Arc::new(tokio::sync::Semaphore::new(config.auth_hash_concurrency)),
        dummy_password_hash,
        guest_proxy_limits: Arc::new(GuestProxyLimits::default()),
    });
    let upload_cleanup_task = tokio::spawn(periodically_cleanup_expired_uploads(state.clone()));

    // 会话与业务数据共用连接池，避免同一个 SQLite 文件被两个独立池放大写锁竞争。
    let session_store = SqliteStore::new(state.db.clone())
        .with_table_name("mew_image_sessions")
        .map_err(anyhow::Error::msg)?;
    session_store.migrate().await.with_context(|| {
        format!(
            "初始化 SQLite 会话表失败。数据库位置：`{}`。{DOCKER_DATA_PERMISSION_HINT}",
            config.database_url
        )
    })?;
    let session_cleanup_task = tokio::spawn(
        session_store
            .clone()
            .continuously_delete_expired(std::time::Duration::from_secs(900)),
    );

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(config.session_secure)
        .with_same_site(tower_sessions::cookie::SameSite::Lax);

    let cors_layer = build_cors_layer(&config)?;
    let max_upload_body_limit = usize::try_from(config.max_upload_bytes).unwrap_or(usize::MAX);

    let app = Router::new()
        .route("/api/health", get(health))
        .route(
            "/api/auth/register",
            post(register).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route("/api/auth/check-username", get(check_username))
        .route("/api/auth/setup-status", get(admin_setup_status))
        .route(
            "/api/auth/bootstrap-admin",
            post(bootstrap_admin).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route(
            "/api/auth/login",
            post(login).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route(
            "/api/auth/change-password",
            post(change_password).layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT)),
        )
        .route("/api/admin/users", get(admin_list_users))
        .route("/api/admin/users/approve", post(admin_approve_user))
        .route("/api/admin/users/disable", post(admin_disable_user))
        .route("/api/admin/users/restore", post(admin_restore_user))
        .route("/api/admin/users/delete", post(admin_delete_user))
        .route(
            "/api/sync/push",
            post(sync_push).layer(DefaultBodyLimit::max(SYNC_BODY_LIMIT)),
        )
        .route("/api/sync/pull", get(sync_pull))
        .route(
            "/api/sync/merge-preview",
            post(sync_merge_preview).layer(DefaultBodyLimit::max(SYNC_BODY_LIMIT)),
        )
        .route("/api/data/stats", get(cloud_data_stats))
        .route("/api/data/clear", post(clear_cloud_data))
        .route(
            "/api/providers/templates",
            get(list_provider_templates)
                .post(import_provider_template)
                .layer(DefaultBodyLimit::max(DEFAULT_JSON_BODY_LIMIT)),
        )
        .route(
            "/api/providers/generate",
            post(generate_via_proxy).layer(DefaultBodyLimit::max(GENERATION_BODY_LIMIT)),
        )
        .route(
            "/api/providers/generate/{job_id}",
            get(get_proxy_generation_job).delete(cancel_proxy_generation_job),
        )
        .route("/api/assets/upload-init", post(upload_init))
        .route(
            "/api/assets/upload/{token}",
            put(upload_bytes).layer(DefaultBodyLimit::max(max_upload_body_limit)),
        )
        .route("/api/assets/complete", post(upload_complete))
        .route("/api/assets/presence", post(check_asset_presence))
        .route("/api/assets/{asset_id}", get(get_asset))
        .route(
            "/api/images/fetch",
            post(fetch_image_via_proxy).layer(DefaultBodyLimit::max(IMAGE_FETCH_BODY_LIMIT)),
        )
        .fallback_service(
            ServeDir::new(&config.frontend_dist)
                .precompressed_br()
                .precompressed_gzip()
                .append_index_html_on_directories(true),
        )
        .layer(DefaultBodyLimit::max(DEFAULT_JSON_BODY_LIMIT))
        .layer(middleware::from_fn(
            security_headers::apply_security_headers,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer)
        .layer(session_layer)
        .layer(CookieManagerLayer::new())
        .with_state(state.clone());

    let addr: SocketAddr = config.listen_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!("backend listening on {}", addr);
    let server_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    session_cleanup_task.abort();
    upload_cleanup_task.abort();
    abort_all_proxy_generation_jobs(&state).await;
    cleanup_current_proxy_temp_dir().await;
    server_result?;
    Ok(())
}

async fn run_healthcheck() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    let mut address: SocketAddr = config.listen_addr.parse()?;
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        });
    }
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(StdDuration::from_secs(2))
        .timeout(StdDuration::from_secs(5))
        .no_proxy()
        .build()?
        .get(format!("http://{address}/api/health"))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("healthcheck failed with HTTP {}", response.status());
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            error!("failed to install Ctrl+C handler: {error}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => error!("failed to install SIGTERM handler: {error}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    info!("shutdown signal received");
}

async fn abort_all_proxy_generation_jobs(state: &AppState) {
    let mut jobs = state.generation_jobs.lock().await;
    for abort_handle in jobs.drain().filter_map(|(_, job)| job.abort_handle) {
        abort_handle.abort();
    }
}

fn proxy_temp_root() -> PathBuf {
    std::env::temp_dir().join("mew-image-proxy")
}

fn proxy_temp_dir() -> &'static PathBuf {
    PROXY_TEMP_DIR
        .get_or_init(|| proxy_temp_root().join(format!("{}-{}", std::process::id(), new_id())))
}

async fn prepare_proxy_temp_dir() -> anyhow::Result<()> {
    let directory = proxy_temp_dir();
    tokio::fs::create_dir_all(directory)
        .await
        .with_context(|| writable_path_context("创建代理临时目录", directory))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| writable_path_context("设置代理临时目录权限", directory))?;
    }
    cleanup_stale_proxy_temp_dirs().await;
    Ok(())
}

async fn cleanup_stale_proxy_temp_dirs() {
    let root = proxy_temp_root();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.path() == *proxy_temp_dir() {
            continue;
        }
        let is_stale = entry
            .metadata()
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= PROXY_TEMP_FILE_TTL);
        if is_stale {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

async fn cleanup_current_proxy_temp_dir() {
    let _ = tokio::fs::remove_dir_all(proxy_temp_dir()).await;
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
        std::fs::create_dir_all(parent)
            .with_context(|| writable_path_context("创建 SQLite 数据目录", parent))?;
    }
    Ok(())
}

fn ensure_asset_store_ready(config: &AppConfig) -> anyhow::Result<()> {
    if config.asset_store == AssetStoreKind::Local {
        std::fs::create_dir_all(&config.local_asset_dir).with_context(|| {
            writable_path_context("创建本地图片资源目录", FsPath::new(&config.local_asset_dir))
        })?;
    }
    Ok(())
}

fn writable_path_context(action: &str, path: &FsPath) -> String {
    format!(
        "{action} `{}` 失败。{DOCKER_DATA_PERMISSION_HINT}",
        path.display()
    )
}

async fn build_s3_client(config: &AppConfig) -> anyhow::Result<Option<S3Client>> {
    if config.asset_store != AssetStoreKind::S3 || config.s3_bucket.is_empty() {
        return Ok(None);
    }

    let timeout_config = TimeoutConfig::builder()
        .connect_timeout(S3_CONNECT_TIMEOUT)
        .operation_attempt_timeout(S3_OPERATION_ATTEMPT_TIMEOUT)
        .operation_timeout(S3_OPERATION_TIMEOUT)
        .build();
    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(config.s3_region.clone()))
        .timeout_config(timeout_config);
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
            failed_login_count INTEGER NOT NULL DEFAULT 0,
            locked_until TEXT,
            session_version INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS registration_devices (
            device_hash TEXT PRIMARY KEY,
            registration_count INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS auth_rate_limits (
            scope TEXT NOT NULL,
            key_hash TEXT NOT NULL,
            window_started_at INTEGER NOT NULL,
            attempts INTEGER NOT NULL,
            PRIMARY KEY (scope, key_hash)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS sync_snapshots (
            user_id TEXT PRIMARY KEY,
            payload TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS provider_templates (
            user_id TEXT NOT NULL,
            id TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, id)
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
    migrations::run_data_integrity_migrations(db).await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS users_single_admin ON users(role) WHERE role = 'admin'",
    )
    .execute(db)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS assets_user_sha256 ON assets(user_id, sha256)")
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM auth_rate_limits WHERE window_started_at < ?")
        .bind(Utc::now().timestamp().saturating_sub(7 * 86_400))
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
        ("failed_login_count", "INTEGER NOT NULL DEFAULT 0"),
        ("locked_until", "TEXT"),
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
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
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
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    cookies: Cookies,
    session: Session,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let client_ip = resolve_client_ip(&state.config, &headers, peer_addr);
    enforce_auth_rate_limit(
        &state.db,
        "register_ip",
        &hash_auth_identifier(&state.config.auth_secret, "ip", &client_ip.to_string()),
        state.config.register_ip_limit,
        state.config.register_window_seconds,
        "当前网络注册请求过于频繁，请稍后再试。",
    )
    .await?;
    let device_id = registration_device_id(&cookies, &state.config);
    let device_hash = hash_auth_identifier(&state.config.auth_secret, "device", &device_id);
    ensure_device_registration_available(
        &state.db,
        &device_hash,
        state.config.register_device_limit,
    )
    .await?;
    validate_registration(&payload)?;
    if username_exists(&state.db, &payload.username).await? {
        return Err(AppError::bad_request("用户名已存在"));
    }

    let password_hash = hash_password_with_limit(&state, payload.password.clone()).await?;
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

    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
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
    .execute(&mut *transaction)
    .await;

    if let Err(error) = result {
        if error.to_string().contains("UNIQUE") {
            return Err(AppError::bad_request("用户名已存在"));
        }
        return Err(AppError::internal(error));
    }
    reserve_device_registration(
        &mut transaction,
        &device_hash,
        state.config.register_device_limit,
        &now,
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;

    replace_session_identity(&session, &user.id, 0).await?;
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
    let session_version = sqlx::query_scalar::<_, i64>(
        "UPDATE users
         SET role = 'admin', status = 'approved', approved_at = ?, approved_by = ?,
             session_version = session_version + 1
         WHERE id = ? AND NOT EXISTS (SELECT 1 FROM users WHERE role = 'admin')
         RETURNING session_version",
    )
    .bind(&now)
    .bind(&user.id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?;
    let session_version = session_version.ok_or_else(|| {
        AppError::unauthorized("系统已存在管理员，不能再使用初始化口令升级账号。")
    })?;
    replace_session_identity(&session, &user.id, session_version).await?;

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
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    Json(payload): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let client_ip = resolve_client_ip(&state.config, &headers, peer_addr);
    enforce_auth_rate_limit(
        &state.db,
        "login_ip",
        &hash_auth_identifier(&state.config.auth_secret, "ip", &client_ip.to_string()),
        state.config.login_ip_limit,
        state.config.login_window_seconds,
        "登录尝试过于频繁，请稍后再试。",
    )
    .await?;
    validate_login_credentials(&payload)?;

    let row =
        sqlx::query("SELECT id, username, password_hash, role, status, created_at, locked_until, session_version FROM users WHERE username = ?")
            .bind(payload.username.trim())
            .fetch_optional(&state.db)
            .await
            .map_err(AppError::internal)?;

    let Some(row) = row else {
        let _ =
            verify_password_with_limit(&state, payload.password, state.dummy_password_hash.clone())
                .await?;
        return Err(AppError::unauthorized("用户名或密码错误"));
    };

    let user_id = row.get::<String, _>("id");
    let username = row.get::<String, _>("username");
    let password_hash = row.get::<String, _>("password_hash");
    let role = row.get::<String, _>("role");
    let status = row.get::<String, _>("status");
    let created_at = row.get::<String, _>("created_at");
    let session_version = row.get::<i64, _>("session_version");
    if let Some(retry_after) = active_lock_retry_seconds(row.get("locked_until")) {
        return Err(AppError::rate_limited(
            format!("账号已临时锁定，请在 {retry_after} 秒后重试。"),
            "account_locked",
            retry_after,
        ));
    }
    if !verify_password_with_limit(&state, payload.password, password_hash).await? {
        if let Some(retry_after) = record_failed_login(
            &state.db,
            &user_id,
            state.config.login_failure_limit,
            state.config.login_lock_seconds,
        )
        .await?
        {
            return Err(AppError::rate_limited(
                format!("密码连续输错次数过多，账号已锁定 {retry_after} 秒。"),
                "account_locked",
                retry_after,
            ));
        }
        return Err(AppError::unauthorized("用户名或密码错误"));
    }
    // 先完成密码校验再返回禁用状态，避免通过响应时延枚举已禁用账号。
    if status == "disabled" {
        return Err(AppError::unauthorized("账号已被禁用，请联系管理员。"));
    }

    let image_count = user_image_count(&state.db, &user_id).await?;
    let user = UserSummary {
        id: user_id,
        username,
        role,
        status,
        image_count,
        created_at,
    };
    sqlx::query(
        "UPDATE users SET last_login_at = ?, failed_login_count = 0, locked_until = NULL WHERE id = ?",
    )
        .bind(now_rfc3339())
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    replace_session_identity(&session, &user.id, session_version).await?;
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

    let current_password_hash = row.get::<String, _>("password_hash");
    if !verify_password_with_limit(&state, payload.old_password, current_password_hash).await? {
        return Err(AppError::unauthorized("当前密码错误"));
    }
    let password_hash = hash_password_with_limit(&state, payload.new_password).await?;
    let session_version = sqlx::query_scalar::<_, i64>(
        "UPDATE users
         SET password_hash = ?, password_updated_at = ?, failed_login_count = 0,
             locked_until = NULL, session_version = session_version + 1
         WHERE id = ?
         RETURNING session_version",
    )
    .bind(password_hash)
    .bind(now_rfc3339())
    .bind(&user.id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    replace_session_identity(&session, &user.id, session_version).await?;
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

    let _write_guard = user_data_write_lock(&state, &payload.user_id).lock().await;
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
    let _write_guard = user_data_write_lock(state, user_id).lock().await;
    let target_exists =
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(AppError::internal)?
            != 0;
    if !target_exists {
        return Err(AppError::not_found("用户不存在"));
    }
    let approved_at = if status == "approved" {
        Some(now_rfc3339())
    } else {
        None
    };
    let result = sqlx::query(
        "UPDATE users
         SET status = ?, approved_at = COALESCE(?, approved_at),
             approved_by = COALESCE(?, approved_by), session_version = session_version + 1
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

fn user_data_write_lock<'a>(state: &'a AppState, user_id: &str) -> &'a tokio::sync::Mutex<()> {
    // 同一用户稳定落在同一分片，串行化同步、配额预留与清理；不同用户仍可并行。
    let digest = Sha256::digest(user_id.as_bytes());
    let index = usize::from(digest[0]) % state.user_data_write_locks.len();
    &state.user_data_write_locks[index]
}

async fn revalidate_locked_approved_user(
    state: &AppState,
    session: &Session,
    expected_user_id: &str,
) -> Result<UserSummary, AppError> {
    let user = require_approved_user(state, session).await?;
    if user.id != expected_user_id {
        return Err(AppError::unauthorized(
            "登录状态已变更，请重新执行当前操作。",
        ));
    }
    Ok(user)
}

async fn sync_push(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<SyncPushRequest>,
) -> Result<Json<SyncPullResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    let normalized = normalize_envelope_assets(&state, &user.id, payload.envelope).await?;
    let stored =
        match sync_store::merge_snapshot_transactionally(&state.db, &user.id, &normalized.envelope)
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                error!("transactional sync merge failed: {error:#}");
                if let Err(rollback_error) = normalized.rollback(&state, &user.id).await {
                    error!(
                        "sync asset normalization rollback failed for user {}: {}",
                        user.id, rollback_error.message
                    );
                }
                return Err(AppError::internal_message("服务器内部错误"));
            }
        };
    let merged = stored.envelope;
    let updated_at = stored.updated_at;

    if let Err(error) = cleanup_tombstoned_assets(&state, &user.id, &merged).await {
        // 快照已经提交，清理失败交由下次同步重试，不能把成功提交伪装成失败响应。
        warn!(
            "post-commit tombstone cleanup failed for user {}: {}",
            user.id, error.message
        );
    }

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
            "SELECT
                (SELECT COUNT(*) FROM assets
                 WHERE user_id = ? AND object_key = ? AND id != ?) +
                (SELECT COUNT(*) FROM upload_tokens
                 WHERE user_id = ? AND object_key = ? AND asset_id != ? AND expires_at > ?)",
        )
        .bind(user_id)
        .bind(&object_key)
        .bind(asset_id)
        .bind(user_id)
        .bind(&object_key)
        .bind(asset_id)
        .bind(now_rfc3339())
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
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
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
    let user = current_user(&state, &session)
        .await?
        .filter(|user| user.status == "approved");
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
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    validate_template(
        &state,
        &payload.template,
        state.config.enforce_provider_host_whitelist,
    )?;

    let serialized = serde_json::to_string(&payload.template).map_err(AppError::internal)?;
    sqlx::query(
        "INSERT INTO provider_templates (id, user_id, payload, created_at, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(user_id, id) DO UPDATE SET
             payload = excluded.payload,
             updated_at = excluded.updated_at",
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

async fn check_asset_presence(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<AssetPresenceRequest>,
) -> Result<Json<AssetPresenceResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    if payload.asset_ids.len() > MAX_ASSET_PRESENCE_CHECKS {
        return Err(AppError::bad_request("单次图片完整性检查数量过多。"));
    }

    let rows = sqlx::query("SELECT id, object_key FROM assets WHERE user_id = ?")
        .bind(&user.id)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::internal)?;
    let indexed_objects = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("id"),
                row.get::<String, _>("object_key"),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut checked_objects = HashMap::<String, bool>::new();
    let mut missing_asset_ids = Vec::new();
    let mut stale_object_keys = BTreeSet::new();
    let mut seen_asset_ids = BTreeSet::new();

    for asset_id in payload.asset_ids {
        if !seen_asset_ids.insert(asset_id.clone()) {
            continue;
        }
        let Some(object_key) = indexed_objects.get(&asset_id) else {
            missing_asset_ids.push(asset_id);
            continue;
        };
        let exists = if let Some(exists) = checked_objects.get(object_key) {
            *exists
        } else {
            let exists = object_exists(&state, object_key).await?;
            checked_objects.insert(object_key.clone(), exists);
            exists
        };
        if !exists {
            stale_object_keys.insert(object_key.clone());
            missing_asset_ids.push(asset_id);
        }
    }

    for object_key in stale_object_keys {
        delete_asset_indexes_for_object(&state.db, &user.id, &object_key).await?;
    }
    Ok(Json(AssetPresenceResponse { missing_asset_ids }))
}

struct UploadReservation<'a> {
    user_id: &'a str,
    asset_id: &'a str,
    token: &'a str,
    object_key: &'a str,
    mime_type: &'a str,
    byte_len: u64,
    sha256: &'a str,
    expires_at: &'a str,
}

fn validate_upload_metadata(
    state: &AppState,
    payload: &UploadInitRequest,
) -> Result<(String, String), AppError> {
    if payload.byte_len == 0 {
        return Err(AppError::bad_request("不能上传空文件。"));
    }
    if payload.byte_len > state.config.max_upload_bytes {
        return Err(AppError::bad_request(format!(
            "单个图片资源不能超过 {} MiB。",
            state.config.max_upload_bytes / 1024 / 1024
        )));
    }
    let sha256 = payload.sha256.trim().to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::bad_request("图片资源 SHA-256 格式无效。"));
    }
    let mime_type = payload
        .mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        mime_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err(AppError::bad_request(
            "云端资源只支持 PNG、JPEG 或 WebP 图片。",
        ));
    }
    Ok((sha256, mime_type))
}

async fn reserve_upload_token(
    state: &AppState,
    reservation: UploadReservation<'_>,
) -> Result<(), AppError> {
    let mut connection = state.db.acquire().await.map_err(AppError::internal)?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(AppError::internal)?;
    let result = reserve_upload_token_on_connection(&mut connection, state, &reservation).await;
    match result {
        Ok(()) => match sqlx::query("COMMIT").execute(&mut *connection).await {
            Ok(_) => Ok(()),
            Err(error) => {
                // 手写事务提交失败时显式回滚，避免异常事务状态返回连接池。
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(AppError::internal(error))
            }
        },
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn reserve_upload_token_on_connection(
    connection: &mut SqliteConnection,
    state: &AppState,
    reservation: &UploadReservation<'_>,
) -> Result<(), AppError> {
    let existing_owner =
        sqlx::query_scalar::<_, Option<String>>("SELECT user_id FROM assets WHERE id = ?")
            .bind(reservation.asset_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(AppError::internal)?
            .flatten();
    if existing_owner
        .as_deref()
        .is_some_and(|owner| owner != reservation.user_id)
    {
        return Err(AppError::bad_request("图片资源 ID 与其他用户冲突。"));
    }

    let now = now_rfc3339();
    let pending = sqlx::query(
        "SELECT COALESCE(SUM(byte_len), 0) AS bytes, COUNT(*) AS count
         FROM upload_tokens WHERE user_id = ? AND expires_at > ?",
    )
    .bind(reservation.user_id)
    .bind(&now)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::internal)?;
    let pending_bytes = u64::try_from(pending.get::<i64, _>("bytes")).unwrap_or(u64::MAX);
    let pending_count = u64::try_from(pending.get::<i64, _>("count")).unwrap_or(u64::MAX);
    if state.config.user_pending_upload_count > 0
        && pending_count.saturating_add(1) > state.config.user_pending_upload_count
    {
        return Err(AppError::rate_limited(
            "当前账号等待完成的上传过多，请完成或稍后重试。",
            "pending_upload_limit",
            30,
        ));
    }
    if state.config.user_pending_upload_bytes > 0
        && pending_bytes.saturating_add(reservation.byte_len)
            > state.config.user_pending_upload_bytes
    {
        return Err(AppError::rate_limited(
            "当前账号等待完成的上传总大小过高，请完成或稍后重试。",
            "pending_upload_bytes_limit",
            30,
        ));
    }

    let reserved_bytes = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT SUM(byte_len) FROM (
            SELECT object_key, MAX(byte_len) AS byte_len FROM (
                SELECT object_key, byte_len FROM assets WHERE user_id = ?
                UNION ALL
                SELECT object_key, byte_len FROM upload_tokens
                 WHERE user_id = ? AND expires_at > ?
            ) GROUP BY object_key
         )",
    )
    .bind(reservation.user_id)
    .bind(reservation.user_id)
    .bind(&now)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::internal)?
    .and_then(|value| u64::try_from(value).ok())
    .unwrap_or(0);
    let existing_object_bytes = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(byte_len) FROM (
            SELECT byte_len FROM assets WHERE user_id = ? AND object_key = ?
            UNION ALL
            SELECT byte_len FROM upload_tokens
             WHERE user_id = ? AND object_key = ? AND expires_at > ?
         )",
    )
    .bind(reservation.user_id)
    .bind(reservation.object_key)
    .bind(reservation.user_id)
    .bind(reservation.object_key)
    .bind(&now)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::internal)?
    .and_then(|value| u64::try_from(value).ok())
    .unwrap_or(0);
    // 兼容旧版可能留下的低报索引：同一对象更新为实际大小时也必须补计差额。
    let additional_bytes = reservation.byte_len.saturating_sub(existing_object_bytes);
    let projected_bytes = reserved_bytes.saturating_add(additional_bytes);
    if state.config.user_asset_quota_bytes > 0
        && projected_bytes > state.config.user_asset_quota_bytes
    {
        return Err(AppError::bad_request(
            "云端图片配额已满；现有资源仍可读取或删除。",
        ));
    }

    let asset_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets WHERE user_id = ?")
        .bind(reservation.user_id)
        .fetch_one(&mut *connection)
        .await
        .map_err(AppError::internal)?;
    let is_existing_asset = existing_owner.as_deref() == Some(reservation.user_id);
    if state.config.user_asset_quota_count > 0
        && u64::try_from(asset_count)
            .unwrap_or(u64::MAX)
            .saturating_add(u64::from(!is_existing_asset))
            > state.config.user_asset_quota_count
    {
        return Err(AppError::bad_request(
            "云端图片数量配额已满；现有资源仍可读取或删除。",
        ));
    }

    sqlx::query(
        "INSERT INTO upload_tokens
         (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(reservation.token)
    .bind(reservation.asset_id)
    .bind(reservation.user_id)
    .bind(reservation.object_key)
    .bind(reservation.mime_type)
    .bind(
        i64::try_from(reservation.byte_len)
            .map_err(|_| AppError::bad_request("上传文件大小超过数据库可记录范围。"))?,
    )
    .bind(reservation.sha256)
    .bind(reservation.expires_at)
    .execute(&mut *connection)
    .await
    .map_err(AppError::internal)?;
    Ok(())
}

async fn ensure_user_asset_capacity(
    state: &AppState,
    user_id: &str,
    asset_id: &str,
    object_key: &str,
    byte_len: u64,
) -> Result<(), AppError> {
    let now = now_rfc3339();
    let usage = sqlx::query(
        "SELECT
            COALESCE((
                SELECT SUM(byte_len) FROM (
                    SELECT object_key, MAX(byte_len) AS byte_len FROM (
                        SELECT object_key, byte_len FROM assets WHERE user_id = ?
                        UNION ALL
                        SELECT object_key, byte_len FROM upload_tokens
                         WHERE user_id = ? AND expires_at > ?
                    ) GROUP BY object_key
                )
            ), 0) AS stored_bytes,
            COALESCE((
                SELECT MAX(byte_len) FROM (
                    SELECT byte_len FROM assets WHERE user_id = ? AND object_key = ?
                    UNION ALL
                    SELECT byte_len FROM upload_tokens
                     WHERE user_id = ? AND object_key = ? AND expires_at > ?
                )
            ), 0) AS object_bytes,
            EXISTS(SELECT 1 FROM assets WHERE user_id = ? AND id = ?) AS asset_exists,
            (SELECT COUNT(*) FROM assets WHERE user_id = ?) AS asset_count",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(&now)
    .bind(user_id)
    .bind(object_key)
    .bind(user_id)
    .bind(object_key)
    .bind(&now)
    .bind(user_id)
    .bind(asset_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    let stored_bytes = u64::try_from(usage.get::<i64, _>("stored_bytes")).unwrap_or(u64::MAX);
    let existing_object_bytes = u64::try_from(usage.get::<i64, _>("object_bytes")).unwrap_or(0);
    let additional_bytes = byte_len.saturating_sub(existing_object_bytes);
    if additional_bytes > 0
        && state.config.user_asset_quota_bytes > 0
        && stored_bytes.saturating_add(additional_bytes) > state.config.user_asset_quota_bytes
    {
        return Err(AppError::bad_request(
            "云端图片配额已满；现有资源仍可读取或删除。",
        ));
    }

    let asset_exists = usage.get::<i64, _>("asset_exists") != 0;
    if !asset_exists && state.config.user_asset_quota_count > 0 {
        let asset_count = usage.get::<i64, _>("asset_count");
        if u64::try_from(asset_count)
            .unwrap_or(u64::MAX)
            .saturating_add(1)
            > state.config.user_asset_quota_count
        {
            return Err(AppError::bad_request(
                "云端图片数量配额已满；现有资源仍可读取或删除。",
            ));
        }
    }
    Ok(())
}

async fn upload_init(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<UploadInitRequest>,
) -> Result<Json<UploadInitResponse>, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state).await?;
    let (sha256, mime_type) = validate_upload_metadata(&state, &payload)?;
    let asset_id = payload.asset_id.unwrap_or_else(new_id);
    if uuid::Uuid::parse_str(&asset_id).is_err() {
        return Err(AppError::bad_request("图片资源 ID 格式无效。"));
    }
    let token = random_token();
    let object_key = format!("users/{}/assets/{sha256}.bin", user.id);
    let expires_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();
    reserve_upload_token(
        &state,
        UploadReservation {
            user_id: &user.id,
            asset_id: &asset_id,
            token: &token,
            object_key: &object_key,
            mime_type: &mime_type,
            byte_len: payload.byte_len,
            sha256: &sha256,
            expires_at: &expires_at,
        },
    )
    .await?;

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
    request: Request,
) -> Result<StatusCode, AppError> {
    let user = require_approved_user(&state, &session).await?;
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state).await?;
    let row = lease_upload_token(&state, &user.id, &token).await?;

    let expected_len = usize::try_from(row.get::<i64, _>("byte_len"))
        .map_err(|_| AppError::bad_request("上传大小记录无效。"))?;
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|content_length| content_length != expected_len)
    {
        return Err(AppError::bad_request("上传大小与预期不一致"));
    }
    let _memory_permit =
        acquire_shared_byte_budget(&state, u64::try_from(expected_len).unwrap_or(u64::MAX)).await?;
    // Request 提取器不会预先缓冲正文；鉴权、令牌租约和预算确认后才有界读取。
    let body = to_bytes(request.into_body(), expected_len)
        .await
        .map_err(|error| AppError::bad_request(format!("上传正文读取失败：{error}")))?;
    if expected_len != body.len() {
        return Err(AppError::bad_request("上传大小与预期不一致"));
    }

    let hash = hex_sha256(&body);
    let expected_hash = row.get::<String, _>("sha256");
    if hash != expected_hash {
        return Err(AppError::bad_request("文件哈希校验失败"));
    }
    let detected_mime = detect_image_mime(&body)
        .ok_or_else(|| AppError::bad_request("只允许上传 PNG、JPEG 或 WebP 图片。"))?;
    if detected_mime != row.get::<String, _>("mime_type") {
        return Err(AppError::bad_request(
            "上传文件内容与声明的图片类型不一致。",
        ));
    }

    put_object(
        &state,
        row.get::<String, _>("object_key").as_str(),
        row.get::<String, _>("mime_type").as_str(),
        body,
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
    let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
    let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
    ensure_object_storage_ready(&state)?;
    cleanup_expired_upload_tokens(&state).await?;
    let row = lease_upload_token(&state, &user.id, &payload.upload_token).await?;

    let object_key = row.get::<String, _>("object_key");
    let expected_len = usize::try_from(row.get::<i64, _>("byte_len"))
        .map_err(|_| AppError::bad_request("上传大小记录无效。"))?;
    let _memory_permit =
        acquire_shared_byte_budget(&state, u64::try_from(expected_len).unwrap_or(u64::MAX)).await?;
    let object_bytes = get_object_bytes(&state, &object_key, state.config.max_upload_bytes)
        .await
        .map_err(|_| AppError::bad_request("上传原文件不存在，请重新上传。"))?;
    if object_bytes.len() != expected_len
        || hex_sha256(&object_bytes) != row.get::<String, _>("sha256")
    {
        return Err(AppError::bad_request("上传原文件的大小或哈希校验失败。"));
    }
    let detected_mime = detect_image_mime(&object_bytes)
        .ok_or_else(|| AppError::bad_request("上传原文件不是受支持的图片。"))?;
    if detected_mime != row.get::<String, _>("mime_type") {
        return Err(AppError::bad_request("上传原文件的图片类型校验失败。"));
    }

    let created_at = now_rfc3339();
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    let previous_object_key = sqlx::query_scalar::<_, String>(
        "SELECT object_key FROM assets WHERE id = ? AND user_id = ?",
    )
    .bind(row.get::<String, _>("asset_id"))
    .bind(&user.id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(AppError::internal)?;
    let result = sqlx::query(
        "INSERT INTO assets (id, user_id, object_key, mime_type, sha256, byte_len, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             object_key = excluded.object_key,
             mime_type = excluded.mime_type,
             sha256 = excluded.sha256,
             byte_len = excluded.byte_len
         WHERE assets.user_id = excluded.user_id",
    )
    .bind(row.get::<String, _>("asset_id"))
    .bind(row.get::<Option<String>, _>("user_id"))
    .bind(row.get::<String, _>("object_key"))
    .bind(row.get::<String, _>("mime_type"))
    .bind(row.get::<String, _>("sha256"))
    .bind(row.get::<i64, _>("byte_len"))
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::bad_request("图片资源 ID 与其他用户冲突。"));
    }
    sqlx::query("DELETE FROM upload_tokens WHERE token = ?")
        .bind(&payload.upload_token)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    transaction.commit().await.map_err(AppError::internal)?;
    if let Some(previous_object_key) = previous_object_key
        && previous_object_key != object_key
        && let Err(error) =
            delete_object_if_unreferenced(&state, &user.id, &previous_object_key).await
    {
        // 新资产和凭证状态已经提交，旧对象清理失败不得改写已提交响应。
        warn!(
            "post-commit previous object cleanup failed for user {}: {}",
            user.id, error.message
        );
    }

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
        remote_object_key: Some(object_key),
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
        .bind(&asset_id)
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

    let mut object_key = row.get::<String, _>("object_key");
    let mut mime_type = row.get::<String, _>("mime_type");
    if !object_exists(&state, &object_key).await? {
        let _write_guard = user_data_write_lock(&state, &user.id).lock().await;
        let user = revalidate_locked_approved_user(&state, &session, &user.id).await?;
        let current =
            sqlx::query("SELECT object_key, mime_type FROM assets WHERE id = ? AND user_id = ?")
                .bind(&asset_id)
                .bind(&user.id)
                .fetch_optional(&state.db)
                .await
                .map_err(AppError::internal)?;
        let Some(current) = current else {
            return Err(AppError::not_found("资源不存在"));
        };
        object_key = current.get("object_key");
        mime_type = current.get("mime_type");
        if !object_exists(&state, &object_key).await? {
            delete_asset_indexes_for_object(&state.db, &user.id, &object_key).await?;
            return Err(AppError::not_found(
                "资源原文件不存在，请重新同步本地原图。",
            ));
        }
    }
    // 旧索引可能低报大小，因此下载按单文件安全上限预留，并让许可随响应正文一起释放。
    let response_permit = acquire_shared_byte_budget(&state, state.config.max_upload_bytes).await?;
    let bytes = get_object_bytes(&state, &object_key, state.config.max_upload_bytes).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime_type).map_err(AppError::internal)?,
    );
    let mut response = (StatusCode::OK, headers, bytes).into_response();
    response.extensions_mut().insert(ResponseMemoryPermit {
        _permit: Arc::new(response_permit),
    });
    Ok(response)
}

async fn fetch_image_via_proxy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    Json(payload): Json<FetchImageRequest>,
) -> Result<Response, AppError> {
    let approved_user = current_user(&state, &session)
        .await?
        .filter(|user| user.status == "approved");
    let _guest_permit = if approved_user.is_none() {
        if !state.config.enable_guest_proxy {
            return Err(AppError::unauthorized("当前部署已关闭游客代理。"));
        }
        Some(acquire_guest_proxy_permit(
            &state,
            resolve_client_ip(&state.config, &headers, peer_addr),
            GuestProxyOperation::ImageFetch,
        )?)
    } else {
        None
    };
    // 下载字节与 Base64 JSON 会短暂并存，按三倍文件上限预留并持有到响应发送结束。
    let response_permit =
        acquire_shared_byte_budget(&state, (MAX_REMOTE_IMAGE_BYTES as u64).saturating_mul(3))
            .await?;
    let (mime_type, bytes) = fetch_remote_image_bytes(&state, &payload.url).await?;
    let mut response = Json(FetchImageResponse {
        mime_type,
        body_base64: BASE64.encode(bytes),
    })
    .into_response();
    response.extensions_mut().insert(ResponseMemoryPermit {
        _permit: Arc::new(response_permit),
    });
    Ok(response)
}

fn acquire_guest_proxy_permit(
    state: &AppState,
    client_ip: IpAddr,
    operation: GuestProxyOperation,
) -> Result<GuestProxyPermit, AppError> {
    let (concurrency, requests, scope_name) = match operation {
        GuestProxyOperation::Generation => (
            state.config.guest_generation_concurrency,
            state.config.guest_generation_rate_limit,
            "生图",
        ),
        GuestProxyOperation::ImageFetch => (
            state.config.guest_image_concurrency,
            state.config.guest_image_rate_limit,
            "图片下载",
        ),
    };
    state
        .guest_proxy_limits
        .acquire(
            client_ip,
            operation,
            concurrency,
            requests,
            StdDuration::from_secs(state.config.guest_rate_window_seconds),
        )
        .map_err(|rejection| match rejection {
            GuestLimitRejection::Concurrent => AppError::rate_limited(
                format!("当前 IP 的游客{scope_name}并发数已达上限。"),
                "guest_proxy_concurrency_limit",
                2,
            ),
            GuestLimitRejection::RateLimited {
                retry_after_seconds,
            } => AppError::rate_limited(
                format!("当前 IP 的游客{scope_name}请求过于频繁。"),
                "guest_proxy_rate_limit",
                retry_after_seconds,
            ),
        })
}

fn generation_temp_permits(state: &AppState, headers: &HeaderMap) -> Result<u32, AppError> {
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| AppError::bad_request("生成请求的 Content-Length 无效。"))
        })
        .transpose()?;
    if content_length.is_some_and(|length| length > GENERATION_BODY_LIMIT as u64) {
        return Err(AppError::bad_request("生成请求超过 192 MiB 安全上限。"));
    }

    let request_bytes = content_length.unwrap_or(GENERATION_BODY_LIMIT as u64);
    // 排队参考图会流式落盘；按请求体两倍计费，为 multipart 边界和未知长度保留余量，
    // 并限制所有等待任务占用的临时磁盘总量。
    let weighted_bytes = request_bytes
        .saturating_mul(2)
        .saturating_add(16 * 1024 * 1024);
    Ok(bytes_to_budget_permits(
        weighted_bytes,
        state.config.proxy_memory_budget_mib,
    ))
}

fn estimate_generation_memory_permits(
    state: &AppState,
    request: &mew_image_shared::GenerationRequest,
) -> u32 {
    let reference_bytes = request
        .reference_assets
        .iter()
        .map(|asset| asset.byte_len)
        .fold(0_u64, u64::saturating_add);
    let output_bytes = u64::from(request.width)
        .saturating_mul(u64::from(request.height))
        .saturating_mul(4)
        .saturating_mul(u64::from(request.count));
    // 参考图在 data URL、JSON/multipart 和解码缓冲之间会短暂重复；结果也会同时
    // 存在于上游响应、提取结果与序列化轮询正文中，预算需覆盖峰值而非文件净大小。
    bytes_to_budget_permits(
        reference_bytes
            .saturating_mul(3)
            .saturating_add(output_bytes.saturating_mul(2))
            .saturating_add(32 * 1024 * 1024),
        state.config.proxy_memory_budget_mib,
    )
}

fn bytes_to_budget_permits(bytes: u64, max_budget_mib: usize) -> u32 {
    let permits = bytes.saturating_add(1024 * 1024 - 1) / (1024 * 1024);
    permits
        .max(1)
        .min(max_budget_mib.max(1) as u64)
        .min(u64::from(u32::MAX)) as u32
}

async fn acquire_shared_byte_budget(
    state: &AppState,
    bytes: u64,
) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    let permits = bytes_to_budget_permits(bytes, state.config.proxy_memory_budget_mib);
    state
        .generation_memory_budget
        .clone()
        .acquire_many_owned(permits)
        .await
        .map_err(AppError::internal)
}

async fn generate_via_proxy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    multipart: Multipart,
) -> Result<(StatusCode, Json<ProxyGenerationJobAccepted>), AppError> {
    let approved_user = current_user(&state, &session)
        .await?
        .filter(|user| user.status == "approved");
    if approved_user.is_none() && !state.config.enable_guest_proxy {
        return Err(AppError::unauthorized(
            "当前部署已关闭游客代理，请登录后再试。",
        ));
    }
    let guest_permit = approved_user.is_none().then(|| {
        acquire_guest_proxy_permit(
            &state,
            resolve_client_ip(&state.config, &headers, peer_addr),
            GuestProxyOperation::Generation,
        )
    });
    let guest_permit = guest_permit.transpose()?;
    let job_slot = state
        .generation_job_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::rate_limited(
                "当前代理生成任务较多，请稍后重试。",
                "proxy_generation_queue_full",
                5,
            )
        })?;
    let temp_permits = generation_temp_permits(&state, &headers)?;
    let temp_budget_permit = state
        .generation_temp_budget
        .clone()
        .acquire_many_owned(temp_permits)
        .await
        .map_err(AppError::internal)?;
    let payload = parse_generate_multipart(multipart).await?;
    validate_generate_request(&state, approved_user.as_ref(), &payload.payload)?;
    let memory_permits = estimate_generation_memory_permits(&state, &payload.payload.request);

    cleanup_proxy_generation_jobs(&state).await;
    let job_id = format!("{}{}", new_id(), new_id());
    state.generation_jobs.lock().await.insert(
        job_id.clone(),
        ProxyGenerationJob {
            state: ProxyGenerationJobState::Queued,
            updated_at: Instant::now(),
            abort_handle: None,
            memory_permit: None,
        },
    );
    let task_state = state.clone();
    let task = tokio::spawn(run_proxy_generation_job(
        state,
        job_id.clone(),
        payload,
        job_slot,
        temp_budget_permit,
        memory_permits,
        guest_permit,
    ));
    if let Some(job) = task_state.generation_jobs.lock().await.get_mut(&job_id) {
        job.abort_handle = Some(task.abort_handle());
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(ProxyGenerationJobAccepted { job_id }),
    ))
}

async fn run_proxy_generation_job(
    state: Arc<AppState>,
    job_id: String,
    mut payload: ParsedGeneratePayload,
    job_slot: tokio::sync::OwnedSemaphorePermit,
    temp_budget_permit: tokio::sync::OwnedSemaphorePermit,
    memory_permits: u32,
    guest_permit: Option<GuestProxyPermit>,
) {
    let memory_permit = match state
        .generation_memory_budget
        .clone()
        .acquire_many_owned(memory_permits)
        .await
    {
        Ok(permit) => permit,
        Err(error) => {
            update_proxy_generation_job(
                &state,
                &job_id,
                ProxyGenerationJobState::Failed(format!("生成内存预算不可用：{error}")),
            )
            .await;
            return;
        }
    };
    // 参考图即将载入受执行预算约束的内存，此时再释放排队临时磁盘预算。
    drop(temp_budget_permit);
    let result = tokio::time::timeout(PROXY_GENERATION_JOB_TIMEOUT, async {
        update_proxy_generation_job(&state, &job_id, ProxyGenerationJobState::Running).await;
        hydrate_temporary_reference_files(&mut payload).await?;
        execute_proxy_generation(&state, &payload.payload).await
    })
    .await;

    let final_state = {
        // 结果完成序列化并释放大型临时对象后，再尝试归还 glibc 堆内存。
        let _memory_trim_guard = GenerationMemoryTrimGuard;
        match result {
            Ok(Ok(result)) => serialize_proxy_generation_result(result)
                .and_then(|body| {
                    let reserved_bytes = memory_permits as usize * 1024 * 1024;
                    if body.len() > reserved_bytes {
                        return Err(format!(
                            "上游结果超过本任务的 {} MiB 内存预算，已停止缓存。",
                            memory_permits
                        ));
                    }
                    Ok(ProxyGenerationJobState::Succeeded(body))
                })
                .unwrap_or_else(ProxyGenerationJobState::Failed),
            Ok(Err(error)) => ProxyGenerationJobState::Failed(error.message),
            Err(_) => ProxyGenerationJobState::Failed(
                "代理生成等待超过 30 分钟，任务已停止，请稍后重试。".into(),
            ),
        }
    };
    let keep_memory_permit = matches!(final_state, ProxyGenerationJobState::Succeeded(_));
    complete_proxy_generation_job(
        &state,
        &job_id,
        final_state,
        keep_memory_permit.then_some(memory_permit),
    )
    .await;
    drop(job_slot);
    drop(guest_permit);

    // 即使浏览器关闭后不再轮询，也会按时释放未读取结果。
    tokio::time::sleep(PROXY_GENERATION_RESULT_TTL).await;
    cleanup_proxy_generation_jobs(&state).await;
}

async fn execute_proxy_generation(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<GenerationResult, AppError> {
    let started_at = Utc::now();
    let response_json = match payload.template.kind {
        ProviderKind::OpenAiImage => invoke_openai_image(state, payload).await?,
        ProviderKind::NanoBanana => invoke_nano_banana(state, payload).await?,
        ProviderKind::OpenAiCompatible => invoke_openai_compatible_image(state, payload).await?,
        ProviderKind::CustomHttp => invoke_custom_http(state, payload).await?,
    };

    let duration_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;
    let result = extract_generation_result(
        &payload.template,
        payload.config.output_format.as_deref(),
        &payload.request,
        response_json,
        duration_ms,
    );
    hydrate_proxy_result_images(state, result).await
}

async fn update_proxy_generation_job(
    state: &AppState,
    job_id: &str,
    job_state: ProxyGenerationJobState,
) {
    if let Some(job) = state.generation_jobs.lock().await.get_mut(job_id) {
        job.state = job_state;
        job.updated_at = Instant::now();
    }
}

async fn complete_proxy_generation_job(
    state: &AppState,
    job_id: &str,
    job_state: ProxyGenerationJobState,
    memory_permit: Option<tokio::sync::OwnedSemaphorePermit>,
) {
    if let Some(job) = state.generation_jobs.lock().await.get_mut(job_id) {
        job.state = job_state;
        job.memory_permit = memory_permit;
        job.updated_at = Instant::now();
    }
}

async fn get_proxy_generation_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Response, AppError> {
    let mut jobs = state.generation_jobs.lock().await;
    cleanup_proxy_generation_job_entries(&mut jobs, Instant::now());
    let Some(job) = jobs.get(&job_id) else {
        return Err(AppError::not_found(
            "代理生成任务不存在或结果已过期，请重新生成。",
        ));
    };

    Ok(match &job.state {
        ProxyGenerationJobState::Queued => proxy_generation_job_response(
            StatusCode::ACCEPTED,
            ProxyGenerationJobStatus::Queued,
            None,
        ),
        ProxyGenerationJobState::Running => proxy_generation_job_response(
            StatusCode::ACCEPTED,
            ProxyGenerationJobStatus::Running,
            None,
        ),
        ProxyGenerationJobState::Succeeded(body) => {
            serialized_proxy_generation_job_response(body.clone())
        }
        ProxyGenerationJobState::Failed(error) => proxy_generation_job_response(
            StatusCode::OK,
            ProxyGenerationJobStatus::Failed,
            Some(error.clone()),
        ),
    })
}

async fn cancel_proxy_generation_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> StatusCode {
    let job = state.generation_jobs.lock().await.remove(&job_id);
    if let Some(abort_handle) = job.and_then(|job| job.abort_handle) {
        abort_handle.abort();
    }
    StatusCode::NO_CONTENT
}

fn serialize_proxy_generation_result(result: GenerationResult) -> Result<Bytes, String> {
    serde_json::to_vec(&ProxyGenerationJobResponse {
        status: ProxyGenerationJobStatus::Succeeded,
        result: Some(result),
        error: None,
    })
    .map(Bytes::from)
    .map_err(|error| format!("代理生成结果序列化失败：{error}"))
}

fn proxy_generation_job_response(
    http_status: StatusCode,
    status: ProxyGenerationJobStatus,
    error: Option<String>,
) -> Response {
    let mut response = (
        http_status,
        Json(ProxyGenerationJobResponse {
            status,
            result: None,
            error,
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn serialized_proxy_generation_job_response(body: Bytes) -> Response {
    let mut response = body.into_response();
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn cleanup_proxy_generation_jobs(state: &AppState) {
    let mut jobs = state.generation_jobs.lock().await;
    cleanup_proxy_generation_job_entries(&mut jobs, Instant::now());
}

fn cleanup_proxy_generation_job_entries(
    jobs: &mut HashMap<String, ProxyGenerationJob>,
    now: Instant,
) {
    jobs.retain(|_, job| {
        !job.state.is_terminal()
            || now.saturating_duration_since(job.updated_at) < PROXY_GENERATION_RESULT_TTL
    });
    if jobs.len() <= MAX_STORED_PROXY_GENERATION_JOBS {
        return;
    }

    let mut completed_jobs = jobs
        .iter()
        .filter(|(_, job)| job.state.is_terminal())
        .map(|(job_id, job)| (job_id.clone(), job.updated_at))
        .collect::<Vec<_>>();
    completed_jobs.sort_by_key(|(_, updated_at)| *updated_at);
    let remove_count = jobs.len().saturating_sub(MAX_STORED_PROXY_GENERATION_JOBS);
    for (job_id, _) in completed_jobs.into_iter().take(remove_count) {
        jobs.remove(&job_id);
    }
}

async fn parse_generate_multipart(
    mut multipart: Multipart,
) -> Result<ParsedGeneratePayload, AppError> {
    let mut payload = None;
    let mut reference_assets_meta = None;
    let mut reference_assets_files = Vec::new();
    let mut reference_total_bytes = 0usize;

    while let Some(field) = multipart.next_field().await.map_err(AppError::internal)? {
        let name = field.name().unwrap_or_default().to_string();
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        match name.as_str() {
            "payload" => {
                if payload.is_some() {
                    return Err(AppError::bad_request("生成请求主体不能重复。"));
                }
                let text = read_multipart_text_limited(
                    field,
                    MAX_GENERATION_METADATA_BYTES,
                    "生成请求主体",
                )
                .await?;
                payload = Some(
                    serde_json::from_str::<GenerateViaProxyRequest>(&text).map_err(|error| {
                        AppError::bad_request(format!("生成请求解析失败：{error}"))
                    })?,
                );
            }
            "reference_assets_meta" => {
                if reference_assets_meta.is_some() {
                    return Err(AppError::bad_request("参考图元数据不能重复。"));
                }
                reference_assets_meta = Some(
                    read_multipart_text_limited(
                        field,
                        MAX_GENERATION_METADATA_BYTES,
                        "参考图元数据",
                    )
                    .await?,
                );
            }
            "reference_asset_files" => {
                if reference_assets_files.len() >= MAX_GENERATION_REFERENCE_COUNT {
                    return Err(AppError::bad_request(format!(
                        "参考图最多允许 {MAX_GENERATION_REFERENCE_COUNT} 张。"
                    )));
                }
                if !content_type.to_ascii_lowercase().starts_with("image/") {
                    return Err(AppError::bad_request("参考图文件类型无效。"));
                }
                let temporary_file = write_multipart_field_to_temp_file(
                    field,
                    MAX_GENERATION_REFERENCE_FILE_BYTES,
                    "单张参考图",
                    content_type,
                )
                .await?;
                reference_total_bytes = reference_total_bytes
                    .checked_add(temporary_file.byte_len as usize)
                    .ok_or_else(|| AppError::bad_request("参考图总大小溢出。"))?;
                if reference_total_bytes > MAX_GENERATION_REFERENCE_TOTAL_BYTES {
                    return Err(AppError::bad_request(format!(
                        "参考图总大小不能超过 {} MiB。",
                        MAX_GENERATION_REFERENCE_TOTAL_BYTES / 1024 / 1024
                    )));
                }
                reference_assets_files.push(temporary_file);
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
    if reference_assets_meta.len() > MAX_GENERATION_REFERENCE_COUNT {
        return Err(AppError::bad_request(format!(
            "参考图最多允许 {MAX_GENERATION_REFERENCE_COUNT} 张。"
        )));
    }
    if reference_assets_meta.len() != reference_assets_files.len() {
        return Err(AppError::bad_request("参考图文件数量与元数据数量不一致"));
    }

    let mut reference_assets = Vec::with_capacity(reference_assets_meta.len());
    for (asset, temporary_file) in reference_assets_meta
        .into_iter()
        .zip(reference_assets_files.iter())
    {
        if asset.sha256 != temporary_file.sha256 {
            return Err(AppError::bad_request(format!(
                "参考图 `{}` 的哈希校验失败。",
                asset.id
            )));
        }
        reference_assets.push(ImageAssetRef {
            id: asset.id,
            sha256: asset.sha256,
            mime_type: temporary_file.mime_type.clone(),
            byte_len: temporary_file.byte_len,
            width: asset.width,
            height: asset.height,
            created_at: asset.created_at,
            updated_at: asset.updated_at,
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: asset.source_task_id,
            metadata: asset.metadata,
        });
    }

    payload.request.reference_assets = reference_assets;
    Ok(ParsedGeneratePayload {
        payload,
        reference_files: reference_assets_files,
    })
}

async fn read_multipart_text_limited(
    field: Field<'_>,
    max_bytes: usize,
    label: &str,
) -> Result<String, AppError> {
    let bytes = read_multipart_field_limited(field, max_bytes, label).await?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::bad_request(format!("{label}必须使用 UTF-8 编码。")))
}

async fn read_multipart_field_limited(
    mut field: Field<'_>,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(AppError::internal)? {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| AppError::bad_request(format!("{label}大小溢出。")))?;
        if next_len > max_bytes {
            return Err(AppError::bad_request(format!(
                "{label}不能超过 {} MiB。",
                max_bytes / 1024 / 1024
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn write_multipart_field_to_temp_file(
    mut field: Field<'_>,
    max_bytes: usize,
    label: &str,
    mime_type: String,
) -> Result<TemporaryReferenceFile, AppError> {
    tokio::fs::create_dir_all(proxy_temp_dir())
        .await
        .map_err(AppError::internal)?;
    let path = proxy_temp_dir().join(format!("reference-{}.part", new_id()));
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
        .map_err(AppError::internal)?;
    let mut temporary_file = TemporaryReferenceFile {
        path,
        mime_type,
        byte_len: 0,
        sha256: String::new(),
    };
    let mut hasher = Sha256::new();

    while let Some(chunk) = field.chunk().await.map_err(AppError::internal)? {
        let next_len = temporary_file
            .byte_len
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| AppError::bad_request(format!("{label}大小溢出。")))?;
        if next_len > max_bytes as u64 {
            return Err(AppError::bad_request(format!(
                "{label}不能超过 {} MiB。",
                max_bytes / 1024 / 1024
            )));
        }
        output.write_all(&chunk).await.map_err(AppError::internal)?;
        hasher.update(&chunk);
        temporary_file.byte_len = next_len;
    }
    output.flush().await.map_err(AppError::internal)?;
    temporary_file.sha256 = format!("{:x}", hasher.finalize());
    Ok(temporary_file)
}

async fn hydrate_temporary_reference_files(
    parsed: &mut ParsedGeneratePayload,
) -> Result<(), AppError> {
    if parsed.payload.request.reference_assets.len() != parsed.reference_files.len() {
        return Err(AppError::internal_message(
            "代理生成参考图快照与临时文件数量不一致。",
        ));
    }

    for (asset, temporary_file) in parsed
        .payload
        .request
        .reference_assets
        .iter_mut()
        .zip(&parsed.reference_files)
    {
        asset.data_url = Some(load_temporary_reference_data_url(asset, temporary_file).await?);
    }
    parsed.reference_files.clear();
    Ok(())
}

async fn load_temporary_reference_data_url(
    asset: &ImageAssetRef,
    temporary_file: &TemporaryReferenceFile,
) -> Result<String, AppError> {
    let bytes = tokio::fs::read(&temporary_file.path)
        .await
        .map_err(|error| {
            AppError::internal_message(format!("读取代理参考图临时文件失败：{error}"))
        })?;
    if bytes.len() as u64 != temporary_file.byte_len || hex_sha256(&bytes) != asset.sha256 {
        return Err(AppError::bad_request(format!(
            "参考图 `{}` 的临时文件校验失败。",
            asset.id
        )));
    }
    Ok(format!(
        "data:{};base64,{}",
        temporary_file.mime_type,
        BASE64.encode(bytes)
    ))
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
    let session_version = session
        .get::<i64>("session_version")
        .await
        .map_err(AppError::internal)?;
    let row = sqlx::query(
        "SELECT id, username, role, status, created_at, session_version
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?;

    if let Some(row) = row {
        let current_session_version = row.get::<i64, _>("session_version");
        if session_version != Some(current_session_version) {
            session.delete().await.map_err(AppError::internal)?;
            return Ok(None);
        }
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
        session.delete().await.map_err(AppError::internal)?;
        Ok(None)
    }
}

async fn replace_session_identity(
    session: &Session,
    user_id: &str,
    session_version: i64,
) -> Result<(), AppError> {
    session.clear().await;
    session.cycle_id().await.map_err(AppError::internal)?;
    session
        .insert("user_id", user_id)
        .await
        .map_err(AppError::internal)?;
    session
        .insert("session_version", session_version)
        .await
        .map_err(AppError::internal)?;
    Ok(())
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

    let mut envelope = match row {
        Some(row) => {
            serde_json::from_str(&row.get::<String, _>("payload")).map_err(AppError::internal)?
        }
        None => SyncEnvelope::default(),
    };
    strip_successful_task_payloads(&mut envelope.tasks);
    Ok(envelope)
}

#[derive(Debug, Clone)]
struct StoredAssetIndex {
    id: String,
    object_key: String,
    mime_type: String,
    sha256: String,
    byte_len: i64,
    created_at: String,
}

#[derive(Debug)]
struct AssetIndexMutation {
    asset_id: String,
    previous: Option<StoredAssetIndex>,
}

#[derive(Debug, Default)]
struct AssetNormalizationJournal {
    index_mutations: Vec<AssetIndexMutation>,
    new_object_keys: Vec<String>,
}

impl AssetNormalizationJournal {
    fn record_index_mutation(&mut self, asset_id: String, previous: Option<StoredAssetIndex>) {
        self.index_mutations
            .push(AssetIndexMutation { asset_id, previous });
    }

    fn record_new_object(&mut self, object_key: String) {
        if !self.new_object_keys.contains(&object_key) {
            self.new_object_keys.push(object_key);
        }
    }

    async fn rollback(self, state: &AppState, user_id: &str) -> Result<(), AppError> {
        let mut first_error =
            rollback_asset_index_mutations(&state.db, user_id, self.index_mutations)
                .await
                .err();
        for object_key in self.new_object_keys.into_iter().rev() {
            if let Err(error) = delete_object_if_unreferenced(state, user_id, &object_key).await {
                warn!(
                    "failed to remove rolled-back sync object {object_key}: {}",
                    error.message
                );
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[derive(Debug)]
struct NormalizedSyncEnvelope {
    envelope: SyncEnvelope,
    journal: AssetNormalizationJournal,
}

impl NormalizedSyncEnvelope {
    async fn rollback(self, state: &AppState, user_id: &str) -> Result<(), AppError> {
        self.journal.rollback(state, user_id).await
    }
}

async fn normalize_envelope_assets(
    state: &AppState,
    user_id: &str,
    envelope: SyncEnvelope,
) -> Result<NormalizedSyncEnvelope, AppError> {
    if envelope.assets.len() > MAX_SYNC_ASSETS {
        return Err(AppError::bad_request(format!(
            "单次同步最多包含 {MAX_SYNC_ASSETS} 个图片资源。"
        )));
    }
    let mut journal = AssetNormalizationJournal::default();
    match normalize_envelope_assets_inner(state, user_id, envelope, &mut journal).await {
        Ok(envelope) => Ok(NormalizedSyncEnvelope { envelope, journal }),
        Err(error) => {
            if let Err(rollback_error) = journal.rollback(state, user_id).await {
                error!(
                    "partial sync asset normalization rollback failed for user {}: {}",
                    user_id, rollback_error.message
                );
            }
            Err(error)
        }
    }
}

async fn normalize_envelope_assets_inner(
    state: &AppState,
    user_id: &str,
    mut envelope: SyncEnvelope,
    journal: &mut AssetNormalizationJournal,
) -> Result<SyncEnvelope, AppError> {
    for config in &mut envelope.configs {
        config.api_key_plaintext = None;
    }
    strip_successful_task_payloads(&mut envelope.tasks);
    for asset in &mut envelope.assets {
        let mut invalidated_remote_reference = false;
        if let Some(object_key) = asset.remote_object_key.take() {
            if is_user_asset_object_key(&object_key, user_id, &asset.sha256) {
                let trusted_index = find_trusted_asset_index_for_object(
                    &state.db,
                    user_id,
                    &object_key,
                    &asset.sha256,
                    state.config.max_upload_bytes,
                )
                .await?;
                if let Some(indexed) = trusted_index
                    && object_exists(state, &object_key).await?
                {
                    asset.remote_object_key = Some(object_key.clone());
                    asset.remote_url = Some(format!("/api/assets/{}", asset.id));
                    asset.data_url = None;
                    asset.mime_type = indexed.mime_type.clone();
                    asset.byte_len = indexed.byte_len as u64;
                    asset.sha256 = indexed.sha256;
                    let mime_type = asset.mime_type.clone();
                    upsert_asset_index(state, user_id, asset, &object_key, &mime_type, journal)
                        .await?;
                    continue;
                }

                if object_exists(state, &object_key).await? {
                    // 没有服务端索引时必须读取原文件；客户端声明的 MIME 和大小均不可作为配额依据。
                    let bytes =
                        get_object_bytes(state, &object_key, state.config.max_upload_bytes).await?;
                    let mime_type = detect_image_mime(&bytes).ok_or_else(|| {
                        AppError::bad_request(format!(
                            "同步图片 `{}` 的远程原文件格式无效。",
                            asset.id
                        ))
                    })?;
                    let actual_sha256 = hex_sha256(&bytes);
                    if actual_sha256 != asset.sha256 {
                        return Err(AppError::bad_request(format!(
                            "同步图片 `{}` 的远程原文件哈希校验失败。",
                            asset.id
                        )));
                    }
                    let actual_byte_len = bytes.len() as u64;
                    asset.mime_type = mime_type.to_string();
                    asset.byte_len = actual_byte_len;
                    asset.sha256 = actual_sha256;
                    asset.remote_object_key = Some(object_key.clone());
                    asset.remote_url = Some(format!("/api/assets/{}", asset.id));
                    asset.data_url = None;
                    upsert_asset_index(state, user_id, asset, &object_key, mime_type, journal)
                        .await?;
                    continue;
                }

                delete_asset_indexes_for_object_journaled(&state.db, user_id, &object_key, journal)
                    .await?;
                invalidated_remote_reference = true;
                warn!(
                    "discarded missing synced object for user {}: {}",
                    user_id, object_key
                );
            } else {
                invalidated_remote_reference = true;
                warn!(
                    "ignored invalid synced object key for user {}: {}",
                    user_id, object_key
                );
            }
        }
        asset.remote_url = None;

        if let Some((object_key, mime_type, byte_len, sha256)) =
            find_available_indexed_asset_object(state, user_id, &asset.id, journal).await?
        {
            asset.remote_object_key = Some(object_key.clone());
            asset.remote_url = Some(format!("/api/assets/{}", asset.id));
            asset.data_url = None;
            asset.mime_type = mime_type.clone();
            asset.byte_len = byte_len.max(0) as u64;
            asset.sha256 = sha256;
            upsert_asset_index(state, user_id, asset, &object_key, &mime_type, journal).await?;
            continue;
        }

        if let Some((object_key, mime_type, byte_len)) =
            find_available_asset_object_by_hash(state, user_id, &asset.sha256, journal).await?
        {
            asset.remote_object_key = Some(object_key.clone());
            asset.remote_url = Some(format!("/api/assets/{}", asset.id));
            asset.data_url = None;
            asset.mime_type = mime_type.clone();
            asset.byte_len = byte_len.max(0) as u64;
            upsert_asset_index(state, user_id, asset, &object_key, &mime_type, journal).await?;
            continue;
        }

        if invalidated_remote_reference {
            // 让服务器确认的“远程文件已失效”覆盖客户端同时间戳的旧远程标记。
            asset.updated_at = now_rfc3339();
        }
        let Some(data_url) = asset.data_url.take() else {
            continue;
        };
        let (declared_mime, bytes) = decode_data_url(&data_url)?;
        if bytes.is_empty() || bytes.len() as u64 > state.config.max_upload_bytes {
            return Err(AppError::bad_request(format!(
                "同步图片 `{}` 超过单文件存储限制。",
                asset.id
            )));
        }
        let mime_type = detect_image_mime(&bytes)
            .ok_or_else(|| AppError::bad_request("同步数据包含不受支持的图片格式。"))?;
        if declared_mime != mime_type || hex_sha256(&bytes) != asset.sha256 {
            return Err(AppError::bad_request(format!(
                "同步图片 `{}` 的类型或哈希校验失败。",
                asset.id
            )));
        }
        let byte_len = bytes.len() as u64;
        let object_key = format!("users/{user_id}/assets/{}.bin", asset.sha256);
        ensure_user_asset_capacity(state, user_id, &asset.id, &object_key, byte_len).await?;
        let object_already_existed = object_exists(state, &object_key).await?;
        put_object(state, &object_key, mime_type, bytes).await?;
        if !object_already_existed {
            journal.record_new_object(object_key.clone());
        }
        asset.mime_type = mime_type.to_string();
        asset.byte_len = byte_len;
        asset.remote_object_key = Some(object_key.clone());
        asset.remote_url = Some(format!("/api/assets/{}", asset.id));
        upsert_asset_index_record(state, user_id, asset, &object_key, mime_type, journal).await?;
    }
    envelope.updated_at = now_rfc3339();
    Ok(envelope)
}

async fn find_indexed_asset_object(
    db: &SqlitePool,
    user_id: &str,
    asset_id: &str,
) -> Result<Option<(String, String, i64, String)>, AppError> {
    sqlx::query_as::<_, (String, String, i64, String)>(
        "SELECT object_key, mime_type, byte_len, sha256 FROM assets
         WHERE user_id = ? AND id = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(asset_id)
    .fetch_optional(db)
    .await
    .map_err(AppError::internal)
}

async fn find_asset_index_record(
    db: &SqlitePool,
    user_id: &str,
    asset_id: &str,
) -> Result<Option<StoredAssetIndex>, AppError> {
    let row = sqlx::query(
        "SELECT id, object_key, mime_type, sha256, byte_len, created_at FROM assets
         WHERE user_id = ? AND id = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(asset_id)
    .fetch_optional(db)
    .await
    .map_err(AppError::internal)?;
    Ok(row.map(|row| StoredAssetIndex {
        id: row.get("id"),
        object_key: row.get("object_key"),
        mime_type: row.get("mime_type"),
        sha256: row.get("sha256"),
        byte_len: row.get("byte_len"),
        created_at: row.get("created_at"),
    }))
}

async fn find_trusted_asset_index_for_object(
    db: &SqlitePool,
    user_id: &str,
    object_key: &str,
    expected_sha256: &str,
    max_bytes: u64,
) -> Result<Option<StoredAssetIndex>, AppError> {
    let row = sqlx::query(
        "SELECT id, object_key, mime_type, sha256, byte_len, created_at FROM assets
         WHERE user_id = ? AND object_key = ? AND sha256 = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(object_key)
    .bind(expected_sha256)
    .fetch_optional(db)
    .await
    .map_err(AppError::internal)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let indexed = StoredAssetIndex {
        id: row.get("id"),
        object_key: row.get("object_key"),
        mime_type: row.get("mime_type"),
        sha256: row.get("sha256"),
        byte_len: row.get("byte_len"),
        created_at: row.get("created_at"),
    };
    let metadata_is_valid = indexed.byte_len > 0
        && u64::try_from(indexed.byte_len).is_ok_and(|length| length <= max_bytes)
        && matches!(
            indexed.mime_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        )
        && is_user_asset_object_key(&indexed.object_key, user_id, &indexed.sha256);
    Ok(metadata_is_valid.then_some(indexed))
}

async fn find_available_indexed_asset_object(
    state: &AppState,
    user_id: &str,
    asset_id: &str,
    journal: &mut AssetNormalizationJournal,
) -> Result<Option<(String, String, i64, String)>, AppError> {
    let Some(indexed) = find_indexed_asset_object(&state.db, user_id, asset_id).await? else {
        return Ok(None);
    };
    if object_exists(state, &indexed.0).await? {
        return Ok(Some(indexed));
    }
    delete_asset_indexes_for_object_journaled(&state.db, user_id, &indexed.0, journal).await?;
    Ok(None)
}

fn is_user_asset_object_key(object_key: &str, user_id: &str, sha256: &str) -> bool {
    let expected_prefix = format!("users/{user_id}/assets/{sha256}");
    object_key
        .strip_prefix(&expected_prefix)
        .map(|suffix| {
            matches!(suffix.as_bytes().first(), Some(b'.' | b'-'))
                && !suffix.contains('/')
                && !suffix.contains("..")
        })
        .unwrap_or(false)
}

async fn find_existing_asset_object(
    db: &SqlitePool,
    user_id: &str,
    sha256: &str,
) -> Result<Option<(String, String, i64)>, AppError> {
    sqlx::query_as::<_, (String, String, i64)>(
        "SELECT object_key, mime_type, byte_len FROM assets
         WHERE user_id = ? AND sha256 = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(sha256)
    .fetch_optional(db)
    .await
    .map_err(AppError::internal)
}

async fn find_available_asset_object_by_hash(
    state: &AppState,
    user_id: &str,
    sha256: &str,
    journal: &mut AssetNormalizationJournal,
) -> Result<Option<(String, String, i64)>, AppError> {
    loop {
        let Some(indexed) = find_existing_asset_object(&state.db, user_id, sha256).await? else {
            return Ok(None);
        };
        if object_exists(state, &indexed.0).await? {
            return Ok(Some(indexed));
        }
        delete_asset_indexes_for_object_journaled(&state.db, user_id, &indexed.0, journal).await?;
    }
}

async fn delete_asset_indexes_for_object_journaled(
    db: &SqlitePool,
    user_id: &str,
    object_key: &str,
    journal: &mut AssetNormalizationJournal,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT id, object_key, mime_type, sha256, byte_len, created_at FROM assets
         WHERE user_id = ? AND object_key = ?",
    )
    .bind(user_id)
    .bind(object_key)
    .fetch_all(db)
    .await
    .map_err(AppError::internal)?;
    if rows.is_empty() {
        return Ok(());
    }

    delete_asset_indexes_for_object(db, user_id, object_key).await?;
    for row in rows {
        let previous = StoredAssetIndex {
            id: row.get("id"),
            object_key: row.get("object_key"),
            mime_type: row.get("mime_type"),
            sha256: row.get("sha256"),
            byte_len: row.get("byte_len"),
            created_at: row.get("created_at"),
        };
        journal.record_index_mutation(previous.id.clone(), Some(previous));
    }
    Ok(())
}

async fn delete_asset_indexes_for_object(
    db: &SqlitePool,
    user_id: &str,
    object_key: &str,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM assets WHERE user_id = ? AND object_key = ?")
        .bind(user_id)
        .bind(object_key)
        .execute(db)
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

async fn rollback_asset_index_mutations(
    db: &SqlitePool,
    user_id: &str,
    mutations: Vec<AssetIndexMutation>,
) -> Result<(), AppError> {
    if mutations.is_empty() {
        return Ok(());
    }

    let mut transaction = db.begin().await.map_err(AppError::internal)?;
    for mutation in mutations.into_iter().rev() {
        if let Some(previous) = mutation.previous {
            let result = sqlx::query(
                "INSERT INTO assets
                 (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                     object_key = excluded.object_key,
                     mime_type = excluded.mime_type,
                     sha256 = excluded.sha256,
                     byte_len = excluded.byte_len,
                     created_at = excluded.created_at
                 WHERE assets.user_id = excluded.user_id",
            )
            .bind(&previous.id)
            .bind(user_id)
            .bind(&previous.object_key)
            .bind(&previous.mime_type)
            .bind(&previous.sha256)
            .bind(previous.byte_len)
            .bind(&previous.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
            if result.rows_affected() == 0 {
                transaction.rollback().await.map_err(AppError::internal)?;
                return Err(AppError::internal_message(
                    "同步图片索引回滚遇到资源 ID 冲突。",
                ));
            }
        } else {
            sqlx::query("DELETE FROM assets WHERE id = ? AND user_id = ?")
                .bind(&mutation.asset_id)
                .bind(user_id)
                .execute(&mut *transaction)
                .await
                .map_err(AppError::internal)?;
        }
    }
    transaction.commit().await.map_err(AppError::internal)
}

async fn upsert_asset_index(
    state: &AppState,
    user_id: &str,
    asset: &ImageAssetRef,
    object_key: &str,
    mime_type: &str,
    journal: &mut AssetNormalizationJournal,
) -> Result<(), AppError> {
    ensure_user_asset_capacity(state, user_id, &asset.id, object_key, asset.byte_len).await?;
    upsert_asset_index_record(state, user_id, asset, object_key, mime_type, journal).await
}

async fn upsert_asset_index_record(
    state: &AppState,
    user_id: &str,
    asset: &ImageAssetRef,
    object_key: &str,
    mime_type: &str,
    journal: &mut AssetNormalizationJournal,
) -> Result<(), AppError> {
    let previous = find_asset_index_record(&state.db, user_id, &asset.id).await?;
    let result = sqlx::query(
        "INSERT INTO assets (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             object_key = excluded.object_key,
             mime_type = excluded.mime_type,
             sha256 = excluded.sha256,
             byte_len = excluded.byte_len
         WHERE assets.user_id = excluded.user_id",
    )
    .bind(&asset.id)
    .bind(user_id)
    .bind(object_key)
    .bind(mime_type)
    .bind(&asset.sha256)
    .bind(
        i64::try_from(asset.byte_len)
            .map_err(|_| AppError::bad_request("图片资源大小超过数据库范围。"))?,
    )
    .bind(&asset.created_at)
    .execute(&state.db)
    .await
    .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::bad_request("图片资源 ID 与其他用户冲突。"));
    }
    journal.record_index_mutation(asset.id.clone(), previous);
    Ok(())
}

async fn put_object(
    state: &AppState,
    object_key: &str,
    mime_type: &str,
    bytes: impl Into<Bytes>,
) -> Result<(), AppError> {
    let bytes = bytes.into();
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(AppError::internal)?;
            }
            let expected_hash = hex_sha256(&bytes);
            let staging_dir = PathBuf::from(&state.config.local_asset_dir).join(".staging");
            tokio::fs::create_dir_all(&staging_dir)
                .await
                .map_err(AppError::internal)?;
            let staging_path = staging_dir.join(format!("{}.upload", random_token()));
            tokio::fs::write(&staging_path, &bytes)
                .await
                .map_err(AppError::internal)?;
            if let Err(error) = tokio::fs::rename(&staging_path, &path).await {
                let existing_matches = tokio::fs::read(&path)
                    .await
                    .ok()
                    .is_some_and(|existing| hex_sha256(&existing) == expected_hash);
                if existing_matches {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                    return Ok(());
                }
                if tokio::fs::metadata(&path).await.is_ok() {
                    tokio::fs::remove_file(&path)
                        .await
                        .map_err(AppError::internal)?;
                    tokio::fs::rename(&staging_path, &path)
                        .await
                        .map_err(AppError::internal)?;
                } else {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                    return Err(AppError::internal(error));
                }
            }
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

async fn object_exists(state: &AppState, object_key: &str) -> Result<bool, AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            match tokio::fs::metadata(path).await {
                Ok(metadata) => Ok(metadata.is_file()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(AppError::internal(error)),
            }
        }
        AssetStoreKind::S3 => {
            let client = state.s3.as_ref().ok_or_else(|| {
                AppError::bad_request("服务器未启用远程资源存储，当前操作不可用。")
            })?;
            match client
                .head_object()
                .bucket(&state.config.s3_bucket)
                .key(object_key)
                .send()
                .await
            {
                Ok(_) => Ok(true),
                Err(error)
                    if error
                        .as_service_error()
                        .is_some_and(|service_error| service_error.is_not_found()) =>
                {
                    Ok(false)
                }
                Err(error) => Err(AppError::internal(error)),
            }
        }
        AssetStoreKind::Disabled => Ok(false),
    }
}

async fn get_object_bytes(
    state: &AppState,
    object_key: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    match state.config.asset_store {
        AssetStoreKind::Local => {
            let path = local_object_path(&state.config.local_asset_dir, object_key)?;
            let metadata = tokio::fs::metadata(&path)
                .await
                .map_err(AppError::internal)?;
            if metadata.len() > max_bytes {
                return Err(AppError::bad_request("资源原文件超过服务器读取上限。"));
            }
            let bytes = tokio::fs::read(path).await.map_err(AppError::internal)?;
            if bytes.len() as u64 > max_bytes {
                return Err(AppError::bad_request("资源原文件超过服务器读取上限。"));
            }
            Ok(bytes)
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
            if output
                .content_length()
                .is_some_and(|length| length < 0 || length as u64 > max_bytes)
            {
                return Err(AppError::bad_request("资源原文件超过服务器读取上限。"));
            }
            let content_length = output.content_length();
            read_s3_body_bounded(output.body, max_bytes, content_length).await
        }
        AssetStoreKind::Disabled => Err(AppError::bad_request(
            "服务器未启用远程资源存储，当前资源无法读取。",
        )),
    }
}

async fn read_s3_body_bounded(
    mut body: ByteStream,
    max_bytes: u64,
    content_length: Option<i64>,
) -> Result<Vec<u8>, AppError> {
    let initial_capacity = content_length
        .and_then(|length| usize::try_from(length).ok())
        .filter(|length| u64::try_from(*length).is_ok_and(|length| length <= max_bytes))
        .unwrap_or(0);
    let mut bytes = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(AppError::internal)?;
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| AppError::bad_request("资源原文件大小溢出。"))?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > max_bytes {
            return Err(AppError::bad_request("资源原文件超过服务器读取上限。"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
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

fn resolve_client_ip(config: &AppConfig, headers: &HeaderMap, peer_addr: SocketAddr) -> IpAddr {
    let peer_ip = peer_addr.ip();
    if !config.trust_proxy_headers || !is_trusted_proxy(config, peer_ip) {
        return peer_ip;
    }

    if let Some(value) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    {
        let mut chain = value
            .split(',')
            .filter_map(|value| value.trim().parse::<IpAddr>().ok())
            .collect::<Vec<_>>();
        chain.push(peer_ip);
        if let Some(client_ip) = chain
            .into_iter()
            .rev()
            .find(|ip| !is_trusted_proxy(config, *ip))
        {
            return client_ip;
        }
    }
    if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
    {
        return ip;
    }
    peer_ip
}

fn is_trusted_proxy(config: &AppConfig, ip: IpAddr) -> bool {
    config
        .trusted_proxy_cidrs
        .iter()
        .any(|network| network.contains(&ip))
}

fn registration_device_id(cookies: &Cookies, config: &AppConfig) -> String {
    if let Some(cookie) = cookies.get(REGISTRATION_DEVICE_COOKIE)
        && uuid::Uuid::parse_str(cookie.value()).is_ok()
    {
        return cookie.value().to_string();
    }

    let device_id = new_id();
    let mut cookie = Cookie::new(REGISTRATION_DEVICE_COOKIE, device_id.clone());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(config.session_secure);
    cookie.set_max_age(CookieDuration::days(3650));
    cookies.add(cookie);
    device_id
}

fn hash_auth_identifier(secret: &str, namespace: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update([0]);
    hasher.update(namespace.as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn enforce_auth_rate_limit(
    db: &SqlitePool,
    scope: &str,
    key_hash: &str,
    limit: u32,
    window_seconds: u64,
    message: &str,
) -> Result<(), AppError> {
    if limit == 0 {
        return Ok(());
    }
    let now = Utc::now().timestamp();
    let window_seconds = window_seconds.max(1).min(i64::MAX as u64) as i64;
    let reset_before = now.saturating_sub(window_seconds);
    let row = sqlx::query(
        "INSERT INTO auth_rate_limits (scope, key_hash, window_started_at, attempts)
         VALUES (?, ?, ?, 1)
         ON CONFLICT(scope, key_hash) DO UPDATE SET
             attempts = CASE
                 WHEN auth_rate_limits.window_started_at <= ? THEN 1
                 ELSE auth_rate_limits.attempts + 1
             END,
             window_started_at = CASE
                 WHEN auth_rate_limits.window_started_at <= ? THEN excluded.window_started_at
                 ELSE auth_rate_limits.window_started_at
             END
         RETURNING attempts, window_started_at",
    )
    .bind(scope)
    .bind(key_hash)
    .bind(now)
    .bind(reset_before)
    .bind(reset_before)
    .fetch_one(db)
    .await
    .map_err(AppError::internal)?;
    let attempts = row.get::<i64, _>("attempts");
    if attempts <= i64::from(limit) {
        return Ok(());
    }
    let window_started_at = row.get::<i64, _>("window_started_at");
    let retry_after = window_started_at
        .saturating_add(window_seconds)
        .saturating_sub(now)
        .max(1) as u64;
    Err(AppError::rate_limited(
        message,
        "auth_rate_limited",
        retry_after,
    ))
}

async fn ensure_device_registration_available(
    db: &SqlitePool,
    device_hash: &str,
    limit: u32,
) -> Result<(), AppError> {
    if limit == 0 {
        return Ok(());
    }
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT registration_count FROM registration_devices WHERE device_hash = ?",
    )
    .bind(device_hash)
    .fetch_optional(db)
    .await
    .map_err(AppError::internal)?
    .unwrap_or(0);
    if count >= i64::from(limit) {
        return Err(AppError::device_registration_limited(limit));
    }
    Ok(())
}

async fn reserve_device_registration(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    device_hash: &str,
    limit: u32,
    updated_at: &str,
) -> Result<(), AppError> {
    if limit == 0 {
        return Ok(());
    }
    let result = sqlx::query(
        "INSERT INTO registration_devices (device_hash, registration_count, updated_at)
         VALUES (?, 1, ?)
         ON CONFLICT(device_hash) DO UPDATE SET
             registration_count = registration_devices.registration_count + 1,
             updated_at = excluded.updated_at
         WHERE registration_devices.registration_count < ?",
    )
    .bind(device_hash)
    .bind(updated_at)
    .bind(limit)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::device_registration_limited(limit));
    }
    Ok(())
}

fn active_lock_retry_seconds(locked_until: Option<String>) -> Option<u64> {
    let locked_until = locked_until?;
    let locked_until = chrono::DateTime::parse_from_rfc3339(&locked_until).ok()?;
    let remaining = locked_until
        .timestamp()
        .saturating_sub(Utc::now().timestamp());
    (remaining > 0).then_some(remaining as u64)
}

async fn record_failed_login(
    db: &SqlitePool,
    user_id: &str,
    limit: u32,
    lock_seconds: u64,
) -> Result<Option<u64>, AppError> {
    if limit == 0 {
        return Ok(None);
    }
    let lock_seconds = lock_seconds.max(1);
    let locked_until =
        (Utc::now() + Duration::seconds(lock_seconds.min(i64::MAX as u64) as i64)).to_rfc3339();
    let row = sqlx::query(
        "UPDATE users SET
             locked_until = CASE WHEN failed_login_count + 1 >= ? THEN ? ELSE NULL END,
             failed_login_count = CASE
                 WHEN failed_login_count + 1 >= ? THEN 0
                 ELSE failed_login_count + 1
             END
         WHERE id = ?
         RETURNING locked_until",
    )
    .bind(limit)
    .bind(&locked_until)
    .bind(limit)
    .bind(user_id)
    .fetch_one(db)
    .await
    .map_err(AppError::internal)?;
    let active_lock = row.get::<Option<String>, _>("locked_until");
    Ok(active_lock.map(|_| lock_seconds))
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

fn password_matches(password: &str, password_hash: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(password_hash)
        .map_err(|error| AppError::internal_message(format!("密码哈希格式无效：{error}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

async fn hash_password_with_limit(state: &AppState, password: String) -> Result<String, AppError> {
    let permit = state
        .auth_hash_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::internal_message("认证服务暂时不可用"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_password(&password)
    })
    .await
    .map_err(|error| AppError::internal_message(format!("密码哈希任务失败：{error}")))?
}

async fn verify_password_with_limit(
    state: &AppState,
    password: String,
    password_hash: String,
) -> Result<bool, AppError> {
    let permit = state
        .auth_hash_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::internal_message("认证服务暂时不可用"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        password_matches(&password, &password_hash)
    })
    .await
    .map_err(|error| AppError::internal_message(format!("密码校验任务失败：{error}")))?
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
    let uses_configured_base_url = template_uses_configured_base_url(template);
    if template.base_url.trim().is_empty() && !uses_configured_base_url {
        return Err(AppError::bad_request("模板基础地址不能为空"));
    }
    if !template.base_url.trim().is_empty()
        && !template.base_url.starts_with("http://")
        && !template.base_url.starts_with("https://")
    {
        return Err(AppError::bad_request("模板基础地址必须是 http/https"));
    }
    if require_custom_host_whitelist && !template.base_url.trim().is_empty() {
        resolve_upstream_target(
            state,
            template.kind,
            &template.base_url,
            true,
            require_custom_host_whitelist,
        )?;
    }
    Ok(())
}

fn template_uses_configured_base_url(template: &ProviderTemplate) -> bool {
    template.kind == ProviderKind::OpenAiCompatible
        && template.id == BUILTIN_OPENAI_COMPATIBLE_TEMPLATE_ID
}

fn validate_generate_request(
    state: &AppState,
    user: Option<&UserSummary>,
    payload: &GenerateViaProxyRequest,
) -> Result<(), AppError> {
    let user_is_approved = user.is_some_and(|user| user.status == "approved");
    if !user_is_approved && !state.config.enable_guest_proxy {
        return Err(AppError::unauthorized(
            "当前部署已关闭游客代理，请登录后再试。",
        ));
    }

    if matches!(payload.template.kind, ProviderKind::CustomHttp) && !user_is_approved {
        return Err(AppError::unauthorized(
            "自定义服务商仅对已审批的登录用户开放。",
        ));
    }

    validate_template(
        state,
        &payload.template,
        state.config.enforce_provider_host_whitelist,
    )?;
    let _ = resolve_upstream_target(
        state,
        payload.template.kind,
        &payload.config.base_url,
        false,
        state.config.enforce_provider_host_whitelist,
    )?;
    Ok(())
}

fn resolve_provider_base_url(
    state: &AppState,
    kind: ProviderKind,
    configured_base_url: &str,
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
        false,
        state.config.enforce_provider_host_whitelist,
    )?;
    Ok(target.base_url)
}

fn resolve_upstream_target(
    state: &AppState,
    kind: ProviderKind,
    base_url: &str,
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
    if (require_https || !state.config.allow_insecure_upstreams) && url.scheme() != "https" {
        return Err(AppError::provider_target_blocked(
            "当前上游仅允许 HTTPS 地址；如确需明文 HTTP，部署者必须显式开启兼容开关。",
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

    // 游客可使用任意安全公网 HTTPS 标准上游；白名单只在部署者显式开启时生效。
    let requires_trusted_host = enforce_custom_whitelist;
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
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_private_ip(ip)
    {
        return Err(AppError::provider_target_blocked(
            "不允许访问本机、私网或链路本地 IP。",
        ));
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            let [first, second, third, _] = ipv4.octets();
            ipv4.is_private()
                || ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
                || first == 0
                || first == 100 && (second & 0b1100_0000) == 0b0100_0000
                || first == 192 && second == 0 && third == 0
                || first == 192 && second == 88 && third == 99
                || first == 198 && matches!(second, 18 | 19)
                || first >= 224
        }
        IpAddr::V6(ipv6) => {
            let segments = ipv6.segments();
            if let Some(mapped) = ipv6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(mapped));
            }

            // 公网 IPv6 当前位于 2000::/3；其余地址保守视作内部、保留或特殊用途地址。
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || ipv6.is_unique_local()
                || ipv6.is_unicast_link_local()
                || ipv6.is_multicast()
                || segments[0] & 0xe000 != 0x2000
                || segments[0] == 0x2001 && segments[1] == 0
                || segments[0] == 0x2001 && segments[1] == 2
                || segments[0] == 0x2001 && segments[1] == 0x0db8
                || segments[0] == 0x2002
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

async fn prepare_upstream_request(
    state: &AppState,
    raw_url: &str,
    kind: UpstreamRequestKind,
) -> Result<PreparedUpstreamRequest, AppError> {
    let url =
        Url::parse(raw_url).map_err(|_| AppError::provider_target_blocked("上游地址格式无效。"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::provider_target_blocked(
            "仅允许访问 HTTP/HTTPS 上游。",
        ));
    }
    if url.scheme() != "https" && !state.config.allow_insecure_upstreams {
        return Err(AppError::provider_target_blocked(
            "上游必须使用 HTTPS；明文 HTTP 默认关闭。",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::provider_target_blocked(
            "上游地址不得在 URL 中携带账号或密码。",
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| AppError::provider_target_blocked("上游地址缺少主机名。"))?
        .to_ascii_lowercase();
    reject_unsafe_host(&host)?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::provider_target_blocked("无法确定上游端口。"))?;
    let lookup = tokio::time::timeout(
        StdDuration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| AppError::bad_gateway("上游 DNS 解析超时。"))?
    .map_err(|error| {
        warn!("upstream DNS resolution failed for {host}: {error}");
        AppError::bad_gateway("上游 DNS 解析失败。")
    })?;
    let mut addresses = lookup.collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(AppError::bad_gateway("上游域名没有可用的解析地址。"));
    }
    if addresses.iter().any(|address| is_private_ip(address.ip())) {
        return Err(AppError::provider_target_blocked(
            "上游域名解析到了本机、私网、保留或链路本地地址。",
        ));
    }

    // 固定本次请求已校验的地址，避免校验后再次解析造成 DNS rebinding。
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(StdDuration::from_secs(10))
        .no_proxy()
        .resolve_to_addrs(&host, &addresses);
    builder = match kind {
        UpstreamRequestKind::Generation => builder
            .read_timeout(StdDuration::from_secs(10 * 60))
            .timeout(PROXY_GENERATION_JOB_TIMEOUT),
        UpstreamRequestKind::Image => builder
            .read_timeout(StdDuration::from_secs(30))
            .timeout(StdDuration::from_secs(2 * 60)),
    };
    let client = builder.build().map_err(AppError::internal)?;
    Ok(PreparedUpstreamRequest { client, url })
}

fn upstream_transport_error(label: &str, error: &reqwest::Error) -> AppError {
    let reason = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接失败"
    } else if error.is_request() {
        "请求构建失败"
    } else if error.is_body() {
        "请求或响应正文读取失败"
    } else {
        "网络请求失败"
    };
    warn!(
        "{label} upstream transport error (timeout={}, connect={}, request={}, body={})",
        error.is_timeout(),
        error.is_connect(),
        error.is_request(),
        error.is_body()
    );
    AppError::bad_gateway(format!("{label}{reason}。"))
}

async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(AppError::bad_gateway(format!(
            "{label}响应超过 {} MiB 安全上限。",
            max_bytes / 1024 / 1024
        )));
    }

    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(max_bytes);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| upstream_transport_error(label, &error))?
    {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| AppError::bad_gateway(format!("{label}响应大小溢出。")))?;
        if next_len > max_bytes {
            return Err(AppError::bad_gateway(format!(
                "{label}响应超过 {} MiB 安全上限。",
                max_bytes / 1024 / 1024
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_error_body_prefix(
    mut response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> Result<(Vec<u8>, bool), AppError> {
    let mut body = Vec::with_capacity(max_bytes.min(16 * 1024));
    let mut truncated = response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64);
    loop {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| upstream_transport_error(label, &error))?
        else {
            break;
        };
        if body.len() == max_bytes {
            truncated = true;
            break;
        }
        let remaining = max_bytes - body.len();
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
}

fn is_sensitive_json_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "authorization",
        "cookie",
        "credential",
        "password",
        "secret",
        "sessionid",
        "setcookie",
    ]
    .iter()
    .any(|candidate| normalized.contains(candidate))
        || normalized == "token"
}

fn redact_sensitive_json_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if is_sensitive_json_key(key) {
                    *value = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact_sensitive_json_fields(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_json_fields(value);
            }
        }
        _ => {}
    }
}

fn clean_control_characters(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
}

fn redact_bearer_tokens(value: &str) -> String {
    let lowercase = value.to_ascii_lowercase();
    let lowercase_bytes = lowercase.as_bytes();
    let original_bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while let Some(relative_start) = lowercase[cursor..].find("bearer ") {
        let start = cursor + relative_start;
        let token_start = start + "bearer ".len();
        let mut token_end = token_start;
        while token_end < original_bytes.len()
            && !matches!(
                original_bytes[token_end],
                b' ' | b'\t' | b'\r' | b'\n' | b'\"' | b'\'' | b',' | b'<' | b'>' | b'}'
            )
        {
            token_end += 1;
        }
        output.push_str(&value[cursor..start]);
        output.push_str("Bearer [REDACTED]");
        cursor = token_end;
        if cursor >= lowercase_bytes.len() {
            break;
        }
    }
    output.push_str(&value[cursor..]);
    output
}

fn redact_sensitive_header_lines(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let normalized = line.trim_start().to_ascii_lowercase();
            if [
                "authorization:",
                "cookie:",
                "set-cookie:",
                "x-api-key:",
                "api-key:",
            ]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
            {
                let name = line.split_once(':').map(|(name, _)| name).unwrap_or(line);
                format!("{name}: [REDACTED]")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_large_encoded_blocks(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    let mut encoded_start = None;
    for (index, character) in value
        .char_indices()
        .chain(std::iter::once((value.len(), ' ')))
    {
        let looks_encoded =
            character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=' | '-' | '_');
        if looks_encoded {
            encoded_start.get_or_insert(index);
            continue;
        }
        let Some(start) = encoded_start.take() else {
            continue;
        };
        if index.saturating_sub(start) < 256 {
            continue;
        }
        output.push_str(&value[cursor..start]);
        output.push_str("[LARGE_ENCODED_DATA_REDACTED]");
        cursor = index;
    }
    output.push_str(&value[cursor..]);
    output
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[已截断]", &value[..end])
}

fn sanitize_upstream_error_body(body: &[u8], api_key: &str, truncated: bool) -> String {
    let lossy = String::from_utf8_lossy(body);
    let mut sanitized = if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&lossy) {
        redact_sensitive_json_fields(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| clean_control_characters(&lossy))
    } else {
        redact_sensitive_header_lines(&lossy)
    };
    sanitized = clean_control_characters(&sanitized);
    if !api_key.is_empty() {
        sanitized = sanitized.replace(api_key, "[REDACTED]");
    }
    sanitized = redact_bearer_tokens(&sanitized);
    sanitized = redact_large_encoded_blocks(&sanitized);
    sanitized = truncate_utf8(sanitized.trim(), MAX_UPSTREAM_ERROR_BYTES);
    if sanitized.is_empty() {
        sanitized.push_str("上游未返回错误正文");
    }
    if truncated && !sanitized.ends_with("[已截断]") {
        sanitized.push_str("…[已截断]");
    }
    sanitized
}

async fn upstream_response_error(
    response: reqwest::Response,
    label: &str,
    api_key: &str,
) -> AppError {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(clean_control_characters)
        .map(|value| truncate_utf8(value.trim(), 256));
    let body = match read_error_body_prefix(response, MAX_UPSTREAM_ERROR_BYTES, label).await {
        Ok((body, truncated)) => sanitize_upstream_error_body(&body, api_key, truncated),
        Err(error) => return error,
    };
    let request_id = request_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| format!("，request_id={value}"))
        .unwrap_or_default();
    warn!("{label} upstream error: HTTP {status}{request_id}, {body}");
    AppError::bad_gateway(format!(
        "{label}上游请求失败：HTTP {status}{request_id}，{body}"
    ))
}

async fn parse_upstream_json_response(
    response: reqwest::Response,
    label: &str,
    api_key: &str,
) -> Result<serde_json::Value, AppError> {
    if !response.status().is_success() {
        return Err(upstream_response_error(response, label, api_key).await);
    }
    let body = read_response_bytes_limited(response, MAX_UPSTREAM_RESPONSE_BYTES, label).await?;
    serde_json::from_slice(&body).map_err(|error| {
        warn!("{label} returned invalid JSON: {error}");
        AppError::bad_gateway(format!("{label}返回了无法解析的 JSON。"))
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

async fn lease_upload_token(
    state: &AppState,
    user_id: &str,
    token: &str,
) -> Result<sqlx::sqlite::SqliteRow, AppError> {
    let now = now_rfc3339();
    let lease_expires_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();
    sqlx::query(
        "UPDATE upload_tokens SET expires_at = ?
         WHERE token = ? AND user_id = ? AND expires_at > ?
         RETURNING asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at",
    )
    .bind(lease_expires_at)
    .bind(token)
    .bind(user_id)
    .bind(now)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("上传凭证不存在、已过期或不属于当前用户。"))
}

async fn periodically_cleanup_expired_uploads(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(StdDuration::from_secs(15 * 60));
    loop {
        interval.tick().await;
        if let Err(error) = cleanup_expired_upload_tokens(&state).await {
            warn!("expired upload cleanup failed: {}", error.message);
        }
        if let Err(error) = cleanup_local_staging_files(&state).await {
            warn!("local upload staging cleanup failed: {}", error.message);
        }
        cleanup_stale_proxy_temp_dirs().await;
    }
}

async fn cleanup_local_staging_files(state: &AppState) -> Result<(), AppError> {
    if state.config.asset_store != AssetStoreKind::Local {
        return Ok(());
    }
    let staging_dir = PathBuf::from(&state.config.local_asset_dir).join(".staging");
    let mut entries = match tokio::fs::read_dir(&staging_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::internal(error)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(AppError::internal)? {
        let metadata = entry.metadata().await.map_err(AppError::internal)?;
        let is_expired = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= StdDuration::from_secs(30 * 60));
        if metadata.is_file() && is_expired {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
}

async fn cleanup_expired_upload_tokens(state: &AppState) -> Result<(), AppError> {
    let now = now_rfc3339();
    let rows = sqlx::query("SELECT DISTINCT object_key FROM upload_tokens WHERE expires_at <= ?")
        .bind(&now)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::internal)?;
    for row in rows {
        let object_key = row.get::<String, _>("object_key");
        let live_references = sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT COUNT(*) FROM assets WHERE object_key = ?) +
                (SELECT COUNT(*) FROM upload_tokens
                 WHERE object_key = ? AND expires_at > ?)",
        )
        .bind(&object_key)
        .bind(&object_key)
        .bind(&now)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?;
        if live_references == 0
            && let Err(error) = delete_object(state, &object_key).await
        {
            // 保留过期凭证作为可重试的清理记录，避免暂时的对象存储故障制造永久孤儿。
            warn!(
                "expired upload object cleanup failed for {object_key}: {}",
                error.message
            );
            continue;
        }
        sqlx::query("DELETE FROM upload_tokens WHERE object_key = ? AND expires_at <= ?")
            .bind(&object_key)
            .bind(&now)
            .execute(&state.db)
            .await
            .map_err(AppError::internal)?;
    }
    Ok(())
}

async fn delete_object_if_unreferenced(
    state: &AppState,
    user_id: &str,
    object_key: &str,
) -> Result<(), AppError> {
    let references = if user_id.is_empty() {
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT COUNT(*) FROM assets WHERE object_key = ?) +
                (SELECT COUNT(*) FROM upload_tokens WHERE object_key = ?)",
        )
        .bind(object_key)
        .bind(object_key)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT COUNT(*) FROM assets WHERE user_id = ? AND object_key = ?) +
                (SELECT COUNT(*) FROM upload_tokens WHERE user_id = ? AND object_key = ?)",
        )
        .bind(user_id)
        .bind(object_key)
        .bind(user_id)
        .bind(object_key)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?
    };
    if references == 0 {
        delete_object(state, object_key).await?;
    }
    Ok(())
}

fn decode_data_url(data_url: &str) -> Result<(String, Vec<u8>), AppError> {
    let Some((meta, data)) = data_url.split_once(',') else {
        return Err(AppError::bad_request("无效的数据 URL"));
    };
    let meta = meta
        .strip_prefix("data:")
        .ok_or_else(|| AppError::bad_request("无效的数据 URL"))?;
    let mut parts = meta.split(';');
    let mime_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    if !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return Err(AppError::bad_request("图片数据 URL 必须使用 Base64 编码。"));
    }
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

fn supports_configurable_input_fidelity(model: &str) -> bool {
    model.trim().to_ascii_lowercase().contains("gpt-image-1")
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

async fn invoke_openai_image(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let base_url =
        resolve_provider_base_url(state, ProviderKind::OpenAiImage, &payload.config.base_url)?;
    let url = join_api_url(
        &base_url,
        match payload.config.endpoint_mode {
            ProviderEndpointMode::ImagesApi => openai_images_endpoint(&payload.request),
            ProviderEndpointMode::ResponsesApi => "/v1/responses",
            ProviderEndpointMode::CustomJson => payload.template.endpoint_path.as_str(),
        },
    );
    let prepared = prepare_upstream_request(state, &url, UpstreamRequestKind::Generation).await?;
    let request = prepared
        .client
        .post(prepared.url.clone())
        .bearer_auth(&api_key);

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
        form = form
            .text(
                "output_format",
                normalized_image_output_format(payload.config.output_format.as_deref()),
            )
            .text(
                "background",
                normalized_openai_background(payload.config.background.as_deref()),
            );
        if let Some(compression) = openai_output_compression(
            payload.config.output_format.as_deref(),
            payload.config.output_compression,
        ) {
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
            form = form.part(OPENAI_EDIT_IMAGE_FIELD, part);
        }
        if supports_configurable_input_fidelity(&payload.request.model) {
            form = form.text("input_fidelity", "high");
        }
        request
            .multipart(form)
            .send()
            .await
            .map_err(|error| upstream_transport_error("Images API", &error))?
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
            "output_format": normalized_image_output_format(payload.config.output_format.as_deref()),
            "background": normalized_openai_background(payload.config.background.as_deref()),
            "moderation": payload.config.moderation.clone().unwrap_or_else(|| "auto".into()),
            "partial_images": 1,
        });
        if let Some(quality) = &payload.request.quality {
            tool["quality"] = json!(quality);
        }
        if let Some(compression) = openai_output_compression(
            payload.config.output_format.as_deref(),
            payload.config.output_compression,
        ) {
            tool["output_compression"] = json!(compression);
        }
        let body = json!({
            "model": resolve_responses_main_model(&payload.config, &payload.request.model),
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
        request
            .json(&body)
            .send()
            .await
            .map_err(|error| upstream_transport_error("Responses API", &error))?
    } else {
        let mut body = json!({
            "prompt": payload.request.prompt,
            "model": payload.request.model,
            "size": format!("{}x{}", payload.request.width, payload.request.height),
            "quality": payload.request.quality,
            "n": payload.request.count,
            "output_format": normalized_image_output_format(payload.config.output_format.as_deref()),
            "background": normalized_openai_background(payload.config.background.as_deref()),
            "moderation": payload.config.moderation,
        });
        if let Some(compression) = openai_output_compression(
            payload.config.output_format.as_deref(),
            payload.config.output_compression,
        ) {
            body["output_compression"] = json!(compression);
        }
        request
            .json(&body)
            .send()
            .await
            .map_err(|error| upstream_transport_error("Images API", &error))?
    };

    if payload.config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        let is_event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        if !response.status().is_success() {
            return Err(upstream_response_error(response, "Responses API", &api_key).await);
        }
        if is_event_stream {
            let mut response = response;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_UPSTREAM_RESPONSE_BYTES as u64)
            {
                return Err(AppError::bad_gateway(
                    "Responses API 响应超过 256 MiB 安全上限。",
                ));
            }
            let mut accumulator = OpenAiResponsesStreamAccumulator::new();
            let mut total_bytes = 0usize;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| upstream_transport_error("Responses API", &error))?
            {
                total_bytes = total_bytes
                    .checked_add(chunk.len())
                    .ok_or_else(|| AppError::bad_gateway("Responses API 响应大小溢出。"))?;
                if total_bytes > MAX_UPSTREAM_RESPONSE_BYTES {
                    return Err(AppError::bad_gateway(
                        "Responses API 响应超过 256 MiB 安全上限。",
                    ));
                }
                accumulator
                    .push_chunk(&chunk)
                    .map_err(AppError::bad_gateway)?;
            }
            return accumulator.finish().map_err(AppError::bad_gateway);
        }
        let body =
            read_response_bytes_limited(response, MAX_UPSTREAM_RESPONSE_BYTES, "Responses API")
                .await?;
        let body = String::from_utf8(body)
            .map_err(|_| AppError::bad_gateway("Responses API 返回了非 UTF-8 响应。"))?;
        if body.trim_start().starts_with("data:") {
            return parse_openai_responses_event_stream(&body).map_err(AppError::bad_gateway);
        }
        return serde_json::from_str(&body).map_err(|error| {
            warn!("Responses API returned invalid JSON: {error}");
            AppError::bad_gateway("Responses API 返回了无法解析的 JSON。")
        });
    }
    parse_upstream_json_response(response, "Images API", &api_key).await
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
    )?;
    let url = join_api_url(&base_url, openai_compatible_endpoint(&payload.request));
    let prepared = prepare_upstream_request(state, &url, UpstreamRequestKind::Generation).await?;

    let response = if payload.request.reference_assets.is_empty() {
        prepared
            .client
            .post(prepared.url.clone())
            .bearer_auth(&api_key)
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
            .map_err(|error| upstream_transport_error("OpenAI 兼容接口", &error))?
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
        prepared
            .client
            .post(prepared.url.clone())
            .bearer_auth(&api_key)
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .await
            .map_err(|error| upstream_transport_error("OpenAI 兼容接口", &error))?
    };

    parse_upstream_json_response(response, "OpenAI 兼容接口", &api_key).await
}

async fn invoke_nano_banana(
    state: &AppState,
    payload: &GenerateViaProxyRequest,
) -> Result<serde_json::Value, AppError> {
    let api_key = payload
        .config
        .api_key_plaintext
        .clone()
        .ok_or_else(|| AppError::bad_request("当前配置缺少 API Key"))?;
    let base_url =
        resolve_provider_base_url(state, ProviderKind::NanoBanana, &payload.config.base_url)?;
    let model = if is_google_official_gemini_base_url(&base_url) {
        normalize_google_image_model(&payload.request.model)
    } else {
        payload.request.model.trim().to_string()
    };
    if model.is_empty() {
        return Err(AppError::bad_request("当前配置缺少 Gemini 模型名称"));
    }
    let url = gemini_generate_content_url(&base_url, &model);
    let prepared = prepare_upstream_request(state, &url, UpstreamRequestKind::Generation).await?;
    let body = build_gemini_payload(state, payload, &model).await?;
    let (auth_header, auth_value) = gemini_auth_header(&base_url, &api_key);
    let response = prepared
        .client
        .post(prepared.url)
        .header("Accept", "application/json")
        .header(auth_header, auth_value)
        .json(&body)
        .send()
        .await
        .map_err(|error| upstream_transport_error("Nano Banana", &error))?;

    parse_upstream_json_response(response, "Nano Banana", &api_key).await
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
    let base_url =
        resolve_provider_base_url(state, ProviderKind::CustomHttp, &payload.config.base_url)?;
    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        payload.template.endpoint_path
    );
    let prepared = prepare_upstream_request(state, &url, UpstreamRequestKind::Generation).await?;
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
    if let Some(quality) = &payload.request.quality
        && let Some(path) = payload.template.quality_field.as_deref()
    {
        set_json_path(&mut body, path, json!(quality));
    }

    let response = prepared
        .client
        .request(
            payload
                .template
                .method
                .parse()
                .map_err(|_| AppError::bad_request("自定义模板 HTTP 方法无效"))?,
            prepared.url,
        )
        .header(&payload.template.auth_header, format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|error| upstream_transport_error("自定义服务商", &error))?;

    parse_upstream_json_response(response, "自定义服务商", &api_key).await
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
        // URL 往往带短期签名；必须在任务成功前固化原图，避免画廊只剩失效链接。
        let (mime_type, bytes) = fetch_remote_image_bytes(state, &url).await?;
        image.data_url = Some(format!("data:{mime_type};base64,{}", BASE64.encode(bytes)));
    }
    Ok(result)
}

async fn fetch_remote_image_bytes(
    state: &AppState,
    image_url: &str,
) -> Result<(String, Vec<u8>), AppError> {
    let mut current_url =
        Url::parse(image_url).map_err(|_| AppError::bad_request("上游返回了无效的图片地址。"))?;
    for redirect_count in 0..=MAX_REMOTE_IMAGE_REDIRECTS {
        validate_remote_image_target(state, &current_url)?;
        let prepared =
            prepare_upstream_request(state, current_url.as_str(), UpstreamRequestKind::Image)
                .await?;
        let response = prepared
            .client
            .get(prepared.url.clone())
            .header(header::ACCEPT, "image/png,image/jpeg,image/webp")
            .send()
            .await
            .map_err(|error| upstream_transport_error("下载上游图片", &error))?;

        if response.status().is_redirection() {
            if redirect_count == MAX_REMOTE_IMAGE_REDIRECTS {
                return Err(AppError::bad_gateway(format!(
                    "上游图片重定向超过 {MAX_REMOTE_IMAGE_REDIRECTS} 次安全上限。"
                )));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .ok_or_else(|| AppError::bad_gateway("上游图片重定向缺少 Location。"))?
                .to_str()
                .map_err(|_| AppError::bad_gateway("上游图片重定向地址无效。"))?;
            current_url = current_url
                .join(location)
                .map_err(|_| AppError::bad_gateway("上游图片重定向地址无效。"))?;
            continue;
        }

        if !response.status().is_success() {
            return Err(AppError::bad_gateway(format!(
                "下载上游图片失败：HTTP {}",
                response.status()
            )));
        }
        let declared_mime = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        let bytes =
            read_response_bytes_limited(response, MAX_REMOTE_IMAGE_BYTES, "下载上游图片").await?;
        let detected_mime = detect_image_mime(&bytes).ok_or_else(|| {
            AppError::bad_gateway("上游返回内容不是受支持的 PNG、JPEG 或 WebP 图片。")
        })?;
        if declared_mime
            .as_deref()
            .is_some_and(|declared| declared != detected_mime)
        {
            warn!(
                "remote image content type mismatch: declared={:?}, detected={detected_mime}",
                declared_mime
            );
        }
        return Ok((detected_mime.to_string(), bytes));
    }

    Err(AppError::bad_gateway("下载上游图片失败。"))
}

fn validate_remote_image_target(state: &AppState, url: &Url) -> Result<(), AppError> {
    let host = url
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
    Ok(())
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
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
        let mut result =
            match extract_openai_responses_result(request, &response_json, output_format) {
                Ok(result) => result,
                Err(error) => GenerationResult {
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
                    })),
                },
            };
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
        // Base64/URL 已提取到 images，避免代理结果再次携带整份上游 JSON。
        raw_response_json: None,
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
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let data_urls = gather_data_urls(state, &payload.request.reference_assets).await?;
    Ok(build_gemini_generation_request(
        &payload.request,
        model,
        &data_urls,
    ))
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
    let bytes = get_object_bytes(
        state,
        object_key,
        MAX_GENERATION_REFERENCE_FILE_BYTES as u64,
    )
    .await?;
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
    retry_after_seconds: Option<u64>,
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn provider_target_blocked(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            code: Some("provider_target_blocked"),
            retry_after_seconds: None,
        }
    }

    fn rate_limited(
        message: impl Into<String>,
        code: &'static str,
        retry_after_seconds: u64,
    ) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
            code: Some(code),
            retry_after_seconds: Some(retry_after_seconds.max(1)),
        }
    }

    fn device_registration_limited(limit: u32) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: format!("当前设备最多只能注册 {limit} 个账号。"),
            code: Some("device_registration_limit"),
            retry_after_seconds: None,
        }
    }

    fn internal_message(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn internal(error: impl std::error::Error) -> Self {
        error!("internal error: {}", error);
        Self::internal_message("服务器内部错误")
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({
                "error": self.message,
                "code": self.code,
                "retry_after_seconds": self.retry_after_seconds,
            })),
        )
            .into_response();
        if let Some(retry_after_seconds) = self.retry_after_seconds
            && let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{
        EncryptedApiConfig, GenerationRequest, ProviderAccessMode, ProviderEndpointMode,
        SyncTombstone,
    };
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tower_sessions::{MemoryStore, SessionStore};

    fn test_config(local_asset_dir: String) -> AppConfig {
        AppConfig {
            listen_addr: "127.0.0.1:0".into(),
            database_url: "sqlite::memory:".into(),
            frontend_dist: String::new(),
            session_secure: false,
            trust_proxy_headers: false,
            trusted_proxy_cidrs: Vec::new(),
            auth_secret: "test-auth-secret".into(),
            register_device_limit: 3,
            register_ip_limit: 10,
            register_window_seconds: 86_400,
            login_ip_limit: 20,
            login_window_seconds: 600,
            login_failure_limit: 5,
            login_lock_seconds: 300,
            auth_hash_concurrency: 2,
            allowed_web_origins: Vec::new(),
            trusted_provider_hosts: Vec::new(),
            enforce_provider_host_whitelist: false,
            allow_insecure_upstreams: false,
            enable_guest_proxy: true,
            guest_generation_concurrency: 4,
            guest_image_concurrency: 2,
            guest_generation_rate_limit: 30,
            guest_image_rate_limit: 120,
            guest_rate_window_seconds: 600,
            proxy_memory_budget_mib: 384,
            admin_setup_token: None,
            allow_first_admin_setup: false,
            asset_store: AssetStoreKind::Local,
            local_asset_dir,
            s3_bucket: String::new(),
            s3_region: "auto".into(),
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
            max_upload_bytes: 64 * 1024 * 1024,
            user_asset_quota_bytes: 10 * 1024 * 1024 * 1024,
            user_asset_quota_count: 20_000,
            user_pending_upload_bytes: 256 * 1024 * 1024,
            user_pending_upload_count: 32,
        }
    }

    async fn test_db() -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        init_db(&db).await.unwrap();
        db
    }

    async fn test_app_state(local_asset_dir: String) -> AppState {
        AppState {
            config: test_config(local_asset_dir),
            db: test_db().await,
            s3: None,
            provider_builtins: Vec::new(),
            generation_job_slots: Arc::new(tokio::sync::Semaphore::new(
                MAX_ACTIVE_PROXY_GENERATION_JOBS,
            )),
            generation_temp_budget: Arc::new(tokio::sync::Semaphore::new(384)),
            generation_memory_budget: Arc::new(tokio::sync::Semaphore::new(384)),
            generation_jobs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            user_data_write_locks: Arc::new(
                (0..USER_DATA_WRITE_LOCK_SHARDS)
                    .map(|_| tokio::sync::Mutex::new(()))
                    .collect(),
            ),
            auth_hash_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            dummy_password_hash: hash_password("dummy").unwrap(),
            guest_proxy_limits: Arc::new(GuestProxyLimits::default()),
        }
    }

    fn test_png_bytes(label: &[u8]) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(label);
        bytes
    }

    fn test_sync_asset(
        id: &str,
        sha256: &str,
        mime_type: &str,
        byte_len: u64,
        data_url: Option<String>,
        remote_object_key: Option<String>,
    ) -> ImageAssetRef {
        let now = now_rfc3339();
        ImageAssetRef {
            id: id.into(),
            sha256: sha256.into(),
            mime_type: mime_type.into(),
            byte_len,
            width: None,
            height: None,
            created_at: now.clone(),
            updated_at: now,
            data_url,
            remote_object_key,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        }
    }

    async fn insert_test_user(
        db: &SqlitePool,
        user_id: &str,
        username: &str,
        password_hash: &str,
        role: &str,
        status: &str,
        session_version: i64,
    ) {
        sqlx::query(
            "INSERT INTO users
             (id, username, password_hash, role, status, session_version, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(status)
        .bind(session_version)
        .bind(now_rfc3339())
        .execute(db)
        .await
        .unwrap();
    }

    async fn authenticated_test_session(
        store: Arc<MemoryStore>,
        user_id: &str,
        session_version: i64,
    ) -> Session {
        let session = Session::new(None, store, None);
        replace_session_identity(&session, user_id, session_version)
            .await
            .unwrap();
        session.save().await.unwrap();
        session
    }

    fn test_proxy_request(kind: ProviderKind) -> GenerateViaProxyRequest {
        let now = now_rfc3339();
        let mut template = ProviderTemplate::builtin_openai();
        template.kind = kind;
        template.base_url = "https://api.example.com".into();
        template.id = format!("test-{kind:?}");

        GenerateViaProxyRequest {
            config: EncryptedApiConfig {
                id: "config-1".into(),
                name: "test".into(),
                provider_template_id: template.id.clone(),
                provider_kind: kind,
                endpoint_mode: ProviderEndpointMode::ImagesApi,
                base_url: template.base_url.clone(),
                model: "test-model".into(),
                responses_model: None,
                access_mode: ProviderAccessMode::Proxy,
                known_requires_proxy: true,
                output_format: Some("png".into()),
                output_compression: None,
                background: Some("auto".into()),
                moderation: None,
                api_key_plaintext: Some("test-key".into()),
                api_key_encrypted: None,
                api_key_hint: None,
                prompt_guard_enabled: false,
                created_at: now.clone(),
                updated_at: now,
            },
            request: GenerationRequest {
                prompt: "test".into(),
                model: "test-model".into(),
                width: 1024,
                height: 1024,
                quality: None,
                count: 1,
                endpoint_mode: ProviderEndpointMode::ImagesApi,
                reference_assets: Vec::new(),
            },
            template,
        }
    }

    #[tokio::test]
    async fn replacing_session_identity_cycles_id_and_clears_pre_auth_data() {
        let store = Arc::new(MemoryStore::default());
        let session = Session::new(None, store.clone(), None);
        session.insert("pre_auth", "temporary").await.unwrap();
        session.save().await.unwrap();
        let old_id = session.id().unwrap();

        replace_session_identity(&session, "user-1", 7)
            .await
            .unwrap();
        session.save().await.unwrap();

        assert_ne!(session.id(), Some(old_id));
        assert_eq!(session.get::<String>("pre_auth").await.unwrap(), None);
        assert_eq!(
            session.get::<String>("user_id").await.unwrap().as_deref(),
            Some("user-1")
        );
        assert_eq!(
            session.get::<i64>("session_version").await.unwrap(),
            Some(7)
        );
        assert!(store.load(&old_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn successful_login_rotates_existing_session_id() {
        let state = Arc::new(test_app_state(String::new()).await);
        let password = "OldSecure1!";
        insert_test_user(
            &state.db,
            "user-login",
            "login-user",
            &hash_password(password).unwrap(),
            "user",
            "approved",
            0,
        )
        .await;
        let store = Arc::new(MemoryStore::default());
        let session = Session::new(None, store.clone(), None);
        session.insert("pre_auth", "temporary").await.unwrap();
        session.save().await.unwrap();
        let old_id = session.id().unwrap();

        let _ = login(
            State(state),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12000))),
            HeaderMap::new(),
            session.clone(),
            Json(AuthRequest {
                username: "login-user".into(),
                password: password.into(),
            }),
        )
        .await
        .unwrap();
        session.save().await.unwrap();

        assert_ne!(session.id(), Some(old_id));
        assert_eq!(
            session.get::<String>("user_id").await.unwrap().as_deref(),
            Some("user-login")
        );
        assert!(store.load(&old_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn password_change_invalidates_other_sessions_and_refreshes_current_one() {
        let state = Arc::new(test_app_state(String::new()).await);
        let old_password = "OldSecure1!";
        insert_test_user(
            &state.db,
            "user-password",
            "password-user",
            &hash_password(old_password).unwrap(),
            "user",
            "approved",
            0,
        )
        .await;
        let store = Arc::new(MemoryStore::default());
        let current = authenticated_test_session(store.clone(), "user-password", 0).await;
        let other = authenticated_test_session(store, "user-password", 0).await;

        change_password(
            State(state.clone()),
            current.clone(),
            Json(ChangePasswordRequest {
                old_password: old_password.into(),
                new_password: "NewSecure2!".into(),
                new_password_confirm: "NewSecure2!".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            current.get::<i64>("session_version").await.unwrap(),
            Some(1)
        );
        assert!(current_user(&state, &current).await.unwrap().is_some());
        assert!(current_user(&state, &other).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn account_status_changes_invalidate_sessions_and_gate_cloud_routes() {
        let state = test_app_state(String::new()).await;
        insert_test_user(
            &state.db,
            "user-status",
            "status-user",
            "hash",
            "user",
            "approved",
            0,
        )
        .await;
        let store = Arc::new(MemoryStore::default());
        let approved_session = authenticated_test_session(store.clone(), "user-status", 0).await;

        update_user_status(&state, "user-status", "disabled", None)
            .await
            .unwrap();
        assert!(
            current_user(&state, &approved_session)
                .await
                .unwrap()
                .is_none()
        );
        let disabled_session = authenticated_test_session(store.clone(), "user-status", 1).await;
        assert!(require_user(&state, &disabled_session).await.is_ok());
        assert!(
            require_approved_user(&state, &disabled_session)
                .await
                .is_err()
        );

        update_user_status(&state, "user-status", "approved", Some("admin-1"))
            .await
            .unwrap();
        assert!(
            current_user(&state, &disabled_session)
                .await
                .unwrap()
                .is_none()
        );
        let restored_session = authenticated_test_session(store, "user-status", 2).await;
        assert!(
            require_approved_user(&state, &restored_session)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn locked_write_revalidation_rejects_session_invalidated_after_initial_check() {
        let state = test_app_state(String::new()).await;
        insert_test_user(
            &state.db,
            "user-locked-write",
            "locked-write-user",
            "hash",
            "user",
            "approved",
            0,
        )
        .await;
        let session =
            authenticated_test_session(Arc::new(MemoryStore::default()), "user-locked-write", 0)
                .await;
        let initially_approved = require_approved_user(&state, &session).await.unwrap();

        sqlx::query(
            "UPDATE users SET status = 'disabled', session_version = session_version + 1
             WHERE id = ?",
        )
        .bind(&initially_approved.id)
        .execute(&state.db)
        .await
        .unwrap();

        let error =
            revalidate_locked_approved_user(&state, &session, initially_approved.id.as_str())
                .await
                .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(current_user(&state, &session).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn bootstrap_role_change_invalidates_other_sessions() {
        let mut state = test_app_state(String::new()).await;
        state.config.allow_first_admin_setup = true;
        state.config.admin_setup_token = Some("setup-token".into());
        insert_test_user(
            &state.db,
            "user-bootstrap",
            "bootstrap-user",
            "hash",
            "user",
            "pending",
            0,
        )
        .await;
        let state = Arc::new(state);
        let store = Arc::new(MemoryStore::default());
        let current = authenticated_test_session(store.clone(), "user-bootstrap", 0).await;
        let other = authenticated_test_session(store, "user-bootstrap", 0).await;

        let response = bootstrap_admin(
            State(state.clone()),
            current.clone(),
            Json(AdminBootstrapRequest {
                admin_setup_token: "setup-token".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(response.user.role, "admin");
        assert_eq!(response.user.status, "approved");
        assert_eq!(
            current.get::<i64>("session_version").await.unwrap(),
            Some(1)
        );
        assert!(current_user(&state, &other).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pending_and_disabled_users_only_receive_guest_proxy_permissions() {
        let state = test_app_state(String::new()).await;
        let standard_request = test_proxy_request(ProviderKind::OpenAiImage);
        let custom_request = test_proxy_request(ProviderKind::CustomHttp);

        for status in ["pending", "disabled"] {
            let user = UserSummary {
                id: format!("user-{status}"),
                username: status.into(),
                role: "user".into(),
                status: status.into(),
                image_count: 0,
                created_at: now_rfc3339(),
            };
            assert!(validate_generate_request(&state, Some(&user), &standard_request).is_ok());
            let error =
                validate_generate_request(&state, Some(&user), &custom_request).unwrap_err();
            assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        }

        let approved = UserSummary {
            id: "user-approved".into(),
            username: "approved".into(),
            role: "user".into(),
            status: "approved".into(),
            image_count: 0,
            created_at: now_rfc3339(),
        };
        assert!(validate_generate_request(&state, Some(&approved), &custom_request).is_ok());
    }

    #[tokio::test]
    async fn private_provider_templates_are_scoped_and_only_visible_when_approved() {
        let mut state = test_app_state(String::new()).await;
        state.provider_builtins = vec![ProviderTemplate::builtin_openai()];
        for (id, username, status) in [
            ("user-a", "alice", "approved"),
            ("user-b", "bob", "approved"),
            ("user-pending", "pending-user", "pending"),
        ] {
            insert_test_user(&state.db, id, username, "hash", "user", status, 0).await;
            let mut template = ProviderTemplate::builtin_openai();
            template.id = "shared-template-id".into();
            template.name = format!("template-{id}");
            let payload = serde_json::to_string(&template).unwrap();
            sqlx::query(
                "INSERT INTO provider_templates
                 (user_id, id, payload, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&template.id)
            .bind(payload)
            .bind(&template.created_at)
            .bind(&template.updated_at)
            .execute(&state.db)
            .await
            .unwrap();
        }
        let state = Arc::new(state);
        let store = Arc::new(MemoryStore::default());
        let approved = authenticated_test_session(store.clone(), "user-a", 0).await;
        let pending = authenticated_test_session(store, "user-pending", 0).await;

        let approved_templates = list_provider_templates(State(state.clone()), approved)
            .await
            .unwrap()
            .0;
        let pending_templates = list_provider_templates(State(state), pending)
            .await
            .unwrap()
            .0;

        assert_eq!(approved_templates.len(), 2);
        assert!(
            approved_templates
                .iter()
                .any(|item| item.name == "template-user-a")
        );
        assert!(
            !approved_templates
                .iter()
                .any(|item| item.name == "template-user-b")
        );
        assert_eq!(pending_templates.len(), 1);
    }

    #[test]
    fn proxy_generation_job_cleanup_expires_and_caps_completed_results() {
        let now = Instant::now();
        let mut jobs = HashMap::new();
        jobs.insert(
            "running".into(),
            ProxyGenerationJob {
                state: ProxyGenerationJobState::Running,
                updated_at: now - PROXY_GENERATION_RESULT_TTL - StdDuration::from_secs(1),
                abort_handle: None,
                memory_permit: None,
            },
        );
        jobs.insert(
            "expired".into(),
            ProxyGenerationJob {
                state: ProxyGenerationJobState::Failed("expired".into()),
                updated_at: now - PROXY_GENERATION_RESULT_TTL - StdDuration::from_secs(1),
                abort_handle: None,
                memory_permit: None,
            },
        );
        for index in 0..=MAX_STORED_PROXY_GENERATION_JOBS {
            jobs.insert(
                format!("completed-{index}"),
                ProxyGenerationJob {
                    state: ProxyGenerationJobState::Failed("failed".into()),
                    updated_at: now - StdDuration::from_secs(index as u64),
                    abort_handle: None,
                    memory_permit: None,
                },
            );
        }

        cleanup_proxy_generation_job_entries(&mut jobs, now);

        assert!(jobs.contains_key("running"));
        assert!(!jobs.contains_key("expired"));
        assert_eq!(jobs.len(), MAX_STORED_PROXY_GENERATION_JOBS);
        assert!(!jobs.contains_key(&format!("completed-{}", MAX_STORED_PROXY_GENERATION_JOBS)));
    }

    #[test]
    fn proxy_generation_active_slots_are_capped_at_twenty() {
        let slots = Arc::new(tokio::sync::Semaphore::new(
            MAX_ACTIVE_PROXY_GENERATION_JOBS,
        ));
        let permits = (0..MAX_ACTIVE_PROXY_GENERATION_JOBS)
            .map(|_| slots.clone().try_acquire_owned().unwrap())
            .collect::<Vec<_>>();

        assert!(slots.clone().try_acquire_owned().is_err());
        drop(permits);
        assert!(slots.try_acquire_owned().is_ok());
    }

    #[tokio::test]
    async fn response_extension_holds_byte_budget_until_response_is_dropped() {
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = budget.clone().acquire_owned().await.unwrap();
        let mut response = StatusCode::OK.into_response();
        response.extensions_mut().insert(ResponseMemoryPermit {
            _permit: Arc::new(permit),
        });

        assert!(budget.clone().try_acquire_owned().is_err());
        drop(response);
        assert!(budget.try_acquire_owned().is_ok());
    }

    #[tokio::test]
    async fn generation_memory_estimate_uses_references_pixels_and_fixed_overhead() {
        let mut state = test_app_state(String::new()).await;
        state.config.proxy_memory_budget_mib = 512;
        let request = GenerationRequest {
            prompt: "test".into(),
            model: "gpt-image-2".into(),
            width: 1024,
            height: 1024,
            quality: None,
            count: 2,
            endpoint_mode: ProviderEndpointMode::ImagesApi,
            reference_assets: vec![ImageAssetRef {
                id: "asset".into(),
                sha256: "hash".into(),
                mime_type: "image/png".into(),
                byte_len: 8 * 1024 * 1024,
                width: None,
                height: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
                data_url: None,
                remote_object_key: None,
                remote_url: None,
                source_task_id: None,
                metadata: Default::default(),
            }],
        };

        assert_eq!(estimate_generation_memory_permits(&state, &request), 72);
    }

    #[test]
    fn proxy_generation_result_serializes_as_reusable_poll_response() {
        let body = serialize_proxy_generation_result(GenerationResult {
            images: Vec::new(),
            parameter_snapshot: ParameterSnapshot::default(),
            raw_response_json: None,
        })
        .unwrap();
        let response: ProxyGenerationJobResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.status, ProxyGenerationJobStatus::Succeeded);
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn builtin_openai_compatible_template_uses_config_base_url() {
        let builtin = ProviderTemplate::builtin_openai_compatible();
        assert!(builtin.base_url.is_empty());
        assert!(template_uses_configured_base_url(&builtin));

        let mut imported = builtin;
        imported.id = "imported-openai-compatible".into();
        assert!(!template_uses_configured_base_url(&imported));
    }

    #[test]
    fn regular_openai_compatible_endpoints_remain_unchanged() {
        let mut request = GenerationRequest {
            prompt: "test".into(),
            model: "gemini-2.5-flash-image".into(),
            width: 1024,
            height: 1024,
            quality: None,
            count: 1,
            endpoint_mode: ProviderEndpointMode::CustomJson,
            reference_assets: Vec::new(),
        };
        assert_eq!(
            openai_compatible_endpoint(&request),
            "/v1/images/generations"
        );

        request.reference_assets.push(ImageAssetRef {
            id: "asset-1".into(),
            sha256: "hash".into(),
            mime_type: "image/png".into(),
            byte_len: 1,
            width: None,
            height: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: Some("data:image/png;base64,AA==".into()),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        });
        assert_eq!(openai_compatible_endpoint(&request), "/v1/images/edits");
    }

    #[test]
    fn openai_edit_keeps_official_multipart_field_and_skips_image2_fidelity() {
        let mut request = GenerationRequest {
            prompt: "test".into(),
            model: "gpt-image2-vip".into(),
            width: 1024,
            height: 1024,
            quality: None,
            count: 1,
            endpoint_mode: ProviderEndpointMode::ImagesApi,
            reference_assets: Vec::new(),
        };
        assert_eq!(openai_images_endpoint(&request), "/v1/images/generations");
        request.reference_assets.push(ImageAssetRef {
            id: "asset-1".into(),
            sha256: "hash".into(),
            mime_type: "image/png".into(),
            byte_len: 1,
            width: None,
            height: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: Some("data:image/png;base64,AA==".into()),
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        });

        assert_eq!(openai_images_endpoint(&request), "/v1/images/edits");
        assert_eq!(OPENAI_EDIT_IMAGE_FIELD, "image[]");
        assert!(!supports_configurable_input_fidelity(&request.model));
        assert!(supports_configurable_input_fidelity("gpt-image-1.5"));
        assert!(supports_configurable_input_fidelity(
            "openai/gpt-image-1.5-vip"
        ));
    }

    #[test]
    fn data_url_can_be_decoded() {
        let (mime, bytes) = decode_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(mime, "text/plain");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn synced_object_key_must_stay_in_current_user_asset_namespace() {
        assert!(is_user_asset_object_key(
            "users/user-1/assets/hash.bin",
            "user-1",
            "hash"
        ));
        assert!(is_user_asset_object_key(
            "users/user-1/assets/hash-image.png",
            "user-1",
            "hash"
        ));
        assert!(!is_user_asset_object_key(
            "users/user-2/assets/hash.bin",
            "user-1",
            "hash"
        ));
        assert!(!is_user_asset_object_key(
            "users/user-1/assets/hash/other.bin",
            "user-1",
            "hash"
        ));
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
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(239, 1, 1, 1))));
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_private_ip(IpAddr::V6(
            "fd00::1".parse::<Ipv6Addr>().unwrap()
        )));
        assert!(is_private_ip(IpAddr::V6(
            "::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap()
        )));
        assert!(is_private_ip(IpAddr::V6(
            "ff02::1".parse::<Ipv6Addr>().unwrap()
        )));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_private_ip(IpAddr::V6(
            "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()
        )));
    }

    #[test]
    fn upstream_error_sanitization_redacts_credentials_and_controls() {
        let body = br#"{
            "error": {
                "api_key": "another-secret",
                "authorization": "Bearer upstream-token",
                "message": "request failed for caller-key\u0000"
            }
        }"#;
        let sanitized = sanitize_upstream_error_body(body, "caller-key", false);

        assert!(!sanitized.contains("another-secret"));
        assert!(!sanitized.contains("upstream-token"));
        assert!(!sanitized.contains("caller-key"));
        assert!(!sanitized.contains('\0'));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn upstream_error_sanitization_removes_cookies_and_large_encoded_data() {
        let encoded = "A".repeat(512);
        let body = format!("Cookie: session=secret\nupstream payload: {encoded}");
        let sanitized = sanitize_upstream_error_body(body.as_bytes(), "", false);

        assert!(!sanitized.contains("session=secret"));
        assert!(!sanitized.contains(&encoded));
        assert!(sanitized.contains("[LARGE_ENCODED_DATA_REDACTED]"));
    }

    #[test]
    fn image_mime_is_derived_from_magic_bytes() {
        assert_eq!(
            detect_image_mime(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_image_mime(b"\xff\xd8\xffrest"), Some("image/jpeg"));
        assert_eq!(
            detect_image_mime(b"RIFF\x00\x00\x00\x00WEBPrest"),
            Some("image/webp")
        );
        assert_eq!(detect_image_mime(b"GIF89arest"), None);
        assert_eq!(detect_image_mime(b"<html>error</html>"), None);
    }

    #[tokio::test]
    async fn bounded_s3_reader_accepts_exact_limit() {
        let bytes = read_s3_body_bounded(ByteStream::from(vec![1_u8, 2, 3, 4]), 4, Some(4))
            .await
            .unwrap();

        assert_eq!(bytes, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn bounded_s3_reader_rejects_stream_larger_than_limit() {
        let error = read_s3_body_bounded(ByteStream::from(vec![0_u8; 5]), 4, None)
            .await
            .unwrap_err();

        assert!(error.message.contains("读取上限"));
    }

    #[test]
    fn trusted_proxy_headers_are_only_used_when_enabled() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.9, 172.18.0.2".parse().unwrap(),
        );
        headers.insert("x-real-ip", "198.51.100.7".parse().unwrap());
        let peer = SocketAddr::from(([172, 18, 0, 2], 1234));
        let mut config = test_config(String::new());

        assert_eq!(resolve_client_ip(&config, &headers, peer), peer.ip());
        config.trust_proxy_headers = true;
        assert_eq!(resolve_client_ip(&config, &headers, peer), peer.ip());
        config.trusted_proxy_cidrs = vec!["172.18.0.0/16".parse().unwrap()];
        assert_eq!(
            resolve_client_ip(&config, &headers, peer),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[tokio::test]
    async fn auth_rate_limit_blocks_requests_after_window_limit() {
        let db = test_db().await;
        for _ in 0..2 {
            enforce_auth_rate_limit(&db, "login_ip", "ip-hash", 2, 600, "blocked")
                .await
                .unwrap();
        }
        let error = enforce_auth_rate_limit(&db, "login_ip", "ip-hash", 2, 600, "blocked")
            .await
            .unwrap_err();

        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.code, Some("auth_rate_limited"));
        assert!(error.retry_after_seconds.is_some());
    }

    #[tokio::test]
    async fn fifth_failed_login_locks_account_and_resets_counter() {
        let db = test_db().await;
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, status, created_at)
             VALUES ('user-1', 'tester', 'hash', 'user', 'approved', ?)",
        )
        .bind(now_rfc3339())
        .execute(&db)
        .await
        .unwrap();

        for _ in 0..4 {
            assert_eq!(
                record_failed_login(&db, "user-1", 5, 300).await.unwrap(),
                None
            );
        }
        assert_eq!(
            record_failed_login(&db, "user-1", 5, 300).await.unwrap(),
            Some(300)
        );
        let row =
            sqlx::query("SELECT failed_login_count, locked_until FROM users WHERE id = 'user-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(row.get::<i64, _>("failed_login_count"), 0);
        assert!(active_lock_retry_seconds(row.get("locked_until")).is_some());
    }

    #[tokio::test]
    async fn device_registration_limit_is_enforced_atomically() {
        let db = test_db().await;
        for _ in 0..3 {
            let mut transaction = db.begin().await.unwrap();
            reserve_device_registration(&mut transaction, "device-hash", 3, &now_rfc3339())
                .await
                .unwrap();
            transaction.commit().await.unwrap();
        }
        let mut transaction = db.begin().await.unwrap();
        let error = reserve_device_registration(&mut transaction, "device-hash", 3, &now_rfc3339())
            .await
            .unwrap_err();

        assert_eq!(error.code, Some("device_registration_limit"));
    }

    #[tokio::test]
    async fn indexed_asset_can_be_recovered_by_stable_id_when_hash_changed() {
        let db = test_db().await;
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("asset-1")
        .bind("user-1")
        .bind("users/user-1/assets/real-hash.bin")
        .bind("image/png")
        .bind("real-hash")
        .bind(4_i64)
        .bind(now_rfc3339())
        .execute(&db)
        .await
        .unwrap();

        let indexed = find_indexed_asset_object(&db, "user-1", "asset-1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(indexed.0, "users/user-1/assets/real-hash.bin");
        assert_eq!(indexed.3, "real-hash");
    }

    #[tokio::test]
    async fn upload_reservation_enforces_user_byte_quota_atomically() {
        let mut state = test_app_state(String::new()).await;
        state.config.user_asset_quota_bytes = 100;
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES ('asset-a', 'user-a', 'users/user-a/assets/a.bin',
                     'image/png', 'a', 80, ?)",
        )
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let expires_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();
        let reservation = UploadReservation {
            user_id: "user-a",
            asset_id: "asset-b",
            token: "token-b",
            object_key: "users/user-a/assets/b.bin",
            mime_type: "image/png",
            byte_len: 30,
            sha256: "b",
            expires_at: &expires_at,
        };

        let error = reserve_upload_token(&state, reservation).await.unwrap_err();
        assert!(error.message.contains("配额已满"));
        let token_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM upload_tokens")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(token_count, 0);
    }

    #[tokio::test]
    async fn underreported_asset_index_cannot_bypass_user_byte_quota() {
        let mut state = test_app_state(String::new()).await;
        state.config.user_asset_quota_bytes = 100;
        let underreported_object_key = "users/user-a/assets/shared.bin";
        for (asset_id, object_key, sha256, byte_len) in [
            ("asset-a", "users/user-a/assets/a.bin", "a", 80_i64),
            ("asset-shared", underreported_object_key, "shared", 1_i64),
        ] {
            sqlx::query(
                "INSERT INTO assets
                 (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
                 VALUES (?, 'user-a', ?, 'image/png', ?, ?, ?)",
            )
            .bind(asset_id)
            .bind(object_key)
            .bind(sha256)
            .bind(byte_len)
            .bind(now_rfc3339())
            .execute(&state.db)
            .await
            .unwrap();
        }

        let error = ensure_user_asset_capacity(
            &state,
            "user-a",
            "asset-shared",
            underreported_object_key,
            30,
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("配额已满"));

        let expires_at = (Utc::now() + Duration::minutes(15)).to_rfc3339();
        let mut connection = state.db.acquire().await.unwrap();
        let reservation = UploadReservation {
            user_id: "user-a",
            asset_id: "asset-shared",
            token: "direct-token",
            object_key: underreported_object_key,
            mime_type: "image/png",
            byte_len: 30,
            sha256: "shared",
            expires_at: &expires_at,
        };
        let error = reserve_upload_token_on_connection(&mut connection, &state, &reservation)
            .await
            .unwrap_err();
        assert!(error.message.contains("配额已满"));
        drop(connection);

        let reservation = UploadReservation {
            token: "transaction-token",
            ..reservation
        };
        let error = reserve_upload_token(&state, reservation).await.unwrap_err();
        assert!(error.message.contains("配额已满"));
        let token_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM upload_tokens")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(token_count, 0);
    }

    #[tokio::test]
    async fn active_upload_token_is_leased_but_expired_token_cannot_be_revived() {
        let state = test_app_state(String::new()).await;
        let active_expiry = (Utc::now() + Duration::minutes(1)).to_rfc3339();
        let expired_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        for (token, expires_at) in [
            ("active-token", active_expiry.as_str()),
            ("expired-token", expired_at.as_str()),
        ] {
            sqlx::query(
                "INSERT INTO upload_tokens
                 (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
                 VALUES (?, ?, 'user-a', ?, 'image/png', 4, ?, ?)",
            )
            .bind(token)
            .bind(format!("asset-{token}"))
            .bind(format!("users/user-a/assets/{token}.bin"))
            .bind(format!("hash-{token}"))
            .bind(expires_at)
            .execute(&state.db)
            .await
            .unwrap();
        }

        let leased = lease_upload_token(&state, "user-a", "active-token")
            .await
            .unwrap();
        assert!(leased.get::<String, _>("expires_at") > active_expiry);
        assert!(
            lease_upload_token(&state, "user-a", "expired-token")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn expired_upload_cleanup_keeps_live_shared_object_then_removes_it() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        let object_key = "users/user-a/assets/shared.bin";
        put_object(&state, object_key, "image/png", b"image".to_vec())
            .await
            .unwrap();
        for (token, expires_at) in [
            (
                "expired-token",
                (Utc::now() - Duration::minutes(1)).to_rfc3339(),
            ),
            (
                "active-token",
                (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            ),
        ] {
            sqlx::query(
                "INSERT INTO upload_tokens
                 (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
                 VALUES (?, ?, 'user-a', ?, 'image/png', 5, 'shared', ?)",
            )
            .bind(token)
            .bind(format!("asset-{token}"))
            .bind(object_key)
            .bind(expires_at)
            .execute(&state.db)
            .await
            .unwrap();
        }

        cleanup_expired_upload_tokens(&state).await.unwrap();
        assert!(object_exists(&state, object_key).await.unwrap());
        let remaining = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM upload_tokens")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(remaining, 1);

        sqlx::query("UPDATE upload_tokens SET expires_at = ?")
            .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
            .execute(&state.db)
            .await
            .unwrap();
        cleanup_expired_upload_tokens(&state).await.unwrap();
        assert!(!object_exists(&state, object_key).await.unwrap());
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn expired_upload_cleanup_retries_when_object_store_is_unavailable() {
        let mut state = test_app_state(String::new()).await;
        state.config.asset_store = AssetStoreKind::S3;
        sqlx::query(
            "INSERT INTO upload_tokens
             (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
             VALUES ('expired-token', 'asset-a', 'user-a', 'users/user-a/assets/a.bin',
                     'image/png', 1, 'a', ?)",
        )
        .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        cleanup_expired_upload_tokens(&state).await.unwrap();
        let remaining = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM upload_tokens")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn sync_remote_object_uses_trusted_server_metadata() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        let bytes = test_png_bytes(b"trusted-index");
        let sha256 = hex_sha256(&bytes);
        let object_key = format!("users/user-1/assets/{sha256}.bin");
        put_object(&state, &object_key, "image/png", bytes.clone())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES ('server-asset', 'user-1', ?, 'image/png', ?, ?, ?)",
        )
        .bind(&object_key)
        .bind(&sha256)
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let envelope = SyncEnvelope {
            assets: vec![test_sync_asset(
                "client-alias",
                &sha256,
                "image/jpeg",
                1,
                None,
                Some(object_key),
            )],
            ..SyncEnvelope::default()
        };

        let normalized = normalize_envelope_assets(&state, "user-1", envelope)
            .await
            .unwrap();
        let asset = &normalized.envelope.assets[0];
        assert_eq!(asset.mime_type, "image/png");
        assert_eq!(asset.byte_len, bytes.len() as u64);
        let stored = find_indexed_asset_object(&state.db, "user-1", "client-alias")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.1, "image/png");
        assert_eq!(stored.2, bytes.len() as i64);
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn sync_rejects_excessive_asset_count_before_storage_work() {
        let state = test_app_state(String::new()).await;
        let asset = test_sync_asset("asset", "hash", "image/png", 1, None, None);
        let envelope = SyncEnvelope {
            assets: vec![asset; MAX_SYNC_ASSETS + 1],
            ..SyncEnvelope::default()
        };

        let error = normalize_envelope_assets(&state, "user-1", envelope)
            .await
            .unwrap_err();
        assert!(error.message.contains("最多包含"));
    }

    #[tokio::test]
    async fn sync_unindexed_remote_object_validates_actual_metadata_and_quota() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let mut state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        let bytes = test_png_bytes(b"unindexed-object");
        let sha256 = hex_sha256(&bytes);
        let object_key = format!("users/user-1/assets/{sha256}.bin");
        put_object(&state, &object_key, "image/png", bytes.clone())
            .await
            .unwrap();
        let spoofed_asset = || {
            test_sync_asset(
                "unindexed-asset",
                &sha256,
                "image/jpeg",
                1,
                None,
                Some(object_key.clone()),
            )
        };

        state.config.user_asset_quota_bytes = bytes.len() as u64 - 1;
        let error = normalize_envelope_assets(
            &state,
            "user-1",
            SyncEnvelope {
                assets: vec![spoofed_asset()],
                ..SyncEnvelope::default()
            },
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("配额已满"));

        state.config.user_asset_quota_bytes = bytes.len() as u64;
        let normalized = normalize_envelope_assets(
            &state,
            "user-1",
            SyncEnvelope {
                assets: vec![spoofed_asset()],
                ..SyncEnvelope::default()
            },
        )
        .await
        .unwrap();
        let asset = &normalized.envelope.assets[0];
        assert_eq!(asset.mime_type, "image/png");
        assert_eq!(asset.byte_len, bytes.len() as u64);
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn sync_normalization_rolls_back_prior_assets_when_later_asset_fails() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;

        let old_bytes = test_png_bytes(b"old-index");
        let old_sha256 = hex_sha256(&old_bytes);
        let old_object_key = format!("users/user-1/assets/{old_sha256}.bin");
        put_object(&state, &old_object_key, "image/png", old_bytes.clone())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES ('overwritten-asset', 'user-1', ?, 'image/png', ?, ?, ?)",
        )
        .bind(&old_object_key)
        .bind(&old_sha256)
        .bind(i64::try_from(old_bytes.len()).unwrap())
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let remote_bytes = test_png_bytes(b"replacement-index");
        let remote_sha256 = hex_sha256(&remote_bytes);
        let remote_object_key = format!("users/user-1/assets/{remote_sha256}.bin");
        put_object(&state, &remote_object_key, "image/png", remote_bytes)
            .await
            .unwrap();
        let new_bytes = test_png_bytes(b"new-object");
        let new_sha256 = hex_sha256(&new_bytes);
        let new_object_key = format!("users/user-1/assets/{new_sha256}.bin");
        let invalid_bytes = test_png_bytes(b"invalid-hash");
        let envelope = SyncEnvelope {
            assets: vec![
                test_sync_asset(
                    "new-asset",
                    &new_sha256,
                    "image/png",
                    new_bytes.len() as u64,
                    Some(format!(
                        "data:image/png;base64,{}",
                        BASE64.encode(&new_bytes)
                    )),
                    None,
                ),
                test_sync_asset(
                    "overwritten-asset",
                    &remote_sha256,
                    "image/jpeg",
                    1,
                    None,
                    Some(remote_object_key.clone()),
                ),
                test_sync_asset(
                    "invalid-asset",
                    "not-the-real-hash",
                    "image/png",
                    invalid_bytes.len() as u64,
                    Some(format!(
                        "data:image/png;base64,{}",
                        BASE64.encode(invalid_bytes)
                    )),
                    None,
                ),
            ],
            ..SyncEnvelope::default()
        };

        assert!(
            normalize_envelope_assets(&state, "user-1", envelope)
                .await
                .is_err()
        );
        assert!(
            find_indexed_asset_object(&state.db, "user-1", "new-asset")
                .await
                .unwrap()
                .is_none()
        );
        let restored = find_indexed_asset_object(&state.db, "user-1", "overwritten-asset")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.0, old_object_key);
        assert_eq!(restored.3, old_sha256);
        assert!(!object_exists(&state, &new_object_key).await.unwrap());
        assert!(object_exists(&state, &remote_object_key).await.unwrap());
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn sync_normalization_discards_index_when_object_file_is_missing() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        let object_key = "users/user-1/assets/hash-1.bin";
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("asset-1")
        .bind("user-1")
        .bind(object_key)
        .bind("image/png")
        .bind("hash")
        .bind(4_i64)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let old_updated_at = "2026-01-01T00:00:00+00:00".to_string();
        let envelope = SyncEnvelope {
            assets: vec![ImageAssetRef {
                id: "asset-1".into(),
                sha256: "hash".into(),
                mime_type: "image/png".into(),
                byte_len: 4,
                width: None,
                height: None,
                created_at: old_updated_at.clone(),
                updated_at: old_updated_at.clone(),
                data_url: None,
                remote_object_key: Some(object_key.into()),
                remote_url: Some("/api/assets/asset-1".into()),
                source_task_id: None,
                metadata: Default::default(),
            }],
            ..SyncEnvelope::default()
        };

        let normalized = normalize_envelope_assets(&state, "user-1", envelope)
            .await
            .unwrap();
        let asset = &normalized.envelope.assets[0];
        assert!(asset.remote_object_key.is_none());
        assert!(asset.remote_url.is_none());
        assert!(asset.updated_at > old_updated_at);
        let indexed_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets WHERE user_id = 'user-1'")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(indexed_count, 0);
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

        let result =
            extract_openai_responses_result(&request, &response_json, Some("png")).unwrap();
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
    async fn temporary_reference_is_verified_encoded_and_removed_on_drop() {
        let path = std::env::temp_dir().join(format!("mew-reference-test-{}.part", new_id()));
        let bytes = b"\x89PNG\r\n\x1a\nreference";
        tokio::fs::write(&path, bytes).await.unwrap();
        let temporary_file = TemporaryReferenceFile {
            path: path.clone(),
            mime_type: "image/png".into(),
            byte_len: bytes.len() as u64,
            sha256: hex_sha256(bytes),
        };
        let asset = ImageAssetRef {
            id: "reference-1".into(),
            sha256: temporary_file.sha256.clone(),
            mime_type: temporary_file.mime_type.clone(),
            byte_len: temporary_file.byte_len,
            width: None,
            height: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            data_url: None,
            remote_object_key: None,
            remote_url: None,
            source_task_id: None,
            metadata: Default::default(),
        };

        let data_url = load_temporary_reference_data_url(&asset, &temporary_file)
            .await
            .unwrap();
        assert_eq!(
            data_url,
            format!("data:image/png;base64,{}", BASE64.encode(bytes))
        );
        tokio::fs::write(&path, b"tampered").await.unwrap();
        assert!(
            load_temporary_reference_data_url(&asset, &temporary_file)
                .await
                .is_err()
        );
        drop(temporary_file);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn tombstone_cleanup_keeps_shared_object_until_last_reference_is_deleted() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
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

    #[tokio::test]
    async fn tombstone_cleanup_keeps_object_referenced_by_an_active_upload() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        let object_key = "users/user-1/assets/active-upload.bin";
        put_object(&state, object_key, "image/png", b"image".to_vec())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES ('deleted-asset', 'user-1', ?, 'image/png', 'shared', 5, ?)",
        )
        .bind(object_key)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO upload_tokens
             (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
             VALUES ('active-token', 'uploading-asset', 'user-1', ?, 'image/png', 5, 'shared', ?)",
        )
        .bind(object_key)
        .bind((Utc::now() + Duration::minutes(5)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let envelope = SyncEnvelope {
            tombstones: vec![SyncTombstone {
                entity_kind: SyncEntityKind::Asset,
                entity_id: "deleted-asset".into(),
                deleted_at: now_rfc3339(),
            }],
            ..SyncEnvelope::default()
        };

        cleanup_tombstoned_assets(&state, "user-1", &envelope)
            .await
            .unwrap();
        assert!(object_exists(&state, object_key).await.unwrap());
        let active_token_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM upload_tokens WHERE token = 'active-token'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(active_token_count, 1);
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn sync_push_returns_committed_snapshot_when_tombstone_cleanup_needs_retry() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        insert_test_user(
            &state.db,
            "user-sync-cleanup",
            "sync-cleanup-user",
            "hash",
            "user",
            "approved",
            0,
        )
        .await;
        let object_key = "users/user-sync-cleanup/assets/retry.bin";
        let object_path = local_object_path(&state.config.local_asset_dir, object_key).unwrap();
        // 用目录模拟暂时无法删除的对象，确保快照提交不会被后置清理伪装成失败。
        tokio::fs::create_dir_all(&object_path).await.unwrap();
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES ('asset-retry', 'user-sync-cleanup', ?, 'image/png', 'retry', 1, ?)",
        )
        .bind(object_key)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let state = Arc::new(state);
        let session =
            authenticated_test_session(Arc::new(MemoryStore::default()), "user-sync-cleanup", 0)
                .await;

        let response = sync_push(
            State(state.clone()),
            session,
            Json(SyncPushRequest {
                client_updated_at: now_rfc3339(),
                envelope: SyncEnvelope {
                    tombstones: vec![SyncTombstone {
                        entity_kind: SyncEntityKind::Asset,
                        entity_id: "asset-retry".into(),
                        deleted_at: now_rfc3339(),
                    }],
                    ..SyncEnvelope::default()
                },
            }),
        )
        .await
        .unwrap();

        assert!(
            response
                .envelope
                .tombstones
                .iter()
                .any(|item| item.entity_id == "asset-retry")
        );
        let indexed_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets WHERE id = 'asset-retry'")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(indexed_count, 1, "失败的清理记录应保留供后续重试");
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn sync_push_rolls_back_normalized_assets_when_snapshot_merge_fails() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        insert_test_user(
            &state.db,
            "user-sync-rollback",
            "sync-rollback-user",
            "hash",
            "user",
            "approved",
            0,
        )
        .await;
        sqlx::query("INSERT INTO sync_snapshots (user_id, payload, updated_at) VALUES (?, ?, ?)")
            .bind("user-sync-rollback")
            .bind("not-valid-json")
            .bind(now_rfc3339())
            .execute(&state.db)
            .await
            .unwrap();
        let bytes = test_png_bytes(b"merge-failure");
        let sha256 = hex_sha256(&bytes);
        let object_key = format!("users/user-sync-rollback/assets/{sha256}.bin");
        let envelope = SyncEnvelope {
            assets: vec![test_sync_asset(
                "merge-failure-asset",
                &sha256,
                "image/png",
                bytes.len() as u64,
                Some(format!("data:image/png;base64,{}", BASE64.encode(bytes))),
                None,
            )],
            ..SyncEnvelope::default()
        };
        let state = Arc::new(state);
        let session =
            authenticated_test_session(Arc::new(MemoryStore::default()), "user-sync-rollback", 0)
                .await;

        assert!(
            sync_push(
                State(state.clone()),
                session,
                Json(SyncPushRequest {
                    client_updated_at: now_rfc3339(),
                    envelope,
                }),
            )
            .await
            .is_err()
        );
        assert!(
            find_indexed_asset_object(&state.db, "user-sync-rollback", "merge-failure-asset")
                .await
                .unwrap()
                .is_none()
        );
        assert!(!object_exists(&state, &object_key).await.unwrap());
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }

    #[tokio::test]
    async fn upload_complete_returns_committed_asset_when_old_object_cleanup_needs_retry() {
        let asset_dir = std::env::temp_dir().join(format!("mew-image-test-{}", new_id()));
        let state = test_app_state(asset_dir.to_string_lossy().into_owned()).await;
        insert_test_user(
            &state.db,
            "user-upload-cleanup",
            "upload-cleanup-user",
            "hash",
            "user",
            "approved",
            0,
        )
        .await;
        let asset_id = new_id();
        let bytes = b"\x89PNG\r\n\x1a\nverified".to_vec();
        let sha256 = hex_sha256(&bytes);
        let old_object_key = "users/user-upload-cleanup/assets/old.bin";
        let new_object_key = format!("users/user-upload-cleanup/assets/{sha256}.bin");
        put_object(&state, &new_object_key, "image/png", bytes.clone())
            .await
            .unwrap();
        let old_path = local_object_path(&state.config.local_asset_dir, old_object_key).unwrap();
        tokio::fs::create_dir_all(&old_path).await.unwrap();
        sqlx::query(
            "INSERT INTO assets
             (id, user_id, object_key, mime_type, sha256, byte_len, created_at)
             VALUES (?, 'user-upload-cleanup', ?, 'image/png', 'old', 1, ?)",
        )
        .bind(&asset_id)
        .bind(old_object_key)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO upload_tokens
             (token, asset_id, user_id, object_key, mime_type, byte_len, sha256, expires_at)
             VALUES ('cleanup-token', ?, 'user-upload-cleanup', ?, 'image/png', ?, ?, ?)",
        )
        .bind(&asset_id)
        .bind(&new_object_key)
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(&sha256)
        .bind((Utc::now() + Duration::minutes(5)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
        let state = Arc::new(state);
        let session =
            authenticated_test_session(Arc::new(MemoryStore::default()), "user-upload-cleanup", 0)
                .await;

        let response = upload_complete(
            State(state.clone()),
            session,
            Json(UploadCompleteRequest {
                upload_token: "cleanup-token".into(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            response.asset.remote_object_key.as_deref(),
            Some(new_object_key.as_str())
        );
        let stored_key =
            sqlx::query_scalar::<_, String>("SELECT object_key FROM assets WHERE id = ?")
                .bind(&asset_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(stored_key, new_object_key);
        let token_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM upload_tokens WHERE token = 'cleanup-token'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(token_count, 0);
        let _ = tokio::fs::remove_dir_all(asset_dir).await;
    }
}
