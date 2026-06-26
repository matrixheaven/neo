use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(String);

impl KeyId {
    pub fn new(value: impl Into<String>) -> Result<Self, KeyIdError> {
        let value = value.into();
        let normalized = normalize_key_id(&value).ok_or_else(|| KeyIdError {
            value: value.clone(),
        })?;
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_text_insertion_key(&self) -> bool {
        let mut parts = self.0.split('+').collect::<Vec<_>>();
        let Some(base) = parts.pop() else {
            return false;
        };
        let has_action_modifier = parts
            .iter()
            .any(|modifier| matches!(*modifier, "ctrl" | "alt" | "super"));
        !has_action_modifier && (base == "space" || base.chars().count() == 1)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyIdError {
    value: String,
}

impl fmt::Display for KeyIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid key id: {}", self.value)
    }
}

impl std::error::Error for KeyIdError {}

fn normalize_key_id(value: &str) -> Option<String> {
    let mut parts = value
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let base = parts.pop()?.to_ascii_lowercase();
    let base = match base.as_str() {
        "esc" => "escape".to_string(),
        "return" => "enter".to_string(),
        "pageup" => "pageup".to_string(),
        "pagedown" => "pagedown".to_string(),
        _ => base,
    };

    if !is_valid_base_key(&base) {
        return None;
    }

    let mut modifiers = Vec::new();
    for part in parts {
        let modifier = part.to_ascii_lowercase();
        if !matches!(modifier.as_str(), "ctrl" | "alt" | "shift" | "super") {
            return None;
        }
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
    }
    modifiers.push(base);
    Some(modifiers.join("+"))
}

fn is_valid_base_key(base: &str) -> bool {
    matches!(
        base,
        "escape"
            | "enter"
            | "tab"
            | "space"
            | "backspace"
            | "delete"
            | "insert"
            | "clear"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "up"
            | "down"
            | "left"
            | "right"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "`"
            | "-"
            | "="
            | "["
            | "]"
            | "\\"
            | ";"
            | "'"
            | ","
            | "."
            | "/"
            | "!"
            | "@"
            | "#"
            | "$"
            | "%"
            | "^"
            | "&"
            | "*"
            | "("
            | ")"
            | "_"
            | "|"
            | "~"
            | "{"
            | "}"
            | ":"
            | "<"
            | ">"
            | "?"
    ) || base.chars().count() == 1
}
