use crate::{AiError, AiStreamEvent};

pub fn collect_tool_arguments(
    events: &[AiStreamEvent],
    tool_call_id: &str,
) -> Result<serde_json::Value, AiError> {
    let mut out = String::new();
    let mut saw_delta = false;

    for event in events {
        match event {
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } if id == tool_call_id => {
                saw_delta = true;
                out.push_str(json_fragment);
            }
            AiStreamEvent::ToolCallEnd { id, arguments } if id == tool_call_id => {
                return Ok(arguments.clone());
            }
            _ => {}
        }
    }

    if !saw_delta {
        return Err(AiError::Stream {
            message: format!("missing tool arguments for tool call {tool_call_id}"),
        });
    }

    serde_json::from_str(&out).map_err(|err| AiError::Stream {
        message: format!("invalid tool arguments: {err}"),
    })
}
