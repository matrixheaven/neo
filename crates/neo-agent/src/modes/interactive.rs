use crate::config::AppConfig;
use neo_tui::NeoTuiApp;
use ratatui::{Terminal, backend::TestBackend, buffer::Cell};

pub fn execute(config: &AppConfig) -> String {
    render_terminal_fallback(&NeoTuiApp::new(
        "neo",
        "new",
        format!("{}/{}", config.default_provider, config.default_model),
    ))
}

fn render_terminal_fallback(app: &NeoTuiApp) -> String {
    let width = 80;
    let height = 24;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is valid");
    terminal
        .draw(|frame| frame.render_widget(app, frame.area()))
        .expect("fallback app render succeeds");

    let lines = terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|line| {
            line.iter()
                .map(Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>();
    format!("{}\n", lines.join("\n").trim_end())
}
