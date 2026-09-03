use super::*;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::ResponseItemId;
use codex_protocol::items::AgentMessageContent;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tracing_subscriber::prelude::*;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", "1")),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn post_sampling_token_estimate_is_disabled_by_always_on_sinks() {
    let feedback = codex_feedback::CodexFeedback::new();
    let subscriber = tracing_subscriber::registry()
        .with(feedback.logger_layer())
        .with(tracing_subscriber::fmt::layer().with_filter(codex_state::log_db::default_filter()));

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        assert!(!tracing::event_enabled!(
            target: POST_SAMPLING_TOKEN_ESTIMATE_TARGET,
            tracing::Level::TRACE,
            turn_id,
            estimated_token_count,
            message
        ));
    });
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}

#[test]
fn clarification_only_accepts_successful_control_tool_output() {
    let output = |success| ResponseInputItem::FunctionCallOutput {
        call_id: "submit-understanding".to_string(),
        output: codex_protocol::models::FunctionCallOutputPayload {
            body: codex_protocol::models::FunctionCallOutputBody::Text(String::new()),
            success: Some(success),
        },
    };

    assert!(tool_call_output_succeeded(&output(true)));
    assert!(!tool_call_output_succeeded(&output(false)));
}

#[test]
fn clarification_decision_retry_is_bounded() {
    let mut retries = ClarificationRetryState::default();
    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::NotAttempted,
        &mut retries,
    )
    .expect("the first missing decision should be retried");
    assert_eq!(retries.missing_decisions, 1);

    let err = update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::NotAttempted,
        &mut retries,
    )
    .expect_err("a repeated missing decision should stop the turn");
    assert!(err.to_string().contains("repeatedly skipped"));

    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ true,
        TaskCandidateReview::NotAttempted,
        &mut retries,
    )
    .expect("a valid question should reset the retry counter");
    assert_eq!(retries.missing_decisions, 0);
}

#[test]
fn rejected_task_contract_can_return_to_clarification_before_being_bounded() {
    let mut retries = ClarificationRetryState::default();
    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::NeedsClarification,
        &mut retries,
    )
    .expect("the first rejected contract should return reviewer feedback");
    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::NeedsClarification,
        &mut retries,
    )
    .expect("the second rejected contract should still allow a user question");
    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ true,
        TaskCandidateReview::NotAttempted,
        &mut retries,
    )
    .expect("a valid question should reset rejected contract retries");

    for attempt in 0..=REJECTED_TASK_CANDIDATE_RETRY_LIMIT {
        let result = update_clarification_decision_retries(
            /*clarification_active*/ true,
            /*requested_user_input*/ false,
            TaskCandidateReview::NeedsClarification,
            &mut retries,
        );
        if attempt < REJECTED_TASK_CANDIDATE_RETRY_LIMIT {
            result.expect("rejected contract should return feedback before the limit");
        } else {
            let err = result.expect_err("repeated rejected contracts should stop the turn");
            assert!(err.to_string().contains("unsupported task candidates"));
        }
    }
}

#[test]
fn invalid_task_contract_retries_are_bounded_independently_of_review_rejections() {
    let mut retries = ClarificationRetryState::default();
    for _ in 0..INVALID_TASK_CANDIDATE_RETRY_LIMIT {
        update_clarification_decision_retries(
            /*clarification_active*/ true,
            /*requested_user_input*/ false,
            TaskCandidateReview::Invalid,
            &mut retries,
        )
        .expect("invalid submissions should receive bounded repair feedback");
    }
    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::NeedsClarification,
        &mut retries,
    )
    .expect("a first review rejection must allow clarification after format repairs");
    let err = update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ false,
        TaskCandidateReview::Invalid,
        &mut retries,
    )
    .expect_err("a review rejection must not replenish the invalid submission budget");
    assert!(
        err.to_string()
            .contains("failed validation or review execution")
    );

    update_clarification_decision_retries(
        /*clarification_active*/ true,
        /*requested_user_input*/ true,
        TaskCandidateReview::NotAttempted,
        &mut retries,
    )
    .expect("a user answer resets both budgets");
    assert_eq!(
        (retries.invalid_candidates, retries.rejected_candidates),
        (0, 0)
    );
}
