# ADR-0010: Follow-up rounds

- **Status:** Accepted
- **Date:** 2026-11-09
- **Amends:** ADR-0001 D8 (graded is final for the commit), ADR-0002 D6
  (session rules)
- **Refines:** ADR-0001 D9 (local hooks), ADR-0002 D5 (format rule),
  ADR-0002 D9 (rounds), ADR-0004 D5 and D7 (frozen and superseded comments),
  and the `Examiner` port (ADR-0001 D3)

## Context

A graded session was final for its commit (ADR-0001 D8, ADR-0002 D6). On
PR #9 the author answered 1 of 3 questions correctly. The results comment
explained the two wrong answers, but nothing checked whether the
explanations landed: the concepts only came back as reviews in a later
session, possibly days and several pull requests later. In Blocking mode,
a failed session could only be fixed by pushing a new commit, which asks
about whatever that push changes, not about what was missed (issue #11).

So after grading, Soqrates asks one new question about each concept that
was answered wrongly, as a follow-up round on the same commit.

## Glossary additions

This ADR adds the term Follow-up (`FollowUp`) to the
[ADR-0002 glossary](0002-domain-model.md#glossary).

## Decisions

### D1: Every wrong answer gets a new question about its concept
**Enforcement:** automated

When a session is graded and some of its answers are wrong (`Again`,
unanswered ones included), Soqrates opens a follow-up round: one new
question about each of those concepts, and only those. Concepts answered
correctly are done.

The question is new, not the one asked before: the examiner writes it with
`Examiner::write_follow_up(concept, brief, previous, answer, code)`, which
is shown the earlier question and the author's answer, so that it can aim
at the misunderstanding. It asks about the concept from another angle, in
another situation or about another misconception, so that the right
answer can't simply be copied from the explanation just shown. The core
checks that the question is about the same concept, fits its brief
(ADR-0007 D2) and has a different prompt from the earlier one; otherwise
the follow-up fails like any other examiner error.

### D2: A follow-up round is a session of the same commit, up to a limit
**Enforcement:** automated

This amends ADR-0001 D8 and ADR-0002 D6: a *session* is graded at most
once and its answers are final, but a graded commit can ask again, in a
follow-up round.

- A follow-up round is a new session. Its key is the first round's key
  with the round number appended, `…@<sha>#2`, then `#3`
  (`SessionKey::follow_up`). Its session records where the round stands
  (`FollowUp`): its number, and how many of the first round's concepts
  were answered correctly before it, out of how many.
- At most `max_follow_ups` follow-up rounds per session (default 2;
  `max_follow_ups = 0`, or `SOQRATES_MAX_FOLLOW_UPS=0`, turns them off).
  After the last one, the result stands and the concepts come back as
  reviews (ADR-0001 D6).
- The CI check reports on the latest round of the session for its head
  (`Soqrates::latest_follow_up`), not on one session per head.
- A follow-up round is opened only after its first round's results are
  posted. If it can't be written or posted, the run fails, its session is
  discarded, and the graded result stands: the next run reports it.

### D3: Follow-ups are multiple choice
**Enforcement:** automated

This refines ADR-0002 D5. A follow-up comes right after an explanation, so
it is quick to answer: every follow-up is multiple choice, at any level and
whatever `open_questions` says. The kinds are those the author's level
allows (ADR-0007 D2), with the earlier question's kind moved to the back,
so the follow-up tends to come from another angle. It has no hunks, so it
is never `ExplainYourChange`.

### D4: The check waits for the follow-up, and the final round sets it
**Enforcement:** automated

While a follow-up round is open, the check stays `Pending`, also when the
first round already passed. When the final round is graded, the check is
set from the result after it: each concept of the first round counts with
its latest answer. So a follow-up can turn a failure into a pass, in
Blocking mode too, and never turns a pass into a failure. The check
summary, and the results comment of a follow-up, say "passed after a
follow-up", so the first attempt stays visible.

### D5: The first answer stays the review
**Enforcement:** automated

Only the first round records reviews. A wrong first answer is recorded as
`Again`, as before, so the concept stays due soon. Grading a follow-up
records nothing on the learner profile, right or wrong: it only settles
the session. A correct follow-up straight after the explanation says
little about long-term memory, and a second review on the same day would
distort the schedule. Follow-ups don't count as "already asked on this
pull request" either (ADR-0002 D9): their concepts are those of the first
round, which already count.

### D6: A new push supersedes an open follow-up round
**Enforcement:** automated

This refines ADR-0002 D9 and ADR-0004 D7. A push that brings something new
to ask freezes an open follow-up comment as superseded, like any question
comment, and discards its session. Its questions are not carried into the
new round: their concepts were already graded, recorded as `Again`, and
come back as due reviews. Answers to the follow-up submitted before the
push are graded first, as for any round, but that grading opens no further
follow-up: the pull request has moved on. A push with nothing new to ask
keeps the follow-up open, and answering it sets the check on the new head.

On GitHub, a follow-up is posted as a new question comment, answered with
checkboxes or `/soqrates answer` like any other (ADR-0004 D3); it says it
is a follow-up. The earlier question comment stays frozen as graded
(ADR-0004 D5).

### D7: Local hooks ask follow-ups right away
**Enforcement:** automated

This refines ADR-0001 D9. After a local hook session is graded and its
results are shown, the hook asks the follow-up questions at the terminal
right away, up to `max_follow_ups` rounds, and shows each round's results.
The commit or push is allowed or blocked on the final round's result (D4).

## Consequences

- An author learns whether an explanation landed while it is still fresh,
  and in Blocking mode can fix a failed check by understanding, not only by
  pushing.
- Each wrong answer costs another question to write, a model call or more
  (ADR-0004 D1), per follow-up round; `max_follow_ups` bounds it.
- The `Examiner` port gains `write_follow_up`. Its default fails, so an
  examiner that can't write follow-ups, such as the MCP server's test
  examiners, never asks one by mistake; the Rig examiner and the
  acceptance-test fake implement it.
- A first round that passed with a wrong answer still waits for its
  follow-up before the check turns green. `max_follow_ups = 0` restores the
  old behaviour for teams that find this too strict.
- Evals (ADR-0001 D13) can compare follow-up answers with first answers to
  see whether explanations work. The reviews in the learner profile don't
  record follow-ups (D5), so this needs the session events.
- MCP clients are unchanged: they grade a session and can start another
  one themselves.
