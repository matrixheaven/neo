//! Extracted: terminal image-capability detection from env + protocol policy.

use std::env;

use neo_tui::terminal_image::{ImageProtocolPreference, TerminalImageCapabilities};

pub(super) fn terminal_image_capabilities_for_policy(
    protocol: ImageProtocolPreference,
    env_var: impl Fn(&str) -> std::result::Result<String, env::VarError>,
) -> TerminalImageCapabilities {
    if matches!(protocol, ImageProtocolPreference::None) {
        return TerminalImageCapabilities::default();
    }

    let term = env_var("TERM").unwrap_or_default().to_ascii_lowercase();
    let term_program = env_var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let has_env = |name: &str| env_var(name).is_ok();
    let conservative_multiplexer = has_env("TMUX")
        || has_env("STY")
        || has_env("SSH_CONNECTION")
        || has_env("SSH_TTY")
        || term.starts_with("screen")
        || term.contains("tmux");
    if conservative_multiplexer {
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
