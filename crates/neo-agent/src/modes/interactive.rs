use crate::config::AppConfig;
use std::{
    future::Future,
    io::{IsTerminal as _, Stdout, stdout},
    pin::Pin,
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use neo_agent_core::AgentEvent;
use neo_tui::{InputEvent, NeoTuiApp, PromptEdit};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    buffer::Cell,
};

type BoxedTurnFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<AgentEvent>>> + Send + 'a>>;

pub fn execute(config: &AppConfig) -> String {
    let mut controller = controller_for_config(config);
    let _ = controller.submit_empty_prompt();
    controller.render_snapshot()
}

pub async fn execute_tty(config: &AppConfig) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute(config)));
    }

    let mut terminal = RawTerminal::enter()?;
    let mut controller = controller_for_config(config);
    controller
        .run_terminal_loop(|app| terminal.draw(app), CrosstermEvents)
        .await?;
    Ok(None)
}

pub(crate) struct InteractiveController<RunTurn> {
    app: NeoTuiApp,
    run_turn: RunTurn,
}

impl<RunTurn, Fut> InteractiveController<RunTurn>
where
    RunTurn: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<AgentEvent>>> + Send,
{
    pub fn new(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
    ) -> Self {
        Self {
            app: NeoTuiApp::new(title, session_label, model_label),
            run_turn,
        }
    }

    #[allow(dead_code)]
    pub fn type_text(&mut self, text: &str) {
        self.app.prompt_mut().apply_edit(PromptEdit::Insert(text));
    }

    pub fn submit_empty_prompt(&mut self) -> Option<String> {
        self.app.submit_prompt()
    }

    #[allow(dead_code)]
    pub async fn submit_prompt(&mut self) -> Result<String> {
        let Some(prompt) = self.app.submit_prompt() else {
            return Ok(self.render_snapshot());
        };

        let events = (self.run_turn)(vec![prompt]).await?;
        for event in events {
            self.app.apply_agent_event(event);
        }

        Ok(self.render_snapshot())
    }

    pub async fn run_terminal_loop(
        &mut self,
        mut render: impl FnMut(&NeoTuiApp) -> Result<()>,
        mut events: impl TerminalEvents,
    ) -> Result<()> {
        render(&self.app)?;
        loop {
            let event = events.next_input_event()?;
            if self.handle_input_event(event).await? {
                break;
            }
            render(&self.app)?;
        }
        Ok(())
    }

    async fn handle_input_event(&mut self, event: InputEvent) -> Result<bool> {
        match event {
            InputEvent::Insert(character) => {
                self.app
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert(&character.to_string()));
            }
            InputEvent::Backspace => {
                self.app.prompt_mut().apply_edit(PromptEdit::Backspace);
            }
            InputEvent::Delete => {
                self.app.prompt_mut().apply_edit(PromptEdit::Delete);
            }
            InputEvent::MoveLeft => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveLeft);
            }
            InputEvent::MoveRight => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveRight);
            }
            InputEvent::MoveHome => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveHome);
            }
            InputEvent::MoveEnd => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveEnd);
            }
            InputEvent::NewLine => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
            }
            InputEvent::Submit => {
                let Some(prompt) = self.app.submit_prompt() else {
                    return Ok(false);
                };
                let events = (self.run_turn)(vec![prompt]).await?;
                for event in events {
                    self.app.apply_agent_event(event);
                }
            }
            InputEvent::Cancel | InputEvent::Interrupt => return Ok(true),
        }

        Ok(false)
    }

    #[allow(dead_code)]
    #[must_use]
    pub const fn app(&self) -> &NeoTuiApp {
        &self.app
    }

    #[must_use]
    pub fn render_snapshot(&self) -> String {
        render_terminal_fallback(&self.app)
    }
}

pub trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;
}

struct CrosstermEvents;

impl TerminalEvents for CrosstermEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        loop {
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key_event) = event::read()?
                && let Some(input) = InputEvent::from_key_event(key_event)
            {
                return Ok(input);
            }
        }
    }
}

struct RawTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw_mode: RawModeGuard,
}

impl RawTerminal {
    fn enter() -> Result<Self> {
        let raw_mode = RawModeGuard::enable()?;
        let mut output = stdout();
        execute!(output, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(output);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self { terminal, raw_mode })
    }

    fn draw(&mut self, app: &NeoTuiApp) -> Result<()> {
        self.terminal
            .draw(|frame| frame.render_widget(app, frame.area()))?;
        Ok(())
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
        self.raw_mode.disable();
    }
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self { active: true })
    }

    fn disable(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.disable();
    }
}

pub fn controller_for_config<'a>(
    config: &'a AppConfig,
) -> InteractiveController<impl Fn(Vec<String>) -> BoxedTurnFuture<'a> + 'a> {
    InteractiveController::new(
        "neo",
        "new",
        format!("{}/{}", config.default_provider, config.default_model),
        move |prompt| {
            let future: BoxedTurnFuture<'a> = Box::pin(async move {
                let turn = crate::modes::run::run_prompt(&prompt, config).await?;
                Ok(turn.events)
            });
            future
        },
    )
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

#[cfg(test)]
mod tests {
    use neo_agent_core::{AgentEvent, StopReason};

    use super::*;

    #[tokio::test]
    async fn controller_submits_prompt_reduces_turn_events_and_renders_snapshot() {
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |prompt| async move {
                assert_eq!(prompt, vec!["hello neo".to_owned()]);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "Hello".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: ", Neo".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            },
        );

        controller.type_text("hello neo");
        let snapshot = controller.submit_prompt().await.expect("turn succeeds");

        assert!(snapshot.contains("neo | session: test-session | model: openai/gpt-4.1 | Editing"));
        assert!(snapshot.contains("You"));
        assert!(snapshot.contains("hello neo"));
        assert!(snapshot.contains("Assistant"));
        assert!(snapshot.contains("Hello, Neo"));
        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
    }

    #[tokio::test]
    async fn event_loop_types_submits_renders_and_exits_without_a_real_terminal() {
        struct FakeEvents {
            events: std::vec::IntoIter<InputEvent>,
        }

        impl TerminalEvents for FakeEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("expected test event"))
            }
        }

        let mut rendered = Vec::new();
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |prompt| async move {
                assert_eq!(prompt, vec!["hi".to_owned()]);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "hello from controller".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            },
        );

        controller
            .run_terminal_loop(
                |app| {
                    rendered.push(render_terminal_fallback(app));
                    Ok(())
                },
                FakeEvents {
                    events: vec![
                        InputEvent::Insert('h'),
                        InputEvent::Insert('i'),
                        InputEvent::Submit,
                        InputEvent::Cancel,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
        assert!(rendered.iter().any(|snapshot| snapshot.contains("> hi")));
        assert!(
            rendered
                .last()
                .expect("final render")
                .contains("hello from controller")
        );
    }
}
