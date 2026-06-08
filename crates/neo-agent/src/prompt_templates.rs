use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub content: String,
    pub path: PathBuf,
}

pub(crate) fn expand_prompt_template_args(
    prompt: Vec<String>,
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
    explicit_selectors: &[String],
    disabled: bool,
) -> anyhow::Result<Vec<String>> {
    let explicit_templates =
        load_explicit_prompt_templates(explicit_selectors, project_dir, global_prompts_dir)?;

    let Some(invocation) = PromptInvocation::from_prompt_args(&prompt) else {
        if let [template] = explicit_templates.as_slice() {
            return Ok(vec![substitute_args(&template.content, &prompt)]);
        }
        return Ok(prompt);
    };
    if let Some(template) = explicit_templates
        .iter()
        .find(|template| template.name == invocation.name)
    {
        return Ok(vec![substitute_args(&template.content, &invocation.args)]);
    }
    if disabled {
        return Ok(prompt);
    }
    let Some(template) =
        find_prompt_template_by_name(&invocation.name, project_dir, global_prompts_dir)?
    else {
        return Ok(prompt);
    };
    Ok(vec![substitute_args(&template.content, &invocation.args)])
}

pub(crate) fn load_project_prompt_templates(
    project_dir: &Path,
) -> anyhow::Result<Vec<PromptTemplate>> {
    let prompts_dir = project_dir.join(".neo/prompts");
    load_prompt_templates_from_dir(&prompts_dir)
}

fn load_user_prompt_templates(prompts_dir: &Path) -> anyhow::Result<Vec<PromptTemplate>> {
    load_prompt_templates_from_dir(prompts_dir)
}

fn load_prompt_templates_from_dir(prompts_dir: &Path) -> anyhow::Result<Vec<PromptTemplate>> {
    let Ok(entries) = fs::read_dir(prompts_dir) else {
        return Ok(Vec::new());
    };
    let mut templates = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("md")) {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        templates.push(load_template_from_file(&path)?);
    }
    templates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(templates)
}

fn find_prompt_template_by_name(
    name: &str,
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
) -> anyhow::Result<Option<PromptTemplate>> {
    if let Some(template) = load_project_prompt_templates(project_dir)?
        .into_iter()
        .find(|template| template.name == name)
    {
        return Ok(Some(template));
    }
    let Some(global_prompts_dir) = global_prompts_dir else {
        return Ok(None);
    };
    Ok(load_user_prompt_templates(global_prompts_dir)?
        .into_iter()
        .find(|template| template.name == name))
}

fn load_explicit_prompt_templates(
    selectors: &[String],
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
) -> anyhow::Result<Vec<PromptTemplate>> {
    let mut templates = Vec::new();
    for selector in selectors {
        let selected = load_selected_prompt_templates(selector, project_dir, global_prompts_dir)?;
        for template in selected {
            if !templates
                .iter()
                .any(|candidate: &PromptTemplate| candidate.name == template.name)
            {
                templates.push(template);
            }
        }
    }
    Ok(templates)
}

fn load_selected_prompt_templates(
    selector: &str,
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
) -> anyhow::Result<Vec<PromptTemplate>> {
    if selector.is_empty() {
        anyhow::bail!("prompt template selector cannot be empty");
    }
    if let Some(path) = selector_as_template_path(selector) {
        return load_templates_from_checked_path(path, project_dir, global_prompts_dir);
    }
    if let Some(template) = find_prompt_template_by_name(selector, project_dir, global_prompts_dir)?
    {
        return Ok(vec![template]);
    }
    let path = Path::new(selector);
    if explicit_path_exists(path, project_dir, global_prompts_dir) {
        return load_templates_from_checked_path(path, project_dir, global_prompts_dir);
    }
    Err(anyhow::anyhow!("unknown prompt template: {selector}"))
}

fn selector_as_template_path(selector: &str) -> Option<&Path> {
    let path = Path::new(selector);
    (path.is_absolute()
        || path.components().count() > 1
        || selector.contains(std::path::MAIN_SEPARATOR)
        || selector.contains('/')
        || path.extension() == Some(OsStr::new("md")))
    .then_some(path)
}

fn explicit_path_exists(
    path: &Path,
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
) -> bool {
    if path.is_absolute() {
        return path.exists();
    }
    project_dir.join(path).exists()
        || global_prompts_dir.is_some_and(|prompts_dir| prompts_dir.join(path).exists())
}

fn load_templates_from_checked_path(
    path: &Path,
    project_dir: &Path,
    global_prompts_dir: Option<&Path>,
) -> anyhow::Result<Vec<PromptTemplate>> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_dir.join(path)
    };
    let candidate = candidate.canonicalize().map_err(|err| {
        anyhow::anyhow!(
            "failed to resolve prompt template {}: {err}",
            candidate.display()
        )
    })?;
    let project_dir = project_dir.canonicalize().map_err(|err| {
        anyhow::anyhow!(
            "failed to resolve project directory {}: {err}",
            project_dir.display()
        )
    })?;
    let global_prompts_dir = global_prompts_dir.and_then(|path| path.canonicalize().ok());
    anyhow::ensure!(
        candidate.starts_with(&project_dir)
            || global_prompts_dir
                .as_ref()
                .is_some_and(|prompts_dir| candidate.starts_with(prompts_dir)),
        "prompt template path must stay inside project directory or user prompt directory"
    );
    if candidate.is_dir() {
        return load_prompt_templates_from_dir(&candidate);
    }
    anyhow::ensure!(
        candidate.extension() == Some(OsStr::new("md")),
        "prompt template path must point to a .md file: {}",
        candidate.display()
    );
    anyhow::ensure!(
        candidate.is_file(),
        "prompt template path must be a regular file: {}",
        candidate.display()
    );
    load_template_from_file(&candidate).map(|template| vec![template])
}

fn load_template_from_file(path: &Path) -> anyhow::Result<PromptTemplate> {
    let source = fs::read_to_string(path).map_err(|err| {
        anyhow::anyhow!("failed to read prompt template {}: {err}", path.display())
    })?;
    let (frontmatter, body) = split_frontmatter(&source);
    let metadata = frontmatter.map(parse_frontmatter).unwrap_or_default();
    let content = body
        .trim_start_matches(['\r', '\n'])
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let description = metadata
        .get("description")
        .cloned()
        .or_else(|| first_non_empty_line(&content))
        .unwrap_or_default();
    let name = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_owned();
    Ok(PromptTemplate {
        name,
        description,
        argument_hint: metadata.get("argument-hint").cloned(),
        content,
        path: path.to_path_buf(),
    })
}

fn split_frontmatter(source: &str) -> (Option<&str>, &str) {
    let Some(rest) = source
        .strip_prefix("---\r\n")
        .or_else(|| source.strip_prefix("---\n"))
    else {
        return (None, source);
    };
    let Some(separator_start) = rest.find("\n---") else {
        return (None, source);
    };
    let frontmatter = rest[..separator_start]
        .strip_suffix('\r')
        .unwrap_or(&rest[..separator_start]);
    let Some(after_separator) = rest[separator_start + 1..].strip_prefix("---") else {
        return (None, source);
    };
    let body = after_separator
        .strip_prefix("\r\n")
        .or_else(|| after_separator.strip_prefix('\n'))
        .unwrap_or(after_separator);
    (Some(frontmatter), body)
}

fn parse_frontmatter(frontmatter: &str) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':').or_else(|| line.split_once('=')) else {
            continue;
        };
        metadata.insert(key.trim().to_owned(), unquote(value.trim()).to_owned());
    }
    metadata
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn first_non_empty_line(content: &str) -> Option<String> {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            let mut chars = line.chars();
            let summary = chars.by_ref().take(60).collect::<String>();
            if chars.next().is_some() {
                format!("{summary}...")
            } else {
                summary
            }
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptInvocation {
    name: String,
    args: Vec<String>,
}

impl PromptInvocation {
    fn from_prompt_args(prompt: &[String]) -> Option<Self> {
        let first = prompt.first()?;
        if !first.starts_with('/') || first == "/" {
            return None;
        }
        if first.split_whitespace().count() > 1 {
            let tokens = parse_command_args(&prompt.join(" "));
            return Self::from_tokens(tokens);
        }
        let mut tokens = Vec::with_capacity(prompt.len());
        tokens.push(first.clone());
        tokens.extend(prompt.iter().skip(1).cloned());
        Self::from_tokens(tokens)
    }

    fn from_tokens(tokens: Vec<String>) -> Option<Self> {
        let command = tokens.first()?;
        let name = command.strip_prefix('/')?;
        if name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            args: tokens.into_iter().skip(1).collect(),
        })
    }
}

pub(crate) fn parse_command_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in input.chars() {
        if let Some(quote_character) = quote {
            if character == quote_character {
                quote = None;
            } else {
                current.push(character);
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub(crate) fn substitute_args(content: &str, args: &[String]) -> String {
    let mut output = String::with_capacity(content.len());
    let mut index = 0;
    while index < content.len() {
        let remaining = &content[index..];
        if let Some(consumed) = remaining
            .strip_prefix("${@:")
            .and_then(|slice| slice.find('}').map(|end| end + 5))
        {
            let expression = &remaining[4..consumed - 1];
            output.push_str(&substitute_arg_slice(expression, args));
            index += consumed;
        } else if remaining.starts_with("$ARGUMENTS") {
            output.push_str(&args.join(" "));
            index += "$ARGUMENTS".len();
        } else if remaining.starts_with("$@") {
            output.push_str(&args.join(" "));
            index += "$@".len();
        } else if let Some(position) = positional_arg_ref(remaining) {
            output.push_str(
                args.get(position.saturating_sub(1))
                    .map_or("", String::as_str),
            );
            index += 1 + position.to_string().len();
        } else {
            let character = remaining.chars().next().expect("non-empty remaining");
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

fn positional_arg_ref(input: &str) -> Option<usize> {
    let mut chars = input.chars();
    if chars.next()? != '$' {
        return None;
    }
    let digits = chars.take_while(char::is_ascii_digit).collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn substitute_arg_slice(expression: &str, args: &[String]) -> String {
    let mut pieces = expression.split(':');
    let start = pieces
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let length = pieces.next().and_then(|value| value.parse::<usize>().ok());
    let end = length.map_or(args.len(), |length| start.saturating_add(length));
    args.get(start..args.len().min(end))
        .unwrap_or_default()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{parse_command_args, substitute_args};

    #[test]
    fn parse_command_args_preserves_quoted_segments() {
        assert_eq!(
            parse_command_args("/review src/lib.rs \"security pass\" 'api audit'"),
            vec!["/review", "src/lib.rs", "security pass", "api audit"]
        );
    }

    #[test]
    fn substitute_args_replaces_positional_and_slice_refs() {
        let args = vec![
            "Button".to_owned(),
            "click handler".to_owned(),
            "disabled support".to_owned(),
        ];

        let result = substitute_args(
            "name=$1 all=$@ named=$ARGUMENTS rest=${@:2} one=${@:2:1} missing=$4",
            &args,
        );

        assert_eq!(
            result,
            "name=Button all=Button click handler disabled support named=Button click handler disabled support rest=click handler disabled support one=click handler missing="
        );
    }
}
