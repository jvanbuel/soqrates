//! `Examiner` on top of Rig (ADR-0001 D3). This is the only crate that may
//! depend on `rig-core`.

use soqrates_core::ports::Examiner;
use soqrates_core::{Answer, Concept, Diff, Grade, Question, QuestionFormat, Result};

/// Which model to use, e.g. `anthropic:claude-sonnet-5-5` or `ollama:qwen3`.
#[derive(Debug, Clone)]
pub struct ModelSpec(pub String);

pub struct RigExaminer {
    pub model: ModelSpec,
}

impl RigExaminer {
    pub fn new(model: ModelSpec) -> Self {
        Self { model }
    }
}

impl Examiner for RigExaminer {
    async fn extract_concepts(&self, diff: &Diff) -> Result<Vec<Concept>> {
        let _ = diff;
        todo!("ADR-0001 D3")
    }
    async fn write_question(&self, concept: &Concept, format: QuestionFormat) -> Result<Question> {
        let _ = (concept, format);
        todo!("ADR-0001 D3")
    }
    async fn grade(&self, question: &Question, answer: &Answer) -> Result<Grade> {
        let _ = (question, answer);
        todo!("ADR-0001 D3")
    }
}
