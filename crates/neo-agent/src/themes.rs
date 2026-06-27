use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::config::expand_user_path;
#[cfg(test)]
use crate::config::expand_user_path_with_home;
use anyhow::{Context, bail};
use neo_tui::primitive::Color;
use neo_tui::shell::TuiTheme;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub name: String,
    pub theme: TuiTheme,
    pub source: Option<PathBuf>,
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            theme: TuiTheme::default(),
            source: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ThemeFile {
    name: Option<String>,
    #[serde(default)]
    colors: ThemeColors,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeColors {
    text_primary: Option<String>,
    prompt: Option<String>,
    brand: Option<String>,
    status_ok: Option<String>,
    status_error: Option<String>,
    status_warn: Option<String>,
    text_muted: Option<String>,
    user_message: Option<String>,
    diff_added: Option<String>,
    diff_removed: Option<String>,
    diff_hunk: Option<String>,
    diff_context: Option<String>,
    selection_bg: Option<String>,
    status_pending: Option<String>,
    status_cancelled: Option<String>,
    approval_border: Option<String>,
    selected_fg: Option<String>,
    selected_bg: Option<String>,
    overlay_border: Option<String>,
    footer_permission_allow: Option<String>,
    footer_permission_ask: Option<String>,
    footer_permission_deny: Option<String>,
    footer_working: Option<String>,
    footer_context_ok: Option<String>,
    footer_context_warn: Option<String>,
    footer_context_critical: Option<String>,
    shell_mode: Option<String>,
}

pub fn resolve_theme() -> anyhow::Result<ResolvedTheme> {
    let mut paths = Vec::new();
    paths.extend(discover_themes()?);

    let Some(path) = paths.first() else {
        return Ok(ResolvedTheme::default());
    };
    load_theme(path)
}

/// Discover theme files under `~/.neo/themes` (the single neo home).
fn discover_themes() -> anyhow::Result<Vec<PathBuf>> {
    let Some(home) = crate::config::neo_home() else {
        return Ok(Vec::new());
    };
    discover_theme_dir(&home.join("themes"))
}

fn discover_theme_dir(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_theme_paths(dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_theme_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read theme dir {}", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read theme dir entry {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_theme_paths(&path, paths)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && path.is_file()
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn load_theme(path: &Path) -> anyhow::Result<ResolvedTheme> {
    let path = resolve_theme_path(path);
    if path.is_dir() {
        let discovered = discover_theme_dir(&path)?;
        let Some(path) = discovered.first() else {
            bail!(
                "theme directory {} contains no .json themes",
                path.display()
            );
        };
        return load_theme_file(path);
    }
    load_theme_file(&path)
}

fn resolve_theme_path(path: &Path) -> PathBuf {
    let path = expand_user_path(path.to_path_buf());
    if path.is_absolute() {
        path
    } else {
        // Relative theme paths resolve against the neo home.
        crate::config::neo_home().map_or_else(|| PathBuf::from("."), |home| home.join(path))
    }
}

fn load_theme_file(path: &Path) -> anyhow::Result<ResolvedTheme> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read theme {}", path.display()))?;
    let file: ThemeFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse theme {}", path.display()))?;
    let mut theme = TuiTheme::default();
    apply_colors(&mut theme, &file.colors, path)?;
    let name = file
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("theme")
                .to_owned()
        });
    Ok(ResolvedTheme {
        name,
        theme,
        source: Some(path.to_path_buf()),
    })
}

fn apply_colors(theme: &mut TuiTheme, colors: &ThemeColors, path: &Path) -> anyhow::Result<()> {
    apply_core_colors(theme, colors, path)?;
    apply_diff_and_selection_colors(theme, colors, path)?;
    apply_footer_colors(theme, colors, path)?;
    Ok(())
}

fn apply_core_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.text_primary,
        "text_primary",
        colors.text_primary.as_deref(),
        path,
    )?;
    apply_color(&mut theme.prompt, "prompt", colors.prompt.as_deref(), path)?;
    apply_color(&mut theme.brand, "brand", colors.brand.as_deref(), path)?;
    apply_color(
        &mut theme.status_ok,
        "status_ok",
        colors.status_ok.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_error,
        "status_error",
        colors.status_error.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_warn,
        "status_warn",
        colors.status_warn.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.text_muted,
        "text_muted",
        colors.text_muted.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.user_message,
        "user_message",
        colors.user_message.as_deref(),
        path,
    )
}

fn apply_diff_and_selection_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.diff_added,
        "diff_added",
        colors.diff_added.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_removed,
        "diff_removed",
        colors.diff_removed.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_hunk,
        "diff_hunk",
        colors.diff_hunk.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_context,
        "diff_context",
        colors.diff_context.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selection_bg,
        "selection_bg",
        colors.selection_bg.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_pending,
        "status_pending",
        colors.status_pending.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_cancelled,
        "status_cancelled",
        colors.status_cancelled.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.approval_border,
        "approval_border",
        colors.approval_border.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selected_fg,
        "selected_fg",
        colors.selected_fg.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selected_bg,
        "selected_bg",
        colors.selected_bg.as_deref(),
        path,
    )
}

fn apply_footer_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.overlay_border,
        "overlay_border",
        colors.overlay_border.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_allow,
        "footer_permission_allow",
        colors.footer_permission_allow.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_ask,
        "footer_permission_ask",
        colors.footer_permission_ask.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_deny,
        "footer_permission_deny",
        colors.footer_permission_deny.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_working,
        "footer_working",
        colors.footer_working.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_ok,
        "footer_context_ok",
        colors.footer_context_ok.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_warn,
        "footer_context_warn",
        colors.footer_context_warn.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_critical,
        "footer_context_critical",
        colors.footer_context_critical.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.shell_mode,
        "shell_mode",
        colors.shell_mode.as_deref(),
        path,
    )?;
    Ok(())
}

fn apply_color(
    target: &mut Color,
    field: &str,
    value: Option<&str>,
    path: &Path,
) -> anyhow::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    *target = parse_color(value)
        .with_context(|| format!("invalid color for {field} in {}", path.display()))?;
    Ok(())
}

fn parse_color(value: &str) -> anyhow::Result<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    named_color(value)
}

fn parse_hex_color(hex: &str) -> anyhow::Result<Color> {
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("expected #rrggbb");
    }
    let red = u8::from_str_radix(&hex[0..2], 16)?;
    let green = u8::from_str_radix(&hex[2..4], 16)?;
    let blue = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Color::Rgb(red, green, blue))
}

fn named_color(value: &str) -> anyhow::Result<Color> {
    let normalized = value.to_ascii_lowercase();
    named_color_table()
        .iter()
        .find_map(|(name, color)| (*name == normalized).then_some(*color))
        .ok_or_else(|| anyhow::anyhow!("unknown color {value:?}"))
}

fn named_color_table() -> &'static [(&'static str, Color)] {
    &[
        ("reset", Color::Reset),
        ("black", Color::Black),
        ("red", Color::Red),
        ("green", Color::Green),
        ("yellow", Color::Yellow),
        ("blue", Color::Blue),
        ("magenta", Color::Magenta),
        ("cyan", Color::Cyan),
        ("gray", Color::Gray),
        ("grey", Color::Gray),
        ("darkgray", Color::DarkGray),
        ("dark_gray", Color::DarkGray),
        ("dark-grey", Color::DarkGray),
        ("lightred", Color::LightRed),
        ("light_red", Color::LightRed),
        ("light-red", Color::LightRed),
        ("lightgreen", Color::LightGreen),
        ("light_green", Color::LightGreen),
        ("light-green", Color::LightGreen),
        ("lightyellow", Color::LightYellow),
        ("light_yellow", Color::LightYellow),
        ("light-yellow", Color::LightYellow),
        ("lightblue", Color::LightBlue),
        ("light_blue", Color::LightBlue),
        ("light-blue", Color::LightBlue),
        ("lightmagenta", Color::LightMagenta),
        ("light_magenta", Color::LightMagenta),
        ("light-magenta", Color::LightMagenta),
        ("lightcyan", Color::LightCyan),
        ("light_cyan", Color::LightCyan),
        ("light-cyan", Color::LightCyan),
        ("white", Color::White),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn theme_json_uses_role_color_keys() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("role-theme.json");
        fs::write(
            &path,
            r##"
{
  "name": "Role Theme",
  "colors": {
    "text_primary": "#010203",
    "text_muted": "#040506",
    "brand": "#070809",
    "status_ok": "#0a0b0c",
    "status_warn": "#0d0e0f",
    "status_error": "#101112",
    "status_pending": "#131415",
    "status_cancelled": "darkgray",
    "user_message": "#161718"
  }
}
"##,
        )
        .expect("write theme");

        let resolved = load_theme_file(&path).expect("load theme");

        assert_eq!(resolved.theme.text_primary, Color::Rgb(1, 2, 3));
        assert_eq!(resolved.theme.text_muted, Color::Rgb(4, 5, 6));
        assert_eq!(resolved.theme.brand, Color::Rgb(7, 8, 9));
        assert_eq!(resolved.theme.status_ok, Color::Rgb(10, 11, 12));
        assert_eq!(resolved.theme.status_warn, Color::Rgb(13, 14, 15));
        assert_eq!(resolved.theme.status_error, Color::Rgb(16, 17, 18));
        assert_eq!(resolved.theme.status_pending, Color::Rgb(19, 20, 21));
        assert_eq!(resolved.theme.status_cancelled, Color::DarkGray);
        assert_eq!(resolved.theme.user_message, Color::Rgb(22, 23, 24));
    }

    #[test]
    fn theme_json_rejects_old_color_keys() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("old-theme.json");
        fs::write(
            &path,
            r##"
{
  "name": "Old Theme",
  "colors": {
    "accent": "#070809"
  }
}
"##,
        )
        .expect("write theme");

        let error = load_theme_file(&path).expect_err("old key should fail");
        assert!(error.to_string().contains("failed to parse theme"));
    }

    #[test]
    fn theme_path_tilde_expands_to_user_home() {
        assert_eq!(
            expand_user_path_with_home(
                PathBuf::from("~/themes/night.json"),
                Some(Path::new("/home/alice")),
            ),
            PathBuf::from("/home/alice/themes/night.json")
        );
    }
}
