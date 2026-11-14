//! Asks questions at the terminal (ADR-0001 D9). Git hooks often run with
//! stdin redirected, so this talks to `/dev/tty` directly.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use soqrates_core::answer_command::parse_feedback;
use soqrates_core::ports::Prompter;
use soqrates_core::{
    Answer, AnswerBody, Error, FeedbackRequest, Question, QuestionBody, Result, SessionOutcome,
};

pub struct TtyPrompter {
    input: BufReader<File>,
    output: File,
    asked: usize,
}

impl TtyPrompter {
    /// `None` when there is no terminal (e.g. a commit from an IDE).
    pub fn open() -> Option<Self> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        Some(Self {
            input: BufReader::new(tty.try_clone().ok()?),
            output: tty,
            asked: 0,
        })
    }

    fn say(&mut self, text: &str) -> Result<()> {
        self.output
            .write_all(text.as_bytes())
            .map_err(|e| Error::Port(e.to_string()))
    }

    fn line(&mut self) -> Result<String> {
        let mut line = String::new();
        self.input
            .read_line(&mut line)
            .map_err(|e| Error::Port(e.to_string()))?;
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    }
}

/// Letter `a`.. for option `i`.
fn letter(i: usize) -> char {
    (b'a' + i as u8) as char
}

/// Parse a multiple-choice reply: a letter, or a 1-based number.
pub fn parse_choice(reply: &str, options: usize) -> Option<usize> {
    let reply = reply.trim().to_lowercase();
    let index = match reply.as_bytes() {
        [c] if c.is_ascii_lowercase() => (c - b'a') as usize,
        _ => reply.parse::<usize>().ok()?.checked_sub(1)?,
    };
    (index < options).then_some(index)
}

impl Prompter for TtyPrompter {
    fn ask(&mut self, question: &Question) -> Result<Answer> {
        self.asked += 1;
        let n = self.asked;
        self.say(&render_question(n, question))?;
        let body = match &question.body {
            QuestionBody::MultipleChoice { options, .. } => loop {
                self.say("   Your answer (letter): ")?;
                let reply = self.line()?;
                if let Some(i) = parse_choice(&reply, options.len()) {
                    break AnswerBody::Choice(i);
                }
                self.say(&format!(
                    "   Please type a letter from a to {}.\n",
                    letter(options.len() - 1)
                ))?;
            },
            QuestionBody::Open { .. } => {
                self.say("   Answer in your own words; finish with an empty line.\n")?;
                let mut lines = Vec::new();
                loop {
                    self.say("   > ")?;
                    let l = self.line()?;
                    if l.trim().is_empty() {
                        break;
                    }
                    lines.push(l);
                }
                AnswerBody::Text(lines.join("\n"))
            }
        };
        Ok(Answer {
            question: question.id.clone(),
            body,
        })
    }

    fn report(&mut self, outcome: &SessionOutcome) {
        let text = render_results(outcome, chrono::Utc::now());
        let _ = self.say(&text);
    }

    fn ask_feedback(&mut self, outcome: &SessionOutcome) -> Result<Option<FeedbackRequest>> {
        if outcome.results.is_empty() {
            return Ok(None);
        }
        self.say(FEEDBACK_PROMPT)?;
        loop {
            self.say("> ")?;
            match read_feedback_reply(&self.line()?) {
                Ok(request) => return Ok(request),
                Err(why) => self.say(&format!("   {why}\n"))?,
            }
        }
    }

    fn feedback_noted(&mut self, note: &str) {
        let _ = self.say(&format!("{}\n\n", indent_code(note, "")));
    }
}

/// Asked once after the results (ADR-0011 D2).
pub const FEEDBACK_PROMPT: &str = "Any feedback on a question? Type its number, a kind \
(disagree, unclear, wrong-explanation, off-topic, good) and why, e.g. \
`2 disagree b is also right`, or press Enter to skip.\n";

/// A reply to [`FEEDBACK_PROMPT`]: `None` for an empty line, which skips.
pub fn read_feedback_reply(line: &str) -> std::result::Result<Option<FeedbackRequest>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    parse_feedback(line).map(Some).map_err(|why| {
        why.replace("/soqrates feedback ", "")
            .replace("Say", "Please say")
    })
}

/// `code` as it is, every line prefixed with `indent`, without control
/// characters other than tabs, so that code from a change can't drive the
/// terminal (ADR-0007 D5).
pub fn indent_code(code: &str, indent: &str) -> String {
    let mut out = String::new();
    for line in code.trim_end().lines() {
        out.push_str(indent);
        out.extend(line.chars().filter(|&c| c == '\t' || !c.is_control()));
        out.push('\n');
    }
    out
}

/// Question `n` as shown at the terminal: the prompt, then its code
/// indented, then the options, code options indented under their letter
/// (ADR-0007 D5).
pub fn render_question(n: usize, question: &Question) -> String {
    let mut text = format!("\n{n}. {}\n", question.body.prompt());
    if let Some(code) = question.body.code() {
        text.push('\n');
        text.push_str(&indent_code(&code.text, "       "));
        text.push('\n');
    }
    if let QuestionBody::MultipleChoice { options, .. } = &question.body {
        for (i, o) in options.iter().enumerate() {
            if question.body.options_are_code() {
                text.push_str(&format!("   {}.\n", letter(i)));
                text.push_str(&indent_code(o, "       "));
            } else {
                text.push_str(&format!("   {}. {o}\n", letter(i)));
            }
        }
    }
    text
}

/// An option in the results: inline, or on its own indented lines when it
/// spans several (code options).
fn option_line(o: &str) -> String {
    if o.contains('\n') {
        format!("\n{}", indent_code(o, "       ").trim_end_matches('\n'))
    } else {
        format!(" {o}")
    }
}

/// "today", "tomorrow", "in 3 days" or "in 2 weeks".
fn when(due: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> String {
    let days = (due - now).num_hours().div_euclid(24).max(0);
    match days {
        0 => "today".into(),
        1 => "tomorrow".into(),
        2..=13 => format!("in {days} days"),
        _ => format!("in {} weeks", days / 7),
    }
}

/// The results of a graded session for the terminal (ADR-0004 D6): each
/// question with ✓ or ✗, the correct option, the explanation and the next
/// review, then the score and level. The terminal is private to the
/// learner, so the profile data is shown here (ADR-0004 D2).
pub fn render_results(outcome: &SessionOutcome, now: chrono::DateTime<chrono::Utc>) -> String {
    let mut out = String::from("\n");
    for (i, r) in outcome.results.iter().enumerate() {
        let mark = if r.is_correct() { "✓" } else { "✗" };
        out.push_str(&format!("{mark} {}. {}\n", i + 1, r.prompt.trim()));
        match (&r.answer, r.correct_option) {
            (None, _) => out.push_str("     Not answered.\n"),
            (Some(AnswerBody::Choice(c)), Some(right)) if *c != right => {
                let yours = r.options.get(*c).map(String::as_str).unwrap_or("?");
                out.push_str(&format!(
                    "     Your answer: {}.{}\n",
                    letter(*c),
                    option_line(yours)
                ));
            }
            _ => {}
        }
        if let Some(right) = r.correct_option
            && !r.is_correct()
        {
            let text = r.options.get(right).map(String::as_str).unwrap_or("?");
            out.push_str(&format!(
                "     Correct answer: {}.{}\n",
                letter(right),
                option_line(text)
            ));
        }
        for line in r.feedback.trim().lines() {
            out.push_str(&format!("     {line}\n"));
        }
        if let Some(due) = r.next_review {
            out.push_str(&format!("     Next review: {}\n", when(due, now)));
        }
        out.push('\n');
    }
    let correct = outcome.results.iter().filter(|r| r.is_correct()).count();
    out.push_str(&format!(
        "Soqrates: {correct} of {} correct, {}. Level: {:?}.\n\n",
        outcome.results.len(),
        if outcome.passed {
            "passed"
        } else {
            "not passed"
        },
        outcome.level_after
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::{parse_choice, render_results};
    use chrono::{Duration, TimeZone, Utc};
    use soqrates_core::{
        AnswerBody, ConceptId, Grade, Level, ProfileLocation, QuestionId, QuestionResult,
        SessionOutcome,
    };

    fn outcome() -> SessionOutcome {
        let now = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
        let mc = |grade, answer| QuestionResult {
            question: QuestionId("rust.borrow/mc".into()),
            concept: ConceptId("rust.borrow".into()),
            prompt: "When does the borrow end?".into(),
            options: vec!["At last use".into(), "At end of scope".into()],
            correct_option: Some(0),
            answer,
            grade,
            feedback: "Non-lexical lifetimes end a borrow at its last use.".into(),
            next_review: Some(now + Duration::days(3)),
        };
        SessionOutcome {
            grades: vec![],
            passed: false,
            level_after: Level::Novice,
            events: vec![],
            profile_location: ProfileLocation::LearnerStore,
            results: vec![
                mc(Grade::Good, Some(AnswerBody::Choice(0))),
                mc(Grade::Again, Some(AnswerBody::Choice(1))),
                mc(Grade::Again, None),
            ],
        }
    }

    // @adr-0004-d6
    #[test]
    fn terminal_results_show_verdict_correct_option_explanation_and_next_review() {
        let now = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
        let text = render_results(&outcome(), now);
        assert!(text.contains("✓ 1. When does the borrow end?"), "{text}");
        assert!(text.contains("✗ 2. When does the borrow end?"), "{text}");
        assert!(text.contains("Your answer: b. At end of scope"), "{text}");
        assert!(text.contains("Correct answer: a. At last use"), "{text}");
        assert!(text.contains("Not answered."), "{text}");
        assert!(
            text.contains("Non-lexical lifetimes end a borrow"),
            "{text}"
        );
        assert!(text.contains("Next review: in 3 days"), "{text}");
        assert!(
            text.contains("1 of 3 correct, not passed. Level: Novice."),
            "{text}"
        );
        // A correct answer doesn't repeat the correct option.
        let first = text.split("✗ 2.").next().unwrap();
        assert!(!first.contains("Correct answer"), "{text}");
    }

    // @adr-0007-d5
    #[test]
    fn terminal_shows_code_as_is_indented() {
        use soqrates_core::{
            CodeSnippet, ConceptScope, Question, QuestionBody, QuestionKind, SnippetOrigin,
        };
        let q = Question {
            id: QuestionId("rust.borrow/mc".into()),
            concept: ConceptId("rust.borrow".into()),
            scope: ConceptScope::General,
            kind: QuestionKind::ChooseTheFix,
            body: QuestionBody::MultipleChoice {
                prompt: "Which fix compiles?".into(),
                code: Some(CodeSnippet {
                    language: "rust".into(),
                    text: "let v = vec![1];\nlet w = v;\n\tprintln!(\"{} `x` </details> @here\", v.len()); // \u{1b}[2J".into(),
                    origin: SnippetOrigin::Change,
                }),
                options: vec!["let w = v.clone();".into(), "let w = &v;\nlet n = 1;".into()],
                options_language: Some("rust".into()),
                correct: 0,
                reasons: vec![],
            },
            explanation: "E".into(),
            checked: false,
            banked: None,
        };
        let text = super::render_question(2, &q);
        assert_eq!(
            text,
            "\n2. Which fix compiles?\n\n\
             \x20      let v = vec![1];\n\
             \x20      let w = v;\n\
             \x20      \tprintln!(\"{} `x` </details> @here\", v.len()); // [2J\n\n\
             \x20  a.\n\
             \x20      let w = v.clone();\n\
             \x20  b.\n\
             \x20      let w = &v;\n\
             \x20      let n = 1;\n"
        );
    }

    // @adr-0011-d2
    #[test]
    fn the_feedback_prompt_is_skipped_with_enter() {
        use soqrates_core::{FeedbackKind, QuestionRef};
        assert_eq!(super::read_feedback_reply("  "), Ok(None));
        let request = super::read_feedback_reply("2 disagree b is also right")
            .unwrap()
            .unwrap();
        assert_eq!(request.question, QuestionRef::Number(2));
        assert_eq!(request.kind, FeedbackKind::Disagree);
        assert_eq!(request.text.as_deref(), Some("b is also right"));
        let why = super::read_feedback_reply("disagree").unwrap_err();
        assert!(why.starts_with("Please say which question"), "{why}");
        assert!(!why.contains("/soqrates"), "{why}");
    }

    #[test]
    fn choices() {
        assert_eq!(parse_choice("b", 3), Some(1));
        assert_eq!(parse_choice(" C ", 3), Some(2));
        assert_eq!(parse_choice("1", 3), Some(0));
        assert_eq!(parse_choice("d", 3), None);
        assert_eq!(parse_choice("0", 3), None);
        assert_eq!(parse_choice("maybe", 3), None);
    }
}
