//! Pure logic of the GitHub adapter: rendering the question, graded and
//! results comments, reading the author's answers from boxes, answer fields and comments,
//! and mapping check statuses (ADR-0004). No I/O, so
//! all of it is unit tested (ADR-0001 D12).

use chrono::{DateTime, Utc};
use soqrates_core::answer_command::{ANSWER_COMMAND, Command, read_command};
use soqrates_core::{
    Answer, AnswerBody, CheckStatus, Question, QuestionBody, QuestionResult, Session, SessionKey,
};

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
    q.body.prompt()
}

// ---------------------------------------------------------------------------
// Code snippets (ADR-0007 D5)

/// A fence for `code`: backticks, one more than the longest run of
/// backticks in it and at least three, so that no line of the code can
/// close it.
fn fence_for(code: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in code.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

/// A language tag safe to put after a fence: lowercase letters, digits and
/// `+#._-` only, at most 20 characters; `text` if nothing is left.
fn fence_language(language: &str) -> String {
    let tag: String = language
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "+#._-".contains(*c))
        .take(20)
        .collect::<String>()
        .to_ascii_lowercase();
    if tag.is_empty() { "text".into() } else { tag }
}

/// Append `code` as a fenced block tagged `language`, every line prefixed
/// with `indent` (to nest it in a list item). Inside the block the code is
/// shown as is and is inert: no tag, mention, box or marker in it means
/// anything. Carriage returns and control characters other than tabs are
/// dropped.
fn push_fenced(code: &str, language: &str, indent: &str, out: &mut String) {
    let code: String = code
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect();
    let fence = fence_for(&code);
    out.push_str(&format!("{indent}{fence}{}\n", fence_language(language)));
    for line in code.trim_end().lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("{indent}{fence}\n"));
}

/// The question's snippet as a fenced block, followed by a blank line.
fn push_snippet(q: &Question, out: &mut String) {
    if let Some(code) = q.body.code() {
        push_fenced(&code.text, &code.language, "", out);
        out.push('\n');
    }
}

/// One option after its list marker (`- [ ] `, `- `): `a. text`, or for a
/// code option `a.` with the code in a fenced block nested in the item.
fn option_item(index: usize, option: &str, language: Option<&str>, bold: bool) -> String {
    let letter = if bold {
        format!("<b>{}.</b>", option_letter(index))
    } else {
        format!("{}.", option_letter(index))
    };
    match language {
        Some(language) => {
            let mut out = format!("{letter}\n");
            push_fenced(option, language, "  ", &mut out);
            out.pop();
            out
        }
        None if bold => format!("<b>{}. {}</b>", option_letter(index), inline(option)),
        None => format!("{letter} {}", inline(option)),
    }
}

// ---------------------------------------------------------------------------
// Answer fields (ADR-0004 D11)

/// The hidden line that starts the answer field of a question, followed by
/// its 1-based number and [`MARKER_END`].
const FIELD_START: &str = "<!-- soqrates:answer=";
/// The hidden line that ends it, likewise.
const FIELD_END: &str = "<!-- soqrates:answer-end=";

/// The marker line `prefix` `n` [`MARKER_END`].
fn field_marker(prefix: &str, n: usize) -> String {
    format!("{prefix}{n}{MARKER_END}")
}

/// The number in a field marker line starting with `prefix`, if `line` is
/// one (leading and trailing whitespace allowed).
fn field_number(line: &str, prefix: &str) -> Option<usize> {
    let rest = line.trim().strip_prefix(prefix)?.strip_suffix(MARKER_END)?;
    (!rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

/// A question comment split into what Soqrates reads from it.
struct Scanned<'a> {
    /// The lines outside fenced code blocks opened with backticks and
    /// outside answer fields, fence lines and field markers excluded.
    outside: Vec<&'a str>,
    /// The answer fields: the number in their markers and the lines between
    /// them, in order.
    fields: Vec<(usize, Vec<&'a str>)>,
}

/// Split `body` as GitHub's markdown sees it. Only the renderer opens fenced
/// blocks outside answer fields (prose has its backticks escaped), so this
/// is how reading a comment ignores the code in it (ADR-0007 D5).
///
/// A field starts at a start marker line outside code and ends at the first
/// end marker line with the same number. Everything between the two is the
/// author's text, fences and markers included, so nothing typed there can
/// tick a box, start a question or hide the rest of the comment (ADR-0004
/// D11). A start marker without its end marker starts no field.
fn scan_comment(body: &str) -> Scanned<'_> {
    let lines: Vec<&str> = body.lines().collect();
    let mut scanned = Scanned {
        outside: Vec::new(),
        fields: Vec::new(),
    };
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim_start();
        let run = trimmed.chars().take_while(|&c| c == '`').count();
        match open {
            Some(n) if run >= n && trimmed[run..].trim().is_empty() => open = None,
            Some(_) => {}
            None if run >= 3 => open = Some(run),
            None => {
                let Some(n) = field_number(line, FIELD_START) else {
                    scanned.outside.push(line);
                    continue;
                };
                if let Some(len) = lines[i..]
                    .iter()
                    .position(|l| field_number(l, FIELD_END) == Some(n))
                {
                    scanned.fields.push((n, lines[i..i + len].to_vec()));
                    i += len + 1;
                }
            }
        }
    }
    scanned
}

/// The lines of `body` that make up its structure, as Soqrates reads it:
/// outside fenced code and outside answer fields (see [`scan_comment`]).
fn structure_lines(body: &str) -> Vec<&str> {
    scan_comment(body).outside
}

/// The text in the answer field of each question of a question comment, in
/// question order: `None` for a multiple-choice question, and for a field
/// that is missing or blank. If a question has more than one field, the
/// first counts.
fn parse_fields(body: &str, questions: &[Question]) -> Vec<Option<String>> {
    let mut fields: Vec<Option<String>> = vec![None; questions.len()];
    let mut seen = vec![false; questions.len()];
    for (n, lines) in scan_comment(body).fields {
        let Some(i) = n.checked_sub(1).filter(|i| *i < questions.len()) else {
            continue;
        };
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        if !matches!(questions[i].body, QuestionBody::Open { .. }) {
            continue;
        }
        let text = lines.join("\n");
        let text = text.trim();
        fields[i] = (!text.is_empty()).then(|| text.to_string());
    }
    fields
}

/// The empty answer field of question `n`, with the line that says how to
/// fill it.
fn push_field(n: usize, out: &mut String) {
    out.push_str(&format!(
        "_Open question: edit this comment and write your answer in your own \
         words below this line, between `soqrates:answer={n}` and \
         `soqrates:answer-end={n}`._\n\n"
    ));
    out.push_str(&field_marker(FIELD_START, n));
    out.push_str("\n\n");
    out.push_str(&field_marker(FIELD_END, n));
    out.push_str("\n\n");
}

const HOW_TO_ANSWER: &str = "<details>
<summary>How to answer</summary>

- **Multiple choice:** tick the box of one option per question.
- **Open questions:** edit this comment (**…** → **Edit**) and write your \
answer under the question, between the `soqrates:answer` and \
`soqrates:answer-end` lines. Keep those two lines, and save.
- **Then tick Grade my answers.** It submits everything in this comment at \
once, so tick it last.
- **Can't edit this comment?** Reply to this pull request with a comment \
that starts with `/soqrates answer`, followed by one line per question: its \
number, then your answer (the letter of an option for multiple choice). An \
answer may span several lines. Posting it submits all your answers, ticked \
boxes and answer fields included. An answer in the comment replaces the \
boxes or the answer field of that question.

```
/soqrates answer
1. b
2. Your answer in your own words
```

Only answers from the author of this pull request count: if anyone else \
edited this comment last, its boxes and answer fields are ignored. Until \
your answers are graded, you can still edit your answer comment, and a \
newer answer comment replaces an earlier one. After grading, answers for \
this commit are final; a new push asks new questions.

</details>
";

/// The Markdown comment putting the session's questions to the author
/// (ADR-0004 D3, D4, D11). Multiple-choice options are task-list boxes, each
/// open question has an empty answer field, and the "Grade my answers" box
/// comes last. It never contains the correct option, the
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
        push_snippet(q, &mut out);
        match &q.body {
            QuestionBody::MultipleChoice { options, .. } => {
                let language = q.body.options_language();
                for (j, option) in options.iter().enumerate() {
                    out.push_str(&format!(
                        "- [ ] {}\n",
                        option_item(j, option, language, false)
                    ));
                }
                out.push('\n');
            }
            QuestionBody::Open { .. } => push_field(i + 1, &mut out),
        }
    }
    out.push_str("---\n\n");
    out.push_str(&format!("- [ ] {DONE_BOX}\n\n"));
    out.push_str(HOW_TO_ANSWER);
    out
}

// ---------------------------------------------------------------------------
// Reading the boxes and answer fields (ADR-0004 D3, D11)

/// The ticked options of one multiple-choice question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ticked {
    None,
    One(usize),
    Many,
}

/// The answers given in the question comment itself: its boxes and its
/// answer fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InPlaceAnswers {
    /// Whether "Grade my answers" is ticked.
    pub done: bool,
    /// Per question, in question order; always `Ticked::None` for open
    /// questions.
    pub ticked: Vec<Ticked>,
    /// The text in each question's answer field, in question order; always
    /// `None` for multiple-choice questions (ADR-0004 D11).
    pub fields: Vec<Option<String>>,
}

impl InPlaceAnswers {
    /// Whether any box at all is ticked, or any answer field filled in.
    pub fn any(&self) -> bool {
        self.done
            || self.ticked.iter().any(|t| *t != Ticked::None)
            || self.fields.iter().any(Option::is_some)
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

/// Read the boxes and answer fields of a question comment rendered by
/// [`render_questions`]. A box belongs to the question whose section it is
/// in, and an option is identified by its position in that section, never
/// by its text. A field belongs to the question numbered in its markers.
/// The "How to answer" section and anything after it are ignored, and so is
/// everything inside fenced code blocks (ADR-0007 D5) and, for boxes,
/// everything typed into answer fields (ADR-0004 D11).
pub fn parse_in_place(body: &str, questions: &[Question]) -> InPlaceAnswers {
    let mut boxes = InPlaceAnswers {
        done: false,
        ticked: vec![Ticked::None; questions.len()],
        fields: parse_fields(body, questions),
    };
    // The current question, and the position of the next box in it.
    let mut current: Option<(usize, usize)> = None;
    for line in structure_lines(body) {
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

/// Combine the question comment's boxes and answer fields with the answer
/// comment into the author's answers (ADR-0004 D3, D11). `boxes` must be
/// `None` unless the question comment's last editor is the PR author;
/// `comment` is the parsed `/soqrates answer` comment, if any. Returns
/// `None` until the author has submitted, by ticking "Grade my answers" or
/// by posting an answer comment.
///
/// Per question, an answer in the comment wins; otherwise one ticked option
/// is that choice, several are wrong and none is unanswered, and an open
/// question is answered by the text in its answer field.
pub fn merge_answers(
    questions: &[Question],
    boxes: Option<&InPlaceAnswers>,
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
            let boxes = boxes?;
            if let Some(text) = &boxes.fields[i] {
                return Some(Answer {
                    question: q.id.clone(),
                    body: AnswerBody::Text(text.clone()),
                });
            }
            let body = match boxes.ticked[i] {
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

/// Option `index` in a list item; a code option as code (ADR-0007 D5).
fn option_text(options: &[String], index: usize, language: Option<&str>) -> String {
    match options.get(index) {
        Some(o) => option_item(index, o, language, false),
        None => format!("option {}", index + 1),
    }
}

fn answer_text(result: &QuestionResult, language: Option<&str>) -> String {
    match &result.answer {
        None => "_No answer_".into(),
        Some(AnswerBody::Choice(i)) => option_text(&result.options, *i, language),
        Some(AnswerBody::Text(t)) if t.trim().is_empty() => "_No answer_".into(),
        Some(AnswerBody::Text(t)) => inline(t),
    }
}

/// The comment with the results of a graded session (ADR-0004 D1, D2).
/// It takes the results and the pass flag only, so nothing from the
/// learner's profile (level, review schedule, other concepts) can appear.
/// With `source`, it says which answer comment and boxes it graded
/// (ADR-0004 D8).
pub fn render_results(
    session: &Session,
    results: &[QuestionResult],
    passed: bool,
    source: Option<&AnswerSource>,
) -> String {
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
    if let Some(source) = source {
        out.push_str(&format!("<sub>{}</sub>\n\n", describe_source(source)));
    }
    for (i, r) in results.iter().enumerate() {
        let mark = if r.is_correct() { "✅" } else { "❌" };
        out.push_str(&format!("{} {mark}\n\n", question_heading(i + 1, total)));
        out.push_str(&inline(&r.prompt));
        out.push_str("\n\n");
        // The code shown with the question, from the session's question.
        let question = session.questions().iter().find(|q| q.id == r.question);
        if let Some(q) = question {
            push_snippet(q, &mut out);
        }
        let language = question.and_then(|q| q.body.options_language());
        out.push_str(&format!(
            "- **Your answer:** {}\n",
            answer_text(r, language)
        ));
        if let Some(c) = r.correct_option {
            out.push_str(&format!(
                "- **Correct answer:** {}\n",
                option_text(&r.options, c, language)
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
/// marked) and open answers shown inert, so no box can be ticked and no
/// answer field filled in any more (D11).
pub fn render_graded_questions(session: &Session, results: &[QuestionResult]) -> String {
    let mut out = String::new();
    out.push_str(&session_marker(session.key()));
    out.push_str("\n> [!NOTE]\n");
    out.push_str(
        "> **Soqrates learning check: graded.** The results are in a comment \
         below. The answers for this commit are final; a new push asks new \
         questions.\n\n",
    );
    push_collapsed_questions(session.questions(), Some(results), &[], &mut out);
    out
}

/// The question comment of `key` once a newer push superseded it (ADR-0004
/// D7): it says its answers no longer count and links the newer question
/// comment at `newer_url`, if known, saying whether that one asks anything.
/// The questions of `session`, if still stored, are collapsed with options
/// as plain text, so no box can be ticked and no field filled in any more.
/// `drafts` is the text that was left in each question's answer field, in
/// question order; it is shown inert, so the author can reuse it when the
/// question is asked again (ADR-0004 D11).
pub fn render_superseded_questions(
    key: &SessionKey,
    session: Option<&Session>,
    newer_url: Option<&str>,
    newer_asks: bool,
    drafts: &[Option<String>],
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
        push_collapsed_questions(session.questions(), None, drafts, &mut out);
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
/// `results`, the author's answers are marked; without, the text left in
/// each open question's answer field (`drafts`, in question order) is shown,
/// so it isn't lost.
fn push_collapsed_questions(
    questions: &[Question],
    results: Option<&[QuestionResult]>,
    drafts: &[Option<String>],
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
        push_snippet(q, out);
        match &q.body {
            QuestionBody::MultipleChoice { options, .. } => {
                let language = q.body.options_language();
                for (j, option) in options.iter().enumerate() {
                    let picked = answer == Some(&AnswerBody::Choice(j));
                    let item = option_item(j, option, language, picked);
                    // "(your answer)" goes on the item's first line, before
                    // any code nested in it.
                    let item = match item.split_once('\n') {
                        Some((first, code)) if picked => format!("{first} (your answer)\n{code}"),
                        None if picked => format!("{item} (your answer)"),
                        _ => item,
                    };
                    out.push_str(&format!("- {item}\n"));
                }
                if let Some(AnswerBody::Text(t)) = answer {
                    out.push_str(&format!("\nYour answer: {}\n", inline(t)));
                }
                out.push('\n');
            }
            // The author's text, from the answer field or the answer
            // comment, is shown inert like any other text (ADR-0004 D4,
            // D11).
            QuestionBody::Open { .. } if results.is_some() => match answer {
                Some(AnswerBody::Text(t)) if !t.trim().is_empty() => {
                    out.push_str(&format!("_Open question._ Your answer: {}\n\n", inline(t)));
                }
                _ => out.push_str("_Open question, not answered._\n\n"),
            },
            QuestionBody::Open { .. } => match drafts.get(i).and_then(Option::as_deref) {
                Some(draft) => out.push_str(&format!(
                    "_Open question._ Left in its answer field: {}\n\n",
                    inline(draft)
                )),
                None => out.push_str("_Open question._\n\n"),
            },
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
    structure_lines(body)
        .into_iter()
        .filter_map(task_item)
        .any(|(_, text)| text == DONE_BOX)
}

/// The answer comment for a session: the latest comment by `author_id`
/// posted after the first comment carrying `marker`, whose body starts with
/// the answer command (`/soqrates answer`, read forgivingly, ADR-0004 D8).
/// Comments by anyone else are ignored, so nobody can answer for the
/// author. `comments` are `(author_id, created_at, body)` in the order the
/// forge lists them (oldest first). Returns the index of the comment.
pub fn select_answer_comment(
    comments: &[(String, DateTime<Utc>, String)],
    marker: &str,
    author_id: &str,
    app_slug: Option<&str>,
) -> Option<usize> {
    let start = comments.iter().position(|(_, _, b)| b.contains(marker))?;
    let marked_at = comments[start].1;
    comments
        .iter()
        .enumerate()
        .skip(start + 1)
        .filter(|(_, (who, at, body))| {
            who == author_id
                && *at >= marked_at
                && matches!(read_command(body, app_slug), Command::Answer(_))
        })
        // The latest by creation time; ties go to the later in the list.
        .max_by_key(|(i, (_, at, _))| (*at, *i))
        .map(|(i, _)| i)
}

// ---------------------------------------------------------------------------
// Hints for mistyped commands (ADR-0004 D8)

const HINT_MARKER_START: &str = "<!-- soqrates:hint=";

/// The hidden marker of a hint, with the id of the comment it is about.
pub fn hint_marker(comment_id: u64) -> String {
    format!("{HINT_MARKER_START}{comment_id}{MARKER_END}")
}

/// The comment id in the hint marker a comment body starts with, if any.
pub fn hinted_comment(body: &str) -> Option<u64> {
    let rest = body.trim_start().strip_prefix(HINT_MARKER_START)?;
    let (id, _) = rest.split_once(MARKER_END)?;
    id.parse().ok()
}

/// The hint for the comment `comment_id` at `url`, which starts like an
/// answer command but isn't one (ADR-0004 D8). It quotes nothing from that
/// comment, so nothing in it needs escaping (ADR-0004 D4).
pub fn render_hint(comment_id: u64, url: &str, app_slug: Option<&str>) -> String {
    let mut out = hint_marker(comment_id);
    out.push('\n');
    let comment = if is_plain_url(url) {
        format!("[Your comment]({url})")
    } else {
        "Your comment".to_string()
    };
    out.push_str(&format!(
        "Did you mean `{ANSWER_COMMAND}`? {comment} starts like a Soqrates \
         command but isn't one, so it was not read as an answer. Edit it to \
         start with `{ANSWER_COMMAND}`, or post a new comment."
    ));
    if let Some(slug) = app_slug.map(str::trim).filter(|s| is_slug(s)) {
        out.push_str(&format!(" `@{slug} answer` works too."));
    }
    out.push('\n');
    out
}

/// A GitHub App slug: lowercase letters, digits and dashes.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ---------------------------------------------------------------------------
// What the graded answers were read from (ADR-0004 D8)

/// The answer comment a session was graded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerComment {
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// What a session's answers were read from (ADR-0004 D8, D11): the answer
/// comment, if any, and the 1-based numbers of the questions whose ticked
/// boxes, and whose answer fields, were read, and of those whose boxes or
/// field the answer comment replaced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnswerSource {
    pub comment: Option<AnswerComment>,
    pub boxes: Vec<usize>,
    pub fields: Vec<usize>,
    pub replaced: Vec<usize>,
}

impl AnswerSource {
    /// What was read: the answer comment `comment`, if any, with the
    /// answers parsed from it, and the question comment's boxes and fields
    /// `in_place`, which must be `None` unless they count. Per question,
    /// the answer comment wins (ADR-0004 D3, D11).
    pub fn read(
        questions: &[Question],
        comment: Option<AnswerComment>,
        answered: Option<&[Answer]>,
        in_place: Option<&InPlaceAnswers>,
    ) -> Self {
        let mut source = AnswerSource {
            comment,
            ..AnswerSource::default()
        };
        let Some(in_place) = in_place else {
            return source;
        };
        for (i, q) in questions.iter().enumerate() {
            let ticked = in_place.ticked.get(i).is_some_and(|t| *t != Ticked::None);
            let field = in_place.fields.get(i).is_some_and(Option::is_some);
            if answered.is_some_and(|c| c.iter().any(|a| a.question == q.id)) {
                if ticked || field {
                    source.replaced.push(i + 1);
                }
            } else if field {
                source.fields.push(i + 1);
            } else if ticked {
                source.boxes.push(i + 1);
            }
        }
        source
    }
}

/// `1`, `1 and 3`, `1, 2 and 3`.
fn number_list(numbers: &[usize]) -> String {
    let words: Vec<String> = numbers.iter().map(usize::to_string).collect();
    match words.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// `what question 2`, or `what questions 1 and 3`; `None` for none.
fn for_questions(what: &str, numbers: &[usize]) -> Option<String> {
    match numbers.len() {
        0 => None,
        1 => Some(format!("{what} question {}", number_list(numbers))),
        _ => Some(format!("{what} questions {}", number_list(numbers))),
    }
}

/// What the results were graded from, e.g. "Graded from your answer
/// comment (edited 11:18 UTC) and the boxes you ticked for question 1."
fn describe_source(source: &AnswerSource) -> String {
    let comment = source.comment.as_ref().map(|c| {
        let link = if is_plain_url(&c.url) {
            format!("your [answer comment]({})", c.url)
        } else {
            "your answer comment".to_string()
        };
        let when = match c.updated_at.filter(|u| *u > c.created_at) {
            Some(edited) => format!("edited {}", edited.format("%H:%M UTC")),
            None => format!("posted {}", c.created_at.format("%H:%M UTC")),
        };
        format!("{link} ({when})")
    });
    let parts: Vec<String> = [
        comment.clone(),
        for_questions("the boxes you ticked for", &source.boxes),
        for_questions("the answer field of", &source.fields),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut out = match parts.split_last() {
        None => "Graded from the question comment: no boxes were ticked and no answer \
                 field was filled in."
            .to_string(),
        Some((only, [])) if comment.is_some() => {
            format!("Graded from {only}. No ticked boxes or answer fields were read.")
        }
        Some((only, [])) => format!("Graded from {only}."),
        Some((last, rest)) => format!("Graded from {} and {last}.", rest.join(", ")),
    };
    if let Some(replaced) = for_questions("for", &source.replaced) {
        out.push_str(&format!(
            " Your answer comment replaced what the question comment said {replaced}."
        ));
    }
    out
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

/// A unified diff from the files of a compare response: `(path, previous
/// path, patch)`. GitHub gives each file's hunks without headers, and no
/// patch for binary or very large files. Those keep their header, as in the
/// pull request's own diff, so the change still counts and stays within the
/// pull request's files (ADR-0002 D9).
pub fn diff_of_files(files: &[(String, Option<String>, Option<String>)]) -> String {
    let mut out = String::new();
    for (path, previous, patch) in files {
        let old = previous.as_deref().unwrap_or(path);
        out.push_str(&format!("diff --git a/{old} b/{path}\n"));
        match patch.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(patch) => {
                out.push_str(&format!("--- a/{old}\n+++ b/{path}\n{patch}"));
                if !patch.ends_with('\n') {
                    out.push('\n');
                }
            }
            None => out.push_str(&format!(
                "Binary or large files a/{old} and b/{path} differ\n"
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use soqrates_core::{
        AuthorId, CodeSnippet, ConceptId, ConceptScope, Grade, QuestionId, QuestionKind,
        SnippetOrigin,
    };

    fn mc(id: &str, options: &[&str], correct: usize) -> Question {
        Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            kind: QuestionKind::Scenario,
            body: QuestionBody::MultipleChoice {
                prompt: format!("Prompt {id}?"),
                code: None,
                options: options.iter().map(|o| o.to_string()).collect(),
                options_language: None,
                correct,
                reasons: options.iter().map(|_| "SECRET-REASON".into()).collect(),
            },
            explanation: "SECRET-EXPLANATION".into(),
        }
    }

    fn open(id: &str) -> Question {
        Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            kind: QuestionKind::Scenario,
            body: QuestionBody::Open {
                prompt: format!("Explain {id}."),
                code: None,
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
        // D11: the open question has an empty answer field, and nothing to
        // read back from it yet.
        assert!(s.contains(
            "Explain q2.\n\n_Open question: edit this comment and write your answer in your own \
             words below this line, between `soqrates:answer=2` and `soqrates:answer-end=2`._\n\n\
             <!-- soqrates:answer=2 -->\n\n<!-- soqrates:answer-end=2 -->\n\n### Question 3 of 3"
        ));
        assert_eq!(parse_fields(&s, &questions()), vec![None, None, None]);
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
        assert!(!rendered.contains("SECRET-REASON"));
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

    fn parse_answers(body: &str, questions: &[Question]) -> Option<Vec<Answer>> {
        soqrates_core::answer_command::parse_answers(body, questions, None)
    }

    /// The body of the selected answer comment.
    fn selected<'a>(
        comments: &'a [(String, DateTime<Utc>, String)],
        marker: &str,
        author_id: &str,
        app_slug: Option<&str>,
    ) -> Option<&'a str> {
        select_answer_comment(comments, marker, author_id, app_slug).map(|i| comments[i].2.as_str())
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
            selected(&comments, &m, me, None),
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
        assert_eq!(selected(&comments, &m, "github.com:42", None), None);
    }

    #[test]
    fn no_marker_means_no_answers() {
        let m = session_marker(&SessionKey("k".into()));
        let other = session_marker(&SessionKey("k2".into()));
        let comments = vec![
            c("github.com:7", 1, &other),
            c("github.com:42", 2, "/soqrates answer\n1. b"),
        ];
        assert_eq!(selected(&comments, &m, "github.com:42", None), None);
    }

    // @adr-0004-d8
    #[test]
    fn forgiving_commands_and_the_app_mention_are_answers() {
        let slug = Some("soqrates-review");
        let expected = vec![answer("q1", AnswerBody::Choice(1))];
        for body in [
            "/socrates answer\n1. b",
            "/SoCrAtEs ANSWER\n1. b",
            "@soqrates-review answer\n1. b",
        ] {
            assert_eq!(
                soqrates_core::answer_command::parse_answers(body, &questions(), slug),
                Some(expected.clone()),
                "{body:?}"
            );
        }
        // Never a bare @soqrates, and a mention only with the app's slug.
        for (body, slug) in [
            ("@soqrates answer\n1. b", slug),
            ("@soqrates-review answer\n1. b", None),
        ] {
            assert_eq!(
                soqrates_core::answer_command::parse_answers(body, &questions(), slug),
                None,
                "{body:?}"
            );
        }
    }

    // @adr-0004-d8
    #[test]
    fn selects_forgiving_commands_and_skips_near_misses() {
        let m = session_marker(&SessionKey("k".into()));
        let me = "github.com:42";
        let comments = vec![
            c("github.com:7", 1, &m),
            c(me, 2, "/socrates answer\n1. a"),
            c(me, 3, "@soqrates-review answer\n1. b"),
            c(me, 4, "/socrates answr\n1. c"), // a near miss
        ];
        assert_eq!(
            selected(&comments, &m, me, Some("soqrates-review")),
            Some("@soqrates-review answer\n1. b")
        );
        assert_eq!(
            selected(&comments, &m, me, None),
            Some("/socrates answer\n1. a")
        );
    }

    // @adr-0004-d8
    #[test]
    fn a_hint_points_to_the_command_and_carries_its_marker() {
        let hint = render_hint(
            17,
            "https://github.com/o/r/pull/1#issuecomment-17",
            Some("soqrates-review"),
        );
        assert_eq!(hinted_comment(&hint), Some(17));
        assert!(hint.contains("Did you mean `/soqrates answer`?"));
        assert!(hint.contains("[Your comment](https://github.com/o/r/pull/1#issuecomment-17)"));
        assert!(hint.contains("`@soqrates-review answer` works too."));
        let plain = render_hint(18, "javascript:alert(1)", Some("Not A Slug"));
        assert!(!plain.contains("javascript"));
        assert!(!plain.contains("works too"));
        assert_eq!(hinted_comment("<!-- soqrates:hint=x -->"), None);
        assert_eq!(
            hinted_comment(&session_marker(&SessionKey("k".into()))),
            None
        );
        assert!(tags(&hint).is_empty());
    }

    // @adr-0004-d8
    #[test]
    fn results_say_which_comment_and_boxes_were_graded() {
        let comment = AnswerComment {
            url: "https://github.com/o/r/pull/1#issuecomment-9".into(),
            created_at: Utc.with_ymd_and_hms(2026, 11, 8, 11, 2, 0).unwrap(),
            updated_at: Some(Utc.with_ymd_and_hms(2026, 11, 8, 11, 18, 0).unwrap()),
        };
        let both = AnswerSource {
            comment: Some(comment.clone()),
            boxes: vec![1, 3],
            ..AnswerSource::default()
        };
        let s = render_results(&session(), &results(), true, Some(&both));
        assert!(s.contains(
            "<sub>Graded from your [answer comment](https://github.com/o/r/pull/1#issuecomment-9) \
             (edited 11:18 UTC) and the boxes you ticked for questions 1 and 3.</sub>"
        ));
        let posted = AnswerSource {
            comment: Some(AnswerComment {
                updated_at: Some(comment.created_at),
                ..comment
            }),
            ..AnswerSource::default()
        };
        let s = render_results(&session(), &results(), true, Some(&posted));
        assert!(s.contains("(posted 11:02 UTC). No ticked boxes or answer fields were read."));
        let boxes = AnswerSource {
            boxes: vec![2],
            ..AnswerSource::default()
        };
        let s = render_results(&session(), &results(), true, Some(&boxes));
        assert!(s.contains("Graded from the boxes you ticked for question 2."));
        assert!(!render_results(&session(), &results(), true, None).contains("Graded from"));
        let nothing = AnswerSource::default();
        let s = render_results(&session(), &results(), true, Some(&nothing));
        assert!(s.contains("no boxes were ticked and no answer field was filled in."));
    }

    // @adr-0004-d8 @adr-0004-d11
    #[test]
    fn results_say_which_answer_fields_were_graded_and_which_the_comment_replaced() {
        let comment = AnswerComment {
            url: "https://github.com/o/r/pull/1#issuecomment-9".into(),
            created_at: Utc.with_ymd_and_hms(2026, 11, 8, 11, 2, 0).unwrap(),
            updated_at: None,
        };
        let all = AnswerSource {
            comment: Some(comment),
            boxes: vec![1],
            fields: vec![2],
            replaced: vec![3],
        };
        let s = render_results(&session(), &results(), true, Some(&all));
        assert!(
            s.contains(
                "(posted 11:02 UTC), the boxes you ticked for question 1 and the answer field of \
                 question 2. Your answer comment replaced what the question comment said for \
                 question 3.</sub>"
            ),
            "{s}"
        );
        let fields = AnswerSource {
            fields: vec![2, 4],
            ..AnswerSource::default()
        };
        let s = render_results(&session(), &results(), true, Some(&fields));
        assert!(s.contains("<sub>Graded from the answer field of questions 2 and 4.</sub>"));
    }

    // @adr-0004-d8 @adr-0004-d11
    #[test]
    fn what_was_read_is_what_the_comment_does_not_override() {
        let qs = questions();
        let mut in_place = checkboxes(true, &[Ticked::One(1), Ticked::None, Ticked::Many]);
        in_place.fields[1] = Some("it moves".into());
        let read = |comment: Option<&[Answer]>, in_place: Option<&InPlaceAnswers>| {
            let s = AnswerSource::read(&qs, None, comment, in_place);
            (s.boxes, s.fields, s.replaced)
        };
        assert_eq!(read(None, Some(&in_place)), (vec![1, 3], vec![2], vec![]));
        let comment = vec![
            answer("q1", AnswerBody::Choice(0)),
            answer("q2", text("it borrows")),
        ];
        assert_eq!(
            read(Some(&comment), Some(&in_place)),
            (vec![3], vec![], vec![1, 2])
        );
        assert_eq!(read(Some(&comment), None), (vec![], vec![], vec![]));
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

    fn checkboxes(done: bool, ticked: &[Ticked]) -> InPlaceAnswers {
        InPlaceAnswers {
            done,
            ticked: ticked.to_vec(),
            fields: vec![None; ticked.len()],
        }
    }

    // @adr-0004-d3
    #[test]
    fn reads_ticked_boxes_by_section_and_position() {
        let body = render_questions(&session());
        let boxes = parse_in_place(&body, &questions());
        assert_eq!(boxes, checkboxes(false, &[Ticked::None; 3]));
        assert!(!boxes.any());

        let body = tick(&body, "- [ ] b. SECRET-CORRECT");
        let body = tick(&body, "- [ ] a. x");
        let body = tick(&body, "- [ ] **Grade my answers**");
        assert_eq!(
            parse_in_place(&body, &questions()),
            checkboxes(true, &[Ticked::One(1), Ticked::None, Ticked::One(0)])
        );
    }

    // @adr-0004-d3
    #[test]
    fn box_syntax_variants_are_read() {
        let body = "### Question 1 of 3\n\nP\n\n  * [ ] a. first\n- [X] b. renamed by hand\n- [ ] c. third\n\n\
                    ### Question 3 of 3\n\n   -  [x]  a. x\n+ [ ] b. y\n\n---\n\n * [X] **Grade my answers**\n";
        assert_eq!(
            parse_in_place(body, &questions()),
            checkboxes(true, &[Ticked::One(1), Ticked::None, Ticked::One(0)])
        );
        // Not boxes: `[x]` not followed by a space, or no list marker.
        let body = "### Question 1 of 3\n- [x]a. first\n[x] b. second\n-[x] c. third\n";
        assert_eq!(
            parse_in_place(body, &questions()),
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
            parse_in_place(body, &questions()),
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
        let boxes = parse_in_place(&body, &questions());
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
        let boxes = parse_in_place(&body, &questions());
        assert!(boxes.any() && !boxes.done);
        assert_eq!(merge_answers(&questions(), Some(&boxes), None), None);
        let body = tick(&body, "- [ ] **Grade my answers**");
        let boxes = parse_in_place(&body, &questions());
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

    /// Type `text` into the answer field of question `n`, as an author
    /// editing the comment does.
    fn fill(body: &str, n: usize, text: &str) -> String {
        let empty = format!("<!-- soqrates:answer={n} -->\n\n<!-- soqrates:answer-end={n} -->");
        assert!(body.contains(&empty), "no empty field {n} in:\n{body}");
        body.replacen(
            &empty,
            &format!("<!-- soqrates:answer={n} -->\n{text}\n<!-- soqrates:answer-end={n} -->"),
            1,
        )
    }

    // @adr-0004-d11
    #[test]
    fn an_answer_typed_into_the_field_is_read_and_submitted_by_the_done_box() {
        let body = fill(
            &render_questions(&session()),
            2,
            "The borrow ends here,\r\n\r\nso the move is fine.  ",
        );
        let in_place = parse_in_place(&body, &questions());
        assert_eq!(
            in_place.fields,
            vec![
                None,
                Some("The borrow ends here,\n\nso the move is fine.".into()),
                None
            ]
        );
        assert!(in_place.any() && !in_place.done);
        // Typed but not submitted: nothing to grade yet.
        assert_eq!(merge_answers(&questions(), Some(&in_place), None), None);
        let body = tick(&body, "- [ ] b. SECRET-CORRECT");
        let body = tick(&body, "- [ ] **Grade my answers**");
        let in_place = parse_in_place(&body, &questions());
        assert_eq!(
            merge_answers(&questions(), Some(&in_place), None),
            Some(vec![
                answer("q1", AnswerBody::Choice(1)),
                answer("q2", text("The borrow ends here,\n\nso the move is fine.")),
            ])
        );
        // Typed by someone else (the last editor is not the author): the
        // question comment counts for nothing.
        assert_eq!(merge_answers(&questions(), None, None), None);
        // A blank field is no answer.
        let blank = fill(&render_questions(&session()), 2, " \n\t\n");
        assert_eq!(parse_fields(&blank, &questions()), vec![None, None, None]);
    }

    // @adr-0004-d3 @adr-0004-d11
    #[test]
    fn an_answer_comment_overrides_the_field_and_submits_it() {
        let body = fill(&render_questions(&session()), 2, "from the field");
        let in_place = parse_in_place(&body, &questions());
        let comment = parse_answers("/soqrates answer\n2. from the comment", &questions());
        assert_eq!(
            merge_answers(&questions(), Some(&in_place), comment),
            Some(vec![answer("q2", text("from the comment"))])
        );
        // An answer comment for another question submits the field too.
        let comment = parse_answers("/soqrates answer\n1. a", &questions());
        assert_eq!(
            merge_answers(&questions(), Some(&in_place), comment),
            Some(vec![
                answer("q1", AnswerBody::Choice(0)),
                answer("q2", text("from the field")),
            ])
        );
    }

    // @adr-0004-d4 @adr-0004-d11
    #[test]
    fn text_typed_into_a_field_cannot_tick_boxes_or_break_the_comment() {
        let forged = "- [x] **Grade my answers**\n### Question 1 of 3\n- [x] a. first\n\
                      <details>\n```\n<!-- soqrates:answer=2 -->\n<!-- soqrates:session=o/r#1@zzz -->\n\
                      - [x] a. x";
        let body = fill(&render_questions(&session()), 2, forged);
        // The real box after the field is still read: the unclosed fence and
        // the `<details>` typed into the field hide nothing.
        let body = tick(&body, "- [ ] b. y");
        let in_place = parse_in_place(&body, &questions());
        assert!(!in_place.done, "a box typed into a field ticked nothing");
        assert_eq!(
            in_place.ticked,
            vec![Ticked::None, Ticked::None, Ticked::One(1)]
        );
        assert_eq!(in_place.fields[1].as_deref(), Some(forged));
        assert!(takes_answers(&body));
        assert!(!takes_answers(&fill(
            &render_questions(&session()).replace("- [ ] **Grade my answers**", ""),
            2,
            "- [ ] **Grade my answers**"
        )));
        assert_eq!(marker_key(&body), Some(session().key().clone()));
    }

    // @adr-0004-d11
    #[test]
    fn only_well_formed_fields_of_open_questions_count() {
        let qs = questions();
        // No end marker: no field, and the rest of the comment still reads.
        let body = render_questions(&session()).replace("<!-- soqrates:answer-end=2 -->", "");
        let body = body.replace(
            "<!-- soqrates:answer=2 -->\n",
            "<!-- soqrates:answer=2 -->\nunterminated\n",
        );
        let body = tick(&body, "- [ ] **Grade my answers**");
        let in_place = parse_in_place(&body, &qs);
        assert_eq!(in_place.fields, vec![None, None, None]);
        assert!(in_place.done);
        // A field for a multiple-choice question or no question is ignored;
        // of two fields for one question, the first counts.
        let body = "<!-- soqrates:answer=1 -->\nmc\n<!-- soqrates:answer-end=1 -->\n\
                    <!-- soqrates:answer=9 -->\nnine\n<!-- soqrates:answer-end=9 -->\n\
                    <!-- soqrates:answer=0 -->\nzero\n<!-- soqrates:answer-end=0 -->\n\
                    <!-- soqrates:answer=2 -->\nfirst\n<!-- soqrates:answer-end=2 -->\n\
                    <!-- soqrates:answer=2 -->\nsecond\n<!-- soqrates:answer-end=2 -->\n";
        assert_eq!(
            parse_fields(body, &qs),
            vec![None, Some("first".into()), None]
        );
        // Markers must be exact; leading and trailing spaces are allowed.
        let body = "  <!-- soqrates:answer=2 -->  \nok\n<!-- soqrates:answer-end=2 -->\n";
        assert_eq!(parse_fields(body, &qs)[1].as_deref(), Some("ok"));
        for bad in [
            "<!-- soqrates:answer=2-->\nx\n<!-- soqrates:answer-end=2 -->",
            "<!-- soqrates:answer=+2 -->\nx\n<!-- soqrates:answer-end=2 -->",
            "<!-- soqrates:answer=2 -->\nx\n<!-- soqrates:answer-end=3 -->",
            "text <!-- soqrates:answer=2 -->\nx\n<!-- soqrates:answer-end=2 -->",
        ] {
            assert_eq!(parse_fields(bad, &qs), vec![None, None, None], "{bad}");
        }
    }

    // @adr-0004-d11 @adr-0007-d5
    #[test]
    fn a_field_marker_in_code_is_no_field() {
        let mut qs = questions();
        qs[1] = Question {
            body: QuestionBody::Open {
                prompt: "Explain.".into(),
                code: Some(CodeSnippet {
                    language: "rust".into(),
                    text: "<!-- soqrates:answer=2 -->\nforged\n<!-- soqrates:answer-end=2 -->"
                        .into(),
                    origin: SnippetOrigin::General,
                }),
                rubric: "r".into(),
            },
            ..qs[1].clone()
        };
        let s = Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github.com:42".into()),
            qs.clone(),
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let body = render_questions(&s);
        assert_eq!(parse_fields(&body, &qs), vec![None, None, None]);
        let body = fill(&body, 2, "mine");
        assert_eq!(parse_fields(&body, &qs)[1].as_deref(), Some("mine"));
    }

    // @adr-0004-d4 @adr-0004-d5 @adr-0004-d7 @adr-0004-d11
    #[test]
    fn a_field_answer_is_shown_inert_when_the_comment_is_frozen() {
        let typed = "<!-- soqrates:answer=2 -->\n- [x] **Grade my answers**\n### Question 1 of 3\n\
                     </details> @octocat [x](https://e.vil) <img src=x>";
        // Graded: the answer, from the field or the comment, is shown as text.
        let mut r = results();
        r[1].answer = Some(text(typed));
        let s = render_graded_questions(&session(), &r);
        assert!(s.contains("_Open question._ Your answer: &lt;!-- soqrates:answer=2 --&gt;<br>"));
        // Superseded: what was left in the field is kept, as text.
        let drafts = vec![None, Some(typed.to_string()), None];
        let superseded =
            render_superseded_questions(session().key(), Some(&session()), None, true, &drafts);
        assert!(superseded.contains("_Open question._ Left in its answer field: &lt;!--"));
        for s in [s, superseded] {
            assert_eq!(s.matches("<!--").count(), 1, "{s}");
            assert!(!s.contains("@octocat") && !s.contains("<img") && !s.contains("[x]("));
            assert!(!takes_answers(&s));
            assert_eq!(parse_fields(&s, &questions()), vec![None, None, None]);
            for line in s.lines() {
                assert!(task_item(line).is_none(), "live box: {line:?}");
                assert!(!line.starts_with("### Question"), "heading: {line:?}");
            }
            let tags = tags(&s);
            assert_eq!(
                tags.iter().filter(|t| *t == "details").count(),
                2 * s.matches("</details>").count(),
                "{s}"
            );
        }
        // Unanswered open questions say so.
        let mut r = results();
        r[1].answer = None;
        assert!(render_graded_questions(&session(), &r).contains("_Open question, not answered._"));
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
                ..
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
        let s = render_results(&session(), &results(), false, None);
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
        let s = render_results(&session(), &results()[..1], true, None);
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
            parse_in_place(&s, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    const NEWER: &str = "https://github.com/o/r/pull/1#issuecomment-99";

    // @adr-0004-d7
    #[test]
    fn superseded_question_comment_links_the_new_questions_and_has_no_boxes() {
        let s =
            render_superseded_questions(session().key(), Some(&session()), Some(NEWER), true, &[]);
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
            parse_in_place(&s, &questions()),
            checkboxes(false, &[Ticked::None; 3])
        );
    }

    // @adr-0004-d7
    #[test]
    fn superseded_without_session_or_safe_link_still_freezes() {
        let key = SessionKey("o/r#1@old".into());
        let s = render_superseded_questions(&key, None, None, true, &[]);
        assert!(s.contains("superseded.**") && !s.contains("<details>"));
        assert!(s.contains("newer Soqrates comment"));
        assert_eq!(marker_key(&s), Some(key.clone()));
        let s = render_superseded_questions(&key, Some(&session()), Some(NEWER), false, &[]);
        assert!(s.contains(&format!("[nothing to ask]({NEWER}) about the newer commit")));
        assert!(!s.contains("answer the"), "{s}");
        assert!(!takes_answers(&s));
        for bad in ["javascript:alert(1)", "https://x/a)[b](c", "https://x/<b>"] {
            let s = render_superseded_questions(&key, None, Some(bad), true, &[]);
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
    /// than the one marker. Fenced code is not HTML and is skipped (ADR-0007
    /// D5).
    fn tags(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let outside = structure_lines(s).join("\n");
        let mut rest = outside.as_str();
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
            kind: QuestionKind::Scenario,
            body: QuestionBody::MultipleChoice {
                prompt: NASTY.into(),
                code: None,
                options: vec![NASTY.into(), "<b>ok</b>".into(), "- [x] fake".into()],
                options_language: None,
                correct,
                reasons: vec![NASTY.into(); 3],
            },
            explanation: NASTY.into(),
        };
        let open = Question {
            id: QuestionId("o".into()),
            concept: ConceptId("c.o".into()),
            scope: ConceptScope::General,
            kind: QuestionKind::Scenario,
            body: QuestionBody::Open {
                prompt: NASTY.into(),
                code: None,
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

    // @adr-0004-d4 @adr-0007-d5
    #[test]
    fn rendered_comments_use_only_allowed_html() {
        let (nasty, nasty_results) = nasty_session();
        let (code, code_results) = code_session();
        let rendered = [
            render_questions(&code),
            render_graded_questions(&code, &code_results),
            render_results(&code, &code_results, false, None),
            render_superseded_questions(code.key(), Some(&code), Some(NEWER), true, &[]),
            render_questions(&session()),
            render_graded_questions(&session(), &results()),
            render_results(&session(), &results(), true, None),
            render_questions(&nasty),
            render_graded_questions(&nasty, &nasty_results),
            render_results(&nasty, &nasty_results, false, None),
            render_superseded_questions(session().key(), Some(&session()), Some(NEWER), true, &[]),
            render_superseded_questions(nasty.key(), Some(&nasty), Some(NEWER), true, &[]),
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
                let close = structure_lines(s)
                    .iter()
                    .map(|l| l.matches(&format!("</{t}>")).count())
                    .sum::<usize>();
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
            render_results(&nasty, &nasty_results, false, None),
            render_superseded_questions(nasty.key(), Some(&nasty), Some(NEWER), true, &[]),
        ] {
            // The session marker, and two field markers per open question
            // in the live question comment (D11); no model text adds one.
            let open = nasty
                .questions()
                .iter()
                .filter(|q| matches!(q.body, QuestionBody::Open { .. }))
                .count();
            let live = takes_answers(&s);
            assert_eq!(
                s.matches("<!--").count(),
                1 + if live { 2 * open } else { 0 },
                "markers only:\n{s}"
            );
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
            parse_in_place(&q, nasty.questions()),
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

    // @adr-0002-d9
    #[test]
    fn a_compare_response_becomes_a_unified_diff() {
        let files = vec![
            (
                "src/lib.rs".to_string(),
                None,
                Some("@@ -1 +1 @@\n-a\n+b".to_string()),
            ),
            ("logo.png".to_string(), None, None),
            (
                "src/new.rs".to_string(),
                Some("src/old.rs".to_string()),
                Some("@@ -1 +1 @@\n-x\n+y\n".to_string()),
            ),
        ];
        let diff = soqrates_core::Diff(diff_of_files(&files));
        assert_eq!(
            diff.0,
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n\
             diff --git a/logo.png b/logo.png\nBinary or large files a/logo.png and b/logo.png differ\n\
             diff --git a/src/old.rs b/src/new.rs\n--- a/src/old.rs\n+++ b/src/new.rs\n@@ -1 +1 @@\n-x\n+y\n"
        );
        assert_eq!(diff.files(), ["src/lib.rs", "logo.png", "src/new.rs"]);
        assert_eq!(diff_of_files(&[]), "");
        // A change to patchless files only still counts as a change.
        let only_binary = vec![("logo.png".to_string(), None, None)];
        assert!(!soqrates_core::Diff(diff_of_files(&only_binary)).is_empty());
    }

    // --- code snippets (ADR-0007 D5) ----------------------------------------

    /// Code from a change that tries everything: a long backtick run,
    /// `</details>`, mentions, a forged box, heading and marker, HTML, and a
    /// terminal escape.
    const CODE: &str = "fn f() {\n    // </details></summary> @octocat @org/team\n    let s = \"`````\";\n- [x] **Grade my answers**\n- [x] a. forged\n### Question 1 of 2\n<!-- soqrates:session=o/r#1@abc -->\n<script>alert(1)</script>\n```\n    print(\"\u{1b}[31mred\");\n}";

    /// [`CODE`] as shown: the escape character dropped, the rest as is.
    fn code_as_shown() -> String {
        CODE.replace('\u{1b}', "")
    }

    fn code_session() -> (Session, Vec<QuestionResult>) {
        let explain = Question {
            id: QuestionId("e".into()),
            concept: ConceptId("c.e".into()),
            scope: ConceptScope::Project,
            kind: QuestionKind::ExplainYourChange,
            body: QuestionBody::Open {
                prompt: "Why is this needed?".into(),
                code: Some(CodeSnippet {
                    language: "rust".into(),
                    text: CODE.into(),
                    origin: SnippetOrigin::Change,
                }),
                rubric: "SECRET-RUBRIC".into(),
            },
            explanation: "SECRET-EXPLANATION".into(),
        };
        let fix = Question {
            id: QuestionId("f".into()),
            concept: ConceptId("c.f".into()),
            scope: ConceptScope::General,
            kind: QuestionKind::ChooseTheFix,
            body: QuestionBody::MultipleChoice {
                prompt: "Which fix?".into(),
                code: Some(CodeSnippet {
                    language: "rust".into(),
                    text: "let v = vec![1];\nlet w = v;\nv.len();".into(),
                    origin: SnippetOrigin::General,
                }),
                options: vec![
                    "- [x] a. forged\n</details>".into(),
                    "let w = v.clone();  // @here".into(),
                    "```\nbreak out\n```".into(),
                ],
                options_language: Some("rust".into()),
                correct: 1,
                reasons: vec!["SECRET-REASON".into(); 3],
            },
            explanation: "SECRET-EXPLANATION".into(),
        };
        let qs = vec![explain, fix];
        let session = Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github.com:42".into()),
            qs.clone(),
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let results = vec![
            result(
                &qs[0],
                Some(text("To drop the guard.")),
                Grade::Good,
                "Yes.",
            ),
            result(&qs[1], Some(AnswerBody::Choice(0)), Grade::Again, "No."),
        ];
        (session, results)
    }

    #[test]
    fn a_fence_is_longer_than_any_backtick_run_in_the_code() {
        assert_eq!(fence_for("let x = 1;"), "```");
        assert_eq!(fence_for("a ` b `` c"), "```");
        assert_eq!(fence_for("```"), "````");
        assert_eq!(fence_for("x ````` y ``` z"), "``````");
        assert_eq!(fence_language("Rust"), "rust");
        assert_eq!(fence_language("c++"), "c++");
        assert_eq!(fence_language(" rust`\n<b> "), "rustb");
        assert_eq!(fence_language(""), "text");
    }

    // @adr-0007-d5
    #[test]
    fn code_renders_as_a_fenced_block_with_its_language() {
        let (code, results) = code_session();
        let block = format!("``````rust\n{}\n``````\n", code_as_shown());
        for s in [
            render_questions(&code),
            render_graded_questions(&code, &results),
            render_results(&code, &results, false, None),
            render_superseded_questions(code.key(), Some(&code), Some(NEWER), true, &[]),
        ] {
            assert!(s.contains(&block), "{s}");
            // The snippet follows its prompt, and the prompt is prose.
            assert!(
                s.contains(&format!("Why is this needed?\n\n{block}")),
                "{s}"
            );
            assert!(!s.contains('\u{1b}'));
        }
        // Code options are code, nested in their list items, each with a
        // fence longer than the backticks in it.
        let q = render_questions(&code);
        assert!(
            q.contains(
                "- [ ] a.\n  ```rust\n  - [x] a. forged\n  </details>\n  ```\n\
             - [ ] b.\n  ```rust\n  let w = v.clone();  // @here\n  ```\n\
             - [ ] c.\n  ````rust\n  ```\n  break out\n  ```\n  ````\n"
            ),
            "{q}"
        );
        let graded = render_graded_questions(&code, &results);
        assert!(
            graded.contains("- <b>a.</b> (your answer)\n  ```rust\n  - [x] a. forged\n"),
            "{graded}"
        );
        let shown = render_results(&code, &results, false, None);
        assert!(
            shown.contains("- **Your answer:** a.\n  ```rust\n  - [x] a. forged\n"),
            "{shown}"
        );
        assert!(
            shown.contains(
                "- **Correct answer:** b.\n  ```rust\n  let w = v.clone();  // @here\n  ```\n"
            ),
            "{shown}"
        );
        assert!(!q.contains("SECRET") && !graded.contains("SECRET"));
        assert!(!shown.contains("SECRET-RUBRIC"));
    }

    // @adr-0007-d5 @adr-0004-d8
    #[test]
    fn a_command_in_code_is_never_a_command() {
        use soqrates_core::answer_command::hints_due;
        // Code that answers, or nearly answers, at the start of its lines.
        let commands = "/soqrates answer\n1. a\n/socrates answr\n@soqrates-review answer";
        let (mut code, results) = code_session();
        let mut qs = code.questions().to_vec();
        if let QuestionBody::Open { code: c, .. } = &mut qs[0].body {
            c.as_mut().unwrap().text = commands.into();
        }
        code = Session::open(
            code.key().clone(),
            code.author().clone(),
            qs,
            10,
            Utc.timestamp_opt(0, 0).unwrap(),
        )
        .unwrap();
        let slug = Some("soqrates-review");
        let comments = [
            render_questions(&code),
            render_graded_questions(&code, &results),
            render_results(&code, &results, false, None),
            render_superseded_questions(code.key(), Some(&code), Some(NEWER), true, &[]),
            // An author's comment that starts with code.
            format!("```\n{commands}\n```"),
            format!("    {}", commands.replace('\n', "\n    ")),
        ];
        for body in &comments {
            assert!(body.contains("/soqrates answer\n") || body.contains("    /soqrates answer"));
            assert_eq!(read_command(body, slug), Command::NotACommand, "{body}");
            assert_eq!(
                soqrates_core::answer_command::parse_answers(body, code.questions(), slug),
                None
            );
        }
        let listed: Vec<(usize, &str)> = comments.iter().map(String::as_str).enumerate().collect();
        assert_eq!(hints_due(&listed, &[], slug), Vec::<usize>::new());
        // The question comment's boxes are untouched by the code.
        assert_eq!(
            parse_in_place(&comments[0], code.questions()),
            checkboxes(false, &[Ticked::None, Ticked::None])
        );
    }

    // @adr-0007-d5 @adr-0004-d3 @adr-0004-d4
    #[test]
    fn code_cannot_break_out_of_its_fence() {
        let (code, results) = code_session();
        let live = render_questions(&code);
        for s in [
            live.clone(),
            render_graded_questions(&code, &results),
            render_results(&code, &results, false, None),
            render_superseded_questions(code.key(), Some(&code), Some(NEWER), true, &[]),
        ] {
            // Outside the fences there is nothing of the code: no tag,
            // mention, box, heading or second marker.
            let outside = structure_lines(&s).join("\n");
            assert_eq!(outside.matches("<!--").count(), 1, "{outside}");
            for forged in [
                "<script",
                "</details></summary>",
                "@octocat",
                "@org",
                "@here",
                "- [x]",
                "### Question 1 of 2\n<!--",
                "break out",
            ] {
                assert!(
                    !outside.contains(forged),
                    "{forged:?} outside a fence:\n{s}"
                );
            }
            // Every fence that opens closes, so the end of the comment is
            // not code.
            assert_eq!(structure_lines(&s).last().copied(), s.lines().last(), "{s}");
        }
        // The boxes are exactly the renderer's: 3 options and the done box.
        let boxes = structure_lines(&live)
            .into_iter()
            .filter(|l| task_item(l).is_some())
            .count();
        assert_eq!(boxes, 3 + 1);
        assert_eq!(
            parse_in_place(&live, code.questions()),
            checkboxes(false, &[Ticked::None, Ticked::None])
        );
        assert!(takes_answers(&live));
        // Ticking a real box counts; the forged ones in the code never do.
        let ticked = live.replacen("- [ ] b.", "- [x] b.", 1);
        assert_eq!(
            parse_in_place(&ticked, code.questions()),
            checkboxes(false, &[Ticked::None, Ticked::One(1)])
        );
        // A frozen comment takes no answers, even with a done box in its code.
        assert!(!takes_answers(&render_graded_questions(&code, &results)));
        assert!(!takes_answers(&render_superseded_questions(
            code.key(),
            Some(&code),
            Some(NEWER),
            true,
            &[]
        )));
    }
}
