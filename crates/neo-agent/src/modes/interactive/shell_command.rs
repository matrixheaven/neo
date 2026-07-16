use super::*;

pub(super) type BoxedShellFuture = Pin<
    Box<
        dyn Future<Output = Result<neo_agent_core::tools::ShellExecutionResult, ShellDriverError>>
            + Send
            + 'static,
    >,
>;
pub(super) type ShellDriver = Arc<dyn Fn(ShellRunRequest) -> BoxedShellFuture + Send + Sync>;

pub(super) struct ShellRunRequest {
    pub(super) id: String,
    pub(super) command: String,
    pub(super) cwd: PathBuf,
    pub(super) foreground_timeout: Duration,
    pub(super) background_timeout: Duration,
    pub(super) max_output_bytes: usize,
    pub(super) cancel_token: CancellationToken,
    pub(super) background_tasks: neo_agent_core::tools::BackgroundTaskManager,
    pub(super) shell_runtime: neo_agent_core::tools::ShellRuntime,
    pub(super) event_tx: mpsc::UnboundedSender<AgentEvent>,
}

#[derive(Debug)]
pub(super) enum ShellDriverError {
    Tool(neo_agent_core::ToolError),
    Other(anyhow::Error),
}

impl std::fmt::Display for ShellDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(err) => write!(f, "{err}"),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ShellDriverError {}

impl From<neo_agent_core::ToolError> for ShellDriverError {
    fn from(value: neo_agent_core::ToolError) -> Self {
        Self::Tool(value)
    }
}

impl From<anyhow::Error> for ShellDriverError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}

pub(super) struct RunningShellCommand {
    pub(super) id: String,
    pub(super) command: String,
    pub(super) task:
        JoinHandle<Result<neo_agent_core::tools::ShellExecutionResult, ShellDriverError>>,
    pub(super) cancel_token: CancellationToken,
    pub(super) background_tasks: neo_agent_core::tools::BackgroundTaskManager,
    pub(super) foreground_task_id: Option<String>,
    pub(super) max_output_bytes: usize,
    pub(super) events: mpsc::UnboundedReceiver<AgentEvent>,
}

pub(super) async fn current_shell_foreground_task_id(
    manager: &neo_agent_core::tools::BackgroundTaskManager,
) -> Option<String> {
    manager.foreground_bash_task_id().await
}

impl InteractiveController {
    pub(super) async fn submit_shell_command(&mut self, prompt: String) -> Result<()> {
        let command = prompt.trim().to_owned();
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        if command.is_empty() {
            return Ok(());
        }
        if self.active_turn.is_some() || self.active_shell_command.is_some() {
            self.tui
                .chrome_mut()
                .pending_input_mut()
                .queue_shell_command(command);
            return Ok(());
        }
        self.start_shell_command(command).await
    }

    #[allow(clippy::unused_async)] // sync API kept for future awaiter hookups; signature stays async.
    pub(super) async fn start_shell_command(&mut self, command: String) -> Result<()> {
        let id = self.next_shell_id();
        let cancel_token = CancellationToken::new();
        let background_tasks = self
            .local_config
            .as_ref()
            .map(|config| config.background_tasks.clone())
            .unwrap_or_default();
        let shell_runtime = self
            .local_config
            .as_ref()
            .map(|config| config.runtime.shell_runtime.clone())
            .unwrap_or_default();
        let shell_limits = shell_runtime.limits();
        let (event_tx, events) = mpsc::unbounded_channel();
        let request = ShellRunRequest {
            id: id.clone(),
            command: command.clone(),
            cwd: self.workspace_root.clone(),
            foreground_timeout: Duration::from_secs(shell_limits.foreground_timeout_secs),
            background_timeout: Duration::from_secs(shell_limits.background_timeout_secs),
            max_output_bytes: shell_limits.max_output_bytes,
            cancel_token: cancel_token.clone(),
            background_tasks: background_tasks.clone(),
            shell_runtime,
            event_tx,
        };
        let task = tokio::spawn((self.shell_driver)(request));
        self.tui.chrome_mut().set_shell_running(true);
        self.apply_turn_event(AgentEvent::ShellCommandStarted {
            turn: 0,
            id: id.clone(),
            command: command.clone(),
            cwd: self.workspace_root.clone(),
            origin: ShellCommandOrigin::UserShellMode,
        });
        self.active_shell_command = Some(RunningShellCommand {
            id,
            command,
            task,
            cancel_token,
            background_tasks,
            foreground_task_id: None,
            max_output_bytes: shell_limits.max_output_bytes,
            events,
        });
        Ok(())
    }

    pub(super) fn next_shell_id(&mut self) -> String {
        let id = format!("shell-{}", self.next_shell_command_id);
        self.next_shell_command_id = self.next_shell_command_id.saturating_add(1);
        id
    }

    pub(super) async fn drain_active_shell_command(&mut self) -> Result<()> {
        let (events, is_finished) = {
            let Some(shell) = self.active_shell_command.as_mut() else {
                return Ok(());
            };
            let mut events = Vec::new();
            while let Ok(event) = shell.events.try_recv() {
                events.push(event);
            }
            if shell.foreground_task_id.is_none()
                && let Some(task_id) =
                    current_shell_foreground_task_id(&shell.background_tasks).await
            {
                shell.foreground_task_id = Some(task_id);
            }
            (events, shell.task.is_finished())
        };
        for event in events {
            self.apply_turn_event(event);
        }
        if !is_finished {
            return Ok(());
        }
        let shell = self
            .active_shell_command
            .take()
            .expect("active shell was checked");
        let result = shell
            .task
            .await
            .map_err(|error| anyhow::anyhow!("interactive shell task failed: {error}"))?;
        let result = match result {
            Ok(result) => result,
            Err(ShellDriverError::Tool(neo_agent_core::ToolError::ResourceLimited { cause })) => {
                neo_agent_core::tools::ShellExecutionResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    signal: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                    truncated: false,
                    outcome: ShellCommandOutcome::ResourceLimited,
                    foreground_task_id: None,
                    resource_limit: Some(neo_agent_core::ResourceLimitDetail {
                        cause,
                        configured: None,
                        observed: None,
                    }),
                }
            }
            Err(error) => neo_agent_core::tools::ShellExecutionResult {
                stdout: String::new(),
                stderr: error.to_string(),
                exit_code: None,
                signal: None,
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                outcome: ShellCommandOutcome::Cancelled,
                foreground_task_id: None,
                resource_limit: None,
            },
        };
        self.finish_shell_command(shell.id, shell.command, result)
            .await?;
        self.start_next_queued_after_shell().await
    }

    #[cfg(test)]
    pub(super) async fn wait_for_active_shell_command(&mut self) -> Result<()> {
        let initial_id = self
            .active_shell_command
            .as_ref()
            .map(|shell| shell.id.clone());
        loop {
            self.drain_active_shell_command().await?;
            let current_id = self
                .active_shell_command
                .as_ref()
                .map(|shell| shell.id.clone());
            if current_id.is_none() || current_id != initial_id {
                tokio::task::yield_now().await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    pub(super) async fn finish_shell_command(
        &mut self,
        id: String,
        command: String,
        mut result: neo_agent_core::tools::ShellExecutionResult,
    ) -> Result<()> {
        if let Some(limit) = &result.resource_limit {
            if !result.stderr.is_empty() && !result.stderr.ends_with('\n') {
                result.stderr.push('\n');
            }
            result.stderr.push_str("Resource limit: ");
            result.stderr.push_str(limit.cause.as_str());
            if let (Some(observed), Some(configured)) = (limit.observed, limit.configured) {
                let _ = write!(result.stderr, " (observed {observed}, limit {configured})");
            }
            result.stderr.push('.');
        }
        self.apply_turn_event(AgentEvent::ShellCommandFinished {
            turn: 0,
            id,
            exit_code: result.exit_code,
            signal: result.signal,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            truncated: result.truncated,
            origin: ShellCommandOrigin::UserShellMode,
            outcome: result.outcome.clone(),
        });
        let message = AgentMessage::shell_command(
            command,
            result.stdout,
            result.stderr,
            result.exit_code,
            result.outcome,
            result.truncated,
        );
        let event = AgentEvent::MessageAppended { message };
        self.persist_shell_event(&event).await?;
        self.apply_turn_event(event);
        Ok(())
    }

    pub(super) async fn persist_shell_event(&mut self, event: &AgentEvent) -> Result<()> {
        let Some(config) = self.local_config.clone() else {
            return Ok(());
        };
        let session_path = self.ensure_shell_session_path(&config).await?;
        let mut writer = neo_agent_core::session::JsonlSessionWriter::open_append(&session_path)
            .await
            .with_context(|| format!("failed to append session {}", session_path.display()))?;
        writer.append_event(event).await?;
        writer.flush().await?;
        Ok(())
    }

    pub(super) async fn ensure_shell_session_path(
        &mut self,
        config: &AppConfig,
    ) -> Result<PathBuf> {
        if let Some(session_id) = self.active_session_id.as_deref() {
            return crate::modes::sessions::session_path(session_id, config);
        }
        let session_path = create_interactive_session_path(config).await?;
        let session_id = session_id_from_wire_path(&session_path)?;
        self.set_active_session_id(session_id.clone());
        let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
            .await
            .with_context(|| format!("failed to create session {}", session_path.display()))?;
        writer.flush().await?;
        let _ = neo_agent_core::session::SessionMetadataStore::new(workspace_sessions_dir(config))
            .record_activity(
                &session_id,
                Some(config.project_dir.display().to_string()),
                Some("shell command".to_owned()),
                current_unix_timestamp(),
            );
        Ok(session_path)
    }

    pub(super) async fn start_next_queued_after_shell(&mut self) -> Result<()> {
        if let Some(command) = self
            .tui
            .chrome_mut()
            .pending_input_mut()
            .drain_next_shell_command()
        {
            return self.start_shell_command(command).await;
        }
        self.tui.chrome_mut().set_shell_running(false);
        self.tui
            .chrome_mut()
            .apply_stream_update(StreamUpdate::TurnFinished);
        if self.active_turn.is_none()
            && let Some(prompt) = self
                .tui
                .chrome_mut()
                .pending_input_mut()
                .drain_next_follow_up()
        {
            let prompt = resolve_submitted_prompt(
                prompt,
                self.local_config.as_ref(),
                &self.completion_root,
            )?;
            let content = crate::prompt::parts::expand_prompt_markers(
                &prompt,
                &self.paste_store,
                &self.image_attachment_store,
                &self.file_reference_store,
                &self.completion_root,
            );
            self.append_prompt_history(&content_to_display_text(&content));
            self.start_turn_with_prompt(content);
        }
        Ok(())
    }

    pub(super) async fn cancel_shell_command(&mut self) -> Result<()> {
        let Some(shell) = self.active_shell_command.as_ref() else {
            return Ok(());
        };
        shell.cancel_token.cancel();
        self.wait_for_shell_cancel_or_abort().await
    }

    pub(super) async fn detach_shell_command(&mut self) -> Result<()> {
        let Some(shell) = self.active_shell_command.as_ref() else {
            return Ok(());
        };
        let task_id = if let Some(task_id) = shell.foreground_task_id.clone() {
            Some(task_id)
        } else {
            current_shell_foreground_task_id(&shell.background_tasks).await
        };
        let Some(task_id) = task_id else {
            self.push_status("Shell command is not ready to background yet");
            return Ok(());
        };
        shell.background_tasks.detach(&task_id).await?;
        self.wait_for_shell_detach_or_abort(task_id).await
    }

    pub(super) async fn wait_for_shell_cancel_or_abort(&mut self) -> Result<()> {
        for _ in 0..200 {
            self.drain_active_shell_command().await?;
            if self.active_shell_command.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(shell) = self.active_shell_command.take() {
            let foreground_task_id = match shell.foreground_task_id {
                Some(task_id) => Some(task_id),
                None => current_shell_foreground_task_id(&shell.background_tasks).await,
            };
            let tasks = shell.background_tasks.list(true, 50).await;
            for task in tasks
                .into_iter()
                .filter(|task| foreground_task_id.as_deref() == Some(task.task_id.as_str()))
            {
                let _ = shell
                    .background_tasks
                    .stop(
                        &task.task_id,
                        "Cancelled foreground shell command",
                        shell.max_output_bytes,
                    )
                    .await;
            }
            shell.task.abort();
            self.tui.chrome_mut().set_shell_running(false);
            self.tui
                .chrome_mut()
                .apply_stream_update(StreamUpdate::TurnFinished);
            let result = neo_agent_core::tools::ShellExecutionResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                signal: None,
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                outcome: ShellCommandOutcome::Cancelled,
                foreground_task_id,
                resource_limit: None,
            };
            self.finish_shell_command(shell.id, shell.command, result)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn wait_for_shell_detach_or_abort(&mut self, task_id: String) -> Result<()> {
        for _ in 0..200 {
            self.drain_active_shell_command().await?;
            if self.active_shell_command.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(shell) = self.active_shell_command.take() {
            let snapshot = shell.background_tasks.snapshot(&task_id).await.ok();
            let output = snapshot.and_then(|snapshot| snapshot.output).unwrap_or(
                neo_agent_core::tools::CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    signal: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                    resource_limit: None,
                },
            );
            shell.task.abort();
            self.tui.chrome_mut().set_shell_running(false);
            self.tui
                .chrome_mut()
                .apply_stream_update(StreamUpdate::TurnFinished);
            let result = neo_agent_core::tools::ShellExecutionResult {
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: output.exit_code,
                signal: output.signal,
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                outcome: ShellCommandOutcome::Backgrounded {
                    task_id: task_id.into(),
                },
                foreground_task_id: shell.foreground_task_id,
                resource_limit: None,
            };
            self.finish_shell_command(shell.id, shell.command, result)
                .await?;
        }
        Ok(())
    }
}
