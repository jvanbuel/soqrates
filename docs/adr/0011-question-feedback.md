# ADR-0011: Authors give feedback on questions

- **Status:** Accepted
- **Date:** 2026-11-09
- **Refines:** ADR-0001 D10 (the MCP tools), ADR-0002 D6 (grades are
  final), ADR-0003 D5 and D6 (what the learner store holds; deleting a
  learner), ADR-0004 D1, D5 and D8 (results and commands), ADR-0009 D4 and
  D6 (which bank questions are reused)

## Context

An author who thinks a question is wrong has nowhere to say so (issue #22).
Results are final (ADR-0002 D6, ADR-0004 D5): if the answer key is wrong,
the question is ambiguous, or an explanation discusses an answer that isn't
offered (issue #5), the author takes the `Again` and nobody hears about it.
That is the most direct signal of question quality there is, and it is
thrown away.

Feedback needs something to attach to. A `GradedSession` holds no question
text (ADR-0002 D6, ADR-0003 D5), and the question bank (ADR-0009) keeps only
checked, general questions without code from the change. Most questions,
those about project concepts or showing the author's own code, are never
stored once their session is graded.

Four forces shape the design:

1. **Privacy.** Questions quote code and answers are the author's own
   words; both stay out of the learner store (ADR-0003 D5) and with the
   organisation whose code they are about (ADR-0007 D6).
2. **Trust.** Feedback is free text from the author. It must not steer
   grading, inject markup into comments, or instruct a model.
3. **Learning.** If arguing could change a grade, authors would argue with
   the grader instead of learning the concept.
4. **Fairness.** One author's ten disputes are one opinion, and only the
   author whose answer was graded knows what they meant.

## Glossary additions

This ADR adds the terms Asked questions (`AskedSession`) and Question
feedback (`QuestionFeedback`) to the [ADR-0002 glossary](0002-domain-model.md#glossary).

## Decisions

### D1: Every graded question is kept with its answer, for feedback only
**Enforcement:** automated

Grading a session also keeps its questions, as asked, with the author's
answer, the grade and the explanation the author was shown: an
`AskedSession`, one per graded session, behind the `StateStore` port
(`load_asked`, `save_asked`, `asked_sessions`). It is written by the grading
journal (ADR-0002 D6), so it exists exactly when the session is graded. The
file store keeps it under `asked/`, the SQL store in the table
`soqrates_asked` (`session_key`, `learner`, `doc`), within schema version 1
(ADR-0005 D3).

An `AskedSession` lives in the store of the author's sessions: the
organisation's store, or the single store of a deployment without
organisations. With organisation stores configured, an author whose forge
belongs to no organisation has nothing kept, as for the bank (ADR-0009
D2): the remaining store is the learner store, which holds no question text
(ADR-0003 D5). It never goes to a shared bank either.

Asked questions are not a bank: they are never offered again. Reuse stays
with the question bank's own rules (ADR-0009 D3, D4). A question that came
from or went into the bank keeps its `BankRef`, id and version, so feedback
on a revised question is not mixed up with feedback on its first version,
and feedback on one bank question from several sessions can be combined.

Feedback is kept as long as the asked session: until the learner is deleted
(D8) or a maintainer clears the store. It never leaves the organisation's
store: a shared bank only learns which of its questions are disputed (D6),
by feedback id, without text.

### D2: Feedback is a command, a prompt or a tool, after grading
**Enforcement:** automated

- **GitHub.** The author comments `/soqrates feedback <question> <kind>
  [text]`, e.g. `/soqrates feedback 2 disagree option c is also correct`.
  The command is read by `soqrates-core`'s `read_command` with the same
  forgiving prefixes as answers (ADR-0004 D8): `/soqrates` or `/socrates` in
  any case, or `@<app slug> feedback`. It is about the latest results
  comment before it. Soqrates answers every feedback comment once, with a
  short comment saying it was recorded or why not, and quotes nothing from
  it. Editing a feedback comment changes nothing; a new comment is new
  feedback. The results comment shows a one-line "Disagree?" hint under each
  question with the command for that question. There are no checkboxes for
  kinds: a tick says nothing about why, and checkbox edits are already the
  answer mechanism (ADR-0004 D3).
- **Terminal.** After the results of a local session, the author is asked
  once: "Any feedback on a question?", answered as `<number> <kind> [text]`.
  Enter skips it; without a terminal there is no prompt (ADR-0001 D9).
- **MCP.** A fifth tool, `give_feedback`, takes `{session_key, question,
  kind, text?}`, where `question` is the question's id or its 1-based
  number.

Feedback is only on graded questions: on a session that wasn't graded, or
a question number it doesn't have, the author is told so and nothing is
stored. The same forge comment counts once, however often the check runs.

### D3: Five kinds, plus free text
**Enforcement:** automated

`FeedbackKind` is one of:

- `disagree`: the answer key or the grade is wrong;
- `unclear`: the question or an option is ambiguous;
- `wrong-explanation`: an explanation or an option's reason is wrong, or
  discusses an answer that isn't offered;
- `off-topic`: the question isn't about the change, or is about something
  it merely touches;
- `good`: a question worth keeping.

The first three are disputes (`FeedbackKind::disputes`). Text is optional,
trimmed and cut to 2000 characters. Kinds are read in any case, with `_`
for `-`.

### D4: Only the session's author counts, under a reviewer key
**Enforcement:** automated

Only the PR author's feedback comments are read, as for answers (ADR-0004
D3), and the core checks that the feedback comes from the learner whose
session it is. The feedback itself stores no identity: the asked session
names its learner by the opaque `LearnerId` (ADR-0003 D1), and an export
names it by a `ReviewerKey` derived from that, so several entries by one
person can be counted as one person. This is pseudonymous, not anonymous:
whoever holds the store can tell which entries share a learner.

### D5: Feedback text is untrusted and stays inert
**Enforcement:** automated

Feedback never changes a grade by itself and is never shown to the examiner
while grading. Soqrates' answers to feedback quote nothing of it, the
results hint is fixed text, and an export writes it as a JSON string. When a
later tool shows feedback to a model, for judging or rewriting a question,
it goes in a delimited block, as answers do (`<learner_answer>`):
`QuestionFeedback::for_model` wraps it in `<author_feedback>` and escapes
anything that would close the block.

### D6: Open disputes keep a bank question from being reused
**Enforcement:** automated

A dispute on a bank question is recorded on the `BankedQuestion` by its
feedback id (no text), in the bank the question came from. A bank question
with an open dispute doesn't suit any brief (ADR-0009 D4), so it is not
asked again until a maintainer has resolved every dispute on it (D7). `good`
feedback is recorded the same way and raises the question: among questions
that suit equally, by seen order and kind, the one with more `good`
feedback is taken first.

### D7: Grades are final; maintainers resolve feedback
**Enforcement:** automated

No feedback regrades anything (ADR-0002 D6 still holds). A maintainer marks
a feedback entry accepted or rejected (`Soqrates::resolve_feedback`, `soqrates
questions resolve <id> accept|reject`). Either closes the dispute on the bank
question. Accepting a dispute also retires the bank question (ADR-0009 D6):
the question was wrong, so it is not asked again until someone banks a fixed
one.

Whether accepting a `disagree` should also correct the author's review card
(e.g. `Again` → `Good`) is left for later. FSRS state can't simply be
replayed with one grade changed once later reviews have been recorded on
top of it, and the review may live in the shared learner store, which a
maintainer of one organisation doesn't control. Who may resolve feedback is
whoever can run the CLI against the organisation's store.

### D8: Deleting a learner deletes their feedback
**Enforcement:** automated

Deleting a learner (ADR-0003 D6) deletes every asked session of theirs, with
its answers and feedback, from every store, and removes their disputes and
`good` feedback from the bank questions they were about, so a deleted
learner's dispute no longer blocks a question.

### D9: Asked questions and feedback are exported as JSONL
**Enforcement:** automated

`soqrates questions export` writes one JSON line per asked question: the
question as asked (concept, scope, kind, format, body and explanation) and
its bank reference. With `--with-feedback` each line also has the reviewer
key, the author's answer, the grade, the explanation shown and every
feedback entry (id, kind, text, status, time). Lines never contain the
session key, which can name the author, or any identity. The export is for
the evals (ADR-0001 D13) and for tuning prompts: which concepts, kinds and
formats produce disputed questions.

## Consequences

- Every graded question's text and answer is now kept in the
  organisation's store, where only sessions used to be. Stores grow with
  every graded session; nothing prunes them yet.
- The MCP server has five tools, not four (ADR-0001 D10).
- An author can't win a dispute by arguing. A maintainer has to look, and a
  dispute nobody resolves keeps its bank question out of rounds for good.
- Accepting a dispute retires the question but leaves the author's review
  card as it was, so a wrong key still schedules that concept as failed
  once (D7).
- Merging learners (ADR-0003 D3) doesn't move their asked sessions: feedback
  on a session from before the merge is refused for the merged learner.
- Feedback lives in the organisation's store even for general questions, so
  a shared bank (ADR-0009 D2) sees which questions are disputed in which
  organisation only as anonymous ids, never the reasons.
