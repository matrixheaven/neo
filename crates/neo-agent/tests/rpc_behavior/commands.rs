use super::sessions::{isolated_home, neo, parse_jsonl, run_with_stdin, write_home_config};

use tempfile::TempDir;

#[test]
fn rpc_get_commands_returns_local_prompt_template_commands() {
    // Prompt templates now live only under the single neo home (~/.neo/prompts).
    // There is no project tier, so configured selectors + user prompts are the
    // only sources.
    let project = TempDir::new().expect("project tempdir");
    write_home_config(
        r#"
prompt_templates = ["prompts"]
"#,
    );
    // Configured prompt template (relative selector resolved against home).
    let configured = isolated_home().join("prompts");
    std::fs::create_dir_all(&configured).expect("create configured prompts");
    std::fs::write(
        configured.join("review.md"),
        r#"---
description: Review a target
argument-hint: "<path>"
---
Review $1
"#,
    )
    .expect("write configured prompt template");
    // User prompt templates (auto-discovered from ~/.neo/prompts).
    let user_prompts = isolated_home().join("prompts");
    std::fs::write(user_prompts.join("explain.md"), "Explain the target\n")
        .expect("write user prompt template");

    let mut command = neo();
    command.current_dir(project.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"commands-1","method":"get_commands","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "commands-1");
    let commands = messages[0]["result"]["commands"]
        .as_array()
        .expect("commands array");
    let names = commands
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert!(names.contains(&"/explain"));
    assert!(names.contains(&"/review"));

    let review = commands
        .iter()
        .find(|command| command["name"] == "/review")
        .expect("review command");
    assert_eq!(review["kind"], "prompt_template");
    assert_eq!(review["template"], "review");
    assert_eq!(review["description"], "Review a target");
    assert_eq!(review["argument_hint"], "<path>");
}

#[test]
fn rpc_get_commands_omits_excluded_auto_discovered_prompt_template() {
    let project = TempDir::new().expect("project tempdir");
    let prompts_dir = isolated_home().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts");
    std::fs::write(prompts_dir.join("review.md"), "Review should be excluded\n")
        .expect("write excluded prompt template");
    std::fs::write(prompts_dir.join("fix.md"), "Fix remains\n")
        .expect("write kept prompt template");
    write_home_config(
        r#"
prompt_templates = ["-prompts/review.md"]
"#,
    );

    let mut command = neo();
    command.current_dir(project.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"commands-1","method":"get_commands","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    let commands = messages[0]["result"]["commands"]
        .as_array()
        .expect("commands array");
    let names = commands
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/fix"]);
}
