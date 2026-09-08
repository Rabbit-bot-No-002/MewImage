mod actions;
mod components;
mod controller;
mod derived;
mod effects;
mod models;
mod state;
mod utils;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
};

use crate::crypto::derive_trusted_sync_secret;
use crate::providers::{
    GenerationLifecycle, ProxyBudgetRequest, ProxyGenerationPhase, default_config,
    generate_with_strategy, generation_uses_proxy, hydrate_local_state, load_templates,
    prepare_sync_envelope,
};
use crate::storage::{
    GenerationStagingManifest, apply_asset_payload_changes, clear_asset_payloads,
    clear_generation_queue_mode, clear_generation_staging, clear_trusted_sync_secret,
    load_api_key_sync_enabled, load_asset_object_urls, load_asset_payloads, load_snapshot,
    load_trusted_sync_secret, prepare_generation_asset_blobs, revoke_all_asset_object_urls,
    revoke_asset_object_url, runtime_asset_object_url, save_api_key_sync_enabled,
    save_generation_queue_mode, save_trusted_sync_secret, save_ui_state, save_workspace_snapshot,
    stage_generation_asset_blobs, store_asset_bytes_for_display,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gloo_file::{File, futures::read_as_bytes, futures::read_as_data_url};
use gloo_net::http::Request;
use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::{
    AdminBootstrapRequest, AdminSetupStatusResponse, AdminUserActionRequest, AdminUsersResponse,
    AppPreferences, AssetPresenceRequest, AssetPresenceResponse, AuthRequest, AuthResponse,
    BUILTIN_OPENAI_IMAGE_TEMPLATE_ID, BackgroundLayer, BackgroundPosition, ChangePasswordRequest,
    CloudDataClearRequest, CloudDataClearScope, CloudDataStatsResponse, ConversationThread,
    DEFAULT_FAVORITE_FOLDER_ID, DecorationLevel, EncryptedApiConfig, FavoriteFolder,
    FavoriteFolderTombstone, GenerationSettingsSnapshot, ImageAssetRef, LocalAppState,
    LocalTaskRecord, MeResponse, ProviderKind, ProviderTemplate, RegisterRequest, SyncCheckpoint,
    SyncEntityKind, SyncPullResponse, SyncTombstone, TaskStatus, ThemePreference,
    UploadCompleteRequest, UploadCompleteResponse, UploadInitRequest, UploadInitResponse,
    UserSummary, UsernameAvailabilityResponse, VisualTheme, new_id, normalize_api_config,
    normalized_background_mode, normalized_image_output_format, now_rfc3339,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, Event, FileList, HtmlAnchorElement, HtmlCanvasElement, HtmlImageElement,
    HtmlInputElement, HtmlTextAreaElement, KeyboardEvent, MouseEvent,
};

use crate::{api::api_url, data_management};
use actions::account::build_account_actions;
use actions::appearance::build_appearance_actions;
use actions::data::build_data_actions;
use actions::generation::build_generation_actions;
use actions::preview::build_preview_actions;
use actions::workspace::build_workspace_actions;
use components::appearance::ThemeBackdrop;
use components::favorites::FavoritesOverlay;
use components::gallery::GallerySidebar;
use components::overlays::{
    ContextMenuOverlay, FailureLogOverlay, FloatingTipOverlay, ReferenceMenuOverlay,
};
use components::popovers::GlobalPopovers;
use components::preview::PreviewOverlay;
use components::settings::SettingsOverlay;
use components::template_plaza::TemplatePlaza;
use components::top_bar::TopBar;
use components::workspace::WorkspaceMain;
use controller::AppController;
use derived::*;
use models::*;
use state::*;
use utils::appearance::*;
use utils::audio::*;
use utils::formatting::*;
pub(crate) use utils::image::*;
pub(crate) use utils::persistence::*;
use utils::resolution::*;
use utils::sync::*;
use utils::transparency::*;
use utils::workspace::*;

const THUMBNAIL_DATA_URL_KEY: &str = "thumbnail_data_url";
const FAVORITE_ARCHIVE_ASSET_KEY: &str = "favorite_archive";
const THUMBNAIL_MAX_EDGE: u32 = 320;
const GALLERY_PAGE_SIZE: usize = 10;
const FAVORITE_PAGE_SIZE: usize = 9;
const VISIBLE_THREAD_LIMIT: usize = 5;
const ASSET_PAYLOAD_CACHE_MAX_ITEMS: usize = 6;
const ASSET_PAYLOAD_CACHE_MAX_BYTES: u64 = 48 * 1024 * 1024;
pub(crate) const MAX_ACTIVE_GENERATION_TASKS: usize = 20;
const MAX_GENERATION_REFERENCE_ASSETS: usize = 16;
const DEFAULT_ACTIVE_GENERATION_BYTE_BUDGET: u64 = 256 * 1024 * 1024;
const MIN_ACTIVE_GENERATION_BYTE_BUDGET: u64 = 192 * 1024 * 1024;
const MAX_ACTIVE_GENERATION_BYTE_BUDGET: u64 = 512 * 1024 * 1024;
const GENERATION_PREPARATION_FIXED_BYTE_OVERHEAD: u64 = 16 * 1024 * 1024;
const GENERATION_TASK_FIXED_BYTE_OVERHEAD: u64 = 32 * 1024 * 1024;
const THEME_BACKGROUND_ROLE_KEY: &str = "asset_role";
const THEME_BACKGROUND_ROLE: &str = "theme_background";

thread_local! {
    static ASSET_PAYLOAD_LRU: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[component]
pub(crate) fn App() -> impl IntoView {
    let AppState {
        workspace,
        composer,
        account,
        ui,
        persistence,
    } = AppState::new();
    provide_context(workspace);
    provide_context(composer);
    provide_context(account);
    provide_context(ui);
    provide_context(persistence);

    view! { <AppController /> }
}
