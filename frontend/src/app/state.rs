use std::collections::{HashMap, HashSet};

use leptos::{html, prelude::*};
use mew_image_shared::{
    AdminUserSummary, AppPreferences, CloudDataStatsResponse, ConversationThread,
    EncryptedApiConfig, ImageAssetRef, LocalTaskRecord, ProviderTemplate, SyncCheckpoint,
    SyncTombstone, UserSummary,
};

use crate::storage::load_generation_queue_mode;

use super::{
    default_thread,
    models::{
        ConfirmPopoverState, ContextMenuState, FailureLogState, FavoriteFolderPickerState,
        FloatingTipState, PreviewPanelState, PreviewState, TextPopoverState,
    },
    system_prefers_dark,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalStateLoadStatus {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MainView {
    Workspace,
    TemplatePlaza,
}

impl MainView {
    pub(crate) fn from_location() -> Self {
        let search = web_sys::window()
            .and_then(|window| window.location().search().ok())
            .unwrap_or_default();
        main_view_from_search(&search)
    }
}

fn main_view_from_search(search: &str) -> MainView {
    if search
        .trim_start_matches('?')
        .split('&')
        .any(|part| part == "view=templates")
    {
        MainView::TemplatePlaza
    } else {
        MainView::Workspace
    }
}

impl LocalStateLoadStatus {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_deep_link_selects_plaza_view() {
        assert_eq!(
            main_view_from_search("?view=templates&template=123"),
            MainView::TemplatePlaza
        );
        assert_eq!(
            main_view_from_search("?view=workspace"),
            MainView::Workspace
        );
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceState {
    pub(crate) configs: RwSignal<Vec<EncryptedApiConfig>>,
    pub(crate) tasks: RwSignal<Vec<LocalTaskRecord>>,
    pub(crate) threads: RwSignal<Vec<ConversationThread>>,
    pub(crate) assets: RwSignal<Vec<ImageAssetRef>>,
    pub(crate) preferences: RwSignal<AppPreferences>,
    pub(crate) checkpoint: RwSignal<SyncCheckpoint>,
    pub(crate) tombstones: RwSignal<Vec<SyncTombstone>>,
    pub(crate) templates: RwSignal<Vec<ProviderTemplate>>,
    pub(crate) current_thread_id: RwSignal<String>,
    pub(crate) current_config_id: RwSignal<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationRuntimePhase {
    WaitingPreparationBudget,
    WaitingFullTaskBudget,
    LoadingReferences,
    Submitting,
    ServerQueued,
    AwaitingUpstream,
    WaitingResultBudget,
    ReceivingResult,
    ProcessingResult { current: usize, total: usize },
    RemovingBackground { current: usize, total: usize },
    PersistingResult { retry: usize },
    DirectProtected,
    LegacyProxyProtected,
}

impl GenerationRuntimePhase {
    pub(crate) fn label(self) -> String {
        match self {
            Self::WaitingPreparationBudget => "等待参考图处理资源".into(),
            Self::WaitingFullTaskBudget => "等待完整任务处理资源".into(),
            Self::LoadingReferences => "正在加载参考图".into(),
            Self::Submitting => "正在提交生成请求".into(),
            Self::ServerQueued => "服务端排队".into(),
            Self::AwaitingUpstream => "等待上游结果".into(),
            Self::WaitingResultBudget => "等待结果处理资源".into(),
            Self::ReceivingResult => "正在领取生成结果".into(),
            Self::ProcessingResult { current, total } => {
                format!("处理结果 {current}/{total}")
            }
            Self::RemovingBackground { current, total } => {
                format!("本地去背 {current}/{total}")
            }
            Self::PersistingResult { retry: 0 } => "正在保存生成结果".into(),
            Self::PersistingResult { retry } => format!("等待本地保存（重试 {retry}）"),
            Self::DirectProtected => "直连请求等待上游".into(),
            Self::LegacyProxyProtected => "兼容代理等待上游".into(),
        }
    }

    pub(crate) fn waits_for_budget(self) -> bool {
        matches!(
            self,
            Self::WaitingPreparationBudget
                | Self::WaitingFullTaskBudget
                | Self::WaitingResultBudget
        )
    }

    pub(crate) fn result_priority(self) -> bool {
        matches!(self, Self::WaitingResultBudget)
    }
}

#[derive(Clone)]
pub(crate) struct ActiveGenerationRuntime {
    pub(crate) abort_controller: web_sys::AbortController,
    pub(crate) dependency_asset_ids: HashSet<String>,
    pub(crate) thread_id: String,
    pub(crate) phase: GenerationRuntimePhase,
    pub(crate) sequence: u64,
    pub(crate) requested_bytes: u64,
    pub(crate) budget_bytes: u64,
    /// 首次等待上游时为 0；多批结果已在内存时继续保留累计额度，直到全部落盘。
    pub(crate) reserved_bytes: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct ComposerState {
    pub(crate) selected_reference_ids: RwSignal<Vec<String>>,
    pub(crate) show_all_reference_assets: RwSignal<bool>,
    pub(crate) dragging_reference_id: RwSignal<Option<String>>,
    pub(crate) drag_over_reference_id: RwSignal<Option<String>>,
    pub(crate) reference_menu_asset_id: RwSignal<Option<String>>,
    pub(crate) continuation_asset_id: RwSignal<Option<String>>,
    pub(crate) draft_prompt: RwSignal<String>,
    pub(crate) draft_prompt_ref: NodeRef<html::Textarea>,
    pub(crate) custom_width: RwSignal<u32>,
    pub(crate) custom_height: RwSignal<u32>,
    pub(crate) resolution_mode: RwSignal<String>,
    pub(crate) resolution_group: RwSignal<String>,
    pub(crate) aspect_ratio: RwSignal<String>,
    pub(crate) custom_aspect_ratio_input: RwSignal<String>,
    pub(crate) effective_custom_aspect_ratio: RwSignal<String>,
    pub(crate) quality: RwSignal<String>,
    pub(crate) count: RwSignal<u32>,
    pub(crate) status_text: RwSignal<String>,
    pub(crate) queue_mode_enabled: RwSignal<bool>,
    pub(crate) active_generation_ids: RwSignal<HashSet<String>>,
    pub(crate) cancelled_generation_ids: RwSignal<HashSet<String>>,
    pub(crate) generation_runtimes: RwSignal<HashMap<String, ActiveGenerationRuntime>>,
    pub(crate) foreground_generation_task_id: RwSignal<Option<String>>,
    pub(crate) generating: RwSignal<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct AccountState {
    pub(crate) auth_user: RwSignal<Option<UserSummary>>,
    pub(crate) login_username: RwSignal<String>,
    pub(crate) login_password: RwSignal<String>,
    pub(crate) auth_mode: RwSignal<String>,
    pub(crate) register_password_confirm: RwSignal<String>,
    pub(crate) admin_setup_token: RwSignal<String>,
    pub(crate) show_admin_setup_token: RwSignal<bool>,
    pub(crate) admin_setup_allowed: RwSignal<bool>,
    pub(crate) auth_form_message: RwSignal<Option<String>>,
    pub(crate) username_check_message: RwSignal<Option<String>>,
    pub(crate) change_old_password: RwSignal<String>,
    pub(crate) change_new_password: RwSignal<String>,
    pub(crate) change_new_password_confirm: RwSignal<String>,
    pub(crate) password_form_message: RwSignal<Option<String>>,
    pub(crate) admin_users: RwSignal<Vec<AdminUserSummary>>,
    pub(crate) loading_admin_users: RwSignal<bool>,
    pub(crate) sync_secret: RwSignal<String>,
    pub(crate) legacy_sync_secret: RwSignal<String>,
    pub(crate) sync_api_keys_enabled: RwSignal<bool>,
    pub(crate) sync_unlock_password: RwSignal<String>,
    pub(crate) sync_unlocking: RwSignal<bool>,
    pub(crate) sync_status_text: RwSignal<Option<String>>,
    pub(crate) syncing: RwSignal<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct UiState {
    pub(crate) main_view: RwSignal<MainView>,
    pub(crate) gallery_template_draft_task_id: RwSignal<Option<String>>,
    pub(crate) show_favorites_panel: RwSignal<bool>,
    pub(crate) favorite_folder_picker: RwSignal<Option<FavoriteFolderPickerState>>,
    pub(crate) text_popover: RwSignal<Option<TextPopoverState>>,
    pub(crate) text_popover_value: RwSignal<String>,
    pub(crate) confirm_popover: RwSignal<Option<ConfirmPopoverState>>,
    pub(crate) show_thread_archive_menu: RwSignal<bool>,
    pub(crate) gallery_page: RwSignal<usize>,
    pub(crate) favorite_page: RwSignal<usize>,
    pub(crate) show_settings: RwSignal<bool>,
    pub(crate) show_settings_menu: RwSignal<bool>,
    pub(crate) settings_tab: RwSignal<String>,
    pub(crate) data_management_tab: RwSignal<String>,
    pub(crate) data_management_busy: RwSignal<bool>,
    pub(crate) data_management_message: RwSignal<Option<String>>,
    pub(crate) session_export_thread_id: RwSignal<String>,
    pub(crate) cloud_data_stats: RwSignal<Option<CloudDataStatsResponse>>,
    pub(crate) backup_file_input: NodeRef<html::Input>,
    pub(crate) background_file_input: NodeRef<html::Input>,
    pub(crate) background_processing: RwSignal<bool>,
    pub(crate) appearance_message: RwSignal<Option<String>>,
    pub(crate) background_display_src: RwSignal<Option<String>>,
    pub(crate) background_display_asset_id: RwSignal<Option<String>>,
    pub(crate) system_dark: RwSignal<bool>,
    pub(crate) show_resolution_menu: RwSignal<bool>,
    pub(crate) show_config_switcher: RwSignal<bool>,
    pub(crate) preview_state: RwSignal<Option<PreviewState>>,
    pub(crate) preview_panel_state: RwSignal<Option<PreviewPanelState>>,
    pub(crate) preview_fullscreen: RwSignal<bool>,
    pub(crate) context_menu_state: RwSignal<Option<ContextMenuState>>,
    pub(crate) failure_log_state: RwSignal<Option<FailureLogState>>,
    pub(crate) floating_tip_state: RwSignal<Option<FloatingTipState>>,
    pub(crate) floating_tip_token: RwSignal<u64>,
}

#[derive(Clone, Copy)]
pub(crate) struct PersistenceState {
    pub(crate) local_state_status: RwSignal<LocalStateLoadStatus>,
    pub(crate) workspace_persist_requested_revision: RwSignal<u64>,
    pub(crate) workspace_persist_completed_revision: RwSignal<u64>,
    pub(crate) workspace_persist_scheduled: RwSignal<bool>,
    pub(crate) workspace_persist_inflight: RwSignal<bool>,
    pub(crate) workspace_persist_pending: RwSignal<bool>,
    pub(crate) ui_persist_scheduled: RwSignal<bool>,
    pub(crate) ui_persist_inflight: RwSignal<bool>,
    pub(crate) ui_persist_pending: RwSignal<bool>,
    pub(crate) payload_write_queue: RwSignal<HashMap<String, String>>,
    pub(crate) payload_delete_queue: RwSignal<HashSet<String>>,
    pub(crate) payload_flush_scheduled: RwSignal<bool>,
    pub(crate) payload_flush_inflight: RwSignal<bool>,
    pub(crate) payload_flush_pending: RwSignal<bool>,
    pub(crate) payload_flush_failures: RwSignal<u8>,
}

pub(crate) struct AppState {
    pub(crate) workspace: WorkspaceState,
    pub(crate) composer: ComposerState,
    pub(crate) account: AccountState,
    pub(crate) ui: UiState,
    pub(crate) persistence: PersistenceState,
}

impl AppState {
    pub(crate) fn new() -> Self {
        let workspace = WorkspaceState {
            configs: RwSignal::new(Vec::new()),
            tasks: RwSignal::new(Vec::new()),
            threads: RwSignal::new(vec![default_thread()]),
            assets: RwSignal::new(Vec::new()),
            preferences: RwSignal::new(AppPreferences::default()),
            checkpoint: RwSignal::new(SyncCheckpoint::default()),
            tombstones: RwSignal::new(Vec::new()),
            templates: RwSignal::new(vec![
                ProviderTemplate::builtin_openai(),
                ProviderTemplate::builtin_nano_banana(),
                ProviderTemplate::builtin_openai_compatible(),
            ]),
            current_thread_id: RwSignal::new(String::new()),
            current_config_id: RwSignal::new(String::new()),
        };
        let composer = ComposerState {
            selected_reference_ids: RwSignal::new(Vec::new()),
            show_all_reference_assets: RwSignal::new(false),
            dragging_reference_id: RwSignal::new(None),
            drag_over_reference_id: RwSignal::new(None),
            reference_menu_asset_id: RwSignal::new(None),
            continuation_asset_id: RwSignal::new(None),
            draft_prompt: RwSignal::new(String::new()),
            draft_prompt_ref: NodeRef::new(),
            custom_width: RwSignal::new(1024),
            custom_height: RwSignal::new(1024),
            resolution_mode: RwSignal::new("auto".into()),
            resolution_group: RwSignal::new("1k".into()),
            aspect_ratio: RwSignal::new("1:1".into()),
            custom_aspect_ratio_input: RwSignal::new("16:9".into()),
            effective_custom_aspect_ratio: RwSignal::new("16:9".into()),
            quality: RwSignal::new("high".into()),
            count: RwSignal::new(1),
            status_text: RwSignal::new(
                "准备就绪，当前默认是游客本地 + 受限代理模式：数据留在浏览器，本服务仅对受信任图像上游做临时中转。".into(),
            ),
            queue_mode_enabled: RwSignal::new(load_generation_queue_mode()),
            active_generation_ids: RwSignal::new(HashSet::new()),
            cancelled_generation_ids: RwSignal::new(HashSet::new()),
            generation_runtimes: RwSignal::new(HashMap::new()),
            foreground_generation_task_id: RwSignal::new(None),
            generating: RwSignal::new(false),
        };
        let account = AccountState {
            auth_user: RwSignal::new(None),
            login_username: RwSignal::new(String::new()),
            login_password: RwSignal::new(String::new()),
            auth_mode: RwSignal::new("login".into()),
            register_password_confirm: RwSignal::new(String::new()),
            admin_setup_token: RwSignal::new(String::new()),
            show_admin_setup_token: RwSignal::new(false),
            // 状态接口不可用时仍保留初始化入口，避免部署漏配被静默隐藏。
            admin_setup_allowed: RwSignal::new(true),
            auth_form_message: RwSignal::new(None),
            username_check_message: RwSignal::new(None),
            change_old_password: RwSignal::new(String::new()),
            change_new_password: RwSignal::new(String::new()),
            change_new_password_confirm: RwSignal::new(String::new()),
            password_form_message: RwSignal::new(None),
            admin_users: RwSignal::new(Vec::new()),
            loading_admin_users: RwSignal::new(false),
            sync_secret: RwSignal::new(String::new()),
            legacy_sync_secret: RwSignal::new(String::new()),
            sync_api_keys_enabled: RwSignal::new(true),
            sync_unlock_password: RwSignal::new(String::new()),
            sync_unlocking: RwSignal::new(false),
            sync_status_text: RwSignal::new(None),
            syncing: RwSignal::new(false),
        };
        let ui = UiState {
            main_view: RwSignal::new(MainView::from_location()),
            gallery_template_draft_task_id: RwSignal::new(None),
            show_favorites_panel: RwSignal::new(false),
            favorite_folder_picker: RwSignal::new(None),
            text_popover: RwSignal::new(None),
            text_popover_value: RwSignal::new(String::new()),
            confirm_popover: RwSignal::new(None),
            show_thread_archive_menu: RwSignal::new(false),
            gallery_page: RwSignal::new(1),
            favorite_page: RwSignal::new(1),
            show_settings: RwSignal::new(false),
            show_settings_menu: RwSignal::new(false),
            settings_tab: RwSignal::new("providers".into()),
            data_management_tab: RwSignal::new("local".into()),
            data_management_busy: RwSignal::new(false),
            data_management_message: RwSignal::new(None),
            session_export_thread_id: RwSignal::new(String::new()),
            cloud_data_stats: RwSignal::new(None),
            backup_file_input: NodeRef::new(),
            background_file_input: NodeRef::new(),
            background_processing: RwSignal::new(false),
            appearance_message: RwSignal::new(None),
            background_display_src: RwSignal::new(None),
            background_display_asset_id: RwSignal::new(None),
            system_dark: RwSignal::new(system_prefers_dark()),
            show_resolution_menu: RwSignal::new(false),
            show_config_switcher: RwSignal::new(false),
            preview_state: RwSignal::new(None),
            preview_panel_state: RwSignal::new(None),
            preview_fullscreen: RwSignal::new(false),
            context_menu_state: RwSignal::new(None),
            failure_log_state: RwSignal::new(None),
            floating_tip_state: RwSignal::new(None),
            floating_tip_token: RwSignal::new(0),
        };
        let persistence = PersistenceState {
            local_state_status: RwSignal::new(LocalStateLoadStatus::Loading),
            workspace_persist_requested_revision: RwSignal::new(0),
            workspace_persist_completed_revision: RwSignal::new(0),
            workspace_persist_scheduled: RwSignal::new(false),
            workspace_persist_inflight: RwSignal::new(false),
            workspace_persist_pending: RwSignal::new(false),
            ui_persist_scheduled: RwSignal::new(false),
            ui_persist_inflight: RwSignal::new(false),
            ui_persist_pending: RwSignal::new(false),
            payload_write_queue: RwSignal::new(HashMap::new()),
            payload_delete_queue: RwSignal::new(HashSet::new()),
            payload_flush_scheduled: RwSignal::new(false),
            payload_flush_inflight: RwSignal::new(false),
            payload_flush_pending: RwSignal::new(false),
            payload_flush_failures: RwSignal::new(0),
        };

        Self {
            workspace,
            composer,
            account,
            ui,
            persistence,
        }
    }
}
