use leptos::prelude::*;

use crate::app::{
    state::{ComposerState, PersistenceState, WorkspaceState},
    utils::persistence::request_workspace_persist,
};
use crate::image_editor::runtime::EditorRuntime;

pub(super) fn current_runtime(workspace: WorkspaceState, composer: ComposerState) -> EditorRuntime {
    EditorRuntime {
        editing_by_thread: composer.editing_by_thread.get_untracked(),
        thread_id: workspace.current_thread_id.get_untracked(),
        reference_ids: composer.selected_reference_ids.get_untracked(),
        continuation_id: composer.continuation_asset_id.get_untracked(),
    }
}

pub(super) fn runtime_matches(
    runtime: &EditorRuntime,
    workspace: WorkspaceState,
    composer: ComposerState,
) -> bool {
    workspace
        .current_thread_id
        .with_untracked(|id| id == &runtime.thread_id)
        && composer
            .selected_reference_ids
            .with_untracked(|ids| ids == &runtime.reference_ids)
        && composer
            .continuation_asset_id
            .with_untracked(|id| id == &runtime.continuation_id)
        && composer
            .editing_by_thread
            .with_untracked(|items| items == &runtime.editing_by_thread)
}

pub(super) async fn preserve_current_runtime(
    workspace: WorkspaceState,
    composer: ComposerState,
    persistence: PersistenceState,
) -> Result<(), String> {
    if persistence
        .local_state_status
        .with_untracked(|state| state.is_ready())
    {
        crate::image_editor::runtime::save_runtime(current_runtime(workspace, composer)).await?;
    }
    Ok(())
}

/// 暂停后台快照写入，直到应用事务完成且内存状态已更新；析构时恢复待保存的变化。
pub(super) struct ApplicationPersistGuard {
    workspace: WorkspaceState,
    persistence: PersistenceState,
}

impl ApplicationPersistGuard {
    pub(super) async fn acquire(
        workspace: WorkspaceState,
        persistence: PersistenceState,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, String> {
        loop {
            if cancelled()
                || !persistence
                    .local_state_status
                    .with_untracked(|state| state.is_ready())
            {
                return Err("已取消应用或本地数据状态已变化，草稿已保留。".into());
            }
            if !persistence.workspace_persist_inflight.get_untracked() {
                persistence.workspace_persist_inflight.set(true);
                return Ok(Self {
                    workspace,
                    persistence,
                });
            }
            gloo_timers::future::TimeoutFuture::new(50).await;
        }
    }
}

impl Drop for ApplicationPersistGuard {
    fn drop(&mut self) {
        self.persistence.workspace_persist_inflight.set(false);
        request_workspace_persist(
            self.workspace.tasks,
            self.workspace.threads,
            self.workspace.assets,
            self.workspace.checkpoint,
            self.workspace.tombstones,
            self.persistence,
        );
    }
}
