use std::cell::Cell;

use leptos::prelude::*;

use crate::app::{actions::generation::browser_generation_byte_budget, state::ComposerState};

thread_local! {
    static EDITOR_BYTES: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn reserved_editor_bytes() -> u64 {
    EDITOR_BYTES.get()
}

pub(crate) struct EditorBudgetGuard;

impl Drop for EditorBudgetGuard {
    fn drop(&mut self) {
        EDITOR_BYTES.set(0);
    }
}

fn encoding_bytes(width: u32, height: u32) -> u64 {
    // 底图、叠加层、PNG 编码以及 SHA 读取会短暂并存。
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(16)
        .saturating_add(64 * 1024 * 1024)
}

fn fits(requested: u64, budget: u64, reserved: u64) -> bool {
    if requested > budget {
        reserved == 0
    } else {
        reserved.saturating_add(requested) <= budget
    }
}

/// 编辑不占生成任务槽，但和生成的大内存阶段共享预算，退出自动释放。
pub(crate) async fn acquire_editor_budget(
    composer: ComposerState,
    width: u32,
    height: u32,
    cancelled: impl Fn() -> bool,
) -> Result<EditorBudgetGuard, String> {
    let requested = encoding_bytes(width, height);
    let budget = browser_generation_byte_budget();
    loop {
        if cancelled() {
            return Err("编辑应用已取消或会话已切换。".into());
        }
        let allowed = composer.generation_runtimes.with_untracked(|runtimes| {
            let reserved = runtimes
                .values()
                .map(|runtime| runtime.reserved_bytes)
                .fold(0_u64, u64::saturating_add);
            // 已完成的生成结果优先领取，避免服务器结果等待编辑时过期。
            let pending_result = runtimes.values().any(|runtime| {
                runtime.phase.result_priority()
                    && runtime.phase.waits_for_budget()
                    && runtime.reserved_bytes == 0
            });
            !pending_result && fits(requested, budget, reserved)
        });
        if allowed && reserved_editor_bytes() == 0 {
            EDITOR_BYTES.set(requested);
            return Ok(EditorBudgetGuard);
        }
        gloo_timers::future::TimeoutFuture::new(100).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_encoding_runs_only_exclusively() {
        let budget = 256 * 1024 * 1024;
        let requested = encoding_bytes(4096, 4096);
        assert!(requested > budget);
        assert!(fits(requested, budget, 0));
        assert!(!fits(requested, budget, 1));
        assert!(!fits(64, 128, 65));
        assert!(fits(64, 128, 64));
    }

    #[test]
    fn guard_releases_reservation() {
        EDITOR_BYTES.set(1024);
        {
            let _guard = EditorBudgetGuard;
        }
        assert_eq!(reserved_editor_bytes(), 0);
    }
}
