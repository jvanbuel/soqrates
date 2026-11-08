//! The examiner's logic, independent of how the model is reached.
//!
//! [`Llm`] is the seam: [`crate::RigExaminer`] implements it with Rig, the
//! tests with canned replies.
//!
//! Each step may first read the codebase in a bounded loop (ADR-0008 D2):
//! the model calls `read_file` and `list_dir`, the examiner answers, and
//! the model then gives its structured answer in a turn that may not call
//! tools. Retries replay what was read; they don't read again.

use std::future::Future;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use soqrates_core::ports::Codebase;
use soqrates_core::{
    Answer, AnswerBody, Concept, Diff, Error, Grade, Grading, Question, QuestionBody,
    QuestionBrief, QuestionFormat, QuestionId, Result, code_path,
};

use crate::output::{
    self, ConceptsOutput, GradeOutput, MultipleChoice, MultipleChoiceOutput, OpenOutput,
    OptionCheckOutput,
};
use crate::prompts;

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON schema of the arguments.
    pub parameters: serde_json::Value,
}

/// A tool call the model made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolCall {
    /// The provider's id for the call, echoed in its result.
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The answer to one tool call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolOutput {
    pub call: ToolCall,
    pub text: String,
    /// The call was refused or failed; `text` says why.
    pub is_error: bool,
}

/// One turn after the task prompt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Turn {
    Assistant { text: String, calls: Vec<ToolCall> },
    ToolResults(Vec<ToolOutput>),
    User(String),
}

/// One request to the model: a system prompt, the task prompt, the turns
/// that followed it, the tools, and the JSON schema the reply must satisfy
/// if it is the answer.
#[derive(Debug, Clone)]
pub(crate) struct LlmRequest {
    pub system: String,
    pub user: String,
    pub turns: Vec<Turn>,
    /// Sent whenever the conversation used tools: providers want the
    /// definitions of tools called earlier in it.
    pub tools: Vec<ToolSpec>,
    /// Whether the model may call `tools` in this turn.
    pub may_call_tools: bool,
    /// Set for an answer, which may not call tools.
    pub schema: Option<schemars::Schema>,
}

impl LlmRequest {
    /// A request for a `schema` answer to `user`, without tools.
    fn answer(system: &str, user: String, schema: schemars::Schema) -> Self {
        LlmRequest {
            system: system.to_owned(),
            user,
            turns: Vec::new(),
            tools: Vec::new(),
            may_call_tools: false,
            schema: Some(schema),
        }
    }
}

/// The model's reply: its text and the tools it called.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LlmReply {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl LlmReply {
    #[cfg(test)]
    pub fn text(text: impl Into<String>) -> Self {
        LlmReply {
            text: text.into(),
            calls: Vec::new(),
        }
    }
}

/// Sends a request and returns the model's reply.
pub(crate) trait Llm: Send + Sync {
    fn complete(&self, request: LlmRequest) -> impl Future<Output = Result<LlmReply>> + Send;
}

/// How many times a reply that fails validation is asked for again.
const RETRIES: usize = 1;

/// Bounds of the reading loop of one step (ADR-0008 D2).
pub(crate) const MAX_TOOL_TURNS: usize = 6;
pub(crate) const MAX_TOOL_CALLS: usize = 12;
pub(crate) const MAX_FILE_BYTES: usize = 20_000;
pub(crate) const MAX_TOOL_BYTES: usize = 80_000;

/// What one step has read so far.
#[derive(Default)]
struct Budget {
    calls: usize,
    bytes: usize,
}

impl Budget {
    fn exhausted(&self) -> bool {
        self.calls >= MAX_TOOL_CALLS || self.bytes >= MAX_TOOL_BYTES
    }
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    prompts::tools()
        .into_iter()
        .map(|(name, description, parameters)| ToolSpec {
            name,
            description,
            parameters,
        })
        .collect()
}

/// `text` cut to at most `max` bytes at a char boundary, preferring a line
/// boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    match head.rfind('\n') {
        Some(newline) if newline > 0 => &head[..=newline],
        _ => head,
    }
}

/// Answer `call` from `code`, within `budget`.
async fn run_tool(call: &ToolCall, code: &impl Codebase, budget: &mut Budget) -> ToolOutput {
    let refuse = |text: String| ToolOutput {
        call: call.clone(),
        text,
        is_error: true,
    };
    if budget.exhausted() {
        return refuse(
            "The reading limit is reached. Do not call more tools; answer from what you have read."
                .into(),
        );
    }
    budget.calls += 1;
    let raw = call.arguments.get("path").and_then(|p| p.as_str());
    let Some(path) = raw.and_then(code_path) else {
        return refuse(format!(
            "Invalid path {:?}: give a path relative to the repository root, without `..`.",
            raw.unwrap_or_default()
        ));
    };
    let room = MAX_TOOL_BYTES.saturating_sub(budget.bytes);
    let text = match call.name.as_str() {
        prompts::READ_FILE => match code.read_file(&path).await {
            Ok(Some(content)) => {
                let shown = cut(&content, MAX_FILE_BYTES.min(room));
                prompts::file_block(&path, shown, content.len(), shown.len() < content.len())
            }
            Ok(None) => return refuse(format!("There is no text file at {path:?}.")),
            Err(e) => return refuse(format!("Reading {path:?} failed: {e}")),
        },
        prompts::LIST_DIR => match code.list_dir(&path).await {
            Ok(Some(entries)) => {
                let block = prompts::listing_block(&path, &entries);
                if block.len() > room {
                    cut(&block, room).to_owned()
                } else {
                    block
                }
            }
            Ok(None) => return refuse(format!("There is no directory at {path:?}.")),
            Err(e) => return refuse(format!("Listing {path:?} failed: {e}")),
        },
        other => return refuse(format!("There is no tool {other:?}.")),
    };
    budget.bytes += text.len();
    ToolOutput {
        call: call.clone(),
        text,
        is_error: false,
    }
}

/// What a step read before answering: the system prompt to answer with,
/// and the turns to replay in every attempt (ADR-0008 D2, D6).
pub(crate) struct Reading {
    system: String,
    turns: Vec<Turn>,
}

impl Reading {
    /// A request for a `schema` answer to `user`, after what was read.
    fn answer(&self, user: String, schema: schemars::Schema) -> LlmRequest {
        LlmRequest {
            system: self.system.clone(),
            user,
            turns: self.turns.clone(),
            // Only when the conversation called tools.
            tools: if self.turns.is_empty() {
                Vec::new()
            } else {
                tool_specs()
            },
            may_call_tools: false,
            schema: Some(schema),
        }
    }
}

pub(crate) struct Core<L> {
    pub llm: L,
}

impl<L: Llm> Core<L> {
    /// Let the model read `code` for the task `user`, if there is any
    /// code. Without a codebase, nothing is sent and the step answers as
    /// it did before it could read (ADR-0008 D5).
    async fn read(&self, system: &str, user: &str, code: &impl Codebase) -> Result<Reading> {
        if !code.is_available() {
            return Ok(Reading {
                system: system.to_owned(),
                turns: Vec::new(),
            });
        }
        let system = prompts::with_tools(system);
        let mut turns = Vec::new();
        let mut budget = Budget::default();
        for _ in 0..MAX_TOOL_TURNS {
            let reply = self
                .llm
                .complete(LlmRequest {
                    system: system.clone(),
                    user: user.to_owned(),
                    turns: turns.clone(),
                    tools: tool_specs(),
                    may_call_tools: true,
                    schema: None,
                })
                .await?;
            if reply.calls.is_empty() {
                // Done reading. Its text, if any, is not the answer: that
                // comes next, with the schema.
                break;
            }
            let mut outputs = Vec::with_capacity(reply.calls.len());
            for call in &reply.calls {
                outputs.push(run_tool(call, code, &mut budget).await);
            }
            turns.push(Turn::Assistant {
                text: reply.text,
                calls: reply.calls,
            });
            turns.push(Turn::ToolResults(outputs));
        }
        if !turns.is_empty() {
            turns.push(Turn::User(prompts::FINISH.to_owned()));
        }
        Ok(Reading { system, turns })
    }

    /// Let the model read `code`, then ask for a `T`, validate it with
    /// `validate`, and retry once with the problem explained if the reply
    /// is unusable.
    async fn structured<T, U>(
        &self,
        what: &str,
        system: &'static str,
        user: String,
        code: &impl Codebase,
        validate: impl Fn(T) -> std::result::Result<U, String> + Sync,
    ) -> Result<U>
    where
        T: DeserializeOwned + JsonSchema,
    {
        let schema = schemars::schema_for!(T);
        let reading = self.read(system, &user, code).await?;
        self.retrying(what, user, |prompt| {
            self.attempt(&reading, prompt, &schema, &validate)
        })
        .await
    }

    /// Run `attempt` with `user`, and once more with the problem explained
    /// if it gives an unusable result (`Ok(Err(problem))`). Errors of the
    /// model itself are not retried.
    async fn retrying<U, F, Fut>(&self, what: &str, user: String, mut attempt: F) -> Result<U>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<std::result::Result<U, String>>>,
    {
        let mut prompt = user.clone();
        let mut problem = String::new();
        for _ in 0..=RETRIES {
            match attempt(prompt).await? {
                Ok(value) => return Ok(value),
                Err(why) => problem = why,
            }
            prompt = prompts::retry(&user, &problem);
        }
        Err(Error::Port(format!(
            "examiner: the model gave invalid output for {what} after {} attempts: {problem}",
            RETRIES + 1
        )))
    }

    /// One request for a `T` after `reading`, parsed and validated:
    /// `Ok(Err(problem))` if the reply is unusable.
    async fn attempt<T, U>(
        &self,
        reading: &Reading,
        prompt: String,
        schema: &schemars::Schema,
        validate: &(impl Fn(T) -> std::result::Result<U, String> + Sync),
    ) -> Result<std::result::Result<U, String>>
    where
        T: DeserializeOwned,
    {
        let raw = self
            .llm
            .complete(reading.answer(prompt, schema.clone()))
            .await?
            .text;
        Ok(output::parse_json::<T>(&raw).and_then(validate))
    }

    /// Write a multiple-choice question whose reasons discuss only the
    /// options it offers (issue #5). A question that passes validation is
    /// checked with a second, short prompt; one that fails the check counts
    /// as invalid output, so it is asked for again once, with the passages
    /// quoted, and fails like any other invalid output after that.
    ///
    /// The question is written as `brief` asks (ADR-0007), and the check
    /// is shown its code too (D7).
    async fn multiple_choice(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> Result<MultipleChoice> {
        let schema = schemars::schema_for!(MultipleChoiceOutput);
        let schema = &schema;
        let user = prompts::write_multiple_choice(concept, brief);
        let reading = self
            .read(prompts::WRITE_MULTIPLE_CHOICE_SYSTEM, &user, code)
            .await?;
        let reading = &reading;
        self.retrying("a multiple-choice question", user, |prompt| async move {
            let mc = match self
                .attempt(reading, prompt, schema, &|out| {
                    output::validate_multiple_choice(out, brief)
                })
                .await?
            {
                Ok(mc) => mc,
                Err(why) => return Ok(Err(why)),
            };
            Ok(match self.unlisted_options(&mc).await? {
                None => Ok(mc),
                Some(why) => Err(why),
            })
        })
        .await
    }

    /// Ask the model whether `mc`'s reasons discuss answers it doesn't
    /// offer: the problem if they do. An unusable reply from the check
    /// keeps the question; the check is a safeguard on output that has
    /// already passed validation, not a reason to lose it.
    async fn unlisted_options(&self, mc: &MultipleChoice) -> Result<Option<String>> {
        let raw = self
            .llm
            .complete(LlmRequest::answer(
                prompts::CHECK_OPTIONS_SYSTEM,
                prompts::check_options(
                    &mc.prompt,
                    mc.code.as_ref(),
                    &mc.options,
                    mc.options_language.as_deref(),
                    &mc.reasons,
                    mc.correct,
                ),
                schemars::schema_for!(OptionCheckOutput),
            ))
            .await?
            .text;
        Ok(output::parse_json::<OptionCheckOutput>(&raw)
            .ok()
            .and_then(output::unlisted_option_problem))
    }

    pub async fn extract_concepts(
        &self,
        diff: &Diff,
        code: &impl Codebase,
    ) -> Result<Vec<Concept>> {
        if diff.is_empty() {
            return Ok(Vec::new());
        }
        let files = diff.files();
        self.structured(
            "concept extraction",
            prompts::EXTRACT_CONCEPTS_SYSTEM,
            prompts::extract_concepts(&diff.0),
            code,
            |out: ConceptsOutput| output::validate_concepts(out, &files),
        )
        .await
    }

    /// Write a question as `brief` asks (ADR-0007): its format, a kind it
    /// offers, and code in its own field, copied from a hunk for
    /// `explain_your_change`.
    pub async fn write_question(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> Result<Question> {
        let format = brief.format;
        let (kind, body, explanation) = match format {
            QuestionFormat::MultipleChoice => {
                // One reason per option, written with the options and
                // checked to discuss only those (issue #5).
                let mc = self.multiple_choice(concept, brief, code).await?;
                // The reasons name options by content, so they survive the
                // shuffle unchanged.
                let mc = output::shuffle(mc, &concept.id.0);
                let explanation = mc.explanation().to_owned();
                let body = QuestionBody::MultipleChoice {
                    prompt: mc.prompt,
                    code: mc.code,
                    options: mc.options,
                    options_language: mc.options_language,
                    correct: mc.correct,
                    reasons: mc.reasons,
                };
                (mc.kind, body, explanation)
            }
            QuestionFormat::Open => {
                let open = self
                    .structured(
                        "an open question",
                        prompts::WRITE_OPEN_SYSTEM,
                        prompts::write_open(concept, brief),
                        code,
                        |out: OpenOutput| output::validate_open(out, brief),
                    )
                    .await?;
                let body = QuestionBody::Open {
                    prompt: open.prompt,
                    code: open.code,
                    rubric: open.rubric,
                };
                (open.kind, body, open.explanation)
            }
        };
        Ok(Question {
            id: question_id(concept, format),
            concept: concept.id.clone(),
            scope: concept.scope,
            kind,
            body,
            explanation,
        })
    }

    /// Grade `answer`. Multiple choice is graded without the model and with
    /// empty feedback, so that the engine shows the picked option's reason
    /// and the question's explanation (the presenters show the correct
    /// option). An open answer is graded by
    /// the model, whose rationale becomes the feedback. The question's
    /// explanation is never sent to the model.
    ///
    /// The grader may read `code` to check what the answer says about it
    /// (ADR-0008 D2).
    pub async fn grade(
        &self,
        question: &Question,
        answer: &Answer,
        code: &impl Codebase,
    ) -> Result<Grading> {
        let choice = |grade| Grading {
            grade,
            feedback: String::new(),
        };
        match (&question.body, &answer.body) {
            (QuestionBody::MultipleChoice { correct, .. }, AnswerBody::Choice(picked)) => {
                Ok(choice(grade_choice(*picked, *correct)))
            }
            (
                QuestionBody::MultipleChoice {
                    options, correct, ..
                },
                AnswerBody::Text(text),
            ) => Ok(choice(match output::parse_letter(text) {
                Some(picked) if picked < options.len() => grade_choice(picked, *correct),
                _ => Grade::Again,
            })),
            (
                QuestionBody::Open {
                    prompt,
                    code: snippet,
                    rubric,
                },
                AnswerBody::Text(text),
            ) => {
                if text.trim().is_empty() {
                    return Ok(Grading {
                        grade: Grade::Again,
                        feedback: NO_ANSWER_FEEDBACK.to_owned(),
                    });
                }
                self.structured(
                    "grading",
                    prompts::GRADE_OPEN_SYSTEM,
                    prompts::grade_open(prompt, snippet.as_ref(), rubric, text),
                    code,
                    |out: GradeOutput| output::validate_grade(out),
                )
                .await
            }
            (QuestionBody::Open { .. }, AnswerBody::Choice(_)) => Ok(Grading {
                grade: Grade::Again,
                feedback: CHOICE_FOR_OPEN_FEEDBACK.to_owned(),
            }),
        }
    }
}

/// Feedback for an empty answer to an open question.
pub(crate) const NO_ANSWER_FEEDBACK: &str = "No answer was given.";

/// Feedback for an option picked in answer to an open question.
pub(crate) const CHOICE_FOR_OPEN_FEEDBACK: &str = "This question needs a written answer.";

/// `"{concept_id}/{mc|open}"`.
pub(crate) fn question_id(concept: &Concept, format: QuestionFormat) -> QuestionId {
    let suffix = match format {
        QuestionFormat::MultipleChoice => "mc",
        QuestionFormat::Open => "open",
    };
    QuestionId(format!("{}/{suffix}", concept.id.0))
}

fn grade_choice(choice: usize, correct: usize) -> Grade {
    if choice == correct {
        Grade::Good
    } else {
        Grade::Again
    }
}
