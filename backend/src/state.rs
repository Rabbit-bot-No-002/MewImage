use aws_sdk_s3::Client as S3Client;
use bytes::Bytes;
use ipnet::IpNet;
use mew_image_shared::{ProviderTemplate, new_id};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const GUEST_LIMIT_STATE_TTL: Duration = Duration::from_secs(20 * 60);

pub enum ProxyGenerationJobState {
    Queued,
    Running,
    Succeeded(Bytes),
    Failed(String),
}

impl ProxyGenerationJobState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded(_) | Self::Failed(_))
    }
}

pub struct ProxyGenerationJob {
    pub state: ProxyGenerationJobState,
    pub updated_at: Instant,
    pub abort_handle: Option<tokio::task::AbortHandle>,
    // 成功结果被浏览器确认或 TTL 清理前继续占用预算，避免缓存结果绕过内存限制。
    pub memory_permit: Option<OwnedSemaphorePermit>,
}

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
    pub trust_proxy_headers: bool,
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub auth_secret: String,
    pub register_device_limit: u32,
    pub register_ip_limit: u32,
    pub register_window_seconds: u64,
    pub login_ip_limit: u32,
    pub login_window_seconds: u64,
    pub login_failure_limit: u32,
    pub login_lock_seconds: u64,
    pub auth_hash_concurrency: usize,
    pub allowed_web_origins: Vec<String>,
    pub trusted_provider_hosts: Vec<String>,
    pub enforce_provider_host_whitelist: bool,
    pub allow_insecure_upstreams: bool,
    pub enable_guest_proxy: bool,
    pub guest_generation_concurrency: u32,
    pub guest_image_concurrency: u32,
    pub guest_generation_rate_limit: u32,
    pub guest_image_rate_limit: u32,
    pub guest_rate_window_seconds: u64,
    pub proxy_memory_budget_mib: usize,
    pub admin_setup_token: Option<String>,
    pub allow_first_admin_setup: bool,
    pub asset_store: AssetStoreKind,
    pub local_asset_dir: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_endpoint: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub max_upload_bytes: u64,
    pub user_asset_quota_bytes: u64,
    pub user_asset_quota_count: u64,
    pub user_pending_upload_bytes: u64,
    pub user_pending_upload_count: u64,
    pub gallery_asset_quota_bytes: u64,
    pub gallery_asset_quota_count: u64,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let admin_setup_token = env_value("MEW_ADMIN_TOKEN", "MEW_IMAGE_ADMIN_SETUP_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let auth_secret = env_value("MEW_AUTH_SECRET", "MEW_IMAGE_AUTH_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| admin_setup_token.clone())
            .unwrap_or_else(|| format!("{}{}", new_id(), new_id()));
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
            trust_proxy_headers: env_value(
                "MEW_TRUST_PROXY_HEADERS",
                "MEW_IMAGE_TRUST_PROXY_HEADERS",
            )
            .map(|value| value == "true")
            .unwrap_or(false),
            trusted_proxy_cidrs: parse_cidr_env(
                "MEW_TRUSTED_PROXY_CIDRS",
                "MEW_IMAGE_TRUSTED_PROXY_CIDRS",
            )?,
            auth_secret,
            register_device_limit: parse_u32_env(
                "MEW_REGISTER_DEVICE_LIMIT",
                "MEW_IMAGE_REGISTER_DEVICE_LIMIT",
                3,
            ),
            register_ip_limit: parse_u32_env(
                "MEW_REGISTER_IP_LIMIT",
                "MEW_IMAGE_REGISTER_IP_LIMIT",
                10,
            ),
            register_window_seconds: parse_u64_env(
                "MEW_REGISTER_WINDOW_SECONDS",
                "MEW_IMAGE_REGISTER_WINDOW_SECONDS",
                86_400,
            ),
            login_ip_limit: parse_u32_env("MEW_LOGIN_IP_LIMIT", "MEW_IMAGE_LOGIN_IP_LIMIT", 20),
            login_window_seconds: parse_u64_env(
                "MEW_LOGIN_WINDOW_SECONDS",
                "MEW_IMAGE_LOGIN_WINDOW_SECONDS",
                600,
            ),
            login_failure_limit: parse_u32_env(
                "MEW_LOGIN_FAILURE_LIMIT",
                "MEW_IMAGE_LOGIN_FAILURE_LIMIT",
                5,
            ),
            login_lock_seconds: parse_u64_env(
                "MEW_LOGIN_LOCK_SECONDS",
                "MEW_IMAGE_LOGIN_LOCK_SECONDS",
                300,
            ),
            auth_hash_concurrency: parse_usize_env(
                "MEW_AUTH_HASH_CONCURRENCY",
                "MEW_IMAGE_AUTH_HASH_CONCURRENCY",
                2,
            )
            .max(1),
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
            allow_insecure_upstreams: std::env::var("MEW_ALLOW_HTTP_UPSTREAM")
                .or_else(|_| std::env::var("MEW_ALLOW_HTTP_UPSTREAMS"))
                .or_else(|_| std::env::var("MEW_IMAGE_ALLOW_HTTP_UPSTREAM"))
                .or_else(|_| std::env::var("MEW_IMAGE_ALLOW_INSECURE_UPSTREAMS"))
                .map(|value| value == "true")
                .unwrap_or(false),
            enable_guest_proxy: env_value("MEW_GUEST_PROXY", "MEW_IMAGE_ENABLE_GUEST_PROXY")
                .map(|value| value != "false")
                .unwrap_or(true),
            guest_generation_concurrency: parse_u32_env_aliases(
                &[
                    "MEW_GUEST_GENERATION_MAX_ACTIVE",
                    "MEW_GUEST_GENERATION_CONCURRENCY",
                    "MEW_IMAGE_GUEST_GENERATION_MAX_ACTIVE",
                    "MEW_IMAGE_GUEST_GENERATION_CONCURRENCY",
                ],
                4,
            )
            .max(1),
            guest_image_concurrency: parse_u32_env_aliases(
                &[
                    "MEW_GUEST_IMAGE_FETCH_MAX_ACTIVE",
                    "MEW_GUEST_IMAGE_CONCURRENCY",
                    "MEW_IMAGE_GUEST_IMAGE_FETCH_MAX_ACTIVE",
                    "MEW_IMAGE_GUEST_IMAGE_CONCURRENCY",
                ],
                2,
            )
            .max(1),
            guest_generation_rate_limit: parse_u32_env(
                "MEW_GUEST_GENERATION_RATE_LIMIT",
                "MEW_IMAGE_GUEST_GENERATION_RATE_LIMIT",
                30,
            ),
            guest_image_rate_limit: parse_u32_env_aliases(
                &[
                    "MEW_GUEST_IMAGE_FETCH_RATE_LIMIT",
                    "MEW_GUEST_IMAGE_RATE_LIMIT",
                    "MEW_IMAGE_GUEST_IMAGE_FETCH_RATE_LIMIT",
                    "MEW_IMAGE_GUEST_IMAGE_RATE_LIMIT",
                ],
                120,
            ),
            guest_rate_window_seconds: parse_u64_env(
                "MEW_GUEST_RATE_WINDOW_SECONDS",
                "MEW_IMAGE_GUEST_RATE_WINDOW_SECONDS",
                600,
            )
            .max(1),
            proxy_memory_budget_mib: parse_usize_env(
                "MEW_PROXY_MEMORY_BUDGET_MIB",
                "MEW_IMAGE_PROXY_MEMORY_BUDGET_MIB",
                default_proxy_memory_budget_mib(),
            )
            .clamp(64, 4096),
            admin_setup_token,
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
            max_upload_bytes: parse_mib_or_bytes_env(
                "MEW_MAX_ASSET_MIB",
                "MEW_IMAGE_MAX_ASSET_MIB",
                "MEW_MAX_UPLOAD_BYTES",
                "MEW_IMAGE_MAX_UPLOAD_BYTES",
                64,
            ),
            user_asset_quota_bytes: parse_mib_or_bytes_env(
                "MEW_USER_ASSET_QUOTA_MIB",
                "MEW_IMAGE_USER_ASSET_QUOTA_MIB",
                "MEW_USER_ASSET_QUOTA_BYTES",
                "MEW_IMAGE_USER_ASSET_QUOTA_BYTES",
                5 * 1024,
            ),
            user_asset_quota_count: parse_u64_env(
                "MEW_USER_ASSET_QUOTA_COUNT",
                "MEW_IMAGE_USER_ASSET_QUOTA_COUNT",
                20_000,
            ),
            user_pending_upload_bytes: parse_u64_env(
                "MEW_USER_PENDING_UPLOAD_BYTES",
                "MEW_IMAGE_USER_PENDING_UPLOAD_BYTES",
                256 * 1024 * 1024,
            ),
            user_pending_upload_count: parse_u64_env(
                "MEW_USER_PENDING_UPLOAD_COUNT",
                "MEW_IMAGE_USER_PENDING_UPLOAD_COUNT",
                32,
            ),
            gallery_asset_quota_bytes: parse_mib_or_bytes_env(
                "MEW_GALLERY_ASSET_QUOTA_MIB",
                "MEW_IMAGE_GALLERY_ASSET_QUOTA_MIB",
                "MEW_GALLERY_ASSET_QUOTA_BYTES",
                "MEW_IMAGE_GALLERY_ASSET_QUOTA_BYTES",
                5 * 1024,
            ),
            gallery_asset_quota_count: parse_u64_env(
                "MEW_GALLERY_ASSET_QUOTA_COUNT",
                "MEW_IMAGE_GALLERY_ASSET_QUOTA_COUNT",
                20_000,
            ),
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub db: SqlitePool,
    pub s3: Option<S3Client>,
    pub provider_builtins: Vec<ProviderTemplate>,
    pub generation_job_slots: Arc<Semaphore>,
    // 控制排队参考图临时文件的总量；该许可不代表常驻内存。
    pub generation_temp_budget: Arc<Semaphore>,
    pub generation_memory_budget: Arc<Semaphore>,
    pub generation_jobs: Arc<Mutex<HashMap<String, ProxyGenerationJob>>>,
    pub user_data_write_locks: Arc<Vec<Mutex<()>>>,
    pub gallery_write_lock: Arc<Mutex<()>>,
    pub auth_hash_semaphore: Arc<Semaphore>,
    pub dummy_password_hash: String,
    pub guest_proxy_limits: Arc<GuestProxyLimits>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestProxyOperation {
    Generation,
    ImageFetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestLimitRejection {
    Concurrent,
    RateLimited { retry_after_seconds: u64 },
}

#[derive(Debug)]
struct GuestProxyIpState {
    window_started_at: Instant,
    generation_attempts: u32,
    image_attempts: u32,
    active_generations: u32,
    active_images: u32,
    last_seen_at: Instant,
}

impl GuestProxyIpState {
    fn new(now: Instant) -> Self {
        Self {
            window_started_at: now,
            generation_attempts: 0,
            image_attempts: 0,
            active_generations: 0,
            active_images: 0,
            last_seen_at: now,
        }
    }
}

#[derive(Debug, Default)]
pub struct GuestProxyLimits {
    entries: Arc<StdMutex<HashMap<IpAddr, GuestProxyIpState>>>,
}

pub struct GuestProxyPermit {
    entries: Arc<StdMutex<HashMap<IpAddr, GuestProxyIpState>>>,
    ip: IpAddr,
    operation: GuestProxyOperation,
}

impl GuestProxyLimits {
    pub fn acquire(
        &self,
        ip: IpAddr,
        operation: GuestProxyOperation,
        concurrency_limit: u32,
        request_limit: u32,
        window: Duration,
    ) -> Result<GuestProxyPermit, GuestLimitRejection> {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|_, entry| {
            entry.active_generations > 0
                || entry.active_images > 0
                || now.saturating_duration_since(entry.last_seen_at) < GUEST_LIMIT_STATE_TTL
        });

        let entry = entries
            .entry(ip)
            .or_insert_with(|| GuestProxyIpState::new(now));
        entry.last_seen_at = now;
        if now.saturating_duration_since(entry.window_started_at) >= window {
            entry.window_started_at = now;
            entry.generation_attempts = 0;
            entry.image_attempts = 0;
        }

        let (active, attempts) = match operation {
            GuestProxyOperation::Generation => (
                &mut entry.active_generations,
                &mut entry.generation_attempts,
            ),
            GuestProxyOperation::ImageFetch => {
                (&mut entry.active_images, &mut entry.image_attempts)
            }
        };
        if *active >= concurrency_limit {
            return Err(GuestLimitRejection::Concurrent);
        }
        if request_limit > 0 && *attempts >= request_limit {
            let elapsed = now.saturating_duration_since(entry.window_started_at);
            return Err(GuestLimitRejection::RateLimited {
                retry_after_seconds: window.saturating_sub(elapsed).as_secs().max(1),
            });
        }

        if request_limit > 0 {
            *attempts += 1;
        }
        *active += 1;
        Ok(GuestProxyPermit {
            entries: self.entries.clone(),
            ip,
            operation,
        })
    }
}

impl Drop for GuestProxyPermit {
    fn drop(&mut self) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(entry) = entries.get_mut(&self.ip) else {
            return;
        };
        let active = match self.operation {
            GuestProxyOperation::Generation => &mut entry.active_generations,
            GuestProxyOperation::ImageFetch => &mut entry.active_images,
        };
        *active = active.saturating_sub(1);
        entry.last_seen_at = Instant::now();
    }
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

fn parse_cidr_env(short_key: &str, legacy_key: &str) -> anyhow::Result<Vec<IpNet>> {
    parse_csv_env(short_key, legacy_key)
        .into_iter()
        .map(|value| {
            value
                .parse::<IpNet>()
                .map_err(|error| anyhow::anyhow!("{short_key} 包含无效 CIDR `{value}`：{error}"))
        })
        .collect()
}

fn parse_u32_env(short_key: &str, legacy_key: &str, default: u32) -> u32 {
    env_value(short_key, legacy_key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn parse_u32_env_aliases(keys: &[&str], default: u32) -> u32 {
    keys.iter()
        .find_map(|key| std::env::var(key).ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn parse_u64_env(short_key: &str, legacy_key: &str, default: u64) -> u64 {
    env_value(short_key, legacy_key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn parse_mib_or_bytes_env(
    mib_key: &str,
    legacy_mib_key: &str,
    bytes_key: &str,
    legacy_bytes_key: &str,
    default_mib: u64,
) -> u64 {
    if let Ok(value) = env_value(mib_key, legacy_mib_key)
        && let Ok(mib) = value.parse::<u64>()
    {
        return mib.saturating_mul(1024 * 1024);
    }
    env_value(bytes_key, legacy_bytes_key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| default_mib.saturating_mul(1024 * 1024))
}

fn parse_usize_env(short_key: &str, legacy_key: &str, default: usize) -> usize {
    env_value(short_key, legacy_key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn default_proxy_memory_budget_mib() -> usize {
    const FALLBACK_MIB: usize = 384;
    const MIN_MIB: usize = 192;
    const MAX_MIB: usize = 512;

    // 优先读取容器限制，避免宿主机内存很大时在小容器里放行过多并发任务。
    let limit_bytes = [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ]
    .iter()
    .find_map(|path| {
        let raw = std::fs::read_to_string(path).ok()?;
        let bytes = raw.trim().parse::<u64>().ok()?;
        (bytes > 0 && bytes < (1_u64 << 60)).then_some(bytes)
    });

    let Some(limit_bytes) = limit_bytes else {
        return FALLBACK_MIB;
    };
    let budget_bytes = limit_bytes.saturating_mul(40) / 100;
    usize::try_from(budget_bytes / (1024 * 1024))
        .unwrap_or(MAX_MIB)
        .clamp(MIN_MIB, MAX_MIB)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn guest_generation_and_image_concurrency_are_isolated_per_ip() {
        let limits = GuestProxyLimits::default();
        let first_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));
        let second_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11));
        let window = Duration::from_secs(600);
        let generation_permits = (0..4)
            .map(|_| {
                limits
                    .acquire(first_ip, GuestProxyOperation::Generation, 4, 30, window)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            limits.acquire(first_ip, GuestProxyOperation::Generation, 4, 30, window),
            Err(GuestLimitRejection::Concurrent)
        ));

        let image_permits = (0..2)
            .map(|_| {
                limits
                    .acquire(first_ip, GuestProxyOperation::ImageFetch, 2, 120, window)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            limits.acquire(first_ip, GuestProxyOperation::ImageFetch, 2, 120, window),
            Err(GuestLimitRejection::Concurrent)
        ));
        assert!(
            limits
                .acquire(second_ip, GuestProxyOperation::Generation, 4, 30, window)
                .is_ok()
        );

        drop(generation_permits);
        drop(image_permits);
        assert!(
            limits
                .acquire(first_ip, GuestProxyOperation::Generation, 4, 30, window)
                .is_ok()
        );
    }

    #[test]
    fn guest_rate_limit_can_be_enforced_or_disabled() {
        let limits = GuestProxyLimits::default();
        let ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20));
        let window = Duration::from_secs(600);
        for _ in 0..2 {
            drop(
                limits
                    .acquire(ip, GuestProxyOperation::Generation, 1, 2, window)
                    .unwrap(),
            );
        }
        assert!(matches!(
            limits.acquire(ip, GuestProxyOperation::Generation, 1, 2, window),
            Err(GuestLimitRejection::RateLimited { .. })
        ));

        let unlimited_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 21));
        for _ in 0..20 {
            drop(
                limits
                    .acquire(unlimited_ip, GuestProxyOperation::ImageFetch, 1, 0, window)
                    .unwrap(),
            );
        }
    }
}
