//! Question kinds and the code shown with questions (ADR-0007).

use serde::{Deserialize, Serialize};

use super::{Diff, Question, QuestionBody, QuestionFormat};
use crate::assessment::ConceptScope;

/// `raw` as a path in a codebase (ADR-0008 D1): relative to the repository
/// root and `/`-separated, without `.` segments or trailing slashes. `""`
/// is the root. `None` for anything that could leave the repository or
/// isn't a plain path: absolute paths, `..`, backslashes, control
/// characters, and `:` (a drive, or a revision prefix such as `HEAD:`).
pub fn code_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with('/') || raw.contains('\\') || raw.chars().any(char::is_control) {
        return None;
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// The most lines a code snippet, or a code option, may have (ADR-0007 D3).
pub const MAX_SNIPPET_LINES: usize = 15;

/// The most hunks a writer is given for one concept (ADR-0007 D4).
pub const MAX_HUNKS: usize = 3;

/// What a question asks of the author, separate from its format
/// (ADR-0007 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum QuestionKind {
    /// A realistic situation in prose. No code. What every question was
    /// before kinds existed.
    #[default]
    Scenario,
    /// A snippet: what happens when it runs, which line fails, and why.
    PredictOutcome,
    /// A snippet with one subtle bug: pick or describe it.
    SpotTheBug,
    /// A broken snippet; the options are code alternatives. Multiple choice
    /// only.
    ChooseTheFix,
    /// A hunk of the author's own change: why is it needed, what breaks
    /// without it.
    ExplainYourChange,
}

impl QuestionKind {
    pub const ALL: [QuestionKind; 5] = [
        QuestionKind::Scenario,
        QuestionKind::PredictOutcome,
        QuestionKind::SpotTheBug,
        QuestionKind::ChooseTheFix,
        QuestionKind::ExplainYourChange,
    ];

    /// The kind's name in prompts, scenarios and tools, e.g.
    /// `predict_outcome`.
    pub fn name(self) -> &'static str {
        match self {
            QuestionKind::Scenario => "scenario",
            QuestionKind::PredictOutcome => "predict_outcome",
            QuestionKind::SpotTheBug => "spot_the_bug",
            QuestionKind::ChooseTheFix => "choose_the_fix",
            QuestionKind::ExplainYourChange => "explain_your_change",
        }
    }

    /// The kind called `name` (see [`QuestionKind::name`]); also accepts
    /// the variant name, e.g. `PredictOutcome`.
    pub fn from_name(name: &str) -> Option<QuestionKind> {
        let wanted: String = name
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        QuestionKind::ALL
            .into_iter()
            .find(|k| k.name().replace('_', "") == wanted)
    }

    /// Whether a question of this kind can have `format`: `ChooseTheFix`
    /// is multiple choice only.
    pub fn fits(self, format: QuestionFormat) -> bool {
        !(self == QuestionKind::ChooseTheFix && format == QuestionFormat::Open)
    }

    /// Whether a question of this kind shows code. Only `Scenario` doesn't.
    pub fn shows_code(self) -> bool {
        self != QuestionKind::Scenario
    }

    /// Whether the code must name a flawed line the explanation is about
    /// (ADR-0007 D7).
    pub fn has_flawed_line(self) -> bool {
        matches!(self, QuestionKind::SpotTheBug | QuestionKind::ChooseTheFix)
    }
}

/// Where a snippet came from, which decides where it may be stored
/// (ADR-0007 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SnippetOrigin {
    /// Copied from the author's change. Never leaves the repository. The
    /// safe default.
    #[default]
    Change,
    /// Written by the examiner to mirror the change.
    General,
}

/// Code shown with a question, kept separate from its prompt (ADR-0007 D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSnippet {
    /// A language tag such as `rust`, or `text`.
    pub language: String,
    /// The code as shown, at most [`MAX_SNIPPET_LINES`] lines.
    pub text: String,
    #[serde(default)]
    pub origin: SnippetOrigin,
}

impl CodeSnippet {
    /// The number of lines of `text`.
    pub fn lines(&self) -> usize {
        line_count(&self.text)
    }

    /// Whether the snippet has some code and at most [`MAX_SNIPPET_LINES`]
    /// lines.
    pub fn fits(&self) -> bool {
        !self.text.trim().is_empty() && self.lines() <= MAX_SNIPPET_LINES
    }
}

/// The number of lines of `text`, ignoring one trailing newline.
pub fn line_count(text: &str) -> usize {
    text.lines().count()
}

/// Where in a change a concept was found: a file and a range of lines,
/// 1-based and inclusive, on the new side of the diff (ADR-0007 D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLocation {
    pub file: String,
    pub start: u32,
    pub end: u32,
}

/// A contiguous piece of a change in one file, as it is after the change:
/// context and added lines, without removed lines or diff markers. At most
/// [`MAX_SNIPPET_LINES`] lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub file: String,
    /// The new-side line number of the first line of `text`.
    pub start: u32,
    /// The language of `file`, from its extension.
    pub language: String,
    pub text: String,
}

impl Hunk {
    /// The last line number of `text`.
    pub fn end(&self) -> u32 {
        self.start + (line_count(&self.text) as u32).saturating_sub(1)
    }

    /// This hunk as a snippet: the author's own code (ADR-0007 D6).
    pub fn snippet(&self) -> CodeSnippet {
        CodeSnippet {
            language: self.language.clone(),
            text: self.text.clone(),
            origin: SnippetOrigin::Change,
        }
    }
}

/// What the examiner is asked to write for one concept (ADR-0007 D2, D4):
/// a question in `format`, of the first of `kinds` that suits the concept,
/// with `hunks` of the change to show or mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionBrief {
    pub format: QuestionFormat,
    /// The kinds allowed, most preferred first. Never empty: `Scenario`
    /// is always allowed.
    pub kinds: Vec<QuestionKind>,
    pub hunks: Vec<Hunk>,
}

impl QuestionBrief {
    /// A brief for `format`, keeping the `kinds` that fit it, in order.
    /// `ExplainYourChange` is dropped when there are no `hunks` to explain.
    /// `Scenario` is added last if missing, so a question can always be
    /// written.
    pub fn new(format: QuestionFormat, kinds: &[QuestionKind], hunks: Vec<Hunk>) -> Self {
        let mut allowed: Vec<QuestionKind> = Vec::new();
        for &k in kinds {
            let possible = k.fits(format)
                && (k != QuestionKind::ExplainYourChange || !hunks.is_empty())
                && !allowed.contains(&k);
            if possible {
                allowed.push(k);
            }
        }
        if !allowed.contains(&QuestionKind::Scenario) {
            allowed.push(QuestionKind::Scenario);
        }
        QuestionBrief {
            format,
            kinds: allowed,
            hunks,
        }
    }

    /// Whether `question` is what this brief asked for (ADR-0007 D2, D3,
    /// D4): the format, an allowed kind, code exactly when the kind shows
    /// code, within the length limit, and for `ExplainYourChange` a snippet
    /// copied from one of the hunks. The problem otherwise.
    pub fn check(&self, question: &Question) -> Result<(), String> {
        let kind = question.kind;
        if question.format() != self.format {
            return Err(format!(
                "a {:?} question was asked for, not {:?}",
                self.format,
                question.format()
            ));
        }
        if !self.kinds.contains(&kind) {
            return Err(format!("kind {} was not offered", kind.name()));
        }
        match (kind.shows_code(), question.body.code()) {
            (true, None) => return Err(format!("a {} question needs code", kind.name())),
            (false, Some(_)) => return Err(format!("a {} question has no code", kind.name())),
            (_, Some(code)) if !code.fits() => {
                return Err(format!(
                    "the code must have 1 to {MAX_SNIPPET_LINES} lines, not {}",
                    code.lines()
                ));
            }
            _ => {}
        }
        if let QuestionBody::MultipleChoice {
            options,
            options_language: Some(_),
            ..
        } = &question.body
            && options.iter().any(|o| line_count(o) > MAX_SNIPPET_LINES)
        {
            return Err(format!(
                "a code option has more than {MAX_SNIPPET_LINES} lines"
            ));
        }
        if kind == QuestionKind::ChooseTheFix && !question.body.options_are_code() {
            return Err("the options of a choose_the_fix question are code".into());
        }
        if kind == QuestionKind::ExplainYourChange {
            let code = question.body.code().expect("checked above");
            let copied = code.origin == SnippetOrigin::Change
                && self
                    .hunks
                    .iter()
                    .any(|h| h.text == code.text && h.language == code.language);
            if !copied {
                return Err("an explain_your_change snippet is a hunk of the change".into());
            }
        }
        Ok(())
    }
}

impl Question {
    /// Whether this question may be stored or reused outside the
    /// repository it was asked in, e.g. by a shared question bank
    /// (ADR-0007 D6): only a question about a `General` concept, whose
    /// snippet, if any, the examiner wrote.
    pub fn may_leave_repository(&self) -> bool {
        self.scope == ConceptScope::General
            && self
                .body
                .code()
                .is_none_or(|c| c.origin == SnippetOrigin::General)
    }
}

impl Diff {
    /// The hunks of this diff at `locations`, as they are after the change
    /// (ADR-0007 D4): for each location, the new-side lines of each hunk of
    /// its file that fall in its range, at most [`MAX_SNIPPET_LINES`] of
    /// them, and at most [`MAX_HUNKS`] hunks in all, without duplicates.
    /// Locations in files this diff doesn't touch give nothing.
    pub fn hunks_at(&self, locations: &[CodeLocation]) -> Vec<Hunk> {
        let mut out: Vec<Hunk> = Vec::new();
        if locations.is_empty() {
            return out;
        }
        for (path, section) in self.sections() {
            let Some(path) = path else { continue };
            let wanted: Vec<&CodeLocation> = locations.iter().filter(|l| l.file == path).collect();
            if wanted.is_empty() {
                continue;
            }
            let lines = new_side_lines(section);
            for loc in wanted {
                let (start, end) = (loc.start.min(loc.end), loc.start.max(loc.end));
                // Lines in range, grouped into runs of consecutive numbers,
                // so that two hunks of the file stay apart.
                let mut run: Vec<(u32, &str)> = Vec::new();
                let flush = |run: &mut Vec<(u32, &str)>, out: &mut Vec<Hunk>| {
                    if run.is_empty() {
                        return;
                    }
                    let text: Vec<&str> = run
                        .iter()
                        .take(MAX_SNIPPET_LINES)
                        .map(|(_, l)| *l)
                        .collect();
                    let hunk = Hunk {
                        file: path.to_string(),
                        start: run[0].0,
                        language: language_of(path).to_string(),
                        text: text.join("\n").trim_end().to_string(),
                    };
                    run.clear();
                    if !hunk.text.trim().is_empty() && !out.contains(&hunk) {
                        out.push(hunk);
                    }
                };
                for &(n, line) in lines.iter().filter(|(n, _)| (start..=end).contains(n)) {
                    if run.last().is_some_and(|&(last, _)| last + 1 != n) {
                        flush(&mut run, &mut out);
                    }
                    run.push((n, line));
                }
                flush(&mut run, &mut out);
            }
        }
        out.truncate(MAX_HUNKS);
        out
    }
}

/// The new-side lines of one file's section of a diff, with their line
/// numbers: context and added lines of every `@@` hunk.
fn new_side_lines(section: &str) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    let mut next: Option<u32> = None;
    for line in section.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            next = hunk_new_start(header);
            continue;
        }
        let Some(n) = next else { continue };
        if let Some(code) = line.strip_prefix('+') {
            if line.starts_with("+++ ") {
                continue;
            }
            out.push((n, code));
            next = Some(n + 1);
        } else if let Some(code) = line.strip_prefix(' ') {
            out.push((n, code));
            next = Some(n + 1);
        } else if line.is_empty() {
            // Some tools drop the space of an empty context line.
            out.push((n, ""));
            next = Some(n + 1);
        } else if line.starts_with('-') || line.starts_with('\\') {
            // Removed lines and "\ No newline at end of file".
        } else {
            next = None;
        }
    }
    out
}

/// The new-side start of a hunk header's rest, `-a,b +c,d @@ ...` → `c`.
fn hunk_new_start(header: &str) -> Option<u32> {
    let plus = header.split_whitespace().find(|w| w.starts_with('+'))?;
    let start = plus[1..].split(',').next()?;
    // A hunk of an emptied file starts at line 0 and has no new lines.
    start.parse::<u32>().ok().map(|s| s.max(1))
}

/// The language tag for a file, from its name or extension; `text` if
/// unknown.
pub fn language_of(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "Dockerfile" || name.starts_with("Dockerfile.") {
        return "dockerfile";
    }
    if name == "Makefile" {
        return "makefile";
    }
    let ext = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => return "text",
    };
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "sh" | "bash" | "zsh" => "bash",
        "sql" => "sql",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "json" => "json",
        "html" | "htm" => "html",
        "css" => "css",
        "md" => "markdown",
        "nix" => "nix",
        "tf" => "hcl",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ConceptId, QuestionId};

    // @adr-0008-d1
    #[test]
    fn code_paths_stay_inside_the_repository() {
        for (raw, path) in [
            ("src/lib.rs", Some("src/lib.rs")),
            ("./src//lib.rs/", Some("src/lib.rs")),
            ("", Some("")),
            (".", Some("")),
            ("/etc/passwd", None),
            ("../secrets", None),
            ("src/../../x", None),
            ("C:/x", None),
            ("HEAD:src", None),
            ("src\\lib.rs", None),
            ("src/\nlib.rs", None),
        ] {
            assert_eq!(code_path(raw).as_deref(), path, "{raw:?}");
        }
    }

    fn loc(file: &str, start: u32, end: u32) -> CodeLocation {
        CodeLocation {
            file: file.into(),
            start,
            end,
        }
    }

    const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\n\
        --- a/src/lib.rs\n\
        +++ b/src/lib.rs\n\
        @@ -10,4 +10,5 @@ fn outer() {\n \
        let a = 1;\n\
        -    let b = 2;\n\
        +    let b = 3;\n\
        +    let c = a + b;\n \
        println!(\"{c}\");\n\
        \n\
        @@ -40,2 +41,2 @@\n \
        x();\n\
        +y();\n\
        diff --git a/README.md b/README.md\n\
        --- a/README.md\n\
        +++ b/README.md\n\
        @@ -1 +1 @@\n\
        -old\n\
        +new\n";

    // @adr-0007-d4
    #[test]
    fn hunks_are_the_new_side_at_a_location() {
        let diff = Diff(DIFF.into());
        let hunks = diff.hunks_at(&[loc("src/lib.rs", 11, 12)]);
        assert_eq!(
            hunks,
            vec![Hunk {
                file: "src/lib.rs".into(),
                start: 11,
                language: "rust".into(),
                text: "    let b = 3;\n    let c = a + b;".into(),
            }]
        );
        assert_eq!(hunks[0].end(), 12);
        // A range across two hunks gives two hunks; removed lines are gone.
        let both = diff.hunks_at(&[loc("src/lib.rs", 1, 100)]);
        assert_eq!(both.len(), 2, "{both:?}");
        assert_eq!(
            both[0].text,
            "let a = 1;\n    let b = 3;\n    let c = a + b;\nprintln!(\"{c}\");"
        );
        assert_eq!(both[1].start, 41);
        assert_eq!(both[1].text, "x();\ny();");
        // Unknown files and empty ranges give nothing; duplicates are kept once.
        assert!(diff.hunks_at(&[loc("src/other.rs", 1, 9)]).is_empty());
        assert!(diff.hunks_at(&[loc("src/lib.rs", 20, 30)]).is_empty());
        assert_eq!(
            diff.hunks_at(&[loc("README.md", 1, 1), loc("README.md", 1, 1)])
                .len(),
            1
        );
        assert_eq!(
            diff.hunks_at(&[loc("README.md", 1, 1)])[0].language,
            "markdown"
        );
    }

    // @adr-0007-d3
    #[test]
    fn hunks_are_cut_to_the_snippet_limit() {
        let mut text =
            String::from("diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -0,0 +1,40 @@\n");
        for i in 1..=40 {
            text.push_str(&format!("+line {i}\n"));
        }
        let hunks = Diff(text).hunks_at(&[loc("a.py", 1, 40), loc("a.py", 30, 31)]);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].snippet().lines(), MAX_SNIPPET_LINES);
        assert!(hunks[0].snippet().fits());
        assert_eq!(hunks[1].text, "line 30\nline 31");
        assert_eq!(hunks[0].snippet().origin, SnippetOrigin::Change);
    }

    #[test]
    fn languages_come_from_the_file_name() {
        for (path, lang) in [
            ("src/main.rs", "rust"),
            ("a/b/c.PY", "python"),
            ("web/x.tsx", "tsx"),
            (".github/workflows/ci.yml", "yaml"),
            ("Dockerfile", "dockerfile"),
            ("LICENSE", "text"),
            (".gitignore", "text"),
        ] {
            assert_eq!(language_of(path), lang, "{path}");
        }
    }

    #[test]
    fn kinds_have_names() {
        for k in QuestionKind::ALL {
            assert_eq!(QuestionKind::from_name(k.name()), Some(k));
        }
        assert_eq!(
            QuestionKind::from_name("PredictOutcome"),
            Some(QuestionKind::PredictOutcome)
        );
        assert_eq!(
            QuestionKind::from_name(" Spot the bug "),
            Some(QuestionKind::SpotTheBug)
        );
        assert_eq!(QuestionKind::from_name("quiz"), None);
    }

    fn hunk() -> Hunk {
        Hunk {
            file: "src/lib.rs".into(),
            start: 3,
            language: "rust".into(),
            text: "let x = 1;".into(),
        }
    }

    fn question(kind: QuestionKind, body: QuestionBody) -> Question {
        Question {
            id: QuestionId("q".into()),
            concept: ConceptId("c.q".into()),
            scope: ConceptScope::General,
            kind,
            body,
            explanation: "why".into(),
            checked: false,
            banked: None,
        }
    }

    fn open(code: Option<CodeSnippet>) -> QuestionBody {
        QuestionBody::Open {
            prompt: "Why?".into(),
            code,
            rubric: "r".into(),
        }
    }

    fn general(text: &str) -> CodeSnippet {
        CodeSnippet {
            language: "rust".into(),
            text: text.into(),
            origin: SnippetOrigin::General,
        }
    }

    // @adr-0007-d1 @adr-0007-d2
    #[test]
    fn a_brief_keeps_the_kinds_that_fit() {
        use QuestionKind::*;
        let all = QuestionKind::ALL;
        let mc = QuestionBrief::new(QuestionFormat::MultipleChoice, &all, vec![]);
        assert_eq!(
            mc.kinds,
            [Scenario, PredictOutcome, SpotTheBug, ChooseTheFix],
            "no hunk, nothing to explain"
        );
        let open = QuestionBrief::new(
            QuestionFormat::Open,
            &[ExplainYourChange, ChooseTheFix],
            vec![hunk()],
        );
        assert_eq!(
            open.kinds,
            [ExplainYourChange, Scenario],
            "choose_the_fix is multiple choice only"
        );
    }

    // @adr-0007-d2 @adr-0007-d3 @adr-0007-d4
    #[test]
    fn a_brief_checks_what_was_written() {
        use QuestionKind::*;
        let brief = QuestionBrief::new(
            QuestionFormat::Open,
            &[ExplainYourChange, PredictOutcome],
            vec![hunk()],
        );
        assert_eq!(
            brief.check(&question(ExplainYourChange, open(Some(hunk().snippet())))),
            Ok(())
        );
        assert_eq!(
            brief.check(&question(PredictOutcome, open(Some(general("f()"))))),
            Ok(())
        );
        assert_eq!(brief.check(&question(Scenario, open(None))), Ok(()));
        // Not offered, or not the format asked for.
        assert!(
            brief
                .check(&question(SpotTheBug, open(Some(general("f()")))))
                .is_err()
        );
        let mc = QuestionBody::MultipleChoice {
            prompt: "?".into(),
            code: None,
            options: vec!["a".into(), "b".into(), "c".into()],
            options_language: None,
            correct: 0,
            reasons: vec![],
        };
        assert!(brief.check(&question(Scenario, mc)).is_err());
        // Code exactly when the kind shows code.
        assert!(brief.check(&question(PredictOutcome, open(None))).is_err());
        assert!(
            brief
                .check(&question(Scenario, open(Some(general("f()")))))
                .is_err()
        );
        // The author's code is copied from a hunk, not retyped.
        let retyped = CodeSnippet {
            text: "let x = 2;".into(),
            ..hunk().snippet()
        };
        assert!(
            brief
                .check(&question(ExplainYourChange, open(Some(retyped))))
                .is_err()
        );
        let relabelled = CodeSnippet {
            origin: SnippetOrigin::General,
            ..hunk().snippet()
        };
        assert!(
            brief
                .check(&question(ExplainYourChange, open(Some(relabelled))))
                .is_err()
        );
        // At most 15 lines.
        let long = general(&vec!["x();"; MAX_SNIPPET_LINES + 1].join("\n"));
        assert!(
            brief
                .check(&question(PredictOutcome, open(Some(long))))
                .is_err()
        );
    }

    // @adr-0007-d1 @adr-0007-d3
    #[test]
    fn choose_the_fix_has_code_options() {
        let brief = QuestionBrief::new(
            QuestionFormat::MultipleChoice,
            &[QuestionKind::ChooseTheFix],
            vec![],
        );
        let body = |options_language: Option<String>| QuestionBody::MultipleChoice {
            prompt: "Which fix?".into(),
            code: Some(general("let v = &mut x;\nlet w = &x;")),
            options: vec!["drop(v);".into(), "let w = x.clone();".into(), "x;".into()],
            options_language,
            correct: 0,
            reasons: vec![],
        };
        assert_eq!(
            brief.check(&question(
                QuestionKind::ChooseTheFix,
                body(Some("rust".into()))
            )),
            Ok(())
        );
        assert!(
            brief
                .check(&question(QuestionKind::ChooseTheFix, body(None)))
                .is_err()
        );
    }

    // @adr-0007-d6
    #[test]
    fn only_general_questions_with_general_code_leave_the_repository() {
        let mut q = question(QuestionKind::Scenario, open(None));
        assert!(q.may_leave_repository());
        q.body = open(Some(general("f()")));
        assert!(q.may_leave_repository());
        q.body = open(Some(hunk().snippet()));
        assert!(!q.may_leave_repository(), "the author's code stays");
        q.body = open(Some(general("f()")));
        q.scope = ConceptScope::Project;
        assert!(!q.may_leave_repository(), "project concepts stay");
    }

    #[test]
    fn stored_questions_without_kind_or_code_still_load() {
        let stored = r#"{"id":"q","concept":"c.q","scope":"General","body":{"Open":{"prompt":"P?","rubric":"R"}},"explanation":"E"}"#;
        let q: Question = serde_json::from_str(stored).unwrap();
        assert_eq!(q.kind, QuestionKind::Scenario);
        assert_eq!(q.body.code(), None);
        let snippet: CodeSnippet =
            serde_json::from_str(r#"{"language":"rust","text":"x"}"#).unwrap();
        assert_eq!(
            snippet.origin,
            SnippetOrigin::Change,
            "unknown origin is the safe one"
        );
    }
}
