# API gaps found while writing the acceptance tests

The BDD scenarios in `features/` test only the public API. These are the places
where the API, or the ADR, does not let a scenario say exactly what ADR-0001
asks for. The public API was not changed. Each gap says which scenario is
affected, what is missing, and a proposed fix.

## G1: MCP tools cannot be called in-process (D10), blocking

> **Resolved**: `soqrates_mcp::SoqratesMcp::new(Arc<Soqrates<S, E, C>>)` and
> `SoqratesMcp::call_tool(&self, name, args: serde_json::Value) ->
> Result<serde_json::Value>` dispatch a tool call in-process, exactly as the
> rmcp handler behind `SoqratesMcp::serve_stdio` does. The scenario "An MCP
> client gets the same learner profile as the engine" now runs (no longer
> `@wip @blocked`): `SpecWorld::call_mcp` lends the world's engine to a
> `SoqratesMcp` for the call and takes it back afterwards.

- **Scenario:** `mcp_server.feature`, "An MCP client gets the same learner
  profile as the engine".
- **Was missing:** `soqrates-mcp` only offered `tool_names()` and a free
  `serve_stdio()` that built its own engine and talked over real stdio, so a
  test could not hand it a `Soqrates` with fakes (D12) or call a tool and
  read the result.
- **Differences from the proposal:** `new` takes the engine in an `Arc`
  (the CLI and the rmcp handler share it), and there is no generic
  `serve(server, transport)`; the public transport is `serve_stdio(self)`.

## G2: "Sustained" is not quantified (D7), spec ambiguity

> **Resolved** by ADR-0002 D4 (6 correct promote, 3 incorrect demote).

- **Scenarios:** `adaptive_questions.feature`, "The level only moves after
  sustained answers", "A novice who keeps answering correctly is promoted...",
  "An expert who keeps answering wrongly is demoted...".
- **Missing:** the ADR does not say how many answers count as sustained, and
  `next_level` takes no thresholds. The scenarios assume that one answer never
  moves the level, and that 20 answers in a row (or 7 sessions of 3) of the
  same kind do. Moving one or two levels both pass ("higher than Novice").
- **Proposed:** say it in the ADR, or add `Config::promotion_streak` /
  `Config::demotion_streak` and
  `next_level(current, history, &LevelRules) -> Level`, so scenarios can name
  the threshold.

## G3: "A mix" for intermediates is not defined (D7), spec ambiguity

> **Resolved** by ADR-0002 D5 (multiple choice on even, open on odd indices).

- **Scenarios:** "An intermediate gets a mix of question formats", "The
  format policy per level".
- **Assumed:** with 4 questions an intermediate gets at least one
  multiple-choice and at least one open question. "Session questions follow the
  format policy" also requires the i-th session question to have the format
  `policy::format_for(level, i)`.

## G4: Tie order of due reviews is unspecified (D6)

> **Resolved** by ADR-0002 D8 (ties broken by concept ID, ascending).

- **Scenario:** "Due reviews can use up the whole budget".
- **Assumed:** cards that fall due at the same instant may come in any order,
  so this scenario checks the set and the count, not the order. "Most overdue
  first" is checked with cards that have different due times (a wrong answer
  falls due before a correct one).

## G5: FSRS state is opaque (D6)

> **Resolved**: `ReviewCard::reps()` and `stability()` are exposed.

- **Scenarios:** all of `spaced_repetition.feature`.
- **Missing:** `ReviewCard` wraps `rs_fsrs::Card` privately and only exposes
  `due()`/`is_due()`. That is enough for the ADR's behaviour ("a correct answer
  pushes the next review further out"), but a test cannot check that the card
  is really scheduled by FSRS (stability, difficulty, state).
- **Proposed (optional):** `ReviewCard::stability() -> f64` and
  `ReviewCard::reps() -> u32`, if we ever want to pin this down.

## G6: Hook behaviour that is not specified (D9)

- **Assumed, not asserted:** whether `Prompter::report` is called for an empty
  diff, and what `SessionKey` a hook run uses. The empty-diff scenarios only
  check that nothing is asked, the hook allows, and the profile is not
  touched.

## G7: Unknown session on submit (D5/D8)

> **Resolved** by ADR-0002 D6 (`NotFound`).

- Not covered: the ADR does not say what `Soqrates::submit` does for a
  `SessionKey` that has no pending session (presumably `Error::NotFound`).
  Worth adding to the ADR, then a scenario.

# ADR-0002

No ADR-0002 rule is blocked: every scenario in `domain_learner_profile.feature`
and `domain_session.feature` can be written against the public API, so none
is tagged `@blocked`. What the scenarios assume but the ADR leaves open:

## G8: Order of events is not fully specified (D7), spec ambiguity

- **Scenarios:** "Recording a review emits ReviewRecorded and
  ReviewScheduled", "The review that promotes the author also emits
  LevelChanged", "The session outcome carries the events of every review and
  one SessionGraded".
- **Assumed, not asserted:** the order of the events returned by one
  `LearnerProfile::record`, and the position of `SessionGraded` in
  `SessionOutcome::events` ("in order" is not defined). The scenarios
  compare the events as a set and count them per kind.
- **Proposed:** say in the ADR that `record` returns `ReviewRecorded`,
  `ReviewScheduled`, then `LevelChanged`, and that `SessionGraded` comes last
  in the outcome; then the scenarios can assert the order.

## G9: Does the review that changes the level count toward the new streak? (D4)

> **Resolved** as assumed: it does not; implemented in `LearnerProfile::record`.

- **Scenario:** "A level change resets the streak".
- **Assumed:** right after the review that changes the level,
  `reviews_since_level_change()` is empty (that review does not count toward
  the next streak), so after 6 correct reviews a Novice is Intermediate with
  0 reviews since the change, and 6 more are needed to reach Expert.

## Not a scenario: "graded at most once" on the aggregate (D3, D6)

`Session::conclude(self, ...)` consumes the session, so concluding it twice
does not compile; that is a type-level guarantee, not something a scenario
can exercise. The engine-level rule (a second `Soqrates::submit` fails with
`NotFound`) is covered by "A session is graded at most once".
