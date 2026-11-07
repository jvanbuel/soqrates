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
        let correct = outcome
            .grades
            .iter()
            .filter(|(_, g)| g.is_correct())
            .count();
        let _ = self.say(&format!(
            "\nSoqrates: {correct} of {} correct, {}. Level: {:?}.\n\n",
            outcome.grades.len(),
            if outcome.passed {
                "passed"
            } else {
                "not passed"
            },
            outcome.level_after
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::parse_choice;

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
