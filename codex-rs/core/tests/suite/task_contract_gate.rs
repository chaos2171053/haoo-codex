#![allow(clippy::unwrap_used)]

use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::request_user_input::RequestUserInputAnswer;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_message_item_added;
use core_test_support::responses::ev_output_text_delta;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use test_case::test_case;

fn streamed_assistant_message(id: &str, text: &str, phase: Option<&str>) -> Vec<serde_json::Value> {
    let mut added = ev_message_item_added(id, "");
    let mut done = ev_assistant_message(id, text);
    if let Some(phase) = phase {
        added["item"]["phase"] = json!(phase);
        done["item"]["phase"] = json!(phase);
    }
    vec![added, ev_output_text_delta(text), done]
}

fn assert_no_assistant_message(event: &EventMsg) {
    match event {
        EventMsg::AgentMessage(_) | EventMsg::AgentMessageContentDelta(_) => {
            panic!("unreviewed message was shown: {event:?}")
        }
        EventMsg::ItemStarted(event) if matches!(event.item, TurnItem::AgentMessage(_)) => {
            panic!("unreviewed message was started: {event:?}")
        }
        EventMsg::ItemCompleted(event) if matches!(event.item, TurnItem::AgentMessage(_)) => {
            panic!("unreviewed message was completed: {event:?}")
        }
        _ => {}
    }
}

#[test_case("allow", None; "revised question is shown")]
#[test_case(
    "clarify",
    Some("clarification could not be completed because the model repeatedly proposed unsupported task candidates");
    "rejections stop with their actual reason"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_contract_answer_review_preserves_revision_state(
    final_decision: &str,
    expected_error: Option<&str>,
) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    const QUESTION: &str = "What scope would you like me to assess?";

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::DefaultModeRequestUserInput)
                .unwrap();
            config
                .features
                .enable(Feature::DefaultModeTaskContract)
                .unwrap();
        })
        .build(&server)
        .await?;

    let mut candidate_mocks = Vec::new();
    for round in 0..3 {
        let candidate = if round == 2 {
            QUESTION
        } else {
            "I will assess the whole organization over five years."
        };
        candidate_mocks.push(
            responses::mount_sse_once(
                &server,
                sse(vec![
                    ev_assistant_message(&format!("candidate-{round}"), candidate),
                    ev_completed(&format!("main-{round}")),
                ]),
            )
            .await,
        );
        let decision = if round == 2 {
            final_decision
        } else {
            "clarify"
        };
        let unsupported_decisions = if decision == "allow" {
            vec![]
        } else {
            vec!["The user has not established the proposed scope."]
        };
        responses::mount_sse_once(
            &server,
            sse(vec![
                ev_assistant_message(
                    &format!("assessment-{round}"),
                    &json!({"decision": decision, "unsupported_decisions": unsupported_decisions})
                        .to_string(),
                ),
                ev_completed(&format!("review-{round}")),
            ]),
        )
        .await;
    }

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Assess this proposal.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut messages = Vec::new();
    let completion = loop {
        match test.codex.next_event().await?.msg {
            EventMsg::TurnComplete(completion) => break completion,
            EventMsg::AgentMessage(message) => messages.push(message.message),
            _ => {}
        }
    };
    assert_eq!(
        completion
            .error
            .as_ref()
            .map(|error| error.message.as_str()),
        expected_error
    );
    assert_eq!(
        messages,
        if expected_error.is_none() {
            vec![QUESTION.to_string()]
        } else {
            vec![]
        }
    );
    for candidate in candidate_mocks.iter().skip(1) {
        assert!(
            candidate
                .single_request()
                .body_contains_text("The user has not established the proposed scope.")
        );
    }
    Ok(())
}

#[test_case("Inspect the named file and report its first line.")]
#[test_case("Please help with this file.\nInspect the named file and report its first line.")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_contract_unlocks_tools_only_after_independent_review(
    user_message: &str,
) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::DefaultModeRequestUserInput)
                .unwrap();
            config
                .features
                .enable(Feature::DefaultModeTaskContract)
                .unwrap();
        })
        .build(&server)
        .await?;

    let contract_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call(
                "contract-call",
                "submit_task_contract",
                &json!({
                    "result": "Inspect the named file.",
                    "boundary": "Read only the named file.",
                    "completion": "Report its first line.",
                    "evidence": ["Inspect the named file and report its first line."]
                })
                .to_string(),
            ),
            ev_completed("main-contract"),
        ]),
    )
    .await;
    let reviewer_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message(
                "review-result",
                r#"{"decision":"allow","unsupported_decisions":[]}"#,
            ),
            ev_completed("review"),
        ]),
    )
    .await;
    let work_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call("work-call", "test_sync_tool", "{}"),
            ev_completed("main-work"),
        ]),
    )
    .await;
    let final_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("final", "first line"),
            ev_completed("main-final"),
        ]),
    )
    .await;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: user_message.to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = [contract_mock, reviewer_mock, work_mock, final_mock]
        .into_iter()
        .flat_map(|mock| mock.requests())
        .collect::<Vec<_>>();
    let reviewer = requests
        .iter()
        .find(|request| {
            request.body_json()["client_metadata"]["x-openai-subagent"].as_str()
                == Some("task_contract_reviewer")
        })
        .expect("independent task contract review request");
    assert!(reviewer.body_contains_text("Inspect the named file and report its first line."));

    let main_requests = requests
        .iter()
        .filter(|request| {
            request.body_json()["client_metadata"]["x-openai-subagent"]
                .as_str()
                .is_none()
        })
        .collect::<Vec<_>>();
    let initial_request = main_requests[0].body_json();
    let initial_tools = initial_request["tools"].as_array().expect("initial tools");
    let initial_tool_names = initial_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        initial_tool_names,
        vec!["request_user_input", "submit_task_contract"]
    );
    assert!(main_requests[1].body_contains_text("contract-call"));
    assert!(main_requests[2].body_contains_text("work-call"));

    Ok(())
}

#[test_case(false, false; "valid submission")]
#[test_case(true, false; "repaired submission")]
#[test_case(false, true; "oversized submission")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_contract_clarification_completes_before_work_starts(
    repair_arguments: bool,
    oversized_submission: bool,
) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    const USER_NOTE: &str = "Only \"payments\".\nPath: C:\\work\\repo";

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::DefaultModeRequestUserInput)
                .unwrap();
            config
                .features
                .enable(Feature::DefaultModeTaskContract)
                .unwrap();
        })
        .build(&server)
        .await?;

    let mut repair_mocks = Vec::new();
    if oversized_submission {
        repair_mocks.push(
            responses::mount_sse_once(
                &server,
                sse(vec![
                    ev_function_call(
                        "oversized-contract",
                        "submit_task_contract",
                        &json!({
                            "result": "Assess this proposal. ".repeat(3_000),
                            "boundary": "Proposal", "completion": "Assessment",
                            "evidence": ["Assess this proposal."]
                        })
                        .to_string(),
                    ),
                    ev_completed("oversized-contract"),
                ]),
            )
            .await,
        );
    }
    if repair_arguments {
        for (call_id, args) in [
            (
                "missing-result",
                json!({"boundary": "Proposal", "completion": "Assessment", "evidence": ["Assess this proposal."]}),
            ),
            (
                "invalid-evidence",
                json!({"result": "Assess", "boundary": "Proposal", "completion": "Assessment", "evidence": ["The user said: Assess this proposal."]}),
            ),
        ] {
            repair_mocks.push(
                responses::mount_sse_once(
                    &server,
                    sse(vec![
                        ev_function_call(call_id, "submit_task_contract", &args.to_string()),
                        ev_completed(call_id),
                    ]),
                )
                .await,
            );
        }
    }

    let contract_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call(
                "unsupported-contract",
                "submit_task_contract",
                &json!({
                    "result": "Produce a long-term organization-wide assessment.",
                    "boundary": "Cover every department over five years.",
                    "completion": "Deliver a full report and implementation schedule.",
                    "evidence": ["Assess this proposal."]
                })
                .to_string(),
            ),
            ev_completed("main-contract"),
        ]),
    )
    .await;
    let reviewer_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message(
                "review-result",
                r#"{"decision":"clarify","unsupported_decisions":["organization-wide scope","five-year horizon","full report and implementation schedule"]}"#,
            ),
            ev_completed("review"),
        ]),
    )
    .await;
    let question_mock = responses::mount_sse_once(
        &server,
        sse([
            streamed_assistant_message(
                "unsupported-plan",
                "I will deliver a five-year full report.",
                Some("commentary"),
            ),
            vec![
                ev_function_call(
                    "clarify-scope",
                    "request_user_input",
                    &json!({
                        "questions": [{
                            "id": "scope",
                            "header": "Scope",
                            "question": "What scope and deliverable do you want?",
                            "options": [{
                                "label": "Focused review (Recommended)",
                                "description": "Assess only the named proposal."
                            }, {
                                "label": "Full report",
                                "description": "Define a broader report before work starts."
                            }]
                        }]
                    })
                    .to_string(),
                ),
                ev_completed("main-question"),
            ],
        ]
        .concat()),
    )
    .await;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Assess this proposal.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let question = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::RequestUserInput(question) => Some(question.clone()),
        _ => {
            assert_no_assistant_message(event);
            None
        }
    })
    .await;

    let requests = repair_mocks
        .into_iter()
        .chain([contract_mock, reviewer_mock, question_mock])
        .flat_map(|mock| mock.requests())
        .collect::<Vec<_>>();
    assert!(requests.iter().all(|request| {
        !request.body_json()["input"]
            .to_string()
            .contains("test_sync_tool")
    }));
    let question_request = requests
        .iter()
        .find(|request| request.body_contains_text("organization-wide scope"))
        .expect("review feedback must return to the main model");
    assert!(question_request.body_contains_text("five-year horizon"));
    if oversized_submission {
        assert!(
            requests
                .iter()
                .any(|request| request.body_contains_text("shorten the candidate"))
        );
        assert!(requests.iter().filter(|request| {
            request.body_json()["client_metadata"]["x-openai-subagent"].as_str().is_some()
        }).all(|request| !request.body_contains_text(&"Assess this proposal. ".repeat(3_000))));
    }

    if repair_arguments {
        assert!(
            requests
                .iter()
                .any(|request| request.body_contains_text("missing field `result`"))
        );
        assert!(
            requests
                .iter()
                .any(|request| request.body_contains_text("Copy a verbatim excerpt"))
        );
    }
    for request in requests.iter().filter(|request| {
        request.body_json()["client_metadata"]["x-openai-subagent"]
            .as_str()
            .is_none()
    }) {
        let body = request.body_json();
        let names = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["request_user_input", "submit_task_contract"]);
    }

    let clarified_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call(
                "clarified-contract",
                "submit_task_contract",
                &json!({
                    "result": "Assess the proposal.",
                    "boundary": "Only the named proposal.",
                    "completion": "A focused assessment.",
                    "evidence": ["Assess this proposal.", "Focused review (Recommended)", USER_NOTE]
                })
                .to_string(),
            ),
            ev_completed("clarified-contract"),
        ]),
    )
    .await;
    let allow_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message(
                "allow",
                r#"{"decision":"allow","unsupported_decisions":[]}"#,
            ),
            ev_completed("allow"),
        ]),
    )
    .await;
    let work_mock = responses::mount_sse_once(
        &server,
        sse([
            streamed_assistant_message(
                "reviewed-progress",
                "I am assessing the named proposal.",
                Some("commentary"),
            ),
            vec![
                ev_function_call("work-call", "test_sync_tool", "{}"),
                ev_completed("work"),
            ],
        ]
        .concat()),
    )
    .await;
    let final_mock = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("final", "Focused assessment complete."),
            ev_completed("final"),
        ]),
    )
    .await;

    test.codex
        .submit(Op::UserInputAnswer {
            id: question.turn_id,
            response: RequestUserInputResponse {
                answers: HashMap::from([(
                    "scope".to_string(),
                    RequestUserInputAnswer {
                        answers: vec![
                            "Focused review (Recommended)".to_string(),
                            format!("user_note: {USER_NOTE}"),
                        ],
                    },
                )]),
            },
        })
        .await?;
    let mut progress_shown = false;
    loop {
        let event = test.codex.next_event().await?.msg;
        match event {
            EventMsg::TurnComplete(_) => break,
            EventMsg::AgentMessage(message) => {
                progress_shown |= message.message == "I am assessing the named proposal.";
                assert!(!message.message.contains("five-year"));
            }
            _ => {}
        }
    }
    assert!(progress_shown, "reviewed work progress must remain visible");
    assert!(
        clarified_mock
            .single_request()
            .body_contains_text("Focused review (Recommended)")
    );
    assert!(
        allow_mock
            .single_request()
            .body_contains_text("user_answer:")
    );
    assert!(
        allow_mock
            .single_request()
            .body_contains_text("assistant_question:")
    );
    assert!(
        allow_mock
            .single_request()
            .body_contains_text("What scope and deliverable do you want?")
    );
    assert!(
        work_mock
            .single_request()
            .body_contains_text("clarified-contract")
    );
    let (content, _) = final_mock
        .single_request()
        .function_call_output_content_and_success("work-call")
        .unwrap();
    assert_eq!(content.as_deref(), Some("ok"));

    Ok(())
}

#[test_case(None; "untagged")]
#[test_case(Some("final_answer"); "final answer")]
#[test_case(Some("commentary"); "commentary")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_direct_answer_is_not_shown(phase: Option<&str>) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::DefaultModeRequestUserInput)
                .unwrap();
            config
                .features
                .enable(Feature::DefaultModeTaskContract)
                .unwrap();
        })
        .build(&server)
        .await?;

    responses::mount_sse_once(
        &server,
        sse([
            streamed_assistant_message(
                "unsupported-answer",
                "I will deliver a five-year full report.",
                phase,
            ),
            vec![ev_completed("main-answer")],
        ]
        .concat()),
    )
    .await;
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message(
                "review-result",
                r#"{"decision":"clarify","unsupported_decisions":["five-year horizon","full report"]}"#,
            ),
            ev_completed("review"),
        ]),
    )
    .await;
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call(
                "clarify-output",
                "request_user_input",
                &json!({
                    "questions": [{
                        "id": "output",
                        "header": "Output",
                        "question": "What output do you need?",
                        "options": [{
                            "label": "Short assessment (Recommended)",
                            "description": "Answer the stated question directly."
                        }, {
                            "label": "Full report",
                            "description": "Define report scope first."
                        }]
                    }]
                })
                .to_string(),
            ),
            ev_completed("main-question"),
        ]),
    )
    .await;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Assess this proposal.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    loop {
        match test.codex.next_event().await?.msg {
            EventMsg::RequestUserInput(_) => break,
            event => assert_no_assistant_message(&event),
        }
    }
    test.codex.submit(Op::Interrupt).await?;

    Ok(())
}
