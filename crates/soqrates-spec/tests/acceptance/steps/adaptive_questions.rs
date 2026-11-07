//! Steps for `adaptive_questions.feature` (ADR-0001 D7).

use chrono::Duration;
use cucumber::{given, then, when};
use soqrates_core::policy::{format_for, next_level};
use soqrates_core::ports::Clock;
use soqrates_core::{ConceptId, Grade, GradedAnswer, Identity, Level, QuestionFormat, Session};

use super::{author, correctness, format, level};
use crate::world::SpecWorld;

fn session(w: &SpecWorld) -> &Session {
    w.session.as_ref().expect("no session started")
}

fn formats(s: &Session) -> Vec<QuestionFormat> {
    s.questions().iter().map(|q| q.body.format()).collect()
}

#[then(regex = r"^every question in the session is (multiple-choice|open)$")]
fn every_question(w: &mut SpecWorld, f: String) {
    let s = session(w);
    assert!(!s.questions().is_empty(), "the session asks nothing");
    let want = format(&f);
    assert!(
        formats(s).iter().all(|g| *g == want),
        "formats: {:?}",
        formats(s)
    );
}

#[then(regex = r"^the session contains at least one (multiple-choice|open) question$")]
fn at_least_one(w: &mut SpecWorld, f: String) {
    let s = session(w);
    assert!(
        formats(s).contains(&format(&f)),
        "formats: {:?}",
        formats(s)
    );
}

#[then("the session mixes multiple-choice and open questions")]
fn mixes(w: &mut SpecWorld) {
    let f = formats(session(w));
    assert!(
        f.contains(&QuestionFormat::MultipleChoice) && f.contains(&QuestionFormat::Open),
        "formats: {f:?}"
    );
}

#[then(regex = r"^the question formats follow the policy for (\w+) authors$")]
fn follow_policy(w: &mut SpecWorld, lvl: String) {
    let lvl = level(&lvl);
    let s = session(w);
    assert!(!s.questions().is_empty(), "the session asks nothing");
    let want: Vec<_> = (0..s.questions().len())
        .map(|i| format_for(lvl, i))
        .collect();
    assert_eq!(formats(s), want, "formats for a {lvl:?} author");
}

#[then(
    regex = r"^the first (\d+) questions for an? (\w+) author are (all multiple-choice|all open|a mix of multiple-choice and open)$"
)]
fn first_questions(_w: &mut SpecWorld, n: usize, lvl: String, expect: String) {
    let lvl = level(&lvl);
    let f: Vec<_> = (0..n).map(|i| format_for(lvl, i)).collect();
    let mc = f.contains(&QuestionFormat::MultipleChoice);
    let open = f.contains(&QuestionFormat::Open);
    match expect.as_str() {
        "all multiple-choice" => assert!(mc && !open, "{lvl:?}: {f:?}"),
        "all open" => assert!(open && !mc, "{lvl:?}: {f:?}"),
        _ => assert!(mc && open, "{lvl:?}: {f:?}"),
    }
}

#[given(regex = r"^an? (\w+) author with a history of (\d+) (correct|wrong) answers?$")]
fn history_of(w: &mut SpecWorld, lvl: String, n: usize, how: String) {
    let lvl = level(&lvl);
    let correct = correctness(&how);
    let start = w.clock.now();
    let history = (0..n)
        .map(|i| GradedAnswer {
            concept: ConceptId(format!("history.c{i}")),
            format: format_hint(lvl),
            grade: if correct { Grade::Good } else { Grade::Again },
            at: start + Duration::minutes(i as i64),
            via: Identity::parse("github:1001").expect("identity"),
        })
        .collect();
    w.recompute = Some((lvl, history));
}

/// The format a question would plausibly have had; `next_level` must not
/// depend on the policy being implemented, so this avoids `format_for`.
fn format_hint(lvl: Level) -> QuestionFormat {
    match lvl {
        Level::Expert => QuestionFormat::Open,
        _ => QuestionFormat::MultipleChoice,
    }
}

#[when("their level is recomputed")]
fn recompute(w: &mut SpecWorld) {
    let (current, history) = w.recompute.as_ref().expect("no history given");
    w.recomputed = Some(next_level(*current, history));
}

#[then(regex = r"^the recomputed level is (.+)$")]
fn recomputed(w: &mut SpecWorld, expect: String) {
    let got = w.recomputed.expect("level not recomputed");
    if let Some(l) = expect.strip_prefix("higher than ") {
        assert!(got > level(l), "{got:?} is not higher than {l}");
    } else if let Some(l) = expect.strip_prefix("lower than ") {
        assert!(got < level(l), "{got:?} is not lower than {l}");
    } else {
        assert_eq!(got, level(&expect));
    }
}

#[then(regex = r#"^the last outcome reports the level stored in the profile of "([^"]+)"$"#)]
async fn outcome_level(w: &mut SpecWorld, id: String) {
    let p = w.profile(&author(&id)).await;
    let outcome = w.outcome.as_ref().expect("no session submitted");
    assert_eq!(outcome.level_after, p.level());
}
