use schemars::JsonSchema;

#[must_use]
pub fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema generation should serialize")
}

#[must_use]
pub(crate) fn root_schema_for<T: JsonSchema>() -> serde_json::Value {
    let schema = schema_for::<T>();
    schema.get("schema").cloned().unwrap_or(schema)
}
