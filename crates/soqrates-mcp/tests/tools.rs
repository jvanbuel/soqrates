//! The MCP tools, called in-process through `SoqratesMcp::call_tool`.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use soqrates_core::ports::{Clock, Codebase, Examiner};
use soqrates_core::{
    Answer, AnswerBody, AuthorId, CodeSnippet, Concept, ConceptId, ConceptScope, Config, Diff,
    Error, Grade, Grading, Question, QuestionBody, QuestionBrief, QuestionFormat, QuestionId,
    QuestionKind, Result, SnippetOrigin, Soqrates, Weight,
};
use soqrates_mcp::SoqratesMcp;
use soqrates_store::InMemoryStore;

const RUBRIC: &str = "SECRET-RUBRIC: must mention the borrow checker";
const EXPLANATION: &str = "SECRET-EXPLANATION: option c is right because";
const RIGHT: usize = 2;

/// One concept per `+concept:` line; multiple choice is right at index
/// `RIGHT`; open answers are right when they start with "right".
struct FakeExaminer;

impl Examiner for FakeExaminer {
    async fn extract_concepts(&self, diff: &Diff, code: &impl Codebase) -> Result<Vec<Concept>> {
        // The MCP server has no codebase (ADR-0008 D4).
        assert!(!code.is_available());
        Ok(diff
            .0
            .lines()
            .filter_map(|l| l.strip_prefix("+concept:"))
            .map(|id| Concept {
                id: ConceptId(id.into()),
                title: id.into(),
                summary: String::new(),
                scope: ConceptScope::General,
                weight: Weight::Core,
                locations: Vec::new(),
            })
            .collect())
    }

    async fn write_question(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        codebase: &impl Codebase,
    ) -> Result<Question> {
        assert!(!codebase.is_available());
        let format = brief.format;
        // The brief's first kind, with a general snippet when it shows code;
        // a scenario instead of the kinds that need more (ADR-0007 D2).
        let kind = match brief.kinds[0] {
            QuestionKind::ChooseTheFix | QuestionKind::ExplainYourChange => QuestionKind::Scenario,
            kind => kind,
        };
        let code = kind.shows_code().then(|| CodeSnippet {
            language: "rust".into(),
            text: format!("let {0} = {0}();", concept.title),
            origin: SnippetOrigin::General,
        });
        let body = match format {
            QuestionFormat::MultipleChoice => QuestionBody::MultipleChoice {
                prompt: format!("Which is true of {}?", concept.title),
                code,
                options: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                options_language: None,
                correct: RIGHT,
                reasons: Vec::new(),
            },
            QuestionFormat::Open => QuestionBody::Open {
                prompt: format!("Explain {}.", concept.title),
                code,
                rubric: RUBRIC.into(),
            },
        };
        Ok(Question {
            id: QuestionId(format!("{}#{format:?}", concept.id.0)),
            concept: concept.id.clone(),
            scope: concept.scope,
            kind,
            body,
            explanation: EXPLANATION.into(),
            checked: false,
            banked: None,
        })
    }

    async fn grade(
        &self,
        question: &Question,
        answer: &Answer,
        codebase: &impl Codebase,
    ) -> Result<Grading> {
        assert!(!codebase.is_available());
        let right = match (&question.body, &answer.body) {
            (QuestionBody::MultipleChoice { correct, .. }, AnswerBody::Choice(c)) => c == correct,
            (QuestionBody::Open { .. }, AnswerBody::Text(t)) => t.starts_with("right"),
            _ => false,
        };
        Ok(Grading {
            grade: if right { Grade::Good } else { Grade::Again },
            feedback: String::new(),
        })
    }
}

#[derive(Clone)]
struct FakeClock(Arc<Mutex<DateTime<Utc>>>);

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

type App = Soqrates<InMemoryStore, FakeExaminer, FakeClock>;

fn setup(
    max_questions: usize,
    open_questions: bool,
) -> (
    Arc<App>,
    SoqratesMcp<InMemoryStore, FakeExaminer, FakeClock>,
) {
    let clock = FakeClock(Arc::new(Mutex::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap(),
    )));
    let config = Config {
        max_questions,
        open_questions,
        ..Config::default()
    };
    let app = Arc::new(Soqrates::new(
        InMemoryStore::new(),
        FakeExaminer,
        clock,
        config,
    ));
    (app.clone(), SoqratesMcp::new(app))
}

fn diff(concepts: &[&str]) -> String {
    concepts.iter().map(|c| format!("+concept:{c}\n")).collect()
}

/// Every object key anywhere in `value`.
fn keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.push(k.clone());
                keys(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
        _ => {}
    }
}

#[tokio::test]
async fn start_session_never_reveals_the_answer_key() {
    // With open questions on, experts get open questions (ADR-0002 D5): the
    // author answers correctly until promoted to Expert (ADR-0002 D4), and
    // every session on the way, in either format, is checked.
    let (_, mcp) = setup(4, true);
    let author = "github.com:1001";
    let mut formats = std::collections::BTreeSet::new();
    // New concepts each round: a concept answered correctly isn't asked
    // again until it is due (ADR-0002 D10).
    for round in 0..6 {
        let concepts: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|c| format!("{c}{round}"))
            .collect();
        let concepts: Vec<&str> = concepts.iter().map(String::as_str).collect();
        let out = mcp
            .call_tool(
                "start_session",
                json!({"author": author, "diff": diff(&concepts),
                       "session_key": format!("k{round}")}),
            )
            .await
            .expect("start_session");
        assert_no_answer_key(&out);
        let questions = out["questions"].as_array().unwrap();
        let session: std::collections::BTreeSet<&str> = questions
            .iter()
            .map(|q| q["format"].as_str().unwrap())
            .collect();
        assert_eq!(session.len(), 1, "a session never mixes formats: {out}");
        formats.extend(session.iter().map(|f| f.to_string()));
        let answers: Vec<Value> = questions
            .iter()
            .map(|q| match q["format"].as_str().unwrap() {
                "MultipleChoice" => json!({"question": q["id"], "choice": RIGHT}),
                _ => json!({"question": q["id"], "text": "right"}),
            })
            .collect();
        mcp.call_tool(
            "submit_answers",
            json!({"session_key": format!("k{round}"), "answers": answers}),
        )
        .await
        .expect("submit_answers");
    }
    assert!(formats.contains("MultipleChoice"), "{formats:?}");
    assert!(formats.contains("Open"), "{formats:?}");
}

/// `out`, a `start_session` result, reveals no answer key: no correct
/// option, rubric or explanation.
fn assert_no_answer_key(out: &Value) {
    let questions = out["questions"].as_array().unwrap();
    for q in questions {
        assert!(q["prompt"].is_string(), "{q}");
        if q["format"] == "MultipleChoice" {
            assert_eq!(q["options"].as_array().unwrap().len(), 4);
        }
        // The kind, and code in its own field (ADR-0007 D5).
        let kind = q["kind"].as_str().expect("kind");
        assert_eq!(q["code"]["text"].is_string(), kind != "scenario", "{q}");
        assert_eq!(q["code"]["language"].is_string(), kind != "scenario", "{q}");
    }
    assert!(questions.iter().any(|q| q["code"].is_object()), "{out}");

    let mut all = Vec::new();
    keys(out, &mut all);
    for leak in ["correct", "correct_option", "rubric", "answer", "body"] {
        assert!(!all.iter().any(|k| k == leak), "{leak:?} leaked in {out}");
    }
    assert!(
        !out.to_string().contains("SECRET-RUBRIC"),
        "rubric leaked in {out}"
    );
    assert!(
        !out.to_string().contains("SECRET-EXPLANATION"),
        "start_session must not reveal the explanation before grading (ADR-0004 D1)"
    );
}

#[tokio::test]
async fn a_full_session_through_the_tools_matches_the_engine() {
    let (app, mcp) = setup(3, false);
    let author = "email:jan@example.com";
    let started = mcp
        .call_tool(
            "start_session",
            json!({"author": author, "diff": diff(&["rust.ownership", "rust.traits"])}),
        )
        .await
        .unwrap();
    let key = started["session_key"].as_str().unwrap().to_string();
    assert!(key.starts_with("mcp:email:jan@example.com:"), "{key}");
    let questions = started["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 2);
    // A novice gets multiple choice: answer the first right, the second wrong.
    let answers = json!([
        {"question": questions[0]["id"], "choice": RIGHT},
        {"question": questions[1]["id"], "choice": 0},
    ]);
    let graded = mcp
        .call_tool(
            "submit_answers",
            json!({"session_key": key, "answers": answers}),
        )
        .await
        .unwrap();
    assert_eq!(graded["level_after"], "Novice");
    assert_eq!(graded["grades"][0]["grade"], "Good");
    assert_eq!(graded["grades"][1]["grade"], "Again");
    assert_eq!(graded["feedback"][0]["correct_option"], RIGHT);
    assert_eq!(graded["feedback"][1]["question"], questions[1]["id"]);

    let profile = mcp
        .call_tool("get_learner_profile", json!({"author": author}))
        .await
        .unwrap();
    let engine = app.profile(&AuthorId(author.into())).await.unwrap();
    assert_eq!(profile["reviews"], engine.history().len());
    assert_eq!(
        profile["cards"].as_array().unwrap().len(),
        engine.cards().len()
    );
    assert_eq!(profile["cards"][0]["concept"], "rust.ownership");
    assert_eq!(profile["cards"][0]["reps"], 1);
    let mut all = Vec::new();
    keys(&profile, &mut all);
    assert!(
        !all.iter().any(|k| k == "learner" || k == "via"),
        "{profile}"
    );

    let due = mcp
        .call_tool("due_reviews", json!({"author": author}))
        .await
        .unwrap();
    let engine_due: Vec<String> = app
        .due_reviews(&AuthorId(author.into()))
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(due, json!({"concepts": engine_due}));
}

#[tokio::test]
async fn unknown_tools_and_bad_arguments_are_invalid() {
    let (_, mcp) = setup(3, false);
    let cases = [
        ("delete_everything", json!({})),
        ("get_learner_profile", json!({})),
        ("get_learner_profile", json!({"author": "not an identity"})),
        (
            "get_learner_profile",
            json!({"author": "github.com:1", "x": 1}),
        ),
        ("due_reviews", json!({"author": 7})),
        (
            "submit_answers",
            json!({"session_key": "k", "answers": [{"question": "q"}]}),
        ),
        (
            "submit_answers",
            json!({"session_key": "k", "answers": [{"question": "q", "choice": 1, "text": "t"}]}),
        ),
    ];
    for (tool, args) in cases {
        match mcp.call_tool(tool, args.clone()).await {
            Err(Error::Invalid(msg)) => assert!(!msg.is_empty()),
            other => panic!("{tool} {args}: expected Invalid, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn engine_errors_pass_through() {
    let (_, mcp) = setup(3, false);
    let r = mcp
        .call_tool(
            "submit_answers",
            json!({"session_key": "nope", "answers": []}),
        )
        .await;
    assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
}
