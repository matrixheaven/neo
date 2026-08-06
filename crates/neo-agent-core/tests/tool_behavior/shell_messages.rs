use neo_agent_core::{AgentEvent, AgentMessage, ShellCommandOrigin, ShellCommandOutcome};

#[test]
fn shell_command_message_serializes_as_structured_variant() {
    let message = AgentMessage::shell_command(
        "whoami",
        "chenyuanhao\n",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
        false,
    );
    let json = serde_json::to_value(&message).expect("serialize");
    assert!(json.to_string().contains("ShellCommand"));
    assert!(json.to_string().contains("whoami"));
}

#[test]
fn shell_command_message_converts_to_user_text_for_model() {
    let message = AgentMessage::shell_command(
        "whoami",
        "me\n",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
        false,
    );
    let chat = message.to_chat_message();
    let text = match chat {
        neo_ai::ChatMessage::User { content } => content
            .into_iter()
            .filter_map(|part| match part {
                neo_ai::ContentPart::Text { text } => Some(text),
                _ => None,
            })
            .collect::<String>(),
        _ => panic!("shell command should convert to a user message"),
    };
    assert!(text.contains("<bash-input>"));
    assert!(text.contains("whoami"));
    assert!(text.contains("<bash-stdout>"));
    assert!(text.contains("me"));
    assert!(text.contains("<bash-status"));
    assert!(text.contains("truncated=\"false\""));
}

#[test]
fn shell_command_message_includes_truncation_status_for_model() {
    let message = AgentMessage::shell_command(
        "printf long",
        "long",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
        true,
    );

    assert!(message.text().contains("truncated=\"true\""));
}

#[test]
fn shell_events_include_origin_and_outcome() {
    let started = AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: std::path::PathBuf::from("/tmp"),
        origin: ShellCommandOrigin::UserShellMode,
    };
    let started_json = serde_json::to_string(&started).expect("serialize");
    assert!(started_json.contains("UserShellMode"));

    let finished = AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "me\n".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::UserShellMode,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    };
    let finished_json = serde_json::to_string(&finished).expect("serialize");
    assert!(finished_json.contains("Completed"));
}

#[test]
fn shell_command_message_escapes_xml_text_without_escaping_quotes() {
    let message = AgentMessage::shell_command(
        r#"echo "hello" & <world>"#,
        r#"out "quoted"
"#,
        r"err <x>
",
        Some(0),
        ShellCommandOutcome::Completed,
        false,
    );
    let text = message.text();
    // Element text escapes markup delimiters only.
    assert!(text.contains("&amp;"));
    assert!(text.contains("&lt;"));
    assert!(text.contains("&gt;"));
    assert!(text.contains(r#""hello""#));
    assert!(text.contains(r#""quoted""#));
    assert!(!text.contains("&quot;"));
}
