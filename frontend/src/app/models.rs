use mew_image_shared::CloudDataClearScope;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreviewState {
    pub(crate) task_id: String,
    pub(crate) asset_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreviewReferenceThumb {
    pub(crate) id: String,
    pub(crate) src: String,
}

#[derive(Clone, PartialEq)]
pub(crate) struct FailureLogState {
    pub(crate) task_id: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) details: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreviewPanelState {
    pub(crate) task_id: String,
    pub(crate) asset_id: Option<String>,
    pub(crate) prompt: String,
    pub(crate) display_src: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) source_label: String,
    pub(crate) requested_model: String,
    pub(crate) moderation_label: String,
    pub(crate) background_label: String,
    pub(crate) requested_quality_label: String,
    pub(crate) actual_quality_label: String,
    pub(crate) format_label: String,
    pub(crate) image_count: usize,
    pub(crate) created_at: String,
    pub(crate) duration_label: String,
    pub(crate) favorite: bool,
    pub(crate) reference_thumbs: Vec<PreviewReferenceThumb>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct ContextMenuState {
    pub(crate) task_id: String,
    pub(crate) asset_id: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[derive(Clone, PartialEq)]
pub(crate) struct FloatingTipState {
    pub(crate) text: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) token: u64,
    pub(crate) persistent: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) struct FavoriteFolderPickerState {
    pub(crate) task_id: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) is_favorite: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) enum TextPopoverKind {
    RenameThread(String),
    AddFavoriteFolder,
    RenameFavoriteFolder(String),
}

#[derive(Clone, PartialEq)]
pub(crate) struct TextPopoverState {
    pub(crate) kind: TextPopoverKind,
    pub(crate) title: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

#[derive(Clone, PartialEq)]
pub(crate) enum ConfirmPopoverKind {
    CancelGeneration(String),
    CancelAllGenerations,
    DeleteAsset(String),
    DeleteConfig(String),
    DeleteThread(String),
    DeleteFavoriteFolder(String),
    DeleteTask(String),
    DeleteUser(String),
    DeleteThemeBackground,
    ClearLocalData(LocalDataClearScope),
    ClearLocalDataFinal(LocalDataClearScope),
    ClearCloudData(CloudDataClearScope),
    ClearCloudDataFinal(CloudDataClearScope),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalDataClearScope {
    Workspace,
    Configs,
    Preferences,
    All,
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConfirmPopoverState {
    pub(crate) kind: ConfirmPopoverKind,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
}
