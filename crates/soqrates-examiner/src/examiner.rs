//! The examiner's logic, independent of how the model is reached.
//!
//! [`Llm`] is the seam: [`crate::RigExaminer`] implements it with Rig, the
//! tests with canned replies.

use std::future::Future;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use soqrates_core::{
    Answer, AnswerBody, Concept, Diff, Error, Grade, Grading, Question, QuestionBody,
    QuestionBrief, QuestionFormat, QuestionId, Result,
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

/// Sends a request and returns the model's raw text reply.
pub(crate) trait Llm: Send + Sync {
    fn complete(&self, request: LlmRequest) -> impl Future<Output = Result<String>> + Send;
}

/// How many times a reply that fails validation is asked for again.
const RETRIES: usize = 1;

pub(crate) struct Core<L> {
    pub llm: L,
}

impl<L: Llm> Core<L> {
    /// Ask for a `T`, validate it with `validate`, and retry once with the
    /// problem explained if the reply is unusable.
    async fn structured<T, U>(
        &self,
        what: &str,
        system: &'static str,
        user: String,
        validate: impl Fn(T) -> std::result::Result<U, String> + Sync,
    ) -> Result<U>
    where
        T: DeserializeOwned + JsonSchema,
    {
        let schema = schemars::schema_for!(T);
        self.retrying(what, user, |prompt| {
            self.attempt(system, prompt, &schema, &validate)
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

    /// One request for a `T`, parsed and validated: `Ok(Err(problem))` if
    /// the reply is unusable.
    async fn attempt<T, U>(
        &self,
        system: &'static str,
        prompt: String,
        schema: &schemars::Schema,
        validate: &(impl Fn(T) -> std::result::Result<U, String> + Sync),
    ) -> Result<std::result::Result<U, String>>
    where
        T: DeserializeOwned,
    {
        let raw = self
            .llm
            .complete(LlmRequest {
                system,
                user: prompt,
                schema: schema.clone(),
            })
            .await?;
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
    ) -> Result<MultipleChoice> {
        let schema = schemars::schema_for!(MultipleChoiceOutput);
        let schema = &schema;
        self.retrying(
            "a multiple-choice question",
            prompts::write_multiple_choice(concept, brief),
            |prompt| async move {
                let mc = match self
                    .attempt(
                        prompts::WRITE_MULTIPLE_CHOICE_SYSTEM,
                        prompt,
                        schema,
                        &|out| output::validate_multiple_choice(out, brief),
                    )
                    .await?
                {
                    Ok(mc) => mc,
                    Err(why) => return Ok(Err(why)),
                };
                Ok(match self.unlisted_options(&mc).await? {
                    None => Ok(mc),
                    Some(why) => Err(why),
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
        let raw = self
            .llm
            .complete(LlmRequest {
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
            })
            .await?;
        Ok(output::parse_json::<OptionCheckOutput>(&raw)
            .ok()
            .and_then(output::unlisted_option_problem))
    }

    pub async fn extract_concepts(&self, diff: &Diff) -> Result<Vec<Concept>> {
        if diff.is_empty() {
            return Ok(Vec::new());
        }
        let files = diff.files();
        self.structured(
            "concept extraction",
            prompts::EXTRACT_CONCEPTS_SYSTEM,
            prompts::extract_concepts(&diff.0),
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
    ) -> Result<Question> {
        let format = brief.format;
        let (kind, body, explanation) = match format {
            QuestionFormat::MultipleChoice => {
                // One reason per option, written with the options and
                // checked to discuss only those (issue #5).
                let mc = self.multiple_choice(concept, brief).await?;
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
    pub async fn grade(&self, question: &Question, answer: &Answer) -> Result<Grading> {
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
                    code,
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
                    prompts::grade_open(prompt, code.as_ref(), rubric, text),
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
