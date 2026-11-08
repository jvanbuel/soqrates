//! The structured outputs the model is asked for, and their validation.
//!
//! Everything here is pure: it takes the model's raw reply and either turns
//! it into domain values or explains what is wrong with it, so that the
//! caller can retry once (see [`crate::examiner`]).

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use soqrates_core::{Concept, ConceptId, ConceptScope, Grade, Grading, Weight};

use crate::prompts::MAX_CONCEPTS;

/// The longest explanation kept, in characters; longer ones are cut at a
/// sentence or word boundary.
pub const MAX_EXPLANATION_CHARS: usize = 600;

/// The longest reason kept for a wrong option, in characters. It is shown
/// before the explanation, so it is kept shorter.
pub const MAX_REASON_CHARS: usize = 400;

/// The longest grading feedback kept, in characters.
pub const MAX_FEEDBACK_CHARS: usize = 400;

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
    /// `core`, `supporting` or `incidental`.
    #[serde(default)]
    pub weight: String,
}

/// Reply to [`crate::prompts::WRITE_MULTIPLE_CHOICE_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "multiple_choice_question")]
pub struct MultipleChoiceOutput {
    pub prompt: String,
    /// 3 to 5 distinct options, exactly one correct, each with the reason
    /// it is right or wrong.
    pub options: Vec<RawOption>,
    /// 0-based index of the correct option.
    pub correct: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RawOption {
    /// The option as shown to the author.
    pub text: String,
    /// Plain text on why this option is right (the correct one) or wrong,
    /// referring to options by content only, never by letter or position.
    pub why: String,
}

/// Reply to [`crate::prompts::CHECK_OPTIONS_SYSTEM`] (issue #5).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "option_check")]
pub struct OptionCheckOutput {
    /// Every passage of a reason that argues for or against an answer that
    /// is not one of the listed options, quoted. Empty if there is none.
    pub unlisted: Vec<String>,
}

/// Reply to [`crate::prompts::WRITE_OPEN_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "open_question")]
pub struct OpenOutput {
    pub prompt: String,
    /// What a good answer must cover.
    pub rubric: String,
    /// 1 to 3 plain-text sentences on what a good answer covers.
    pub explanation: String,
}

/// Reply to [`crate::prompts::GRADE_OPEN_SYSTEM`].
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(title = "grade")]
pub struct GradeOutput {
    /// One of `again`, `hard`, `good`, `easy`.
    pub grade: String,
    /// One or two plain-text sentences for the learner; returned as the
    /// grading's feedback (ADR-0004 D1).
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

/// `core` and `incidental` as written; anything else, including a missing
/// weight, is `Supporting`: asked, but after the core concepts.
pub fn parse_weight(weight: &str) -> Weight {
    match weight.trim().to_ascii_lowercase().as_str() {
        "core" => Weight::Core,
        "incidental" => Weight::Incidental,
        _ => Weight::Supporting,
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
            weight: parse_weight(&raw.weight),
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
    /// Why each option is right or wrong, in option order.
    pub reasons: Vec<String>,
}

impl MultipleChoice {
    /// The question's explanation: why the correct option is correct.
    pub fn explanation(&self) -> &str {
        &self.reasons[self.correct]
    }
}

/// Check a multiple-choice reply: a prompt, 3 to 5 non-empty distinct
/// options, a correct index in range, and for every option a reason that
/// does not depend on the order of the options (see [`validate_reasons`]).
pub fn validate_multiple_choice(output: MultipleChoiceOutput) -> Result<MultipleChoice, String> {
    let prompt = output.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("the prompt is empty".to_owned());
    }
    let options: Vec<String> = output
        .options
        .iter()
        .map(|o| o.text.trim().to_owned())
        .collect();
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
    let reasons = validate_reasons(&output.options, &options, correct)?;
    Ok(MultipleChoice {
        prompt,
        options,
        correct,
        reasons,
    })
}

/// Check the reason of every option: non-empty, cleaned to plain text (see
/// [`clean_text`]) and independent of the order of the options (see
/// [`normalise_option_refs`]). The correct option's reason is the
/// explanation and may be longer than the others.
fn validate_reasons(
    raw: &[RawOption],
    options: &[String],
    correct: usize,
) -> Result<Vec<String>, String> {
    raw.iter()
        .map(|o| &o.why)
        .enumerate()
        .map(|(i, why)| {
            let max = if i == correct {
                MAX_EXPLANATION_CHARS
            } else {
                MAX_REASON_CHARS
            };
            let why = clean_text(why, max).ok_or_else(|| {
                if i == correct {
                    "the reason for the correct option (the explanation) is empty".to_owned()
                } else {
                    format!("the reason for option {:?} is empty", options[i])
                }
            })?;
            normalise_option_refs(&why, options)
        })
        .collect()
}

/// The most passages quoted back to the model when the option check rejects
/// a question, and the longest each may be, in characters.
const MAX_QUOTED_PASSAGES: usize = 3;
const MAX_QUOTED_CHARS: usize = 200;

/// What is wrong with a multiple-choice question according to the option
/// check (issue #5): `None` if its reasons discuss only the listed options,
/// otherwise the problem to explain when the question is asked for again.
pub fn unlisted_option_problem(output: OptionCheckOutput) -> Option<String> {
    let quoted: Vec<String> = output
        .unlisted
        .iter()
        .filter_map(|p| clean_text(p, MAX_QUOTED_CHARS))
        .take(MAX_QUOTED_PASSAGES)
        .map(|p| format!("{p:?}"))
        .collect();
    if quoted.is_empty() {
        return None;
    }
    Some(format!(
        "a reason discusses an answer that is not one of the options ({}); the author \
         sees only the options listed, so discuss and contrast only those",
        quoted.join("; ")
    ))
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
    let permute = |items: &[String]| -> Vec<String> {
        order
            .iter()
            .map(|&original| items[original].clone())
            .collect()
    };
    MultipleChoice {
        options: permute(&question.options),
        reasons: permute(&question.reasons),
        prompt: question.prompt,
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

/// A validated open question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub prompt: String,
    pub rubric: String,
    pub explanation: String,
}

/// Check an open-question reply: a non-empty prompt, rubric and explanation.
pub fn validate_open(output: OpenOutput) -> Result<Open, String> {
    let prompt = output.prompt.trim();
    let rubric = output.rubric.trim();
    if prompt.is_empty() {
        return Err("the prompt is empty".to_owned());
    }
    if rubric.is_empty() {
        return Err("the rubric is empty".to_owned());
    }
    let explanation = clean_text(&output.explanation, MAX_EXPLANATION_CHARS)
        .ok_or_else(|| "the explanation is empty".to_owned())?;
    Ok(Open {
        prompt: prompt.to_owned(),
        rubric: rubric.to_owned(),
        explanation,
    })
}

/// Check a grading reply: a known grade and a non-empty rationale, which
/// becomes the feedback.
pub fn validate_grade(output: GradeOutput) -> Result<Grading, String> {
    let grade = match output.grade.trim().to_ascii_lowercase().as_str() {
        "again" => Grade::Again,
        "hard" => Grade::Hard,
        "good" => Grade::Good,
        "easy" => Grade::Easy,
        other => {
            return Err(format!(
                "grade {other:?} is not one of again, hard, good, easy"
            ));
        }
    };
    let feedback = clean_text(&output.rationale, MAX_FEEDBACK_CHARS)
        .ok_or_else(|| "the rationale is empty".to_owned())?;
    Ok(Grading { grade, feedback })
}

/// Turn model prose into one short plain-text paragraph: drop markdown
/// heading markers, join lines, collapse whitespace, and cut it to at most
/// `max` characters (see [`truncate_text`]). `None` if nothing is left.
pub fn clean_text(text: &str, max: usize) -> Option<String> {
    let mut words = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let line = match line.trim_start_matches('#') {
            rest if rest.len() < line.len() && (rest.is_empty() || rest.starts_with(' ')) => rest,
            _ => line,
        };
        words.extend(line.split_whitespace());
    }
    let joined = words.join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(truncate_text(&joined, max))
    }
}

/// Cut `text` to at most `max` characters: at the last sentence end that
/// keeps at least a third of the allowance, otherwise at the last space
/// (with an ellipsis), otherwise at a character boundary.
pub fn truncate_text(text: &str, max: usize) -> String {
    let Some((end, _)) = text.char_indices().nth(max) else {
        return text.to_owned();
    };
    let mut chars = text[..end].char_indices().peekable();
    let mut sentence_end = None;
    while let Some((at, c)) = chars.next() {
        let next = match chars.peek() {
            Some(&(_, next)) => Some(next),
            None => text[end..].chars().next(),
        };
        if matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace) {
            sentence_end = Some(at + c.len_utf8());
        }
    }
    let min = text[..end].chars().count() / 3;
    if let Some(cut) = sentence_end.filter(|&cut| text[..cut].chars().count() >= min) {
        return text[..cut].to_owned();
    }
    let head = match text[..end].rfind(' ') {
        Some(space) if space > 0 => &text[..space],
        _ => &text[..end],
    };
    format!("{}…", head.trim_end())
}

/// Words that introduce a reference to one option.
const OPTION_WORDS: [&str; 3] = ["option", "choice", "answer"];

/// Ordinals that, before an option word, refer to a position.
const ORDINALS: [&str; 7] = [
    "first", "second", "third", "fourth", "fifth", "last", "final",
];

/// Punctuation that may follow a word.
const TRAILING: &[char] = &[',', '.', ';', ':', '!', '?'];

/// Make a multiple-choice reason independent of the option order,
/// which is shuffled after validation.
///
/// - `option C`, `choice (b)` or `answer 2` (an option word followed by an
///   upper-case letter A-E, a parenthesised letter a-e or a digit 1-5) is
///   rewritten to name the option by its text: `option "…"`.
/// - Any other positional reference (a bare `(b)`, `options A and C`, `the
///   first option`, or a letter or digit past the last option) is rejected,
///   so that the caller asks again.
pub fn normalise_option_refs(text: &str, options: &[String]) -> Result<String, String> {
    let rejected = |what: String| {
        Err(format!(
            "a reason refers to an option by letter, number or position ({what:?}); \
             options are shuffled, so refer to them by their content only"
        ))
    };
    let words = word_spans(text);
    let is_option_word = |word: &str| OPTION_WORDS.contains(&word);
    let is_option_words = |word: &str| word.strip_suffix('s').is_some_and(is_option_word);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (i, &(_, word)) in words.iter().enumerate() {
        let trimmed = word.trim_end_matches(TRAILING);
        let lower = trimmed.to_ascii_lowercase();
        let previous = i
            .checked_sub(1)
            .map(|p| words[p].1.to_ascii_lowercase())
            .unwrap_or_default();
        let next = words.get(i + 1).copied();

        if paren_letter(trimmed).is_some() && !is_option_word(&previous) {
            return rejected(word.to_owned());
        }
        if is_option_words(&lower)
            && let Some((_, next)) = next.filter(|&(_, n)| option_ref(n).is_some())
        {
            return rejected(format!("{word} {next}"));
        }
        if ORDINALS.contains(&lower.as_str())
            && let Some((_, next)) = next
        {
            let next_lower = next.trim_end_matches(TRAILING).to_ascii_lowercase();
            if is_option_word(&next_lower) || is_option_words(&next_lower) {
                return rejected(format!("{word} {next}"));
            }
        }
        // Rewrite `option C` (no punctuation between the two words).
        if !is_option_word(&lower) || trimmed.len() != word.len() {
            continue;
        }
        let Some((next_at, next)) = next else {
            continue;
        };
        let Some((index, len)) = option_ref(next) else {
            continue;
        };
        let Some(option) = options.get(index) else {
            return rejected(format!("{word} {next}"));
        };
        out.push_str(&text[copied..next_at]);
        out.push('"');
        out.push_str(option);
        out.push('"');
        copied = next_at + len;
    }
    out.push_str(&text[copied..]);
    Ok(out)
}

/// The words of `text` with their byte offsets.
fn word_spans(text: &str) -> Vec<(usize, &str)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                spans.push((s, &text[s..at]));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = start {
        spans.push((s, &text[s..]));
    }
    spans
}

/// `(b)` or `(B)`, for letters a-e, as a 0-based index.
fn paren_letter(word: &str) -> Option<usize> {
    let inner = word.strip_prefix('(')?.strip_suffix(')')?;
    match inner.as_bytes() {
        [c @ b'a'..=b'e'] | [c @ b'A'..=b'E'] => Some((c.to_ascii_lowercase() - b'a') as usize),
        _ => None,
    }
}

/// The option that `word`, following an option word, names, and the byte
/// length of the part of `word` that names it (trailing punctuation is not
/// part of it): an upper-case letter A-E, a parenthesised letter a-e or a
/// digit 1-5.
fn option_ref(word: &str) -> Option<(usize, usize)> {
    let core = word.trim_end_matches(TRAILING);
    if let Some(index) = paren_letter(core) {
        return Some((index, core.len()));
    }
    match core.as_bytes() {
        [c @ b'A'..=b'E'] => Some(((c - b'A') as usize, 1)),
        [c @ b'1'..=b'5'] => Some(((c - b'1') as usize, 1)),
        _ => None,
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
