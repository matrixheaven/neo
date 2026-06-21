use std::path::Path;

use crate::AgentEvent;

pub fn encode_event(event: &AgentEvent) -> serde_json::Result<String> {
    serde_json::to_string(event)
}

pub fn decode_event(line: &str) -> serde_json::Result<AgentEvent> {
    serde_json::from_str(line)
}

pub async fn append_event(path: &Path, event: &AgentEvent) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    ensure_parent_dir(path).await?;
    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await?;
    file.write_all(encode_event(event).expect("agent event should serialize").as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

async fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}
