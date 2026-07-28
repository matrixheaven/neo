//! Schema-driven answer form for the Workflow Operator.
//!
//! Renders a durable pending user request as human controls derived from
//! JSON Schema. Supports boolean, string enum, array enum, string, number,
//! integer, object, nested object, and titled `oneOf`/`anyOf`.
//!
//! Unsupported advanced schemas fall back to a validated structured editor.

use serde_json::Value;

/// A primitive answer value that can be incrementally edited.
#[derive(Debug, Clone)]
pub enum AnswerDraft {
    /// Boolean choice (yes/no).
    Boolean { value: bool },
    /// Single-choice from a list of string options.
    SingleChoice { options: Vec<String>, selected: usize },
    /// Multi-choice from a list of string options.
    MultiChoice {
        options: Vec<String>,
        selected: Vec<bool>,
    },
    /// Plain string input.
    Text { value: String },
    /// Multiline text input.
    Multiline { value: String },
    /// Numeric input (integer or number).
    Number { value: String, is_integer: bool },
    /// Object with named fields.
    Object {
        title: Option<String>,
        description: Option<String>,
        fields: Vec<AnswerField>,
        /// Index into `fields` for nested/breadcrumb drill-down.
        focused_field: Option<usize>,
    },
    /// `oneOf`/`anyOf` branch selection.
    BranchChoice {
        branches: Vec<BranchOption>,
        selected_branch: Option<usize>,
    },
    /// Fallback: freeform JSON editor for unsupported schemas.
    Structured { value: String },
}

/// A single branch in a `oneOf`/`anyOf` schema.
#[derive(Debug, Clone)]
pub struct BranchOption {
    pub title: String,
    pub description: Option<String>,
    pub schema: Value,
}

/// A single field inside an object answer form.
#[derive(Debug, Clone)]
pub struct AnswerField {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub default: Option<Value>,
    pub draft: AnswerDraft,
    pub error: Option<String>,
}

/// Top-level answer form state.
#[derive(Debug, Clone)]
pub struct AnswerForm {
    pub request_id: String,
    pub prompt: String,
    pub title: Option<String>,
    pub draft: AnswerDraft,
    /// Validation errors keyed by JSON Pointer path.
    pub errors: Vec<(String, String)>,
}

impl AnswerForm {
    /// Build an answer form from a pending user request schema.
    pub fn from_pending_request(
        request_id: String,
        prompt: String,
        title: Option<String>,
        answer_schema: &Value,
        default: Option<&Value>,
    ) -> Self {
        let draft = build_draft(answer_schema, default);
        Self {
            request_id,
            prompt,
            title,
            draft,
            errors: Vec::new(),
        }
    }

    /// Collect the current draft into a JSON value (for validation or
    /// submission). Returns `None` if the draft is incomplete.
    pub fn collect_value(&self) -> Value {
        collect_draft_value(&self.draft)
    }

    /// Validate the current draft against a compiled schema.
    /// Populates `self.errors`.
    pub fn validate(&mut self, compiled: &neo_agent_core::workflow::CompiledSchema) -> bool {
        self.errors.clear();
        let value = self.collect_value();
        if let Err(err) = compiled.validate_instance(&value) {
            // Attach error to the relevant path.
            let path = if err.instance_path.is_empty() {
                String::new()
            } else {
                err.instance_path.clone()
            };
            self.errors.push((path, err.message));
            return false;
        }
        true
    }
}

/// Build an answer draft from a JSON Schema value.
fn build_draft(schema: &Value, default: Option<&Value>) -> AnswerDraft {
    let schema_type = schema.get("type").and_then(|v| v.as_str());

    // Check for enum first, regardless of type.
    if let Some(enum_values) = schema.get("enum").and_then(|v| v.as_array()) {
        let options: Vec<String> = enum_values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !options.is_empty() {
            let selected = default
                .and_then(|d| d.as_str())
                .and_then(|d| options.iter().position(|o| o == d))
                .unwrap_or(0);
            return AnswerDraft::SingleChoice { options, selected };
        }
    }

    // Check for oneOf/anyOf with titled branches.
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(|v| v.as_array())
    {
        let branch_options: Vec<BranchOption> = branches
            .iter()
            .map(|branch| BranchOption {
                title: branch
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "Untitled option".to_owned()),
                description: branch
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                schema: branch.clone(),
            })
            .collect();
        return AnswerDraft::BranchChoice {
            branches: branch_options,
            selected_branch: None,
        };
    }

    match schema_type {
        Some("boolean") => AnswerDraft::Boolean {
            value: default.and_then(|v| v.as_bool()).unwrap_or(false),
        },
        Some("string") => {
            let default_text = default
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            if schema.get("multiline").and_then(|v| v.as_bool()).unwrap_or(false)
                || schema
                    .get("format")
                    .and_then(|v| v.as_str())
                    == Some("multiline")
            {
                AnswerDraft::Multiline {
                    value: default_text,
                }
            } else {
                AnswerDraft::Text {
                    value: default_text,
                }
            }
        }
        Some("integer") | Some("number") => {
            let is_integer = schema_type == Some("integer");
            let value = default
                .and_then(|v| {
                    if let Some(n) = v.as_i64() {
                        Some(n.to_string())
                    } else {
                        v.as_f64().map(|f| f.to_string())
                    }
                })
                .unwrap_or_default();
            AnswerDraft::Number { value, is_integer }
        }
        Some("array") => {
            // Array of enum items -> multi-choice.
            if let Some(items) = schema.get("items") {
                if let Some(enum_values) = items.get("enum").and_then(|v| v.as_array()) {
                    let options: Vec<String> = enum_values
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    if !options.is_empty() {
                        let default_selected: Vec<bool> = default
                            .and_then(|d| d.as_array())
                            .map(|defaults| {
                                options
                                    .iter()
                                    .map(|o| {
                                        defaults
                                            .iter()
                                            .any(|d| d.as_str() == Some(o.as_str()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_else(|| vec![false; options.len()]);
                        return AnswerDraft::MultiChoice {
                            options,
                            selected: default_selected,
                        };
                    }
                }
                // Array of objects is not supported in the simple form.
                // Fall through to structured fallback.
            }
            AnswerDraft::Structured {
                value: default
                    .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
                    .unwrap_or_else(|| "[]".to_owned()),
            }
        }
        Some("object") => {
            let properties = schema
                .get("properties")
                .and_then(|v| v.as_object());
            if let Some(props) = properties {
                let required: Vec<String> = schema
                    .get("required")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let fields: Vec<AnswerField> = props
                    .iter()
                    .map(|(name, field_schema)| {
                        let field_default = default
                            .and_then(|d| d.get(name));
                        AnswerField {
                            name: name.clone(),
                            title: field_schema
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            description: field_schema
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            required: required.contains(name),
                            default: field_default.cloned(),
                            draft: build_draft(field_schema, field_default),
                            error: None,
                        }
                    })
                    .collect();
                return AnswerDraft::Object {
                    title: schema
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    description: schema
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    fields,
                    focused_field: None,
                };
            }
            AnswerDraft::Structured {
                value: default
                    .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
                    .unwrap_or_else(|| "{}".to_owned()),
            }
        }
        _ => {
            // Fallback: structured JSON editor.
            AnswerDraft::Structured {
                value: default
                    .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
                    .unwrap_or_default(),
            }
        }
    }
}

/// Collect a draft into its JSON value representation.
fn collect_draft_value(draft: &AnswerDraft) -> Value {
    match draft {
        AnswerDraft::Boolean { value } => Value::Bool(*value),
        AnswerDraft::SingleChoice { options, selected } => {
            options.get(*selected).map_or(Value::Null, |s| Value::String(s.clone()))
        }
        AnswerDraft::MultiChoice { options, selected } => {
            let arr: Vec<Value> = options
                .iter()
                .zip(selected.iter())
                .filter(|(_, sel)| **sel)
                .map(|(opt, _)| Value::String(opt.clone()))
                .collect();
            Value::Array(arr)
        }
        AnswerDraft::Text { value } => Value::String(value.clone()),
        AnswerDraft::Multiline { value } => Value::String(value.clone()),
        AnswerDraft::Number { value, is_integer } => {
            if *is_integer {
                value
                    .parse::<i64>()
                    .map(|n| Value::Number(n.into()))
                    .unwrap_or(Value::String(value.clone()))
            } else {
                value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or(Value::String(value.clone()))
            }
        }
        AnswerDraft::Object { fields, focused_field, .. } => {
            if let Some(idx) = focused_field {
                // When drilling into a nested object, return just that field's value.
                if let Some(field) = fields.get(*idx) {
                    return collect_draft_value(&field.draft);
                }
            }
            let mut map = serde_json::Map::new();
            for field in fields {
                map.insert(field.name.clone(), collect_draft_value(&field.draft));
            }
            Value::Object(map)
        }
        AnswerDraft::BranchChoice {
            branches,
            selected_branch,
        } => {
            if let Some(idx) = selected_branch
                && let Some(branch) = branches.get(*idx)
            {
                // Recurse into the selected branch's schema to build its form value.
                let branch_draft = build_draft(&branch.schema, None);
                collect_draft_value(&branch_draft)
            } else {
                Value::Null
            }
        }
        AnswerDraft::Structured { value } => {
            serde_json::from_str(value).unwrap_or(Value::Null)
        }
    }
}
