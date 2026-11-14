//! Step definitions, one module per feature area, plus shared parsing helpers.

mod adaptive_questions;
mod answer_commands;
mod ci_check;
mod codebase;
mod common;
mod domain_model;
mod feedback;
mod grading_journal;
mod learner_identity;
mod learner_state;
mod local_hooks;
mod mcp_server;
mod question_bank;
mod question_kinds;
mod spaced_repetition;

use std::collections::HashMap;
use std::sync::LazyLock;

use cucumber::gherkin::Step;
use regex::Regex;
use soqrates_core::{
    AuthorId, CheckStatus, ConceptId, Grade, HookKind, Level, Mode, QuestionFormat,
};

/// `"a", "b", "c"` -> `["a", "b", "c"]`.
pub fn quoted(list: &str) -> Vec<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)""#).unwrap());
    let items: Vec<String> = RE.captures_iter(list).map(|c| c[1].to_string()).collect();
    assert!(
        !items.is_empty(),
        "expected a list of quoted items, got: {list}"
    );
    items
}

pub fn concepts(list: &str) -> Vec<ConceptId> {
    quoted(list).into_iter().map(ConceptId).collect()
}

pub fn author(id: &str) -> AuthorId {
    AuthorId(id.to_string())
}

pub fn level(s: &str) -> Level {
    match s {
        "Novice" => Level::Novice,
        "Intermediate" => Level::Intermediate,
        "Expert" => Level::Expert,
        other => panic!("unknown level {other:?}"),
    }
}

pub fn mode(s: &str) -> Mode {
    match s {
        "Advisory" => Mode::Advisory,
        "Blocking" => Mode::Blocking,
        other => panic!("unknown mode {other:?}"),
    }
}

pub fn grade(s: &str) -> Grade {
    match s {
        "Again" => Grade::Again,
        "Hard" => Grade::Hard,
        "Good" => Grade::Good,
        "Easy" => Grade::Easy,
        other => panic!("unknown grade {other:?}"),
    }
}

pub fn format(s: &str) -> QuestionFormat {
    match s {
        "multiple-choice" => QuestionFormat::MultipleChoice,
        "open" => QuestionFormat::Open,
        other => panic!("unknown question format {other:?}"),
    }
}

pub fn check_status(s: &str) -> CheckStatus {
    match s {
        "Pending" => CheckStatus::Pending,
        "Success" => CheckStatus::Success,
        "Failure" => CheckStatus::Failure,
        "Neutral" => CheckStatus::Neutral,
        other => panic!("unknown check status {other:?}"),
    }
}

pub fn hook_kind(s: &str) -> HookKind {
    match s {
        "pre-commit" => HookKind::PreCommit,
        "pre-push" => HookKind::PrePush,
        other => panic!("unknown hook {other:?}"),
    }
}

/// `correct`/`correctly` -> true, `wrong`/`wrongly` -> false.
pub fn correctness(s: &str) -> bool {
    match s {
        "correct" | "correctly" => true,
        "wrong" | "wrongly" => false,
        other => panic!("expected correct or wrong, got {other:?}"),
    }
}

/// A `| concept | answer |` table as concept -> answered correctly.
pub fn answer_table(step: &Step) -> HashMap<ConceptId, bool> {
    let table = step.table.as_ref().expect("this step needs a data table");
    let mut rows = table.rows.iter();
    let header = rows.next().expect("table header");
    assert_eq!(
        header,
        &["concept", "answer"],
        "table columns must be | concept | answer |"
    );
    rows.map(|r| (ConceptId(r[0].clone()), correctness(&r[1])))
        .collect()
}
