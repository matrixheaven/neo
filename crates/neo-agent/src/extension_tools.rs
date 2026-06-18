use std::path::{Path, PathBuf};

use anyhow::Context as _;
use neo_agent_core::{Tool, ToolContext, ToolError, ToolFuture, ToolRegistry, ToolResult};
use neo_extensions::{
    DiscoveredExtension, ExtensionDiscovery, ExtensionLifecycleStore, ExtensionRunner,
    ExtensionStatus, ExtensionTransport,
};
use neo_sdk::{RpcOutcome, RpcRequest};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize)]
struct ExtensionToolSpec {
    name: String,
    description: String,
    input_schema: Value,
    method: String,
}

pub(crate) async fn register_enabled_extension_tools(
    registry: &mut ToolRegistry,
    root: &Path,
    state_path: &Path,
) -> anyhow::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let lifecycle_store = ExtensionLifecycleStore::new(state_path);
    for extension in ExtensionDiscovery::new(root)
        .discover()
        .with_context(|| format!("failed to discover extensions under {}", root.display()))?
    {
        let lifecycle = lifecycle_store.status(root, &extension.manifest.id)?;
        if lifecycle.status == ExtensionStatus::Disabled {
            continue;
        }
        for tool in discover_extension_tools(&extension).await? {
            registry.register(ExtensionTool {
                name: extension_tool_name(&extension.manifest.id, &tool.name),
                description: tool.description,
                input_schema: tool.input_schema,
                extension_id: extension.manifest.id.clone(),
                transport: extension.manifest.transport.clone(),
                method: tool.method,
            });
        }
    }
    Ok(())
}

async fn discover_extension_tools(
    extension: &DiscoveredExtension,
) -> anyhow::Result<Vec<ExtensionToolSpec>> {
    let mut runner = ExtensionRunner::spawn(extension.manifest.transport.clone())?;
    let response = runner
        .request(RpcRequest::new(
            format!("{}:tools.list", extension.manifest.id),
            "tools.list",
            Value::Object(Map::default()),
        ))
        .await?;
    let RpcOutcome::Success { result } = response.outcome else {
        anyhow::bail!(
            "extension {} returned tools.list failure",
            extension.manifest.id
        );
    };
    let tools = serde_json::from_value::<Vec<ExtensionToolSpec>>(result).with_context(|| {
        format!(
            "extension {} returned invalid tools.list result",
            extension.manifest.id
        )
    })?;
    for tool in &tools {
        validate_input_schema(&extension.manifest.id, tool)?;
    }
    Ok(tools)
}

fn validate_input_schema(extension_id: &str, tool: &ExtensionToolSpec) -> anyhow::Result<()> {
    let object = tool.input_schema.as_object().with_context(|| {
        format!(
            "extension {extension_id} tool {} input_schema must be a JSON Schema object",
            tool.name
        )
    })?;
    if let Some(schema_type) = object.get("type") {
        validate_schema_type(schema_type).with_context(|| {
            format!(
                "extension {extension_id} tool {} input_schema has invalid JSON Schema type",
                tool.name
            )
        })?;
    }
    for key in ["properties", "patternProperties", "$defs", "definitions"] {
        if object.get(key).is_some_and(|value| !value.is_object()) {
            anyhow::bail!(
                "extension {extension_id} tool {} input_schema `{key}` must be an object",
                tool.name
            );
        }
    }
    Ok(())
}

fn validate_schema_type(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::String(schema_type) => validate_schema_type_name(schema_type),
        Value::Array(schema_types) => {
            if schema_types.is_empty() {
                anyhow::bail!("type array must not be empty");
            }
            for schema_type in schema_types {
                let Some(schema_type) = schema_type.as_str() else {
                    anyhow::bail!("type array entries must be strings");
                };
                validate_schema_type_name(schema_type)?;
            }
            Ok(())
        }
        _ => anyhow::bail!("type must be a string or string array"),
    }
}

fn validate_schema_type_name(schema_type: &str) -> anyhow::Result<()> {
    match schema_type {
        "null" | "boolean" | "object" | "array" | "number" | "string" | "integer" => Ok(()),
        _ => anyhow::bail!("unsupported JSON Schema type `{schema_type}`"),
    }
}

#[derive(Clone)]
struct ExtensionTool {
    name: String,
    description: String,
    input_schema: Value,
    extension_id: String,
    transport: ExtensionTransport,
    method: String,
}

impl Tool for ExtensionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            execute_extension_tool(
                &self.name,
                &self.extension_id,
                self.transport.clone(),
                &self.method,
                input,
            )
            .await
        })
    }
}

async fn execute_extension_tool(
    tool_name: &str,
    extension_id: &str,
    transport: ExtensionTransport,
    method: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    let mut runner =
        ExtensionRunner::spawn(transport).map_err(|err| extension_tool_error(tool_name, err))?;
    let response = runner
        .request(RpcRequest::new(
            format!("{extension_id}:{method}"),
            method,
            input,
        ))
        .await
        .map_err(|err| extension_tool_error(tool_name, err))?;
    let RpcOutcome::Success { result } = response.outcome else {
        return Ok(ToolResult::error(format!(
            "extension {extension_id} returned failure for {method}"
        )));
    };
    Ok(tool_result_from_value(&result))
}

fn tool_result_from_value(value: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct WireToolResult {
        content: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        details: Option<Value>,
        #[serde(default)]
        terminate: bool,
    }

    match serde_json::from_value::<WireToolResult>(value.clone()) {
        Ok(wire) => {
            let result = if wire.is_error {
                ToolResult::error(wire.content)
            } else {
                ToolResult::ok(wire.content)
            };
            let result = if let Some(details) = wire.details {
                result.with_details(details)
            } else {
                result
            };
            if wire.terminate {
                result.terminate()
            } else {
                result
            }
        }
        Err(_) => ToolResult::ok(value.to_string()),
    }
}

fn extension_tool_error(tool_name: &str, err: impl std::fmt::Display) -> ToolError {
    ToolError::InvalidInput {
        tool: tool_name.to_owned(),
        message: err.to_string(),
    }
}

fn extension_tool_name(extension_id: &str, tool_name: &str) -> String {
    format!(
        "extension__{}__{}",
        sanitize_tool_name(extension_id),
        sanitize_tool_name(tool_name)
    )
}

fn sanitize_tool_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn default_extension_root(project_dir: &Path) -> PathBuf {
    project_dir.join(".neo/extensions")
}

pub(crate) fn default_extension_state_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".neo/extensions-state.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extension_tool_name_sanitizes_extension_and_tool_ids() {
        assert_eq!(
            extension_tool_name("my.extension", "tools/echo-text"),
            "extension__my_extension__tools_echo_text"
        );
    }

    #[test]
    fn tool_result_from_value_accepts_wire_tool_result_object() {
        let result = tool_result_from_value(&json!({
            "content": "hello",
            "details": {"source": "test"},
            "terminate": true
        }));

        assert_eq!(result.content, "hello");
        assert!(!result.is_error);
        assert_eq!(result.details, Some(json!({"source": "test"})));
        assert!(result.terminate);
    }
}
