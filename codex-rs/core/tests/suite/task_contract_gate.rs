#![allow(clippy::unwrap_used)]

use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_contract_unlocks_tools_only_after_independent_review() -> anyhow::Result<()> {
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
            text: "Inspect the named file and report its first line.".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_scope_is_clarified_before_work_starts() -> anyhow::Result<()> {
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
        sse(vec![
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
        ]),
    )
    .await;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Assess this proposal.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RequestUserInput(_))
    })
    .await;
    test.codex.submit(Op::Interrupt).await?;

    let requests = [contract_mock, reviewer_mock, question_mock]
        .into_iter()
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

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_direct_answer_is_not_shown() -> anyhow::Result<()> {
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
        sse(vec![
            ev_assistant_message(
                "unsupported-answer",
                "I will deliver a five-year full report.",
            ),
            ev_completed("main-answer"),
        ]),
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
            EventMsg::AgentMessage(message) => {
                panic!("unsupported answer was shown: {}", message.message)
            }
            _ => {}
        }
    }
    test.codex.submit(Op::Interrupt).await?;

    Ok(())
}
