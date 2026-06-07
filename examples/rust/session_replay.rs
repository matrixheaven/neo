use neo_agent_core::{
    AgentEvent, AgentMessage, Content, StopReason,
    session::{JsonlSessionReader, JsonlSessionWriter},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join("neo-session-example.jsonl");
    let mut writer = JsonlSessionWriter::create(&path).await?;

    writer
        .append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("hello"),
        })
        .await?;
    writer
        .append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("hello back")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        })
        .await?;
    writer.flush().await?;

    let messages = JsonlSessionReader::replay_messages(&path).await?;
    println!("replayed {} messages", messages.len());
    Ok(())
}
