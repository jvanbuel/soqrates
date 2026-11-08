//! Steps for `learner_identity.feature` (ADR-0003 D1-D7).
//!
//! Pull requests go through the CI check with the fake forge; local commits
//! through the hook workflow. Usernames and verified e-mails of forge
//! accounts are tracked only in the world: they are not in the domain on
//! purpose (D1), so renaming is a no-op for Soqrates.

// @adr-0003-d1 @adr-0003-d2 @adr-0003-d3 @adr-0003-d4 @adr-0003-d5 @adr-0003-d6 @adr-0003-d7

use std::collections::{BTreeMap, BTreeSet};

use chrono::Duration;
use cucumber::{given, then, when};
use soqrates_core::ci::run_check;
use soqrates_core::hook::run_hook;
use soqrates_core::ports::{Clock, StateStore};
use soqrates_core::{
    AuthorId, ConceptId, ConceptScope, Error, Grade, HookKind, Identity, LearnerId, LearnerProfile,
    LinkRejection, PrRef, PullRequest, Verification, local_identity,
};

use super::{author, grade, level, quoted};
use crate::fakes::{AnyStore, CORRECT_TEXT, FakeClock, FakeExaminer, WRONG_TEXT, diff_touching};
use crate::world::SpecWorld;

// ---------------------------------------------------------------------------
// helpers

fn identity(text: &str) -> Identity {
    Identity::parse(text).unwrap_or_else(|e| panic!("{text:?} is not an identity: {e}"))
}

/// The learner behind `who`, as the engine resolves it (creating it if new).
async fn learner_of(w: &mut SpecWorld, who: &AuthorId) -> LearnerId {
    let l = w.app.learner_of(who).await.expect("learner_of");
    if !w.identity.seen.contains(&l) {
        w.identity.seen.push(l.clone());
    }
    l
}

async fn name_learner(w: &mut SpecWorld, name: &str, who: &AuthorId) {
    let l = learner_of(w, who).await;
    w.identity.learners.insert(name.to_string(), l);
    w.identity
        .identity_of
        .entry(name.to_string())
        .or_insert_with(|| who.clone());
}

fn identity_of_name(w: &SpecWorld, name: &str) -> AuthorId {
    w.identity
        .identity_of
        .get(name)
        .unwrap_or_else(|| panic!("no learner called {name:?} in this scenario"))
        .clone()
}

/// The learner called `name` now, as the learner store links it (without
/// creating anything). Falls back to the learner it first resolved to.
async fn current_learner(w: &SpecWorld, name: &str) -> LearnerId {
    let who = identity_of_name(w, name);
    let linked = w
        .app
        .store
        .inner()
        .learner_of(&identity(&who.0))
        .await
        .expect("learner_of in the learner store");
    linked.unwrap_or_else(|| w.identity.learners[name].clone())
}

fn next(w: &mut SpecWorld) -> usize {
    w.identity.counter += 1;
    w.identity.counter
}

/// Open a pull request by `who` and run the Soqrates check once, which
/// starts the session and posts its questions.
async fn open_pr(w: &mut SpecWorld, who: &str, pr: Option<PrRef>, concepts: &[String]) {
    let n = next(w);
    let pr = pr.unwrap_or_else(|| PrRef {
        repo: format!("{}/repo", identity(who).host().unwrap_or("local")),
        number: n as u64,
    });
    w.scm.open(PullRequest {
        pr: pr.clone(),
        author: author(who),
        head_sha: format!("head-{n}"),
        diff: diff_touching(concepts),
    });
    w.pr = Some(pr.clone());
    let status = run_check(&w.app, &w.scm, &pr).await.expect("run_check");
    w.status = Some(status);
    let session = w
        .scm
        .posted_on(&pr)
        .pop()
        .expect("no questions were posted on the pull request");
    w.identity.session_keys.push(session.key().clone());
    w.identity
        .questions
        .extend(session.questions().iter().cloned());
    w.session = Some(session);
}

/// The author answers the latest posted questions and the check runs again,
/// which grades them.
async fn answer_pr(w: &mut SpecWorld, correct: bool) {
    let session = w.session.clone().expect("no session posted");
    let answers = session
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, correct))
        .collect();
    w.scm.reply(session.key(), answers);
    let pr = w.current_pr().clone();
    let status = run_check(&w.app, &w.scm, &pr).await.expect("run_check");
    w.status = Some(status);
}

/// `who` answers a session about `concepts` through the engine, and the
/// world remembers each review for the replay reference (D3).
async fn quiz(w: &mut SpecWorld, who: &str, concepts: &[String], correct: bool) {
    let a = author(who);
    let session = w.start_touching(&a, concepts).await;
    w.identity.session_keys.push(session.key().clone());
    w.identity
        .questions
        .extend(session.questions().iter().cloned());
    let g = if correct {
        w.examiner.correct_grade()
    } else {
        Grade::Again
    };
    let at = w.clock.now();
    for q in session.questions() {
        w.identity
            .reviews
            .push((q.concept.clone(), q.format(), g, at, identity(who)));
    }
    w.submit_all(&session, correct).await;
}

/// Move the clock to the start of day `day` (day 1 is the first day).
fn on_day(w: &mut SpecWorld, day: i64) {
    let start = FakeClock::default().now();
    w.clock.set(start + Duration::days(day - 1));
}

async fn request_code(w: &mut SpecWorld, who: &str) {
    let code = format!("link-code-{}", next(w));
    let issued = w
        .app
        .request_link_code(&author(who), code)
        .await
        .expect("request_link_code");
    w.identity.link_code = Some(issued);
}

async fn present_code(w: &mut SpecWorld, who: &str) -> soqrates_core::Result<LearnerId> {
    let code = w
        .identity
        .link_code
        .clone()
        .expect("no link code was requested");
    let requester = AuthorId(code.requested_by().to_string());
    let before = w
        .app
        .identities(&requester)
        .await
        .expect("identities of the requester");
    w.identity.before_presenting = Some(before);
    w.app.present_link_code(&author(who), code.code()).await
}

async fn link(w: &mut SpecWorld, a: &str, b: &str) -> LearnerId {
    request_code(w, a).await;
    present_code(w, b).await.expect("present_link_code")
}

fn link_rejection(w: &SpecWorld) -> &LinkRejection {
    match w
        .identity
        .link_result
        .as_ref()
        .expect("no link code presented")
    {
        Err(Error::Link(r)) => r,
        other => panic!("expected the link to be rejected, got {other:?}"),
    }
}

/// Every learner the scenario knows of, as currently linked.
async fn known_learners(w: &SpecWorld) -> BTreeSet<LearnerId> {
    let mut all: BTreeSet<LearnerId> = w.identity.seen.iter().cloned().collect();
    all.extend(w.identity.learners.values().cloned());
    let names: Vec<String> = w.identity.identity_of.keys().cloned().collect();
    for name in names {
        all.insert(current_learner(w, &name).await);
    }
    all
}

/// Everything the scenario can find in `store` for the known learners and
/// sessions, serialized (D5).
pub(super) async fn dump(w: &SpecWorld, store: &AnyStore) -> String {
    let mut parts = Vec::new();
    for l in known_learners(w).await {
        let profile = store.load_profile(&l).await.expect("load_profile");
        let links = store.links_of(&l).await.expect("links_of");
        parts.push(serde_json::to_value((&l, profile, links)).expect("serialize"));
    }
    for key in &w.identity.session_keys {
        let session = store.load_session(key).await.expect("load_session");
        parts.push(serde_json::to_value(session).expect("serialize"));
    }
    serde_json::to_string(&parts).expect("serialize")
}

fn has_review_of(profile: &Option<LearnerProfile>, concept: &ConceptId) -> bool {
    profile
        .as_ref()
        .is_some_and(|p| p.history().iter().any(|r| r.concept == *concept))
}

pub(super) fn org_store<'a>(w: &'a SpecWorld, org: &str) -> &'a AnyStore {
    w.app
        .org_stores
        .get(org)
        .unwrap_or_else(|| panic!("no store for organisation {org:?}"))
        .inner()
}

// ---------------------------------------------------------------------------
// Background

#[given(regex = r#"^the forge "([^"]+)" is administered by organisation "([^"]+)"$"#)]
fn administered_by(w: &mut SpecWorld, host: String, org: String) {
    w.app.config.forges.insert(host, org.clone());
    w.app.org_stores.entry(org).or_insert_with(AnyStore::memory);
}

#[given("all forges use the same learner store")]
fn same_learner_store(w: &mut SpecWorld) {
    // One engine, one learner store (`app.store`) for every forge.
    assert!(
        !matches!(w.app.store, AnyStore::Unreachable(_)),
        "the learner store is unreachable"
    );
}

// ---------------------------------------------------------------------------
// D1: profiles belong to learners

#[given(
    regex = r#"^identity "([^"]+)"(?: with (username|verified e-mail) "([^"]+)")? belongs to learner "([^"]+)"$"#
)]
async fn identity_belongs_to(
    w: &mut SpecWorld,
    who: String,
    kind: String,
    value: String,
    name: String,
) {
    match kind.as_str() {
        "username" => {
            w.identity.usernames.insert(who.clone(), value);
        }
        "verified e-mail" => {
            w.identity.verified_emails.insert(who.clone(), value);
        }
        _ => {}
    }
    name_learner(w, &name, &author(&who)).await;
}

#[given(regex = r#"^identity "([^"]+)" is renamed to "([^"]+)"$"#)]
#[when(regex = r#"^the username of identity "([^"]+)" changes to "([^"]+)"$"#)]
fn renamed(w: &mut SpecWorld, who: String, username: String) {
    // Usernames are not identities (D1): a rename is invisible to Soqrates.
    w.identity.usernames.insert(who, username);
}

#[when(regex = r#"^identity "([^"]+)" opens a pull request touching (.+)$"#)]
async fn opens_pr(w: &mut SpecWorld, who: String, list: String) {
    open_pr(w, &who, None, &quoted(&list)).await;
}

#[when(
    regex = r#"^a new identity "([^"]+)" takes the username "([^"]+)" and opens a pull request$"#
)]
async fn takes_username(w: &mut SpecWorld, who: String, username: String) {
    assert!(
        !w.identity.usernames.values().any(|u| *u == username),
        "the username {username:?} is still taken"
    );
    w.identity.usernames.insert(who.clone(), username);
    open_pr(w, &who, None, &["rust.traits".to_string()]).await;
}

#[then(regex = r#"^identity "([^"]+)" belongs to a new learner$"#)]
async fn belongs_to_new_learner(w: &mut SpecWorld, who: String) {
    let a = author(&who);
    let l = w.app.learner_of(&a).await.expect("learner_of");
    assert!(
        !w.identity.seen.contains(&l),
        "{who} resolves to a learner seen before in this scenario"
    );
    w.identity.seen.push(l.clone());
    let links = w.app.store.inner().links_of(&l).await.expect("links_of");
    let linked: Vec<_> = links.iter().map(|k| k.identity.clone()).collect();
    assert_eq!(
        linked,
        vec![identity(&who)],
        "identities of the new learner"
    );
    let p = w.app.profile(&a).await.expect("profile");
    assert!(p.history().is_empty(), "the new learner has reviews");
    w.identity.that_identity = Some(a);
}

#[then(regex = r"^that learner's level is (\w+)$")]
async fn that_learners_level(w: &mut SpecWorld, lvl: String) {
    let a = w.identity.that_identity.clone().expect("no new learner");
    let p = w.app.profile(&a).await.expect("profile");
    assert_eq!(p.level(), level(&lvl));
}

#[then(regex = r#"^the session is for learner "([^"]+)"$"#)]
async fn session_is_for(w: &mut SpecWorld, name: String) {
    let session = w.session.clone().expect("no session");
    let l = w
        .app
        .learner_of(session.author())
        .await
        .expect("learner_of");
    assert_eq!(l, current_learner(w, &name).await, "learner of the session");
}

#[then(regex = r#"^learner "([^"]+)" has no review card for "([^"]+)"$"#)]
async fn has_no_card(w: &mut SpecWorld, name: String, concept: String) {
    let l = current_learner(w, &name).await;
    let p = w
        .app
        .store
        .inner()
        .load_profile(&l)
        .await
        .expect("load_profile");
    if let Some(p) = p {
        assert!(
            p.card(&ConceptId(concept.clone())).is_none(),
            "{name} has a review card for {concept}"
        );
    }
}

// ---------------------------------------------------------------------------
// D2: linking requires proof

#[given(regex = r#"^identity "([^"]+)" (?:requests|requested) a link code$"#)]
#[when(regex = r#"^identity "([^"]+)" (?:requests|requested) a link code$"#)]
async fn requests_code(w: &mut SpecWorld, who: String) {
    request_code(w, &who).await;
}

#[when(regex = r#"^identity "([^"]+)" presents that link code$"#)]
async fn presents_code(w: &mut SpecWorld, who: String) {
    let result = present_code(w, &who).await;
    w.identity.link_result = Some(result);
}

#[given(regex = r#"^identity "([^"]+)" already presented that code$"#)]
async fn already_presented(w: &mut SpecWorld, who: String) {
    present_code(w, &who)
        .await
        .expect("the first presentation is accepted");
}

#[given(regex = r"^(\d+) minutes have passed$")]
fn minutes_pass(w: &mut SpecWorld, minutes: i64) {
    w.clock.advance(Duration::minutes(minutes));
}

#[given("nothing else has happened")]
fn nothing_else(_w: &mut SpecWorld) {}

#[then(regex = r#"^identities "([^"]+)" and "([^"]+)" belong to the same learner$"#)]
async fn same_learner(w: &mut SpecWorld, a: String, b: String) {
    if let Some(r) = &w.identity.link_result {
        assert!(r.is_ok(), "the link was rejected: {r:?}");
    }
    if let Some(r) = &w.identity.declaration {
        assert!(r.is_ok(), "the declaration was rejected: {r:?}");
    }
    let la = learner_of(w, &author(&a)).await;
    let lb = learner_of(w, &author(&b)).await;
    assert_eq!(la, lb, "learners of {a} and {b}");
    let ids: BTreeSet<_> = w
        .app
        .identities(&author(&a))
        .await
        .expect("identities")
        .into_iter()
        .collect();
    assert!(
        ids.contains(&identity(&a)) && ids.contains(&identity(&b)),
        "identities of the learner: {ids:?}"
    );
}

#[then(
    regex = r"^the link is rejected because the code is (expired|already used|from the same identity)$"
)]
fn rejected_because_code(w: &mut SpecWorld, reason: String) {
    let want = match reason.as_str() {
        "expired" => LinkRejection::Expired,
        "already used" => LinkRejection::AlreadyUsed,
        _ => LinkRejection::SameIdentity,
    };
    assert_eq!(*link_rejection(w), want);
}

#[then(regex = r#"^the learner of "([^"]+)" gained no identity from that link code$"#)]
async fn gained_no_identity(w: &mut SpecWorld, who: String) {
    let before: BTreeSet<_> = w
        .identity
        .before_presenting
        .clone()
        .expect("no link code presented")
        .into_iter()
        .collect();
    let now: BTreeSet<_> = w
        .app
        .identities(&author(&who))
        .await
        .expect("identities")
        .into_iter()
        .collect();
    assert_eq!(now, before, "identities of the learner of {who}");
}

#[then(regex = r#"^the link is rejected because "([^"]+)" already belongs to another learner$"#)]
fn rejected_belongs_elsewhere(w: &mut SpecWorld, who: String) {
    assert_eq!(
        *link_rejection(w),
        LinkRejection::BelongsToAnotherLearner(identity(&who))
    );
}

#[when(
    regex = r#"^a local commit by "([^"]+)" is checked by the pre-commit hook in a repository without a configured identity$"#
)]
async fn local_commit_unconfigured(w: &mut SpecWorld, email: String) {
    let id = local_identity(Some("github.com"), &email, &BTreeMap::new());
    run_local_hook(w, id).await;
}

async fn run_local_hook(w: &mut SpecWorld, id: Identity) {
    let a = AuthorId(id.to_string());
    run_hook(
        &w.app,
        &mut w.prompter,
        HookKind::PreCommit,
        &a,
        &diff_touching(&["rust.traits".to_string()]),
    )
    .await
    .expect("run_hook");
    let l = learner_of(w, &a).await;
    w.identity.hook_learners.push((id, l));
}

#[then(regex = r#"^the hook resolves to a learner other than "([^"]+)"$"#)]
async fn hook_other_learner(w: &mut SpecWorld, name: String) {
    let (_, l) = w
        .identity
        .hook_learners
        .last()
        .cloned()
        .expect("no hook ran");
    assert_ne!(l, current_learner(w, &name).await);
}

#[then(regex = r#"^identity "([^"]+)" is unverified$"#)]
fn is_unverified(w: &mut SpecWorld, who: String) {
    let id = identity(&who);
    assert_eq!(id.verification(), Verification::Unverified);
    if let Some((used, _)) = w.identity.hook_learners.last() {
        assert_eq!(*used, id, "the identity the hook used");
    }
}

#[when(
    regex = r#"^organisation "([^"]+)" declares that "([^"]+)" and "([^"]+)" are the same learner$"#
)]
async fn declares(w: &mut SpecWorld, org: String, a: String, b: String) {
    let r = w.app.declare_link(&org, &author(&a), &author(&b)).await;
    w.identity.declaration = Some(r);
}

#[then(
    regex = r#"^the declaration is rejected because "([^"]+)" is not administered by "([^"]+)"$"#
)]
fn declaration_rejected(w: &mut SpecWorld, host: String, org: String) {
    match w.identity.declaration.as_ref().expect("nothing declared") {
        Err(Error::Link(LinkRejection::NotAdministered {
            host: h,
            organisation: o,
        })) => {
            assert_eq!((h, o), (&host, &org));
        }
        other => panic!("expected the declaration to be rejected, got {other:?}"),
    }
}

#[given(regex = r#"^identities "([^"]+)" and "([^"]+)" belong to the same learner "([^"]+)"$"#)]
async fn linked_learner(w: &mut SpecWorld, a: String, b: String, name: String) {
    link(w, &a, &b).await;
    name_learner(w, &name, &author(&a)).await;
}

// ---------------------------------------------------------------------------
// D3: linking merges by replaying reviews

fn remember_pair(w: &mut SpecWorld, who: &str) {
    let a = author(who);
    if !w.identity.pair.contains(&a) {
        w.identity.pair.push(a);
    }
}

#[given(
    regex = r#"^learner "([^"]+)" with identity "([^"]+)" reviewed "([^"]+)" as (\w+) on day (\d+)$"#
)]
async fn reviewed_on_day(
    w: &mut SpecWorld,
    name: String,
    who: String,
    concept: String,
    g: String,
    day: i64,
) {
    remember_pair(w, &who);
    on_day(w, day);
    let g = grade(&g);
    if g.is_correct() {
        w.examiner.set_correct_grade(g);
    }
    quiz(w, &who, &[concept], g.is_correct()).await;
    name_learner(w, &name, &author(&who)).await;
}

#[given(
    regex = r#"^learner "([^"]+)" with identity "([^"]+)" had (\d+) correct reviews on days (\d+) to (\d+)$"#
)]
async fn correct_on_days(
    w: &mut SpecWorld,
    name: String,
    who: String,
    n: usize,
    from: i64,
    to: i64,
) {
    assert_eq!(n as i64, to - from + 1, "one review per day");
    remember_pair(w, &who);
    for day in from..=to {
        on_day(w, day);
        let concept = format!("streak.c{}", next(w));
        quiz(w, &who, &[concept], true).await;
    }
    name_learner(w, &name, &author(&who)).await;
}

#[given("the two identities are linked")]
#[when("the two identities are linked")]
async fn two_linked(w: &mut SpecWorld) {
    let [a, b] = <[AuthorId; 2]>::try_from(w.identity.pair.clone())
        .unwrap_or_else(|p| panic!("expected two identities, got {p:?}"));
    link(w, &a.0, &b.0).await;
}

async fn linked_profile(w: &SpecWorld) -> LearnerProfile {
    let a = w.identity.pair.first().expect("no identities to link");
    w.app.profile(a).await.expect("profile")
}

#[then(
    regex = r#"^the review card for "([^"]+)" equals that of one learner profile that recorded both reviews in time order$"#
)]
async fn card_equals_replay(w: &mut SpecWorld, concept: String) {
    let concept = ConceptId(concept);
    let mut reviews = w.identity.reviews.clone();
    reviews.sort_by_key(|r| r.3);
    let mut reference = LearnerProfile::new(LearnerId("reference".into()));
    for (c, f, g, at, via) in &reviews {
        reference.record(c, *f, *g, *at, via);
    }
    let p = linked_profile(w).await;
    assert_eq!(p.card(&concept), reference.card(&concept), "review card");
    assert!(p.card(&concept).is_some(), "no review card for {concept:?}");
}

#[then(regex = r#"^the history contains (\d+) reviews of "([^"]+)"$"#)]
async fn history_contains(w: &mut SpecWorld, n: usize, concept: String) {
    let p = linked_profile(w).await;
    let got = p
        .history()
        .iter()
        .filter(|r| r.concept.0 == concept)
        .count();
    assert_eq!(got, n, "reviews of {concept}");
}

#[then(regex = r"^the linked learner's level is (\w+)$")]
async fn linked_level(w: &mut SpecWorld, lvl: String) {
    assert_eq!(linked_profile(w).await.level(), level(&lvl));
}

#[when(regex = r#"^identity "([^"]+)" is unlinked$"#)]
async fn unlinked(w: &mut SpecWorld, who: String) {
    w.app.unlink(&author(&who)).await.expect("unlink");
}

#[then(regex = r#"^the learner of "([^"]+)" has review cards for exactly (.+)$"#)]
async fn learner_cards_exactly(w: &mut SpecWorld, who: String, list: String) {
    let p = w.app.profile(&author(&who)).await.expect("profile");
    let got: BTreeSet<String> = p.cards().keys().map(|c| c.0.clone()).collect();
    let want: BTreeSet<String> = quoted(&list).into_iter().collect();
    assert_eq!(got, want, "review cards of the learner of {who}");
}

#[given(
    regex = r#"^identity "([^"]+)" answered a question about (?:(Project) concept )?"([^"]+)" wrongly$"#
)]
async fn answered_wrongly(w: &mut SpecWorld, who: String, project: String, concept: String) {
    if !project.is_empty() {
        w.examiner
            .classify(&ConceptId(concept.clone()), ConceptScope::Project);
    }
    quiz(w, &who, &[concept], false).await;
}

#[then(regex = r#"^the first question is about "([^"]+)"$"#)]
fn first_question(w: &mut SpecWorld, concept: String) {
    let s = w.session.as_ref().expect("no session");
    let first = s.questions().first().expect("the session asks nothing");
    assert_eq!(first.concept.0, concept);
}

// ---------------------------------------------------------------------------
// D4: shared learner store

#[given(regex = r#"^the forge "([^"]+)" cannot reach the learner store$"#)]
async fn cannot_reach(w: &mut SpecWorld, host: String) {
    // This engine plays the CI of `host` from now on.
    assert!(
        w.app.config.forges.contains_key(&host),
        "unknown forge {host}"
    );
    let mira = current_learner(w, "Mira").await;
    let profile = w.app.store.load_profile(&mira).await.expect("load_profile");
    let links = w.app.store.links_of(&mira).await.expect("links_of");
    w.identity.snapshot = Some((profile, links));
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_unreachable();
}

#[then(regex = r#"^the check output says the learner profile is local to "([^"]+)"$"#)]
fn output_says_local(w: &mut SpecWorld, host: String) {
    let summary = w
        .scm
        .last_summary_on(w.current_pr())
        .expect("no check output");
    assert!(
        summary.contains(&host) && summary.to_lowercase().contains("local"),
        "check output: {summary:?}"
    );
}

#[then(regex = r#"^learner "([^"]+)" in the learner store is unchanged$"#)]
async fn unchanged(w: &mut SpecWorld, name: String) {
    let l = w.identity.learners[&name].clone();
    let store = w.app.store.inner();
    let profile = store.load_profile(&l).await.expect("load_profile");
    let links = store.links_of(&l).await.expect("links_of");
    let (p0, l0) = w.identity.snapshot.clone().expect("no snapshot");
    assert_eq!(profile, p0, "profile of {name} in the learner store");
    assert_eq!(links, l0, "links of {name} in the learner store");
}

// ---------------------------------------------------------------------------
// D5: only general concepts cross organisations

#[given(regex = r#"^the examiner classifies "([^"]+)" as (General|Project)$"#)]
fn classifies(w: &mut SpecWorld, concept: String, scope: String) {
    let scope = if scope == "General" {
        ConceptScope::General
    } else {
        ConceptScope::Project
    };
    w.examiner.classify(&ConceptId(concept), scope);
}

#[when(
    regex = r##"^identity "([^"]+)" answers questions on pull request "([^"#]+)#(\d+)" about (.+)$"##
)]
async fn answers_on_pr(w: &mut SpecWorld, who: String, repo: String, number: u64, list: String) {
    let pr = PrRef { repo, number };
    let concepts = quoted(&list);
    w.app.config.max_questions = w.app.config.max_questions.max(concepts.len());
    open_pr(w, &who, Some(pr), &concepts).await;
    answer_pr(w, true).await;
}

#[then(regex = r#"^the learner store has a review of "([^"]+)" for learner "([^"]+)"$"#)]
async fn learner_store_has(w: &mut SpecWorld, concept: String, name: String) {
    let l = current_learner(w, &name).await;
    let p = w
        .app
        .store
        .inner()
        .load_profile(&l)
        .await
        .expect("load_profile");
    assert!(
        has_review_of(&p, &ConceptId(concept.clone())),
        "no review of {concept} for {name} in the learner store"
    );
}

#[then(regex = r#"^the learner store has no review of "([^"]+)"$"#)]
async fn learner_store_has_not(w: &mut SpecWorld, concept: String) {
    let c = ConceptId(concept.clone());
    for l in known_learners(w).await {
        let p = w
            .app
            .store
            .inner()
            .load_profile(&l)
            .await
            .expect("load_profile");
        assert!(!has_review_of(&p, &c), "{concept} is in the learner store");
        assert!(
            p.as_ref().is_none_or(|p| p.card(&c).is_none()),
            "a review card for {concept} is in the learner store"
        );
    }
    let content = dump(w, w.app.store.inner()).await;
    assert!(
        !content.contains(&concept),
        "the learner store mentions {concept}"
    );
}

#[then(regex = r#"^the store of organisation "([^"]+)" has a review of "([^"]+)"$"#)]
async fn org_store_has(w: &mut SpecWorld, org: String, concept: String) {
    let c = ConceptId(concept.clone());
    let store = org_store(w, &org);
    let mut candidates = known_learners(w).await;
    for who in w.identity.identity_of.values() {
        if let Some(l) = store
            .learner_of(&identity(&who.0))
            .await
            .expect("learner_of")
        {
            candidates.insert(l);
        }
    }
    let mut found = false;
    for l in candidates {
        let p = store.load_profile(&l).await.expect("load_profile");
        found |= has_review_of(&p, &c);
    }
    assert!(found, "no review of {concept} in the store of {org}");
}

#[then(regex = r#"^the learner store contains none of (.+), the question text or the answers$"#)]
async fn learner_store_contains_none(w: &mut SpecWorld, list: String) {
    let content = dump(w, w.app.store.inner()).await;
    let mut forbidden = quoted(&list);
    forbidden.push(CORRECT_TEXT.to_string());
    forbidden.push(WRONG_TEXT.to_string());
    for q in &w.identity.questions {
        match &q.body {
            soqrates_core::QuestionBody::MultipleChoice {
                prompt,
                options,
                reasons,
                ..
            } => {
                forbidden.push(prompt.clone());
                forbidden.extend(options.iter().cloned());
                forbidden.extend(reasons.iter().cloned());
            }
            soqrates_core::QuestionBody::Open { prompt, rubric, .. } => {
                forbidden.push(prompt.clone());
                forbidden.push(rubric.clone());
            }
        }
        // Code shown with a question is question text too (ADR-0007 D6),
        // line by line, as the dump is JSON.
        if let Some(code) = q.body.code() {
            let lines = code.text.lines().map(str::trim).filter(|l| l.len() >= 10);
            forbidden.extend(lines.map(str::to_string));
        }
    }
    assert!(!w.identity.questions.is_empty(), "no questions were asked");
    for f in forbidden {
        assert!(
            !content.contains(&f),
            "the learner store contains {f:?}: {content}"
        );
    }
}

#[then(regex = r#"^no question is about "([^"]+)"$"#)]
fn no_question_about(w: &mut SpecWorld, concept: String) {
    let s = w.session.as_ref().expect("no session");
    assert!(
        s.questions().iter().all(|q| q.concept.0 != concept),
        "questions: {:?}",
        s.questions()
    );
}

// ---------------------------------------------------------------------------
// D6: the learner is in control

#[when(regex = r#"^identity "([^"]+)" lists its learner's identities$"#)]
async fn lists_identities(w: &mut SpecWorld, who: String) {
    let ids = w.app.identities(&author(&who)).await.expect("identities");
    w.identity.listed = Some(ids);
}

#[then(regex = r"^the list is exactly (.+)$")]
fn list_is_exactly(w: &mut SpecWorld, list: String) {
    let listed = w.identity.listed.clone().expect("nothing listed");
    let want: Vec<Identity> = quoted(&list).iter().map(|s| identity(s)).collect();
    assert_eq!(listed.len(), want.len(), "identities: {listed:?}");
    let got: BTreeSet<_> = listed.into_iter().collect();
    let want: BTreeSet<_> = want.into_iter().collect();
    assert_eq!(got, want, "identities");
}

#[when(regex = r#"^identity "([^"]+)" deletes its learner profile$"#)]
async fn deletes_profile(w: &mut SpecWorld, who: String) {
    w.app
        .delete_learner(&author(&who))
        .await
        .expect("delete_learner");
}

#[then(regex = r#"^no store contains reviews or links for learner "([^"]+)"$"#)]
async fn nothing_left(w: &mut SpecWorld, name: String) {
    let l = w.identity.learners[&name].clone();
    let mut identities: Vec<Identity> = w
        .identity
        .identity_of
        .values()
        .map(|a| identity(&a.0))
        .collect();
    if let Some(listed) = &w.identity.listed {
        identities.extend(listed.iter().cloned());
    }
    let mut stores: Vec<(String, &AnyStore)> = vec![("learner store".into(), w.app.store.inner())];
    stores.extend(
        w.app
            .org_stores
            .iter()
            .map(|(o, s)| (format!("store of {o}"), s.inner())),
    );
    for (what, store) in stores {
        let p = store.load_profile(&l).await.expect("load_profile");
        assert!(
            p.as_ref().is_none_or(|p| p.history().is_empty()),
            "{what} still has reviews of {name}"
        );
        let links = store.links_of(&l).await.expect("links_of");
        assert!(links.is_empty(), "{what} still links {name}: {links:?}");
        for id in &identities {
            let linked = store.learner_of(id).await.expect("learner_of");
            assert_ne!(linked.as_ref(), Some(&l), "{what} still links {id}");
        }
    }
}

#[given(regex = r##"^identity "([^"]+)" has a graded session on pull request "([^"#]+)#(\d+)"$"##)]
async fn graded_session_on_pr(w: &mut SpecWorld, who: String, repo: String, number: u64) {
    let pr = PrRef { repo, number };
    open_pr(w, &who, Some(pr), &["rust.traits".to_string()]).await;
    answer_pr(w, true).await;
}

#[when(regex = r##"^the Soqrates check output for "([^"#]+)#(\d+)" is shown$"##)]
fn output_shown(w: &mut SpecWorld, repo: String, number: u64) {
    let pr = PrRef { repo, number };
    let summary = w.scm.last_summary_on(&pr).expect("no check output");
    w.identity.shown = Some(summary);
}

#[then(regex = r#"^it does not mention "([^"]+)"$"#)]
fn does_not_mention(w: &mut SpecWorld, text: String) {
    let shown = w.identity.shown.as_ref().expect("nothing shown");
    assert!(!shown.contains(&text), "check output: {shown:?}");
}

// ---------------------------------------------------------------------------
// D7: local hooks use a configured identity

#[given(
    regex = r#"^a repository with remote host "([^"]+)", commit e-mail "([^"]+)" and configured identity "([^"]+)"$"#
)]
fn repository(w: &mut SpecWorld, host: String, email: String, configured: String) {
    w.identity.repos.push((host, email, Some(configured)));
}

#[when("the pre-commit hook runs in each repository")]
async fn hook_in_each(w: &mut SpecWorld) {
    for (host, email, configured) in w.identity.repos.clone() {
        let mut map = BTreeMap::new();
        if let Some(c) = configured {
            map.insert(host.clone(), identity(&c));
        }
        let id = local_identity(Some(&host), &email, &map);
        run_local_hook(w, id).await;
    }
}

#[then(regex = r#"^both hooks resolve to learner "([^"]+)"$"#)]
async fn both_hooks_resolve(w: &mut SpecWorld, name: String) {
    let runs = w.identity.hook_learners.clone();
    assert_eq!(runs.len(), 2, "hook runs: {runs:?}");
    let want = current_learner(w, &name).await;
    for (id, l) in runs {
        assert_eq!(l, want, "learner of the hook run as {id}");
    }
}
