//! Steps for `learner_state.feature` (ADR-0001 D5).

use std::collections::BTreeSet;
use std::path::Path;

use cucumber::{given, then, when};
use soqrates_core::ports::StateStore;

use super::{author, concepts, level};
use crate::world::SpecWorld;

#[when(regex = r#"^I look up the profile of "([^"]+)"$"#)]
async fn look_up(w: &mut SpecWorld, id: String) {
    w.looked_up = Some(w.profile(&author(&id)).await);
}

#[then(regex = r#"^the profile belongs to "([^"]+)"$"#)]
async fn profile_belongs_to(w: &mut SpecWorld, id: String) {
    let p = w.profile(&author(&id)).await;
    assert_eq!(p.author, author(&id));
}

#[then(regex = r#"^the profile of "([^"]+)" is at level (\w+)$"#)]
async fn at_level(w: &mut SpecWorld, id: String, lvl: String) {
    let p = w.profile(&author(&id)).await;
    assert_eq!(p.level, level(&lvl), "level of {id}");
}

#[then(regex = r#"^the profile of "([^"]+)" is at a level (higher|lower) than (\w+)$"#)]
async fn at_level_relative(w: &mut SpecWorld, id: String, dir: String, lvl: String) {
    let p = w.profile(&author(&id)).await;
    let reference = level(&lvl);
    match dir.as_str() {
        "higher" => assert!(p.level > reference, "{id} is {:?}, not above {reference:?}", p.level),
        _ => assert!(p.level < reference, "{id} is {:?}, not below {reference:?}", p.level),
    }
}

#[then(regex = r#"^"([^"]+)" has no review cards$"#)]
async fn no_cards(w: &mut SpecWorld, id: String) {
    let p = w.profile(&author(&id)).await;
    assert!(p.cards.is_empty(), "{id} has cards: {:?}", p.cards.keys());
}

#[then(regex = r#"^"([^"]+)" has review cards for exactly (.+)$"#)]
async fn cards_exactly(w: &mut SpecWorld, id: String, list: String) {
    let p = w.profile(&author(&id)).await;
    let have: BTreeSet<_> = p.cards.keys().cloned().collect();
    let want: BTreeSet<_> = concepts(&list).into_iter().collect();
    assert_eq!(have, want, "review cards of {id}");
}

#[then(regex = r#"^the history of "([^"]+)" has (\d+) graded answers?$"#)]
async fn history_len(w: &mut SpecWorld, id: String, n: usize) {
    let p = w.profile(&author(&id)).await;
    assert_eq!(p.history.len(), n, "history of {id}: {:?}", p.history);
}

#[then("the session is stored as pending")]
async fn session_pending(w: &mut SpecWorld) {
    let key = w.session.as_ref().expect("no session started").key.clone();
    let stored = w.app.store.load_session(&key).await.expect("load_session");
    assert_eq!(stored.as_ref(), w.session.as_ref(), "stored session");
}

#[then("the session is no longer stored as pending")]
async fn session_not_pending(w: &mut SpecWorld) {
    let key = w.session.as_ref().expect("no session started").key.clone();
    let stored = w.app.store.load_session(&key).await.expect("load_session");
    assert!(stored.is_none(), "session {key:?} is still pending");
}

#[given("Soqrates keeps state in a file store in a fresh directory")]
fn file_store(w: &mut SpecWorld) {
    w.use_file_store();
}

#[when("Soqrates restarts with the same state directory")]
fn restart(w: &mut SpecWorld) {
    w.restart();
}

fn json_documents(dir: &Path) -> Vec<String> {
    let mut docs = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read state directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                docs.push(std::fs::read_to_string(&path).expect("read JSON document"));
            }
        }
    }
    docs
}

#[then(regex = r#"^the state directory holds exactly one JSON document mentioning "([^"]+)"$"#)]
fn one_document(w: &mut SpecWorld, id: String) {
    let dir = w.state_dir.as_ref().expect("no file store in this scenario");
    let n = json_documents(dir)
        .iter()
        .filter(|d| d.contains(&id))
        .count();
    assert_eq!(n, 1, "JSON documents mentioning {id}");
}

#[then(regex = r#"^no JSON document in the state directory mentions both "([^"]+)" and "([^"]+)"$"#)]
fn no_shared_document(w: &mut SpecWorld, a: String, b: String) {
    let dir = w.state_dir.as_ref().expect("no file store in this scenario");
    assert!(
        !json_documents(dir)
            .iter()
            .any(|d| d.contains(&a) && d.contains(&b)),
        "a JSON document mentions both {a} and {b}"
    );
}
