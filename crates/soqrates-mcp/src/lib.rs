//! MCP server exposing the Soqrates core through `rmcp` (ADR-0001 D10).
//!
//! [`SoqratesMcp`] wraps a shared [`Soqrates`] engine. It can be driven in
//! two ways:
//!
//! - in-process with [`SoqratesMcp::call_tool`], transport-free (used by
//!   tests and by anything embedding the server);
//! - over MCP on stdio with [`SoqratesMcp::serve_stdio`], via an `rmcp`
//!   [`ServerHandler`] that lists the four tools with JSON input schemas and
//!   delegates every call to [`SoqratesMcp::call_tool`].
//!
//! Other harnesses therefore get exactly the same learner model and
//! scheduling as the Soqrates agent, which is just one client.
//!
//! # Tools
//!
//! Exactly the four tools of ADR-0001 D10 (see [`tool_names`]). An *author*
//! is an identity string (ADR-0003 D1): `<forge host>:<account id>`, e.g.
//! `github.com:1001`, or `email:<address>`. Enum values (`format`, `grade`,
//! `level`) use the core's names, e.g. `"MultipleChoice"`, `"Good"`,
//! `"Novice"`. Times are RFC 3339 strings.
//!
//! ## `start_session`
//!
//! Arguments: `{author: string, diff: string, session_key?: string}`. Without
//! `session_key`, a key is derived from the author and the current time
//! (`mcp:<author>:<time>:<n>`).
//!
//! Result: `{session_key, questions: [{id, concept, format, prompt,
//! options?}]}`. `options` is present for multiple-choice questions only.
//!
//! The result **never** contains the answer key: neither the index of the
//! correct option nor the rubric of an open question. The client is the
//! learner's own harness and could leak them to the learner.
//!
//! ## `submit_answers`
//!
//! Arguments: `{session_key: string, answers: [{question: string, choice?:
//! number, text?: string}]}`. Each answer gives exactly one of `choice` (an
//! index into the question's `options`) or `text`.
//!
//! Result: `{passed: bool, level_after, grades: [{question, grade}],
//! feedback: [{question, correct_option?}]}`. After grading, the correct
//! option of each multiple-choice question is revealed in `feedback`.
//!
//! ## `get_learner_profile`
//!
//! Arguments: `{author: string}`.
//!
//! Result: `{level, cards: [{concept, due, reps}], reviews: number}`, where
//! `reviews` is the number of reviews recorded. Only the profile of that
//! author's own learner is returned; never another learner's data, links or
//! identities (ADR-0003 D6).
//!
//! ## `due_reviews`
//!
//! Arguments: `{author: string}`. Result: `{concepts: [string]}`, earliest
//! due first (ADR-0002 D8).
//!
//! # Errors
//!
//! An unknown tool or arguments that do not fit the tool's schema fail with
//! [`Error::Invalid`]; errors of the engine pass through unchanged. Over MCP,
//! an unknown tool is a protocol error (`invalid_params`); every other error
//! is returned as a tool result with `isError: true` and the message as text.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::{Value, json};
use soqrates_core::ports::{Clock, Examiner, StateStore};
use soqrates_core::{
    Answer, AnswerBody, AuthorId, Diff, Error, Identity, QuestionBody, QuestionId, Result,
    SessionKey, Soqrates,
};

/// Names of the MCP tools this server exposes. ADR-0001 D10 fixes this set.
pub fn tool_names() -> Vec<&'static str> {
    vec![
        "start_session",
        "submit_answers",
        "get_learner_profile",
        "due_reviews",
    ]
}

/// The MCP server: the four tools of ADR-0001 D10 over a shared engine.
pub struct SoqratesMcp<S, E, C> {
    app: Arc<Soqrates<S, E, C>>,
    /// Disambiguates derived session keys created at the same instant.
    sessions: AtomicU64,
}

// --- arguments --------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartSessionArgs {
    author: String,
    diff: String,
    #[serde(default)]
    session_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitAnswersArgs {
    session_key: String,
    answers: Vec<AnswerArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerArgs {
    question: String,
    #[serde(default)]
    choice: Option<usize>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorArgs {
    author: String,
}

fn parse_args<T: for<'de> Deserialize<'de>>(tool: &str, args: Value) -> Result<T> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args)
        .map_err(|e| Error::Invalid(format!("{tool}: invalid arguments: {e}")))
}

/// An author identity, checked to be a valid identity string (ADR-0003 D1).
fn author(tool: &str, text: String) -> Result<AuthorId> {
    Identity::parse(&text).map_err(|_| {
        Error::Invalid(format!(
            "{tool}: `author` must be an identity like \"github.com:1001\" or \
             \"email:jan@example.com\", got {text:?}"
        ))
    })?;
    Ok(AuthorId(text))
}

fn to_json<T: serde::Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::Invalid(format!("serialising result: {e}")))
}

// --- in-process API -----------------------------------------------------------

impl<S: StateStore, E: Examiner, C: Clock> SoqratesMcp<S, E, C> {
    pub fn new(app: Arc<Soqrates<S, E, C>>) -> Self {
        Self {
            app,
            sessions: AtomicU64::new(0),
        }
    }

    /// The engine this server exposes.
    pub fn app(&self) -> &Arc<Soqrates<S, E, C>> {
        &self.app
    }

    /// Dispatch one tool call, exactly as the MCP handler does, without any
    /// transport. See the crate docs for the tool contracts.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "start_session" => self.start_session(parse_args(name, args)?).await,
            "submit_answers" => self.submit_answers(parse_args(name, args)?).await,
            "get_learner_profile" => self.get_learner_profile(parse_args(name, args)?).await,
            "due_reviews" => self.due_reviews(parse_args(name, args)?).await,
            _ => Err(Error::Invalid(format!(
                "unknown tool {name:?}; available tools: {}",
                tool_names().join(", ")
            ))),
        }
    }

    async fn start_session(&self, args: StartSessionArgs) -> Result<Value> {
        let author = author("start_session", args.author)?;
        let key = match args.session_key {
            Some(k) if k.trim().is_empty() => {
                return Err(Error::Invalid(
                    "start_session: `session_key` must not be empty".into(),
                ));
            }
            Some(k) => SessionKey(k),
            None => SessionKey(format!(
                "mcp:{}:{}:{}",
                author.0,
                self.app.clock.now().to_rfc3339(),
                self.sessions.fetch_add(1, Ordering::Relaxed) + 1
            )),
        };
        let session = self
            .app
            .start_session(key, &author, &Diff(args.diff))
            .await?;
        // Only what the learner may see: never `correct` or `rubric`.
        let questions: Vec<Value> = session
            .questions()
            .iter()
            .map(|q| {
                let mut out = json!({
                    "id": q.id.0,
                    "concept": q.concept.0,
                    "format": q.format(),
                });
                match &q.body {
                    QuestionBody::MultipleChoice {
                        prompt, options, ..
                    } => {
                        out["prompt"] = json!(prompt);
                        out["options"] = json!(options);
                    }
                    QuestionBody::Open { prompt, .. } => {
                        out["prompt"] = json!(prompt);
                    }
                }
                out
            })
            .collect();
        Ok(json!({
            "session_key": session.key().0,
            "questions": questions,
        }))
    }

    async fn submit_answers(&self, args: SubmitAnswersArgs) -> Result<Value> {
        let key = SessionKey(args.session_key);
        let answers = args
            .answers
            .into_iter()
            .map(|a| {
                let body = match (a.choice, a.text) {
                    (Some(c), None) => AnswerBody::Choice(c),
                    (None, Some(t)) => AnswerBody::Text(t),
                    _ => {
                        return Err(Error::Invalid(format!(
                            "submit_answers: answer to {:?} needs exactly one of `choice` or `text`",
                            a.question
                        )));
                    }
                };
                Ok(Answer {
                    question: QuestionId(a.question),
                    body,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Read the questions before grading removes the session; an unknown
        // session is reported by `submit` itself.
        let questions = self
            .app
            .pending_session(&key)
            .await?
            .map(|s| s.questions().to_vec())
            .unwrap_or_default();
        let outcome = self.app.submit(&key, &answers).await?;
        let grades: Vec<Value> = outcome
            .grades
            .iter()
            .map(|(q, g)| json!({"question": q.0, "grade": g}))
            .collect();
        let feedback: Vec<Value> = questions
            .iter()
            .filter(|q| outcome.grades.iter().any(|(id, _)| id == &q.id))
            .map(|q| match &q.body {
                QuestionBody::MultipleChoice { correct, .. } => {
                    json!({"question": q.id.0, "correct_option": correct})
                }
                QuestionBody::Open { .. } => json!({"question": q.id.0}),
            })
            .collect();
        Ok(json!({
            "passed": outcome.passed,
            "level_after": to_json(outcome.level_after)?,
            "grades": grades,
            "feedback": feedback,
        }))
    }

    async fn get_learner_profile(&self, args: AuthorArgs) -> Result<Value> {
        let author = author("get_learner_profile", args.author)?;
        let profile = self.app.profile(&author).await?;
        // Deliberately no learner ID, links or identities (ADR-0003 D6).
        let cards: Vec<Value> = profile
            .cards()
            .iter()
            .map(|(concept, card)| {
                json!({
                    "concept": concept.0,
                    "due": card.due().to_rfc3339(),
                    "reps": card.reps(),
                })
            })
            .collect();
        Ok(json!({
            "level": to_json(profile.level())?,
            "cards": cards,
            "reviews": profile.history().len(),
        }))
    }

    async fn due_reviews(&self, args: AuthorArgs) -> Result<Value> {
        let author = author("due_reviews", args.author)?;
        let concepts: Vec<String> = self
            .app
            .due_reviews(&author)
            .await?
            .into_iter()
            .map(|c| c.0)
            .collect();
        Ok(json!({ "concepts": concepts }))
    }
}

// --- MCP --------------------------------------------------------------------

fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => unreachable!("tool schemas are JSON objects"),
    }
}

/// The four tools with their JSON input schemas.
fn tools() -> Vec<Tool> {
    let author = json!({
        "type": "string",
        "description": "The author's identity: `<forge host>:<account id>` (e.g. \
                        `github.com:1001`) or `email:<address>`."
    });
    vec![
        Tool::new(
            "start_session",
            "Start a learning session about a diff. Returns the questions to ask the \
             author (without the answer key).",
            schema(json!({
                "type": "object",
                "properties": {
                    "author": author,
                    "diff": {"type": "string", "description": "The unified diff to ask about."},
                    "session_key": {
                        "type": "string",
                        "description": "Key of the session; derived from the author and \
                                        the current time when omitted."
                    }
                },
                "required": ["author", "diff"],
                "additionalProperties": false
            })),
        ),
        Tool::new(
            "submit_answers",
            "Grade the author's answers to a session and record the reviews on their \
             learner profile.",
            schema(json!({
                "type": "object",
                "properties": {
                    "session_key": {"type": "string"},
                    "answers": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"type": "string", "description": "Question ID."},
                                "choice": {
                                    "type": "integer",
                                    "minimum": 0,
                                    "description": "Index into the question's options."
                                },
                                "text": {"type": "string", "description": "An open answer."}
                            },
                            "required": ["question"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["session_key", "answers"],
                "additionalProperties": false
            })),
        ),
        Tool::new(
            "get_learner_profile",
            "The author's own learner profile: level, review cards and number of reviews.",
            schema(json!({
                "type": "object",
                "properties": {"author": author},
                "required": ["author"],
                "additionalProperties": false
            })),
        ),
        Tool::new(
            "due_reviews",
            "Concepts whose spaced-repetition review is due for the author, earliest first.",
            schema(json!({
                "type": "object",
                "properties": {"author": author},
                "required": ["author"],
                "additionalProperties": false
            })),
        ),
    ]
}

/// The `rmcp` handler: lists [`tools`] and delegates to
/// [`SoqratesMcp::call_tool`].
struct Handler<S, E, C>(SoqratesMcp<S, E, C>);

impl<S, E, C> ServerHandler for Handler<S, E, C>
where
    S: StateStore + 'static,
    E: Examiner + 'static,
    C: Clock + 'static,
{
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("soqrates", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Soqrates asks authors about the concepts in their changes and schedules \
                 reviews with spaced repetition. Call start_session with a diff, ask the \
                 questions, then submit_answers.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, McpError> {
        if !tool_names().contains(&request.name.as_ref()) {
            return Err(McpError::invalid_params(
                format!(
                    "unknown tool {:?}; available tools: {}",
                    request.name,
                    tool_names().join(", ")
                ),
                None,
            ));
        }
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        Ok(match self.0.call_tool(&request.name, args).await {
            Ok(value) => CallToolResult::structured(value),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        }
        .into())
    }
}

impl<S, E, C> SoqratesMcp<S, E, C>
where
    S: StateStore + 'static,
    E: Examiner + 'static,
    C: Clock + 'static,
{
    /// Serve MCP over stdio until the client disconnects.
    pub async fn serve_stdio(self) -> Result<()> {
        let (reader, writer) = rmcp::transport::stdio_pipes();
        self.serve_on(reader, writer).await
    }

    /// Serve MCP on a byte stream pair until the client disconnects.
    async fn serve_on<R, W>(self, reader: R, writer: W) -> Result<()>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let running = Handler(self)
            .serve((reader, writer))
            .await
            .map_err(|e| Error::Port(format!("MCP initialisation failed: {e}")))?;
        running
            .waiting()
            .await
            .map_err(|e| Error::Port(format!("MCP server stopped: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod wire {
    //! The rmcp handler, driven over an in-memory byte stream with raw
    //! JSON-RPC, as an MCP client would.

    use super::*;
    use soqrates_core::{Answer, Concept, Grade, Question, QuestionFormat};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct NoExaminer;
    impl Examiner for NoExaminer {
        async fn extract_concepts(&self, _: &Diff) -> Result<Vec<Concept>> {
            Ok(Vec::new())
        }
        async fn write_question(&self, _: &Concept, _: QuestionFormat) -> Result<Question> {
            Err(Error::Invalid("no questions".into()))
        }
        async fn grade(&self, _: &Question, _: &Answer) -> Result<Grade> {
            Err(Error::Invalid("no grading".into()))
        }
    }

    struct EpochClock;
    impl Clock for EpochClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::UNIX_EPOCH
        }
    }

    #[tokio::test]
    async fn lists_and_calls_the_tools_over_mcp() {
        let app = Soqrates::new(
            soqrates_store::InMemoryStore::new(),
            NoExaminer,
            EpochClock,
            Default::default(),
        );
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server);
        let serving =
            tokio::spawn(SoqratesMcp::new(Arc::new(app)).serve_on(server_read, server_write));
        let (client_read, mut client_write) = tokio::io::split(client);
        let mut lines = BufReader::new(client_read).lines();
        let mut send = async |msg: Value| {
            let mut text = msg.to_string();
            text.push('\n');
            client_write.write_all(text.as_bytes()).await.unwrap();
        };

        send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
        )
        .await;
        let init: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "soqrates", "{init}");
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;

        send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
        let list: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{list}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, tool_names());
        for tool in list["result"]["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        }

        send(
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "get_learner_profile", "arguments": {"author": "github.com:1001"}}}),
        )
        .await;
        let call: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(
            call["result"]["structuredContent"],
            json!({"level": "Novice", "cards": [], "reviews": 0}),
            "{call}"
        );

        send(
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "get_learner_profile", "arguments": {"author": "nobody"}}}),
        )
        .await;
        let bad: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(bad["result"]["isError"], true, "{bad}");

        send(
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "nope", "arguments": {}}}),
        )
        .await;
        let unknown: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert!(unknown["error"].is_object(), "{unknown}");

        drop(client_write);
        drop(lines);
        serving.await.unwrap().unwrap();
    }
}
