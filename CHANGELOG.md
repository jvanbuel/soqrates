# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
