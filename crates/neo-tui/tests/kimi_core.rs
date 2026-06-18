use neo_tui::ansi::{Color, Style};
use neo_tui::core::{
    Component, Container, Finalization, GutterContainer, InputResult, Line, Span, Text,
};

struct StaticComponent {
    rows: Vec<Line>,
    finalization: Finalization,
}

impl Component for StaticComponent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.rows.clone()
    }

    fn finalization(&self) -> Finalization {
        self.finalization
    }
}

#[test]
fn line_visible_width_ignores_ansi_styles() {
    let line = Line::from_spans(vec![
        Span::styled("hello", Style::default().fg(Color::Green)),
        Span::raw(" 世界"),
    ]);

    assert_eq!(line.visible_width(), 10);
    let ansi = line.to_ansi();
    assert!(ansi.contains("\x1b[32m"));
    assert!(ansi.contains("hello"));
    assert!(ansi.contains("世界"));
}

#[test]
fn line_truncate_preserves_visible_width_contract() {
    let line = Line::raw("abcdef世界");
    let truncated = line.truncate_to_width(8);

    assert_eq!(truncated.visible_width(), 7);
    assert_eq!(neo_tui::ansi::strip_ansi(&truncated.to_ansi()), "abcdef…");
}

#[test]
fn component_defaults_to_live_and_ignored_input() {
    let mut component = StaticComponent {
        rows: vec![Line::raw("ready")],
        finalization: Finalization::Live,
    };

    assert_eq!(component.finalization(), Finalization::Live);
    assert_eq!(
        component.handle_input(neo_tui::InputEvent::Cancel),
        InputResult::Ignored
    );
    assert_eq!(component.render(80), vec![Line::raw("ready")]);
}

#[test]
fn container_stacks_children_in_order() {
    let mut container = Container::new();
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("first")],
        finalization: Finalization::Finalized,
    }));
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("second")],
        finalization: Finalization::Finalized,
    }));

    let rendered = container.render(80);
    assert_eq!(rendered, vec![Line::raw("first"), Line::raw("second")]);
}

#[test]
fn gutter_container_pads_left_without_trailing_spaces() {
    let mut container = GutterContainer::new(2, 4);
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("x")],
        finalization: Finalization::Finalized,
    }));

    let rendered = container.render(10);
    assert_eq!(rendered, vec![Line::raw("  x")]);
}

#[test]
fn text_wraps_by_visible_width() {
    let mut text = Text::new("hello world 世界");
    let rendered = text.render(8);

    assert!(rendered.iter().all(|line| line.visible_width() <= 8));
    assert_eq!(rendered[0], Line::raw("hello"));
}
