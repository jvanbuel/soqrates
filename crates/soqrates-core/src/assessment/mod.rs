//! Assessment context: what a change is about, and how answers are judged
//! (ADR-0002). Uses only the shared kernel.

use serde::{Deserialize, Serialize};

use crate::ids::{ConceptId, QuestionId};

/// A change: the unified diff Soqrates asks about.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff(pub String);

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// The paths this diff touches, from its `diff --git a/… b/…` headers.
    pub fn files(&self) -> Vec<&str> {
        let mut files = Vec::new();
        for file in self.sections().filter_map(|(path, _)| path) {
            if !files.contains(&file) {
                files.push(file);
            }
        }
        files
    }

    /// Only the parts of this diff that touch a file `other` touches too.
    /// Text before the first file header is dropped as well. Used to keep
    /// the changes a base-branch merge brought in out of the change since
    /// the last round (ADR-0002 D9).
    pub fn restricted_to_files_of(&self, other: &Diff) -> Diff {
        let keep = other.files();
        let mut out = String::new();
        for (path, text) in self.sections() {
            if path.is_some_and(|p| keep.contains(&p)) {
                out.push_str(text);
            }
        }
        Diff(out)
    }

    /// The diff split at each `diff --git` header, with the header's new
    /// path (`None` for text before the first header).
    fn sections(&self) -> impl Iterator<Item = (Option<&str>, &str)> {
        let text = self.0.as_str();
        let mut starts: Vec<usize> = text
            .match_indices("diff --git ")
            .map(|(i, _)| i)
            .filter(|&i| i == 0 || text.as_bytes()[i - 1] == b'\n')
            .collect();
        if starts.first() != Some(&0) {
            starts.insert(0, 0);
        }
        let ends: Vec<usize> = starts.iter().skip(1).copied().chain([text.len()]).collect();
        starts.into_iter().zip(ends).filter_map(move |(s, e)| {
            let section = &text[s..e];
            if section.is_empty() {
                return None;
            }
            let path = section
                .strip_prefix("diff --git ")
                .and_then(|rest| rest.lines().next())
                .and_then(|header| header.rsplit_once(" b/"))
                .map(|(_, b)| b);
            Some((path, section))
        })
    }
}

/// Whether a concept is general technology knowledge or specific to one
/// organisation's code (ADR-0003 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ConceptScope {
    /// e.g. `rust.async.cancellation`. May be stored in a learner store
    /// shared across organisations.
    General,
    /// Specific to one organisation's code. Never leaves that organisation,
    /// and never becomes `General`. The safe default.
    #[default]
    Project,
}

/// One idea that a change exercises. The unit of learning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub title: String,
    pub summary: String,
    pub scope: ConceptScope,
}

/// The shape of a question (ADR-0001 D7, ADR-0002 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuestionFormat {
    MultipleChoice,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionBody {
    MultipleChoice {
        prompt: String,
        options: Vec<String>,
        /// Index into `options`.
        correct: usize,
        /// Why each option is right or wrong, in option order (ADR-0004
        /// D1). `reasons[correct]` is the question's explanation. Shown only
        /// after grading. Empty for questions written before reasons
        /// existed, and in tests that don't need them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasons: Vec<String>,
    },
    Open {
        prompt: String,
        /// What a good answer must cover; used when grading.
        rubric: String,
    },
}

impl QuestionBody {
    /// Why `option` of a multiple-choice question is right or wrong, if the
    /// examiner gave a reason for it.
    pub fn reason(&self, option: usize) -> Option<&str> {
        match self {
            QuestionBody::MultipleChoice { reasons, .. } => reasons
                .get(option)
                .map(|r| r.trim())
                .filter(|r| !r.is_empty()),
            QuestionBody::Open { .. } => None,
        }
    }

    pub fn format(&self) -> QuestionFormat {
        match self {
            QuestionBody::MultipleChoice { .. } => QuestionFormat::MultipleChoice,
            QuestionBody::Open { .. } => QuestionFormat::Open,
        }
    }
}

/// A question about one concept, in one format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub concept: ConceptId,
    /// The scope of `concept`, which decides where its review is stored
    /// (ADR-0003 D5).
    pub scope: ConceptScope,
    pub body: QuestionBody,
    /// Why the correct answer is correct (multiple choice) or what a good
    /// answer covers (open). Shown only after grading (ADR-0004 D1).
    pub explanation: String,
}

impl Question {
    pub fn format(&self) -> QuestionFormat {
        self.body.format()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnswerBody {
    Choice(usize),
    Text(String),
}

/// The author's response to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: QuestionId,
    pub body: AnswerBody,
}

/// The grade of one answer. Maps one-to-one onto FSRS ratings (ADR-0001 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grade {
    /// Wrong.
    Again,
    /// Correct with significant gaps.
    Hard,
    /// Correct.
    Good,
    /// Correct and thorough.
    Easy,
}

/// The examiner's verdict on one answer: the grade and a short explanation
/// for the author (ADR-0004 D1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grading {
    pub grade: Grade,
    pub feedback: String,
}

impl Grade {
    /// Only `Again` is incorrect.
    pub fn is_correct(self) -> bool {
        !matches!(self, Grade::Again)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn questions_stored_before_option_reasons_still_load() {
        let stored = r#"{"MultipleChoice":{"prompt":"Q?","options":["a","b","c"],"correct":1}}"#;
        let body: QuestionBody = serde_json::from_str(stored).unwrap();
        assert_eq!(body.reason(0), None);
        let QuestionBody::MultipleChoice { reasons, .. } = &body else {
            panic!("not multiple choice");
        };
        assert!(reasons.is_empty());
        // And without reasons, they are stored as before.
        assert_eq!(serde_json::to_string(&body).unwrap(), stored);
    }

    #[test]
    fn a_reason_is_given_per_option() {
        let body = QuestionBody::MultipleChoice {
            prompt: "Q?".into(),
            options: vec!["a".into(), "b".into(), "c".into()],
            correct: 1,
            reasons: vec!["A is wrong.".into(), "B is right.".into(), "  ".into()],
        };
        assert_eq!(body.reason(0), Some("A is wrong."));
        assert_eq!(body.reason(2), None, "a blank reason is no reason");
        assert_eq!(body.reason(3), None);
    }
    use super::{Diff, QuestionBody};

    fn file(path: &str, line: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1 @@\n+{line}\n"
        )
    }

    // @adr-0002-d9
    #[test]
    fn a_diff_is_restricted_to_the_files_of_another() {
        let since = Diff(format!(
            "{}{}{}",
            file("src/lib.rs", "mine"),
            file("docs/merged.md", "from main"),
            file("src/a b.rs", "spaced")
        ));
        let pr = Diff(format!(
            "{}{}",
            file("src/lib.rs", "old"),
            file("src/a b.rs", "x")
        ));
        assert_eq!(
            since.files(),
            ["src/lib.rs", "docs/merged.md", "src/a b.rs"]
        );
        let kept = since.restricted_to_files_of(&pr);
        assert_eq!(
            kept.0,
            format!(
                "{}{}",
                file("src/lib.rs", "mine"),
                file("src/a b.rs", "spaced")
            )
        );
        assert!(since.restricted_to_files_of(&Diff::default()).is_empty());
        assert!(Diff("no header\n".into()).files().is_empty());
        assert!(
            Diff("no header\n".into())
                .restricted_to_files_of(&pr)
                .is_empty()
        );
    }
}
