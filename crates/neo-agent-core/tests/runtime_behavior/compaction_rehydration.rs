use super::context::event_index;
use super::context::finished_tool_results;
use super::context::instruction_epochs;
use super::context::instruction_fixture;
use super::context::reconcile_defer_epoch;
use super::fake_harness::end_turn_events;
use super::fake_harness::run_turn_collect;
use super::fake_harness::tool_call_turn;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings,
    CompactionSummary, Content, InstructionContextBridge, PermissionMode, StopReason, ToolRegistry,
    harness::FakeHarness,
    instructions::{
        InstructionEpochOutcome, InstructionPreflightDecision, InstructionReconcileKind,
        InstructionReconcileRequest, InstructionRegistry, InstructionRegistryConfig,
    },
};
use neo_ai::{AiError, ChatRequest};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) fn chat_request_text(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .map(|message| {
            let content = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            content
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn request_contains_exact_text(request: &ChatRequest, expected: &str) -> bool {
    request_exact_text_count(request, expected) > 0
}

pub(crate) fn request_exact_text_count(request: &ChatRequest, expected: &str) -> usize {
    request
        .messages
        .iter()
        .map(|message| {
            let content = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            content
                .iter()
                .filter(
                    |part| matches!(part, neo_ai::ContentPart::Text { text } if text == expected),
                )
                .count()
        })
        .sum()
}

pub(crate) async fn apply_preflight_baseline(
    fixture: &PreflightFixture,
    config: &AgentConfig,
    context: &mut AgentContext,
) -> String {
    let InstructionPreflightDecision::Defer { epoch, fingerprint } = fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::Baseline,
                target_directories: Vec::new(),
                budget: InstructionContextBridge::budget(config, context),
                deferred_tool_ids: Vec::new(),
            },
            context.instruction_state(),
        )
        .await
    else {
        panic!("expected baseline Defer")
    };
    let authority = epoch.model_content.clone().expect("baseline authority");
    InstructionContextBridge::apply_epoch(context, &epoch, &fingerprint);
    authority
}

#[tokio::test]
async fn threshold_compaction_rehydrates_exact_authority_before_same_request() {
    let fixture = preflight_fixture(&[], "# exact threshold authority\n");
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let baseline_config = preflight_config(&fixture, &harness);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &baseline_config, &mut context).await;
    let mut config = baseline_config.with_compaction(CompactionSettings {
        trigger_ratio: 0.05,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 1)
    });
    config.model.capabilities.max_context_tokens = Some(200_000);
    context.append_message(AgentMessage::user_text("history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("previous answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "go",
    )
    .await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let requests = harness.requests();
    assert_eq!(requests.len(), 2, "summary then continued request");
    assert!(
        request_contains_exact_text(&requests[1], &authority),
        "the first provider request after threshold compaction must contain the exact authority"
    );
}

#[tokio::test]
async fn history_pressure_compacts_before_baseline_selection_and_user_append() {
    let root_rules = format!("# exact resumed authority\n{}\n", "r".repeat(64_000));
    let fixture = preflight_fixture(&[], &root_rules);
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    context.append_message(AgentMessage::user_text(format!(
        "old history {}",
        "x".repeat(40_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "next prompt",
    )
    .await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("history compaction");
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended { message }
                if message.text() == "next prompt"
        )
    })
    .expect("new user message");
    assert!(compaction_index < epoch_index && epoch_index < user_index);
    let epoch = instruction_epochs(&events)[0];
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Ready);
    assert!(epoch.ignored_bundles.is_empty());
    let authority = epoch
        .model_content
        .as_deref()
        .expect("full baseline authority");
    assert!(request_contains_exact_text(
        &harness.requests()[1],
        authority
    ));
}

#[tokio::test]
async fn history_pressure_compacts_before_blocked_baseline_notice_and_user_append() {
    let fixture = preflight_fixture(&[], "@./missing.md\n");
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.3,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    context.append_message(AgentMessage::user_text("old history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "next prompt",
    )
    .await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("history compaction");
    let epoch_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::InstructionEpoch { epoch }
                if epoch.outcome == InstructionEpochOutcome::Blocked
        )
    })
    .expect("Blocked baseline epoch");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended { message }
                if message.text() == "next prompt"
        )
    })
    .expect("new user message");
    assert!(compaction_index < epoch_index && epoch_index < user_index);
    let blocked = instruction_epochs(&events)[0]
        .model_content
        .as_deref()
        .expect("Blocked notice");
    assert!(request_contains_exact_text(&harness.requests()[1], blocked));
}

#[tokio::test]
async fn overflow_recovery_rehydrates_exact_authority_before_retry_request() {
    let fixture = preflight_fixture(&[], "# exact overflow authority\n");
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        end_turn_events("summary output")
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>(),
        end_turn_events("recovered")
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>(),
    ]);
    let mut config = preflight_config(&fixture, &harness)
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.model.capabilities.max_context_tokens = Some(200_000);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text("old history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "go",
    )
    .await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let requests = harness.requests();
    assert_eq!(requests.len(), 3, "initial, summary, retry");
    assert!(
        request_contains_exact_text(&requests[2], &authority),
        "the overflow retry request must contain the exact authority"
    );
}

#[tokio::test]
async fn retained_blocked_notice_does_not_replace_compacted_authority() {
    let fixture = preflight_fixture(&[], "# exact prior authority\n");
    let harness = FakeHarness::from_turns([end_turn_events("continued")]);
    let config = preflight_config(&fixture, &harness);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text("ordinary history"));
    std::fs::write(fixture.workspace.join("AGENTS.md"), "@./missing.md\n")
        .expect("break root bundle");
    let InstructionPreflightDecision::Block {
        epoch: blocked,
        fingerprint: blocked_fingerprint,
    } = fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![fixture.workspace.clone()],
                budget: InstructionContextBridge::budget(&config, &context),
                deferred_tool_ids: vec!["blocked-call".to_owned()],
            },
            context.instruction_state(),
        )
        .await
    else {
        panic!("expected Block")
    };
    let blocked_notice = blocked.model_content.clone().expect("blocked notice");
    InstructionContextBridge::apply_epoch(&mut context, &blocked, &blocked_fingerprint);
    let blocked_index = context.messages().len() - 1;
    context.apply_compaction(CompactionSummary {
        summary: "summary".to_owned(),
        tokens_before: 100,
        tokens_after: 10,
        first_kept_message_index: blocked_index,
    });

    run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "continue",
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_exact_text_count(&requests[0], &authority),
        1,
        "the complete authority snapshot must be present exactly once"
    );
    assert_eq!(
        request_exact_text_count(&requests[0], &blocked_notice),
        1,
        "rehydration must preserve exactly one current Blocked notice"
    );
}

pub(crate) async fn run_manual_compaction_collect(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
) {
    let events = runtime
        .run_manual_compaction_turn(context)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("compaction turn should succeed");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "expected a CompactionApplied event"
    );
}

#[tokio::test]
async fn compaction_excludes_instruction_bodies_and_rehydrates_exact_bytes() {
    const INSTRUCTION_SENTINEL: &str = "INSTRUCTION-SENTINEL-4f8c2e-rules";
    const ORDINARY_SENTINEL: &str = "ORDINARY-SENTINEL-91bd7a-history";

    let fixture = instruction_fixture(&[], &format!("# rules\n{INSTRUCTION_SENTINEL}\n"));
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    // Pin the workspace baseline epoch carrying the instruction sentinel.
    let (epoch, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![fixture.workspace.clone()]).await;
    let model_content = epoch
        .model_content
        .clone()
        .expect("baseline epoch carries model content");
    assert!(model_content.contains(INSTRUCTION_SENTINEL));
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);

    // Ordinary history with its own sentinel, then a manual compaction.
    context.append_message(AgentMessage::user_text(format!(
        "please remember {ORDINARY_SENTINEL}"
    )));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    context.append_message(AgentMessage::user_text("and now something else"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("done")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    run_manual_compaction_collect(&runtime, &mut context).await;

    // The summary request excludes the instruction body but still summarizes
    // ordinary history.
    let requests = harness.requests();
    assert_eq!(requests.len(), 1);
    let summary_text = chat_request_text(&requests[0]);
    assert!(
        !summary_text.contains(INSTRUCTION_SENTINEL),
        "summary input must exclude pinned instruction bodies: {summary_text}"
    );
    assert!(
        summary_text.contains(ORDINARY_SENTINEL),
        "summary input must keep ordinary history: {summary_text}"
    );

    // Rehydrate the exact current rules from registry state.
    let repinned =
        InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
            .await
            .expect("rehydration succeeds");
    assert!(repinned, "current instruction chain must be re-pinned");

    run_turn_collect(&runtime, &mut context, "continue working").await;

    // The post-compaction request contains the byte-identical instruction
    // content exactly once.
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let post_compaction = chat_request_text(&requests[1]);
    assert_eq!(
        post_compaction.matches(INSTRUCTION_SENTINEL).count(),
        1,
        "instruction sentinel must appear exactly once: {post_compaction}"
    );
    let pinned = requests[1]
        .messages
        .iter()
        .map(|message| {
            let parts = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            parts
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .find(|text| text.contains(INSTRUCTION_SENTINEL))
        .expect("one message carries the rehydrated sentinel");
    assert_eq!(
        pinned, model_content,
        "rehydrated content must be byte-identical to the epoch content"
    );
}

#[tokio::test]
async fn compaction_rehydration_never_admits_previously_ignored_bundle() {
    const ROOT: &str = "ROOT-ADMITTED-41b7";
    const IGNORED: &str = "NESTED-IGNORED-e230";
    let nested_rules = format!("{IGNORED} {}\n", "large ".repeat(2_000));
    let fixture = instruction_fixture(&[("nested", &nested_rules)], &format!("{ROOT}\n"));
    let nested = fixture.workspace.join("nested");
    let request = InstructionReconcileRequest {
        agent_id: "main".to_owned(),
        kind: InstructionReconcileKind::ToolPreflight,
        target_directories: vec![nested],
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 512,
        },
        deferred_tool_ids: vec!["call-1".to_owned()],
    };
    let context = &mut AgentContext::new();
    let (epoch, fingerprint) = match fixture
        .registry
        .reconcile(request, context.instruction_state())
        .await
    {
        InstructionPreflightDecision::Defer { epoch, fingerprint } => (epoch, fingerprint),
        InstructionPreflightDecision::Proceed { .. } => {
            panic!("expected partially loaded epoch, got Proceed")
        }
        InstructionPreflightDecision::Block { epoch, .. } => {
            panic!(
                "expected partially loaded epoch, got Block: {:?}",
                epoch.failure
            )
        }
    };
    assert_eq!(epoch.outcome, InstructionEpochOutcome::PartiallyLoaded);
    assert!(
        epoch
            .model_content
            .as_deref()
            .is_some_and(|body| body.contains(ROOT)),
        "{epoch:?}"
    );
    assert!(
        epoch
            .model_content
            .as_deref()
            .is_some_and(|body| !body.contains(IGNORED))
    );
    InstructionContextBridge::apply_epoch(context, &epoch, &fingerprint);

    InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, context)
        .await
        .expect("rehydration succeeds");

    let pinned = context
        .messages()
        .iter()
        .filter(|message| message.is_injection_variant("instruction_epoch"))
        .map(AgentMessage::text)
        .collect::<String>();
    assert!(
        pinned.contains(ROOT),
        "admitted root must remain pinned: {pinned}"
    );
    assert!(
        !pinned.contains(IGNORED),
        "ignored nested bundle must remain unpinned: {pinned}"
    );
    assert_eq!(
        context.instruction_state().visited_revisions,
        context.instruction_state().visible_revisions,
        "ignored bundles must never enter agent-local visited history"
    );
}

#[tokio::test]
async fn compacted_sibling_scope_reactivates_when_reentered() {
    let fixture = instruction_fixture(
        &[("a", "ALPHA-RULES-a11\n"), ("b", "BETA-RULES-b22\n")],
        "ROOT-RULES-c01\n",
    );
    let scope_a = fixture.workspace.join("a");
    let scope_b = fixture.workspace.join("b");
    let harness = FakeHarness::from_turns([end_turn_events("summary output")]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    // Activate sibling scope A, then sibling scope B.
    let (epoch_a, fingerprint_a) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_a.clone()]).await;
    assert_eq!(epoch_a.outcome, InstructionEpochOutcome::Activated);
    InstructionContextBridge::apply_epoch(&mut context, &epoch_a, &fingerprint_a);
    let (epoch_b, fingerprint_b) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_b.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut context, &epoch_b, &fingerprint_b);
    context.append_message(AgentMessage::user_text("working in b"));

    // Compact while B is current, then rehydrate from registry state.
    run_manual_compaction_collect(&runtime, &mut context).await;
    let repinned =
        InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
            .await
            .expect("rehydration succeeds");
    assert!(repinned);

    // A remains cached but unpinned; the current chain (root + B) is pinned.
    let pinned = context
        .messages()
        .iter()
        .filter(|message| message.is_injection_variant("instruction_epoch"))
        .map(AgentMessage::text)
        .collect::<Vec<_>>()
        .concat();
    assert!(
        !pinned.contains("ALPHA-RULES"),
        "sibling A must stay unpinned"
    );
    assert!(
        pinned.contains("ROOT-RULES"),
        "workspace baseline is rehydrated"
    );
    assert!(
        pinned.contains("BETA-RULES"),
        "current scope B is rehydrated"
    );

    let state = context.instruction_state();
    assert_eq!(state.active_scopes, vec![fixture.workspace.clone()]);
    assert_eq!(state.most_recent_scope.as_deref(), Some(scope_b.as_path()));
    for scope in [&fixture.workspace, &scope_b] {
        assert!(
            state.visible_revisions.contains_key(scope),
            "current authority retained for {}",
            scope.display()
        );
    }
    assert!(!state.visible_revisions.contains_key(&scope_a));
    for scope in [&fixture.workspace, &scope_a, &scope_b] {
        assert!(
            state.visited_revisions.contains_key(scope),
            "visited metadata retained for {}",
            scope.display()
        );
    }

    // Re-entering A emits exactly one Reactivated epoch.
    let (epoch_reentry, fingerprint_reentry) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_a.clone()]).await;
    assert_eq!(epoch_reentry.outcome, InstructionEpochOutcome::Reactivated);
    assert!(
        epoch_reentry
            .model_content
            .as_deref()
            .is_some_and(|content| content.contains("ALPHA-RULES")),
        "re-entry re-pins A's exact content"
    );
    InstructionContextBridge::apply_epoch(&mut context, &epoch_reentry, &fingerprint_reentry);

    // The identical probe afterwards proceeds silently — no second epoch.
    let decision = fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![scope_a],
                budget: InstructionContextBridge::budget(&config, &context),
                deferred_tool_ids: vec!["call-1".to_owned()],
            },
            context.instruction_state(),
        )
        .await;
    assert!(
        matches!(decision, InstructionPreflightDecision::Proceed { .. }),
        "an unchanged scope must not emit a second epoch"
    );
}

#[tokio::test]
async fn replayed_compacted_sibling_reactivates_with_fresh_registry() {
    let fixture = instruction_fixture(
        &[("a", "ALPHA-RULES-a11\n"), ("b", "BETA-RULES-b22\n")],
        "ROOT-RULES-c01\n",
    );
    let scope_a = fixture.workspace.join("a");
    let scope_b = fixture.workspace.join("b");
    let harness = FakeHarness::from_turns([end_turn_events("summary output")]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());

    let mut live = AgentContext::new();
    let (epoch_a, fingerprint_a) =
        reconcile_defer_epoch(&fixture, &config, &live, vec![scope_a.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut live, &epoch_a, &fingerprint_a);
    let (epoch_b, _fingerprint_b) =
        reconcile_defer_epoch(&fixture, &config, &live, vec![scope_b.clone()]).await;

    let replay_events = [
        AgentEvent::InstructionEpoch { epoch: epoch_a },
        AgentEvent::InstructionEpoch {
            epoch: epoch_b.clone(),
        },
    ];
    let mut replayed = AgentContext::from_replay(replay_events.iter());
    for scope in [&fixture.workspace, &scope_a, &scope_b] {
        assert!(
            replayed
                .instruction_state()
                .visited_revisions
                .contains_key(scope),
            "replay retained {}",
            scope.display()
        );
    }

    let fresh_registry = InstructionRegistry::new(InstructionRegistryConfig {
        primary_workspace: fixture.workspace.clone(),
        neo_home: None,
        project_trusted: true,
    })
    .expect("fresh registry");
    fresh_registry.restore_epoch(&epoch_b);
    replayed.append_message(AgentMessage::user_text("working in b after replay"));
    run_manual_compaction_collect(&runtime, &mut replayed).await;
    InstructionContextBridge::rehydrate_after_compaction(&fresh_registry, &mut replayed)
        .await
        .expect("fresh-registry rehydration");

    assert!(
        replayed
            .instruction_state()
            .visited_revisions
            .contains_key(&scope_a),
        "rehydration must preserve replayed agent-local history"
    );
    let decision = fresh_registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![scope_a],
                budget: InstructionContextBridge::budget(&config, &replayed),
                deferred_tool_ids: vec!["call-1".to_owned()],
            },
            replayed.instruction_state(),
        )
        .await;
    let InstructionPreflightDecision::Defer { epoch, .. } = decision else {
        panic!("re-entering replayed sibling must defer")
    };
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Reactivated);
}

pub(crate) struct PreflightFixture {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) workspace: PathBuf,
    pub(crate) registry: Arc<InstructionRegistry>,
}

pub(crate) fn preflight_fixture(nested: &[(&str, &str)], root_rules: &str) -> PreflightFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("AGENTS.md"), root_rules).expect("root AGENTS.md");
    for (dir, rules) in nested {
        let nested_dir = workspace.join(dir);
        std::fs::create_dir_all(&nested_dir).expect("nested dir");
        std::fs::write(nested_dir.join("AGENTS.md"), rules).expect("nested AGENTS.md");
    }
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let registry = InstructionRegistry::new(InstructionRegistryConfig {
        primary_workspace: workspace.clone(),
        neo_home: None,
        project_trusted: true,
    })
    .expect("registry");
    PreflightFixture {
        _temp: temp,
        workspace,
        registry: Arc::new(registry),
    }
}

pub(crate) fn preflight_context(fixture: &PreflightFixture) -> AgentContext {
    let mut context = AgentContext::new();
    context.attach_instruction_registry(Arc::clone(&fixture.registry));
    context
}

pub(crate) fn preflight_config(fixture: &PreflightFixture, harness: &FakeHarness) -> AgentConfig {
    let mut config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&fixture.workspace)
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Auto);
    config.instruction_registry = Some(Arc::clone(&fixture.registry));
    config
}

pub(crate) fn assert_pending_epoch_events<'a>(
    events: &'a [AgentEvent],
    target: &std::path::Path,
    nested_sentinel: &str,
) -> (&'a str, &'a str, u64) {
    let epochs = instruction_epochs(events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus nested activation: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["call_1".to_owned()]);
    let baseline_model_content = epochs[0]
        .model_content
        .as_deref()
        .expect("baseline epoch carries model content");
    let nested_model_content = epochs[1]
        .model_content
        .as_deref()
        .expect("activated epoch carries model content");
    assert!(nested_model_content.contains(nested_sentinel));

    let compacted_index = event_index(events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("one compaction");
    let epoch_index = event_index(events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.outcome == InstructionEpochOutcome::Activated)
    })
    .expect("activated epoch index");
    assert!(
        compacted_index < epoch_index,
        "compaction must precede the pending epoch admission: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::CompactionStarted { .. }))
            .count(),
        1,
        "no summarize-after-inject: exactly one compaction: {events:?}"
    );

    let deferred = finished_tool_results(events, "call_1");
    assert_eq!(deferred.len(), 1, "call_1");
    assert!(!deferred[0].is_error, "deferred result must be non-error");
    assert_eq!(
        deferred[0].details.as_ref().expect("deferred details")["status"],
        "deferred"
    );
    assert_eq!(
        std::fs::read_to_string(target).expect("read target"),
        "beta"
    );

    (
        baseline_model_content,
        nested_model_content,
        epochs[1].generation,
    )
}

pub(crate) fn assert_compaction_request_inputs(
    harness: &FakeHarness,
    nested_rules: &str,
    root_sentinel: &str,
    nested_sentinel: &str,
    ordinary_sentinel: &str,
) {
    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        4,
        "turn, summary, post-admission, final: {requests:?}"
    );
    let summary_input = chat_request_text(&requests[1]);
    assert!(
        summary_input.contains(ordinary_sentinel),
        "ordinary history is summarized: {summary_input}"
    );
    assert!(
        !summary_input.contains(nested_sentinel),
        "pending epoch bytes must never enter the summary input: {summary_input}"
    );
    assert!(
        !summary_input.contains(root_sentinel),
        "pinned baseline bodies stay out of the summary input: {summary_input}"
    );

    let post_admission = chat_request_text(&requests[2]);
    assert_eq!(
        post_admission.matches(nested_sentinel).count(),
        1,
        "nested instruction bytes appear exactly once: {post_admission}"
    );
    assert!(
        post_admission.contains(nested_rules),
        "the nested AGENTS.md body is preserved byte-for-byte: {post_admission}"
    );
}

pub(crate) fn assert_rehydrated_instruction_context(
    context: &AgentContext,
    baseline_model_content: &str,
    nested_model_content: &str,
    _nested_generation: u64,
) {
    let pinned: Vec<String> = context
        .messages()
        .iter()
        .filter(|message| message.is_injection_variant("instruction_epoch"))
        .map(AgentMessage::text)
        .collect();
    assert_eq!(
        pinned,
        vec![
            baseline_model_content.to_owned(),
            nested_model_content.to_owned(),
        ],
        "rehydrate-then-admit must preserve instruction bytes exactly"
    );
}

#[tokio::test]
async fn context_pressure_compacts_before_pending_epoch_admission() {
    const ROOT_SENTINEL: &str = "ROOT-SENTINEL-c4d9e1-rules";
    const NESTED_SENTINEL: &str = "NESTED-SENTINEL-77aa10-rules";
    const ORDINARY_SENTINEL: &str = "ORDINARY-SENTINEL-0f3b55-history";

    // Token economics (builtin tool schemas ≈ 15_400 tokens, workspace
    // overhead ≈ 250): in a 32_000-token window the trigger is 25_600. The
    // ~7_500 tokens of ordinary history keep the first request below the
    // trigger (~23_200), but admitting the ~4_000-token nested epoch on top
    // (~27_200) crosses it — so compact-first admission must run. After
    // compaction the request shrinks below the trigger (~15_700 with the
    // epoch), so the epoch is admitted without a second compaction.
    let nested_rules = format!(
        "# nested rules\n{NESTED_SENTINEL}\n{}\n",
        "n".repeat(16_000)
    );
    let fixture = preflight_fixture(
        &[("nested", &nested_rules)],
        &format!("# root rules\n{ROOT_SENTINEL}\n"),
    );
    let target = fixture.workspace.join("nested").join("target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta"
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        end_turn_events("summary of earlier work"),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    // ~7_500 tokens of ordinary history carrying its own sentinel.
    context.append_message(AgentMessage::user_text(format!(
        "please remember {ORDINARY_SENTINEL} {}",
        "x".repeat(30_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = match runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("edit the nested file"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(events) => events,
        Err(error) => {
            panic!("turn should succeed: {error}");
        }
    };
    let (baseline_model_content, nested_model_content, nested_generation) =
        assert_pending_epoch_events(&events, &target, NESTED_SENTINEL);
    assert_compaction_request_inputs(
        &harness,
        &nested_rules,
        ROOT_SENTINEL,
        NESTED_SENTINEL,
        ORDINARY_SENTINEL,
    );
    assert_rehydrated_instruction_context(
        &context,
        baseline_model_content,
        nested_model_content,
        nested_generation,
    );
}

#[tokio::test]
async fn history_pressure_compacts_before_whole_bundle_omission() {
    let nested_rules = format!("# exact nested authority\n{}\n", "n".repeat(96_000));
    let fixture = preflight_fixture(&[("nested", &nested_rules)], "# root authority\n");
    let target = fixture.workspace.join("nested/target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta"
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        end_turn_events("summary output"),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.99,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text(format!(
        "ordinary history {}",
        "x".repeat(40_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = run_turn_collect(&runtime, &mut context, "edit the nested file").await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "ordinary history must compact before an applicable bundle is omitted: {events:?}"
    );
    let epoch = instruction_epochs(&events)
        .into_iter()
        .find(|epoch| epoch.outcome == InstructionEpochOutcome::Activated)
        .expect("nested authority activates after fresh admission");
    assert!(epoch.ignored_bundles.is_empty());
    let authority = epoch.model_content.as_deref().expect("nested authority");
    let requests = harness.requests();
    assert!(
        request_contains_exact_text(&requests[2], authority),
        "post-compaction admission must use the fresh byte-exact authority"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionStarted { id, .. } if id == "call_2"
        )),
        "retried edit never started; requests={}, epochs={:?}",
        requests.len(),
        instruction_epochs(&events)
            .iter()
            .map(|epoch| epoch.outcome)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(target).expect("target contents"),
        "beta",
        "same-turn replan must execute the retried edit: {events:?}"
    );
}

#[tokio::test]
async fn post_tool_instruction_update_compacts_before_fresh_admission() {
    const UPDATED_SENTINEL: &str = "POST-TOOL-UPDATED-AUTHORITY";
    let fixture = preflight_fixture(&[], "old root rules\n");
    let updated_rules = format!("{UPDATED_SENTINEL}\n{}\n", "r".repeat(120_000));
    let agents_path = fixture.workspace.join("AGENTS.md");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_agents",
            "Write",
            json!({
                "path": agents_path.to_string_lossy(),
                "content": updated_rules,
            }),
        )]),
        end_turn_events("summary output"),
        end_turn_events("continued with updated rules"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.99,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(100_000);
    let mut context = preflight_context(&fixture);
    apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text(format!(
        "ordinary history {}",
        "h".repeat(160_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("history acknowledged")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = run_turn_collect(&runtime, &mut context, "replace the root instructions").await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("post-tool update must compact ordinary history");
    let updated_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::InstructionEpoch { epoch }
                if epoch.outcome == InstructionEpochOutcome::Updated
                    && epoch.ignored_bundles.is_empty()
                    && epoch.model_content.as_deref().is_some_and(|content| content.contains(UPDATED_SENTINEL))
        )
    })
    .expect("fresh post-compaction Updated epoch");
    assert!(compaction_index < updated_index, "events: {events:#?}");
    assert_eq!(
        harness.requests().len(),
        3,
        "tool request, compaction summary, continued request"
    );
}
