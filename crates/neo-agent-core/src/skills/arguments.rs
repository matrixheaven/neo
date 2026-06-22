use std::collections::HashMap;

use super::{LoadedSkill, SkillArgument};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    pub name: String,
    pub raw_arguments: String,
    pub positional: Vec<String>,
    pub named: HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillArgumentError {
    #[error("missing required skill argument `{0}`")]
    MissingRequired(String),
    #[error("failed to parse invocation arguments: {0}")]
    ParseError(String),
}

pub fn parse_skill_invocation(input: &str) -> Result<SkillInvocation, SkillArgumentError> {
    let raw_arguments = input.trim().to_owned();
    let tokens = tokenize_skill_args(&raw_arguments).map_err(SkillArgumentError::ParseError)?;

    let mut positional = Vec::new();
    let mut named = HashMap::new();
    let mut iter = tokens.into_iter().peekable();

    while let Some(token) = iter.next() {
        if let Some(key) = token.strip_prefix("--") {
            let (key, value) = if let Some((key, value)) = key.split_once('=') {
                (key, Some(value.to_owned()))
            } else if let Some(next) = iter.peek() {
                if next.starts_with('-') {
                    (key, None)
                } else {
                    (key, iter.next())
                }
            } else {
                (key, None)
            };
            named.insert(key.to_owned(), value.unwrap_or_default());
        } else if let Some(key) = token.strip_prefix('-') {
            let value = if let Some(next) = iter.peek() {
                if next.starts_with('-') {
                    None
                } else {
                    iter.next()
                }
            } else {
                None
            };
            named.insert(key.to_owned(), value.unwrap_or_default());
        } else {
            positional.push(token);
        }
    }

    Ok(SkillInvocation {
        name: String::new(),
        raw_arguments,
        positional,
        named,
    })
}

fn tokenize_skill_args(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else if ch == '\\' {
                current.push(chars.next().unwrap_or(ch));
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '\\' {
            current.push(chars.next().unwrap_or(ch));
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }

    if quote.is_some() {
        return Err("unclosed quote in skill arguments".into());
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

pub fn expand_skill_body(
    skill: &LoadedSkill,
    invocation: &SkillInvocation,
) -> Result<String, SkillArgumentError> {
    let mut body = skill.body.clone();

    let values = resolve_arguments(&skill.manifest.arguments, invocation)?;

    if !body.contains('$') && !body.contains("${NEO_SKILL_DIR}") {
        body.push_str("\n\nARGUMENTS: ");
        body.push_str(&invocation.raw_arguments);
        return Ok(body);
    }

    body = body.replace("${NEO_SKILL_DIR}", &skill.root.to_string_lossy());
    body = body.replace("$ARGUMENTS", &invocation.raw_arguments);

    for (index, value) in invocation.positional.iter().enumerate() {
        body = body.replace(&format!("$ARGUMENTS[{index}]"), value);
        body = body.replace(&format!("${index}"), value);
    }

    for (name, value) in &values {
        body = body.replace(&format!("${name}"), value);
    }

    Ok(body)
}

fn resolve_arguments(
    declared: &[SkillArgument],
    invocation: &SkillInvocation,
) -> Result<HashMap<String, String>, SkillArgumentError> {
    let mut resolved = HashMap::new();

    for (index, arg) in declared.iter().enumerate() {
        let value = if let Some(value) = invocation.named.get(&arg.name) {
            value.clone()
        } else if let Some(value) = invocation.positional.get(index) {
            value.clone()
        } else if let Some(default) = &arg.default {
            default.clone()
        } else if arg.required {
            return Err(SkillArgumentError::MissingRequired(arg.name.clone()));
        } else {
            String::new()
        };
        resolved.insert(arg.name.clone(), value);
    }

    // Undeclared named arguments are tolerated: they are merged into the
    // resolved map so a skill body that references `$<name>` can still
    // substitute them. When the body has no matching placeholder they simply
    // ride along in `raw_arguments` via the no-placeholder fallback path.
    for (key, value) in &invocation.named {
        resolved.entry(key.clone()).or_insert_with(|| value.clone());
    }

    Ok(resolved)
}
