use neo_ai::ModelSpec;

pub(crate) fn scoped_models<'a>(
    models: impl IntoIterator<Item = &'a ModelSpec>,
    scope: &[String],
) -> Vec<ModelSpec> {
    let scope = scope
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .collect::<Vec<_>>();
    models
        .into_iter()
        .filter(|model| {
            scope.is_empty()
                || scope
                    .iter()
                    .any(|pattern| model_matches_scope_pattern(model, pattern))
        })
        .cloned()
        .collect()
}

fn model_matches_scope_pattern(model: &ModelSpec, pattern: &str) -> bool {
    let pattern = strip_thinking_suffix(pattern).trim();
    if pattern.is_empty() {
        return false;
    }
    let qualified = format!("{}/{}", model.provider.0, model.model);
    if pattern == qualified || pattern == model.model {
        return true;
    }
    if has_glob_meta(pattern) {
        return wildcard_match(pattern, &qualified) || wildcard_match(pattern, &model.model);
    }
    fuzzy_match(&qualified, pattern) || fuzzy_match(&model.model, pattern)
}

fn strip_thinking_suffix(pattern: &str) -> &str {
    let Some((model, suffix)) = pattern.rsplit_once(':') else {
        return pattern;
    };
    if matches!(
        suffix,
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh"
    ) {
        model
    } else {
        pattern
    }
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '*' | '?' | '['))
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let mut row = wildcard_initial_row(&pattern);
    for character in text.chars() {
        row = wildcard_advance_row(&pattern, &row, character);
    }
    row.last().copied().unwrap_or(false)
}

fn wildcard_initial_row(pattern: &[char]) -> Vec<bool> {
    let mut row = vec![false; pattern.len() + 1];
    row[0] = true;
    for (index, character) in pattern.iter().enumerate() {
        row[index + 1] = row[index] && *character == '*';
    }
    row
}

fn wildcard_advance_row(pattern: &[char], previous: &[bool], text_character: char) -> Vec<bool> {
    let mut current = vec![false; pattern.len() + 1];
    for (index, pattern_character) in pattern.iter().copied().enumerate() {
        current[index + 1] = wildcard_cell_matches(
            pattern_character,
            text_character,
            previous[index],
            previous[index + 1],
            current[index],
        );
    }
    current
}

fn wildcard_cell_matches(
    pattern_character: char,
    text_character: char,
    diagonal: bool,
    previous_row: bool,
    current_row: bool,
) -> bool {
    match pattern_character {
        '*' => previous_row || current_row,
        '?' => diagonal,
        literal => diagonal && literal == text_character,
    }
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    if haystack.contains(&needle) {
        return true;
    }
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_char| chars.any(|candidate| candidate == needle_char))
}
