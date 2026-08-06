use super::*;

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::user_text(text)
}

#[test]
fn strategy_should_compact_below_threshold() {
    let strategy = CompactionStrategy::default();
    assert!(!strategy.should_compact(1000, 100_000));
}

#[test]
fn strategy_should_compact_above_threshold() {
    let strategy = CompactionStrategy::default();
    // trigger_ratio = 0.85, so 85000+ should compact at 100000 max
    assert!(strategy.should_compact(86_000, 100_000));
}

#[test]
fn strategy_reserved_context_forces_compact() {
    let strategy = CompactionStrategy {
        reserved_context_tokens: 50_000,
        ..CompactionStrategy::default()
    };
    // used=60000, reserved=50000, max=100000 → 60000+50000 >= 100000 → compact
    assert!(strategy.should_compact(60_000, 100_000));
}

#[test]
fn estimate_tokens_grows_with_content() {
    let short = user_msg("hi");
    let long = user_msg(&"x".repeat(1000));
    assert!(estimate_message_tokens(&long) > estimate_message_tokens(&short));
}
