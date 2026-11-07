//! The structured outputs the model is asked for, and their validation.
//!
//! Everything here is pure: it takes the model's raw reply and either turns
//! it into domain values or explains what is wrong with it, so that the
//! caller can retry once (see [`crate::examiner`]).

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use soqrates_core::{Concept, ConceptId, ConceptScope, Grade};

use crate::prompts::MAX_CONCEPTS;

/// Reply to [`crate::prompts::EXTRACT_CONCEPTS_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "concepts")]
pub struct ConceptsOutput {
    /// The concepts, the most important first.
    pub concepts: Vec<RawConcept>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RawConcept {
    /// Lowercase dotted identifier, e.g. `rust.async.cancellation`.
    pub id: String,
    pub title: String,
    /// What the concept is and how the change uses it.
    pub summary: String,
    /// `general` or `project`.
    pub scope: String,
}

/// Reply to [`crate::prompts::WRITE_MULTIPLE_CHOICE_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "multiple_choice_question")]
pub struct MultipleChoiceOutput {
    pub prompt: String,
    /// 3 to 5 distinct options, exactly one correct.
    pub options: Vec<String>,
    /// 0-based index of the correct option.
    pub correct: u32,
}

/// Reply to [`crate::prompts::WRITE_OPEN_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "open_question")]
pub struct OpenOutput {
    pub prompt: String,
    /// What a good answer must cover.
    pub rubric: String,
}

/// Reply to [`crate::prompts::GRADE_OPEN_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "grade")]
pub struct GradeOutput {
    /// One of `again`, `hard`, `good`, `easy`.
    pub grade: String,
    /// Asked for because explaining first improves grading; not used.
    #[allow(dead_code, reason = "requested from the model, not read")]
    pub rationale: String,
}

/// Parse a JSON reply, tolerating surrounding prose or a markdown fence.
pub fn parse_json<T: DeserializeOwned>(raw: &str) -> Result<T, String> {
    let text = raw.trim();
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let start = text.find('{');
    let end = text.rfind('}');
    match (start, end) {
        (Some(start), Some(end)) if start < end => serde_json::from_str(&text[start..=end])
            .map_err(|e| format!("the reply is not valid JSON for the schema: {e}")),
        _ => Err("the reply contains no JSON object".to_owned()),
    }
}

/// Whether `id` is a lowercase dotted identifier with at least two segments,
/// i.e. matches `^[a-z0-9_-]+(\.[a-z0-9_-]+)+$`.
pub fn is_valid_concept_id(id: &str) -> bool {
    let mut segments = 0;
    for segment in id.split('.') {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

/// Only an explicit `general` is general; anything else is `Project`
/// (ADR-0003 D5: err towards `Project`).
pub fn parse_scope(scope: &str) -> ConceptScope {
    if scope.trim().eq_ignore_ascii_case("general") {
        ConceptScope::General
    } else {
        ConceptScope::Project
    }
}

/// Turn the model's concepts into domain concepts: normalise ids (trimmed,
/// lowercase), drop invalid entries, dedupe by id keeping the first, and keep
/// at most [`MAX_CONCEPTS`]. A reply with concepts of which none is valid is
/// rejected, so that the caller retries.
pub fn validate_concepts(output: ConceptsOutput) -> Result<Vec<Concept>, String> {
    let offered = output.concepts.len();
    let mut seen = HashSet::new();
    let mut rejected = Vec::new();
    let mut concepts = Vec::new();
    for raw in output.concepts {
        let id = raw.id.trim().to_lowercase();
        let title = raw.title.trim();
        let summary = raw.summary.trim();
        if !is_valid_concept_id(&id) {
            rejected.push(format!("id {:?} is not lowercase dotted", raw.id));
            continue;
        }
        if title.is_empty() || summary.is_empty() {
            rejected.push(format!("concept {id:?} has an empty title or summary"));
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        concepts.push(Concept {
            id: ConceptId(id),
            title: title.to_owned(),
            summary: summary.to_owned(),
            scope: parse_scope(&raw.scope),
        });
    }
    if offered > 0 && concepts.is_empty() {
        return Err(format!("no valid concept: {}", rejected.join("; ")));
    }
    concepts.truncate(MAX_CONCEPTS);
    Ok(concepts)
}

/// A validated multiple-choice question, before shuffling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleChoice {
    pub prompt: String,
    pub options: Vec<String>,
    pub correct: usize,
}

/// Check a multiple-choice reply: a prompt, 3 to 5 non-empty distinct
/// options, and a correct index in range.
pub fn validate_multiple_choice(output: MultipleChoiceOutput) -> Result<MultipleChoice, String> {
    let prompt = output.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("the prompt is empty".to_owned());
    }
    let options: Vec<String> = output.options.iter().map(|o| o.trim().to_owned()).collect();
    if !(3..=5).contains(&options.len()) {
        return Err(format!(
            "there must be 3 to 5 options, not {}",
            options.len()
        ));
    }
    if options.iter().any(String::is_empty) {
        return Err("an option is empty".to_owned());
    }
    let mut seen = HashSet::new();
    if !options.iter().all(|o| seen.insert(o.to_lowercase())) {
        return Err("the options are not distinct".to_owned());
    }
    let correct = output.correct as usize;
    if correct >= options.len() {
        return Err(format!(
            "correct index {correct} is out of range for {} options",
            options.len()
        ));
    }
    Ok(MultipleChoice {
        prompt,
        options,
        correct,
    })
}

/// Shuffle the options deterministically, seeded by `seed` (the concept
/// id), keeping `correct` pointing at the same option text.
pub fn shuffle(question: MultipleChoice, seed: &str) -> MultipleChoice {
    let mut rng = SplitMix64(fnv1a(seed));
    let mut order: Vec<usize> = (0..question.options.len()).collect();
    // Fisher-Yates.
    for i in (1..order.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let correct = order
        .iter()
        .position(|&original| original == question.correct)
        .unwrap_or(0);
    let options = order
        .iter()
        .map(|&original| question.options[original].clone())
        .collect();
    MultipleChoice {
        prompt: question.prompt,
        options,
        correct,
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Check an open-question reply: a non-empty prompt and rubric.
pub fn validate_open(output: OpenOutput) -> Result<(String, String), String> {
    let prompt = output.prompt.trim();
    let rubric = output.rubric.trim();
    if prompt.is_empty() {
        return Err("the prompt is empty".to_owned());
    }
    if rubric.is_empty() {
        return Err("the rubric is empty".to_owned());
    }
    Ok((prompt.to_owned(), rubric.to_owned()))
}

/// Check a grading reply.
pub fn validate_grade(output: GradeOutput) -> Result<Grade, String> {
    match output.grade.trim().to_ascii_lowercase().as_str() {
        "again" => Ok(Grade::Again),
        "hard" => Ok(Grade::Hard),
        "good" => Ok(Grade::Good),
        "easy" => Ok(Grade::Easy),
        other => Err(format!(
            "grade {other:?} is not one of again, hard, good, easy"
        )),
    }
}

/// Interpret a typed multiple-choice answer as a single option letter
/// (`b`, `B`, `b)`, `(b)` or `b.`), giving its 0-based index.
pub fn parse_letter(text: &str) -> Option<usize> {
    let text = text.trim();
    let text = text
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(')'))
        .unwrap_or(text);
    let text = text
        .strip_suffix(')')
        .or_else(|| text.strip_suffix('.'))
        .unwrap_or(text)
        .trim();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase() as u8 - b'a') as usize)
        }
        _ => None,
    }
}
