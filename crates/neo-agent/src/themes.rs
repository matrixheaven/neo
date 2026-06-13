use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use neo_tui::TuiTheme;
use ratatui::style::Color;
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
struct ThemeColors {
    header: Option<String>,
    prompt: Option<String>,
    user: Option<String>,
    assistant: Option<String>,
    notice: Option<String>,
    diff_added: Option<String>,
    diff_removed: Option<String>,
    diff_hunk: Option<String>,
    diff_context: Option<String>,
    selection_bg: Option<String>,
    pending: Option<String>,
    running: Option<String>,
    succeeded: Option<String>,
    failed: Option<String>,
    cancelled: Option<String>,
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
    footer_hint: Option<String>,
}

pub fn resolve_theme(
    project_dir: &Path,
    explicit_paths: &[PathBuf],
    no_themes: bool,
) -> anyhow::Result<ResolvedTheme> {
    let mut paths = explicit_paths.to_vec();
    if !no_themes {
        paths.extend(discover_project_themes(project_dir)?);
        paths.extend(discover_global_themes()?);
    }

    let Some(path) = paths.first() else {
        return Ok(ResolvedTheme::default());
    };
    load_theme(path, project_dir)
}

fn discover_project_themes(project_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    discover_theme_dir(&project_dir.join(".neo").join("themes"))
}

fn discover_global_themes() -> anyhow::Result<Vec<PathBuf>> {
    let Some(home) = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
    else {
        return Ok(Vec::new());
    };
    discover_theme_dir(&home.join(".neo").join("themes"))
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

fn load_theme(path: &Path, project_dir: &Path) -> anyhow::Result<ResolvedTheme> {
    let path = resolve_theme_path(path, project_dir);
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

fn resolve_theme_path(path: &Path, project_dir: &Path) -> PathBuf {
    let path = expand_user_path(path.to_path_buf());
    if path.is_absolute() {
        path
    } else {
        project_dir.join(path)
    }
}

fn expand_user_path(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str().map(str::to_owned) else {
        return path;
    };
    if raw == "~" {
        return home_dir().unwrap_or(path);
    }
    let Some(rest) = raw.strip_prefix("~/") else {
        return path;
    };
    home_dir().map_or(path, |home| home.join(rest))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
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

pub fn list_project_themes(project_dir: &Path) -> anyhow::Result<String> {
    let themes = load_project_theme_files(project_dir)?;
    if themes.is_empty() {
        return Ok("no themes\n".to_owned());
    }

    let mut output = String::new();
    for theme in themes {
        use std::fmt::Write as _;
        let _ = writeln!(
            output,
            "{}\t{}",
            theme.name,
            theme
                .source
                .as_ref()
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string())
        );
    }
    Ok(output)
}

pub fn preview_project_theme(project_dir: &Path, name: &str) -> anyhow::Result<String> {
    let theme = load_project_theme_files(project_dir)?
        .into_iter()
        .find(|theme| {
            theme.name == name
                || theme.source.as_ref().is_some_and(|path| {
                    path.file_stem()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|stem| stem == name)
                })
        })
        .ok_or_else(|| anyhow::anyhow!("theme {name:?} not found"))?;
    let source = theme
        .source
        .as_ref()
        .with_context(|| format!("theme {name:?} has no source"))?;
    let content = fs::read_to_string(source)
        .with_context(|| format!("failed to read theme {}", source.display()))?;
    Ok(format!(
        "{}\t{}\n{}\n",
        theme.name,
        source.display(),
        content
    ))
}

fn load_project_theme_files(project_dir: &Path) -> anyhow::Result<Vec<ResolvedTheme>> {
    let mut themes = Vec::new();
    for path in discover_project_themes(project_dir)? {
        themes.push(load_theme_file(&path)?);
    }
    themes.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.source.cmp(&right.source))
    });
    Ok(themes)
}

fn apply_colors(theme: &mut TuiTheme, colors: &ThemeColors, path: &Path) -> anyhow::Result<()> {
    apply_color(&mut theme.header, "header", colors.header.as_deref(), path)?;
    apply_color(&mut theme.prompt, "prompt", colors.prompt.as_deref(), path)?;
    apply_color(&mut theme.user, "user", colors.user.as_deref(), path)?;
    apply_color(
        &mut theme.assistant,
        "assistant",
        colors.assistant.as_deref(),
        path,
    )?;
    apply_color(&mut theme.notice, "notice", colors.notice.as_deref(), path)?;
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
        &mut theme.pending,
        "pending",
        colors.pending.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.running,
        "running",
        colors.running.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.succeeded,
        "succeeded",
        colors.succeeded.as_deref(),
        path,
    )?;
    apply_color(&mut theme.failed, "failed", colors.failed.as_deref(), path)?;
    apply_color(
        &mut theme.cancelled,
        "cancelled",
        colors.cancelled.as_deref(),
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
    )?;
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
        &mut theme.footer_hint,
        "footer_hint",
        colors.footer_hint.as_deref(),
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
    match value.to_ascii_lowercase().as_str() {
        "reset" => Ok(Color::Reset),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" | "grey" => Ok(Color::Gray),
        "darkgray" | "dark_gray" | "dark-grey" => Ok(Color::DarkGray),
        "lightred" | "light_red" | "light-red" => Ok(Color::LightRed),
        "lightgreen" | "light_green" | "light-green" => Ok(Color::LightGreen),
        "lightyellow" | "light_yellow" | "light-yellow" => Ok(Color::LightYellow),
        "lightblue" | "light_blue" | "light-blue" => Ok(Color::LightBlue),
        "lightmagenta" | "light_magenta" | "light-magenta" => Ok(Color::LightMagenta),
        "lightcyan" | "light_cyan" | "light-cyan" => Ok(Color::LightCyan),
        "white" => Ok(Color::White),
        _ => bail!("unknown color {value:?}"),
    }
}
