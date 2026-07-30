use std::fmt::Write as _;

use neo_agent_core::workflow::{
    ResolvedWorkflowDefinition, WorkflowDefinitionRegistry, WorkflowListScope, WorkflowSourceOrigin,
};

pub(crate) const WORKFLOW_CONTEXT_TOO_LARGE: &str = "The workflow catalog is too large for the selected model. Remove unused workflow definitions or choose a model with a larger context window.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkflowSlashRequest {
    Picker,
    Automatic { task: String },
    Named { name: String, task: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkflowSlashError {
    MissingName,
    MissingTask { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkflowCatalogItem {
    pub(super) name: String,
    pub(super) display_name: String,
    pub(super) description: String,
    pub(super) source_label: &'static str,
    pub(super) required_inputs: Vec<String>,
}

pub(super) fn parse_workflow_slash(
    prompt: &str,
) -> Option<Result<WorkflowSlashRequest, WorkflowSlashError>> {
    let prompt = prompt.trim();
    let remainder = prompt.strip_prefix("/workflow")?;
    if remainder.is_empty() {
        return Some(Ok(WorkflowSlashRequest::Picker));
    }

    if let Some(named) = remainder.strip_prefix(':') {
        let (name, task) = named.split_once(char::is_whitespace).unwrap_or((named, ""));
        if name.is_empty() {
            return Some(Err(WorkflowSlashError::MissingName));
        }
        let task = task.trim();
        if task.is_empty() {
            return Some(Err(WorkflowSlashError::MissingTask {
                name: name.to_owned(),
            }));
        }
        return Some(Ok(WorkflowSlashRequest::Named {
            name: name.to_owned(),
            task: task.to_owned(),
        }));
    }

    if remainder.chars().next().is_some_and(char::is_whitespace) {
        let task = remainder.trim();
        return if task.is_empty() {
            Some(Ok(WorkflowSlashRequest::Picker))
        } else {
            Some(Ok(WorkflowSlashRequest::Automatic {
                task: task.to_owned(),
            }))
        };
    }

    None
}

pub(super) fn effective_workflow_catalog(
    registry: &WorkflowDefinitionRegistry,
) -> Result<Vec<WorkflowCatalogItem>, String> {
    let mut catalog = registry
        .list(WorkflowListScope::Effective)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|summary| {
            let source_label = match summary.source_origin {
                WorkflowSourceOrigin::Builtin => "Built-in",
                WorkflowSourceOrigin::User => "All projects",
                WorkflowSourceOrigin::Project => "This project",
                WorkflowSourceOrigin::Dynamic => {
                    return Err("workflow registry returned an unexpected source".to_owned());
                }
            };
            Ok(WorkflowCatalogItem {
                name: summary.name.as_str().to_owned(),
                display_name: summary.display_name,
                description: summary.description,
                source_label,
                required_inputs: summary
                    .schema
                    .input
                    .map_or_else(Vec::new, |schema| schema.required),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    catalog.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(catalog)
}

pub(super) fn suggest_workflow_name(
    registry: &WorkflowDefinitionRegistry,
    name: &str,
) -> Result<Option<String>, String> {
    let catalog = effective_workflow_catalog(registry)?;
    let names = catalog
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    Ok(super::prompt_completion::reliable_slash_suggestion(
        name, &names,
    ))
}

pub(super) fn render_automatic_workflow_context(catalog: &[WorkflowCatalogItem]) -> String {
    let mut context = String::from(concat!(
        "<neo-workflow-request mode=\"automatic\">\n",
        "The user explicitly asked to use an existing Neo workflow.\n",
        "Choose the best matching definition from the complete catalog below.\n",
        "Do not create a workflow, silently continue as ordinary execution, or choose by\n",
        "keyword matching alone. If no definition fits, ask whether to create one. If a\n",
        "definition fits but the task lacks required information, ask for it. After\n",
        "choosing, use Workflow(run_saved). Use Workflow(show) for the chosen definition\n",
        "only when its full input schema is needed before a safe run.\n\n",
        "<workflow-catalog complete=\"true\">\n",
    ));
    for item in catalog {
        let required = if item.required_inputs.is_empty() {
            "None".to_owned()
        } else {
            item.required_inputs.join(", ")
        };
        let _ = writeln!(
            context,
            "<workflow name=\"{}\" source=\"{}\">\n<display-name>{}</display-name>\n<description>{}</description>\n<required-inputs>{}</required-inputs>\n</workflow>",
            escape_xml_text(&item.name),
            escape_xml_text(item.source_label),
            escape_xml_text(&item.display_name),
            escape_xml_text(&item.description),
            escape_xml_text(&required),
        );
    }
    context.push_str("</workflow-catalog>\n</neo-workflow-request>");
    context
}

pub(super) fn render_named_workflow_context(definition: &ResolvedWorkflowDefinition) -> String {
    let source_label = match definition.source_origin {
        WorkflowSourceOrigin::Builtin => "Built-in",
        WorkflowSourceOrigin::User => "All projects",
        WorkflowSourceOrigin::Project => "This project",
        WorkflowSourceOrigin::Dynamic => "Dynamic",
    };
    let input_schema = definition
        .input_schema
        .clone()
        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
    let input_schema = serde_json::to_string(&input_schema).unwrap_or_else(|_| "{}".to_owned());
    format!(
        "<neo-workflow-request mode=\"named\" name=\"{}\">\nThe user explicitly selected this workflow. Use it unless it clearly cannot\nsatisfy the request. Translate the natural-language task into arguments that\nmatch the full input schema. Ask for missing required information instead of\nguessing. If the selected workflow is clearly unsuitable, explain why and ask\npermission before choosing another workflow or continuing without one.\n\n<workflow-definition source=\"{}\">\n<display-name>{}</display-name>\n<description>{}</description>\n<input-schema>{}</input-schema>\n</workflow-definition>\n</neo-workflow-request>",
        escape_xml_text(definition.name.as_str()),
        escape_xml_text(source_label),
        escape_xml_text(&definition.display_name),
        escape_xml_text(&definition.description),
        escape_xml_text(&input_schema),
    )
}

pub(super) fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use neo_agent_core::workflow::{
        BuiltinWorkflowDefinition, WorkflowDefinitionRegistry, WorkflowDefinitionRegistryConfig,
        WorkflowLimits, source_sha256_hex,
    };

    use super::{
        WorkflowCatalogItem, WorkflowSlashError, WorkflowSlashRequest, effective_workflow_catalog,
        parse_workflow_slash, render_automatic_workflow_context, render_named_workflow_context,
    };

    fn paired_definition(
        name: &str,
        display_name: &str,
        description: &str,
        required_input: Option<&str>,
        source: &str,
    ) -> (Vec<u8>, Vec<u8>) {
        let source_hash = source_sha256_hex(source.as_bytes());
        let input_schema = required_input.map_or_else(
            || "".to_owned(),
            |input| format!("\n[input_schema]\ntype = \"object\"\nrequired = [\"{input}\"]\n"),
        );
        let manifest = format!(
            "name = \"{name}\"\ndisplay_name = \"{display_name}\"\ndescription = \"{description}\"\nsource_sha256 = \"{source_hash}\"\n\n[[phases]]\nid = \"run\"\ndescription = \"run\"\n\n[output_schema]\ntype = \"object\"\n{input_schema}"
        );
        (manifest.into_bytes(), source.as_bytes().to_vec())
    }

    fn write_paired_definition(root: &Path, name: &str, manifest: &[u8], source: &[u8]) {
        std::fs::create_dir_all(root).expect("workflow directory");
        std::fs::write(root.join(format!("{name}.workflow.toml")), manifest)
            .expect("workflow manifest");
        std::fs::write(root.join(format!("{name}.lua")), source).expect("workflow source");
    }

    fn fixture_registry(temp: &tempfile::TempDir) -> WorkflowDefinitionRegistry {
        let neo_home = temp.path().join("neo-home");
        let workspace = temp.path().join("workspace");
        let (shadow_manifest, shadow_source) = paired_definition(
            "shadow",
            "Built-in Shadow",
            "lower priority",
            Some("topic"),
            "return { ok = true }\n",
        );
        let registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
            neo_home: neo_home.clone(),
            workspace: workspace.clone(),
            project_trusted: true,
            limits: WorkflowLimits::default(),
            builtins: vec![BuiltinWorkflowDefinition {
                name: "shadow".to_owned(),
                manifest_bytes: shadow_manifest,
                source_bytes: shadow_source,
            }],
        });
        let (user_shadow_manifest, user_shadow_source) = paired_definition(
            "shadow",
            "User Shadow",
            "user winner",
            Some("topic"),
            "return { user = true }\n",
        );
        let (user_manifest, user_source) = paired_definition(
            "user-flow",
            "User Flow",
            "user workflow",
            None,
            "return { user = true }\n",
        );
        write_paired_definition(
            &WorkflowDefinitionRegistry::user_workflows_dir(&neo_home),
            "shadow",
            &user_shadow_manifest,
            &user_shadow_source,
        );
        write_paired_definition(
            &WorkflowDefinitionRegistry::user_workflows_dir(&neo_home),
            "user-flow",
            &user_manifest,
            &user_source,
        );
        let (project_manifest, project_source) = paired_definition(
            "project-flow",
            "Project Flow",
            "project workflow",
            Some("target"),
            "return { project = true }\n",
        );
        write_paired_definition(
            &WorkflowDefinitionRegistry::project_workflows_dir(&workspace),
            "project-flow",
            &project_manifest,
            &project_source,
        );
        registry
    }

    #[test]
    fn workflow_slash_parser_distinguishes_picker_automatic_named_and_prose() {
        assert_eq!(
            parse_workflow_slash("/workflow"),
            Some(Ok(WorkflowSlashRequest::Picker))
        );
        assert_eq!(
            parse_workflow_slash("/workflow   "),
            Some(Ok(WorkflowSlashRequest::Picker))
        );
        assert_eq!(
            parse_workflow_slash("/workflow Research this API"),
            Some(Ok(WorkflowSlashRequest::Automatic {
                task: "Research this API".to_owned()
            }))
        );
        assert_eq!(
            parse_workflow_slash("/workflow:deep-research Research this API"),
            Some(Ok(WorkflowSlashRequest::Named {
                name: "deep-research".to_owned(),
                task: "Research this API".to_owned()
            }))
        );
        assert_eq!(
            parse_workflow_slash("/workflow:"),
            Some(Err(WorkflowSlashError::MissingName))
        );
        assert_eq!(
            parse_workflow_slash("/workflow:deep-research"),
            Some(Err(WorkflowSlashError::MissingTask {
                name: "deep-research".to_owned()
            }))
        );
        assert_eq!(parse_workflow_slash("/workflowish research"), None);
        assert_eq!(parse_workflow_slash("Please use /workflow for this"), None);
    }

    #[test]
    fn workflow_name_suggestion_requires_a_unique_existing_rank() {
        let values = vec!["deep-research".to_owned(), "deep-review".to_owned()];
        assert_eq!(
            super::super::prompt_completion::reliable_slash_suggestion("deep-reseach", &values,),
            Some("deep-research".to_owned())
        );
        assert_eq!(
            super::super::prompt_completion::reliable_slash_suggestion("deep", &values),
            None
        );
    }

    #[test]
    fn workflow_catalog_is_effective_stable_and_public() {
        let temp = tempfile::tempdir().expect("tempdir");
        let registry = fixture_registry(&temp);
        let catalog = effective_workflow_catalog(&registry).expect("effective catalog");

        assert_eq!(
            catalog
                .iter()
                .map(|item| (
                    item.name.as_str(),
                    item.display_name.as_str(),
                    item.source_label
                ))
                .collect::<Vec<_>>(),
            vec![
                ("project-flow", "Project Flow", "This project"),
                ("user-flow", "User Flow", "All projects"),
                ("shadow", "User Shadow", "All projects"),
            ]
        );
        assert_eq!(catalog[0].required_inputs, vec!["target"]);
        assert_eq!(catalog[2].required_inputs, vec!["topic"]);
        assert!(catalog.iter().all(|item| {
            !item.description.contains("source_sha256") && !item.description.contains("revision")
        }));
    }

    #[test]
    fn workflow_context_is_complete_escaped_and_mode_specific() {
        let catalog = vec![WorkflowCatalogItem {
            name: "safe-name".to_owned(),
            display_name: "Research & <Review>".to_owned(),
            description: "Use \"primary\" <sources> & evidence".to_owned(),
            source_label: "This project",
            required_inputs: vec!["topic&scope".to_owned()],
        }];
        let automatic = render_automatic_workflow_context(&catalog);
        assert_eq!(automatic.matches("<workflow name=\"safe-name\"").count(), 1);
        assert!(automatic.contains("Research &amp; &lt;Review&gt;"));
        assert!(automatic.contains("Use &quot;primary&quot; &lt;sources&gt; &amp; evidence"));
        assert!(automatic.contains("topic&amp;scope"));
        assert!(automatic.contains("complete=\"true\""));

        let temp = tempfile::tempdir().expect("tempdir");
        let (manifest, source) = paired_definition(
            "named",
            "Named Workflow",
            "Named description",
            Some("topic"),
            "return { ok = true }\n",
        );
        let registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
            neo_home: temp.path().join("neo-home"),
            workspace: temp.path().join("workspace"),
            project_trusted: true,
            limits: WorkflowLimits::default(),
            builtins: vec![BuiltinWorkflowDefinition {
                name: "named".to_owned(),
                manifest_bytes: manifest,
                source_bytes: source,
            }],
        });
        let definition = registry.resolve("named").expect("named definition");
        let named = render_named_workflow_context(&definition);
        assert!(named.contains("<input-schema>"));
        assert!(named.contains("&quot;type&quot;:&quot;object&quot;"));
        assert!(named.contains("&quot;required&quot;:[&quot;topic&quot;]"));
        assert!(!named.contains("return { ok = true }"));
        assert!(!named.contains("source_locator"));
        assert!(!named.contains("source_sha256"));
        assert!(!named.contains("revision"));
    }
}
