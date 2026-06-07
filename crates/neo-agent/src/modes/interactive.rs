use crate::config::AppConfig;
use std::{future::Future, pin::Pin};

use anyhow::Result;
use neo_agent_core::AgentEvent;
use neo_tui::{NeoTuiApp, PromptEdit};
use ratatui::{Terminal, backend::TestBackend, buffer::Cell};

type BoxedTurnFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<AgentEvent>>> + Send + 'a>>;

pub fn execute(config: &AppConfig) -> String {
    let mut controller = controller_for_config(config);
    let _ = controller.submit_empty_prompt();
    controller.render_snapshot()
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
}
