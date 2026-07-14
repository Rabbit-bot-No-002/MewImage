use aws_sdk_s3::Client as S3Client;
use mew_image_shared::ProviderTemplate;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetStoreKind {
    Disabled,
    Local,
    S3,
}

impl AssetStoreKind {
    fn from_env_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Self::Local,
            "s3" => Self::S3,
            "disabled" | "none" | "off" => Self::Disabled,
            _ => Self::Disabled,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub listen_addr: String,
    pub database_url: String,
    pub frontend_dist: String,
    pub session_secure: bool,
    pub allowed_web_origins: Vec<String>,
    pub trusted_provider_hosts: Vec<String>,
    pub enforce_provider_host_whitelist: bool,
    pub enable_guest_proxy: bool,
    pub require_login_for_custom_provider: bool,
    pub admin_setup_token: Option<String>,
    pub allow_first_admin_setup: bool,
    pub asset_store: AssetStoreKind,
    pub local_asset_dir: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_endpoint: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            listen_addr: std::env::var("MEW_IMAGE_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("MEW_IMAGE_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://./data/mew-image.db?mode=rwc".into()),
            frontend_dist: std::env::var("MEW_IMAGE_FRONTEND_DIST")
                .unwrap_or_else(|_| "./frontend/dist-app".into()),
            session_secure: std::env::var("MEW_IMAGE_SESSION_SECURE")
                .map(|value| value == "true")
                .unwrap_or(false),
            allowed_web_origins: parse_csv_env("MEW_IMAGE_ALLOWED_WEB_ORIGINS"),
            trusted_provider_hosts: parse_csv_env("MEW_IMAGE_TRUSTED_PROVIDER_HOSTS"),
            enforce_provider_host_whitelist: std::env::var(
                "MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST",
            )
            .map(|value| value == "true")
            .unwrap_or(false),
            enable_guest_proxy: std::env::var("MEW_IMAGE_ENABLE_GUEST_PROXY")
                .map(|value| value != "false")
                .unwrap_or(true),
            require_login_for_custom_provider: std::env::var(
                "MEW_IMAGE_REQUIRE_LOGIN_FOR_CUSTOM_PROVIDER",
            )
            .map(|value| value != "false")
            .unwrap_or(true),
            admin_setup_token: std::env::var("MEW_IMAGE_ADMIN_SETUP_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            allow_first_admin_setup: std::env::var("MEW_IMAGE_ALLOW_FIRST_ADMIN_SETUP")
                .map(|value| value != "false")
                .unwrap_or(true),
            asset_store: std::env::var("MEW_IMAGE_ASSET_STORE")
                .ok()
                .map(|value| AssetStoreKind::from_env_value(&value))
                .unwrap_or_else(|| {
                    if std::env::var("MEW_IMAGE_S3_BUCKET")
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    {
                        AssetStoreKind::S3
                    } else {
                        AssetStoreKind::Local
                    }
                }),
            local_asset_dir: std::env::var("MEW_IMAGE_LOCAL_ASSET_DIR")
                .unwrap_or_else(|_| "./data/assets".into()),
            s3_bucket: std::env::var("MEW_IMAGE_S3_BUCKET").unwrap_or_default(),
            s3_region: std::env::var("MEW_IMAGE_S3_REGION").unwrap_or_else(|_| "auto".into()),
            s3_endpoint: std::env::var("MEW_IMAGE_S3_ENDPOINT").ok(),
            s3_access_key: std::env::var("MEW_IMAGE_S3_ACCESS_KEY").ok(),
            s3_secret_key: std::env::var("MEW_IMAGE_S3_SECRET_KEY").ok(),
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub db: SqlitePool,
    pub s3: Option<S3Client>,
    pub http: reqwest::Client,
    pub provider_builtins: Vec<ProviderTemplate>,
}

fn parse_csv_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
