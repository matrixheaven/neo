use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

struct CountingTool {
    name: &'static str,
    schema_calls: Arc<AtomicUsize>,
}

impl Tool for CountingTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &'static str {
        "count schema calls"
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema_calls.fetch_add(1, Ordering::SeqCst);
        serde_json::json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolResult::ok("ok")) })
    }
}

#[test]
fn specs_are_cached_until_registry_mutates() {
    let first_calls = Arc::new(AtomicUsize::new(0));
    let second_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(CountingTool {
        name: "First",
        schema_calls: Arc::clone(&first_calls),
    });

    assert_eq!(registry.specs().len(), 1);
    assert_eq!(registry.specs().len(), 1);
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);

    registry.register(CountingTool {
        name: "Second",
        schema_calls: Arc::clone(&second_calls),
    });

    assert_eq!(registry.specs().len(), 2);
    assert_eq!(first_calls.load(Ordering::SeqCst), 2);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn snip_hint_registration_and_lookup() {
    struct Hinted {
        hint: SnipHint,
    }
    impl Tool for Hinted {
        fn name(&self) -> &str {
            "Hinted"
        }
        fn description(&self) -> &str {
            "hinted"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(ToolResult::ok("ok")) })
        }
        fn snip_hint(&self) -> Option<SnipHint> {
            Some(self.hint)
        }
    }
    struct Plain;
    impl Tool for Plain {
        fn name(&self) -> &str {
            "Plain"
        }
        fn description(&self) -> &str {
            "plain"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(ToolResult::ok("ok")) })
        }
    }

    let hint = SnipHint {
        head_lines: 120,
        tail_lines: 12,
        head_chars: 12_000,
        tail_chars: 2_000,
    };
    let mut registry = ToolRegistry::default();
    registry.register(Hinted { hint });
    registry.register(Plain);

    assert_eq!(snip_hint_for("Hinted"), Some(hint));
    assert_eq!(snip_hint_for("Plain"), None);
    assert_eq!(snip_hint_for("Missing"), None);
}
