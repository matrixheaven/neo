use futures::StreamExt;
use neo_agent_core::{AgentConfig, AgentContext, AgentMessage, AgentRuntime, FakeHarness};
use neo_ai::{AiStreamEvent, StopReason};

#[tokio::main]
async fn main() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello from fake".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);

    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();
    let mut events = runtime.run_turn(&mut context, AgentMessage::user_text("hello"));

    while let Some(event) = events.next().await {
        println!("{:?}", event.unwrap());
    }
}
