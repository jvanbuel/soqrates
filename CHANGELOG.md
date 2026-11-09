# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2](https://github.com/jvanbuel/soqrates/compare/v0.1.1...v0.1.2) - 2026-11-09

### Other

- Ask multiple choice only by default; never alternate formats
- Report git grep errors; stream its output up to the cap
- Give the examiner agent a full toolset over git: grep, glob, ranges, log, blame
- Let the examiner read the codebase through bounded tools (ADR-0008)
- Mark the correct multiple-choice option on the option, not by index
- Run the examiner's reading on Rig's agent runtime (ADR-0008 D2)

## [0.1.1](https://github.com/jvanbuel/soqrates/compare/v0.1.0...v0.1.1) - 2026-11-08

### Added

- make answering on GitHub forgiving and visible

### Other

- Add question kinds and code snippets from the change (ADR-0007)
- Merge main into claude/issue-15-code-snippets
- Merge origin/main into the question-kinds branch
- Merge remote-tracking branch 'origin/main' into worktree-agent-af941fdf709bdd6bb
- Merge main into claude/issue-6-grading-journal
- Merge main into claude/issue-23-grade-before-push
- Make grading crash-safe with a write-ahead grading journal
- Grade submitted answers before a push starts the next round
- Exempt only carried questions from the session cap
- Size each round to the change and the author's knowledge
- Separate the open-question sentence in ADR-0004 D1; tidy test imports
- Give every multiple-choice option its own reason
- Ask only about what changed since the last round ([#4](https://github.com/jvanbuel/soqrates/pull/4))
- Keep patchless files in incremental diffs; clear rounds in SQL tests
- Keep numeric constraints out of nullable integers in output schemas
- Check that multiple-choice reasons discuss only the options offered
- Drop the model check of option reasons
- Catch explanations that refer to options the question doesn't offer
- Add @wip scenarios for question kinds and code snippets (ADR-0007)
