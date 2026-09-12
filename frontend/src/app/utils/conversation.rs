use std::collections::HashSet;

use mew_image_shared::{
    ConversationContextMode, ConversationTurnSnapshot, EncryptedApiConfig, LocalTaskRecord,
    ProviderEndpointMode,
};

const MAX_COMPATIBILITY_HISTORY_TURNS: usize = 20;
const MAX_COMPATIBILITY_PROMPT_CHARS: usize = 64_000;

pub(crate) struct PreparedConversation {
    pub prompt: String,
    pub compatibility_prompt: Option<String>,
    pub previous_response_id: Option<String>,
    pub snapshot: ConversationTurnSnapshot,
    pub inherited_reference_ids: Vec<String>,
}

pub(crate) fn conversation_chain<'a>(
    tasks: &'a [LocalTaskRecord],
    anchor_task_id: &str,
) -> Vec<&'a LocalTaskRecord> {
    let mut chain = Vec::new();
    let mut current = Some(anchor_task_id);
    let mut visited = HashSet::new();
    while let Some(task_id) = current {
        if !visited.insert(task_id) {
            break;
        }
        let Some(task) = tasks.iter().find(|task| task.id == task_id) else {
            break;
        };
        chain.push(task);
        current = task
            .conversation
            .as_ref()
            .and_then(|turn| turn.parent_task_id.as_deref());
    }
    chain.reverse();
    chain
}

fn render_compatibility_prompt(
    tasks: &[LocalTaskRecord],
    anchor_task_id: &str,
    current_prompt: &str,
) -> (String, u32) {
    let chain = conversation_chain(tasks, anchor_task_id);
    let mut history = Vec::new();
    if let Some(initial) = chain.first() {
        history.push(("初始要求", initial.prompt.as_str()));
    }
    let recent_start = chain.len().saturating_sub(MAX_COMPATIBILITY_HISTORY_TURNS);
    for task in chain.iter().skip(recent_start) {
        if Some(task.id.as_str()) == chain.first().map(|initial| initial.id.as_str()) {
            continue;
        }
        history.push(("后续修改", task.prompt.as_str()));
    }

    let build = |items: &[(&str, &str)]| {
        let mut output = String::from(
            "这是同一张图片的连续修改任务。请以随请求提供的最新结果和当前参考图为视觉依据，继承未被后续要求明确推翻的约束。\n",
        );
        for (label, prompt) in items {
            output.push_str(label);
            output.push_str("：\n");
            output.push_str(prompt.trim());
            output.push('\n');
        }
        output.push_str("当前要求：\n");
        output.push_str(current_prompt.trim());
        output
    };

    while history.len() > 1 && build(&history).chars().count() > MAX_COMPATIBILITY_PROMPT_CHARS {
        // 始终保留初始要求，从最旧的后续轮次开始缩减。
        history.remove(1);
    }
    (build(&history), history.len() as u32)
}

pub(crate) fn prepare_conversation(
    tasks: &[LocalTaskRecord],
    config: &EncryptedApiConfig,
    parent_task_id: Option<&str>,
    source_result_asset_id: Option<&str>,
    selected_reference_ids: &[String],
    current_prompt: String,
    force_rebase: bool,
) -> PreparedConversation {
    let Some(parent_task_id) = parent_task_id else {
        let mode = if config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
            ConversationContextMode::NativeResponses
        } else {
            ConversationContextMode::Compatibility
        };
        return PreparedConversation {
            prompt: current_prompt,
            compatibility_prompt: None,
            previous_response_id: None,
            snapshot: ConversationTurnSnapshot {
                parent_task_id: None,
                source_result_asset_id: None,
                mode,
                provider_context_revision: Some(config.updated_at.clone()),
                included_history_turns: 0,
            },
            inherited_reference_ids: Vec::new(),
        };
    };

    let parent = tasks.iter().find(|task| task.id == parent_task_id);
    let inherited_reference_ids = parent
        .into_iter()
        .flat_map(LocalTaskRecord::ordinary_reference_asset_ids)
        .cloned()
        .collect::<Vec<_>>();
    let removed_inherited_reference = inherited_reference_ids
        .iter()
        .any(|id| !selected_reference_ids.contains(id));
    let provider_context_matches = parent.is_some_and(|parent| {
        parent.config_id == config.id
            && parent
                .conversation
                .as_ref()
                .and_then(|turn| turn.provider_context_revision.as_deref())
                == Some(config.updated_at.as_str())
    });
    let upstream_response_id = parent
        .and_then(|parent| parent.result.as_ref())
        .and_then(|result| result.upstream_response_id.clone());
    let native = config.endpoint_mode == ProviderEndpointMode::ResponsesApi
        && !force_rebase
        && provider_context_matches
        && !removed_inherited_reference
        && upstream_response_id.is_some();
    let (compatibility_prompt, included_history_turns) =
        render_compatibility_prompt(tasks, parent_task_id, &current_prompt);
    let mode = if native {
        ConversationContextMode::NativeResponses
    } else if config.endpoint_mode == ProviderEndpointMode::ResponsesApi {
        ConversationContextMode::Rebased
    } else {
        ConversationContextMode::Compatibility
    };

    PreparedConversation {
        prompt: if native {
            current_prompt
        } else {
            compatibility_prompt.clone()
        },
        compatibility_prompt: native.then_some(compatibility_prompt),
        previous_response_id: native.then_some(upstream_response_id).flatten(),
        snapshot: ConversationTurnSnapshot {
            parent_task_id: Some(parent_task_id.to_string()),
            source_result_asset_id: source_result_asset_id.map(str::to_owned),
            mode,
            provider_context_revision: Some(config.updated_at.clone()),
            included_history_turns,
        },
        inherited_reference_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{GenerationResult, ParameterSnapshot, TaskStatus};

    fn task(
        id: &str,
        parent: Option<&str>,
        prompt: &str,
        response_id: Option<&str>,
        references: &[&str],
    ) -> LocalTaskRecord {
        LocalTaskRecord {
            editing: None,
            id: id.into(),
            thread_id: "thread".into(),
            config_id: "config".into(),
            prompt: prompt.into(),
            requested_model: "gpt-image-2.5-flare".into(),
            reference_asset_ids: references.iter().map(|id| (*id).into()).collect(),
            conversation: Some(ConversationTurnSnapshot {
                parent_task_id: parent.map(str::to_owned),
                source_result_asset_id: parent.map(|id| format!("result-{id}")),
                mode: ConversationContextMode::NativeResponses,
                provider_context_revision: Some("revision".into()),
                included_history_turns: 0,
            }),
            generation_settings: None,
            result: Some(GenerationResult {
                images: Vec::new(),
                parameter_snapshot: ParameterSnapshot::default(),
                upstream_response_id: response_id.map(str::to_owned),
                raw_response_json: None,
            }),
            favorite: false,
            favorite_folder_id: None,
            detached_from_thread: false,
            source_gallery_template_id: None,
            status: TaskStatus::Succeeded,
            error_message: None,
            created_at: "now".into(),
            updated_at: "now".into(),
        }
    }

    fn responses_config() -> EncryptedApiConfig {
        let mut config =
            crate::providers::default_config(mew_image_shared::BUILTIN_OPENAI_IMAGE_TEMPLATE_ID);
        config.id = "config".into();
        config.endpoint_mode = ProviderEndpointMode::ResponsesApi;
        config.updated_at = "revision".into();
        config
    }

    #[test]
    fn chain_cycle_is_bounded() {
        let mut first = task("first", Some("second"), "one", None, &[]);
        let second = task("second", Some("first"), "two", None, &[]);
        first.conversation.as_mut().unwrap().source_result_asset_id = None;
        let tasks = [first, second];
        let chain = conversation_chain(&tasks, "first");
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn compatibility_history_keeps_initial_and_latest_twenty_turns() {
        let mut tasks = Vec::new();
        for index in 0..25 {
            let id = format!("task-{index}");
            let parent = (index > 0).then(|| format!("task-{}", index - 1));
            tasks.push(task(
                &id,
                parent.as_deref(),
                &format!("prompt-{index}"),
                None,
                &[],
            ));
        }
        let (prompt, included) = render_compatibility_prompt(&tasks, "task-24", "current");
        assert_eq!(included, 21);
        assert!(prompt.contains("prompt-0"));
        assert!(!prompt.contains("prompt-4\n"));
        assert!(prompt.contains("prompt-5"));
        assert!(prompt.ends_with("current"));
    }

    #[test]
    fn native_chain_requires_matching_revision_response_and_inherited_references() {
        let parent = task("parent", None, "initial", Some("resp_123"), &["ref-a"]);
        let config = responses_config();
        let prepared = prepare_conversation(
            std::slice::from_ref(&parent),
            &config,
            Some("parent"),
            Some("result-parent"),
            &["ref-a".into(), "new-ref".into()],
            "change".into(),
            false,
        );
        assert_eq!(prepared.previous_response_id.as_deref(), Some("resp_123"));
        assert_eq!(
            prepared.snapshot.mode,
            ConversationContextMode::NativeResponses
        );
        assert_eq!(prepared.inherited_reference_ids, ["ref-a"]);

        let rebased = prepare_conversation(
            &[parent],
            &config,
            Some("parent"),
            Some("result-parent"),
            &[],
            "change".into(),
            false,
        );
        assert!(rebased.previous_response_id.is_none());
        assert_eq!(rebased.snapshot.mode, ConversationContextMode::Rebased);

        let forced = prepare_conversation(
            std::slice::from_ref(&task("parent", None, "initial", Some("resp_123"), &[])),
            &config,
            Some("parent"),
            Some("result-parent"),
            &[],
            "change".into(),
            true,
        );
        assert!(forced.previous_response_id.is_none());
        assert_eq!(forced.snapshot.mode, ConversationContextMode::Rebased);
        assert!(forced.prompt.contains("初始要求"));
    }
}
