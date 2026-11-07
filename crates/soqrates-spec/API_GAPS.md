# API gaps found while writing the acceptance tests

The BDD scenarios in `features/` test only the public API. These are the places
where the API, or the ADR, does not let a scenario say exactly what ADR-0001
asks for. The public API was not changed. Each gap says which scenario is
affected, what is missing, and a proposed fix.

## G1: MCP tools cannot be called in-process (D10), blocking

- **Scenario:** `mcp_server.feature`, "An MCP client gets the same learner
  model as the engine" (tagged `@blocked`).
- **Missing:** `soqrates-mcp` only offers `tool_names()` and `serve_stdio()`.
  `serve_stdio` builds its own engine and talks over real stdio, so a test
  cannot hand it a `Soqrates` with fakes (D12) or call a tool and read the
  result. The tests can only check the tool names, not that "other harnesses
  get the same learner model and scheduling".
- **Proposed signature:**

  ```rust
  pub struct SoqratesMcp<S, E, C> { /* wraps Soqrates<S, E, C> */ }
  impl<S: StateStore, E: Examiner, C: Clock> SoqratesMcp<S, E, C> {
      pub fn new(app: Soqrates<S, E, C>) -> Self;
      /// Dispatch one tool call, as the rmcp handler would.
      pub async fn call_tool(&self, name: &str, args: serde_json::Value)
          -> soqrates_core::Result<serde_json::Value>;
  }
  pub async fn serve<S, E, C, T>(server: SoqratesMcp<S, E, C>, transport: T) -> Result<()>;
  ```

  The spec crate would also need `serde_json` as a dev-dependency to build the
  arguments.
- **Current steps:** the `When`/`Then` steps exist but panic with
  "blocked by API gap G1".

## G2: "Sustained" is not quantified (D7), spec ambiguity

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

- **Scenarios:** "An intermediate gets a mix of question formats", "The
  format policy per level".
- **Assumed:** with 4 questions an intermediate gets at least one
  multiple-choice and at least one open question. "Session questions follow the
  format policy" also requires the i-th session question to have the format
  `policy::format_for(level, i)`.

## G4: Tie order of due reviews is unspecified (D6)

- **Scenario:** "Due reviews can use up the whole budget".
- **Assumed:** cards that fall due at the same instant may come in any order,
  so this scenario checks the set and the count, not the order. "Most overdue
  first" is checked with cards that have different due times (a wrong answer
  falls due before a correct one).

## G5: FSRS state is opaque (D6)

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

- Not covered: the ADR does not say what `Soqrates::submit` does for a
  `SessionKey` that has no pending session (presumably `Error::NotFound`).
  Worth adding to the ADR, then a scenario.
