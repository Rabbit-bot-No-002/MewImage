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
            listen_addr: env_value("MEW_LISTEN", "MEW_IMAGE_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: env_value("MEW_DATABASE_URL", "MEW_IMAGE_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://./data/mew-image.db?mode=rwc".into()),
            frontend_dist: env_value("MEW_FRONTEND_DIST", "MEW_IMAGE_FRONTEND_DIST")
                .unwrap_or_else(|_| "./frontend/dist-app".into()),
            session_secure: env_value("MEW_SESSION_SECURE", "MEW_IMAGE_SESSION_SECURE")
                .map(|value| value == "true")
                .unwrap_or(false),
            allowed_web_origins: parse_csv_env(
                "MEW_ALLOWED_ORIGINS",
                "MEW_IMAGE_ALLOWED_WEB_ORIGINS",
            ),
            trusted_provider_hosts: parse_csv_env(
                "MEW_TRUSTED_HOSTS",
                "MEW_IMAGE_TRUSTED_PROVIDER_HOSTS",
            ),
            enforce_provider_host_whitelist: env_value(
                "MEW_ENFORCE_HOST_WHITELIST",
                "MEW_IMAGE_ENFORCE_PROVIDER_HOST_WHITELIST",
            )
            .map(|value| value == "true")
            .unwrap_or(false),
            enable_guest_proxy: env_value("MEW_GUEST_PROXY", "MEW_IMAGE_ENABLE_GUEST_PROXY")
                .map(|value| value != "false")
                .unwrap_or(true),
            require_login_for_custom_provider: env_value(
                "MEW_CUSTOM_PROVIDER_LOGIN",
                "MEW_IMAGE_REQUIRE_LOGIN_FOR_CUSTOM_PROVIDER",
            )
            .map(|value| value != "false")
            .unwrap_or(true),
            admin_setup_token: env_value("MEW_ADMIN_TOKEN", "MEW_IMAGE_ADMIN_SETUP_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            allow_first_admin_setup: env_value(
                "MEW_ALLOW_ADMIN_SETUP",
                "MEW_IMAGE_ALLOW_FIRST_ADMIN_SETUP",
            )
            .map(|value| value != "false")
            .unwrap_or(true),
            asset_store: env_value("MEW_ASSET_STORE", "MEW_IMAGE_ASSET_STORE")
                .ok()
                .map(|value| AssetStoreKind::from_env_value(&value))
                .unwrap_or_else(|| {
                    if env_value("MEW_S3_BUCKET", "MEW_IMAGE_S3_BUCKET")
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    {
                        AssetStoreKind::S3
                    } else {
                        AssetStoreKind::Local
                    }
                }),
            local_asset_dir: env_value("MEW_LOCAL_ASSET_DIR", "MEW_IMAGE_LOCAL_ASSET_DIR")
                .unwrap_or_else(|_| "./data/assets".into()),
            s3_bucket: env_value("MEW_S3_BUCKET", "MEW_IMAGE_S3_BUCKET").unwrap_or_default(),
            s3_region: env_value("MEW_S3_REGION", "MEW_IMAGE_S3_REGION")
                .unwrap_or_else(|_| "auto".into()),
            s3_endpoint: env_value("MEW_S3_ENDPOINT", "MEW_IMAGE_S3_ENDPOINT").ok(),
            s3_access_key: env_value("MEW_S3_ACCESS_KEY", "MEW_IMAGE_S3_ACCESS_KEY").ok(),
            s3_secret_key: env_value("MEW_S3_SECRET_KEY", "MEW_IMAGE_S3_SECRET_KEY").ok(),
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

fn env_value(short_key: &str, legacy_key: &str) -> Result<String, std::env::VarError> {
    std::env::var(short_key).or_else(|_| std::env::var(legacy_key))
}

fn parse_csv_env(short_key: &str, legacy_key: &str) -> Vec<String> {
    env_value(short_key, legacy_key)
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
