# ADR-0007: Question kinds and code snippets

- **Status:** Accepted
- **Date:** 2026-11-08
- **Refines:** ADR-0002 D5 (format rule), ADR-0003 D5 (only general
  concepts cross organisations), ADR-0004 D4 (GitHub's markdown subset),
  and the `Examiner` port (ADR-0001 D3)

## Context

No question asked so far showed any code. They were all prose scenarios
("A team packages its tool as a composite action…"). That was by design,
not chance (issue #15):

- Only two formats existed, multiple choice and open. Nothing said *what
  kind* of question it was.
- Both writer prompts required the prompt to be "self-contained,
  answerable without seeing the code".
- `Examiner::write_question(concept, format)` got the extracted concept
  only, never the diff, so it couldn't quote the change even if allowed.
- ADR-0004 D4 allowed no `<pre>`, and fenced code in model text became
  inline `<code>`, so a multi-line snippet would have been unreadable.

Questions about a code change are more useful, and harder to answer from
general knowledge alone, when they show code, ideally the author's own.
Code from the change is also the most sensitive text Soqrates handles: it
is untrusted input that is shown back to people, and it must not leave the
repository it came from.

## Glossary additions

This ADR adds the terms Kind (`QuestionKind`), Code snippet
(`CodeSnippet`), Hunk (`Hunk`) and Brief (`QuestionBrief`) to the
[ADR-0002 glossary](0002-domain-model.md#glossary).

## Decisions

### D1: A question has a kind, separate from its format
**Enforcement:** automated

Every question has a `QuestionKind` next to its `QuestionFormat`:

| Kind | The question | Formats |
|---|---|---|
| `Scenario` | A realistic situation in prose, as before. No code. | both |
| `PredictOutcome` | A snippet; what happens when it runs, or which line fails, and why. | both |
| `SpotTheBug` | A snippet with one subtle bug; pick or describe it. | both |
| `ChooseTheFix` | A broken snippet; the options are code alternatives. | multiple choice only |
| `ExplainYourChange` | A hunk of the author's own diff; why is this needed, what breaks without it. | both |

Questions stored before kinds existed load as `Scenario`. Grading is
unchanged for multiple choice. For open answers the grader is shown the
snippet, and the rubric can refer to it.

### D2: The level steers the kind, the examiner chooses it per concept
**Enforcement:** automated

This extends ADR-0002 D5. The format still follows the level. The kind is
chosen by the examiner for each concept, from the kinds the author's level
allows, in this order of preference:

| Level | Multiple choice | Open |
|---|---|---|
| Novice | `PredictOutcome`, `SpotTheBug`, `Scenario`, `ChooseTheFix` | n/a |
| Intermediate | `SpotTheBug`, `ChooseTheFix`, `PredictOutcome`, `Scenario` | `ExplainYourChange`, `PredictOutcome`, `Scenario`, `SpotTheBug` |
| Expert | n/a | `ExplainYourChange`, `SpotTheBug`, `Scenario`, `PredictOutcome` |

Rounds mix kinds: kinds already asked in the session move to the back of
the list, keeping their order (`policy::kinds_for`). The examiner takes
the first kind that suits the concept and its code; `ExplainYourChange`
needs a hunk of the change, so a due review, which has none, never gets
it. The core checks the written question against the brief
(`QuestionBrief::check`): a kind that wasn't offered, or that doesn't fit
its format, fails the session start like any other examiner error.
Carried questions (ADR-0002 D9) keep their kind.

### D3: Code is a field of the question, never parsed out of prose
**Enforcement:** automated

The question body holds `code: Option<CodeSnippet>`, a `language` and a
`text`, kept separate from the prompt. A multiple-choice question whose
options are code says so (`options_language`), and its options render as
code. Code is never extracted from the model's prose: the examiner asks
for it in its own field of the structured reply, and prose keeps being
rendered as prose (ADR-0004 D4).

A snippet, and each code option, is at most 15 lines
(`MAX_SNIPPET_LINES`), so the comment stays readable. Hunks are cut to
that length before the writer sees them. A snippet the model writes that
is longer is rejected and asked for again, never cut, so that the
question and its explanation are about the code as shown (D7).

### D4: The writer gets the hunks of its concept
**Enforcement:** automated

The `Examiner` port changes: concept extraction returns, per concept, the
files and new-side line ranges it was found in (`Concept::locations`), and
`write_question` takes a `QuestionBrief`: the format, the kinds allowed
(D2), and the hunks of the diff at those locations (`Diff::hunks_at`),
the code as it is after the change.

For `ExplainYourChange`, the snippet *is* the author's code: the examiner
picks one of the hunks it was given and the snippet is copied from it, not
retyped by the model, and the core checks that it was. For the other
kinds, the writer may write a small general snippet that mirrors the
change. The diff and its hunks stay untrusted data: the prompts put them
in delimited blocks and tell the model to ignore any instructions inside
them, as before.

### D5: Code renders as inert code on every surface
**Enforcement:** automated

- **GitHub:** a snippet, and every code option, is a fenced code block
  with a language tag. The fence is made of backticks and is longer than
  any run of backticks in the code, so nothing in the code can close it,
  and the language tag is restricted to letters, digits and `+#._-`.
  Inside the block, `</details>`, `@mentions`, task boxes and markers are
  inert text. Reading the boxes and deciding whether a comment still takes
  answers ignore everything inside fenced blocks, so code can't forge a
  box or a heading. ADR-0004 D4 allows exactly these fenced blocks.
- **Terminal:** the code as it is, indented, with control characters
  (other than tabs) removed, so it can't drive the terminal.
- **MCP:** the snippet and whether the options are code are returned as
  fields, next to the prompt.

### D6: Code from the repository stays with the repository
**Enforcement:** automated

This refines ADR-0003 D5. A `CodeSnippet` records its origin: `Change`
when it was copied from the author's diff, `General` when the examiner
wrote it. A question with a snippet is stored only where question text
may be: in the pull request's comment and in the pending session, which
lives with the repository's sessions. It never reaches the learner store.
Only a question about a `General` concept whose snippet, if any, is
`General` may be reused outside its repository, e.g. by a shared question
bank (issue #14); `Question::may_leave_repository` says which.

### D7: The explanation matches the snippet as shown
**Enforcement:** automated

For `SpotTheBug` and `ChooseTheFix`, the examiner must quote the flawed
line, and the quote must be a line of the snippet exactly as it will be
shown; otherwise the reply is rejected and asked for again. The flawed
line is added to the rubric of an open question, so the grader judges
against the same code the author saw. A `ChooseTheFix` option that just
repeats the flawed line can't be the correct one.

The option check of issue #5 is to cover the code as well: it is to be
given the snippet and the code options, so that it can catch reasons that
discuss code that isn't shown.

## Consequences

- Questions can show the author's own code, which makes them harder to
  answer from general knowledge alone.
- The `Examiner` port changes (D4). Every examiner, including the test
  fakes, gets the brief and must respect it.
- The writer prompts grow: kinds, hunks and code rules. They no longer
  require prompts to be answerable without the code.
- Comments get longer; the 15-line cap (D3) bounds how much.
- Evals (ADR-0001 D13) can compare kinds on discrimination, i.e. whether
  authors who understand the change answer them correctly more often.
  The kind is stored with each question in the session; reviews in the
  learner profile don't record it.
- A carried question can show a hunk the push since removed. Rounds don't
  record where a concept came from, so this is accepted, as for carried
  questions in general (ADR-0002 D10).
