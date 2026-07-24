use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::{Tool, ToolContext, ToolResult};
use crate::{QuestionEventData, QuestionOptionData};

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// A question's input schema as the model sees it.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserInput {
    /// 1–4 questions to ask the user.
    #[schemars(description = "The questions to ask the user (1-4 questions).")]
    pub questions: Vec<AskUserQuestionInput>,
    /// If true, ask the question as a background task and return immediately.
    #[serde(default)]
    #[schemars(
        description = "Set true to ask in the background and return immediately with a background task_id. Use TaskOutput to read the answer later."
    )]
    pub background: bool,
}

/// A single question in the model-facing input schema.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserQuestionInput {
    /// The question text. Must end with `?`.
    #[schemars(description = "A specific, actionable question. End with '?'.")]
    pub question: String,
    /// Optional short header (max ~12 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional short category tag (max ~12 chars, e.g. 'Auth', 'Style').")]
    pub header: Option<String>,
    /// Optional longer body / context.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional longer body or context for the question.")]
    pub body: Option<String>,
    /// 2–4 options the user can choose from.
    #[schemars(
        description = "2-4 meaningful, distinct options. Do NOT include an 'Other' option - the system adds one automatically."
    )]
    pub options: Vec<AskUserOptionInput>,
    /// Whether the user may select multiple options.
    #[serde(default)]
    #[schemars(description = "Whether the user can select multiple options. Defaults to false.")]
    pub multi_select: bool,
}

/// A single option in the model-facing input schema.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserOptionInput {
    /// Short label shown as the choice.
    #[schemars(
        description = "Concise display text (1-5 words). If recommended, append '(Recommended)'."
    )]
    pub label: String,
    /// Optional description explaining the option.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Optional brief explanation of trade-offs or implications.")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Channel types (not serialisable — runtime only)
// ---------------------------------------------------------------------------

/// A pending question sent from [`AskUserTool`] through the channel to the
/// host (TUI / CLI layer).
///
/// The host answers by sending a [`QuestionResponse`] through the
/// `response_tx` oneshot channel.
pub struct PendingQuestion {
    /// Unique identifier for this question batch.
    pub id: String,
    /// The questions to present to the user.
    pub questions: Vec<QuestionEventData>,
    /// Channel to receive the user's answers.
    pub response_tx: oneshot::Sender<QuestionResponse>,
}

/// The user's answers to a [`PendingQuestion`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionResponse {
    /// One answer per question, in the same order as `questions`.
    /// Each answer is the selected option label(s) or a custom typed answer.
    pub answers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// Tool that asks the user structured questions via a reverse-RPC channel.
///
/// The host (TUI / CLI) creates an `mpsc::unbounded_channel::<PendingQuestion>()`,
/// holds the receiver, and passes the sender into [`AskUserTool::new`].
///
/// **Registration:** `AskUserTool` is **not** registered in
/// [`ToolRegistry::with_builtin_tools()`] because it requires a channel sender.
/// Callers that want the tool must register it explicitly:
///
/// ```ignore
/// let (tx, rx) = mpsc::unbounded_channel::<PendingQuestion>();
/// let mut registry = ToolRegistry::with_builtin_tools();
/// registry.register(AskUserTool::new(tx));
/// ```
pub struct AskUserTool {
    question_tx: Arc<mpsc::UnboundedSender<PendingQuestion>>,
}

impl AskUserTool {
    /// Create a new `AskUserTool` that sends questions through `question_tx`.
    #[must_use]
    pub fn new(question_tx: mpsc::UnboundedSender<PendingQuestion>) -> Self {
        Self {
            question_tx: Arc::new(question_tx),
        }
    }
}

/// Validates the input against the model-facing contract: 1-4 questions,
/// 2-4 options per question, non-empty labels.
fn validate_ask_user_input(input: &AskUserInput) -> Result<(), super::ToolError> {
    const MAX_QUESTIONS: usize = 4;
    const MIN_OPTIONS: usize = 2;
    const MAX_OPTIONS: usize = 4;

    if input.questions.is_empty() || input.questions.len() > MAX_QUESTIONS {
        return Err(super::ToolError::InvalidInput {
            tool: "AskUserQuestion".to_owned(),
            message: format!(
                "questions must contain 1 to {MAX_QUESTIONS} items, got {}",
                input.questions.len()
            ),
        });
    }
    for question in &input.questions {
        if question.options.len() < MIN_OPTIONS || question.options.len() > MAX_OPTIONS {
            return Err(super::ToolError::InvalidInput {
                tool: "AskUserQuestion".to_owned(),
                message: format!(
                    "each question must have {MIN_OPTIONS} to {MAX_OPTIONS} options, got {}",
                    question.options.len()
                ),
            });
        }
        for option in &question.options {
            if option.label.trim().is_empty() {
                return Err(super::ToolError::InvalidInput {
                    tool: "AskUserQuestion".to_owned(),
                    message: "option labels must not be empty".to_owned(),
                });
            }
        }
    }
    Ok(())
}

impl Tool for AskUserTool {
    fn name(&self) -> &'static str {
        "AskUserQuestion"
    }

    fn description(&self) -> &'static str {
        "Use this tool when you need to ask the user questions with structured options during execution. This allows you to:\n\
         1. Collect user preferences or requirements before proceeding.\n\
         2. Resolve ambiguous or underspecified instructions.\n\
         3. Let the user decide between implementation approaches as you work.\n\
         4. Present concrete options when multiple valid directions exist.\n\n\
         When NOT to use:\n\
         - When you can infer the answer from context — be decisive and proceed.\n\
         - Trivial decisions that do not materially affect the outcome.\n\n\
         Usage notes:\n\
         - Users always have an \"Other\" option for custom input — do not create one yourself.\n\
         - Use `multi_select` to allow multiple answers to be selected for a question.\n\
         - Keep option labels concise (1-5 words), use descriptions for trade-offs and details.\n\
         - Each question should have 2-4 meaningful, distinct options.\n\
         - You can ask 1-4 questions at a time; group related questions to minimize interruptions.\n\
         - If you recommend a specific option, list it first and append \"(Recommended)\" to its label.\n\
         - Set `background=true` when you can keep working without the answer. This starts a background question task and returns a task_id immediately."
    }

    fn input_schema(&self) -> serde_json::Value {
        super::schema::<AskUserInput>()
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> super::ToolFuture<'a> {
        Box::pin(async move {
            if !ctx.access.user_question {
                return Ok(ToolResult::error(
                    "AskUserQuestion is disabled while auto permission mode is active",
                ));
            }
            let input: AskUserInput = super::parse_input(self.name(), input)?;
            validate_ask_user_input(&input)?;

            // Convert model-facing input to event data.
            let questions = question_event_data(&input);

            let id = Uuid::new_v4().to_string();
            let (response_tx, response_rx) = oneshot::channel::<QuestionResponse>();
            let id = if input.background {
                format!("question-{id}")
            } else {
                id
            };

            if input.background {
                let description = questions
                    .first()
                    .and_then(|question| question.header.clone())
                    .unwrap_or_else(|| {
                        questions.first().map_or_else(
                            || "Question".to_owned(),
                            |question| question.question.clone(),
                        )
                    });
                let result = ctx
                    .background_tasks
                    .start_question(id.clone(), description)
                    .await;
                let manager = ctx.background_tasks.clone();
                let task_id = id.clone();
                self.question_tx
                    .send(PendingQuestion {
                        id,
                        questions,
                        response_tx,
                    })
                    .map_err(|_| super::ToolError::InvalidInput {
                        tool: "AskUserQuestion".to_owned(),
                        message: "question channel closed".to_owned(),
                    })?;
                tokio::spawn(async move {
                    if let Ok(response) = response_rx.await {
                        manager.complete_question(&task_id, response.answers).await;
                    }
                });
                return Ok(result);
            }

            // Send the pending question through the channel.
            self.question_tx
                .send(PendingQuestion {
                    id: id.clone(),
                    questions,
                    response_tx,
                })
                .map_err(|_| super::ToolError::InvalidInput {
                    tool: "AskUserQuestion".to_owned(),
                    message: "question channel closed".to_owned(),
                })?;

            // Wait for the response or cancellation.
            let response = tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => {
                    return Ok(ToolResult::error("Question cancelled"));
                }
                result = response_rx => {
                    match result {
                        Ok(resp) => resp,
                        Err(_) => return Ok(ToolResult::error("Question cancelled (channel dropped)")),
                    }
                }
            };

            // Format answers for the model.
            let answers = response.answers;
            let formatted = format_answers(&answers);

            Ok(ToolResult::ok(formatted).with_details(json!({
                "answers": answers,
                "question_id": id,
            })))
        })
    }
}

fn question_event_data(input: &AskUserInput) -> Vec<QuestionEventData> {
    input
        .questions
        .iter()
        .map(|question| QuestionEventData {
            question: question.question.clone(),
            header: question.header.clone(),
            body: question.body.clone(),
            options: question
                .options
                .iter()
                .map(|option| QuestionOptionData {
                    label: option.label.clone(),
                    description: option.description.clone(),
                })
                .collect(),
            multi_select: question.multi_select,
        })
        .collect()
}

fn format_answers(answers: &[String]) -> String {
    if let [answer] = answers {
        return answer.clone();
    }
    answers
        .iter()
        .enumerate()
        .map(|(index, answer)| format!("{}. {answer}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolAccess, ToolContext};
    use serde_json::json;
    use tokio::sync::mpsc;

    fn make_ctx() -> ToolContext {
        let dir = tempfile::tempdir().unwrap();
        ToolContext::new(dir.path())
            .unwrap()
            .with_access(ToolAccess::all())
    }

    #[test]
    fn tool_name_and_description() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        assert_eq!(tool.name(), "AskUserQuestion");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn ask_user_receives_response() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let input = json!({
            "questions": [{
                "question": "Which framework?",
                "header": "Framework",
                "options": [
                    { "label": "React", "description": "UI library" },
                    { "label": "Vue", "description": "Progressive framework" }
                ],
                "multi_select": false
            }]
        });

        // Spawn a responder that answers the first question.
        tokio::spawn(async move {
            let pending = rx.recv().await.expect("should receive question");
            assert_eq!(pending.questions.len(), 1);
            assert_eq!(pending.questions[0].question, "Which framework?");
            assert_eq!(pending.questions[0].options.len(), 2);
            let _ = pending.response_tx.send(QuestionResponse {
                answers: vec!["React".to_owned()],
            });
        });

        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(!result.is_error);
        assert_eq!(result.content, "React");
        let details = result.details.expect("details");
        assert_eq!(details["question_id"].as_str().unwrap().len(), 36); // UUID
    }

    #[tokio::test]
    async fn ask_user_multiple_questions() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let input = json!({
            "questions": [
                {
                    "question": "Dark or light?",
                    "options": [{ "label": "Dark" }, { "label": "Light" }],
                    "multi_select": false
                },
                {
                    "question": "Tabs or spaces?",
                    "options": [{ "label": "Tabs" }, { "label": "Spaces" }],
                    "multi_select": false
                }
            ]
        });

        tokio::spawn(async move {
            let pending = rx.recv().await.expect("should receive");
            assert_eq!(pending.questions.len(), 2);
            let _ = pending.response_tx.send(QuestionResponse {
                answers: vec!["Dark".to_owned(), "Spaces".to_owned()],
            });
        });

        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("1. Dark"));
        assert!(result.content.contains("2. Spaces"));
    }

    #[tokio::test]
    async fn ask_user_channel_closed_returns_error() {
        let (tx, rx) = mpsc::unbounded_channel::<PendingQuestion>();
        drop(rx); // Close the receiver.
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let input = json!({
            "questions": [{
                "question": "Test?",
                "options": [{ "label": "A" }, { "label": "B" }],
                "multi_select": false
            }]
        });

        let result = tool.execute(&ctx, input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ask_user_response_dropped_returns_cancelled() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let input = json!({
            "questions": [{
                "question": "Test?",
                "options": [{ "label": "A" }, { "label": "B" }],
                "multi_select": false
            }]
        });

        // Drop the response sender without answering.
        tokio::spawn(async move {
            let pending = rx.recv().await.expect("should receive");
            drop(pending.response_tx);
        });

        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(result.is_error);
        assert!(result.content.contains("cancelled"));
    }

    #[tokio::test]
    async fn ask_user_invalid_input() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool.execute(&ctx, json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ask_user_rejects_too_many_questions() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let questions: Vec<_> = (0..5)
            .map(|i| {
                json!({
                    "question": format!("Question {i}?"),
                    "options": [{"label": "A"}, {"label": "B"}]
                })
            })
            .collect();
        let result = tool.execute(&ctx, json!({"questions": questions})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ask_user_rejects_too_few_options() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                json!({
                    "questions": [{
                        "question": "Only one option?",
                        "options": [{"label": "A"}]
                    }]
                }),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ask_user_rejects_empty_option_label() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                json!({
                    "questions": [{
                        "question": "Bad option?",
                        "options": [{"label": ""}, {"label": "B"}]
                    }]
                }),
            )
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn schema_has_questions_array() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let schema = tool.input_schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .unwrap();
        assert!(props.contains_key("questions"));
    }

    #[test]
    fn schema_has_background_flag() {
        let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let schema = tool.input_schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .unwrap();
        assert!(props.contains_key("background"));
    }

    #[tokio::test]
    async fn ask_user_background_returns_task_without_waiting() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                json!({
                    "background": true,
                    "questions": [{
                        "question": "Where should config live?",
                        "header": "Config",
                        "options": [{ "label": "Project" }, { "label": "User" }],
                        "multi_select": false
                    }]
                }),
            )
            .await
            .expect("background question should start");

        assert!(!result.is_error);
        let details = result.details.expect("details");
        let task_id = details["task_id"].as_str().expect("task id");
        assert!(task_id.starts_with("question-"));
        assert_eq!(details["kind"], "question");
        assert_eq!(details["status"], "waiting_for_user");
        assert_eq!(details["automatic_notification"], true);

        let pending = rx.try_recv().expect("question should be visible to host");
        assert_eq!(pending.id, task_id);
        assert_eq!(pending.questions[0].question, "Where should config live?");
    }

    #[tokio::test]
    async fn ask_user_background_answer_is_visible_through_task_output() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                json!({
                    "background": true,
                    "questions": [{
                        "question": "Where should config live?",
                        "options": [{ "label": "Project" }, { "label": "User" }],
                        "multi_select": false
                    }]
                }),
            )
            .await
            .expect("background question should start");
        let task_id = result.details.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let pending = rx.recv().await.expect("pending question");
        pending
            .response_tx
            .send(QuestionResponse {
                answers: vec!["Project".to_owned()],
            })
            .expect("send response");
        for _ in 0..20 {
            let output = ctx
                .background_tasks
                .output(
                    &task_id,
                    false,
                    std::time::Duration::from_secs(0),
                    ctx.max_output_bytes,
                )
                .await
                .expect("TaskOutput result");
            if output.details.as_ref().unwrap()["status"] == "completed" {
                let details = output.details.unwrap();
                assert_eq!(details["kind"], "question");
                assert_eq!(details["answers"], json!(["Project"]));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("background question should complete");
    }

    #[tokio::test]
    async fn ask_user_background_stopped_question_ignores_late_answer() {
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let tool = AskUserTool::new(tx);
        let ctx = make_ctx();

        let result = tool
            .execute(
                &ctx,
                json!({
                    "background": true,
                    "questions": [{
                        "question": "Continue?",
                        "options": [{ "label": "Yes" }, { "label": "No" }],
                        "multi_select": false
                    }]
                }),
            )
            .await
            .expect("background question should start");
        let task_id = result.details.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pending = rx.recv().await.expect("pending question");

        ctx.background_tasks
            .stop(&task_id, "no longer needed", ctx.max_output_bytes)
            .await
            .expect("TaskStop should stop question");
        pending
            .response_tx
            .send(QuestionResponse {
                answers: vec!["Yes".to_owned()],
            })
            .expect("late response can still be sent");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let output = ctx
            .background_tasks
            .output(
                &task_id,
                false,
                std::time::Duration::from_secs(0),
                ctx.max_output_bytes,
            )
            .await
            .expect("TaskOutput result");
        let details = output.details.unwrap();
        assert_eq!(details["status"], "cancelled");
        assert!(details.get("answers").is_none());
    }
}
