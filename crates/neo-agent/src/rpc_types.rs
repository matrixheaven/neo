use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_user_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionsListResult {
    pub sessions: Vec<RpcSessionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionGetResult {
    #[serde(flatten)]
    pub record: RpcSessionRecord,
    pub path: String,
    pub messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionExportHtmlResult {
    pub session_id: String,
    pub html: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcCommandRecord {
    pub name: String,
    pub kind: RpcCommandKind,
    pub template: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub location: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RpcCommandKind {
    PromptTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcCommandsResult {
    pub commands: Vec<RpcCommandRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_rpc_records_have_stable_json_shape() {
        let record = RpcSessionRecord {
            id: "alpha".to_owned(),
            title: Some("Generated title".to_owned()),
            title_model: Some("openai/gpt-4.1".to_owned()),
            title_updated_at: Some("126.0Z".to_owned()),
            workspace: Some("/workspace/neo".to_owned()),
            last_user_prompt: Some("Fix session picker".to_owned()),
            updated_at: Some("127.0Z".to_owned()),
            name: Some("Main thread".to_owned()),
            summary: Some("Local branch summary".to_owned()),
            summary_source: Some("local_extractive".to_owned()),
            summary_model: None,
            summary_updated_at: Some("125.0Z".to_owned()),
            parent_id: None,
            children: vec!["alpha-fork-1".to_owned()],
        };

        let value = serde_json::to_value(&record).expect("serialize session record");

        assert_eq!(value["id"], "alpha");
        assert_eq!(value["title"], "Generated title");
        assert_eq!(value["title_model"], "openai/gpt-4.1");
        assert_eq!(value["title_updated_at"], "126.0Z");
        assert_eq!(value["workspace"], "/workspace/neo");
        assert_eq!(value["last_user_prompt"], "Fix session picker");
        assert_eq!(value["updated_at"], "127.0Z");
        assert_eq!(value["name"], "Main thread");
        assert_eq!(value["summary"], "Local branch summary");
        assert_eq!(value["summary_source"], "local_extractive");
        assert_eq!(value["summary_updated_at"], "125.0Z");
        assert!(value["parent_id"].is_null());
        assert_eq!(value["children"], json!(["alpha-fork-1"]));
        assert!(value.get("cloud_id").is_none());
        assert!(value.get("synced_at").is_none());
        assert!(value.get("remote_parent_id").is_none());
        assert!(value.get("share_ids").is_none());
        assert!(value.get("shares").is_none());
        assert_eq!(
            serde_json::from_value::<RpcSessionRecord>(value).expect("deserialize session record"),
            record
        );
    }

    #[test]
    fn session_get_result_has_stable_json_shape() {
        let result = RpcSessionGetResult {
            record: RpcSessionRecord {
                id: "alpha".to_owned(),
                title: Some("Main thread".to_owned()),
                title_model: None,
                title_updated_at: None,
                workspace: None,
                last_user_prompt: None,
                updated_at: None,
                name: Some("Main thread".to_owned()),
                summary: Some("Local branch summary".to_owned()),
                summary_source: Some("local_extractive".to_owned()),
                summary_model: None,
                summary_updated_at: None,
                parent_id: None,
                children: vec!["alpha-fork-1".to_owned()],
            },
            path: "/tmp/neo/.neo/sessions/alpha.jsonl".to_owned(),
            messages: vec![json!({
                "User": {
                    "content": [
                        {
                            "Text": {
                                "text": "hello"
                            }
                        }
                    ]
                }
            })],
        };

        let value = serde_json::to_value(&result).expect("serialize session get result");

        assert_eq!(value["id"], "alpha");
        assert_eq!(value["name"], "Main thread");
        assert_eq!(value["summary"], "Local branch summary");
        assert!(value["parent_id"].is_null());
        assert_eq!(value["children"], json!(["alpha-fork-1"]));
        assert_eq!(value["path"], "/tmp/neo/.neo/sessions/alpha.jsonl");
        assert_eq!(
            value["messages"][0]["User"]["content"][0]["Text"]["text"],
            "hello"
        );
        assert_eq!(
            serde_json::from_value::<RpcSessionGetResult>(value)
                .expect("deserialize session get result"),
            result
        );
    }

    #[test]
    fn session_export_html_result_has_stable_json_shape() {
        let result = RpcSessionExportHtmlResult {
            session_id: "alpha".to_owned(),
            html: "<!doctype html><title>neo session alpha</title>".to_owned(),
        };

        let value = serde_json::to_value(&result).expect("serialize session export html result");

        assert_eq!(value["session_id"], "alpha");
        assert_eq!(
            value["html"],
            "<!doctype html><title>neo session alpha</title>"
        );
        assert_eq!(
            serde_json::from_value::<RpcSessionExportHtmlResult>(value)
                .expect("deserialize session export html result"),
            result
        );
    }

    #[test]
    fn commands_result_has_stable_prompt_template_json_shape() {
        let result = RpcCommandsResult {
            commands: vec![RpcCommandRecord {
                name: "/review".to_owned(),
                kind: RpcCommandKind::PromptTemplate,
                template: "review".to_owned(),
                description: "Review a target".to_owned(),
                argument_hint: Some("<path>".to_owned()),
                location: "project".to_owned(),
                path: "/tmp/neo/.neo/prompts/review.md".to_owned(),
            }],
        };

        let value = serde_json::to_value(&result).expect("serialize commands result");

        assert_eq!(value["commands"][0]["name"], "/review");
        assert_eq!(value["commands"][0]["kind"], "prompt_template");
        assert_eq!(value["commands"][0]["template"], "review");
        assert_eq!(value["commands"][0]["description"], "Review a target");
        assert_eq!(value["commands"][0]["argument_hint"], "<path>");
        assert_eq!(value["commands"][0]["location"], "project");
        assert_eq!(
            serde_json::from_value::<RpcCommandsResult>(value)
                .expect("deserialize commands result"),
            result
        );
    }
}
