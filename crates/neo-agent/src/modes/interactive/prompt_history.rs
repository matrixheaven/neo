//! Extracted: prompt-history load/append helpers for [`InteractiveController`].

use crate::modes::interactive::InteractiveController;

impl InteractiveController {
    /// Load workspace prompt history into the composer's in-memory history.
    /// Failures are non-fatal: prompt history is a convenience, not a runtime
    /// dependency, so we silently keep an empty history on load errors.
    pub(super) fn load_prompt_history(&mut self) {
        let Some(store) = self.prompt_history.clone() else {
            return;
        };
        match store.load_recent() {
            Ok(entries) => {
                self.tui.chrome_mut().prompt_mut().set_history(entries);
            }
            Err(error) => {
                tracing::warn!(?error, "prompt history unavailable");
            }
        }
    }

    /// Persist an accepted prompt to the workspace history store. Never fails
    /// the calling submit path: append errors become a soft status warning.
    pub(super) fn append_prompt_history(&mut self, prompt: &str) {
        let Some(store) = self.prompt_history.clone() else {
            return;
        };
        let session_id = self.active_session_id.as_deref();
        if let Err(error) = store.append(session_id, prompt) {
            tracing::warn!(?error, "failed to append prompt history");
            self.push_status(format!("Prompt history unavailable: {error}"));
        }
    }
}
