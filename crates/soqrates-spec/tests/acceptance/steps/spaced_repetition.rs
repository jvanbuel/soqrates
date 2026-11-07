//! Steps for `spaced_repetition.feature` (ADR-0001 D6).

use std::collections::BTreeSet;

use cucumber::then;
use soqrates_core::{ConceptId, Session};

use super::{author, concepts};
use crate::world::SpecWorld;

fn session(w: &SpecWorld) -> &Session {
    w.session.as_ref().expect("no session started")
}

#[then(regex = r#"^the session asks (\d+) questions?$"#)]
fn asks_n(w: &mut SpecWorld, n: usize) {
    let s = session(w);
    assert_eq!(s.questions().len(), n, "questions: {:?}", s.questions());
}

#[then(regex = r#"^the session asks about (.+), in (that|any) order$"#)]
fn asks_about(w: &mut SpecWorld, list: String, order: String) {
    let asked: Vec<ConceptId> = session(w)
        .questions()
        .iter()
        .map(|q| q.concept.clone())
        .collect();
    let want = concepts(&list);
    if order == "that" {
        assert_eq!(asked, want, "concepts asked");
    } else {
        assert_eq!(asked.len(), want.len(), "concepts asked: {asked:?}");
        let asked: BTreeSet<_> = asked.into_iter().collect();
        let want: BTreeSet<_> = want.into_iter().collect();
        assert_eq!(asked, want, "concepts asked");
    }
}

#[then(regex = r#"^"([^"]+)" is due for review by "([^"]+)"$"#)]
async fn is_due(w: &mut SpecWorld, concept: String, id: String) {
    let due = w.app.due_reviews(&author(&id)).await.expect("due_reviews");
    assert!(
        due.contains(&ConceptId(concept.clone())),
        "{concept} not due for {id}: {due:?}"
    );
}

#[then(regex = r#"^no reviews are due for "([^"]+)"$"#)]
async fn none_due(w: &mut SpecWorld, id: String) {
    let due = w.app.due_reviews(&author(&id)).await.expect("due_reviews");
    assert!(due.is_empty(), "due for {id}: {due:?}");
}

#[then(regex = r#"^the reviews due for "([^"]+)" are (.+), in that order$"#)]
async fn due_in_order(w: &mut SpecWorld, id: String, list: String) {
    let due = w.app.due_reviews(&author(&id)).await.expect("due_reviews");
    assert_eq!(due, concepts(&list), "due reviews for {id}");
}

#[then(
    regex = r#"^the next review of "([^"]+)" for "([^"]+)" is later than the next review of "([^"]+)"$"#
)]
async fn later_than(w: &mut SpecWorld, a: String, id: String, b: String) {
    let p = w.profile(&author(&id)).await;
    let due = |c: &str| {
        p.cards()
            .get(&ConceptId(c.to_string()))
            .unwrap_or_else(|| panic!("{id} has no review card for {c}"))
            .due()
    };
    let (da, db) = (due(&a), due(&b));
    assert!(da > db, "{a} is due {da}, {b} is due {db}");
}
