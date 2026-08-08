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
