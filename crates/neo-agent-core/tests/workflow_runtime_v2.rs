//! V2 workflow identity, state, and error contract tests.

use neo_agent_core::workflow::{
    WORKFLOW_NAME_MAX_LEN, WorkflowArtifactId, WorkflowCheckpoint, WorkflowError,
    WorkflowErrorCode, WorkflowHumanHandle, WorkflowInvocationId, WorkflowName, WorkflowRequestId,
    WorkflowRevision, WorkflowRunId, WorkflowSourceOrigin, WorkflowState, validate_portable_name,
};

#[test]
fn workflow_v2_identity_rejects_invalid_names() {
    // Valid portable names.
    let max_valid = "a".repeat(WORKFLOW_NAME_MAX_LEN);
    for name in ["a", "review", "review-2", "phase_1", max_valid.as_str()] {
        WorkflowName::parse(name).unwrap_or_else(|e| panic!("expected ok for {name:?}: {e}"));
        WorkflowHumanHandle::parse(name)
            .unwrap_or_else(|e| panic!("expected handle ok for {name:?}: {e}"));
        validate_portable_name(name, "workflow name").unwrap();
    }

    // Invalid: empty, uppercase, unicode, leading separator, too long, illegal chars.
    let too_long = "a".repeat(WORKFLOW_NAME_MAX_LEN + 1);
    let invalid = [
        "",
        "Review",
        "review.2",
        "-leading",
        "_leading",
        "has space",
        "emoji-😀",
        too_long.as_str(),
        "slash/name",
        "dot.name",
    ];
    for name in invalid {
        let err = WorkflowName::parse(name).expect_err("must reject");
        assert_eq!(err.code(), WorkflowErrorCode::InvalidInput, "name={name:?}");
        let err = WorkflowHumanHandle::parse(name).expect_err("handle must reject");
        assert_eq!(
            err.code(),
            WorkflowErrorCode::InvalidInput,
            "handle={name:?}"
        );
    }

    // Run ID: UUID machine key for V2; opaque V1 strings stay readable via from_existing.
    let generated = WorkflowRunId::generate();
    assert!(
        generated.as_str().starts_with("wf_"),
        "generated id should use wf_ prefix"
    );
    WorkflowRunId::parse_v2(generated.as_str()).expect("generated id parses");
    WorkflowRunId::parse_v2("00000000-0000-4000-8000-000000000001").expect("hyphen UUID");
    WorkflowRunId::parse_v2("00000000000040008000000000000001").expect("simple hex");
    let bad = WorkflowRunId::parse_v2("not-a-uuid").expect_err("reject garbage");
    assert_eq!(bad.code(), WorkflowErrorCode::InvalidInput);
    let v1 = WorkflowRunId::from_existing("run_legacy_opaque");
    assert_eq!(v1.as_str(), "run_legacy_opaque");

    // Revision must be lowercase sha-256 hex.
    let rev = WorkflowRevision::from_bytes(b"neo-workflow");
    assert_eq!(rev.as_str().len(), 64);
    WorkflowRevision::parse(rev.as_str()).unwrap();
    let bad_rev = WorkflowRevision::parse("not-hex").expect_err("reject");
    assert_eq!(bad_rev.code(), WorkflowErrorCode::InvalidInput);
    let upper = "A".repeat(64);
    assert!(WorkflowRevision::parse(&upper).is_err());

    // Other identity wrappers construct and display.
    let inv = WorkflowInvocationId::generate();
    assert!(inv.as_str().starts_with("inv_"));
    let req = WorkflowRequestId::generate();
    assert!(req.as_str().starts_with("req_"));
    let art = WorkflowArtifactId::new(generated.clone(), rev.as_str()).unwrap();
    assert_eq!(art.as_content_sha256(), rev.as_str());
    let ckpt = WorkflowCheckpoint::new(generated, 3, rev.as_str()).unwrap();
    assert_eq!(ckpt.sequence, 3);

    // V2 states and transitions.
    assert!(!WorkflowState::Queued.is_terminal());
    assert!(!WorkflowState::AwaitingUser.is_terminal());
    assert!(WorkflowState::Completed.is_terminal());
    assert!(WorkflowState::Queued.rehydrates_as_paused_host_exit());
    assert!(WorkflowState::Running.rehydrates_as_paused_host_exit());
    assert!(!WorkflowState::AwaitingUser.rehydrates_as_paused_host_exit());
    assert!(!WorkflowState::AwaitingUser.allows_ordinary_resume());
    assert!(WorkflowState::Paused.allows_ordinary_resume());

    assert!(WorkflowState::Queued.can_transition_to(WorkflowState::Running));
    assert!(WorkflowState::Running.can_transition_to(WorkflowState::AwaitingUser));
    assert!(WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Queued));
    assert!(!WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Running));
    assert!(!WorkflowState::Completed.can_transition_to(WorkflowState::Running));
    assert_eq!(WorkflowState::AwaitingUser.as_str(), "awaiting_user");
    assert_eq!(WorkflowState::Queued.as_str(), "queued");

    // Stable error codes are not message-parsed.
    let coded = WorkflowError::coded(WorkflowErrorCode::LineageMismatch, "prefix diverged");
    assert_eq!(coded.code(), WorkflowErrorCode::LineageMismatch);
    assert_eq!(
        WorkflowErrorCode::LineageMismatch.as_str(),
        "lineage_mismatch"
    );
    assert_eq!(
        WorkflowError::InvalidInput("x".into()).code(),
        WorkflowErrorCode::InvalidInput
    );

    // Source origin labels are stable.
    assert_eq!(WorkflowSourceOrigin::Builtin.as_str(), "builtin");
    assert_eq!(WorkflowSourceOrigin::Project.as_str(), "project");
}
