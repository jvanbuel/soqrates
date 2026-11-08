//! The examiner's logic, independent of how the model is reached.
//!
//! [`Llm`] is the seam: [`crate::RigExaminer`] implements it with Rig, the
//! tests with canned replies.

use std::future::Future;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use soqrates_core::{
    Answer, AnswerBody, Concept, Diff, Error, Grade, Grading, Question, QuestionBody,
    QuestionFormat, QuestionId, Result,
};

use crate::output::{
    self, ConceptsOutput, GradeOutput, MultipleChoice, MultipleChoiceOutput, OpenOutput,
    ReasonsCheckOutput, ReasonsOutput,
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
        validate: impl Fn(T) -> std::result::Result<U, String> + Send,
    ) -> Result<U>
    where
        T: DeserializeOwned + JsonSchema,
    {
        let schema = schemars::schema_for!(T);
        let mut prompt = user.clone();
        let mut problem = String::new();
        for _ in 0..=RETRIES {
            let raw = self
                .llm
                .complete(LlmRequest {
                    system,
                    user: prompt,
                    schema: schema.clone(),
                })
                .await?;
            match output::parse_json::<T>(&raw).and_then(&validate) {
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

    pub async fn extract_concepts(&self, diff: &Diff) -> Result<Vec<Concept>> {
        if diff.is_empty() {
            return Ok(Vec::new());
        }
        self.structured(
            "concept extraction",
            prompts::EXTRACT_CONCEPTS_SYSTEM,
            prompts::extract_concepts(&diff.0),
            |out: ConceptsOutput| output::validate_concepts(out),
        )
        .await
    }

    #[cfg(test)]
    pub async fn write_question(
        &self,
        concept: &Concept,
        format: QuestionFormat,
    ) -> Result<Question> {
        Ok(self.write_question_checked(concept, format).await?.0)
    }

    /// [`Self::write_question`], also saying how the check of a
    /// multiple-choice question's reasons went (always [`Check::Clean`] for
    /// an open question).
    pub async fn write_question_checked(
        &self,
        concept: &Concept,
        format: QuestionFormat,
    ) -> Result<(Question, Check)> {
        let (body, explanation, check) = match format {
            QuestionFormat::MultipleChoice => {
                let (mc, check) = self.write_multiple_choice(concept).await?;
                // The reasons name options by content, so they survive the
                // shuffle unchanged.
                let mc = output::shuffle(mc, &concept.id.0);
                let explanation = mc.explanation().to_owned();
                let body = QuestionBody::MultipleChoice {
                    prompt: mc.prompt,
                    options: mc.options,
                    correct: mc.correct,
                    reasons: mc.reasons,
                };
                (body, explanation, check)
            }
            QuestionFormat::Open => {
                let open = self
                    .structured(
                        "an open question",
                        prompts::WRITE_OPEN_SYSTEM,
                        prompts::write_open(concept),
                        |out: OpenOutput| output::validate_open(out),
                    )
                    .await?;
                let body = QuestionBody::Open {
                    prompt: open.prompt,
                    rubric: open.rubric,
                };
                (body, open.explanation, Check::Clean)
            }
        };
        let question = Question {
            id: question_id(concept, format),
            concept: concept.id.clone(),
            scope: concept.scope,
            body,
            explanation,
        };
        Ok((question, check))
    }

    /// Write a multiple-choice question with a reason for every option
    /// (issue #5). Because each reason belongs to an option that is shown,
    /// the question has no free-form explanation to discuss options it
    /// doesn't offer. A model call checks this anyway; flagged reasons are
    /// rewritten once, keeping the question and options.
    ///
    /// The check is a model's judgement and may be wrong, so it never fails
    /// the question: the outcome is returned, for the caller to report.
    async fn write_multiple_choice(&self, concept: &Concept) -> Result<(MultipleChoice, Check)> {
        let mc = self
            .structured(
                "a multiple-choice question",
                prompts::WRITE_MULTIPLE_CHOICE_SYSTEM,
                prompts::write_multiple_choice(concept),
                |out: MultipleChoiceOutput| output::validate_multiple_choice(out),
            )
            .await?;
        let problem = match self.review(&mc).await {
            Review::Clean => return Ok((mc, Check::Clean)),
            Review::Unusable => return Ok((mc, Check::Unchecked)),
            Review::Flagged(problem) => problem,
        };
        let repaired = self
            .structured(
                "the option reasons",
                prompts::REPAIR_REASONS_SYSTEM,
                prompts::repair_reasons(&mc.prompt, &mc.options, mc.correct, &mc.reasons, &problem),
                |out: ReasonsOutput| output::validate_reasons(out, &mc.options, mc.correct),
            )
            .await;
        let Ok(reasons) = repaired else {
            return Ok((mc, Check::NotRepaired));
        };
        let mc = MultipleChoice { reasons, ..mc };
        let check = match self.review(&mc).await {
            Review::Clean => Check::Repaired,
            Review::Unusable => Check::RepairedUnchecked,
            Review::Flagged(_) => Check::StillFlagged,
        };
        Ok((mc, check))
    }

    /// Ask the model whether `mc`'s reasons treat as an option anything the
    /// question does not offer. A failed call counts as an unusable reply:
    /// the question is already written, and the check is only a backstop.
    async fn review(&self, mc: &MultipleChoice) -> Review {
        let reply = self
            .llm
            .complete(LlmRequest {
                system: prompts::CHECK_REASONS_SYSTEM,
                user: prompts::check_reasons(&mc.prompt, &mc.options, mc.correct, &mc.reasons),
                schema: schemars::schema_for!(ReasonsCheckOutput),
            })
            .await;
        match reply
            .ok()
            .and_then(|raw| output::parse_json::<ReasonsCheckOutput>(&raw).ok())
        {
            None => Review::Unusable,
            Some(out) => match output::stray_options_problem(out) {
                None => Review::Clean,
                Some(problem) => Review::Flagged(problem),
            },
        }
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
            (QuestionBody::Open { prompt, rubric }, AnswerBody::Text(text)) => {
                if text.trim().is_empty() {
                    return Ok(Grading {
                        grade: Grade::Again,
                        feedback: NO_ANSWER_FEEDBACK.to_owned(),
                    });
                }
                self.structured(
                    "grading",
                    prompts::GRADE_OPEN_SYSTEM,
                    prompts::grade_open(prompt, rubric, text),
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

/// The outcome of the check that a multiple-choice question's reasons
/// discuss only the options it offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// Nothing was flagged.
    Clean,
    /// The check gave no usable reply; the question is kept as written.
    Unchecked,
    /// Flagged, rewritten, and the rewrite passed the check.
    Repaired,
    /// Flagged and rewritten, but the second check gave no usable reply.
    RepairedUnchecked,
    /// Flagged, rewritten, and the rewrite was flagged too; it is kept.
    StillFlagged,
    /// Flagged, but no valid rewrite came back; the original is kept.
    NotRepaired,
}

impl Check {
    /// A one-line note for the log, or `None` if there is nothing to say.
    /// It names no option or reason, so that it gives nothing away when
    /// shown before the author answers.
    pub fn note(self) -> Option<&'static str> {
        Some(match self {
            Check::Clean => return None,
            Check::Unchecked => "the check of the option reasons gave no usable reply",
            Check::Repaired => "the option reasons discussed answers not offered; rewrote them",
            Check::RepairedUnchecked => {
                "the option reasons discussed answers not offered; rewrote them, \
                 but could not check the rewrite"
            }
            Check::StillFlagged => {
                "the option reasons discussed answers not offered; rewrote them, \
                 and the rewrite was flagged too"
            }
            Check::NotRepaired => {
                "the option reasons discussed answers not offered; could not rewrite them"
            }
        })
    }
}

enum Review {
    Clean,
    Unusable,
    Flagged(String),
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
