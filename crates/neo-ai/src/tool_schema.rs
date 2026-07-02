use schemars::JsonSchema;
use serde_json::{Map, Value};

#[must_use]
pub fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema generation should serialize")
}

#[must_use]
pub(crate) fn root_schema_for<T: JsonSchema>() -> serde_json::Value {
    let schema = schema_for::<T>();
    schema.get("schema").cloned().unwrap_or(schema)
}

#[must_use]
pub fn normalize_tool_schema(schema: &Value) -> Value {
    let mut normalized = resolve_schema_refs(schema, schema, &mut Vec::new());
    normalize_schema_node(&mut normalized);
    normalized
}

fn resolve_schema_refs(node: &Value, root: &Value, seen: &mut Vec<String>) -> Value {
    match node {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_schema_refs(item, root, seen))
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(resolved) = resolve_local_ref(root, reference)
            {
                if seen.iter().any(|item| item == reference) {
                    return node.clone();
                }
                seen.push(reference.to_owned());
                let mut merged = resolve_schema_refs(resolved, root, seen);
                seen.pop();
                if let Value::Object(merged_object) = &mut merged {
                    for (key, value) in object {
                        if key != "$ref" {
                            merged_object
                                .insert(key.clone(), resolve_schema_refs(value, root, seen));
                        }
                    }
                    return merged;
                }
            }

            Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), resolve_schema_refs(value, root, seen)))
                    .collect(),
            )
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => node.clone(),
    }
}

fn normalize_schema_node(node: &mut Value) {
    let Value::Object(object) = node else {
        return;
    };

    remove_schema_metadata(object);
    simplify_nullable_type(object);
    simplify_const_one_of(object);

    for key in ["properties", "$defs", "definitions", "patternProperties"] {
        if let Some(Value::Object(children)) = object.get_mut(key) {
            for child in children.values_mut() {
                normalize_schema_node(child);
            }
        }
    }

    for key in [
        "items",
        "additionalItems",
        "additionalProperties",
        "contains",
        "contentSchema",
        "else",
        "if",
        "not",
        "propertyNames",
        "then",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(child) = object.get_mut(key) {
            normalize_schema_or_schema_array(child);
        }
    }

    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = object.get_mut(key) {
            for child in children {
                normalize_schema_node(child);
            }
        }
    }
}

fn normalize_schema_or_schema_array(node: &mut Value) {
    match node {
        Value::Array(items) => {
            for item in items {
                normalize_schema_node(item);
            }
        }
        Value::Object(_) => normalize_schema_node(node),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn remove_schema_metadata(object: &mut Map<String, Value>) {
    for key in [
        "$schema",
        "$defs",
        "definitions",
        "default",
        "examples",
        "format",
        "title",
    ] {
        object.remove(key);
    }
}

fn simplify_nullable_type(object: &mut Map<String, Value>) {
    let Some(Value::Array(types)) = object.get("type") else {
        return;
    };
    let non_null = types
        .iter()
        .filter(|value| value.as_str() != Some("null"))
        .cloned()
        .collect::<Vec<_>>();
    if non_null.len() == 1 {
        object.insert("type".to_owned(), non_null[0].clone());
    }
}

fn simplify_const_one_of(object: &mut Map<String, Value>) {
    let Some(Value::Array(variants)) = object.get("oneOf") else {
        return;
    };
    let mut values = Vec::new();
    for variant in variants {
        let Some(value) = variant.get("const") else {
            return;
        };
        values.push(value.clone());
    }
    if values.is_empty() {
        return;
    }
    if let Some(schema_type) = infer_enum_type(&values) {
        object.insert("type".to_owned(), Value::String(schema_type.to_owned()));
    }
    object.insert("enum".to_owned(), Value::Array(values));
    object.remove("oneOf");
}

fn infer_enum_type(values: &[Value]) -> Option<&'static str> {
    let mut inferred = values.iter().filter_map(value_type).collect::<Vec<_>>();
    inferred.sort_unstable();
    inferred.dedup();
    if inferred.len() == 1 {
        inferred.first().copied()
    } else {
        None
    }
}

fn value_type(value: &Value) -> Option<&'static str> {
    match value {
        Value::String(_) => Some("string"),
        Value::Bool(_) => Some("boolean"),
        Value::Number(number) if number.is_i64() || number.is_u64() => Some("integer"),
        Value::Number(_) => Some("number"),
        Value::Array(_) => Some("array"),
        Value::Object(_) => Some("object"),
        Value::Null => None,
    }
}

fn resolve_local_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    root.pointer(pointer)
}
