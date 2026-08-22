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
    default_config, generate_with_strategy, hydrate_local_state, load_templates,
    prepare_sync_envelope,
};
use crate::storage::{
    apply_asset_payload_changes, clear_asset_payloads, clear_trusted_sync_secret,
    load_api_key_sync_enabled, load_asset_payloads, load_snapshot, load_trusted_sync_secret,
    save_api_key_sync_enabled, save_trusted_sync_secret, save_ui_state, save_workspace_snapshot,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gloo_file::{File, futures::read_as_bytes, futures::read_as_data_url};
use gloo_net::http::Request;
use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use leptos::{prelude::*, task::spawn_local};
use mew_image_shared::{
    AdminBootstrapRequest, AdminSetupStatusResponse, AdminUserActionRequest, AdminUsersResponse,
    AppPreferences, AuthRequest, AuthResponse, BUILTIN_OPENAI_IMAGE_TEMPLATE_ID,
    ChangePasswordRequest, CloudDataClearRequest, CloudDataClearScope, CloudDataStatsResponse,
    ConversationThread, DEFAULT_FAVORITE_FOLDER_ID, EncryptedApiConfig, FavoriteFolder,
    FavoriteFolderTombstone, GenerationSettingsSnapshot, ImageAssetRef, LocalAppState,
    LocalTaskRecord, MeResponse, ProviderKind, ProviderTemplate, RegisterRequest, SyncCheckpoint,
    SyncEntityKind, SyncPullResponse, SyncTombstone, TaskStatus, ThemePreference,
    UploadCompleteRequest, UploadCompleteResponse, UploadInitRequest, UploadInitResponse,
    UserSummary, UsernameAvailabilityResponse, new_id, normalize_api_config, now_rfc3339,
    strip_successful_task_payloads,
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
use actions::data::build_data_actions;
use actions::generation::build_generation_actions;
use actions::preview::build_preview_actions;
use actions::workspace::build_workspace_actions;
use components::favorites::FavoritesOverlay;
use components::gallery::GallerySidebar;
use components::overlays::{
    ContextMenuOverlay, FailureLogOverlay, FloatingTipOverlay, ReferenceMenuOverlay,
};
use components::popovers::GlobalPopovers;
use components::preview::PreviewOverlay;
use components::settings::SettingsOverlay;
use components::top_bar::TopBar;
use components::workspace::WorkspaceMain;
use controller::AppController;
use derived::*;
use models::*;
use state::*;
use utils::audio::*;
use utils::formatting::*;
pub(crate) use utils::image::*;
use utils::persistence::*;
use utils::resolution::*;
use utils::sync::*;
use utils::workspace::*;

const THUMBNAIL_DATA_URL_KEY: &str = "thumbnail_data_url";
const FAVORITE_ARCHIVE_ASSET_KEY: &str = "favorite_archive";
const THUMBNAIL_MAX_EDGE: u32 = 320;
const GALLERY_PAGE_SIZE: usize = 10;
const FAVORITE_PAGE_SIZE: usize = 9;
const VISIBLE_THREAD_LIMIT: usize = 5;
const ASSET_PAYLOAD_CACHE_MAX_ITEMS: usize = 6;
const ASSET_PAYLOAD_CACHE_MAX_BYTES: u64 = 48 * 1024 * 1024;

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
