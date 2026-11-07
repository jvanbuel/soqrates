//! Pure logic of the GitHub adapter: rendering the questions comment, finding
//! and parsing the author's answers, and mapping check statuses. No I/O, so
//! all of it is unit tested (ADR-0001 D12).

use chrono::{DateTime, Utc};
use soqrates_core::{Answer, AnswerBody, CheckStatus, Question, QuestionBody, Session, SessionKey};

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
    format!("<!-- soqrates:session={} -->", key.0)
}

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

/// The Markdown comment putting the session's questions to the author. It
/// never contains the correct option or the rubric.
pub fn render_questions(session: &Session) -> String {
    let mut out = String::new();
    out.push_str(&session_marker(session.key()));
    out.push_str("\n### Soqrates learning check\n\n");
    out.push_str(
        "Soqrates asks a few questions about the concepts in this change, to help \
         you learn and remember them. This is a learning check, not a code review, \
         and it is advisory.\n\n",
    );
    for (i, q) in session.questions().iter().enumerate() {
        let n = i + 1;
        match &q.body {
            QuestionBody::MultipleChoice {
                prompt, options, ..
            } => {
                out.push_str(&format!("**{n}.** {}\n\n", prompt.trim()));
                for (j, option) in options.iter().enumerate() {
                    out.push_str(&format!("   {}. {}\n", option_letter(j), option.trim()));
                }
                out.push_str("\n   _Answer with the letter of one option._\n\n");
            }
            QuestionBody::Open { prompt, .. } => {
                out.push_str(&format!("**{n}.** {}\n\n", prompt.trim()));
                out.push_str("   _Answer in your own words._\n\n");
            }
        }
    }
    out.push_str("#### How to answer\n\n");
    out.push_str(
        "Reply to this pull request with a comment that starts with \
         `/soqrates answer`, followed by one line per question: its number, then \
         your answer. For example:\n\n",
    );
    out.push_str("```\n/soqrates answer\n1. b\n2. Your answer in your own words\n```\n\n");
    out.push_str(
        "An answer in your own words may span several lines. You can post a new \
         answer comment to replace an earlier one; the latest one counts. Only \
         answers from the author of this pull request count.\n",
    );
    out
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
    use soqrates_core::{AuthorId, ConceptId, ConceptScope, QuestionId};

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

    #[test]
    fn render_contains_marker_questions_and_instructions() {
        let s = render_questions(&session());
        assert!(s.contains("<!-- soqrates:session=o/r#1@abc -->"));
        assert!(s.contains("**1.** Prompt q1?"));
        assert!(s.contains("a. first"));
        assert!(s.contains("c. third"));
        assert!(s.contains("**2.** Explain q2."));
        assert!(s.contains("Answer in your own words"));
        assert!(s.contains("advisory"));
        assert!(s.contains("```\n/soqrates answer\n1. b\n2. Your answer in your own words\n```"));
        assert!(s.contains("Only answers from the author of this pull request count"));
    }

    #[test]
    fn render_never_reveals_correct_option_or_rubric() {
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
        assert!(!rendered.to_lowercase().contains("correct"));
        assert!(!rendered.contains("rubric"));
        // The default session's correct option is shown only as an option.
        let rendered = render_questions(&session());
        assert_eq!(rendered.matches("SECRET-CORRECT").count(), 1);
        assert!(!rendered.contains("SECRET-RUBRIC"));
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
}
