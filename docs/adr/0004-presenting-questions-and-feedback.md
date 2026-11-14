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

For a multiple-choice question, the examiner writes a reason for every
option together with the question: why the correct option is correct (the
explanation) and why each wrong option is wrong. Reasons are never shown
before grading (ADR-0001 D10). An author who picked a wrong option gets that
option's reason, then the explanation; every other author gets the
explanation alone, as do questions stored before reasons existed.

The reasons are written in the same reply as the options, and each one is
about an option that is shown. That is not enough on its own: in the first
live run an explanation still argued against an answer the question didn't
offer (issue #5). So a second, short prompt checks every multiple-choice
question that passes validation, and quotes any passage of a reason that
discusses an answer not among the options. A question with such a passage
counts as invalid output: it is written again once, with the passages
quoted, and fails like other invalid output after that. A check reply that
can't be parsed keeps the question. An eval (ADR-0001 D13, `option_consistency`
in `evals/`) tracks how often explanations still discuss
answers that aren't offered.

For an open question, the explanation is the grader's rationale.
`SessionOutcome` carries these results, and the PR, the terminal and MCP
all present them.

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
  `/soqrates answer` comment (D8 says which spellings count).
- Exactly one ticked option is a choice. No ticked option means
  unanswered. More than one means wrong.
- Checkbox answers count only when the comment's last editor is the PR
  author, so nobody else can answer for them.
- Open questions are answered in their answer field in the same comment
  (D11). A `/soqrates answer` comment remains the fallback for authors who
  cannot edit the comment (no write access). Posting that comment submits
  all answers, ticked boxes and answer fields included. An answer in the
  comment overrides the checkboxes or the answer field of that question.

### D4: GitHub output uses only GitHub's markdown and HTML subset
**Enforcement:** automated

Comments use GitHub-flavoured markdown and these HTML elements only:
`details`, `summary`, `sub`, `sup`, `kbd`, `br`, `b`, `i`, `code`. Alerts
introduce the comment and the results. Instructions and explanations are
collapsed in `<details>`. A test fails if rendered output contains any
other tag. The only fenced code blocks are a question's code snippets and
code options, rendered as ADR-0007 D5 says; fenced code in model prose
still becomes inline `<code>`.

### D5: Results are a new comment, and the questions are frozen
**Enforcement:** automated

When a session is graded, the results are posted as a new comment, which
notifies the author. The question comment is then edited to say it was
graded, with the questions collapsed, so that its checkboxes can no longer
be ticked and its answer fields no longer filled in (D11).

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
Their questions are not lost: an ungraded round's questions are asked
again, first, in the new comment (ADR-0002 D9). Graded comments stay as
they are.

A round whose answers were submitted (D3) before the push is not
superseded: the check grades it first and posts its results, so its
comment is frozen as graded (D5), not as superseded (ADR-0002 D9). Its
answers are read as on its own head: only the author's edits and comments
count, and a newer answer comment replaces an earlier one.

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

### D8: Answer commands are forgiving, and answer comments can be edited
**Enforcement:** automated

Issue #17 showed a mistyped `/socrates answer` ignored without a word, and a
fix by editing counted only by luck. So:

- The answer command is `/soqrates answer` or `/socrates answer`, in any
  letter case, or a mention of the app Soqrates posts as, `@<app slug>
  answer` (ADR-0006 D4). One function in `soqrates-core` decides what a
  command is, and the workflow filters list the same prefixes. The syntax is
  meant to be the same on every forge, so it stays in the core and is not
  left to each `ScmProvider` adapter.
- A bare `@soqrates` is never a command, even for an app with that slug: a
  person of that name would be notified of every answer, while an app is
  not notified when mentioned.
- A comment by the PR author that starts like a command but isn't one, such
  as `/socrates answr`, gets a short hint: "Did you mean `/soqrates
  answer`?". Once per comment, and not again for an identical comment.
- Until the session is graded, editing the answer comment counts: the
  workflow also runs when the PR author edits a command comment, and the
  edited text is graded. After grading, an edit changes nothing (D5).
- The results comment says what it graded: the answer comment, with when it
  was posted or last edited, and the questions whose ticked boxes, or
  answer fields (D11), were read.

### D9: Answers are acknowledged before grading
**Enforcement:** automated

Grading can take minutes. When a run picks up answers, the comment that
submitted them (the answer comment, or the question comment when "Grade my
answers" was ticked) gets a 👀 reaction, and the check reads "Grading your
answers…". Once the results are posted, the 👀 is replaced by 👍 (GitHub has
no ✅ reaction). Reactions only show progress: a failed one never stops
grading.

### D10: The examiner marks the correct option on the option itself
**Enforcement:** automated

In the examiner's reply, each multiple-choice option says whether it is
correct (`correct: true` or `false`), next to its text and before its
reason, and exactly one option is marked; a reply that marks none, or more
than one, is invalid output and is asked for again. The reply has no index
of the correct option.

An index beside the options was easy to get wrong: in a live run (PR #32)
the reasons said one option was correct and another wrong, but the index
pointed at the wrong one, so an author who answered correctly was graded
wrong. A mark on the option keeps the verdict next to the text and the
reason it must agree with, and there is nothing to count. The stored
question still records the correct option by position, after the shuffle;
that position is derived from the mark.

A separate check that the marked option really is correct, by having a
model answer the question independently, is not added: that run's content
was right, and a second model's answer can be wrong too. It stays an option
if evals show the examiner marking wrong answers.

### D11: Open questions are answered inside the question comment
**Enforcement:** automated

Issue #17 showed that answering in two places is easy to get wrong: multiple
choice in the question comment, open questions in a reply, and either could
submit. So each open question gets an answer field in the question comment,
and the author answers everything there, then ticks "Grade my answers":

- A field is the lines between two hidden markers under the question,
  `<!-- soqrates:answer=N -->` and `<!-- soqrates:answer-end=N -->`, where
  N is the question's number. The author edits the comment and types the
  answer between them; the rendered comment shows it under the question.
  A short line above the field says how.
- A field counts only when both markers are there, outside code, and
  belong to an open question; the first field of a question counts. A blank
  field is unanswered.
- Like boxes (D3), fields count only when the comment's last editor, as
  GraphQL reports it, is the PR author, and only once the answers are
  submitted: by ticking "Grade my answers", or by posting a `/soqrates
  answer` comment.
- Everything inside a field is the author's text, whatever it says. Boxes,
  headings, markers, `<details>` and fences typed there are not read as
  structure, so a field cannot tick a box, hide the rest of the comment or
  forge a question.
- `/soqrates answer` stays for authors who can't edit the comment. When a
  question has both, the answer comment wins, as it does over boxes: it is
  the newer, explicit answer, and it is the only one that can also be edited
  by an author without write access. The results comment says which source
  it graded (D8): the answer comment, the boxes and the fields it read, and
  the questions whose boxes or field the answer comment replaced.
- When the comment is re-rendered, the author's text is never copied back as
  markdown. Once graded (D5), each open answer is shown escaped like any
  other text (D4). Once superseded (D7), the fields are gone, and the text
  left in each is shown escaped, so the author can reuse it when the
  question is asked again.

Typing into a field edits the question comment, like ticking a box, so the
workflow filter (D8) needs no change: the comment's edits start a run only
once "Grade my answers" is ticked. A field without that box ticked starts no
run, and its text is only read once the author submits.

## Consequences

- The examiner's output grows: a reason per multiple-choice option, and
  feedback per graded open answer.
- Each multiple-choice question costs a second, short model call for the
  option check, and two more when it fails.
- `Examiner::grade` returns a grade with feedback, and `ScmProvider`
  gains `post_results`, `freeze_superseded` and `acknowledge_answers`, and
  `collect_answers` also hints at mistyped commands (D7, D8, D9). A
  submission's receipt is passed from `collect_answers` to
  `acknowledge_answers` and `post_results`, so the provider keeps no state.
- The dogfooding workflow also runs on `issue_comment.edited` for comments
  that carry a Soqrates marker, and for the PR author's own command
  comments (D8).
- Authors without write access answer every question by comment.
- A collaborator could tick boxes or type into an answer field after the
  author's last edit. Those edits only count if the author edits the comment
  again afterwards, and so sees them. This is acceptable for an advisory
  learning check.
- An answer typed into a field but not submitted before a push is not
  graded; the superseded comment keeps it, as text, for the next round
  (D7, D11).
