use crate::input::InputEvent;

use super::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finalization {
    Live,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResult {
    Ignored,
    Handled,
    Submitted,
    Cancelled,
}

pub trait Component {
    fn render(&mut self, width: usize) -> Vec<Line>;

    fn invalidate(&mut self) {}

    fn finalization(&self) -> Finalization {
        Finalization::Live
    }

    fn handle_input(&mut self, _input: InputEvent) -> InputResult {
        InputResult::Ignored
    }
}

pub trait Expandable {
    fn set_expanded(&mut self, expanded: bool);
}
