use neo_ai::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ReadInput {
    path: std::path::PathBuf,
}

fn main() {
    let tool = ToolSpec::from_schema::<ReadInput>("read", "Read a UTF-8 file from the workspace.");

    println!(
        "{}",
        serde_json::to_string_pretty(&tool.input_schema).unwrap()
    );
}
