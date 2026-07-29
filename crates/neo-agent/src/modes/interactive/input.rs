use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitGesture {
    CtrlC,
    CtrlD,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ExitConfirmation {
    gesture: ExitGesture,
    timestamp: Instant,
}

impl ExitConfirmation {
    const WINDOW: Duration = Duration::from_millis(500);

    fn new(gesture: ExitGesture) -> Self {
        Self {
            gesture,
            timestamp: Instant::now(),
        }
    }

    fn matches(self, gesture: ExitGesture) -> bool {
        self.gesture == gesture && self.timestamp.elapsed() <= Self::WINDOW
    }
}

impl InteractiveController {
    pub(super) async fn handle_input_event(&mut self, event: InputEvent) -> Result<bool> {
        self.drain_deferred_approval_response().await;
        if self.handle_pending_approval_event(&event).await? {
            return Ok(false);
        }
        if self.handle_task_browser_event(event.clone()).await? {
            return Ok(false);
        }
        if self.handle_rich_dialog_event(event.clone()).await? {
            return Ok(false);
        }
        if self.tui.chrome().transcript_browser_state().is_some() {
            let global_action = match &event {
                InputEvent::Action(
                    action @ (KeybindingAction::AppClear
                    | KeybindingAction::AppExit
                    | KeybindingAction::AppSuspend),
                ) => Some(*action),
                InputEvent::Key(key) => [
                    KeybindingAction::AppClear,
                    KeybindingAction::AppExit,
                    KeybindingAction::AppSuspend,
                ]
                .into_iter()
                .find(|action| self.keybindings.matches(key, *action)),
                _ => None,
            };
            if let Some(action) = global_action {
                return self.handle_keybinding_action(action).await;
            }
        }
        if self.handle_transcript_browser_event(&event) {
            return Ok(false);
        }
        if self.handle_automatic_overflow_event(&event) {
            return Ok(false);
        }
        if self.handle_prompt_edit_event(&event) {
            return Ok(false);
        }
        match event {
            InputEvent::Key(key) => return self.handle_keybinding_key(&key).await,
            InputEvent::Action(action) => return self.handle_keybinding_action(action).await,
            InputEvent::Submit => {
                self.clear_pending_exit_confirmation();
                self.follow_transcript_tail();
                self.submit_current_prompt().await?;
            }
            InputEvent::ScrollUp(rows) => self.scroll_transcript_up(rows),
            InputEvent::ScrollDown(rows) => self.scroll_transcript_down(rows),
            InputEvent::Cancel => return self.handle_cancel_input().await,
            InputEvent::Interrupt => return self.handle_interrupt_input().await,
            InputEvent::Resize { .. }
            | InputEvent::Insert(_)
            | InputEvent::Paste(_)
            | InputEvent::Backspace
            | InputEvent::Delete
            | InputEvent::MoveLeft
            | InputEvent::MoveRight
            | InputEvent::MoveHome
            | InputEvent::MoveEnd
            | InputEvent::NewLine => {}
        }

        Ok(false)
    }

    fn handle_automatic_overflow_event(&mut self, event: &InputEvent) -> bool {
        // Manual review keeps logical precedence; only consume wheel/page while
        // automatic overflow is latched and review is absent.
        if self.tui.chrome().transcript_browser_state().is_some()
            || !self.tui.automatic_overflow_active()
            || matches!(event, InputEvent::Interrupt)
        {
            return false;
        }
        let key_actions = match event {
            InputEvent::Key(key) => self.keybindings.matching_actions(key),
            _ => Vec::new(),
        };
        let matches_action = |action: KeybindingAction| {
            matches!(event, InputEvent::Action(candidate) if *candidate == action)
                || key_actions.contains(&action)
        };
        let scroll = if matches_action(KeybindingAction::EditorPageUp)
            || matches_action(KeybindingAction::SelectPageUp)
        {
            Some((true, 8))
        } else if matches_action(KeybindingAction::EditorPageDown)
            || matches_action(KeybindingAction::SelectPageDown)
        {
            Some((false, 8))
        } else {
            match event {
                InputEvent::ScrollUp(rows) => Some((true, *rows)),
                InputEvent::ScrollDown(rows) => Some((false, *rows)),
                InputEvent::Key(key) => match key.as_str() {
                    "pageup" => Some((true, 8)),
                    "pagedown" => Some((false, 8)),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some((up, rows)) = scroll {
            if up {
                self.tui.scroll_automatic_overflow_up(rows);
            } else {
                self.tui.scroll_automatic_overflow_down(rows);
            }
            self.transcript_mut().mark_dirty();
            return true;
        }
        false
    }

    fn handle_transcript_browser_event(&mut self, event: &InputEvent) -> bool {
        if matches!(event, InputEvent::Interrupt) {
            return false;
        }
        let key_actions = match event {
            InputEvent::Key(key) => self.keybindings.matching_actions(key),
            _ => Vec::new(),
        };
        let matches_action = |action: KeybindingAction| {
            matches!(event, InputEvent::Action(candidate) if *candidate == action)
                || key_actions.contains(&action)
        };
        let toggles_output = matches_action(KeybindingAction::ToolOutputToggle);

        if self.tui.chrome().transcript_browser_state().is_none() {
            if !toggles_output || !self.tui.transcript().has_committed_expandable_entries() {
                return false;
            }
            let expanded = !self.tui.transcript().tool_output_expanded();
            self.tui.chrome_mut().open_transcript_browser(expanded);
            if let Some(state) = self.tui.chrome_mut().transcript_browser_state_mut() {
                state.follow_bottom();
            }
            self.transcript_mut().mark_dirty();
            return true;
        }

        if matches!(event, InputEvent::Cancel)
            || matches!(event, InputEvent::Key(key) if key.as_str() == "escape")
            || matches_action(KeybindingAction::SelectCancel)
        {
            self.tui.chrome_mut().close_transcript_browser();
            self.transcript_mut().mark_dirty();
            return true;
        }
        if toggles_output {
            if let Some(state) = self.tui.chrome_mut().transcript_browser_state_mut() {
                state.toggle();
            }
            self.transcript_mut().mark_dirty();
            return true;
        }
        if matches!(event, InputEvent::Submit) || matches_action(KeybindingAction::InputSubmit) {
            self.tui.chrome_mut().close_transcript_browser();
            self.transcript_mut().mark_dirty();
            return false;
        }

        let scroll = if matches_action(KeybindingAction::EditorCursorUp)
            || matches_action(KeybindingAction::SelectUp)
        {
            Some((true, 1))
        } else if matches_action(KeybindingAction::EditorCursorDown)
            || matches_action(KeybindingAction::SelectDown)
        {
            Some((false, 1))
        } else if matches_action(KeybindingAction::EditorPageUp)
            || matches_action(KeybindingAction::SelectPageUp)
        {
            Some((true, 8))
        } else if matches_action(KeybindingAction::EditorPageDown)
            || matches_action(KeybindingAction::SelectPageDown)
        {
            Some((false, 8))
        } else {
            match event {
                InputEvent::ScrollUp(rows) => Some((true, *rows)),
                InputEvent::ScrollDown(rows) => Some((false, *rows)),
                InputEvent::Key(key) => match key.as_str() {
                    "up" => Some((true, 1)),
                    "down" => Some((false, 1)),
                    "pageup" => Some((true, 8)),
                    "pagedown" => Some((false, 8)),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some((up, rows)) = scroll {
            if let Some(state) = self.tui.chrome_mut().transcript_browser_state_mut() {
                if up {
                    state.scroll_up(rows);
                } else {
                    state.scroll_down(rows);
                }
            }
            self.transcript_mut().mark_dirty();
            return true;
        }
        false
    }

    fn follow_transcript_tail(&mut self) {
        self.transcript_mut()
            .transcript_mut()
            .viewport_mut()
            .follow_bottom();
    }

    pub(super) async fn handle_pending_approval_event(
        &mut self,
        event: &InputEvent,
    ) -> Result<bool> {
        if !self.tui.chrome().approval_is_pending() {
            return Ok(false);
        }
        // The wheel owns transcript navigation even while approval input is focused.
        if matches!(event, InputEvent::ScrollUp(_) | InputEvent::ScrollDown(_)) {
            return Ok(false);
        }
        // Interrupt rejects every visible approval (and any pending runtime
        // channels) instead of being swallowed by the dialog handler.
        if matches!(event, InputEvent::Interrupt) {
            self.reject_all_pending_approvals();
            if self.active_turn.is_some() {
                self.cancel_active_turn().await?;
                self.show_notice("Interrupted");
            }
            return Ok(true);
        }
        if let Some(response) = self
            .tui
            .chrome_mut()
            .handle_pending_approval_input(event.clone())
        {
            self.resolve_approval_response(response).await;
        } else {
            self.sync_inline_approval_selection();
        }
        Ok(true)
    }

    pub(super) async fn handle_rich_dialog_event(&mut self, event: InputEvent) -> Result<bool> {
        if !self.tui.chrome_mut().focused_overlay_is_rich_dialog() {
            return Ok(false);
        }
        let cancelled_question = self
            .tui
            .chrome()
            .question_dialog_state()
            .map(|state| state.id.clone());
        let event = self.dialog_input_event(event);
        let result = self.tui.chrome_mut().handle_focused_dialog_input(event);
        if result == InputResult::Cancelled
            && let Some(id) = cancelled_question
        {
            self.pending_questions.remove(&id);
            self.pending_question_prompts.remove(&id);
        }
        self.process_rich_dialog_result(result).await?;
        Ok(true)
    }

    pub(super) async fn handle_task_browser_event(&mut self, event: InputEvent) -> Result<bool> {
        if self.tui.chrome().task_browser_state().is_none() {
            return Ok(false);
        }
        if matches!(event, InputEvent::Insert('r' | 'R'))
            && self
                .tui
                .chrome()
                .task_browser_state()
                .is_some_and(|state| state.workflow_item().is_none())
        {
            self.refresh_task_browser().await;
            return Ok(true);
        }
        if self
            .tui
            .chrome()
            .task_browser_state()
            .is_some_and(|state| state.answer_draft().is_some())
        {
            let structured_fallback = self
                .tui
                .chrome()
                .task_browser_state()
                .and_then(|state| state.answer_draft())
                .is_some_and(|draft| draft.form.structured_fallback);
            if !structured_fallback {
                let action = match event {
                    InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                        Some(TaskBrowserAction::SubmitAnswer)
                    }
                    InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                        Some(TaskBrowserAction::DismissAnswer)
                    }
                    InputEvent::Action(KeybindingAction::SelectUp) | InputEvent::ScrollUp(_) => {
                        Some(TaskBrowserAction::SelectPreviousAnswerField)
                    }
                    InputEvent::Action(KeybindingAction::SelectDown)
                    | InputEvent::ScrollDown(_)
                    | InputEvent::Action(KeybindingAction::InputTab)
                    | InputEvent::Insert('\t') => Some(TaskBrowserAction::SelectNextAnswerField),
                    InputEvent::MoveLeft => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.cycle_selected_answer_value(-1);
                        }
                        return Ok(true);
                    }
                    InputEvent::MoveRight => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.cycle_selected_answer_value(1);
                        }
                        return Ok(true);
                    }
                    InputEvent::Insert(' ') => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                            && !state.toggle_selected_answer_value()
                        {
                            state.append_selected_answer_char(' ');
                        }
                        return Ok(true);
                    }
                    InputEvent::Insert('+') => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                            && !state.append_selected_answer_object_row()
                        {
                            state.append_selected_answer_char('+');
                        }
                        return Ok(true);
                    }
                    InputEvent::Insert(ch) => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.append_selected_answer_char(ch);
                        }
                        return Ok(true);
                    }
                    InputEvent::Paste(raw) => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.paste_selected_answer_value(&raw);
                        }
                        return Ok(true);
                    }
                    InputEvent::Backspace => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.delete_selected_answer_char();
                        }
                        return Ok(true);
                    }
                    InputEvent::Delete => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                            && !state.remove_selected_answer_object_row()
                        {
                            state.delete_selected_answer_char();
                        }
                        return Ok(true);
                    }
                    InputEvent::NewLine => {
                        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                            state.append_selected_answer_char('\n');
                        }
                        return Ok(true);
                    }
                    _ => return Ok(true),
                };
                if let Some(action) = action {
                    self.apply_task_browser_action(action).await?;
                }
                return Ok(true);
            }
            let action = match event {
                InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                    Some(TaskBrowserAction::SubmitAnswer)
                }
                InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                    Some(TaskBrowserAction::DismissAnswer)
                }
                InputEvent::Insert(ch) => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                        && let Some(draft) = state.answer_draft()
                    {
                        let initial = serde_json::to_string_pretty(&draft.value)
                            .unwrap_or_else(|_| "null".to_owned());
                        let mut raw = if draft.json_editor == initial {
                            String::new()
                        } else {
                            draft.json_editor.clone()
                        };
                        raw.push(ch);
                        state.set_answer_json(raw);
                    }
                    return Ok(true);
                }
                InputEvent::Paste(raw) => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                        state.set_answer_json(raw);
                    }
                    return Ok(true);
                }
                InputEvent::Backspace | InputEvent::Delete => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                        && let Some(draft) = state.answer_draft()
                    {
                        let mut raw = draft.json_editor.clone();
                        raw.pop();
                        state.set_answer_json(raw);
                    }
                    return Ok(true);
                }
                InputEvent::NewLine => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                        && let Some(draft) = state.answer_draft()
                    {
                        let mut raw = draft.json_editor.clone();
                        raw.push('\n');
                        state.set_answer_json(raw);
                    }
                    return Ok(true);
                }
                _ => return Ok(true),
            };
            if let Some(action) = action {
                self.apply_task_browser_action(action).await?;
            }
            return Ok(true);
        }
        if self
            .tui
            .chrome()
            .task_browser_state()
            .is_some_and(|state| state.save_draft().is_some())
        {
            let replace_confirmation = self
                .tui
                .chrome()
                .task_browser_state()
                .and_then(|state| state.save_draft())
                .is_some_and(|draft| draft.replacement.is_some());
            let action = match event {
                InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                    Some(TaskBrowserAction::SubmitSave)
                }
                InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                    Some(TaskBrowserAction::Cancel)
                }
                _ if replace_confirmation => return Ok(true),
                InputEvent::Action(KeybindingAction::InputTab)
                | InputEvent::Insert('\t')
                | InputEvent::Action(KeybindingAction::SelectUp)
                | InputEvent::Action(KeybindingAction::SelectDown) => {
                    Some(TaskBrowserAction::ToggleSaveDestination)
                }
                InputEvent::Insert(ch) => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                        && let Some(draft) = state.save_draft()
                    {
                        let mut name = draft.name.clone();
                        name.push(ch);
                        state.set_save_name(name);
                    }
                    return Ok(true);
                }
                InputEvent::Paste(name) => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                        state.set_save_name(name);
                    }
                    return Ok(true);
                }
                InputEvent::Backspace | InputEvent::Delete => {
                    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut()
                        && let Some(draft) = state.save_draft()
                    {
                        let mut name = draft.name.clone();
                        name.pop();
                        state.set_save_name(name);
                    }
                    return Ok(true);
                }
                _ => return Ok(true),
            };
            if let Some(action) = action {
                self.apply_task_browser_action(action).await?;
            }
            return Ok(true);
        }
        let Some(action) = self.task_browser_action_for_event(event) else {
            return Ok(true);
        };
        self.apply_task_browser_action(action).await?;
        Ok(true)
    }

    pub(super) fn task_browser_action_for_event(
        &self,
        event: InputEvent,
    ) -> Option<TaskBrowserAction> {
        match event {
            InputEvent::Action(KeybindingAction::SelectUp) => Some(TaskBrowserAction::SelectUp),
            InputEvent::Action(KeybindingAction::SelectDown) => Some(TaskBrowserAction::SelectDown),
            InputEvent::Action(KeybindingAction::SelectPageUp) => {
                Some(TaskBrowserAction::SelectPageUp)
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) => {
                Some(TaskBrowserAction::SelectPageDown)
            }
            InputEvent::ScrollUp(_) => Some(TaskBrowserAction::SelectUp),
            InputEvent::ScrollDown(_) => Some(TaskBrowserAction::SelectDown),
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                self.tui.chrome().task_browser_state().map(|state| {
                    if state.stop_confirmation_task_id().is_some() {
                        TaskBrowserAction::ConfirmStop
                    } else if state
                        .workflow_item()
                        .and_then(|item| item.workflow.as_ref())
                        .is_some_and(|workflow| workflow.pending_user.is_some())
                    {
                        TaskBrowserAction::OpenAnswer
                    } else if state.workflow_item().is_some() {
                        match state.focus() {
                            neo_tui::tasks_browser::TaskBrowserFocus::Agents => {
                                TaskBrowserAction::OpenWorkflowChildDetails
                            }
                            _ => TaskBrowserAction::ToggleWorkflowFocus,
                        }
                    } else if state.selected_item().is_some_and(|item| {
                        item.kind == neo_tui::tasks_browser::TaskBrowserKind::Workflow
                    }) {
                        TaskBrowserAction::OpenWorkflow
                    } else {
                        TaskBrowserAction::OpenTaskDetails
                    }
                })
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                Some(TaskBrowserAction::Cancel)
            }
            InputEvent::Action(KeybindingAction::InputTab) | InputEvent::Insert('\t') => self
                .tui
                .chrome()
                .task_browser_state()
                .map_or(Some(TaskBrowserAction::ToggleFilter), |state| {
                    if state.workflow_item().is_some() {
                        Some(TaskBrowserAction::ToggleWorkflowFocus)
                    } else {
                        Some(TaskBrowserAction::ToggleFilter)
                    }
                }),
            InputEvent::MoveHome => Some(TaskBrowserAction::SelectFirst),
            InputEvent::MoveEnd => Some(TaskBrowserAction::SelectLast),
            InputEvent::Insert('q' | 'Q') => Some(TaskBrowserAction::Close),
            InputEvent::Insert('p' | 'P') => {
                self.tui.chrome().task_browser_state().and_then(|state| {
                    state
                        .workflow_item()
                        .map(|_| TaskBrowserAction::TogglePauseResume)
                })
            }
            InputEvent::Insert('x' | 'X') => Some(TaskBrowserAction::RequestStop),
            InputEvent::Insert('s' | 'S') => {
                self.tui.chrome().task_browser_state().and_then(|state| {
                    state
                        .workflow_item()
                        .and_then(|item| item.workflow.as_ref())
                        .filter(|workflow| workflow.inline_unsaved)
                        .map(|_| TaskBrowserAction::RequestSave)
                })
            }
            InputEvent::Insert('o' | 'O') => {
                self.tui.chrome().task_browser_state().and_then(|state| {
                    state
                        .workflow_item()
                        .is_none()
                        .then_some(TaskBrowserAction::ToggleOutputFocus)
                })
            }
            InputEvent::Insert(']') => self.tui.chrome().task_browser_state().and_then(|state| {
                state
                    .workflow_item()
                    .map(|_| TaskBrowserAction::RequestNextChildPage)
            }),
            InputEvent::Insert('[') => self.tui.chrome().task_browser_state().and_then(|state| {
                state
                    .workflow_item()
                    .map(|_| TaskBrowserAction::RequestPrevChildPage)
            }),
            InputEvent::Key(key) => {
                let actions = self.keybindings.matching_actions(&key);
                OVERLAY_ACTION_PRIORITY
                    .iter()
                    .chain(std::iter::once(&KeybindingAction::InputTab))
                    .copied()
                    .find(|action| actions.contains(action))
                    .and_then(|action| {
                        self.task_browser_action_for_event(InputEvent::Action(action))
                    })
            }
            _ => None,
        }
    }

    pub(super) async fn apply_task_browser_action(
        &mut self,
        action: TaskBrowserAction,
    ) -> Result<()> {
        match action {
            TaskBrowserAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            TaskBrowserAction::ConfirmStop => {
                let task_id = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| state.handle_action(TaskBrowserAction::ConfirmStop));
                if let Some(task_id) = task_id {
                    self.stop_task_from_browser(task_id).await;
                }
            }
            TaskBrowserAction::TogglePauseResume => {
                let intent = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| {
                        let resume = state.workflow_item().is_some_and(|item| {
                            item.status == neo_tui::tasks_browser::TaskBrowserStatus::Paused
                        });
                        state
                            .handle_action(TaskBrowserAction::TogglePauseResume)
                            .map(|task_id| (task_id, resume))
                    });
                if let Some((task_id, resume)) = intent {
                    self.control_workflow_from_browser(task_id, resume).await;
                }
            }
            TaskBrowserAction::SubmitAnswer => {
                let submission = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| {
                        let _ = state.handle_action(TaskBrowserAction::SubmitAnswer);
                        state.take_answer_submission()
                    });
                if let Some(submission) = submission {
                    self.answer_workflow_from_browser(submission).await;
                }
            }
            TaskBrowserAction::SubmitSave => {
                let submission = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| {
                        let _ = state.handle_action(TaskBrowserAction::SubmitSave);
                        state.take_save_submission()
                    });
                if let Some(submission) = submission {
                    self.save_workflow_from_browser(submission).await;
                }
            }
            TaskBrowserAction::RequestSave => {
                let task_id = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| {
                        let task_id = state.workflow_item().map(|item| item.id.clone());
                        let _ = state.handle_action(TaskBrowserAction::RequestSave);
                        state.save_draft().is_some().then_some(task_id).flatten()
                    });
                if let Some(task_id) = task_id {
                    self.set_workflow_save_name_from_runtime(&task_id).await;
                }
            }
            TaskBrowserAction::Cancel => {
                let (result, left_workflow) = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .map_or((None, false), |state| {
                        let was_workflow = state.workflow_item().is_some();
                        let result = state.handle_action(TaskBrowserAction::Cancel);
                        (result, was_workflow && state.workflow_item().is_none())
                    });
                if result.as_deref() == Some("__close__") {
                    self.tui.chrome_mut().close_focused_overlay();
                } else if left_workflow {
                    self.refresh_task_browser().await;
                }
            }
            other => {
                let needs_refresh =
                    self.tui
                        .chrome_mut()
                        .task_browser_state_mut()
                        .is_some_and(|state| {
                            let _ = state.handle_action(other);
                            state.list_refresh_requested() || state.take_child_refresh_request()
                        });
                if needs_refresh {
                    self.refresh_task_browser().await;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn refresh_task_browser(&mut self) -> bool {
        let Some(config) = self.local_config.as_ref() else {
            return self
                .tui
                .chrome_mut()
                .task_browser_state_mut()
                .is_some_and(|state| {
                    let before = state.clone();
                    state.set_footer_message("No config available");
                    *state != before
                });
        };
        let intent = self
            .tui
            .chrome()
            .task_browser_state()
            .map(neo_tui::tasks_browser::TaskBrowserState::list_intent)
            .unwrap_or_default();
        let query = neo_agent_core::tools::BackgroundTaskListQuery {
            active_only: intent.active_only,
            kind: intent
                .workflow_only
                .then_some(neo_agent_core::tools::BackgroundTaskKind::Workflow),
            limit: if intent.limit == 0 { 50 } else { intent.limit },
            cursor: intent.cursor,
            ..neo_agent_core::tools::BackgroundTaskListQuery::default()
        };
        let mut page = match config
            .background_tasks
            .list_page_with_live_agents(query, &config.multi_agent)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                return self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .is_some_and(|state| {
                        let before = state.clone();
                        state.set_footer_message(error.to_string());
                        *state != before
                    });
            }
        };
        let child_intent = self
            .tui
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::workflow_child_page_intent);
        if let Some(intent) = child_intent.as_ref()
            && !page.items.iter().any(|item| item.task_id == intent.task_id)
            && let Ok(snapshot) = config.background_tasks.snapshot(&intent.task_id).await
        {
            page.items.insert(0, snapshot);
        }
        let child_error = if let Some(intent) = child_intent {
            let request = neo_agent_core::workflow::WorkflowOperatorRequest {
                step: intent.step,
                cursor: intent.cursor,
                limit: intent.limit,
            };
            match config
                .background_tasks
                .workflow_operator_child_page(&intent.task_id, &request, &config.multi_agent)
                .await
            {
                Ok(child_page) => {
                    if let Some(workflow) = page
                        .items
                        .iter_mut()
                        .find(|item| item.task_id == intent.task_id)
                        .and_then(|item| item.workflow.as_mut())
                    {
                        workflow.child_page = Some(child_page);
                    }
                    None
                }
                Err(error) => Some(error.to_string()),
            }
        } else {
            None
        };
        task_browser::update_workflow_save_availability(
            &mut page,
            &config.background_tasks,
            &config.workflow_definitions,
        )
        .await;
        let snapshot = task_browser::list_page_to_browser_snapshot(&page);
        let changed = self
            .tui
            .chrome_mut()
            .task_browser_state_mut()
            .is_some_and(|state| {
                let before = state.clone();
                state.apply_snapshot(&snapshot);
                if let Some(message) = child_error {
                    state.set_footer_message(message);
                } else {
                    state.clear_footer_message();
                }
                *state != before
            });
        self.last_task_browser_refresh = Some(Instant::now());
        changed
    }

    pub(super) async fn maybe_refresh_task_browser(&mut self) -> bool {
        if self.tui.chrome().task_browser_state().is_none() {
            self.last_task_browser_refresh = None;
            return false;
        }
        let should_refresh = self
            .last_task_browser_refresh
            .is_none_or(|last_refresh| last_refresh.elapsed() >= TASK_BROWSER_REFRESH_INTERVAL);
        if should_refresh {
            return self.refresh_task_browser().await;
        }
        false
    }

    pub(super) async fn stop_task_from_browser(&mut self, task_id: String) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        if let Some(snapshot) = config.multi_agent.cancel_agent_by_id(&task_id) {
            config
                .background_tasks
                .cancel_delegate(&task_id, snapshot)
                .await;
        }
        let result = config
            .background_tasks
            .stop_with_actor(
                &task_id,
                "Stopped from Task Browser",
                config.runtime.shell.max_output_bytes,
                neo_agent_core::workflow::WorkflowActor::Human,
            )
            .await;
        match result {
            Ok(_) => {
                self.refresh_task_browser().await;
            }
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
            }
        }
    }

    async fn control_workflow_from_browser(&mut self, task_id: String, resume: bool) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        let actor = neo_agent_core::workflow::WorkflowActor::Human;
        let result = if resume {
            config
                .background_tasks
                .resume_workflow(&task_id, actor)
                .await
        } else {
            config
                .background_tasks
                .pause_workflow(&task_id, actor)
                .await
        };
        match result {
            Ok(result) if result.is_error => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(result.content);
                }
            }
            Ok(_) => {
                self.refresh_task_browser().await;
            }
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
            }
        }
    }

    async fn answer_workflow_from_browser(
        &mut self,
        submission: neo_tui::tasks_browser::WorkflowAnswerSubmission,
    ) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        let result = config
            .background_tasks
            .answer_workflow(
                &submission.task_id,
                &submission.request_id,
                submission.value,
                neo_agent_core::workflow::WorkflowActor::Human,
            )
            .await;
        match result {
            Ok(result) if result.is_error => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(result.content);
                }
            }
            Ok(_) => {
                self.refresh_task_browser().await;
            }
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
            }
        }
    }

    async fn save_workflow_from_browser(
        &mut self,
        submission: neo_tui::tasks_browser::WorkflowSaveSubmission,
    ) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        let Some(handle) = config
            .background_tasks
            .workflow_handle(&submission.task_id)
            .await
        else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("Workflow is no longer available");
            }
            return;
        };
        let output = match handle.output().await {
            Ok(output) => output,
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
                return;
            }
        };
        if !output.metadata.inline_unsaved {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("This workflow is already saved.");
            }
            return;
        }
        let Some(output_schema) = output.metadata.output_schema.clone() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message(
                    "This workflow does not contain enough information to save.",
                );
            }
            return;
        };
        let scope = match submission.destination {
            neo_tui::tasks_browser::WorkflowSaveDestination::Project => {
                neo_agent_core::workflow::WorkflowSaveScope::Project
            }
            neo_tui::tasks_browser::WorkflowSaveDestination::AllProjects => {
                neo_agent_core::workflow::WorkflowSaveScope::User
            }
        };
        let request = neo_agent_core::workflow::WorkflowSaveRequest {
            name: submission.name.clone(),
            display_name: output
                .metadata
                .display_name
                .clone()
                .unwrap_or_else(|| output.metadata.name.clone()),
            description: output.metadata.description.clone(),
            phases: output.metadata.phases.clone(),
            lua_source: output.metadata.script.clone(),
            input_schema: output.metadata.input_schema.clone(),
            output_schema,
        };
        match config
            .workflow_definitions
            .save(scope, &request, submission.replace)
        {
            Ok(_) => {
                self.refresh_task_browser().await;
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(format!("Saved workflow `{}`.", request.name));
                }
            }
            Err(error) => {
                let replacement = if error.code()
                    == neo_agent_core::workflow::WorkflowErrorCode::DefinitionConflict
                    && !submission.replace
                {
                    let list_scope = match scope {
                        neo_agent_core::workflow::WorkflowSaveScope::Project => {
                            neo_agent_core::workflow::WorkflowListScope::Project
                        }
                        neo_agent_core::workflow::WorkflowSaveScope::User => {
                            neo_agent_core::workflow::WorkflowListScope::User
                        }
                    };
                    let existing_display_name = config
                        .workflow_definitions
                        .list(list_scope)
                        .ok()
                        .and_then(|definitions| {
                            definitions
                                .into_iter()
                                .find(|definition| definition.name.as_str() == request.name)
                        })
                        .map_or_else(
                            || format!("Saved workflow `{}`", request.name),
                            |definition| definition.display_name,
                        );
                    config
                        .workflow_definitions
                        .save_target(scope, &request.name)
                        .ok()
                        .map(|target| {
                            let location = target
                                .manifest_path
                                .parent()
                                .unwrap_or(target.manifest_path.as_path())
                                .to_string_lossy()
                                .into_owned();
                            (
                                existing_display_name,
                                request.display_name.clone(),
                                location,
                            )
                        })
                } else {
                    None
                };
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    if let Some((existing_display_name, new_display_name, target_location)) =
                        replacement
                    {
                        state.request_save_replace(
                            request.name,
                            submission.destination,
                            existing_display_name,
                            new_display_name,
                            target_location,
                        );
                    } else {
                        state.set_footer_message(error.to_string());
                    }
                }
            }
        }
    }

    async fn set_workflow_save_name_from_runtime(&mut self, task_id: &str) {
        let Some(config) = self.local_config.as_ref() else {
            return;
        };
        let result = match config.background_tasks.workflow_handle(task_id).await {
            Some(handle) => handle.output().await.map(|output| output.metadata.name),
            None => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message("Workflow is no longer available");
                }
                return;
            }
        };
        match result {
            Ok(name) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_save_name(name);
                }
            }
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
            }
        }
    }

    pub(super) async fn handle_cancel_input(&mut self) -> Result<bool> {
        if self.tui.chrome().shell_running() {
            self.cancel_shell_command().await?;
            return Ok(false);
        }
        if self.tui.chrome().shell_mode_active() && self.tui.chrome().prompt().text.is_empty() {
            self.tui.chrome_mut().exit_shell_mode();
            return Ok(false);
        }
        if self.reject_pending_approval() {
            self.drain_deferred_approval_response().await;
            return Ok(false);
        }
        if self.cancel_focused_overlay() {
            return Ok(false);
        }
        if self.tui.chrome().has_btw_panel() {
            if !self.tui.chrome().prompt().text.is_empty() {
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Clear);
                return Ok(false);
            }
            self.cancel_btw_sidecar();
            self.tui.chrome_mut().close_btw_panel();
            return Ok(false);
        }
        if self.interrupt_active_or_stale_turn().await? {
            return Ok(false);
        }
        let _ = self.cancel_mcp_startup().await;
        Ok(false)
    }

    pub(super) async fn handle_interrupt_input(&mut self) -> Result<bool> {
        if self.tui.chrome().shell_running() {
            self.cancel_shell_command().await?;
            return Ok(false);
        }
        if self.reject_all_pending_approvals() {
            if self.active_turn.is_some() {
                self.cancel_active_turn().await?;
                self.show_notice("Interrupted");
            }
            return Ok(false);
        }
        if self.interrupt_active_or_stale_turn().await? {
            return Ok(false);
        }
        if self.cancel_mcp_startup().await {
            return Ok(false);
        }
        Ok(self.handle_app_clear())
    }

    /// Detach any running foreground delegate agent to background mode.
    /// Returns `true` if a delegate was detached, `false` if there's nothing
    /// to detach (so the caller can fall through to other Ctrl+B handling).
    pub(super) async fn detach_foreground_delegate(&mut self) -> Result<bool> {
        let Some(config) = self.local_config.as_ref() else {
            return Ok(false);
        };
        let agents = config.multi_agent.list_agents(false);
        let foreground_agent = agents.into_iter().find(|agent| {
            agent.mode == neo_agent_core::multi_agent::AgentRunMode::Foreground
                && agent.state == neo_agent_core::multi_agent::AgentLifecycleState::Running
        });

        let Some(agent) = foreground_agent else {
            return Ok(false);
        };

        let agent_id = agent.id.clone();
        let detached = config
            .multi_agent
            .detach_agent(&agent_id)
            .expect("agent existed a moment ago");

        config.background_tasks.start_delegate(detached).await;

        self.push_status("Moved to background. Use /tasks to view.");
        Ok(true)
    }

    pub(super) async fn handle_keybinding_key(&mut self, key: &KeyId) -> Result<bool> {
        if self.tui.chrome().shell_running() && key.as_str() == "ctrl+b" {
            self.detach_shell_command().await?;
            return Ok(false);
        }
        if key.as_str() == "ctrl+b" && self.detach_foreground_delegate().await? {
            return Ok(false);
        }
        let actions = self.keybindings.matching_actions(key);
        for action in self.keybinding_priority() {
            if *action == KeybindingAction::TranscriptCopySelection
                && !self.tui.transcript().has_transcript_selection()
            {
                continue;
            }
            if actions.contains(action) {
                return self.handle_keybinding_action(*action).await;
            }
        }

        Ok(false)
    }

    pub(super) fn dialog_input_event(&self, event: InputEvent) -> InputEvent {
        let InputEvent::Key(key) = event else {
            return event;
        };
        let actions = self.keybindings.matching_actions(&key);
        self.keybinding_priority()
            .iter()
            .copied()
            .find(|action| actions.contains(action))
            .map_or(InputEvent::Key(key), InputEvent::Action)
    }

    pub(super) fn keybinding_priority(&self) -> &'static [KeybindingAction] {
        if self.tui.chrome().question_dialog_is_focused() {
            QUESTION_ACTION_PRIORITY
        } else if self
            .tui
            .chrome()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)))
        {
            PROMPT_COMPLETION_ACTION_PRIORITY
        } else if self.tui.chrome().approval_is_pending()
            || self.tui.chrome().focused_overlay_id().is_some()
        {
            OVERLAY_ACTION_PRIORITY
        } else {
            EDITING_ACTION_PRIORITY
        }
    }

    pub(super) async fn handle_keybinding_action(
        &mut self,
        action: KeybindingAction,
    ) -> Result<bool> {
        if action == KeybindingAction::PasteImage {
            self.handle_paste_image().await?;
            return Ok(false);
        }
        if self.handle_prompt_keybinding_action(action) {
            return Ok(false);
        }
        if self.handle_transcript_keybinding_action(action) {
            return Ok(false);
        }

        if let Some(exit) = self.handle_basic_keybinding_action(action).await? {
            return Ok(exit);
        }
        if self.handle_overlay_keybinding_action(action).await? {
            return Ok(false);
        }
        unreachable!("prompt edit actions are handled before overlay actions")
    }

    pub(super) async fn handle_basic_keybinding_action(
        &mut self,
        action: KeybindingAction,
    ) -> Result<Option<bool>> {
        match action {
            KeybindingAction::InputNewLine => self.apply_prompt_edit(PromptEdit::Insert("\n")),
            KeybindingAction::InputTab => self.complete_prompt_or_insert_tab(),
            KeybindingAction::InputCopy => self.copy_prompt_to_clipboard(),
            KeybindingAction::PromptSteer => {
                return self.handle_prompt_steer().await.map(|()| Some(false));
            }
            KeybindingAction::EditNextQueuedMessage => {
                if let Some(text) = self
                    .tui
                    .chrome_mut()
                    .pending_input_mut()
                    .pop_most_recent_shell_command_for_edit()
                {
                    self.tui.chrome_mut().enter_shell_mode();
                    self.tui.chrome_mut().prompt_mut().set_text(text);
                } else {
                    self.dequeue_follow_up_into_prompt_for_edit();
                }
            }
            KeybindingAction::AppClear => return self.handle_app_clear_action().await.map(Some),
            KeybindingAction::AppExit => return Ok(Some(self.handle_app_exit())),
            KeybindingAction::AppSuspend => {
                self.suspend_requested = true;
            }
            KeybindingAction::PromptCompletionToggle => self.toggle_slash_prompt_completion(),
            KeybindingAction::CommandPaletteOpen => self.open_command_palette(),
            KeybindingAction::SessionPickerOpen => {
                self.open_session_picker();
            }
            KeybindingAction::SessionPickerToggleScope => {
                self.toggle_session_picker_scope();
            }
            KeybindingAction::SessionFork => {
                if self.tui.chrome_mut().selected_session().is_some() {
                    self.fork_selected_session().await?;
                } else {
                    self.fork_current_session().await?;
                }
            }
            KeybindingAction::ToolOutputToggle => {
                self.transcript_mut().toggle_tool_output_expanded();
            }
            KeybindingAction::TodoPanelToggle => {
                if self.tui.chrome().todo_panel_has_overflow() {
                    self.clear_pending_exit_confirmation();
                    self.tui.chrome_mut().toggle_todo_panel_expanded();
                }
            }
            KeybindingAction::ModelPickerOpen => {
                self.open_model_picker();
            }
            KeybindingAction::TogglePlanMode => {
                let currently_active = self.tui.chrome_mut().is_plan_mode();
                self.set_plan_mode_from_user(!currently_active);
            }
            KeybindingAction::CycleDevelopmentMode => self.cycle_development_mode(),
            _ => return Ok(None),
        }
        Ok(Some(false))
    }

    pub(super) async fn handle_overlay_keybinding_action(
        &mut self,
        action: KeybindingAction,
    ) -> Result<bool> {
        match action {
            KeybindingAction::InputSubmit => {
                self.clear_pending_exit_confirmation();
                self.follow_transcript_tail();
                self.submit_current_prompt().await?;
            }
            KeybindingAction::SelectUp => {
                self.tui.chrome_mut().move_overlay_selection_up();
                self.sync_inline_approval_selection();
            }
            KeybindingAction::SelectDown => {
                self.tui.chrome_mut().move_overlay_selection_down();
                self.sync_inline_approval_selection();
            }
            KeybindingAction::SelectPageUp => {
                self.tui.chrome_mut().move_overlay_selection_page_up();
            }
            KeybindingAction::SelectPageDown => {
                self.tui.chrome_mut().move_overlay_selection_page_down();
            }
            KeybindingAction::SelectConfirm => self.handle_select_confirm_action().await?,
            KeybindingAction::SelectCancel => {
                let _ = self.handle_cancel_input().await?;
            }
            KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown => {
                unreachable!("prompt history actions are handled before overlay actions")
            }
            KeybindingAction::EditorPageUp => self.transcript_mut().scroll_transcript_up(8),
            KeybindingAction::EditorPageDown => self.transcript_mut().scroll_transcript_down(8),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(super) async fn handle_app_clear_action(&mut self) -> Result<bool> {
        if self.interrupt_active_or_stale_turn().await? {
            return Ok(false);
        }
        Ok(self.handle_app_clear())
    }

    pub(super) async fn handle_select_confirm_action(&mut self) -> Result<()> {
        if self.tui.chrome_mut().selected_command().is_some() {
            self.run_selected_command().await?;
        } else if self.tui.chrome().approval_is_pending() {
            if let Some(response) = self.tui.chrome_mut().confirm_or_edit_selected_approval() {
                self.resolve_approval_response(response).await;
            } else {
                self.sync_inline_approval_selection();
            }
        } else if self.tui.chrome_mut().selected_session().is_some() {
            self.load_selected_session().await?;
        } else if self.tui.chrome_mut().selected_prompt_completion().is_some() {
            let _ = self.confirm_prompt_completion_or_file_reference();
        } else if self.tui.chrome_mut().focused_overlay_id().is_none() {
            self.submit_current_prompt().await?;
        }
        Ok(())
    }

    pub(super) fn handle_prompt_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        let prompt = self.tui.chrome().prompt();
        let prompt_empty = prompt.text.is_empty();
        let in_history_navigation = prompt.in_history_navigation();

        if prompt_empty {
            if self.tui.chrome().has_btw_panel() {
                match action {
                    KeybindingAction::EditorCursorUp => {
                        self.clear_pending_exit_confirmation();
                        self.tui.chrome_mut().scroll_btw_panel_up(1);
                        return true;
                    }
                    KeybindingAction::EditorCursorDown => {
                        self.clear_pending_exit_confirmation();
                        self.tui.chrome_mut().scroll_btw_panel_down(1);
                        return true;
                    }
                    _ => {}
                }
            }
            if self.handle_prompt_history_action(action) {
                return true;
            }
        } else if !in_history_navigation
            && matches!(
                action,
                KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown
            )
        {
            // Normal editing mode: ↑/↓ move the cursor through wrapped lines.
            self.clear_pending_exit_confirmation();
            let body_width = Self::prompt_body_width();
            let edit = if action == KeybindingAction::EditorCursorUp {
                PromptEdit::MoveUp(body_width)
            } else {
                PromptEdit::MoveDown(body_width)
            };
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit_with_width(edit, body_width);
            self.sync_inline_prompt_completion();
            return true;
        } else if in_history_navigation
            && matches!(
                action,
                KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown
            )
        {
            // History navigation mode: keep recalling entries even though the
            // composer now shows a non-empty historical prompt.
            if self.handle_prompt_history_action(action) {
                return true;
            }
        }
        let Some(edit) = prompt_edit_for_action(action) else {
            return false;
        };
        self.clear_pending_exit_confirmation();
        let body_width = Self::prompt_body_width();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit_with_width(edit, body_width);
        self.sync_inline_prompt_completion();
        true
    }

    pub(super) fn handle_prompt_history_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::EditorCursorUp => {
                if !self.tui.chrome_mut().prompt_mut().recall_previous_history() {
                    // No history to recall; pull the most recent queued
                    // shell command or follow-up back into the composer for
                    // editing instead.
                    if let Some(text) = self
                        .tui
                        .chrome_mut()
                        .pending_input_mut()
                        .pop_most_recent_shell_command_for_edit()
                    {
                        self.tui.chrome_mut().enter_shell_mode();
                        self.tui.chrome_mut().prompt_mut().set_text(text);
                    } else {
                        self.dequeue_follow_up_into_prompt_for_edit();
                    }
                }
            }
            KeybindingAction::EditorCursorDown => {
                self.tui.chrome_mut().prompt_mut().recall_next_history();
            }
            _ => return false,
        }
        self.clear_pending_exit_confirmation();
        self.sync_inline_prompt_completion();
        true
    }

    fn dequeue_follow_up_into_prompt_for_edit(&mut self) {
        let has_active_turn = self.active_turn.is_some();
        let text = {
            let pending_input = self.tui.chrome_mut().pending_input_mut();
            if has_active_turn {
                pending_input.dequeue_oldest_follow_up_for_edit_optimistic()
            } else {
                pending_input.dequeue_oldest_follow_up_for_edit()
            }
        };
        let Some(text) = text else {
            return;
        };
        if let Some(turn) = &self.active_turn {
            turn.steer_input
                .push(neo_agent_core::ActiveTurnInput::DequeueFollowUpForEdit);
        }
        self.tui.chrome_mut().exit_shell_mode();
        let prompt = self.tui.chrome_mut().prompt_mut();
        if prompt.text.is_empty() {
            prompt.set_text(text);
            return;
        }
        let mut next = prompt.text.clone();
        if !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&text);
        prompt.set_text(next);
    }

    pub(super) fn handle_transcript_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::TranscriptSelectionStart => {
                self.transcript_mut().select_visible_transcript_entry();
            }
            KeybindingAction::TranscriptSelectionClear => {
                self.transcript_mut().clear_transcript_selection();
            }
            KeybindingAction::TranscriptSelectionExtendUp => {
                self.transcript_mut().extend_transcript_selection_up(1);
            }
            KeybindingAction::TranscriptSelectionExtendDown => {
                self.transcript_mut().extend_transcript_selection_down(1);
            }
            KeybindingAction::TranscriptSelectionExtendPageUp => {
                self.transcript_mut().extend_transcript_selection_up(8);
            }
            KeybindingAction::TranscriptSelectionExtendPageDown => {
                self.transcript_mut().extend_transcript_selection_down(8);
            }
            KeybindingAction::TranscriptCopySelection => {
                self.copy_transcript_selection_to_clipboard();
            }
            _ => return false,
        }
        true
    }

    pub(super) fn handle_app_clear(&mut self) -> bool {
        if self.tui.transcript().has_transcript_selection() {
            self.copy_transcript_selection_to_clipboard();
            self.clear_pending_exit_confirmation();
            return false;
        }
        if !self.tui.chrome_mut().prompt().text.is_empty() {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Clear);
        }
        self.handle_exit_confirmation(ExitGesture::CtrlC)
    }

    pub(super) fn handle_app_exit(&mut self) -> bool {
        if self.tui.chrome_mut().prompt().text.is_empty() {
            return self.handle_exit_confirmation(ExitGesture::CtrlD);
        }
        self.clear_pending_exit_confirmation();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit(PromptEdit::Delete);
        false
    }

    pub(super) fn handle_exit_confirmation(&mut self, gesture: ExitGesture) -> bool {
        if self
            .pending_exit_confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.matches(gesture))
        {
            self.pending_exit_confirmation = None;
            self.tui.chrome_mut().set_exit_confirmation_label(None);
            return true;
        }
        let message = match gesture {
            ExitGesture::CtrlC => "Press Ctrl-C again to exit",
            ExitGesture::CtrlD => "Press Ctrl-D again to exit",
        };
        self.tui
            .chrome_mut()
            .set_exit_confirmation_label(Some(message.to_owned()));
        self.pending_exit_confirmation = Some(ExitConfirmation::new(gesture));
        false
    }
}
