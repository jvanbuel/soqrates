# ADR-0009: A question bank

- **Status:** Accepted
- **Date:** 2026-11-09
- **Refines:** ADR-0001 D5 (state behind `StateStore`), ADR-0002 D9 and D10
  (what a round asks), ADR-0003 D5 (what the learner store holds),
  ADR-0004 D1 (the option check of issue #5), ADR-0005 D2 (a table per
  kind of record), ADR-0007 D6 (code stays with the repository)

## Context

Every question so far was written by the examiner from scratch
(`Examiner::write_question`), even when the same concept was asked minutes
earlier (issue #14). On one pull request, four pushes produced twelve new
questions about largely the same handful of concepts. That is a model call
per question, and the quality of the questions varies from round to round.

Concept ids are already keys: the examiner names concepts such as
`rust.ownership`, so finding earlier questions about a concept is an exact
lookup. Keeping the questions Soqrates writes, and asking them again, saves
those calls and lets a good question be asked more than once.

Four forces shape how:

1. **Memorisation.** A learner who gets the identical question again may
   remember the answer rather than the concept. Spaced repetition works
   better with varied questions, so a reused question must be one the
   learner hasn't seen, or hasn't seen for a while, and Soqrates must know
   which questions each learner has seen.
2. **Privacy.** Questions are written from diffs and can quote code. The
   learner store holds no question text (ADR-0003 D5), and code from a
   repository stays with that repository (ADR-0007 D6).
3. **Quality.** A question whose explanation argued about an answer it
   doesn't offer (issue #5) should not be asked again and again.
4. **Staleness.** A stored question may stop matching how a library works.
   Questions must be revisable and retirable, and issue #22 (author feedback
   on questions) needs a stable handle on a stored question to do that.

Embeddings would help where the examiner names one idea differently across
diffs (`gha.reusable-workflow-permissions` vs
`github-actions.workflow-call-token`). We don't know yet how often that
happens; the evals (ADR-0001 D13) can measure it once the bank exists.

## Glossary additions

This ADR adds the terms Question bank (`BankedQuestion`) and Seen questions
(`SeenQuestions`) to the [ADR-0002 glossary](0002-domain-model.md#glossary).

## Decisions

### D1: A question bank keyed by concept, behind `StateStore`
**Enforcement:** automated

The question bank is a new kind of record behind the `StateStore` port
(ADR-0001 D5): `bank_questions(concept)` returns the bank's questions about
one concept, in the order they were banked, and `save_bank_question`
creates or replaces one. Each `BankedQuestion` holds the question as it was
asked (concept, format, kind, body, explanation), the level of the learner
it was written for, when it was banked, how often it was asked, and whether
it is retired.

Lookup is by concept id only. No embeddings and no similarity search: those
come later, if the evals show that concept ids fragment in practice.

Every adapter implements it. The file store keeps one document per question
in a directory per concept (`bank/<hex concept>/<hex id>.json`). The SQL
store adds the tables `soqrates_bank` (`id`, `concept`, `doc`, indexed by
concept) and `soqrates_seen` (D5), within schema version 1: new tables are
invisible to older builds, so they can't misread them (ADR-0005 D2, D3).

### D2: The bank lives with the sessions, never in the learner store
**Enforcement:** automated

By default the bank is kept in the store of the author's sessions: the
organisation's store, or the single store of a deployment without
organisations. Each organisation therefore has its own bank. With
organisation stores configured, an author whose forge belongs to no
organisation gets no bank: the remaining store is the learner store, shared
across organisations, and it holds no question text (ADR-0003 D5).

A deployment may instead point every organisation at one shared bank, a
separate store (`Soqrates::with_shared_bank`). The shared bank is never the
learner store either. The bank can be turned off (`Config::question_bank`).

### D3: Only consistent questions that may leave the repository are banked
**Enforcement:** automated

A written question is banked only if both hold
(`BankedQuestion::bankable`):

- **It passed the consistency check** (issue #5, ADR-0004 D1). The examiner
  marks a multiple-choice question `checked` when the option check found
  nothing; a check reply it couldn't parse keeps the question, as before,
  but unchecked. An open question has no options to check and is checked
  once it is valid.
- **It may leave its repository** (`Question::may_leave_repository`,
  ADR-0007 D6): its concept is `General` and it shows no code from the
  change. So a question that quotes repository code, such as every
  `ExplainYourChange` question, never reaches any bank, shared or not.

Questions about `Project` concepts are not banked in this first step. A
bank scoped to one repository could hold them, but no surface passes the
repository to the core when a round is composed (local hooks and MCP don't
know it), so that is left for later.

### D4: A round asks from the bank first, as is
**Enforcement:** automated

For each concept a round asks about (ADR-0002 D10), the core builds the
brief as before (ADR-0007 D2) and looks in the bank first. A banked
question suits the brief if it isn't retired and the brief's check accepts
it: the brief's format, a kind it offers, code as that kind needs. Of the
questions that suit and that the learner may be shown (D5), the core takes
an unseen one before a seen one, then the least recently seen, then the
brief's most preferred kind, then the oldest (`pick`). Only if none is left
does it call `write_question`; the new question is banked if it may be
(D3). Writing a question that is already banked banks it once.

A question from the bank is asked as it is, in the concept's scope of this
round. A due review whose scope isn't known, as in a deployment without
organisation stores, counts as `Project` (ADR-0003 D5): it may take a
question from the bank, but a question written for it is not banked. There is no call to adapt it to the current change
("recontextualising") in this step: the questions that may be banked are
general ones (D3), which need no adapting, and an extra model call per
question would undo much of the saving. It can be added later as an
option, measured by the evals.

### D5: A learner is not shown a bank question twice within N rounds
**Enforcement:** automated

The bank records, per learner, which bank questions they were shown and in
which of their rounds (`SeenQuestions`): every composition of a round's
questions is one round, and every question taken from or added to the bank
in it counts as shown. A bank question is shown to a learner at most once
within `Config::reask_after_rounds` rounds (10 by default). Carried
questions (ADR-0002 D9) are not shown again.

`SeenQuestions` lives with the bank it refers to (D2), never in the learner
store, and holds bank ids and round numbers only, no question text.
Deleting a learner deletes it too (ADR-0003 D6).

### D6: A banked question has a stable id and a version
**Enforcement:** automated

A banked question is identified by a `BankId`, a hash of what it asks
(concept, kind, body and explanation) taken when it is first banked, and
has a version, 1 at first. A revision (`BankedQuestion::revise`, for issue
#22) keeps the id and counts the version up; a revision that may not be
banked retires the question instead. A retired question is kept but never
offered again.

Every question taken from or added to the bank carries a `BankRef`, its id
and version, into the session, so later feedback and grades can be traced
to the question that was asked.

## Consequences

- A round about a concept already in the bank makes no `write_question`
  call for it, so repeated rounds and due reviews cost fewer model calls,
  and a learner meets the same good question again only after N rounds.
- Questions about project concepts, and questions that show the author's
  own code, are still written every time (D3). They are the more specific
  questions, so the saving is smaller than it could be.
- Reused questions are not adapted to the change (D4). A general question
  can feel less tied to the diff than a fresh one.
- Grades don't feed back into the bank yet: nothing retires a question
  that is often graded wrong by learners who know the concept. The `asked`
  count, the `checked` flag and the version are the hooks for that, and for
  author feedback in issue #22.
- Merging two learners (ADR-0003 D3) doesn't merge what they were shown,
  so a merged learner can see a question again sooner than N rounds.
- With a fixed bank and few questions per concept, a learner who has seen
  them all within N rounds gets a newly written question, which adds to the
  bank: the bank grows with use until it covers N rounds of every concept.
