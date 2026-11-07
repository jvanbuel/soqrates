//! The examiner's logic, independent of how the model is reached.
//!
//! [`Llm`] is the seam: [`crate::RigExaminer`] implements it with Rig, the
//! tests with canned replies.

use std::future::Future;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use soqrates_core::{
    Answer, AnswerBody, Concept, Diff, Error, Grade, Question, QuestionBody, QuestionFormat,
    QuestionId, Result,
};

use crate::output::{self, ConceptsOutput, GradeOutput, MultipleChoiceOutput, OpenOutput};
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

    pub async fn write_question(
        &self,
        concept: &Concept,
        format: QuestionFormat,
    ) -> Result<Question> {
        let body = match format {
            QuestionFormat::MultipleChoice => {
                let mc = self
                    .structured(
                        "a multiple-choice question",
                        prompts::WRITE_MULTIPLE_CHOICE_SYSTEM,
                        prompts::write_multiple_choice(concept),
                        |out: MultipleChoiceOutput| output::validate_multiple_choice(out),
                    )
                    .await?;
                let mc = output::shuffle(mc, &concept.id.0);
                QuestionBody::MultipleChoice {
                    prompt: mc.prompt,
                    options: mc.options,
                    correct: mc.correct,
                }
            }
            QuestionFormat::Open => {
                let (prompt, rubric) = self
                    .structured(
                        "an open question",
                        prompts::WRITE_OPEN_SYSTEM,
                        prompts::write_open(concept),
                        |out: OpenOutput| output::validate_open(out),
                    )
                    .await?;
                QuestionBody::Open { prompt, rubric }
            }
        };
        Ok(Question {
            id: question_id(concept, format),
            concept: concept.id.clone(),
            scope: concept.scope,
            body,
        })
    }

    pub async fn grade(&self, question: &Question, answer: &Answer) -> Result<Grade> {
        match (&question.body, &answer.body) {
            (QuestionBody::MultipleChoice { correct, .. }, AnswerBody::Choice(choice)) => {
                Ok(grade_choice(*choice, *correct))
            }
            (
                QuestionBody::MultipleChoice {
                    options, correct, ..
                },
                AnswerBody::Text(text),
            ) => Ok(match output::parse_letter(text) {
                Some(choice) if choice < options.len() => grade_choice(choice, *correct),
                _ => Grade::Again,
            }),
            (QuestionBody::Open { prompt, rubric }, AnswerBody::Text(text)) => {
                if text.trim().is_empty() {
                    return Ok(Grade::Again);
                }
                self.structured(
                    "grading",
                    prompts::GRADE_OPEN_SYSTEM,
                    prompts::grade_open(prompt, rubric, text),
                    |out: GradeOutput| output::validate_grade(out),
                )
                .await
            }
            (QuestionBody::Open { .. }, AnswerBody::Choice(_)) => Ok(Grade::Again),
        }
    }
}

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
