//! `Examiner` on top of Rig (ADR-0001 D3). This is the only crate that may
//! depend on `rig-core` and `rig-agent`.
//!
//! - [`prompts`] holds every prompt the examiner sends.
//! - `output` holds the structured-output schemas and their validation.
//! - `examiner` holds the logic, behind a small `Llm` seam so that it can be
//!   tested without a model, and what the tools may read.
//! - `agent` reads the codebase with Rig's agent runtime (ADR-0008 D2).

mod agent;
mod examiner;
mod output;
pub mod prompts;

use std::fmt;
use std::str::FromStr;

use rig_core::DynModel;
use rig_core::completion::{CompletionRequest, Message};
use rig_core::operation::Completion;
use rig_core::providers::{anthropic::Anthropic, ollama::Ollama, openai::OpenAI};
use soqrates_core::ports::{Codebase, Examiner};
use soqrates_core::{Answer, Concept, Diff, Error, Grading, Question, QuestionBrief, Result};

use crate::examiner::{Core, Llm, LlmAnswer, LlmRequest};

/// The model used when none is configured.
pub const DEFAULT_MODEL: &str = "anthropic:claude-sonnet-5-5";

/// A model provider supported by the examiner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// Reads `ANTHROPIC_API_KEY` (and optionally `ANTHROPIC_BASE_URL`).
    Anthropic,
    /// Reads `OPENAI_API_KEY` (and optionally `OPENAI_BASE_URL`).
    OpenAi,
    /// A local daemon; reads the optional `OLLAMA_API_BASE_URL` and
    /// `OLLAMA_API_KEY`.
    Ollama,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Ollama => "ollama",
        }
    }
}

/// Which model to use, written `provider:model`, e.g.
/// `anthropic:claude-sonnet-5-5`, `openai:gpt-5.2` or `ollama:qwen3`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelSpec {
    pub provider: Provider,
    pub model: String,
}

impl ModelSpec {
    /// Parse `provider:model`. The provider is case-insensitive; the model
    /// name is kept as written (trimmed) and may itself contain `:`, as
    /// Ollama tags do (`ollama:qwen3:8b`).
    pub fn parse(s: &str) -> Result<Self> {
        let invalid = |why: &str| {
            Error::Invalid(format!(
                "model {s:?}: {why}; expected `provider:model` with provider \
                 anthropic, openai or ollama, e.g. {DEFAULT_MODEL:?}"
            ))
        };
        let (provider, model) = s
            .trim()
            .split_once(':')
            .ok_or_else(|| invalid("missing provider"))?;
        let provider = match provider.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Provider::Anthropic,
            "openai" => Provider::OpenAi,
            "ollama" => Provider::Ollama,
            _ => return Err(invalid("unknown provider")),
        };
        let model = model.trim();
        if model.is_empty() {
            return Err(invalid("missing model name"));
        }
        Ok(ModelSpec {
            provider,
            model: model.to_owned(),
        })
    }
}

impl Default for ModelSpec {
    fn default() -> Self {
        ModelSpec::parse(DEFAULT_MODEL).expect("DEFAULT_MODEL parses")
    }
}

impl FromStr for ModelSpec {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        ModelSpec::parse(s)
    }
}

impl fmt::Display for ModelSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider.as_str(), self.model)
    }
}

/// The [`Examiner`] backed by a Rig completion model.
pub struct RigExaminer {
    spec: ModelSpec,
    core: Core<RigLlm>,
}

impl fmt::Debug for RigExaminer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RigExaminer")
            .field("model", &self.spec.to_string())
            .finish()
    }
}

impl RigExaminer {
    /// Connect to `model`, reading the provider's credentials from the
    /// environment as Rig does (e.g. `ANTHROPIC_API_KEY`). Nothing is sent
    /// until the first call.
    pub fn new(model: ModelSpec) -> Result<Self> {
        let env_error = |e: rig_core::client::env::EnvError| {
            Error::Port(format!(
                "examiner: cannot configure {}: {e}",
                model.provider.as_str()
            ))
        };
        let dyn_model: DynModel<Completion> = match model.provider {
            Provider::Anthropic => Anthropic::from_env()
                .map_err(env_error)?
                .completion(&model.model)
                .erase(),
            Provider::OpenAi => OpenAI::from_env()
                .map_err(env_error)?
                .completion(&model.model)
                .erase(),
            // The native API takes a JSON schema as `format`.
            Provider::Ollama => Ollama::from_env()
                .map_err(env_error)?
                .native_completion(&model.model)
                .erase(),
        };
        Ok(RigExaminer {
            core: Core {
                llm: RigLlm {
                    model: dyn_model,
                    label: model.to_string(),
                },
            },
            spec: model,
        })
    }

    /// The model this examiner uses.
    pub fn model(&self) -> &ModelSpec {
        &self.spec
    }
}

impl Examiner for RigExaminer {
    async fn extract_concepts(&self, diff: &Diff, code: &impl Codebase) -> Result<Vec<Concept>> {
        self.core.extract_concepts(diff, code).await
    }
    async fn write_question(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> Result<Question> {
        self.core.write_question(concept, brief, code).await
    }
    async fn grade(
        &self,
        question: &Question,
        answer: &Answer,
        code: &impl Codebase,
    ) -> Result<Grading> {
        self.core.grade(question, answer, code).await
    }
}

/// The Rig side of the seam. Without a codebase, one completion request
/// per call, with the schema as native structured output where the
/// provider supports it, as before the examiner could read (ADR-0008 D5).
/// With one, a Rig agent that may read it first ([`agent`]).
struct RigLlm {
    model: DynModel<Completion>,
    label: String,
}

impl Llm for RigLlm {
    type Transcript = Vec<Message>;

    async fn complete(
        &self,
        request: LlmRequest,
        transcript: Vec<Message>,
        code: &impl Codebase,
    ) -> Result<LlmAnswer<Vec<Message>>> {
        if code.is_available() {
            return agent::run(&self.model, &self.label, request, transcript, code).await;
        }
        let request = CompletionRequest::new(request.user)
            .preamble(request.system)
            .output_schema(request.schema);
        let response = self
            .model
            .call(request)
            .await
            .map_err(|e| Error::Port(format!("examiner: {}: {e}", self.label)))?;
        Ok(LlmAnswer {
            text: response.text(),
            transcript: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
