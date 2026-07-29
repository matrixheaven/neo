use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowAnswerControl {
    Boolean,
    Choice(Vec<String>),
    MultipleChoice(Vec<String>),
    Text,
    Number,
    ObjectArray,
    BranchChoice(Vec<String>),
    Structured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowAnswerBranchScope {
    pub parent_path: String,
    pub branch_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowAnswerArrayScope {
    pub parent_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowAnswerField {
    pub path: String,
    pub label: String,
    pub breadcrumb: Vec<String>,
    pub description: Option<String>,
    pub required: bool,
    pub control: WorkflowAnswerControl,
    pub schema: Value,
    pub branches: Vec<Value>,
    pub(crate) branch_scope: Option<WorkflowAnswerBranchScope>,
    pub(crate) array_scope: Option<WorkflowAnswerArrayScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowAnswerForm {
    pub title: Option<String>,
    pub prompt: String,
    pub fields: Vec<WorkflowAnswerField>,
    pub structured_fallback: bool,
}

impl WorkflowAnswerForm {
    #[must_use]
    pub fn from_schema(schema: &Value, title: Option<String>, prompt: String) -> Self {
        let mut fields = Vec::new();
        let structured_fallback =
            !append_fields(schema, "", "", &[], false, None, None, &mut fields);
        if structured_fallback {
            fields = vec![WorkflowAnswerField {
                path: String::new(),
                label: "Answer".to_owned(),
                breadcrumb: vec!["Answer".to_owned()],
                description: Some("Enter the requested values.".to_owned()),
                required: true,
                control: WorkflowAnswerControl::Structured,
                schema: schema.clone(),
                branches: Vec::new(),
                branch_scope: None,
                array_scope: None,
            }];
        }
        Self {
            title,
            prompt,
            fields,
            structured_fallback,
        }
    }

    #[must_use]
    pub fn lines(
        &self,
        value: &Value,
        errors: &[String],
        selected: usize,
        choice_indices: &BTreeMap<String, usize>,
        branch_indices: &BTreeMap<String, usize>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(title) = &self.title {
            lines.push(title.clone());
        }
        lines.push(self.prompt.clone());
        for (index, field) in self
            .visible_fields(value, choice_indices, branch_indices)
            .into_iter()
            .enumerate()
        {
            let field_path = field.resolved_path(choice_indices);
            let choice_index = if matches!(field.control, WorkflowAnswerControl::ObjectArray)
                || index == selected
            {
                choice_indices.get(&field_path).copied()
            } else {
                None
            };
            let pointer = if field_path.is_empty() {
                "/"
            } else {
                &field_path
            };
            let marker = if index == selected { ">" } else { " " };
            let required = if field.required { " required" } else { "" };
            let label = field.display_label();
            lines.push(format!(
                "{marker} {}{}: {}",
                label,
                required,
                format_value(
                    field,
                    value_at_path(value, pointer),
                    choice_index,
                    branch_indices.get(&field_path).copied(),
                )
            ));
            if let Some(description) = &field.description {
                lines.push(format!("  {description}"));
            }
            for error in errors.iter().filter(|error| error.starts_with(pointer)) {
                lines.push(format!("  {error}"));
            }
        }
        lines
    }

    pub(crate) fn visible_fields<'a>(
        &'a self,
        value: &Value,
        choice_indices: &BTreeMap<String, usize>,
        branch_indices: &BTreeMap<String, usize>,
    ) -> Vec<&'a WorkflowAnswerField> {
        self.fields
            .iter()
            .filter(|field| field.is_visible(value, choice_indices, branch_indices))
            .collect()
    }
}

impl WorkflowAnswerField {
    fn display_label(&self) -> String {
        if self.breadcrumb.is_empty() {
            if self.label.is_empty() {
                "Answer".to_owned()
            } else {
                self.label.clone()
            }
        } else {
            self.breadcrumb.join(" > ")
        }
    }

    pub(crate) fn resolved_path(&self, choice_indices: &BTreeMap<String, usize>) -> String {
        let Some(scope) = &self.array_scope else {
            return self.path.clone();
        };
        let row = choice_indices.get(&scope.parent_path).copied().unwrap_or(0);
        let suffix = self.path.strip_prefix(&scope.parent_path).unwrap_or("");
        format!("{}/{row}{suffix}", scope.parent_path)
    }

    fn is_visible(
        &self,
        value: &Value,
        choice_indices: &BTreeMap<String, usize>,
        branch_indices: &BTreeMap<String, usize>,
    ) -> bool {
        if let Some(scope) = &self.branch_scope
            && branch_indices.get(&scope.parent_path).copied().unwrap_or(0) != scope.branch_index
        {
            return false;
        }
        let Some(scope) = &self.array_scope else {
            return true;
        };
        let Some(rows) = value_at_path(value, &scope.parent_path).and_then(Value::as_array) else {
            return false;
        };
        !rows.is_empty()
            && choice_indices.get(&scope.parent_path).copied().unwrap_or(0) < rows.len()
    }
}

fn append_fields(
    schema: &Value,
    path: &str,
    label: &str,
    parent_breadcrumb: &[String],
    required: bool,
    branch_scope: Option<WorkflowAnswerBranchScope>,
    array_scope: Option<WorkflowAnswerArrayScope>,
    fields: &mut Vec<WorkflowAnswerField>,
) -> bool {
    if schema.get("pattern").is_some()
        || schema.get("allOf").is_some()
        || schema.get("not").is_some()
    {
        return false;
    }
    let title = schema
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(label)
        .to_owned();
    let breadcrumb = with_breadcrumb(parent_breadcrumb, &title);
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        let options = options
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if options.is_empty() {
            return false;
        }
        fields.push(WorkflowAnswerField {
            path: path.to_owned(),
            label: title,
            breadcrumb,
            description,
            required,
            control: WorkflowAnswerControl::Choice(options),
            schema: schema.clone(),
            branches: Vec::new(),
            branch_scope,
            array_scope,
        });
        return true;
    }
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        let titles = branches
            .iter()
            .map(|branch| {
                branch
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Option<Vec<_>>>();
        let Some(titles) = titles else {
            return false;
        };
        fields.push(WorkflowAnswerField {
            path: path.to_owned(),
            label: title,
            breadcrumb: breadcrumb.clone(),
            description,
            required,
            control: WorkflowAnswerControl::BranchChoice(titles),
            schema: schema.clone(),
            branches: branches.clone(),
            branch_scope: branch_scope.clone(),
            array_scope: array_scope.clone(),
        });
        for (branch_index, branch) in branches.iter().enumerate() {
            let branch_breadcrumb = branch.get("title").and_then(Value::as_str).map_or_else(
                || breadcrumb.clone(),
                |title| with_breadcrumb(&breadcrumb, title),
            );
            if branch.get("type").and_then(Value::as_str) == Some("object")
                && !append_object_fields(
                    branch,
                    path,
                    &branch_breadcrumb,
                    Some(WorkflowAnswerBranchScope {
                        parent_path: path.to_owned(),
                        branch_index,
                    }),
                    array_scope.clone(),
                    fields,
                )
            {
                return false;
            }
        }
        return true;
    }
    let control = match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => WorkflowAnswerControl::Boolean,
        Some("string") => WorkflowAnswerControl::Text,
        Some("number") | Some("integer") => WorkflowAnswerControl::Number,
        Some("array") => {
            let Some(items) = schema.get("items") else {
                return false;
            };
            if let Some(options) = items.get("enum").and_then(Value::as_array) {
                let options = options
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if options.is_empty() {
                    return false;
                }
                WorkflowAnswerControl::MultipleChoice(options)
            } else if items.get("type").and_then(Value::as_str) == Some("object") {
                fields.push(WorkflowAnswerField {
                    path: path.to_owned(),
                    label: title,
                    breadcrumb: breadcrumb.clone(),
                    description,
                    required,
                    control: WorkflowAnswerControl::ObjectArray,
                    schema: schema.clone(),
                    branches: Vec::new(),
                    branch_scope: branch_scope.clone(),
                    array_scope: array_scope.clone(),
                });
                return append_object_fields(
                    items,
                    path,
                    &items.get("title").and_then(Value::as_str).map_or_else(
                        || breadcrumb.clone(),
                        |title| with_breadcrumb(&breadcrumb, title),
                    ),
                    branch_scope,
                    Some(WorkflowAnswerArrayScope {
                        parent_path: path.to_owned(),
                    }),
                    fields,
                );
            } else {
                return false;
            }
        }
        Some("object") => {
            return append_object_fields(
                schema,
                path,
                &breadcrumb,
                branch_scope,
                array_scope,
                fields,
            );
        }
        _ => return false,
    };
    fields.push(WorkflowAnswerField {
        path: path.to_owned(),
        label: title,
        breadcrumb,
        description,
        required,
        control,
        schema: schema.clone(),
        branches: Vec::new(),
        branch_scope,
        array_scope,
    });
    true
}

fn append_object_fields(
    schema: &Value,
    path: &str,
    breadcrumb: &[String],
    branch_scope: Option<WorkflowAnswerBranchScope>,
    array_scope: Option<WorkflowAnswerArrayScope>,
    fields: &mut Vec<WorkflowAnswerField>,
) -> bool {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let required_names = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    properties.iter().all(|(name, child)| {
        let encoded_name = encode_json_pointer_segment(name);
        append_fields(
            child,
            &format!("{path}/{encoded_name}"),
            name,
            breadcrumb,
            required_names.contains(&name.as_str()),
            branch_scope.clone(),
            array_scope.clone(),
            fields,
        )
    })
}

fn with_breadcrumb(parent: &[String], title: &str) -> Vec<String> {
    let mut breadcrumb = parent.to_vec();
    if !title.is_empty() {
        breadcrumb.push(title.to_owned());
    }
    breadcrumb
}

fn encode_json_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn value_at_path<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer == "/" {
        return Some(value);
    }
    value.pointer(pointer)
}

fn format_value(
    field: &WorkflowAnswerField,
    value: Option<&Value>,
    choice_index: Option<usize>,
    branch_index: Option<usize>,
) -> String {
    let value = value.unwrap_or(&Value::Null);
    match &field.control {
        WorkflowAnswerControl::Boolean => if value.as_bool().unwrap_or(false) {
            "Yes"
        } else {
            "No"
        }
        .to_owned(),
        WorkflowAnswerControl::Choice(options) => {
            format!(
                "{} ({})",
                value.as_str().unwrap_or("Choose"),
                options.join(" | ")
            )
        }
        WorkflowAnswerControl::BranchChoice(options) => {
            let selected = branch_index
                .unwrap_or(0)
                .min(options.len().saturating_sub(1));
            format!("{}: {}", options[selected], display_value(value))
        }
        WorkflowAnswerControl::MultipleChoice(options) => {
            let chosen = value.as_array().cloned().unwrap_or_default();
            options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let cursor = if choice_index == Some(index) {
                        ">"
                    } else {
                        " "
                    };
                    if chosen.iter().any(|item| item.as_str() == Some(option)) {
                        format!("{cursor}[x] {option}")
                    } else {
                        format!("{cursor}[ ] {option}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        WorkflowAnswerControl::ObjectArray => value
            .as_array()
            .map(|rows| {
                if rows.is_empty() {
                    "0 entries".to_owned()
                } else {
                    let selected = choice_index.unwrap_or(0).min(rows.len() - 1);
                    format!("{} entries, row {} selected", rows.len(), selected + 1)
                }
            })
            .unwrap_or_else(|| "0 entries".to_owned()),
        WorkflowAnswerControl::Structured => "Advanced answer".to_owned(),
        WorkflowAnswerControl::Text | WorkflowAnswerControl::Number => {
            value.to_string().trim_matches('"').to_owned()
        }
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Bool(true) => "Yes".to_owned(),
        Value::Bool(false) => "No".to_owned(),
        Value::String(value) if value.is_empty() => "Empty".to_owned(),
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}
