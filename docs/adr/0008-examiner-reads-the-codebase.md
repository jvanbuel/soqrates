# ADR-0008: The examiner reads the codebase

- **Status:** Accepted
- **Date:** 2026-11-08
- **Refines:** ADR-0001 D3 (examiner), D8 (CI round trip), D9 (local hooks),
  D10 (MCP); ADR-0007 D4 (what the writer is given)
- **Amends:** ADR-0006 D1 (the action checks out the pull request, to read)

## Context

Until now each examiner step saw only the change itself. Concept
extraction saw the diff. The writer saw the concept and, since ADR-0007,
the hunks of the diff where the concept was found. The grader saw the
question, its snippet, its rubric and the answer. A diff hides a lot: a
one-line change to a call means little without the function it calls, a
due review has no hunks at all, and the grader couldn't check what an
answer said about code outside the snippet.

The examiner now reads the code itself. It doesn't get a fixed bundle of
files: it gets read-only tools and decides what to open, within a budget.
The rest of Soqrates is unchanged and stays deterministic: the core still
decides which concepts are asked, how many, in which format and kinds, and
what a grade does to the schedule (ADR-0002, ADR-0007 D2). Agency stays
inside the examiner, behind the `Examiner` port.

## Glossary additions

This ADR adds the term Codebase (`Codebase`) to the
[ADR-0002 glossary](0002-domain-model.md#glossary).

## Decisions

### D1: A read-only `Codebase` port, at the change's revision
**Enforcement:** automated

The core defines a `Codebase` port over one revision of the repository,
the code the change produces, with five operations: read a file, list
every file, grep with an extended regular expression (optionally within a
glob), the history of a file, and blame for a range of its lines. A
`Codebase` reads only what version control holds at that revision. It
never reads the working tree, untracked or ignored files, or anything
outside the repository, so a local `.env` never reaches the model.

Paths are relative to the repository root, `/`-separated, and never
absolute or containing `..` (`code_path` checks them); globs are checked
the same way and can't carry git's pathspec magic (`code_glob`).

`NoCodebase` is the codebase of a surface that has none: it holds nothing.
In CI, a `CodeSource` gives the codebase at a commit (D4).

### D2: Every examiner step is a Rig agent with a toolset over the codebase
**Enforcement:** automated

Concept extraction, question writing and grading of open answers each run
as an agent on Rig's agent runtime (`rig-agent`, confined to
`soqrates-examiner` like `rig-core`, ADR-0001 D3). Its tools come from one
registry, a table in `prompts::tools` turned into Rig tools:

| Tool | What it returns |
|---|---|
| `read_file` | a file, or a range of its lines |
| `list_dir` | the entries of a directory |
| `glob` | the files whose path matches a pattern |
| `grep` | the lines matching a regular expression, optionally within a glob |
| `git_log` | the latest commits that changed a file, or the repository |
| `git_blame` | for a range of lines, the commit that last changed each |

Adding a tool is adding a row to the table and a case to `serve`. Rig runs
the loop: it sends the requests, dispatches the tool calls and ends the run
when the model answers. Rig also chooses how the structured answer is
asked for: as a native schema where the provider composes one with tools,
otherwise as an output tool of its own.

Soqrates keeps the policy, so that it stays testable without Rig:

- every tool answers through one function, `serve`, that checks its
  arguments, cuts long output, puts it in its block (D3) and counts the
  budget below;
- a hook takes the tools away for a model call after the reading turns and
  once the budget is spent, so the model then answers;
- an answer that fails validation is asked for again by continuing the
  agent's transcript with the tools away: a retry doesn't read again.

The tools hold no codebase: an agent and its tools live as long as Rig
needs, a codebase only for the step. They send each call to the step,
which answers it from the codebase while it drives the agent. Reading is
bounded:

- at most 8 tool turns and 20 tool calls per step, so a run makes at most
  9 model calls;
- at most 100 000 bytes of tool output per step; an output that doesn't
  fit what is left is refused, never cut;
- a file is cut at 20 000 bytes, and the reply says so and suggests a
  range; at most 200 paths per glob, 60 matches per grep, 15 commits per
  log and 150 lines per blame; long lines are cut.

Past a bound, a call is refused with a message saying so, and the model
answers from what it has read. Grading multiple choice still uses no model
at all (ADR-0004 D1), and the check that a multiple-choice question
discusses only its options reads nothing.

### D3: Tool output is data, not instructions
**Enforcement:** automated

Tool output is put in a delimited block (`<file>`, `<listing>`, `<files>`,
`<matches>`, `<history>`, `<blame>`) that its content can't close, and
every system prompt that offers tools tells the model to treat it as data,
like the diff; commit messages included. The tools only read. A prompt
injection in a file can mislead a question, but it can't write, post or
call anything.

Code read from other files is context, not a hunk of the change: the
snippet of an `ExplainYourChange` question is still copied from a hunk the
writer was given (ADR-0007 D4), and code from the repository stays with the
repository (ADR-0007 D6).

### D4: Each surface reads its revision from git
**Enforcement:** automated

Every surface reads git objects, with git itself (`cat-file`, `ls-files`
or `ls-tree`, `grep`, `log`, `blame`):

- **CI:** the action checks out the pull request's head
  (`refs/pull/<n>/head`, `code-history` commits deep) into `.soqrates/code`,
  without credentials, and passes it as `soqrates ci --code-dir`. This
  amends ADR-0006 D1: the action checks out the code, but only Soqrates
  reads it, and nothing from it is built or run. A session is read at the
  commit it was asked for, even after a newer push; a commit the checkout
  doesn't have, e.g. after a force-push, is read as no codebase (D5). If
  the checkout fails, or `read-code` is false, the check runs on the diff
  alone.
- **git `pre-commit`:** the index, i.e. what is being committed. History
  and blame start from `HEAD`; staged lines blame as not committed yet.
- **jj `pre-push`:** the commit `@-`, as in the diff, in jj's git store.
- **MCP:** no codebase. The harness has the code; Soqrates sees the diff
  it is given.

### D5: Without a codebase, nothing changes
**Enforcement:** automated

With `NoCodebase`, the examiner runs no agent: it makes the same single
request per step as before this ADR, with no tools.

## Consequences

- Questions can be about how the change fits the code around it, and due
  reviews, which have no hunks, can be tied to code in the current change.
- Each step costs more calls and tokens. The budgets in D2 cap the cost,
  and the most a step can take is fixed.
- More of the repository goes to the model provider than the diff alone:
  what the model chooses to read, at the change's revision, commit
  messages and author names included. That provider already sees the diff,
  so the trust boundary is the same, but the amount is larger.
- The CI job checks out pull request code, also on comment-triggered runs,
  which have the repository's secrets. It is safe only as long as nothing
  runs it: the architecture tests check that only Soqrates is pointed at
  the checkout, and that it keeps no credentials.
- Questions vary more from run to run. Evals (ADR-0001 D13) should compare
  runs with and without the codebase.
