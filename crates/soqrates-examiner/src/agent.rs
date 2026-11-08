//! The reading agent: Rig's agent runtime (`rig-agent`) with two read-only
//! tools over the codebase (ADR-0008 D2).
//!
//! Rig runs the loop: it sends the requests, dispatches the tool calls and
//! stops when the model answers. Soqrates keeps the policy:
//!
//! - the tools answer through [`crate::examiner::serve`], which checks the
//!   path, cuts long files, wraps the output in a block it can't close and
//!   counts the budget (D1-D3);
//! - [`ReadingLimit`], a hook, takes the tools away for the model call
//!   after the last reading turn and once the budget is spent, so the model
//!   then answers. Rig still offers the structured answer as it chose for
//!   the run: as a native schema, or as its own output tool;
//! - a retry continues the transcript with tools off: it reads no more.
//!
//! A Rig agent and its tools are `'static`, the codebase a borrow, so the
//! tools don't hold it: they send each call over a channel, and [`run`]
//! answers the calls from the codebase while it drives the agent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::channel::{mpsc, oneshot};
use futures::future::{self, Either};
use futures::{FutureExt, StreamExt};
use rig_agent::agent::{AgentHook, CompletionCallAction, CompletionCallEvent, RequestPatch};
use rig_agent::{AgentBuilder, HookContext};
use rig_core::DynModel;
use rig_core::completion::{Message, ToolAnnotations};
use rig_core::operation::Completion;
use rig_core::tool::{Tool, ToolContext, ToolExecutionError};
use serde::Deserialize;
use soqrates_core::ports::Codebase;
use soqrates_core::{Error, Result};

use crate::examiner::{Budget, LlmAnswer, LlmRequest, MAX_TOOL_TURNS, serve};
use crate::prompts;

/// One tool call, sent from a tool to [`run`], which answers it.
struct Call {
    tool: &'static str,
    path: Option<String>,
    reply: oneshot::Sender<std::result::Result<String, String>>,
}

/// The tools' end of the channel to [`run`].
#[derive(Clone)]
struct Bridge(mpsc::UnboundedSender<Call>);

impl Bridge {
    async fn ask(
        &self,
        tool: &'static str,
        path: Option<String>,
    ) -> std::result::Result<String, ToolExecutionError> {
        let (reply, answer) = oneshot::channel();
        self.0
            .unbounded_send(Call { tool, path, reply })
            .map_err(|_| ToolExecutionError::refused("The codebase is no longer available."))?;
        match answer.await {
            Ok(Ok(text)) => Ok(text),
            // The model sees why, and can correct the call.
            Ok(Err(why)) => Err(ToolExecutionError::refused(why)),
            Err(_) => Err(ToolExecutionError::refused(
                "The codebase is no longer available.",
            )),
        }
    }
}

/// The arguments of both tools. A missing or non-string path is refused by
/// [`serve`], with a message the model can act on.
#[derive(Deserialize)]
struct PathArgs {
    #[serde(default)]
    path: Option<serde_json::Value>,
}

impl PathArgs {
    fn path(self) -> Option<String> {
        match self.path {
            Some(serde_json::Value::String(p)) => Some(p),
            _ => None,
        }
    }
}

/// The description and argument schema of tool `name`, from
/// [`prompts::tools`], where evals find every text the model sees.
fn spec(name: &str) -> (String, serde_json::Value) {
    prompts::tools()
        .into_iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, description, parameters)| (description.to_owned(), parameters))
        .expect("every tool is in prompts::tools")
}

fn read_only() -> ToolAnnotations {
    ToolAnnotations::default()
        .with_read_only(true)
        .with_destructive(false)
        .with_idempotent(true)
        .with_open_world(false)
}

/// `read_file`: one file of the codebase.
struct ReadFile(Bridge);

impl Tool for ReadFile {
    const NAME: &'static str = prompts::READ_FILE;
    type Args = PathArgs;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        spec(Self::NAME).0
    }
    fn parameters(&self) -> serde_json::Value {
        spec(Self::NAME).1
    }
    fn annotations(&self) -> ToolAnnotations {
        read_only()
    }
    async fn call(
        &self,
        _context: &mut ToolContext,
        args: PathArgs,
    ) -> std::result::Result<String, ToolExecutionError> {
        self.0.ask(Self::NAME, args.path()).await
    }
}

/// `list_dir`: one directory of the codebase.
struct ListDir(Bridge);

impl Tool for ListDir {
    const NAME: &'static str = prompts::LIST_DIR;
    type Args = PathArgs;
    type Output = String;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        spec(Self::NAME).0
    }
    fn parameters(&self) -> serde_json::Value {
        spec(Self::NAME).1
    }
    fn annotations(&self) -> ToolAnnotations {
        read_only()
    }
    async fn call(
        &self,
        _context: &mut ToolContext,
        args: PathArgs,
    ) -> std::result::Result<String, ToolExecutionError> {
        self.0.ask(Self::NAME, args.path()).await
    }
}

/// Takes the reading tools away for a model call after the reading turns,
/// once the budget is spent, or for a whole retry, so that the model
/// answers (ADR-0008 D2). An empty `active_tools`, not `tool_choice: none`:
/// when Rig offers the answer as an output tool, that must stay callable.
struct ReadingLimit {
    /// Whether this run may read at all: not on a retry.
    reads: bool,
    spent: Arc<AtomicBool>,
}

impl AgentHook for ReadingLimit {
    async fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let done = !self.reads || event.turn > MAX_TOOL_TURNS || self.spent.load(Ordering::SeqCst);
        if done {
            CompletionCallAction::patch(RequestPatch::new().active_tools(Vec::<String>::new()))
        } else {
            CompletionCallAction::continue_run()
        }
    }
}

/// Let the model read `code` for `request`, then answer it. After a
/// non-empty `transcript` (a retry), the run continues it with tools off.
/// Returns the answer and the transcript so far.
pub(crate) async fn run(
    model: &DynModel<Completion>,
    label: &str,
    request: LlmRequest,
    transcript: Vec<Message>,
    code: &impl Codebase,
) -> Result<LlmAnswer<Vec<Message>>> {
    let (calls, mut answers) = mpsc::unbounded();
    let bridge = Bridge(calls);
    let spent = Arc::new(AtomicBool::new(false));
    let agent = AgentBuilder::new(model.clone())
        .preamble(prompts::with_tools(request.system))
        .tool(ReadFile(bridge.clone()))
        .tool(ListDir(bridge))
        .output_schema_raw(request.schema)
        .add_hook(ReadingLimit {
            reads: transcript.is_empty(),
            spent: spent.clone(),
        })
        .build();
    // The reading turns, and one to answer in.
    let mut runner = agent.prompt(request.user).max_turns(MAX_TOOL_TURNS + 1);
    if !transcript.is_empty() {
        runner = runner.history(transcript.clone());
    }
    let mut pending = runner.into_future().fuse();
    drop(agent);

    // Drive the agent, answering its tool calls from `code` as they come.
    let mut budget = Budget::default();
    let finished = loop {
        match future::select(pending, answers.next()).await {
            Either::Left((finished, _)) => break finished,
            Either::Right((Some(call), still)) => {
                let answer = serve(call.tool, call.path.as_deref(), code, &mut budget).await;
                if budget.exhausted() {
                    spent.store(true, Ordering::SeqCst);
                }
                // A call whose tool gave up waiting has nobody to tell.
                let _ = call.reply.send(answer);
                pending = still;
            }
            // Every tool is gone: the run has no more calls to make.
            Either::Right((None, still)) => break still.await,
        }
    };
    let response = finished.map_err(|e| Error::Port(format!("examiner: {label}: {e}")))?;
    let text = response.output();
    let mut transcript = transcript;
    transcript.extend(response.messages);
    Ok(LlmAnswer { text, transcript })
}
