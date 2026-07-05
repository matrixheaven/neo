//! Terminal capability detection used by the interactive TUI and shell tools.

use std::env;

use neo_tui::terminal_capabilities::{
    AnsiCapabilities, TerminalCapabilities, TerminalImageCapabilities,
};
use neo_tui::terminal_image::ImageProtocolPreference;

/// Detect terminal capabilities from environment variables and whether stdout is
/// a real terminal.
#[must_use]
pub fn detect_terminal_capabilities(
    protocol: ImageProtocolPreference,
    is_terminal: bool,
) -> TerminalCapabilities {
    let env_var = |name: &str| env::var(name);
    detect_terminal_capabilities_with_env(protocol, is_terminal, env_var)
}

#[must_use]
pub fn detect_terminal_capabilities_with_env(
    protocol: ImageProtocolPreference,
    is_terminal: bool,
    env_var: impl Fn(&str) -> std::result::Result<String, env::VarError>,
) -> TerminalCapabilities {
    let term = env_var("TERM").unwrap_or_default().to_ascii_lowercase();
    let term_program = env_var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let has_env = |name: &str| env_var(name).is_ok();

    let is_dumb = term == "dumb" || term.is_empty();
    let no_color = has_env("NO_COLOR");
    let ci = has_env("CI");
    let wt_session = has_env("WT_SESSION");
    let conservative_multiplexer = has_env("TMUX")
        || has_env("STY")
        || has_env("SSH_CONNECTION")
        || has_env("SSH_TTY")
        || term.starts_with("screen")
        || term.contains("tmux");

    let image_disabled =
        !is_terminal || is_dumb || no_color || ci || wt_session || conservative_multiplexer;

    let image = if image_disabled {
        TerminalImageCapabilities::default()
    } else {
        detect_image_capabilities(protocol, &term, &term_program, has_env)
    };

    let ansi = if !is_terminal || is_dumb || ci {
        AnsiCapabilities::default()
    } else {
        let kitty_like = has_env("KITTY_WINDOW_ID")
            || has_env("WEZTERM_PANE")
            || has_env("GHOSTTY_RESOURCES_DIR")
            || term.contains("kitty")
            || term.contains("ghostty")
            || term_program.contains("wezterm")
            || term_program.contains("ghostty");
        AnsiCapabilities {
            color: !no_color,
            cursor_addressing: true,
            bracketed_paste: !no_color,
            kitty_keyboard: kitty_like,
            synchronized_output: true,
        }
    };

    TerminalCapabilities { ansi, image }
}

fn detect_image_capabilities(
    protocol: ImageProtocolPreference,
    term: &str,
    term_program: &str,
    has_env: impl Fn(&str) -> bool,
) -> TerminalImageCapabilities {
    if matches!(protocol, ImageProtocolPreference::None) {
        return TerminalImageCapabilities::default();
    }

    let static_hints = TerminalImageCapabilities::default()
        .with_kitty(
            has_env("KITTY_WINDOW_ID")
                || has_env("WEZTERM_PANE")
                || has_env("GHOSTTY_RESOURCES_DIR")
                || term.contains("kitty")
                || term.contains("ghostty")
                || term_program.contains("wezterm")
                || term_program.contains("ghostty"),
        )
        .with_iterm2(term_program.contains("iterm"))
        .with_sixel(term.contains("sixel") || has_env("SIXEL"));

    match protocol {
        ImageProtocolPreference::Kitty => {
            TerminalImageCapabilities::default().with_kitty(static_hints.kitty())
        }
        ImageProtocolPreference::Iterm2 => {
            TerminalImageCapabilities::default().with_iterm2(static_hints.iterm2())
        }
        ImageProtocolPreference::Sixel => {
            TerminalImageCapabilities::default().with_sixel(static_hints.sixel())
        }
        ImageProtocolPreference::Auto => static_hints,
        ImageProtocolPreference::None => TerminalImageCapabilities::default(),
    }
}

/// Image-only wrapper kept for the existing startup wiring and tests.
#[cfg(test)]
#[must_use]
pub fn terminal_image_capabilities_for_policy(
    protocol: ImageProtocolPreference,
    env_var: impl Fn(&str) -> std::result::Result<String, env::VarError>,
) -> TerminalImageCapabilities {
    detect_terminal_capabilities_with_env(protocol, true, env_var).image
}
