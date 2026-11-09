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
pub(crate) const MAX_TOOL_TURNS: usize = 8;
pub(crate) const MAX_TOOL_CALLS: usize = 20;
pub(crate) const MAX_FILE_BYTES: usize = 20_000;
pub(crate) const MAX_TOOL_BYTES: usize = 100_000;
/// The most paths a glob, matches a grep, commits a log and lines a blame
/// returns.
pub(crate) const MAX_GLOB_PATHS: usize = 200;
pub(crate) const MAX_GREP_MATCHES: usize = 60;
pub(crate) const MAX_LOG_COMMITS: usize = 15;
pub(crate) const MAX_BLAME_LINES: u32 = 150;
/// Bytes kept free for the notes and tags around a file's text.
const BLOCK_ROOM: usize = 400;
/// Longer lines in grep and blame output are cut.
pub(crate) const MAX_LINE_CHARS: usize = 240;
/// The longest regular expression or glob accepted.
pub(crate) const MAX_PATTERN_CHARS: usize = 200;

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

/// `line` cut to [`MAX_LINE_CHARS`] characters.
fn short(line: &str) -> String {
    let mut chars = line.chars();
    let head: String = chars.by_ref().take(MAX_LINE_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// `raw` as a glob over repository paths (ADR-0008 D1): relative, without
/// `..`, backslashes, control characters or `:` (git's pathspec magic). A
/// pattern without `/` matches file names in any directory.
pub(crate) fn code_glob(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches("./");
    if raw.is_empty()
        || raw.chars().count() > MAX_PATTERN_CHARS
        || raw.starts_with('/')
        || raw.contains(['\\', ':'])
        || raw.chars().any(char::is_control)
        || raw.split('/').any(|part| part == "..")
    {
        return None;
    }
    Some(if raw.contains('/') {
        raw.to_owned()
    } else {
        format!("**/{raw}")
    })
}

/// The paths of `files` that match `glob` (as [`code_glob`] returns it).
pub(crate) fn matching<'a>(files: &'a [String], glob: &str) -> Option<Vec<&'a String>> {
    let matcher = globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .ok()?
        .compile_matcher();
    // `**/x` also matches `x` at the root.
    let root = glob.strip_prefix("**/").and_then(|g| {
        globset::GlobBuilder::new(g)
            .literal_separator(true)
            .build()
            .ok()
            .map(|g| g.compile_matcher())
    });
    Some(
        files
            .iter()
            .filter(|f| matcher.is_match(f) || root.as_ref().is_some_and(|r| r.is_match(f)))
            .collect(),
    )
}

/// The entries directly in the directory `dir` (`""` is the root), given
/// every file path: sorted, directories with a trailing `/`. `None` when
/// `dir` holds no file, so is not a directory.
pub(crate) fn children(files: &[String], dir: &str) -> Option<Vec<String>> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut entries: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(&prefix))
        .filter(|rest| !rest.is_empty())
        .map(|rest| match rest.split_once('/') {
            Some((sub, _)) => format!("{sub}/"),
            None => rest.to_owned(),
        })
        .collect();
    entries.sort();
    entries.dedup();
    (!entries.is_empty()).then_some(entries)
}

/// The arguments of a tool call.
struct Args<'a>(&'a serde_json::Value);

impl Args<'_> {
    fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(serde_json::Value::as_str)
    }
    fn line(&self, key: &str) -> std::result::Result<Option<u32>, String> {
        match self.0.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| *n >= 1)
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("`{key}` must be a line number, 1 or more.")),
        }
    }
    fn flag(&self, key: &str) -> bool {
        self.0
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
    /// The `path` argument as a repository path.
    fn path(&self) -> std::result::Result<String, String> {
        let raw = self.text("path");
        raw.and_then(code_path).ok_or_else(|| {
            format!(
                "Invalid path {:?}: give a path relative to the repository root, without `..`.",
                raw.unwrap_or_default()
            )
        })
    }
}

fn port_failure(what: &str, e: Error) -> String {
    format!("{what} failed: {e}")
}

/// Answer one call of the tool `tool` with `args` from `code`, within
/// `budget`: what the model sees, or why the call was refused
/// (ADR-0008 D1-D3). Every tool of `prompts::tools` has its case here.
pub(crate) async fn serve(
    tool: &str,
    args: &serde_json::Value,
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
    let args = Args(args);
    let room = MAX_TOOL_BYTES.saturating_sub(budget.bytes);
    let text = match tool {
        prompts::READ_FILE => {
            let path = args.path()?;
            let (start, end) = (args.line("start_line")?, args.line("end_line")?);
            let content = code
                .read_file(&path)
                .await
                .map_err(|e| port_failure("Reading", e))?
                .ok_or_else(|| format!("There is no text file at {path:?}."))?;
            let (shown, range) = if start.is_none() && end.is_none() {
                (content, None)
            } else {
                let total = content.lines().count();
                let first = start.unwrap_or(1) as usize;
                let last = end.map_or(total, |e| (e as usize).min(total));
                if first > last {
                    return Err(format!(
                        "{path:?} has {total} lines; ask for a range within them."
                    ));
                }
                let lines: Vec<&str> = content
                    .lines()
                    .skip(first - 1)
                    .take(last + 1 - first)
                    .collect();
                (
                    lines.join("\n") + "\n",
                    Some((first as u32, last as u32, total)),
                )
            };
            // Room for the block around it, too.
            let max = MAX_FILE_BYTES.min(room.saturating_sub(BLOCK_ROOM));
            let cut_text = cut(&shown, max);
            let was_cut = (cut_text.len() < shown.len()).then_some(shown.len());
            prompts::file_block(&path, cut_text, range, was_cut)
        }
        prompts::LIST_DIR => {
            let path = args.path()?;
            let files = code
                .list_files()
                .await
                .map_err(|e| port_failure("Listing", e))?;
            let entries = children(&files, &path)
                .ok_or_else(|| format!("There is no directory at {path:?}."))?;
            prompts::listing_block(&path, &entries)
        }
        prompts::GLOB => {
            let raw = args.text("pattern").unwrap_or_default();
            let glob = code_glob(raw).ok_or_else(|| {
                format!("Invalid glob {raw:?}: give a relative pattern, without `..` or `:`.")
            })?;
            let files = code
                .list_files()
                .await
                .map_err(|e| port_failure("Listing", e))?;
            let found = matching(&files, &glob).ok_or_else(|| format!("Invalid glob {raw:?}."))?;
            let shown: Vec<String> = found
                .iter()
                .take(MAX_GLOB_PATHS)
                .map(|p| (*p).clone())
                .collect();
            prompts::files_block(raw, &shown, found.len())
        }
        prompts::GREP => {
            let pattern = args.text("pattern").unwrap_or_default();
            if pattern.is_empty()
                || pattern.chars().count() > MAX_PATTERN_CHARS
                || pattern.chars().any(|c| c.is_control() && c != '\t')
            {
                return Err(format!(
                    "Invalid pattern: give a regular expression of 1 to {MAX_PATTERN_CHARS} characters."
                ));
            }
            let glob = match args.text("glob") {
                None | Some("") => None,
                Some(raw) => Some(code_glob(raw).ok_or_else(|| {
                    format!("Invalid glob {raw:?}: give a relative pattern, without `..` or `:`.")
                })?),
            };
            let found = code
                .grep(
                    pattern,
                    glob.as_deref(),
                    args.flag("ignore_case"),
                    MAX_GREP_MATCHES + 1,
                )
                .await
                .map_err(|e| port_failure("Searching", e))?;
            let more = found.len() > MAX_GREP_MATCHES;
            let lines: Vec<String> = found
                .iter()
                .take(MAX_GREP_MATCHES)
                .map(|m| format!("{}:{}: {}", m.path, m.line, short(&m.text)))
                .collect();
            prompts::matches_block(pattern, &lines, more)
        }
        prompts::GIT_LOG => {
            let path = args.path()?;
            let commits = code
                .history(&path, MAX_LOG_COMMITS)
                .await
                .map_err(|e| port_failure("Reading the history", e))?;
            let lines: Vec<String> = commits
                .iter()
                .map(|c| format!("{} {} {}: {}", c.id, c.date, c.author, short(&c.summary)))
                .collect();
            prompts::history_block(&path, &lines)
        }
        prompts::GIT_BLAME => {
            let path = args.path()?;
            let (Some(start), Some(end)) = (args.line("start_line")?, args.line("end_line")?)
            else {
                return Err("Give start_line and end_line.".into());
            };
            if end < start {
                return Err("end_line must not be before start_line.".into());
            }
            let end = end.min(start.saturating_add(MAX_BLAME_LINES - 1));
            let lines = code
                .blame(&path, start, end)
                .await
                .map_err(|e| port_failure("Blaming", e))?
                .ok_or_else(|| format!("There is no file at {path:?}."))?;
            let lines: Vec<String> = lines
                .iter()
                .map(|l| {
                    format!(
                        "{} {} {} {} | {}",
                        l.line,
                        l.commit,
                        l.date,
                        l.author,
                        short(&l.text)
                    )
                })
                .collect();
            prompts::blame_block(&path, &lines)
        }
        other => return Err(format!("There is no tool {other:?}.")),
    };
    // Never cut a block: its closing tag would go (ADR-0008 D3).
    if text.len() > room {
        return Err(
            "This output doesn't fit in what is left of the reading limit: narrow it, or answer \
             from what you have read."
                .into(),
        );
    }
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
    /// is shown its code too (D7). Also returns whether the check passed:
    /// `false` when its reply couldn't be parsed (ADR-0009 D3).
    async fn multiple_choice(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> Result<(MultipleChoice, bool)> {
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
                    OptionCheck::Passed => (Ok((mc, true)), after),
                    OptionCheck::Unknown => (Ok((mc, false)), after),
                    OptionCheck::Failed(why) => (Err(why), after),
                })
            },
        )
        .await
    }

    /// Ask the model whether `mc`'s reasons discuss answers it doesn't
    /// offer: the problem if they do. An unusable reply from the check
    /// keeps the question; the check is a safeguard on output that has
    /// already passed validation, not a reason to lose it.
    async fn unlisted_options(&self, mc: &MultipleChoice) -> Result<OptionCheck> {
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
        Ok(match output::parse_json::<OptionCheckOutput>(&raw) {
            Err(_) => OptionCheck::Unknown,
            Ok(out) => match output::unlisted_option_problem(out) {
                None => OptionCheck::Passed,
                Some(why) => OptionCheck::Failed(why),
            },
        })
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
        let (kind, body, explanation, checked) = match format {
            QuestionFormat::MultipleChoice => {
                // One reason per option, written with the options and
                // checked to discuss only those (issue #5).
                let (mc, checked) = self.multiple_choice(concept, brief, code).await?;
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
                (mc.kind, body, explanation, checked)
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
                // No options to check: a valid open question is consistent
                // (ADR-0009 D3).
                (open.kind, body, open.explanation, true)
            }
        };
        Ok(Question {
            id: question_id(concept, format),
            concept: concept.id.clone(),
            scope: concept.scope,
            kind,
            body,
            explanation,
            checked,
            banked: None,
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
/// What the option check (issue #5) said about a multiple-choice question.
enum OptionCheck {
    /// Its reasons discuss only its options.
    Passed,
    /// The check's reply couldn't be parsed: the question is kept, unchecked.
    Unknown,
    /// The problem to explain when the question is asked for again.
    Failed(String),
}

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
