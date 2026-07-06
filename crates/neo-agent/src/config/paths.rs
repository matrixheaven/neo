use std::{env, path::PathBuf};

use neo_agent_core::session::workspace_sessions_dir as compute_workspace_sessions_dir;

use crate::config::AppConfig;

const CONFIG_DIR: &str = ".neo";
const CONFIG_FILE: &str = "config.toml";

/// Resolve the user's home directory in a cross-platform way.
///
/// On Windows, the standard location is the `USERPROFILE` environment variable.
/// On Unix and macOS, it is `HOME`. This does not consult `NEO_HOME`, which is
/// reserved for overriding the Neo home directory itself.
fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
    }
}

/// Resolve the neo home directory: `$NEO_HOME` if set, otherwise `~/.neo`.
/// This is the single source of truth for the neo home directory — every
/// config file, skill, prompt, and theme lives under here.
///
/// Returns `None` only when the platform home directory cannot be determined.
/// Callers should surface this as a clear configuration error rather than
/// falling back to a current-directory path, which would violate the no
/// project-local config architecture.
pub(crate) fn neo_home() -> Option<PathBuf> {
    env::var_os("NEO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home_dir().map(|home| home.join(CONFIG_DIR)))
}

/// The single config file path: `<neo_home>/config.toml`, if a home exists.
///
/// Returns `None` when the platform home directory cannot be determined so the
/// caller can report a clear error instead of writing to an arbitrary location.
pub(crate) fn default_config_path() -> Option<PathBuf> {
    neo_home().map(|home| home.join(CONFIG_FILE))
}

pub(crate) fn global_prompts_dir() -> Option<PathBuf> {
    neo_home().map(|home| home.join("prompts"))
}

/// Compute the workspace-scoped sessions directory for a given config.
///
/// Given the central `sessions_dir` (e.g. `~/.neo/sessions`) and the
/// project directory, returns `<sessions_dir>/wd_<slug>_<hash12>/`.
pub(crate) fn workspace_sessions_dir(config: &AppConfig) -> PathBuf {
    compute_workspace_sessions_dir(&config.sessions_dir, &config.project_dir)
}

pub(crate) fn expand_user_path(path: PathBuf) -> PathBuf {
    expand_user_path_with_home(path, user_home().as_deref())
}

pub(crate) fn expand_user_path_with_home(path: PathBuf, home: Option<&std::path::Path>) -> PathBuf {
    let Some(raw) = path.to_str().map(str::to_owned) else {
        return path;
    };
    if raw == "~" {
        return home.map(std::path::Path::to_path_buf).unwrap_or(path);
    }
    let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix(r"~\")) else {
        return path;
    };
    home.map_or(path, |home| home.join(rest))
}

/// Resolve the user's home directory for tilde expansion and similar purposes.
pub(crate) fn user_home() -> Option<PathBuf> {
    user_home_dir()
}
