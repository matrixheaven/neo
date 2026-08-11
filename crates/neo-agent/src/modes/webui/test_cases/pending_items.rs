//! Pending approvals and questions: triple-match one-time resolution,
//! duplicate rejection, closed senders, and answer retry semantics.

use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalRequest, PermissionOperation, QuestionEventData,
    QuestionOptionData,
};

use super::state_fixtures::test_state;
use super::*;

fn sample_approval(id: &str) -> (RunPendingApproval, oneshot::Receiver<ApprovalResponse>) {
    let request = ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Tool,
        presentation: ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: vec!["tool: read".to_owned()],
        },
        options: vec![ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        }],
        workflow_origin: None,
    };
    let (response_tx, response_rx) = oneshot::channel();
    (
        RunPendingApproval {
            request,
            response_tx,
        },
        response_rx,
    )
}

fn sample_question(question: &str) -> QuestionEventData {
    QuestionEventData {
        question: question.to_owned(),
        header: None,
        body: None,
        options: vec![
            QuestionOptionData {
                label: "Yes".to_owned(),
                description: None,
            },
            QuestionOptionData {
                label: "No".to_owned(),
                description: None,
            },
        ],
        multi_select: false,
    }
}

#[test]
fn approval_resolution_requires_triple_match_and_single_winner() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (response_tx, mut response_rx) = oneshot::channel();
    {
        let mut guard = state.lock().expect("state lock");
        let pending = RunPendingApproval {
            request: ApprovalRequest {
                turn: 1,
                id: "approval_1".to_owned(),
                operation: PermissionOperation::Tool,
                presentation: ApprovalPresentation::Tool {
                    title: "Run tool?".to_owned(),
                    details: vec!["tool: read".to_owned()],
                },
                options: vec![ApprovalOption {
                    label: "Approve once".to_owned(),
                    description: None,
                    action: ApprovalAction::PermitOnce,
                }],
                workflow_origin: None,
            },
            response_tx,
        };
        assert!(!guard.register_approval(pending));
        assert!(guard.waiting_approval);
    }
    // Wrong turn id.
    assert_eq!(
        resolve_approval(
            &state,
            "turn_other",
            "approval_1",
            ApprovalAction::PermitOnce,
            None
        )
        .expect_err("wrong turn rejected")
        .code,
        WebUiErrorCode::StaleControl
    );
    // Wrong request id.
    assert_eq!(
        resolve_approval(
            &state,
            "turn_1",
            "approval_other",
            ApprovalAction::PermitOnce,
            None
        )
        .expect_err("wrong request rejected")
        .code,
        WebUiErrorCode::StaleControl
    );
    // The single correct resolver wins exactly once.
    assert!(
        resolve_approval(
            &state,
            "turn_1",
            "approval_1",
            ApprovalAction::PermitOnce,
            None
        )
        .is_ok()
    );
    match response_rx.try_recv() {
        Ok(ApprovalResponse::Selected {
            request_id, action, ..
        }) => {
            assert_eq!(request_id, "approval_1");
            assert_eq!(action, ApprovalAction::PermitOnce);
        }
        other => panic!("expected selected response, got {other:?}"),
    }
    assert!(!state.lock().expect("state lock").waiting_approval);
    // A second resolver loses: the one-time sender was already taken.
    assert_eq!(
        resolve_approval(
            &state,
            "turn_1",
            "approval_1",
            ApprovalAction::PermitOnce,
            None
        )
        .expect_err("second resolver rejected")
        .code,
        WebUiErrorCode::StaleControl
    );
}

#[tokio::test]
async fn concurrent_approval_resolutions_have_exactly_one_winner() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (response_tx, response_rx) = oneshot::channel();
    {
        let mut guard = state.lock().expect("state lock");
        let pending = RunPendingApproval {
            request: ApprovalRequest {
                turn: 1,
                id: "approval_1".to_owned(),
                operation: PermissionOperation::Tool,
                presentation: ApprovalPresentation::Tool {
                    title: "Run tool?".to_owned(),
                    details: vec!["tool: read".to_owned()],
                },
                options: vec![ApprovalOption {
                    label: "Approve once".to_owned(),
                    description: None,
                    action: ApprovalAction::PermitOnce,
                }],
                workflow_origin: None,
            },
            response_tx,
        };
        assert!(!guard.register_approval(pending));
    }
    let _kept_open = response_rx;
    let first = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            resolve_approval(
                &state,
                "turn_1",
                "approval_1",
                ApprovalAction::PermitOnce,
                None,
            )
        })
    };
    let second = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            resolve_approval(
                &state,
                "turn_1",
                "approval_1",
                ApprovalAction::PermitOnce,
                None,
            )
        })
    };
    let outcomes = [
        first.await.expect("first task"),
        second.await.expect("second task"),
    ];
    let winners = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(winners, 1, "exactly one concurrent resolver wins");
}

#[test]
fn closed_approval_senders_are_never_surfaced() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    {
        let mut guard = state.lock().expect("state lock");
        let pending = RunPendingApproval {
            request: ApprovalRequest {
                turn: 1,
                id: "approval_closed".to_owned(),
                operation: PermissionOperation::Tool,
                presentation: ApprovalPresentation::Tool {
                    title: "Run tool?".to_owned(),
                    details: Vec::new(),
                },
                options: Vec::new(),
                workflow_origin: None,
            },
            response_tx: oneshot::channel().0,
        };
        // The receiver side was dropped, so the sender is closed.
        assert!(!guard.register_approval(pending));
        assert!(guard.pending_approval.is_none());
        assert!(!guard.waiting_approval);
    }
}

#[test]
fn empty_question_answer_keeps_the_pending_question_for_retry() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (response_tx, mut response_rx) = oneshot::channel();
    {
        let mut guard = state.lock().expect("state lock");
        let pending = PendingQuestion {
            id: "question_1".to_owned(),
            questions: vec![sample_question("Continue?")],
            workflow_origin: None,
            response_tx,
        };
        assert!(!guard.register_question(pending));
        assert!(guard.waiting_question);
    }
    // An empty answer (no selections, no text) is invalid and must not
    // consume the one-time sender.
    assert_eq!(
        resolve_question(
            &state,
            "turn_1",
            "question_1",
            neo_webui::protocol::WebUiQuestionAnswer {
                selections: Vec::new(),
                text: None,
            },
        )
        .expect_err("empty answer rejected")
        .code,
        WebUiErrorCode::InvalidRequest
    );
    assert!(
        state.lock().expect("state lock").waiting_question,
        "the pending question survives an invalid answer"
    );
    // Whitespace-only text is empty too.
    assert_eq!(
        resolve_question(
            &state,
            "turn_1",
            "question_1",
            neo_webui::protocol::WebUiQuestionAnswer {
                selections: Vec::new(),
                text: Some("   ".to_owned()),
            },
        )
        .expect_err("blank answer rejected")
        .code,
        WebUiErrorCode::InvalidRequest
    );
    // A legal retry wins the sender.
    assert!(
        resolve_question(
            &state,
            "turn_1",
            "question_1",
            neo_webui::protocol::WebUiQuestionAnswer {
                selections: vec!["Yes".to_owned()],
                text: None,
            },
        )
        .is_ok()
    );
    match response_rx.try_recv() {
        Ok(QuestionResponse { answers }) => assert_eq!(answers, vec!["Yes".to_owned()]),
        other => panic!("expected the retried answer, got {other:?}"),
    }
    assert!(!state.lock().expect("state lock").waiting_question);
}

#[test]
fn pending_questions_keep_order_and_resolve_independently() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (first_tx, mut first_rx) = oneshot::channel();
    let (second_tx, mut second_rx) = oneshot::channel();
    {
        let mut guard = state.lock().expect("state lock");
        assert!(!guard.register_question(PendingQuestion {
            id: "question_1".to_owned(),
            questions: vec![sample_question("Continue?")],
            workflow_origin: None,
            response_tx: first_tx,
        }));
        assert!(!guard.register_question(PendingQuestion {
            id: "question_2".to_owned(),
            questions: vec![
                sample_question("Run tests?"),
                sample_question("Run review?")
            ],
            workflow_origin: None,
            response_tx: second_tx,
        }));
        assert_eq!(
            guard
                .pending_questions
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["question_1", "question_2"]
        );
    }

    assert_eq!(
        resolve_question(
            &state,
            "turn_1",
            "question_1",
            neo_webui::protocol::WebUiQuestionAnswer {
                selections: vec!["Yes".to_owned(), "No".to_owned()],
                text: None,
            },
        )
        .expect_err("answer count mismatch rejected")
        .code,
        WebUiErrorCode::InvalidRequest
    );
    assert_eq!(
        state.lock().expect("state lock").pending_questions.len(),
        2,
        "an invalid answer must not consume either sender"
    );

    resolve_question(
        &state,
        "turn_1",
        "question_2",
        neo_webui::protocol::WebUiQuestionAnswer {
            selections: vec!["Yes".to_owned(), "No".to_owned()],
            text: None,
        },
    )
    .expect("second question resolves independently");
    assert_eq!(
        second_rx.try_recv().expect("second answer").answers,
        vec!["Yes".to_owned(), "No".to_owned()]
    );
    {
        let guard = state.lock().expect("state lock");
        assert_eq!(guard.pending_questions.len(), 1);
        assert_eq!(guard.pending_questions[0].id, "question_1");
        assert!(guard.waiting_question);
    }

    resolve_question(
        &state,
        "turn_1",
        "question_1",
        neo_webui::protocol::WebUiQuestionAnswer {
            selections: vec!["Yes".to_owned()],
            text: None,
        },
    )
    .expect("first question still resolves");
    assert_eq!(
        first_rx.try_recv().expect("first answer").answers,
        vec!["Yes".to_owned()]
    );
    let guard = state.lock().expect("state lock");
    assert!(guard.pending_questions.is_empty());
    assert!(!guard.waiting_question);
}

#[test]
fn duplicate_pending_items_fail_the_turn_instead_of_overwriting() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    {
        let mut guard = state.lock().expect("state lock");
        let (first, _first_rx) = sample_approval("approval_1");
        assert!(!guard.register_approval(first));
        let (second, _second_rx) = sample_approval("approval_1");
        assert!(guard.register_approval(second), "duplicate is an anomaly");
        assert!(guard.turn_error.is_some());
    }
}
