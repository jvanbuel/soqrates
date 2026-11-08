//! The examiner's logic, independent of how the model is reached.
//!
//! [`Llm`] is the seam: [`crate::RigExaminer`] implements it with Rig, the
//! tests with canned replies.
//!
//! Each step may first read the codebase (ADR-0008 D2). The reading loop
//! is Rig's agent runtime (`crate::agent`); what the tools may read, how
//! much, and how it is shown to the model is decided here, in [`serve`].
//! A retry continues the step's transcript, so it doesn't read again.

use std::future::Future;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use soqrates_core::ports::{Codebase, NoCodebase};
use soqrates_core::{
    Answer, AnswerBody, Concept, Diff, Error, Grade, Grading, Question, QuestionBody,
    QuestionBrief, QuestionFormat, QuestionId, Result, code_path,
};

use crate::output::{
    self, ConceptsOutput, GradeOutput, MultipleChoice, MultipleChoiceOutput, OpenOutput,
    OptionCheckOutput,
};
use crate::prompts;

/// One request to the model: a system prompt, a user prompt and the JSON
/// schema the reply must satisfy.
#[derive(Debug, Clone)]
pub(crate) struct LlmRequest {
    pub system: &'static str,
    pub user: String,
    pub schema: schemars::Schema,
}

/// The model's answer, and the transcript a retry continues from.
pub(crate) struct LlmAnswer<T> {
    pub text: String,
    pub transcript: T,
}

/// Sends a request and returns the model's raw text reply. With a codebase
/// that is available, the model may first read it (ADR-0008 D2); after a
/// non-empty `transcript` it continues that conversation and reads no more.
pub(crate) trait Llm: Send + Sync {
    /// What a retry continues from. Empty for a first attempt.
    type Transcript: Clone + Default + Send + Sync;

    fn complete(
        &self,
        request: LlmRequest,
        transcript: Self::Transcript,
        code: &impl Codebase,
    ) -> impl Future<Output = Result<LlmAnswer<Self::Transcript>>> + Send;
}

/// How many times a reply that fails validation is asked for again.
const RETRIES: usize = 1;

/// Bounds of the reading of one step (ADR-0008 D2).
pub(crate) const MAX_TOOL_TURNS: usize = 6;
pub(crate) const MAX_TOOL_CALLS: usize = 12;
pub(crate) const MAX_FILE_BYTES: usize = 20_000;
pub(crate) const MAX_TOOL_BYTES: usize = 80_000;

/// What one step has read so far.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    calls: usize,
    bytes: usize,
}

impl Budget {
    pub fn exhausted(&self) -> bool {
        self.calls >= MAX_TOOL_CALLS || self.bytes >= MAX_TOOL_BYTES
    }
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

/// Answer one call of the tool `tool` with the `path` argument from
/// `code`, within `budget`: what the model sees, or why the call was
/// refused (ADR-0008 D1-D3).
pub(crate) async fn serve(
    tool: &str,
    path: Option<&str>,
    code: &impl Codebase,
    budget: &mut Budget,
) -> std::result::Result<String, String> {
    if budget.exhausted() {
        return Err(
            "The reading limit is reached. Do not call more tools; answer from what you have read."
                .into(),
        );
    }
    budget.calls += 1;
    let Some(clean) = path.and_then(code_path) else {
        return Err(format!(
            "Invalid path {:?}: give a path relative to the repository root, without `..`.",
            path.unwrap_or_default()
        ));
    };
    let room = MAX_TOOL_BYTES.saturating_sub(budget.bytes);
    let text = match tool {
        prompts::READ_FILE => match code.read_file(&clean).await {
            Ok(Some(content)) => {
                let shown = cut(&content, MAX_FILE_BYTES.min(room));
                prompts::file_block(&clean, shown, content.len(), shown.len() < content.len())
            }
            Ok(None) => return Err(format!("There is no text file at {clean:?}.")),
            Err(e) => return Err(format!("Reading {clean:?} failed: {e}")),
        },
        prompts::LIST_DIR => match code.list_dir(&clean).await {
            Ok(Some(entries)) => {
                let block = prompts::listing_block(&clean, &entries);
                if block.len() > room {
                    cut(&block, room).to_owned()
                } else {
                    block
                }
            }
            Ok(None) => return Err(format!("There is no directory at {clean:?}.")),
            Err(e) => return Err(format!("Listing {clean:?} failed: {e}")),
        },
        other => return Err(format!("There is no tool {other:?}.")),
    };
    budget.bytes += text.len();
    Ok(text)
}

pub(crate) struct Core<L> {
    pub llm: L,
}

impl<L: Llm> Core<L> {
    /// Ask for a `T`, letting the model read `code` first, validate it with
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
        self.retrying(what, user, |prompt, transcript| {
            self.attempt(system, prompt, transcript, &schema, code, &validate)
        })
        .await
    }

    /// Run `attempt` with `user`, and once more with the problem explained
    /// if it gives an unusable result (`Ok(Err(problem))`), continuing the
    /// transcript of the first attempt. Errors of the model itself are not
    /// retried.
    async fn retrying<U, F, Fut>(&self, what: &str, user: String, mut attempt: F) -> Result<U>
    where
        F: FnMut(String, L::Transcript) -> Fut,
        Fut: Future<Output = Result<(std::result::Result<U, String>, L::Transcript)>>,
    {
        let mut prompt = user.clone();
        let mut transcript = L::Transcript::default();
        let mut problem = String::new();
        for _ in 0..=RETRIES {
            let (result, after) = attempt(prompt, transcript).await?;
            match result {
                Ok(value) => return Ok(value),
                Err(why) => problem = why,
            }
            transcript = after;
            prompt = prompts::retry(&user, &problem);
        }
        Err(Error::Port(format!(
            "examiner: the model gave invalid output for {what} after {} attempts: {problem}",
            RETRIES + 1
        )))
    }

    /// One request for a `T`, parsed and validated: `Ok(Err(problem))` if
    /// the reply is unusable, with the transcript to continue from.
    async fn attempt<T, U>(
        &self,
        system: &'static str,
        prompt: String,
        transcript: L::Transcript,
        schema: &schemars::Schema,
        code: &impl Codebase,
        validate: &(impl Fn(T) -> std::result::Result<U, String> + Sync),
    ) -> Result<(std::result::Result<U, String>, L::Transcript)>
    where
        T: DeserializeOwned,
    {
        let answer = self
            .llm
            .complete(
                LlmRequest {
                    system,
                    user: prompt,
                    schema: schema.clone(),
                },
                transcript,
                code,
            )
            .await?;
        Ok((
            output::parse_json::<T>(&answer.text).and_then(validate),
            answer.transcript,
        ))
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
        self.retrying(
            "a multiple-choice question",
            prompts::write_multiple_choice(concept, brief),
            |prompt, transcript| async move {
                let (mc, after) = self
                    .attempt(
                        prompts::WRITE_MULTIPLE_CHOICE_SYSTEM,
                        prompt,
                        transcript,
                        schema,
                        code,
                        &|out| output::validate_multiple_choice(out, brief),
                    )
                    .await?;
                let mc = match mc {
                    Ok(mc) => mc,
                    Err(why) => return Ok((Err(why), after)),
                };
                Ok(match self.unlisted_options(&mc).await? {
                    None => (Ok(mc), after),
                    Some(why) => (Err(why), after),
                })
            },
        )
        .await
    }

    /// Ask the model whether `mc`'s reasons discuss answers it doesn't
    /// offer: the problem if they do. An unusable reply from the check
    /// keeps the question; the check is a safeguard on output that has
    /// already passed validation, not a reason to lose it.
    async fn unlisted_options(&self, mc: &MultipleChoice) -> Result<Option<String>> {
        // The check reads nothing: it is about the question's own text.
        let raw = self
            .llm
            .complete(
                LlmRequest {
                    system: prompts::CHECK_OPTIONS_SYSTEM,
                    user: prompts::check_options(
                        &mc.prompt,
                        mc.code.as_ref(),
                        &mc.options,
                        mc.options_language.as_deref(),
                        &mc.reasons,
                        mc.correct,
                    ),
                    schema: schemars::schema_for!(OptionCheckOutput),
                },
                L::Transcript::default(),
                &NoCodebase,
            )
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
