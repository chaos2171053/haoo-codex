use super::*;
use codex_protocol::models::FunctionCallOutputPayload;

#[test]
fn user_evidence_is_required_verbatim_after_whitespace_normalization() {
    let transcript =
        "user: Compare the two options for this release.\nassistant: Use a five year horizon.";

    assert!(transcript_supports_evidence(
        transcript,
        "Compare  the two options"
    ));
    assert!(transcript_supports_evidence(
        transcript,
        "“Compare the two options for this release.”"
    ));
    assert!(!transcript_supports_evidence(
        transcript,
        "five year horizon"
    ));
}

#[test]
fn clarification_feedback_names_unsupported_choices() {
    let assessment = TaskContractAssessment {
        decision: TaskContractReviewDecision::Clarify,
        unsupported_decisions: vec!["delivery format".to_string(), "time horizon".to_string()],
    };

    let error = assessment.into_allowed().expect_err("review must deny");
    assert!(error.contains("delivery format"));
    assert!(error.contains("time horizon"));
}

#[test]
fn request_user_input_output_is_labelled_as_user_evidence() {
    let items = [
        ResponseItem::FunctionCall {
            id: None,
            name: "request_user_input".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            encrypted_function_args: None,
            call_id: "question".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("question".to_string()),
            output: FunctionCallOutputPayload::from_text("Focused review".to_string()),
            name: None,
            namespace: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let transcript = render_review_transcript(items.iter());
    assert!(transcript.contains("user_answer:"));
    assert!(transcript_supports_evidence(&transcript, "Focused review"));
}
