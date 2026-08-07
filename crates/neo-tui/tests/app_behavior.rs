//! App shell chrome integration behavior: footer, prompt, blocking
//! dialogs, task browser, theme manager, and question overlays.

#[path = "app_behavior/blocking_dialogs.rs"]
mod blocking_dialogs;
#[path = "app_behavior/footer.rs"]
mod footer;
#[path = "app_behavior/questions.rs"]
mod questions;
#[path = "app_behavior/shell.rs"]
mod shell;
#[path = "app_behavior/shell_prompt.rs"]
mod shell_prompt;
#[path = "app_behavior/shell_transcript.rs"]
mod shell_transcript;
#[path = "app_behavior/task_browser.rs"]
mod task_browser;
#[path = "app_behavior/theme_manager.rs"]
mod theme_manager;
