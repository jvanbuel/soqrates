//! Pure logic of the GitHub adapter: rendering the question, graded and
//! results comments, reading the author's answers from boxes and comments,
//! and mapping check statuses (ADR-0004). No I/O, so
//! all of it is unit tested (ADR-0001 D12).

use chrono::{DateTime, Utc};
use soqrates_core::{
    Answer, AnswerBody, CheckStatus, Question, QuestionBody, QuestionResult, Session, SessionKey,
};

/// The command an answer comment starts with.
pub const ANSWER_COMMAND: &str = "/soqrates answer";

/// The commit status context Soqrates reports under.
pub const STATUS_CONTEXT: &str = "soqrates";

/// GitHub limits a commit status description to 140 characters.
pub const MAX_DESCRIPTION_CHARS: usize = 140;

/// The state of a commit status, independent of octocrab's types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
    Pending,
    Success,
    Failure,
}

/// The hidden marker that ties a questions comment to its session.
pub fn session_marker(key: &SessionKey) -> String {
    format!("{MARKER_START}{}{MARKER_END}", key.0)
}

const MARKER_START: &str = "<!-- soqrates:session=";
const MARKER_END: &str = " -->";

/// The identity host of a server URL such as `https://github.com` or
/// `https://ghe.example.com:8443/`: the host name without scheme, user info,
/// port or path, lowercased.
pub fn host_of(server_url: &str) -> Option<String> {
    let rest = server_url
        .split_once("://")
        .map_or(server_url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The author id Soqrates uses for a GitHub account: host plus the immutable
/// numeric user id, never the login (ADR-0003 D1).
pub fn author_id(host: &str, user_id: u64) -> String {
    format!("{host}:{user_id}")
}

/// Split `owner/name`.
pub fn split_repo(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then_some((owner, name))
}

/// The letter of the option at `index`: `a`, `b`, ...
fn option_letter(index: usize) -> char {
    char::from(b'a' + (index % 26) as u8)
}

// ---------------------------------------------------------------------------
// Neutralising model-provided text (ADR-0004 D4)

/// Escape one character of text that is shown as is. `<`, `>` and `&`
/// become entities, so no tag, comment or session marker can be injected;
/// `[`, `]` and `\` are backslash-escaped, so no link, image or task box can
/// be formed; a backtick becomes an entity, so no code span is left open;
/// and `@` is followed by a zero-width space, so nobody is mentioned.
fn escape_char(c: char, out: &mut String) {
    match c {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        '`' => out.push_str("&#96;"),
        '\\' => out.push_str("\\\\"),
        '[' => out.push_str("\\["),
        ']' => out.push_str("\\]"),
        '@' => out.push_str("@\u{200B}"),
        '\r' => {}
        c if c.is_control() && c != '\t' => {}
        c => out.push(c),
    }
}

/// Text inside a `<code>` element: like [`escape_char`], and emphasis
/// markers are backslash-escaped, because inline HTML does not stop
/// Markdown from being parsed inside it.
fn escape_code(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '*' | '_' | '~' => {
                out.push('\\');
                out.push(c);
            }
            c => escape_char(c, out),
        }
    }
}

fn push_code(text: &str, out: &mut String) {
    out.push_str("<code>");
    escape_code(text, out);
    out.push_str("</code>");
}

/// One line of text outside a fenced block: code spans (same line only)
/// become `<code>`, everything else is escaped.
fn inline_line(line: &str, out: &mut String) {
    let chars: Vec<char> = line.chars().collect();
    let run_at = |i: usize| chars[i..].iter().take_while(|&&c| c == '`').count();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            escape_char(chars[i], out);
            i += 1;
            continue;
        }
        let n = run_at(i);
        // The closing run: the next run of exactly `n` backticks.
        let mut j = i + n;
        let mut close = None;
        while j < chars.len() {
            if chars[j] == '`' {
                let m = run_at(j);
                if m == n {
                    close = Some(j);
                    break;
                }
                j += m;
            } else {
                j += 1;
            }
        }
        match close {
            Some(j) => {
                let content: String = chars[i + n..j].iter().collect();
                let content = content.trim();
                if content.is_empty() {
                    (0..2 * n).for_each(|_| out.push_str("&#96;"));
                } else {
                    push_code(content, out);
                }
                i = j + n;
            }
            None => {
                (0..n).for_each(|_| out.push_str("&#96;"));
                i += n;
            }
        }
    }
}

/// Model- or author-provided text, made safe to embed anywhere in a
/// comment: on one physical line (lines are joined with `<br>`), so it can
/// never start a line of its own (no forged task box, heading, alert, list
/// item or `</details>`), with every `<` escaped and nobody mentioned. Code
/// spans and fenced code stay readable as `<code>`.
pub fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut first = true;
    for line in text.trim().lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !first {
            out.push_str("<br>");
        }
        first = false;
        if in_fence {
            if line.trim().is_empty() {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            out.push_str("<code>");
            (0..indent).for_each(|_| out.push_str("&nbsp;"));
            escape_code(line.trim(), &mut out);
            out.push_str("</code>");
        } else {
            inline_line(line.trim(), &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The question comment (ADR-0004 D3, D4)

/// The text of the box that submits checkbox answers (ADR-0004 D3).
pub const DONE_BOX: &str = "**Grade my answers**";

/// Each question's section starts with this heading, followed by its
/// 1-based number. Checkboxes are mapped to questions by section.
const QUESTION_HEADING: &str = "### Question ";

fn question_heading(n: usize, total: usize) -> String {
    format!("{QUESTION_HEADING}{n} of {total}")
}

fn prompt_of(q: &Question) -> &str {
    match &q.body {
        QuestionBody::MultipleChoice { prompt, .. } | QuestionBody::Open { prompt, .. } => prompt,
    }
}

const HOW_TO_ANSWER: &str = "<details>
<summary>How to answer</summary>

- **Multiple choice:** tick the box of one option per question, then tick \
**Grade my answers**.
- **Open questions:** reply to this pull request with a comment that starts \
with `/soqrates answer`, followed by one line per question: its number, then \
your answer. An answer may span several lines. Posting this comment submits \
all your answers, ticked boxes included, so tick them first.
- **Can't tick the boxes?** If you can't edit this comment, answer the \
multiple-choice questions in the same comment with the letter of an option. \
A letter in the comment replaces the boxes of that question.

```
/soqrates answer
1. b
2. Your answer in your own words
```

Only answers from the author of this pull request count: boxes ticked by \
anyone else are ignored. Until your answers are graded, a newer answer \
comment replaces an earlier one. After grading, answers for this commit are \
final; a new push asks new questions.

</details>
";

/// The Markdown comment putting the session's questions to the author
/// (ADR-0004 D3, D4). Multiple-choice options are task-list boxes, followed
/// by the "Grade my answers" box. It never contains the correct option, the
/// rubric or the explanation. A session without questions gets a short note
/// with no boxes, which earlier question comments can link to (ADR-0004 D7).
pub fn render_questions(session: &Session) -> String {
    let questions = session.questions();
    let total = questions.len();
    let mut out = String::new();
    out.push_str(&session_marker(session.key()));
    out.push_str("\n> [!NOTE]\n");
    if questions.is_empty() {
        out.push_str(
            "> **Soqrates learning check: nothing to ask.** Soqrates found no \
             questions to ask about this commit, so there is nothing to answer.\n",
        );
        return out;
    }
    out.push_str(
        "> **Soqrates learning check.** A few questions about the concepts in \
         this change, to help you learn and remember them. It is advisory, not a \
         code review.\n\n",
    );
    for (i, q) in questions.iter().enumerate() {
        out.push_str(&question_heading(i + 1, total));
        out.push_str("\n\n");
        out.push_str(&inline(prompt_of(q)));
        out.push_str("\n\n");
        match &q.body {
            QuestionBody::MultipleChoice { options, .. } => {
                for (j, option) in options.iter().enumerate() {
                    out.push_str(&format!("- [ ] {}. {}\n", option_letter(j), inline(option)));
                }
                out.push('\n');
            }
            QuestionBody::Open { .. } => {
                out.push_str(
                    "_Open question: answer it in your own words in a \
                     `/soqrates answer` comment._\n\n",
                );
            }
        }
    }
    out.push_str("---\n\n");
    out.push_str(&format!("- [ ] {DONE_BOX}\n\n"));
    out.push_str(HOW_TO_ANSWER);
    out
}

// ---------------------------------------------------------------------------
// Reading the boxes (ADR-0004 D3)

/// The ticked options of one multiple-choice question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ticked {
    None,
    One(usize),
    Many,
}

/// The state of the boxes in a question comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkboxes {
    /// Whether "Grade my answers" is ticked.
    pub done: bool,
    /// Per question, in question order; always `Ticked::None` for open
    /// questions.
    pub ticked: Vec<Ticked>,
}

impl Checkboxes {
    /// Whether any box at all is ticked.
    pub fn any(&self) -> bool {
        self.done || self.ticked.iter().any(|t| *t != Ticked::None)
    }
}

/// A task-list item (`- [x] rest`, `* [ ] rest`, `+ [X] rest`, with any
/// indentation) → `(ticked, rest)`.
fn task_item(line: &str) -> Option<(bool, &str)> {
    let line = line.trim_start();
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))?
        .trim_start();
    let (ticked, rest) = match rest.get(..3)? {
        "[ ]" => (false, &rest[3..]),
        "[x]" | "[X]" => (true, &rest[3..]),
        _ => return None,
    };
    match rest.chars().next() {
        None => Some((ticked, rest)),
        Some(c) if c.is_whitespace() => Some((ticked, rest.trim())),
        Some(_) => None,
    }
}

/// The 1-based number of a question heading line.
fn heading_number(line: &str) -> Option<usize> {
    let rest = line.trim().strip_prefix(QUESTION_HEADING)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Read the boxes of a question comment rendered by [`render_questions`].
/// A box belongs to the question whose section it is in, and an option is
/// identified by its position in that section, never by its text. The
/// "How to answer" section and anything after it are ignored.
pub fn parse_checkboxes(body: &str, questions: &[Question]) -> Checkboxes {
    let mut boxes = Checkboxes {
        done: false,
        ticked: vec![Ticked::None; questions.len()],
    };
    // The current question, and the position of the next box in it.
    let mut current: Option<(usize, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<details") {
            break;
        }
        if let Some(n) = heading_number(trimmed) {
            current = (1..=questions.len()).contains(&n).then_some((n - 1, 0));
            continue;
        }
        if trimmed == "---" {
            current = None;
            continue;
        }
        let Some((ticked, rest)) = task_item(line) else {
            continue;
        };
        if rest == DONE_BOX {
            boxes.done |= ticked;
            current = None;
            continue;
        }
        let Some((q, position)) = current.as_mut() else {
            continue;
        };
        let option = *position;
        *position += 1;
        let QuestionBody::MultipleChoice { options, .. } = &questions[*q].body else {
            continue;
        };
        if !ticked || option >= options.len() {
            continue;
        }
        let t = &mut boxes.ticked[*q];
        *t = match *t {
            Ticked::None => Ticked::One(option),
            _ => Ticked::Many,
        };
    }
    boxes
}

/// The answer recorded for a multiple-choice question with more than one
/// ticked option; graded wrong (ADR-0004 D3).
pub const MULTIPLE_TICKED: &str = "multiple options ticked";

/// Combine the boxes and the answer comment into the author's answers
/// (ADR-0004 D3). `boxes` must be `None` unless the comment's last editor
/// is the PR author; `comment` is the parsed `/soqrates answer` comment, if
/// any. Returns `None` until the author has submitted, by ticking "Grade my
/// answers" or by posting an answer comment.
///
/// Per question, an answer in the comment wins; otherwise one ticked option
/// is that choice, several are wrong and none is unanswered. Open questions
/// are answered only in the comment.
pub fn merge_answers(
    questions: &[Question],
    boxes: Option<&Checkboxes>,
    comment: Option<Vec<Answer>>,
) -> Option<Vec<Answer>> {
    let done = boxes.is_some_and(|b| b.done);
    if !done && comment.is_none() {
        return None;
    }
    let comment = comment.unwrap_or_default();
    let answers = questions
        .iter()
        .enumerate()
        .filter_map(|(i, q)| {
            if let Some(a) = comment.iter().find(|a| a.question == q.id) {
                return Some(a.clone());
            }
            let body = match boxes.map(|b| b.ticked[i])? {
                Ticked::None => return None,
                Ticked::One(option) => AnswerBody::Choice(option),
                Ticked::Many => AnswerBody::Text(MULTIPLE_TICKED.into()),
            };
            Some(Answer {
                question: q.id.clone(),
                body,
            })
        })
        .collect();
    Some(answers)
}

// ---------------------------------------------------------------------------
// The question comment's last editor (ADR-0004 D3)

/// GraphQL query for the last editor of a comment, by its `node_id`. The
/// `editor` of a `Comment` is the actor who last edited it; it is `null` if
/// the comment was never edited. Only a `User` counts.
pub const EDITOR_QUERY: &str = "query($id: ID!) { node(id: $id) { ... on IssueComment { \
     editor { __typename ... on User { databaseId } } } } }";

/// The numeric user id of the last editor in the `data` of an
/// [`EDITOR_QUERY`] response, if the editor is a user.
pub fn editor_database_id(data: &serde_json::Value) -> Option<u64> {
    let editor = data.get("node")?.get("editor")?;
    if editor.get("__typename").and_then(|t| t.as_str()) != Some("User") {
        return None;
    }
    editor.get("databaseId")?.as_u64()
}

/// The base URI under which `/graphql` is found, for a REST API URL:
/// `https://api.github.com` for itself, `https://ghe.example.com/api` for
/// `https://ghe.example.com/api/v3` (GitHub Enterprise Server serves GraphQL
/// at `/api/graphql`).
pub fn graphql_base(api_url: &str) -> String {
    let api_url = api_url.trim_end_matches('/');
    api_url.strip_suffix("/v3").unwrap_or(api_url).to_string()
}

// ---------------------------------------------------------------------------
// Results (ADR-0004 D1, D2, D5)

/// The hidden marker of the results comment; distinct from
/// [`session_marker`], so it is never taken for the question comment.
pub fn results_marker(key: &SessionKey) -> String {
    format!("<!-- soqrates:results={} -->", key.0)
}

fn option_text(options: &[String], index: usize) -> String {
    match options.get(index) {
        Some(o) => format!("{}. {}", option_letter(index), inline(o)),
        None => format!("option {}", index + 1),
    }
}

fn answer_text(result: &QuestionResult) -> String {
    match &result.answer {
        None => "_No answer_".into(),
        Some(AnswerBody::Choice(i)) => option_text(&result.options, *i),
        Some(AnswerBody::Text(t)) if t.trim().is_empty() => "_No answer_".into(),
        Some(AnswerBody::Text(t)) => inline(t),
    }
}

/// The comment with the results of a graded session (ADR-0004 D1, D2).
/// It takes the results and the pass flag only, so nothing from the
/// learner's profile (level, review schedule, other concepts) can appear.
pub fn render_results(session: &Session, results: &[QuestionResult], passed: bool) -> String {
    let total = results.len();
    let correct = results.iter().filter(|r| r.is_correct()).count();
    let mut out = String::new();
    out.push_str(&results_marker(session.key()));
    out.push('\n');
    if passed {
        out.push_str("> [!TIP]\n");
        out.push_str(&format!(
            "> **Soqrates: {correct} of {total} correct.** Nice work. Each \
             question below says why its answer is right, to help it stick.\n\n"
        ));
    } else {
        out.push_str("> [!WARNING]\n");
        out.push_str(&format!(
            "> **Soqrates: {correct} of {total} correct.** Not there yet, and \
             that is what this check is for: each question below explains what \
             it was getting at. These concepts will come back for review.\n\n"
        ));
    }
    for (i, r) in results.iter().enumerate() {
        let mark = if r.is_correct() { "✅" } else { "❌" };
        out.push_str(&format!("{} {mark}\n\n", question_heading(i + 1, total)));
        out.push_str(&inline(&r.prompt));
        out.push_str("\n\n");
        out.push_str(&format!("- **Your answer:** {}\n", answer_text(r)));
        if let Some(c) = r.correct_option {
            out.push_str(&format!(
                "- **Correct answer:** {}\n",
                option_text(&r.options, c)
            ));
        }
        out.push('\n');
        let feedback = inline(&r.feedback);
        if feedback.is_empty() {
            continue;
        }
        if r.is_correct() {
            out.push_str("<details>\n<summary>Why</summary>\n\n");
            out.push_str(&feedback);
            out.push_str("\n\n</details>\n\n");
        } else {
            out.push_str(&format!("**Why:** {feedback}\n\n"));
        }
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// The question comment once graded (ADR-0004 D5): it says so, and the
/// questions are collapsed, with options as plain text (the author's answer
/// marked), so no box can be ticked any more.
pub fn render_graded_questions(session: &Session, results: &[QuestionResult]) -> String {
    let mut out = String::new();
    out.push_str(&session_marker(session.key()));
    out.push_str("\n> [!NOTE]\n");
    out.push_str(
        "> **Soqrates learning check: graded.** The results are in a comment \
         below. The answers for this commit are final; a new push asks new \
         questions.\n\n",
    );
    push_collapsed_questions(session.questions(), Some(results), &mut out);
    out
}

/// The question comment of `key` once a newer push superseded it (ADR-0004
/// D7): it says its answers no longer count and links the newer question
/// comment at `newer_url`, if known, saying whether that one asks anything.
/// The questions of `session`, if still stored, are collapsed with options
/// as plain text, so no box can be ticked any more.
pub fn render_superseded_questions(
    key: &SessionKey,
    session: Option<&Session>,
    newer_url: Option<&str>,
    newer_asks: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&session_marker(key));
    out.push_str("\n> [!NOTE]\n");
    out.push_str(
        "> **Soqrates learning check: superseded.** A newer commit was pushed, \
         so answers to these questions no longer count. ",
    );
    // The URL comes from the forge, not from the model; it is still kept
    // out of the markdown if it could break the link.
    let url = newer_url.filter(|u| is_plain_url(u));
    out.push_str(&match (url, newer_asks) {
        (Some(url), true) => format!("Please answer the [new questions]({url}) instead.\n\n"),
        (Some(url), false) => {
            format!("Soqrates found [nothing to ask]({url}) about the newer commit.\n\n")
        }
        (None, true) => "Please answer the newer Soqrates comment instead.\n\n".into(),
        (None, false) => "Soqrates found nothing to ask about the newer commit.\n\n".into(),
    });
    if let Some(session) = session {
        push_collapsed_questions(session.questions(), None, &mut out);
    }
    out
}

/// An `https` URL safe to put in a markdown link target as is.
fn is_plain_url(url: &str) -> bool {
    url.starts_with("https://")
        && url
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~:/?#@!$&'*+,;=%".contains(c))
}

/// The questions in a `<details>` block, options as plain text. With
/// `results`, the author's answers are marked.
fn push_collapsed_questions(
    questions: &[Question],
    results: Option<&[QuestionResult]>,
    out: &mut String,
) {
    let total = questions.len();
    out.push_str("<details>\n<summary>The questions</summary>\n\n");
    for (i, q) in questions.iter().enumerate() {
        let answer = results
            .and_then(|rs| rs.iter().find(|r| r.question == q.id))
            .and_then(|r| r.answer.as_ref());
        out.push_str(&format!("**Question {} of {total}**\n\n", i + 1));
        out.push_str(&inline(prompt_of(q)));
        out.push_str("\n\n");
        match &q.body {
            QuestionBody::MultipleChoice { options, .. } => {
                for (j, option) in options.iter().enumerate() {
                    let text = format!("{}. {}", option_letter(j), inline(option));
                    if answer == Some(&AnswerBody::Choice(j)) {
                        out.push_str(&format!("- <b>{text}</b> (your answer)\n"));
                    } else {
                        out.push_str(&format!("- {text}\n"));
                    }
                }
                if let Some(AnswerBody::Text(t)) = answer {
                    out.push_str(&format!("\nYour answer: {}\n", inline(t)));
                }
                out.push('\n');
            }
            QuestionBody::Open { .. } if results.is_some() => {
                out.push_str("_Open question, answered by comment._\n\n");
            }
            QuestionBody::Open { .. } => out.push_str("_Open question._\n\n"),
        }
    }
    out.push_str("</details>\n");
}

/// The session key in the marker a comment body starts with, if any.
pub fn marker_key(body: &str) -> Option<SessionKey> {
    let rest = body.trim_start().strip_prefix(MARKER_START)?;
    let (key, _) = rest.split_once(MARKER_END)?;
    (!key.is_empty() && !key.contains(char::is_whitespace)).then(|| SessionKey(key.to_string()))
}

/// Whether a question comment still takes answers: it still has the
/// "Grade my answers" box, which the graded and superseded renderings drop.
pub fn takes_answers(body: &str) -> bool {
    body.lines()
        .filter_map(task_item)
        .any(|(_, text)| text == DONE_BOX)
}

/// The answer comment for a session: the latest comment by `author_id`
/// posted after the first comment carrying `marker`, whose body starts with
/// `/soqrates answer`. Comments by anyone else are ignored, so nobody can
/// answer for the author. `comments` are `(author_id, created_at, body)` in
/// the order the forge lists them (oldest first).
pub fn select_answer_comment<'a>(
    comments: &'a [(String, DateTime<Utc>, String)],
    marker: &str,
    author_id: &str,
) -> Option<&'a str> {
    let start = comments.iter().position(|(_, _, b)| b.contains(marker))?;
    let marked_at = comments[start].1;
    comments[start + 1..]
        .iter()
        .enumerate()
        .filter(|(_, (who, at, body))| {
            who == author_id && *at >= marked_at && strip_command(body).is_some()
        })
        // The latest by creation time; ties go to the later in the list.
        .max_by_key(|(i, (_, at, _))| (*at, *i))
        .map(|(_, (_, _, body))| body.as_str())
}

/// The rest of `body` after `/soqrates answer`, if it starts with that
/// command (case-insensitive, after leading whitespace).
fn strip_command(body: &str) -> Option<&str> {
    let body = body.trim_start();
    let head = body.get(..ANSWER_COMMAND.len())?;
    if !head.eq_ignore_ascii_case(ANSWER_COMMAND) {
        return None;
    }
    let rest = &body[ANSWER_COMMAND.len()..];
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        Some(_) => None,
    }
}

/// `N. text`, `N: text` or `N) text` → `(N, text)`.
fn numbered_line(line: &str) -> Option<(usize, &str)> {
    let line = line.trim_start();
    let digits = line.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    let rest = rest
        .strip_prefix('.')
        .or_else(|| rest.strip_prefix(':'))
        .or_else(|| rest.strip_prefix(')'))?;
    let n = line[..digits].parse().ok()?;
    Some((n, rest.trim()))
}

/// A single option letter, optionally followed by `.` or `)`, within range.
fn choice_index(text: &str, options: usize) -> Option<usize> {
    let text = text.trim();
    let text = text
        .strip_suffix('.')
        .or_else(|| text.strip_suffix(')'))
        .unwrap_or(text);
    let mut chars = text.chars();
    let c = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() || !c.is_ascii_lowercase() {
        return None;
    }
    let index = (c as u8 - b'a') as usize;
    (index < options).then_some(index)
}

/// Parse an answer comment against the session's questions. Returns `None`
/// if `body` does not start with `/soqrates answer`.
///
/// Numbers are 1-based in question order; out-of-range numbers are ignored
/// and for a duplicate number the last one wins. Lines without a number
/// continue the previous answer. Answers come back in question order.
pub fn parse_answers(body: &str, questions: &[Question]) -> Option<Vec<Answer>> {
    let rest = strip_command(body)?;
    let mut texts: Vec<Option<String>> = vec![None; questions.len()];
    // The answer that unnumbered lines continue; `None` before the first
    // number and after an out-of-range one.
    let mut current: Option<usize> = None;
    for line in rest.lines() {
        if let Some((n, text)) = numbered_line(line) {
            current = (1..=questions.len()).contains(&n).then(|| n - 1);
            if let Some(i) = current {
                texts[i] = Some(text.to_string());
            }
        } else if let Some(i) = current {
            let t = texts[i].get_or_insert_with(String::new);
            t.push('\n');
            t.push_str(line.trim_end());
        }
    }
    let answers = questions
        .iter()
        .zip(texts)
        .filter_map(|(q, text)| {
            let text = text?.trim().to_string();
            let body = match &q.body {
                QuestionBody::MultipleChoice { options, .. } => choice_index(&text, options.len())
                    .map_or(AnswerBody::Text(text), AnswerBody::Choice),
                QuestionBody::Open { .. } => AnswerBody::Text(text),
            };
            Some(Answer {
                question: q.id.clone(),
                body,
            })
        })
        .collect();
    Some(answers)
}

/// Truncate to at most `max` characters, ending in `…` when cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The commit status state and description for a check status.
pub fn commit_status(status: CheckStatus, summary: &str) -> (StatusState, String) {
    let (state, description) = match status {
        CheckStatus::Pending => (StatusState::Pending, summary.to_string()),
        CheckStatus::Success => (StatusState::Success, summary.to_string()),
        CheckStatus::Failure => (StatusState::Failure, summary.to_string()),
        CheckStatus::Neutral => (StatusState::Success, format!("Advisory: {summary}")),
    };
    (state, truncate(&description, MAX_DESCRIPTION_CHARS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use soqrates_core::{AuthorId, ConceptId, ConceptScope, Grade, QuestionId};

    fn mc(id: &str, options: &[&str], correct: usize) -> Question {
        Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            body: QuestionBody::MultipleChoice {
                prompt: format!("Prompt {id}?"),
                options: options.iter().map(|o| o.to_string()).collect(),
                correct,
            },
            explanation: "SECRET-EXPLANATION".into(),
        }
    }

    fn open(id: &str) -> Question {
        Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            body: QuestionBody::Open {
                prompt: format!("Explain {id}."),
                rubric: "SECRET-RUBRIC mentions lifetimes".into(),
            },
            explanation: "SECRET-EXPLANATION".into(),
        }
    }

    fn questions() -> Vec<Question> {
        vec![
            mc("q1", &["first", "SECRET-CORRECT", "third"], 1),
            open("q2"),
            mc("q3", &["x", "y"], 0),
        ]
    }

    fn session() -> Session {
        Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github.com:42".into()),
            questions(),
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap()
    }

    fn answer(id: &str, body: AnswerBody) -> Answer {
        Answer {
            question: QuestionId(id.into()),
            body,
        }
    }

    fn text(s: &str) -> AnswerBody {
        AnswerBody::Text(s.into())
    }

    // @adr-0004-d3 @adr-0004-d4
    #[test]
    fn render_contains_marker_boxes_and_collapsed_instructions() {
        let s = render_questions(&session());
        assert!(s.starts_with("<!-- soqrates:session=o/r#1@abc -->\n> [!NOTE]\n"));
        assert!(s.contains("### Question 1 of 3\n\nPrompt q1?\n\n"));
        assert!(s.contains("- [ ] a. first\n- [ ] b. SECRET-CORRECT\n- [ ] c. third\n"));
        assert!(s.contains("### Question 2 of 3\n\nExplain q2."));
        assert!(s.contains("Open question: answer it in your own words"));
        assert!(s.contains("advisory"));
        // The done box comes last, separated from the questions.
        assert!(s.contains("---\n\n- [ ] **Grade my answers**\n\n<details>"));
        let details = &s[s.find("<details>").unwrap()..];
        assert!(details.starts_with("<details>\n<summary>How to answer</summary>"));
        assert!(details.trim_end().ends_with("</details>"));
        assert!(
            details.contains("```\n/soqrates answer\n1. b\n2. Your answer in your own words\n```")
        );
        assert!(details.contains("Only answers from the author of this pull request count"));
        assert!(!details.contains("[ ]"));
        // Every box but the done box is an option.
        assert_eq!(s.matches("- [ ] ").count(), 3 + 2 + 1);
    }

    // @adr-0004-d1 @adr-0004-d3
    #[test]
    fn render_never_reveals_correct_option_rubric_or_explanation() {
        let mut q = questions();
        // Make the correct option text unique to detect any leak of the index.
        q[0] = mc("q1", &["first", "second", "third"], 1);
        let s = Session::open(
            SessionKey("k".into()),
            AuthorId("github.com:42".into()),
            q,
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let rendered = render_questions(&s);
        assert!(!rendered.contains("SECRET-RUBRIC"));
        assert!(!rendered.contains("SECRET-EXPLANATION"));
        assert!(!rendered.to_lowercase().contains("correct"));
        assert!(!rendered.contains("rubric"));
        // The default session's correct option is shown only as an option.
        let rendered = render_questions(&session());
        assert_eq!(rendered.matches("SECRET-CORRECT").count(), 1);
        assert!(!rendered.contains("SECRET-RUBRIC"));
        assert!(!rendered.contains("SECRET-EXPLANATION"));
    }

    #[test]
    fn parses_all_separators_letters_and_case() {
        let body = "/SoQrates Answer\n1) B\n2: borrowing ends\n3. a";
        assert_eq!(
            parse_answers(body, &questions()).unwrap(),
            vec![
                answer("q1", AnswerBody::Choice(1)),
                answer("q2", text("borrowing ends")),
                answer("q3", AnswerBody::Choice(0)),
            ]
        );
    }

    #[test]
    fn multi_line_open_answers_continue() {
        let body = "  \n /soqrates answer\n2. first line\n   second line\n\nthird line\n1. c";
        assert_eq!(
            parse_answers(body, &questions()).unwrap(),
            vec![
                answer("q1", AnswerBody::Choice(2)),
                answer("q2", text("first line\n   second line\n\nthird line")),
            ]
        );
    }

    #[test]
    fn non_letter_choice_is_text_and_open_letter_is_text() {
        let body = "/soqrates answer\n1. the second one\n2. b\n3. z";
        assert_eq!(
            parse_answers(body, &questions()).unwrap(),
            vec![
                answer("q1", text("the second one")),
                answer("q2", text("b")),
                answer("q3", text("z")),
            ]
        );
        let body = "/soqrates answer\n1. b.\n3. (a)";
        assert_eq!(
            parse_answers(body, &questions()).unwrap(),
            vec![
                answer("q1", AnswerBody::Choice(1)),
                answer("q3", text("(a)")),
            ]
        );
    }

    #[test]
    fn out_of_range_ignored_and_duplicates_last_wins() {
        let body = "/soqrates answer\n0. a\n4. b\n   continues nothing\n1. a\n1. c";
        assert_eq!(
            parse_answers(body, &questions()).unwrap(),
            vec![answer("q1", AnswerBody::Choice(2))]
        );
    }

    #[test]
    fn rejects_text_before_command() {
        assert_eq!(
            parse_answers("hi\n/soqrates answer\n1. a", &questions()),
            None
        );
        assert_eq!(
            parse_answers("I said /soqrates answer 1. a", &questions()),
            None
        );
        assert_eq!(parse_answers("/soqrates answers\n1. a", &questions()), None);
        assert_eq!(parse_answers("/soqrates", &questions()), None);
        assert_eq!(
            parse_answers("/soqrates answer", &questions()),
            Some(vec![])
        );
    }

    #[test]
    fn answers_on_the_command_line_count() {
        assert_eq!(
            parse_answers("/soqrates answer 1. a", &questions()).unwrap(),
            vec![answer("q1", AnswerBody::Choice(0))]
        );
    }

    fn at(s: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(s, 0).unwrap()
    }

    fn c(who: &str, t: i64, body: &str) -> (String, DateTime<Utc>, String) {
        (who.into(), at(t), body.into())
    }

    #[test]
    fn selects_latest_answer_of_the_author_after_the_marker() {
        let m = session_marker(&SessionKey("k".into()));
        let me = "github.com:42";
        let comments = vec![
            c(me, 1, "/soqrates answer\n1. a"), // before the marker
            c("github.com:7", 2, &format!("{m}\nquestions")),
            c(me, 3, "/soqrates answer\n1. b"),
            c("github.com:99", 4, "/soqrates answer\n1. c"), // someone else
            c(me, 5, "thanks!"),
            c(me, 6, "  /SOQRATES answer\n1. c"),
            c("github.com:99", 7, "/soqrates answer\n1. a"),
        ];
        assert_eq!(
            select_answer_comment(&comments, &m, me),
            Some("  /SOQRATES answer\n1. c")
        );
    }

    #[test]
    fn nobody_else_can_answer_for_the_author() {
        let m = session_marker(&SessionKey("k".into()));
        let comments = vec![
            c("github.com:7", 1, &m),
            c("github.com:99", 2, "/soqrates answer\n1. b"),
            // Same numeric id on another host is another identity.
            c("ghe.example.com:42", 3, "/soqrates answer\n1. b"),
        ];
        assert_eq!(select_answer_comment(&comments, &m, "github.com:42"), None);
    }

    #[test]
    fn no_marker_means_no_answers() {
        let m = session_marker(&SessionKey("k".into()));
        let other = session_marker(&SessionKey("k2".into()));
        let comments = vec![
            c("github.com:7", 1, &other),
            c("github.com:42", 2, "/soqrates answer\n1. b"),
        ];
        assert_eq!(select_answer_comment(&comments, &m, "github.com:42"), None);
    }

    #[test]
    fn status_mapping_and_truncation() {
        assert_eq!(
            commit_status(CheckStatus::Pending, "Waiting."),
            (StatusState::Pending, "Waiting.".into())
        );
        assert_eq!(
            commit_status(CheckStatus::Success, "ok").0,
            StatusState::Success
        );
        assert_eq!(
            commit_status(CheckStatus::Failure, "no").0,
            StatusState::Failure
        );
        assert_eq!(
            commit_status(CheckStatus::Neutral, "1 of 2 answers correct."),
            (
                StatusState::Success,
                "Advisory: 1 of 2 answers correct.".into()
            )
        );
        let long = "é".repeat(300);
        let (_, d) = commit_status(CheckStatus::Neutral, &long);
        assert_eq!(d.chars().count(), MAX_DESCRIPTION_CHARS);
        assert!(d.starts_with("Advisory: "));
        assert!(d.ends_with('…'));
        assert_eq!(truncate(&"x".repeat(140), 140), "x".repeat(140));
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn hosts_and_ids() {
        assert_eq!(host_of("https://github.com").as_deref(), Some("github.com"));
        assert_eq!(
            host_of("https://GHE.example.com:8443/path").as_deref(),
            Some("ghe.example.com")
        );
        assert_eq!(host_of("https://").as_deref(), None);
        assert_eq!(author_id("github.com", 12345), "github.com:12345");
        assert_eq!(split_repo("o/r"), Some(("o", "r")));
        assert_eq!(split_repo("o"), None);
        assert_eq!(split_repo("o/r/x"), None);
        assert_eq!(split_repo("/r"), None);
    }

    // --- ADR-0004 -----------------------------------------------------------

    /// Tick the box on the first line equal to `line`.
    fn tick(body: &str, line: &str) -> String {
        let ticked = line.replacen("[ ]", "[x]", 1);
        let at = body.find(&format!("{line}\n")).expect("box to tick");
        format!("{}{}{}", &body[..at], ticked, &body[at + line.len()..])
    }

    fn checkboxes(done: bool, ticked: &[Ticked]) -> Checkboxes {
        Checkboxes {
            done,
            ticked: ticked.to_vec(),
        }
    }

    // @adr-0004-d3
    #[test]
    fn reads_ticked_boxes_by_section_and_position() {
        let body = render_questions(&session());
        let boxes = parse_checkboxes(&body, &questions());
        assert_eq!(boxes, checkboxes(false, &[Ticked::None; 3]));
        assert!(!boxes.any());

        let body = tick(&body, "- [ ] b. SECRET-CORRECT");
        let body = tick(&body, "- [ ] a. x");
        let body = tick(&body, "- [ ] **Grade my answers**");
        assert_eq!(
            parse_checkboxes(&body, &questions()),
            checkboxes(true, &[Ticked::One(1), Ticked::None, Ticked::One(0)])
        );
    }

    // @adr-0004-d3
    #[test]
    fn box_syntax_variants_are_read() {
        let body = "### Question 1 of 3\n\nP\n\n  * [ ] a. first\n- [X] b. renamed by hand\n- [ ] c. third\n\n\
                    ### Question 3 of 3\n\n   -  [x]  a. x\n+ [ ] b. y\n\n---\n\n * [X] **Grade my answers**\n";
        assert_eq!(
            parse_checkboxes(body, &questions()),
            checkboxes(true, &[Ticked::One(1), Ticked::None, Ticked::One(0)])
        );
        // Not boxes: `[x]` not followed by a space, or no list marker.
        let body = "### Question 1 of 3\n- [x]a. first\n[x] b. second\n-[x] c. third\n";
        assert_eq!(
            parse_checkboxes(body, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    // @adr-0004-d3
    #[test]
    fn boxes_outside_question_sections_or_in_instructions_are_ignored() {
        let body = "- [x] a. before any question\n### Question 9 of 3\n- [x] a. out of range\n\
                    ### Question 2 of 3\n- [x] a. open question has no options\n---\n- [x] a. after the separator\n\
                    <details>\n- [x] **Grade my answers**\n</details>\n";
        assert_eq!(
            parse_checkboxes(body, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    // @adr-0004-d3
    #[test]
    fn several_ticked_options_are_wrong_and_none_is_unanswered() {
        let body = render_questions(&session());
        let body = tick(&body, "- [ ] a. first");
        let body = tick(&body, "- [ ] c. third");
        let body = tick(&body, "- [ ] **Grade my answers**");
        let boxes = parse_checkboxes(&body, &questions());
        assert_eq!(boxes.ticked, vec![Ticked::Many, Ticked::None, Ticked::None]);
        assert_eq!(
            merge_answers(&questions(), Some(&boxes), None),
            Some(vec![answer("q1", text(MULTIPLE_TICKED))])
        );
    }

    // @adr-0004-d3
    #[test]
    fn boxes_count_only_once_the_done_box_is_ticked() {
        let body = render_questions(&session());
        let body = tick(&body, "- [ ] b. SECRET-CORRECT");
        let boxes = parse_checkboxes(&body, &questions());
        assert!(boxes.any() && !boxes.done);
        assert_eq!(merge_answers(&questions(), Some(&boxes), None), None);
        let body = tick(&body, "- [ ] **Grade my answers**");
        let boxes = parse_checkboxes(&body, &questions());
        assert_eq!(
            merge_answers(&questions(), Some(&boxes), None),
            Some(vec![answer("q1", AnswerBody::Choice(1))])
        );
        // Done with nothing ticked submits no answers.
        let boxes = checkboxes(true, &[Ticked::None; 3]);
        assert_eq!(
            merge_answers(&questions(), Some(&boxes), None),
            Some(vec![])
        );
    }

    // @adr-0004-d3
    #[test]
    fn boxes_are_ignored_unless_the_author_edited_last() {
        // `boxes` is `None` when the last editor is not the author: then a
        // ticked done box submits nothing, and only the comment counts.
        assert_eq!(merge_answers(&questions(), None, None), None);
        let comment = parse_answers("/soqrates answer\n2. because", &questions());
        assert_eq!(
            merge_answers(&questions(), None, comment),
            Some(vec![answer("q2", text("because"))])
        );
        // The editor check itself: only a user whose id is the author's.
        let data = |editor: serde_json::Value| serde_json::json!({ "node": { "editor": editor } });
        assert_eq!(
            editor_database_id(&data(
                serde_json::json!({"__typename": "User", "databaseId": 42})
            )),
            Some(42)
        );
        assert_eq!(editor_database_id(&data(serde_json::Value::Null)), None);
        assert_eq!(
            editor_database_id(&data(serde_json::json!({"__typename": "Bot"}))),
            None
        );
        assert_eq!(
            editor_database_id(&serde_json::json!({ "node": null })),
            None
        );
        assert!(EDITOR_QUERY.contains("... on IssueComment"));
        assert!(EDITOR_QUERY.contains("editor"));
        assert!(EDITOR_QUERY.contains("databaseId"));
    }

    // @adr-0004-d3
    #[test]
    fn a_comment_answer_overrides_the_boxes_and_answers_open_questions() {
        let boxes = checkboxes(false, &[Ticked::One(0), Ticked::None, Ticked::Many]);
        // Without the done box, an answer comment submits, boxes included.
        let comment = parse_answers("/soqrates answer\n1. b\n2. it moves", &questions());
        assert_eq!(
            merge_answers(&questions(), Some(&boxes), comment),
            Some(vec![
                answer("q1", AnswerBody::Choice(1)),
                answer("q2", text("it moves")),
                answer("q3", text(MULTIPLE_TICKED)),
            ])
        );
    }

    // @adr-0004-d3
    #[test]
    fn graphql_base_for_github_and_enterprise() {
        assert_eq!(
            graphql_base("https://api.github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            graphql_base("https://ghe.example.com/api/v3/"),
            "https://ghe.example.com/api"
        );
    }

    fn result(
        q: &Question,
        answer: Option<AnswerBody>,
        grade: Grade,
        feedback: &str,
    ) -> QuestionResult {
        let (prompt, options, correct_option) = match &q.body {
            QuestionBody::MultipleChoice {
                prompt,
                options,
                correct,
            } => (prompt.clone(), options.clone(), Some(*correct)),
            QuestionBody::Open { prompt, .. } => (prompt.clone(), vec![], None),
        };
        QuestionResult {
            question: q.id.clone(),
            concept: q.concept.clone(),
            prompt,
            options,
            correct_option,
            answer,
            grade,
            feedback: feedback.into(),
            next_review: Some(Utc.with_ymd_and_hms(2031, 2, 3, 4, 5, 6).unwrap()),
        }
    }

    fn results() -> Vec<QuestionResult> {
        let q = questions();
        vec![
            result(
                &q[0],
                Some(AnswerBody::Choice(1)),
                Grade::Good,
                "Because `b` borrows.",
            ),
            result(
                &q[1],
                Some(text("it moves")),
                Grade::Again,
                "Moves end the borrow.",
            ),
            result(&q[2], None, Grade::Again, "Not answered. x is right."),
        ]
    }

    // @adr-0004-d1 @adr-0004-d2
    #[test]
    fn results_show_each_question_and_never_the_profile() {
        let s = render_results(&session(), &results(), false);
        assert!(s.starts_with("<!-- soqrates:results=o/r#1@abc -->\n> [!WARNING]\n"));
        assert!(s.contains("**Soqrates: 1 of 3 correct.**"));
        assert!(!s.contains(&session_marker(session().key())));
        assert!(s.contains("### Question 1 of 3 ✅\n\nPrompt q1?\n\n- **Your answer:** b. SECRET-CORRECT\n- **Correct answer:** b. SECRET-CORRECT\n"));
        assert!(s.contains(
            "<details>\n<summary>Why</summary>\n\nBecause <code>b</code> borrows.\n\n</details>"
        ));
        assert!(s.contains("### Question 2 of 3 ❌\n\nExplain q2.\n\n- **Your answer:** it moves\n\n**Why:** Moves end the borrow."));
        assert!(!s.contains("Question 2 of 3 ❌\n\nExplain q2.\n\n- **Your answer:** it moves\n- **Correct answer:**"));
        assert!(s.contains("- **Your answer:** _No answer_\n- **Correct answer:** a. x\n"));
        // D2: nothing from the profile: no review date, level, concept ids.
        for leak in [
            "2031",
            "Level",
            "level",
            "next review",
            "review on",
            "c.q1",
            "c.q2",
            "SECRET-RUBRIC",
        ] {
            assert!(!s.contains(leak), "results leak {leak:?}:\n{s}");
        }
        let s = render_results(&session(), &results()[..1], true);
        assert!(s.contains("> [!TIP]\n> **Soqrates: 1 of 1 correct.**"));
    }

    // @adr-0004-d5
    #[test]
    fn graded_question_comment_is_frozen_with_questions_collapsed() {
        let mut r = results();
        r[2].answer = Some(text(MULTIPLE_TICKED));
        let s = render_graded_questions(&session(), &r);
        assert!(s.starts_with(
            "<!-- soqrates:session=o/r#1@abc -->\n> [!NOTE]\n> **Soqrates learning check: graded.**"
        ));
        for line in s.lines() {
            assert!(
                task_item(line).is_none(),
                "live box in graded comment: {line:?}"
            );
        }
        assert!(!s.contains("[ ]") && !s.contains("[x]") && !s.contains("Grade my answers"));
        let details = &s[s
            .find("<details>\n<summary>The questions</summary>")
            .unwrap()..];
        assert!(details.trim_end().ends_with("</details>"));
        for text in [
            "Prompt q1?",
            "- a. first",
            "- <b>b. SECRET-CORRECT</b> (your answer)",
            "Explain q2.",
            "Prompt q3?",
            "- a. x\n- b. y",
            "Your answer: multiple options ticked",
        ] {
            assert!(details.contains(text), "missing {text:?}:\n{s}");
        }
        assert!(!s.contains("SECRET-EXPLANATION") && !s.contains("SECRET-RUBRIC"));
        // Nothing to read back from it.
        assert_eq!(
            parse_checkboxes(&s, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    const NEWER: &str = "https://github.com/o/r/pull/1#issuecomment-99";

    // @adr-0004-d7
    #[test]
    fn superseded_question_comment_links_the_new_questions_and_has_no_boxes() {
        let s = render_superseded_questions(session().key(), Some(&session()), Some(NEWER), true);
        assert!(s.starts_with(
            "<!-- soqrates:session=o/r#1@abc -->\n> [!NOTE]\n> **Soqrates learning check: superseded.**"
        ));
        assert!(s.contains(&format!("[new questions]({NEWER})")), "{s}");
        for line in s.lines() {
            assert!(task_item(line).is_none(), "live box: {line:?}");
        }
        assert!(!takes_answers(&s));
        assert_eq!(marker_key(&s), Some(session().key().clone()));
        let details = &s[s.find("<details>").unwrap()..];
        for text in [
            "Prompt q1?",
            "- a. first",
            "- b. SECRET-CORRECT",
            "Explain q2.",
        ] {
            assert!(details.contains(text), "missing {text:?}:\n{s}");
        }
        assert!(!s.contains("your answer") && !s.contains("SECRET-EXPLANATION"));
        assert!(!s.contains("SECRET-RUBRIC"));
        assert_eq!(
            parse_checkboxes(&s, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    // @adr-0004-d7
    #[test]
    fn superseded_without_session_or_safe_link_still_freezes() {
        let key = SessionKey("o/r#1@old".into());
        let s = render_superseded_questions(&key, None, None, true);
        assert!(s.contains("superseded.**") && !s.contains("<details>"));
        assert!(s.contains("newer Soqrates comment"));
        assert_eq!(marker_key(&s), Some(key.clone()));
        let s = render_superseded_questions(&key, Some(&session()), Some(NEWER), false);
        assert!(s.contains(&format!("[nothing to ask]({NEWER}) about the newer commit")));
        assert!(!s.contains("answer the"), "{s}");
        assert!(!takes_answers(&s));
        for bad in ["javascript:alert(1)", "https://x/a)[b](c", "https://x/<b>"] {
            let s = render_superseded_questions(&key, None, Some(bad), true);
            assert!(!s.contains(bad) && !s.contains("]("), "{s}");
        }
    }

    // @adr-0004-d7
    #[test]
    fn a_round_with_nothing_to_ask_is_a_short_note_without_boxes() {
        let empty = Session::open(
            SessionKey("o/r#1@typo".into()),
            AuthorId("github.com:42".into()),
            vec![],
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let s = render_questions(&empty);
        assert_eq!(
            s,
            "<!-- soqrates:session=o/r#1@typo -->\n> [!NOTE]\n> **Soqrates learning check: \
             nothing to ask.** Soqrates found no questions to ask about this commit, so there \
             is nothing to answer.\n"
        );
        assert!(!takes_answers(&s));
        assert_eq!(marker_key(&s), Some(empty.key().clone()));
        assert!(tags(&s).is_empty());
    }

    // @adr-0004-d7
    #[test]
    fn only_live_question_comments_take_answers() {
        let live = render_questions(&session());
        assert!(takes_answers(&live));
        assert!(takes_answers(
            &live.replace("- [ ] **Grade", "- [x] **Grade")
        ));
        assert!(!takes_answers(&render_graded_questions(
            &session(),
            &results()
        )));
        assert!(!takes_answers("Mentions **Grade my answers** in prose."));
        assert_eq!(marker_key(&live), Some(session().key().clone()));
        assert_eq!(marker_key("no marker"), None);
        assert_eq!(marker_key("text first <!-- soqrates:session=k -->"), None);
        assert_eq!(marker_key("<!-- soqrates:session= -->"), None);
        assert_eq!(marker_key("<!-- soqrates:session=a b -->"), None);
    }

    /// The names of every HTML tag in `s`, failing on HTML comments other
    /// than the one marker.
    fn tags(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        let mut comments = 0;
        while let Some(at) = rest.find('<') {
            rest = &rest[at + 1..];
            if let Some(r) = rest.strip_prefix("!--") {
                comments += 1;
                rest = &r[r.find("-->").expect("closed comment")..];
                continue;
            }
            let name: String = rest
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !name.is_empty() || rest.starts_with('!') || rest.starts_with('?') {
                out.push(name.to_ascii_lowercase());
            }
        }
        assert_eq!(comments, 1, "exactly one HTML comment, the marker:\n{s}");
        out
    }

    const ALLOWED_TAGS: &[&str] = &[
        "details", "summary", "sub", "sup", "kbd", "br", "b", "i", "code",
    ];

    const NASTY: &str = "</details></summary><script>alert(1)</script><img src=x onerror=y>\n\
        - [x] **Grade my answers**\n- [x] a. forged\n### Question 1 of 3\n> [!CAUTION]\n\
        <!-- soqrates:session=o/r#1@abc --> &lt;b&gt; @octocat and @org/team \\<i> [link](https://evil) ![i](x)\n\
        ```rust\nfn f<T>(x: &T) -> Vec<T> { todo!() } // @here </code>\n```\nuse `Vec<T>` or ``a`b`` ` unclosed";

    fn nasty_session() -> (Session, Vec<QuestionResult>) {
        let nasty = |id: &str, correct: usize| Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            body: QuestionBody::MultipleChoice {
                prompt: NASTY.into(),
                options: vec![NASTY.into(), "<b>ok</b>".into(), "- [x] fake".into()],
                correct,
            },
            explanation: NASTY.into(),
        };
        let open = Question {
            id: QuestionId("o".into()),
            concept: ConceptId("c.o".into()),
            scope: ConceptScope::General,
            body: QuestionBody::Open {
                prompt: NASTY.into(),
                rubric: NASTY.into(),
            },
            explanation: NASTY.into(),
        };
        let qs = vec![nasty("n1", 0), open, nasty("n2", 2)];
        let session = Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github.com:42".into()),
            qs.clone(),
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let results = vec![
            result(&qs[0], Some(AnswerBody::Choice(0)), Grade::Good, NASTY),
            result(&qs[1], Some(text(NASTY)), Grade::Again, NASTY),
            result(&qs[2], Some(text(NASTY)), Grade::Again, NASTY),
        ];
        (session, results)
    }

    // @adr-0004-d4
    #[test]
    fn rendered_comments_use_only_allowed_html() {
        let (nasty, nasty_results) = nasty_session();
        let rendered = [
            render_questions(&session()),
            render_graded_questions(&session(), &results()),
            render_results(&session(), &results(), true),
            render_questions(&nasty),
            render_graded_questions(&nasty, &nasty_results),
            render_results(&nasty, &nasty_results, false),
            render_superseded_questions(session().key(), Some(&session()), Some(NEWER), true),
            render_superseded_questions(nasty.key(), Some(&nasty), Some(NEWER), true),
        ];
        for s in &rendered {
            let tags = tags(s);
            for tag in &tags {
                assert!(ALLOWED_TAGS.contains(&tag.as_str()), "tag <{tag}> in:\n{s}");
            }
            // Every <details> and <summary> is closed by the renderer, and
            // no model text closes one early.
            for t in ["details", "summary", "code", "b"] {
                let open = tags.iter().filter(|x| *x == t).count();
                let close = s.matches(&format!("</{t}>")).count();
                assert_eq!(open, 2 * close, "unbalanced <{t}> in:\n{s}");
            }
        }
    }

    // @adr-0004-d4
    #[test]
    fn model_text_cannot_forge_structure_or_mention() {
        let (nasty, nasty_results) = nasty_session();
        for s in [
            render_questions(&nasty),
            render_graded_questions(&nasty, &nasty_results),
            render_results(&nasty, &nasty_results, false),
            render_superseded_questions(nasty.key(), Some(&nasty), Some(NEWER), true),
        ] {
            assert_eq!(s.matches("<!--").count(), 1, "one marker only:\n{s}");
            assert!(!s.contains("<script") && !s.contains("<img"));
            assert!(!s.contains("@octocat") && !s.contains("@org") && !s.contains("@here"));
            assert!(s.contains("@\u{200B}octocat"));
            assert!(!s.contains("[link](") && !s.contains("![i]("));
            assert!(!s.contains("[!CAUTION]"));
            // Model text never starts a line: no forged headings or alerts.
            for line in s.lines() {
                assert!(!line.starts_with("> [!CAUTION]") && !line.starts_with("- [x]"));
            }
            let headings = s
                .lines()
                .filter(|l| l.starts_with("### Question 1 of 3"))
                .count();
            let frozen = s.contains("graded.**") || s.contains("superseded.**");
            assert_eq!(headings, usize::from(!frozen), "{s}");
        }
        // The question comment has exactly the boxes the renderer made, and
        // none of them is ticked.
        let q = render_questions(&nasty);
        assert_eq!(
            parse_checkboxes(&q, nasty.questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
        assert_eq!(q.matches("- [ ] ").count(), 3 + 3 + 1);
        // Code stays readable.
        assert!(q.contains("<code>Vec&lt;T&gt;</code>"));
        assert!(q.contains("<code>a&#96;b</code>"));
        assert!(q.contains("<code>fn f&lt;T&gt;(x: &amp;T) -&gt; Vec&lt;T&gt; { todo!() } // @\u{200B}here &lt;/code&gt;</code>"));
        assert!(q.contains("&amp;lt;b&amp;gt;"));
        assert!(q.contains("\\\\&lt;i&gt;"));
        assert!(!q.contains("SECRET") && !q.contains("rubric"));
    }

    // @adr-0004-d4
    #[test]
    fn inline_keeps_plain_text_readable() {
        assert_eq!(
            inline("  What does `Arc<T>` add?  "),
            "What does <code>Arc&lt;T&gt;</code> add?"
        );
        assert_eq!(inline("line one\nline two"), "line one<br>line two");
        assert_eq!(inline("a **bold** claim"), "a **bold** claim");
        assert_eq!(
            inline("```\n  let x = 1;\n```"),
            "<code>&nbsp;&nbsp;let x = 1;</code>"
        );
        assert_eq!(
            inline("snake_case `my_var`"),
            "snake_case <code>my\\_var</code>"
        );
        assert!(!inline("a\n\nb\r\n- [x] c").contains('\n'));
    }
}
