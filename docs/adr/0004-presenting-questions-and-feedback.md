# ADR-0004: Presenting questions and feedback

- **Status:** Accepted
- **Date:** 2026-11-07
- **Refines:** ADR-0001 D8 (CI round trip), D9 (local hooks), D10 (MCP)

## Context

Until now, authors only saw a score: a one-line commit status on a PR, or
"1 of 2 correct" in the terminal. A learning tool earns most of its value
after the answer, from learning *why* an answer was right or wrong, and
when the concept comes back. The examiner already produced a rationale
when grading open answers, but it was thrown away.

GitHub comments support GitHub-flavoured markdown and a sanitised subset
of HTML:

- **Allowed:** task-list checkboxes, alerts (`> [!TIP]`), `<details>` and
  `<summary>`, `<sub>`, `<sup>`, `<kbd>`, `<br>`, tables and images.
- **Removed:** forms, `<input>`, `<script>`, `<style>`, inline event
  handlers and classes.

A checkbox in a bot's comment can be ticked by anyone with write access to
the repository. Each tick edits the comment and fires an
`issue_comment.edited` event. The REST API doesn't say who ticked a box,
but GraphQL reports a comment's last editor.

## Decisions

### D1: Every surface gives feedback after grading
**Enforcement:** automated

After a session is graded, each question gets a result:

- whether it was correct;
- the author's answer;
- for multiple choice, the correct option;
- a short explanation.

For a multiple-choice question, the examiner writes the explanation together
with the question. It is never shown before grading (ADR-0001 D10). The
explanation discusses only the options the question offers: a second, short
model call checks this, and a flagged question is written again once. For an
open question, the explanation is the grader's rationale. `SessionOutcome`
carries these results, and the PR, the terminal and MCP all present them.

### D2: The PR shows this session's results, never the profile
**Enforcement:** automated

A PR is read by other people (ADR-0003 D6). The results posted on a PR are
about this session only. The learner's level, review schedule and other
concepts are shown only where only the learner sees them: the terminal
and the MCP client.

### D3: Multiple choice on GitHub is answered with checkboxes
**Enforcement:** automated

The question comment shows each multiple-choice option as a task-list
checkbox, plus a final checkbox, "Grade my answers". The rules:

- The answers are graded when the author ticks that box, or posts a
  `/soqrates answer` comment.
- Exactly one ticked option is a choice. No ticked option means
  unanswered. More than one means wrong.
- Checkbox answers count only when the comment's last editor is the PR
  author, so nobody else can answer for them.
- Open questions are answered in a `/soqrates answer` comment, which also
  remains the fallback for authors who cannot tick boxes (no write access).
  Posting that comment submits all answers, ticked boxes included. An
  answer in the comment overrides the checkboxes of that question.

### D4: GitHub output uses only GitHub's markdown and HTML subset
**Enforcement:** automated

Comments use GitHub-flavoured markdown and these HTML elements only:
`details`, `summary`, `sub`, `sup`, `kbd`, `br`, `b`, `i`, `code`. Alerts
introduce the comment and the results. Instructions and explanations are
collapsed in `<details>`. A test fails if rendered output contains any
other tag.

### D5: Results are a new comment, and the questions are frozen
**Enforcement:** automated

When a session is graded, the results are posted as a new comment, which
notifies the author. The question comment is then edited to say it was
graded, with the questions collapsed, so that its checkboxes can no longer
be ticked.

### D6: The terminal shows results and the next review
**Enforcement:** automated

After a local hook session, the terminal shows:

- each question with ✓ or ✗;
- the correct option for multiple choice;
- the explanation;
- when each concept is next due for review;
- the learner's level.

### D7: A newer push freezes earlier question comments
**Enforcement:** automated

Each push that brings something new to ask starts a new session with its
own question comment (ADR-0001 D8, ADR-0002 D9), and only the newest one
counts. When the check runs for a head, every
earlier question comment on the PR that was never graded is edited to say
a newer commit superseded it. It links the new question comment and shows
its own questions collapsed, without boxes, as a graded comment does (D5). The
superseded sessions are discarded, so answers to them are never graded.
Graded comments stay as they are.

Only comments posted by the same account as the current question comment,
and only sessions of the same PR, are touched, so copying a marker into
another comment can't make Soqrates rewrite it. A failed edit is retried by
the next run.

A push that brings nothing new to ask posts no comment: the latest round's
session carries forward to the new head, and its question comment stays
live and counts (ADR-0002 D9). Only a round that asks nothing and has no
earlier session to carry forward, such as the first round of a PR with an
empty diff, gets a short comment, with no boxes, saying there is nothing to
answer. Earlier comments still taking answers link to it, so none stays
live with answers that would be silently ignored.

## Consequences

- The examiner's output grows: an explanation per multiple-choice
  question, and feedback per graded open answer.
- `Examiner::grade` returns a grade with feedback, and `ScmProvider`
  gains `post_results`, `superseded_questions` and `supersede_questions`.
- The dogfooding workflow also runs on `issue_comment.edited` for comments
  that carry a Soqrates marker.
- Authors without write access answer every question by comment.
- A collaborator could tick boxes after the author's last edit. Those
  ticks only count if the author edits the comment again afterwards. This
  is acceptable for an advisory learning check.
