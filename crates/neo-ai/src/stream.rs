use crate::{AiError, AiStreamEvent};

pub fn collect_tool_arguments(
    events: &[AiStreamEvent],
    tool_call_id: &str,
) -> Result<serde_json::Value, AiError> {
    let mut preview = String::new();
    let mut saw_delta = false;

    for event in events {
        match event {
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } if id == tool_call_id => {
                saw_delta = true;
                preview.push_str(json_fragment);
            }
            AiStreamEvent::ToolCallEnd { id, raw_arguments } if id == tool_call_id => {
                return parse_tool_arguments(raw_arguments);
            }
            _ => {}
        }
    }

    if !saw_delta {
        return Err(AiError::Stream {
            message: format!("missing tool arguments for tool call {tool_call_id}"),
        });
    }

    parse_tool_arguments(&preview)
}

fn parse_tool_arguments(raw: &str) -> Result<serde_json::Value, AiError> {
    serde_json::from_str(raw).map_err(|err| AiError::Stream {
        message: format!("invalid tool arguments: {err}"),
    })
}
