//! Asks questions at the terminal (ADR-0001 D9). Git hooks often run with
//! stdin redirected, so this talks to `/dev/tty` directly.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use soqrates_core::ports::Prompter;
use soqrates_core::{Answer, AnswerBody, Error, Question, QuestionBody, Result, SessionOutcome};

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
        let body = match &question.body {
            QuestionBody::MultipleChoice {
                prompt, options, ..
            } => {
                let mut text = format!("\n{n}. {prompt}\n");
                for (i, o) in options.iter().enumerate() {
                    text.push_str(&format!("   {}. {o}\n", letter(i)));
                }
                self.say(&text)?;
                loop {
                    self.say("   Your answer (letter): ")?;
                    let reply = self.line()?;
                    if let Some(i) = parse_choice(&reply, options.len()) {
                        break AnswerBody::Choice(i);
                    }
                    self.say(&format!(
                        "   Please type a letter from a to {}.\n",
                        letter(options.len() - 1)
                    ))?;
                }
            }
            QuestionBody::Open { prompt, .. } => {
                self.say(&format!(
                    "\n{n}. {prompt}\n   Answer in your own words; finish with an empty line.\n"
                ))?;
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
                out.push_str(&format!("     Your answer: {}. {yours}\n", letter(*c)));
            }
            _ => {}
        }
        if let Some(right) = r.correct_option
            && !r.is_correct()
        {
            let text = r.options.get(right).map(String::as_str).unwrap_or("?");
            out.push_str(&format!("     Correct answer: {}. {text}\n", letter(right)));
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
