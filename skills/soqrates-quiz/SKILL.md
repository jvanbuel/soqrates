---
name: soqrates-quiz
description: Quiz the author of a code change about the concepts in it, adapting the question format to their level and scheduling reviews with spaced repetition. Use before committing or pushing, or when asked to "quiz me on this change".
---

# Soqrates quiz

Use the `soqrates` MCP server's tools:

1. `start_session` with the author's identity and the diff of the change.
   The identity is `email:<address>` (e.g. from `git config user.email`)
   or `<forge host>:<account id>` (e.g. `github.com:1001`). Send the staged
   diff before a commit, and the diff against the upstream branch before a
   push.
2. Ask the questions one at a time. Present multiple-choice questions with
   their options, and open questions as is. Do not reveal answers before the
   author has responded. Never answer on the author's behalf.
3. `submit_answers` with the author's answers, then show the outcome and
   briefly explain any question they got wrong.

Use `get_learner_profile` and `due_reviews` to tell the author what they are
due to revisit.
