use crate::{
    cli::TrustCommand,
    config::AppConfig,
    trust::{self, ProjectTrustDecision, ProjectTrustStore, TrustInputKind},
};

pub(crate) fn execute(config: &AppConfig, command: &TrustCommand) -> anyhow::Result<String> {
    let store = ProjectTrustStore::from_home()?;
    match command {
        TrustCommand::Status => status(config, &store),
        TrustCommand::Approve => {
            store.set(&config.project_dir, Some(true))?;
            Ok(format!(
                "approved trust for {}\n",
                config.project_dir.display()
            ))
        }
        TrustCommand::Deny => {
            store.set(&config.project_dir, Some(false))?;
            Ok(format!(
                "denied trust for {}\n",
                config.project_dir.display()
            ))
        }
        TrustCommand::Clear => {
            store.set(&config.project_dir, None)?;
            Ok(format!(
                "cleared trust decision for {}\n",
                config.project_dir.display()
            ))
        }
    }
}

fn status(config: &AppConfig, store: &ProjectTrustStore) -> anyhow::Result<String> {
    let inputs = trust::collect_project_trust_inputs(&config.project_dir)?;
    let decision = trust::resolve_project_trust_decision(&config.project_dir, false, store)?;

    let mut lines = Vec::new();
    lines.push(format!("Directory: {}", config.project_dir.display()));

    let (target, decision_label) = match &decision {
        ProjectTrustDecision::Trusted { source } => {
            let target = source.target(&config.project_dir);
            (target.display().to_string(), "trusted".to_owned())
        }
        ProjectTrustDecision::Untrusted { source } => {
            let target = source.target(&config.project_dir);
            (target.display().to_string(), "untrusted".to_owned())
        }
        ProjectTrustDecision::Unknown { .. } => (
            config.project_dir.display().to_string(),
            "unknown".to_owned(),
        ),
    };
    lines.push(format!("Trust target: {target}"));

    if inputs.detected.is_empty() && inputs.parent_candidates.is_empty() {
        lines.push("Detected inputs: none".to_owned());
    } else {
        let mut items = Vec::new();
        for (path, kind) in &inputs.detected {
            items.push(format!("{} ({})", path.display(), kind_label(*kind)));
        }
        for path in &inputs.parent_candidates {
            items.push(format!("{} (ancestor)", path.display()));
        }
        lines.push(format!("Detected inputs:\n  {}", items.join("\n  ")));
    }

    lines.push(format!("Effective decision: {decision_label}"));

    Ok(lines.join("\n") + "\n")
}

fn kind_label(kind: TrustInputKind) -> &'static str {
    match kind {
        TrustInputKind::ContextFile => "context file",
        TrustInputKind::NeoDir => "neo directory",
    }
}
