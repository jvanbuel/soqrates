//! Steps for `question_feedback.feature` (ADR-0011): authors' feedback on
//! graded questions, on the fake forge and at the terminal.

// @adr-0011-d1 @adr-0011-d2 @adr-0011-d4 @adr-0011-d6 @adr-0011-d7 @adr-0011-d8 @adr-0011-d9

use cucumber::{given, then, when};
use soqrates_core::ci::run_check;
use soqrates_core::ports::StateStore;
use soqrates_core::{
    AskedQuestion, AskedSession, BankRef, BankedQuestion, ConceptId, FeedbackKind, Grade, PrRef,
    PullRequest, QuestionFeedback, Session,
};

use super::{author, correctness, grade, quoted};
use crate::fakes::{AnyStore, FakeCheckout, FakeExaminer, diff_touching};
use crate::world::SpecWorld;

fn latest_posted(w: &SpecWorld) -> Session {
    w.scm
        .posted_on(w.current_pr())
        .pop()
        .expect("no questions were posted on the pull request")
}

async fn check_runs(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    w.status = Some(
        run_check(&w.app, &w.scm, &FakeCheckout, &pr)
            .await
            .expect("run_check"),
    );
}

/// Every store that can keep asked sessions.
fn stores(w: &SpecWorld) -> Vec<&AnyStore> {
    std::iter::once(w.app.store.inner())
        .chain(w.app.org_stores.values().map(AnyStore::inner))
        .collect()
}

/// Every asked session in the scenario, oldest first.
async fn asked_sessions(w: &SpecWorld) -> Vec<AskedSession> {
    let mut all = Vec::new();
    for store in stores(w) {
        all.extend(store.asked_sessions().await.expect("asked_sessions"));
    }
    all.sort_by_key(|a| a.at());
    all
}

/// The latest graded session, as kept for feedback.
async fn graded_session(w: &SpecWorld) -> AskedSession {
    asked_sessions(w)
        .await
        .pop()
        .expect("no graded session was kept for feedback")
}

async fn graded_question(w: &SpecWorld, n: usize) -> AskedQuestion {
    let asked = graded_session(w).await;
    asked
        .questions()
        .get(n - 1)
        .unwrap_or_else(|| panic!("the graded session has no question {n}: {asked:?}"))
        .clone()
}

fn kind(name: &str) -> FeedbackKind {
    FeedbackKind::parse(name).unwrap_or_else(|| panic!("unknown feedback kind {name:?}"))
}

fn assert_feedback(feedback: &[QuestionFeedback], n: usize, name: &str, text: Option<&str>) {
    let of_kind: Vec<_> = feedback.iter().filter(|f| f.kind == kind(name)).collect();
    assert_eq!(of_kind.len(), n, "feedback: {feedback:?}");
    assert_eq!(feedback.len(), n, "only that feedback: {feedback:?}");
    if let Some(text) = text {
        assert_eq!(of_kind[0].text.as_deref(), Some(text));
    }
}

/// The bank question the latest disputed feedback is about.
async fn disputed(w: &SpecWorld) -> BankRef {
    asked_sessions(w)
        .await
        .iter()
        .flat_map(|a| a.questions().to_vec())
        .filter(|q| q.feedback.iter().any(|f| f.kind.disputes()))
        .find_map(|q| q.bank().cloned())
        .expect("no disputed bank question")
}

async fn bank_questions(w: &SpecWorld, concept: &str) -> Vec<BankedQuestion> {
    w.app
        .store
        .inner()
        .bank_questions(&ConceptId(concept.into()))
        .await
        .expect("bank_questions")
}

fn notes(w: &SpecWorld) -> Vec<String> {
    w.scm
        .state
        .lock()
        .unwrap()
        .feedback_notes
        .iter()
        .map(|(_, n)| n.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Given

#[given(
    regex = r#"^the author's (correct|wrong) answers to a pull request by "([^"]+)" touching (.+) were graded$"#
)]
async fn graded_pull_request(w: &mut SpecWorld, how: String, id: String, list: String) {
    let pr = PrRef {
        repo: "jvanbuel/soqrates".into(),
        number: 7,
    };
    w.scm.open(PullRequest {
        pr: pr.clone(),
        author: author(&id),
        head_sha: "abc123".into(),
        diff: diff_touching(&quoted(&list)),
    });
    w.pr = Some(pr);
    check_runs(w).await;
    let session = latest_posted(w);
    let answers = session
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, correctness(&how)))
        .collect();
    w.scm.reply(session.key(), answers);
    check_runs(w).await;
    assert_eq!(
        w.scm.state.lock().unwrap().results.len(),
        1,
        "the answers were graded"
    );
}

#[given(regex = r#"^the author comments "([^"]*)"$"#)]
#[when(regex = r#"^the author comments "([^"]*)"$"#)]
fn author_comments(w: &mut SpecWorld, body: String) {
    let session = latest_posted(w);
    let pr = w.current_pr().clone();
    w.scm.comment(&pr, session.key(), session.author(), &body);
}

#[when(regex = r#"^"([^"]+)" comments "([^"]*)"$"#)]
fn someone_comments(w: &mut SpecWorld, id: String, body: String) {
    let session = latest_posted(w);
    let pr = w.current_pr().clone();
    w.scm.comment(&pr, session.key(), &author(&id), &body);
}

#[given(regex = r#"^the question about "([^"]+)" came from the question bank$"#)]
async fn came_from_bank(w: &mut SpecWorld, concept: String) {
    let q = graded_session(w)
        .await
        .questions()
        .iter()
        .find(|q| q.question.concept.0 == concept)
        .cloned()
        .expect("no question about the concept");
    assert!(q.bank().is_some(), "{q:?}");
}

#[given(regex = r#"^the author will type "([^"]*)" at the feedback prompt$"#)]
fn will_type(w: &mut SpecWorld, line: String) {
    w.prompter.feedback = Some(line);
}

// ---------------------------------------------------------------------------
// When

#[when(regex = r"^a maintainer (accepts|rejects) that feedback$")]
async fn maintainer_resolves(w: &mut SpecWorld, verdict: String) {
    let id = graded_session(w)
        .await
        .questions()
        .iter()
        .flat_map(|q| q.feedback.clone())
        .last()
        .expect("no feedback to resolve")
        .id;
    w.app
        .resolve_feedback(&id, verdict == "accepts")
        .await
        .expect("resolve_feedback");
}

#[when("the asked questions are exported with feedback")]
async fn exported(w: &mut SpecWorld) {
    w.export = w
        .app
        .export_questions(true)
        .await
        .expect("export_questions");
}

// ---------------------------------------------------------------------------
// Then

#[then(
    regex = r"^(\d+) feedback notes? (?:was|were) posted, saying (the feedback was recorded|the feedback was not recorded|there are no graded results yet)$"
)]
fn notes_posted(w: &mut SpecWorld, n: usize, saying: String) {
    let notes = notes(w);
    assert_eq!(notes.len(), n, "notes: {notes:?}");
    let needle = match saying.as_str() {
        "the feedback was recorded" => "was recorded",
        "the feedback was not recorded" => "was not recorded",
        _ => "no graded results",
    };
    for note in &notes {
        assert!(note.contains(needle), "{note:?} doesn't say {saying:?}");
    }
}

#[then("no feedback note was posted")]
fn no_notes(w: &mut SpecWorld) {
    assert_eq!(notes(w), Vec::<String>::new());
}

#[then(
    regex = r#"^question (\d+) of the graded session has (\d+) "([^"]+)" feedback(?: saying "([^"]*)")?$"#
)]
async fn question_has_feedback(
    w: &mut SpecWorld,
    number: usize,
    n: usize,
    name: String,
    text: String,
) {
    let q = graded_question(w, number).await;
    let text = (!text.is_empty()).then_some(text.as_str());
    assert_feedback(&q.feedback, n, &name, text);
}

#[then(
    regex = r"^question (\d+) of the graded session is kept with the author's answer and the grade (\w+)$"
)]
async fn kept_with_answer(w: &mut SpecWorld, number: usize, g: String) {
    let q = graded_question(w, number).await;
    assert!(q.answer.is_some(), "{q:?}");
    assert_eq!(q.grade, grade(&g));
    assert!(!q.question.body.prompt().is_empty());
    assert!(!q.shown.is_empty(), "what the author was shown: {q:?}");
}

#[then("no question of the graded session has feedback")]
async fn no_feedback(w: &mut SpecWorld) {
    let asked = graded_session(w).await;
    for q in asked.questions() {
        assert!(q.feedback.is_empty(), "{q:?}");
    }
}

#[then(
    regex = r#"^the store of organisation "([^"]+)" keeps (\d+) "([^"]+)" feedback on question (\d+)$"#
)]
async fn org_keeps(w: &mut SpecWorld, org: String, n: usize, name: String, number: usize) {
    let store = w.app.org_stores[&org].inner();
    let asked = store
        .asked_sessions()
        .await
        .expect("asked_sessions")
        .pop()
        .expect("the organisation keeps no asked session");
    assert_feedback(&asked.questions()[number - 1].feedback, n, &name, None);
}

#[then("the learner store keeps no asked question")]
async fn learner_store_keeps_none(w: &mut SpecWorld) {
    let kept = w.app.store.inner().asked_sessions().await.expect("asked");
    assert!(kept.is_empty(), "{kept:?}");
}

#[then("no store keeps an asked question")]
async fn none_kept(w: &mut SpecWorld) {
    let kept = asked_sessions(w).await;
    assert!(kept.is_empty(), "{kept:?}");
}

#[then(regex = r#"^the question about "([^"]+)" is not the disputed bank question$"#)]
async fn not_the_disputed(w: &mut SpecWorld, concept: String) {
    let disputed = disputed(w).await;
    let session = w.session.as_ref().expect("no session");
    let q = session
        .questions()
        .iter()
        .find(|q| q.concept.0 == concept)
        .expect("no question about the concept");
    assert_ne!(
        q.banked.as_ref().map(|b| &b.id),
        Some(&disputed.id),
        "{q:?}"
    );
}

#[then(regex = r#"^the bank question about "([^"]+)" is retired$"#)]
async fn bank_retired(w: &mut SpecWorld, concept: String) {
    let bank = bank_questions(w, &concept).await;
    assert_eq!(bank.len(), 1, "{bank:?}");
    assert!(bank[0].is_retired(), "{bank:?}");
    assert!(!bank[0].is_disputed(), "the dispute is resolved: {bank:?}");
}

#[then(regex = r#"^no bank question about "([^"]+)" is disputed$"#)]
async fn none_disputed(w: &mut SpecWorld, concept: String) {
    let bank = bank_questions(w, &concept).await;
    assert!(!bank.is_empty());
    assert!(bank.iter().all(|q| !q.is_disputed()), "{bank:?}");
}

#[then(regex = r"^the export has (\d+) lines?$")]
fn export_lines(w: &mut SpecWorld, n: usize) {
    assert_eq!(w.export.len(), n, "{:?}", w.export);
}

#[then(
    regex = r#"^line (\d+) of the export has the question, the author's answer, the grade (\w+) and (\d+) "([^"]+)" feedback saying "([^"]*)"$"#
)]
fn export_line_full(
    w: &mut SpecWorld,
    line: usize,
    g: String,
    n: usize,
    name: String,
    text: String,
) {
    let l = &w.export[line - 1];
    assert!(l["body"].is_object(), "{l}");
    assert!(
        !l["explanation"].as_str().unwrap_or_default().is_empty(),
        "{l}"
    );
    assert!(!l["answer"].is_null(), "{l}");
    assert_eq!(l["grade"], serde_json::json!(grade(&g)), "{l}");
    export_feedback(l, n, &name, Some(&text));
}

#[then(regex = r#"^line (\d+) of the export has (\d+) "([^"]+)" feedback$"#)]
fn export_line(w: &mut SpecWorld, line: usize, n: usize, name: String) {
    export_feedback(&w.export[line - 1], n, &name, None);
}

fn export_feedback(line: &serde_json::Value, n: usize, name: &str, text: Option<&str>) {
    let feedback = line["feedback"].as_array().expect("feedback entries");
    assert_eq!(feedback.len(), n, "{line}");
    assert!(feedback.iter().all(|f| f["kind"] == name), "{line}");
    if let Some(text) = text {
        assert_eq!(feedback[0]["text"], text, "{line}");
    }
}

#[then(regex = r#"^the export never names "([^"]+)" or the session$"#)]
fn export_names_nobody(w: &mut SpecWorld, id: String) {
    let text = serde_json::to_string(&w.export).unwrap();
    let key = latest_posted(w).key().0.clone();
    assert!(!text.contains(&id), "{text}");
    assert!(!text.contains(&key), "{text}");
    let reviewers: Vec<_> = w.export.iter().map(|l| l["reviewer"].clone()).collect();
    assert!(
        reviewers
            .iter()
            .all(|r| *r == reviewers[0] && r.is_string())
    );
}

#[then("the author was told the feedback was recorded")]
fn told_recorded(w: &mut SpecWorld) {
    let noted = &w.prompter.noted;
    assert_eq!(noted.len(), 1, "{noted:?}");
    assert!(noted[0].contains("was recorded"), "{noted:?}");
}

#[then("the author was told nothing about feedback")]
fn told_nothing(w: &mut SpecWorld) {
    assert!(w.prompter.noted.is_empty(), "{:?}", w.prompter.noted);
    // The local session was graded all the same, and kept for feedback.
    assert!(w.hook_runs.last().is_some_and(|r| {
        r.reported
            .iter()
            .all(|o| o.grades.iter().all(|(_, g)| *g == Grade::Again))
    }));
}
