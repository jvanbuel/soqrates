//! Steps for `question_kinds.feature` and `code_snippets.feature`
//! (ADR-0007): kinds of questions, and the code they show.

// @adr-0007-d1 @adr-0007-d2 @adr-0007-d3 @adr-0007-d4 @adr-0007-d6

use cucumber::gherkin::Step;
use cucumber::{given, then};
use soqrates_core::ports::StateStore;
use soqrates_core::{
    Diff, PrRef, PullRequest, Question, QuestionFormat, QuestionKind, SnippetOrigin,
};

use super::author;
use super::learner_identity::{dump, org_store};
use crate::world::SpecWorld;

// ---------------------------------------------------------------------------
// helpers

/// The questions `which` refers to: the session started in this scenario
/// (`question`), or the latest posted on the pull request (`posted
/// question`).
fn questions(w: &SpecWorld, which: &str) -> Vec<Question> {
    match which {
        "question" => w.session.as_ref().expect("no session").questions().to_vec(),
        "posted question" => w
            .scm
            .posted_on(w.current_pr())
            .pop()
            .expect("no questions were posted on the pull request")
            .questions()
            .to_vec(),
        other => panic!("unknown questions {other:?}"),
    }
}

fn question(w: &SpecWorld, which: &str, concept: &str) -> Question {
    let qs = questions(w, which);
    qs.iter()
        .find(|q| q.concept.0 == concept)
        .unwrap_or_else(|| panic!("no {which} about {concept}: {qs:?}"))
        .clone()
}

fn kind(name: &str) -> QuestionKind {
    QuestionKind::from_name(name).unwrap_or_else(|| panic!("unknown kind {name:?}"))
}

/// The lines a change adds to `file`, without their `+`.
fn added_lines(diff: &Diff, file: &str) -> Vec<String> {
    let mut current: Option<&str> = None;
    let mut out = Vec::new();
    for line in diff.0.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            current = header.rsplit_once(" b/").map(|(_, b)| b);
        } else if current == Some(file)
            && !line.starts_with("+++")
            && let Some(code) = line.strip_prefix('+')
        {
            out.push(code.to_string());
        }
    }
    out
}

/// A diff adding `lines` to the new file `file`.
fn diff_adding(file: &str, lines: &[String]) -> Diff {
    let mut text = format!(
        "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for l in lines {
        text.push('+');
        text.push_str(l);
        text.push('\n');
    }
    Diff(text)
}

fn open_pr(w: &mut SpecWorld, repo: String, number: u64, who: &str, head: String, diff: Diff) {
    let pr = PrRef { repo, number };
    w.scm.open(PullRequest {
        pr: pr.clone(),
        author: author(who),
        head_sha: head,
        diff,
    });
    w.pr = Some(pr);
}

/// Every question posted on the current pull request, and remember their
/// sessions for the store dumps (ADR-0003 D5).
fn posted_questions(w: &mut SpecWorld) -> Vec<Question> {
    let mut all = Vec::new();
    for session in w.scm.posted_on(w.current_pr()) {
        if !w.identity.session_keys.contains(session.key()) {
            w.identity.session_keys.push(session.key().clone());
        }
        all.extend(session.questions().iter().cloned());
    }
    all
}

/// `text` as it appears inside a JSON string.
fn as_json(text: &str) -> String {
    let quoted = serde_json::to_string(text).expect("serialize");
    quoted[1..quoted.len() - 1].to_string()
}

// ---------------------------------------------------------------------------
// pull requests with code

#[given(
    regex = r##"^a pull request "([^"#]+)#(\d+)" by "([^"]+)" at head "([^"]+)" whose change to "([^"]+)" is:$"##
)]
fn pull_request_with_code(
    w: &mut SpecWorld,
    repo: String,
    number: u64,
    who: String,
    head: String,
    file: String,
    step: &Step,
) {
    let code = step
        .docstring
        .as_ref()
        .expect("this step needs a docstring");
    let lines: Vec<String> = code
        .trim_matches('\n')
        .lines()
        .map(str::to_string)
        .collect();
    open_pr(w, repo, number, &who, head, diff_adding(&file, &lines));
}

#[given(
    regex = r#"^a pull request by "([^"]+)" whose change to "([^"]+)" adds (\d+) lines about "([^"]+)"$"#
)]
fn pull_request_with_long_code(
    w: &mut SpecWorld,
    who: String,
    file: String,
    n: usize,
    concept: String,
) {
    let mut lines = vec![format!("// concept: {concept}")];
    lines.extend((1..n).map(|i| format!("let step_{i} = fold(step_{});", i - 1)));
    let diff = diff_adding(&file, &lines);
    open_pr(
        w,
        "jvanbuel/soqrates".into(),
        17,
        &who,
        "abc123".into(),
        diff,
    );
}

// ---------------------------------------------------------------------------
// kinds (ADR-0007 D1, D2)

#[then(
    regex = r#"^the (question|posted question) about "([^"]+)" is (a multiple-choice|an open) question of kind (\w+)$"#
)]
fn is_of_kind(w: &mut SpecWorld, which: String, concept: String, fmt: String, name: String) {
    let q = question(w, &which, &concept);
    let want = match fmt.as_str() {
        "a multiple-choice" => QuestionFormat::MultipleChoice,
        _ => QuestionFormat::Open,
    };
    assert_eq!(q.format(), want, "{q:?}");
    assert_eq!(q.kind, kind(&name), "{q:?}");
}

#[then(regex = r"^the kinds of the questions are (.+), in that order$")]
fn kinds_in_order(w: &mut SpecWorld, list: String) {
    let want: Vec<QuestionKind> = list.split(',').map(|k| kind(k.trim())).collect();
    let got: Vec<QuestionKind> = questions(w, "question").iter().map(|q| q.kind).collect();
    assert_eq!(got, want);
}

#[then(regex = r#"^the posted question about "([^"]+)" is asked as it was for head "([^"]+)"$"#)]
fn asked_as_before(w: &mut SpecWorld, concept: String, head: String) {
    let now = question(w, "posted question", &concept);
    let earlier = w
        .scm
        .posted_on(w.current_pr())
        .into_iter()
        .find(|s| s.key().0.ends_with(&head))
        .unwrap_or_else(|| panic!("no questions were posted for head {head}"));
    let then = earlier
        .questions()
        .iter()
        .find(|q| q.concept.0 == concept)
        .expect("not asked for that head");
    assert_eq!(&now, then);
}

// ---------------------------------------------------------------------------
// code (ADR-0007 D3, D4)

#[then(
    regex = r#"^the (question|posted question) about "([^"]+)" shows a snippet the examiner wrote, as "([^"]+)" code$"#
)]
fn shows_general_snippet(w: &mut SpecWorld, which: String, concept: String, language: String) {
    let q = question(w, &which, &concept);
    let code = q.body.code().expect("no code");
    assert_eq!(code.origin, SnippetOrigin::General);
    assert_eq!(code.language, language);
    assert!(code.fits(), "{code:?}");
}

#[then(regex = r#"^the (question|posted question) about "([^"]+)" shows the author's own code$"#)]
fn shows_authors_code(w: &mut SpecWorld, which: String, concept: String) {
    let q = question(w, &which, &concept);
    let code = q.body.code().expect("no code");
    assert_eq!(code.origin, SnippetOrigin::Change);
    // The fake diff names each concept on the line it adds.
    assert!(
        code.text.contains(&format!("// concept: {concept}")),
        "{code:?}"
    );
}

#[then(
    regex = r#"^the (question|posted question) about "([^"]+)" shows the lines the change adds to "([^"]+)", as "([^"]+)" code$"#
)]
fn shows_added_lines(
    w: &mut SpecWorld,
    which: String,
    concept: String,
    file: String,
    language: String,
) {
    let q = question(w, &which, &concept);
    let code = q.body.code().expect("no code");
    assert_eq!(code.origin, SnippetOrigin::Change, "the author's own code");
    assert_eq!(code.language, language);
    let added = added_lines(&w.scm.pull_request_now(w.current_pr()).diff, &file).join("\n");
    // Exactly as written: a run of the added lines, from the concept's own.
    assert!(
        code.text.starts_with(&format!("// concept: {concept}")),
        "{code:?}"
    );
    assert!(
        added.contains(&code.text),
        "{:?} is not in {added:?}",
        code.text
    );
}

#[then(regex = r#"^the (question|posted question) about "([^"]+)" shows (\d+) lines of code$"#)]
fn shows_n_lines(w: &mut SpecWorld, which: String, concept: String, n: usize) {
    let q = question(w, &which, &concept);
    assert_eq!(q.body.code().expect("no code").lines(), n);
}

#[then(
    regex = r#"^the prompt of the (question|posted question) about "([^"]+)" contains none of its code$"#
)]
fn prompt_has_no_code(w: &mut SpecWorld, which: String, concept: String) {
    let q = question(w, &which, &concept);
    let code = q.body.code().expect("no code");
    for line in code.text.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            !q.body.prompt().contains(line.trim()),
            "{line:?} in the prompt"
        );
    }
}

#[then(
    regex = r#"^the options of the (question|posted question) about "([^"]+)" are "([^"]+)" code$"#
)]
fn options_are_code(w: &mut SpecWorld, which: String, concept: String, language: String) {
    let q = question(w, &which, &concept);
    assert_eq!(q.body.options_language(), Some(language.as_str()), "{q:?}");
}

// ---------------------------------------------------------------------------
// privacy (ADR-0007 D6, ADR-0003 D5)

#[then("no posted question may leave the repository")]
fn none_may_leave(w: &mut SpecWorld) {
    let qs = posted_questions(w);
    assert!(!qs.is_empty(), "nothing was posted");
    for q in qs {
        assert!(!q.may_leave_repository(), "{q:?}");
    }
}

#[then(regex = r#"^the pending session in the store of organisation "([^"]+)" shows the code$"#)]
async fn pending_session_shows_code(w: &mut SpecWorld, org: String) {
    let posted = w
        .scm
        .posted_on(w.current_pr())
        .pop()
        .expect("nothing was posted");
    let stored = org_store(w, &org)
        .load_session(posted.key())
        .await
        .expect("load_session")
        .expect("the pending session is stored with the repository's sessions");
    assert!(stored.questions().iter().any(|q| q.body.code().is_some()));
    assert_eq!(stored.questions(), posted.questions());
}

#[then("the learner store contains none of the code shown with the questions")]
async fn learner_store_has_no_code(w: &mut SpecWorld) {
    let qs = posted_questions(w);
    let content = dump(w, w.app.store.inner()).await;
    let mut checked = 0;
    for code in qs.iter().filter_map(|q| q.body.code()) {
        assert!(!content.contains(&as_json(&code.text)), "{content}");
        for line in code.text.lines().map(str::trim).filter(|l| l.len() >= 10) {
            assert!(
                !content.contains(&as_json(line)),
                "the learner store contains {line:?}: {content}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no code was shown");
}
