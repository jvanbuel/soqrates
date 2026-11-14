//! The answer command: how an author answers a session in a comment
//! (ADR-0004 D3, D8).
//!
//! A comment answers when it starts with `/soqrates answer`, followed by one
//! numbered line per question. The command is forgiving: `/socrates` (the
//! philosopher's spelling) and any letter case are accepted, and so is a
//! mention of the posting app, `@<app slug> answer`. A bare `@soqrates` is
//! never accepted: a person of that name would be notified of every answer,
//! while an app is not notified when mentioned. A comment that starts like a
//! command but isn't one is a near miss, which the forge answers with a hint.
//!
//! A comment that starts with `/soqrates feedback`, read just as forgivingly,
//! gives feedback on a graded question (ADR-0011 D2).
//!
//! This is the one place that decides what a command looks like, so the
//! workflow filters (which list [`COMMAND_PREFIXES`]) and the parser cannot
//! drift apart.

use crate::assessment::{Answer, AnswerBody, Question, QuestionBody};
use crate::delivery::{FeedbackKind, FeedbackRequest, QuestionRef};

/// The canonical answer command, as shown to authors.
pub const ANSWER_COMMAND: &str = "/soqrates answer";

/// The canonical feedback command, as shown to authors (ADR-0011 D2).
pub const FEEDBACK_COMMAND: &str = "/soqrates feedback";

/// The verb of the feedback command.
const FEEDBACK_VERB: &str = "feedback";

/// The words a command may start with, besides a mention of the posting
/// app. Compared case-insensitively.
pub const COMMAND_PREFIXES: [&str; 2] = ["/soqrates", "/socrates"];

/// The name that is never accepted as a mention (see the module docs).
const BARE_NAME: &str = "soqrates";

/// The verb that follows the prefix.
const VERB: &str = "answer";

/// How far a first word may be from `/soqrates` (or from the app's mention)
/// and still count as a near miss.
const NEAR_MISS_DISTANCE: usize = 2;

/// What a comment says to Soqrates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// An answer command, with the text after it.
    Answer(&'a str),
    /// A feedback command, with the text after it (ADR-0011 D2).
    Feedback(&'a str),
    /// Starts like a command but isn't a valid one, e.g. `/socrates answr`
    /// or `/soqrate answer`. Gets a hint.
    NearMiss,
    /// Not addressed to Soqrates.
    NotACommand,
}

/// Read the command a comment `body` starts with, after leading whitespace.
/// `app_slug` is the slug of the app that posts Soqrates' comments, if any:
/// `@<app_slug> answer` then counts as the command too.
///
/// A comment that starts with code, a fenced block or a line indented as
/// code, is not a command and not a near miss: code is inert, whatever it
/// says (ADR-0007 D5).
pub fn read_command<'a>(body: &'a str, app_slug: Option<&str>) -> Command<'a> {
    let first_line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if first_line.starts_with("    ") || first_line.starts_with('\t') {
        return Command::NotACommand;
    }
    let body = body.trim_start();
    if fence_of(body.lines().next().unwrap_or("")).is_some() {
        return Command::NotACommand;
    }
    let (first, rest) = split_word(body);
    if first.is_empty() {
        return Command::NotACommand;
    }
    // Even an app whose slug is `soqrates` is never mentioned as `@soqrates`:
    // that would notify a person of that name, should one exist.
    let mention = app_slug
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(BARE_NAME))
        .map(|s| format!("@{s}"));
    let addressed = COMMAND_PREFIXES
        .iter()
        .any(|p| first.eq_ignore_ascii_case(p))
        || mention.as_deref().is_some_and(|m| is_mention_of(first, m));
    let (verb, after) = split_word(rest.trim_start_matches([' ', '\t']));
    if addressed {
        return if verb.eq_ignore_ascii_case(VERB) {
            Command::Answer(after)
        } else if verb.eq_ignore_ascii_case(FEEDBACK_VERB) {
            Command::Feedback(after)
        } else {
            Command::NearMiss
        };
    }
    let lower = first.to_lowercase();
    let near_prefix = lower.starts_with('/')
        && COMMAND_PREFIXES
            .iter()
            .any(|p| distance(&lower, p) <= NEAR_MISS_DISTANCE);
    // A mention close to the app's is only a near miss when it goes on with
    // the verb: `@someone` is usually a person being addressed.
    let near_mention = lower.starts_with('@')
        && verb.eq_ignore_ascii_case(VERB)
        && mention
            .as_deref()
            .is_some_and(|m| distance(&lower, &m.to_lowercase()) <= NEAR_MISS_DISTANCE);
    if near_prefix || near_mention {
        Command::NearMiss
    } else {
        Command::NotACommand
    }
}

/// `@slug` or `@slug[bot]`, in any case.
fn is_mention_of(word: &str, mention: &str) -> bool {
    let word = word.to_lowercase();
    let mention = mention.to_lowercase();
    word == mention || word.strip_suffix("[bot]") == Some(mention.as_str())
}

/// The first word of `text` (up to whitespace) and the rest, which starts at
/// that whitespace.
fn split_word(text: &str) -> (&str, &str) {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    text.split_at(end)
}

/// Levenshtein distance, by characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(cur).min(row[j])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// The feedback a comment `body` gives, if it is a feedback command
/// (ADR-0011 D2): `Err` with a short reason when it can't be read.
pub fn read_feedback(
    body: &str,
    app_slug: Option<&str>,
) -> Option<std::result::Result<FeedbackRequest, String>> {
    match read_command(body, app_slug) {
        Command::Feedback(rest) => Some(parse_feedback(rest)),
        _ => None,
    }
}

/// `<question> <kind> [text]`, as after the feedback command or at the
/// terminal prompt (ADR-0011 D2, D3). The question is its number (`2`,
/// `#2`, `2.`); the text, if any, may span lines.
pub fn parse_feedback(args: &str) -> std::result::Result<FeedbackRequest, String> {
    let usage = format!(
        "write `{FEEDBACK_COMMAND} <question number> <kind> [why]`, e.g. \
         `{FEEDBACK_COMMAND} 2 disagree option c is also correct`"
    );
    let (number, rest) = split_word(args.trim_start());
    let number = number.strip_prefix('#').unwrap_or(number);
    let number = number
        .strip_suffix(['.', ':', ')'])
        .unwrap_or(number)
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("Say which question the feedback is about: {usage}."))?;
    let (kind, text) = split_word(rest.trim_start());
    let kind = FeedbackKind::parse(kind).ok_or_else(|| {
        format!(
            "Say what kind of feedback it is, one of {}: {usage}.",
            FeedbackKind::names()
        )
    })?;
    let text = text.trim();
    Ok(FeedbackRequest {
        question: QuestionRef::Number(number),
        kind,
        text: (!text.is_empty()).then(|| text.to_string()),
    })
}

/// The comments that get a hint (ADR-0004 D8): near misses that have none
/// yet and don't repeat one that already has, so each mistake is pointed out
/// once. `comments` are `(id, body)`, oldest first; `hinted` are the ids
/// that already got a hint.
pub fn hints_due<I: Copy + PartialEq>(
    comments: &[(I, &str)],
    hinted: &[I],
    app_slug: Option<&str>,
) -> Vec<I> {
    let near_misses: Vec<(I, &str)> = comments
        .iter()
        .copied()
        .filter(|(_, body)| read_command(body, app_slug) == Command::NearMiss)
        .collect();
    let mut seen: Vec<&str> = near_misses
        .iter()
        .filter(|(id, _)| hinted.contains(id))
        .map(|(_, body)| body.trim())
        .collect();
    let mut due = Vec::new();
    for (id, body) in near_misses {
        if hinted.contains(&id) || seen.contains(&body.trim()) {
            continue;
        }
        seen.push(body.trim());
        due.push(id);
    }
    due
}

/// The fence a line opens or closes, `(character, length)`: a run of at
/// least three backticks or tildes at its start, after up to three spaces.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = rest.chars().take_while(|&x| x == c).count();
    (n >= 3).then_some((c, n))
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
/// unless `body` starts with the answer command (see [`read_command`]).
///
/// Numbers are 1-based in question order; out-of-range numbers are ignored
/// and for a duplicate number the last one wins. Lines without a number
/// continue the previous answer, and so does every line of a fenced code
/// block, numbered or not: code in an answer is part of it (ADR-0007 D5).
/// Answers come back in question order.
pub fn parse_answers(
    body: &str,
    questions: &[Question],
    app_slug: Option<&str>,
) -> Option<Vec<Answer>> {
    let Command::Answer(rest) = read_command(body, app_slug) else {
        return None;
    };
    let mut texts: Vec<Option<String>> = vec![None; questions.len()];
    // The answer that unnumbered lines continue; `None` before the first
    // number and after an out-of-range one.
    let mut current: Option<usize> = None;
    // The fence of the code block the line is in, if any.
    let mut fence: Option<(char, usize)> = None;
    for line in rest.lines() {
        let in_code = match (fence, fence_of(line)) {
            (None, Some(open)) => {
                fence = Some(open);
                true
            }
            (Some((c, n)), Some((d, m))) if c == d && m >= n && line.trim()[m..].is_empty() => {
                fence = None;
                true
            }
            (Some(_), _) => true,
            (None, None) => false,
        };
        if in_code {
            if let Some(i) = current {
                let t = texts[i].get_or_insert_with(String::new);
                t.push('\n');
                t.push_str(line.trim_end());
            }
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SLUG: Option<&str> = Some("soqrates-review");

    // @adr-0004-d8
    #[test]
    fn forgiving_prefixes_and_case() {
        for body in [
            "/soqrates answer",
            "/socrates answer\n1. a",
            "/SOQRATES Answer 1. a",
            "  \n/Socrates ANSWER",
            "@soqrates-review answer\n1. a",
            "@Soqrates-Review answer",
            "@soqrates-review[bot] answer",
        ] {
            assert!(
                matches!(read_command(body, SLUG), Command::Answer(_)),
                "{body:?}"
            );
        }
    }

    // @adr-0004-d8
    #[test]
    fn a_bare_soqrates_mention_or_another_app_is_never_a_command() {
        assert_eq!(read_command("@soqrates answer", SLUG), Command::NotACommand);
        assert_eq!(read_command("@soqrates answer", None), Command::NotACommand);
        assert_eq!(
            read_command("@soqrates-review answer", None),
            Command::NotACommand
        );
        assert_eq!(read_command("@alice answer", SLUG), Command::NotACommand);
        assert_eq!(
            read_command("@soqrates answer", Some("Soqrates")),
            Command::NotACommand
        );
        assert_eq!(
            read_command("@soqrates-review answer", Some(" ")),
            Command::NotACommand
        );
    }

    // @adr-0004-d8
    #[test]
    fn near_misses() {
        for body in [
            "/soqrates",
            "/soqrates answers",
            "/socrates anwser\n1. a",
            "/soqrate answer",
            "/sokrates answer",
            "/Soqartes answer",
            "@soqrates-reveiw answer",
            "@soqrates-review please",
        ] {
            assert_eq!(read_command(body, SLUG), Command::NearMiss, "{body:?}");
        }
        for body in [
            "",
            "LGTM",
            "/approve",
            "/label bug",
            "hi /soqrates answer",
            "@soqrates-reveiw can you look",
            "soqrates answer",
        ] {
            assert_eq!(read_command(body, SLUG), Command::NotACommand, "{body:?}");
        }
    }

    // @adr-0004-d8
    #[test]
    fn each_near_miss_is_hinted_once() {
        let comments = [
            (1, "/socrates answr\n1. a"),
            (2, "/soqrates answer\n1. a"),
            (3, "/socrates answr\n1. a"),
            (4, "/soqrates answers"),
            (5, "thanks"),
        ];
        assert_eq!(hints_due(&comments, &[], None), vec![1, 4]);
        assert_eq!(hints_due(&comments, &[1], None), vec![4]);
        assert_eq!(hints_due(&comments, &[1, 4], None), Vec::<i32>::new());
        // A repeat of an earlier, already hinted comment gets none.
        assert_eq!(hints_due(&comments[2..], &[], None), vec![3, 4]);
    }

    // @adr-0007-d5
    #[test]
    fn code_is_never_a_command() {
        for body in [
            "```\n/soqrates answer\n1. a\n```",
            "~~~text\n/socrates answr\n~~~",
            "\n````rust\n/soqrates answer\n````\n/soqrates answer\n1. a",
            "    /soqrates answer\n    1. a",
            "\t/soqrate answer",
            "<!-- soqrates:session=k -->\n```\n/soqrates answer\n```",
        ] {
            assert_eq!(read_command(body, SLUG), Command::NotACommand, "{body:?}");
        }
        let comments = [
            (1, "```\n/socrates answr\n```"),
            (2, "    /soqrates answers"),
        ];
        assert_eq!(hints_due(&comments, &[], SLUG), Vec::<i32>::new());
        // Leading blank lines and a little indentation are still fine.
        assert!(matches!(
            read_command("  \n  /soqrates answer", SLUG),
            Command::Answer(_)
        ));
    }

    // @adr-0007-d5
    #[test]
    fn numbered_lines_in_code_belong_to_the_answer() {
        use crate::assessment::{ConceptScope, QuestionKind};
        use crate::ids::{ConceptId, QuestionId};
        let open = |id: &str| Question {
            id: QuestionId(id.into()),
            concept: ConceptId(format!("c.{id}")),
            scope: ConceptScope::General,
            kind: QuestionKind::Scenario,
            body: QuestionBody::Open {
                prompt: "Why?".into(),
                code: None,
                rubric: "R".into(),
            },
            explanation: "E".into(),
            checked: false,
            banked: None,
        };
        let qs = [open("a"), open("b")];
        let body = "/soqrates answer\n1. Like this:\n```\n2. not an answer\n/soqrates answer\n```\n2. The second.";
        let answers = parse_answers(body, &qs, None).unwrap();
        assert_eq!(
            answers[0].body,
            AnswerBody::Text("Like this:\n```\n2. not an answer\n/soqrates answer\n```".into())
        );
        assert_eq!(answers[1].body, AnswerBody::Text("The second.".into()));
        // A fence before any number answers nothing, and its numbers neither.
        let body = "/soqrates answer\n```\n1. x\n```\n2. y";
        let answers = parse_answers(body, &qs, None).unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].question, QuestionId("b".into()));
    }

    // @adr-0011-d2 @adr-0011-d3
    #[test]
    fn feedback_commands_are_read_forgivingly() {
        for body in [
            "/soqrates feedback 2 disagree option c is also correct",
            "/Socrates FEEDBACK #2 Disagree option c is also correct",
            "@soqrates-review feedback 2. disagree option c is also correct",
        ] {
            let request = read_feedback(body, SLUG).unwrap().unwrap();
            assert_eq!(request.question, QuestionRef::Number(2), "{body:?}");
            assert_eq!(request.kind, FeedbackKind::Disagree);
            assert_eq!(request.text.as_deref(), Some("option c is also correct"));
        }
        let request = read_feedback("/soqrates feedback 1 good", SLUG)
            .unwrap()
            .unwrap();
        assert_eq!(request.text, None);
        let text = read_feedback("/soqrates feedback 1 unclear\nwhich one?\n\nb or c", None)
            .unwrap()
            .unwrap()
            .text;
        assert_eq!(text.as_deref(), Some("which one?\n\nb or c"));
        for bad in [
            "/soqrates feedback",
            "/soqrates feedback disagree 2",
            "/soqrates feedback 0 good",
            "/soqrates feedback 2 wrong",
        ] {
            assert!(read_feedback(bad, SLUG).unwrap().is_err(), "{bad:?}");
        }
        assert!(read_feedback("/soqrates answer 1. a", SLUG).is_none());
        assert!(read_feedback("```\n/soqrates feedback 1 good\n```", SLUG).is_none());
        // Neither an answer nor a near miss.
        assert!(matches!(
            read_command("/soqrates feedback 1 good", SLUG),
            Command::Feedback(_)
        ));
        assert!(parse_answers("/soqrates feedback 1 good", &[], SLUG).is_none());
    }

    #[test]
    fn distances() {
        assert_eq!(distance("/soqrates", "/soqrates"), 0);
        assert_eq!(distance("/socrates", "/soqrates"), 1);
        assert_eq!(distance("/soqartes", "/soqrates"), 2);
        assert_eq!(distance("", "abc"), 3);
    }
}
