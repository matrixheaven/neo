use super::*;

#[tokio::test]
async fn live_cancel_guard_cancels_own_token_without_removing_replacement() {
    let runtime = MultiAgentRuntime::new();
    let parent = CancellationToken::new();
    let first = runtime.register_live_cancel("agent_test", &parent);
    let first_token = first.token();
    let second = runtime.register_live_cancel("agent_test", &parent);
    let second_token = second.token();

    drop(first);

    assert!(
        first_token.is_cancelled(),
        "dropping a live-cancel guard should stop its parent bridge"
    );
    assert!(
        !second_token.is_cancelled(),
        "dropping an old live-cancel guard must not cancel a newer run token"
    );
    assert!(
        runtime
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .agent_cancel_tokens
            .contains_key("agent_test"),
        "dropping an old live-cancel guard must not remove a newer run token"
    );

    drop(second);

    assert!(
        second_token.is_cancelled(),
        "dropping the active live-cancel guard should stop its parent bridge"
    );
    assert!(
        !runtime
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .agent_cancel_tokens
            .contains_key("agent_test"),
        "dropping the active live-cancel guard should unregister its token"
    );
}

#[test]
fn live_delivery_rejects_terminal_agent_even_if_steer_registered() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("terminal child");
    let _ = runtime.complete_delegate_for_test(&agent.id, "done");
    let _registration = runtime.register_live_steer(agent.id.as_str());
    let outcome = runtime.deliver_live_message(
        agent.id.as_str(),
        &crate::multi_agent::DelegateMailboxMessage {
            id: "msg_after_terminal".to_owned(),
            text: "should not deliver".to_owned(),
            delivered: false,
        },
    );
    assert_eq!(
        outcome,
        LiveMessageDelivery::NotRunning,
        "terminal agents must never report Delivered, even with a stale steer handle"
    );
}

#[test]
fn live_delivery_unregister_race_never_reports_false_delivered() {
    let runtime = MultiAgentRuntime::new();
    let registration = runtime.register_live_steer("agent_race");
    let orphan_handle = registration.handle();

    // Unregister before delivery: the old race cloned the handle under the
    // registry lock, released it, then pushed and reported success even though
    // Drop had already cleared the live entry.
    drop(registration);

    let outcome = runtime.deliver_live_message(
        "agent_race",
        &crate::multi_agent::DelegateMailboxMessage {
            id: "msg_after_unregister".to_owned(),
            text: "should not deliver".to_owned(),
            delivered: false,
        },
    );
    assert_ne!(
        outcome,
        LiveMessageDelivery::Delivered,
        "unregister must not report Delivered"
    );
    assert_eq!(
        orphan_handle.pending(),
        0,
        "orphaned receiver must not accept after unregister"
    );

    // While the registry entry is live, delivery is accepted exactly once.
    let live = runtime.register_live_steer("agent_race");
    let live_handle = live.handle();
    assert_eq!(
        runtime.deliver_live_message(
            "agent_race",
            &crate::multi_agent::DelegateMailboxMessage {
                id: "msg_while_live".to_owned(),
                text: "deliver me".to_owned(),
                delivered: false,
            },
        ),
        LiveMessageDelivery::Delivered
    );
    assert_eq!(live_handle.pending(), 1);

    // After Drop of the live registration, a subsequent push cannot claim success.
    drop(live);
    assert_ne!(
        runtime.deliver_live_message(
            "agent_race",
            &crate::multi_agent::DelegateMailboxMessage {
                id: "msg_after_drop".to_owned(),
                text: "too late".to_owned(),
                delivered: false,
            },
        ),
        LiveMessageDelivery::Delivered
    );
    assert_eq!(live_handle.pending(), 1);
}

#[test]
fn live_steer_guard_does_not_remove_replacement() {
    let runtime = MultiAgentRuntime::new();
    let first = runtime.register_live_steer("agent_test");
    let second = runtime.register_live_steer("agent_test");
    let second_handle = second.handle();

    drop(first);

    assert_eq!(
        runtime.deliver_live_message(
            "agent_test",
            &crate::multi_agent::DelegateMailboxMessage {
                id: "message_test".to_owned(),
                text: "keep replacement".to_owned(),
                delivered: false,
            },
        ),
        LiveMessageDelivery::Delivered
    );
    assert_eq!(second_handle.pending(), 1);

    drop(second);

    assert!(
        !runtime
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .contains_key("agent_test")
    );
}

#[test]
fn live_generation_overflow_panics_without_replacing_steer() {
    let runtime = MultiAgentRuntime::new();
    let existing = runtime.register_live_steer("agent_test");
    let existing_handle = existing.handle();
    runtime
        .state
        .lock()
        .expect("multi-agent state poisoned")
        .next_live_generation = u64::MAX;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.register_live_steer("agent_test")
    }));

    assert!(result.is_err(), "generation overflow must panic");
    let state = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let preserved = state
        .steer_handles
        .get("agent_test")
        .expect("existing steer entry must remain");
    assert_eq!(preserved.generation, existing.generation);
    drop(state);
    runtime.state.clear_poison();
    assert_eq!(
        runtime.deliver_live_message(
            "agent_test",
            &crate::multi_agent::DelegateMailboxMessage {
                id: "message_after_overflow".to_owned(),
                text: "keep existing handle".to_owned(),
                delivered: false,
            },
        ),
        LiveMessageDelivery::Delivered
    );
    assert_eq!(existing_handle.pending(), 1);
}
