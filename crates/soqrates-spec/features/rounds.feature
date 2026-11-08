@adr-0002-d9 @adr-0001-d8 @adr-0001-d12
Feature: After a push, ask about what changed and what is still unanswered
  Each push to a pull request starts a round. A later round asks again,
  first and as written, whatever the latest round left unanswered. Then it
  asks about the change since the latest round, not the whole pull request,
  leaving out concepts already asked on the pull request unless their
  review is due. With nothing new to ask, the latest round still stands:
  its questions stay open, or its result is reported again. After a
  force-push the whole pull request is asked about, still without the
  concepts already asked. Answers the author submitted before the push
  are graded first, so their questions are not asked again.

  Background:
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"
    And Soqrates runs in Blocking mode
    And the Soqrates check has run

  Scenario: A push with a new concept asks the unanswered questions again, then about the new concept
    When the author pushes head "def456" touching "rust.ownership", "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the change was compared from head "abc123" to head "def456"
    And the questions were posted 2 times
    And the latest posted session is for the current head
    And the posted questions are about "rust.ownership", "rust.traits", "tokio.select"
    And the latest posted session starts with the questions posted for head "abc123"
    And the question comment for head "abc123" was marked superseded 1 time

  Scenario: A push after the round was graded asks only about the new concept
    Given the author replies with wrong answers
    And the Soqrates check has run
    When the author pushes head "def456" touching "rust.ownership", "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the posted questions are about "tokio.select"

  Scenario: Questions carried over two pushes are each asked once
    Given the author pushes head "def456" touching "tokio.select"
    And the Soqrates check has run
    When the author pushes head "ghi789" touching "rust.traits", "tokio.select", "serde.derive"
    And the Soqrates check runs
    Then the change was compared from head "def456" to head "ghi789"
    And the posted questions are about "rust.ownership", "rust.traits", "tokio.select", "serde.derive"
    And the latest posted session starts with the questions posted for head "def456"

  @adr-0002-d10
  Scenario: Carried questions that fill the budget keep the earlier questions open
    Given the question budget is 2
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And no question comment was marked superseded

  @adr-0002-d10
  Scenario: A concept left out because carried questions filled the budget is asked on a later round
    Given the question budget is 2
    And the author pushes head "def456" touching "tokio.select"
    And the Soqrates check has run
    And the author replies with correct answers
    And the Soqrates check has run
    When the author pushes head "ghi789" touching "rust.ownership"
    And the Soqrates check runs
    Then the change was compared from head "abc123" to head "ghi789"
    And the posted questions are about "tokio.select"

  Scenario: A push with nothing new keeps the earlier questions open
    When the author pushes head "fix001" touching "rust.ownership"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And no question comment was marked superseded
    And a pending session is stored for head "abc123"

  Scenario: A push with nothing to ask at all keeps the earlier questions open
    When the author pushes head "fix001" touching nothing
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And no question comment was marked superseded

  Scenario: Answering the earlier questions after a push with nothing new completes the check
    Given the author pushes head "fix001" touching "rust.traits"
    And the Soqrates check has run
    And the author replies with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the questions were posted 1 time
    And the history of "github:1001" has 2 graded answers

  Scenario: A push with nothing new reports the latest graded result again
    Given the author replies with wrong answers
    And the Soqrates check has run
    When the author pushes head "fix001" touching "rust.ownership"
    And the Soqrates check runs
    Then the check is Failure
    And the questions were posted 1 time
    And the history of "github:1001" has 2 graded answers

  Scenario: A concept asked earlier on the pull request is asked again once its review is due
    Given the author replies with correct answers
    And the Soqrates check has run
    And 60 days pass
    When the author pushes head "def456" touching "rust.ownership"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 2 times
    And the posted questions are about "rust.ownership", "rust.traits"

  Scenario: After a force-push the whole pull request is asked about, without the concepts already asked
    When the author force-pushes head "zzz999" touching "rust.traits", "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the change was compared from head "abc123" to head "zzz999"
    And the posted questions are about "rust.ownership", "rust.traits", "tokio.select"
    And the question comment for head "abc123" was marked superseded 1 time

  Scenario: Re-running the check on a head with nothing new changes nothing
    Given the author pushes head "fix001" touching nothing
    And the Soqrates check has run
    When the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And the change was compared 1 time

  @adr-0004-d3 @adr-0004-d7
  Scenario: Answers submitted before a push are graded before the next round is asked
    Given the author replies with correct answers
    When the author pushes head "def456" touching "rust.ownership", "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 1 time
    And the results posted are for head "abc123"
    And the history of "github:1001" has 2 graded answers
    And the questions were posted 2 times
    And the latest posted session is for the current head
    And the posted questions are about "tokio.select"
    And no question comment was marked superseded

  @adr-0004-d3 @adr-0004-d7
  Scenario: Grading the earlier round sets no check on the new head
    Given the author replies with wrong answers
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the check was never Failure
    And the results posted are for head "abc123"
    And the posted questions are about "tokio.select"

  @adr-0004-d3 @adr-0004-d7
  Scenario: Answers ticked but not submitted before a push are carried into the next round
    Given the author ticks correct answers without submitting them
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 0 times
    And the history of "github:1001" has 0 graded answers
    And the posted questions are about "rust.ownership", "rust.traits", "tokio.select"
    And the latest posted session starts with the questions posted for head "abc123"
    And the question comment for head "abc123" was marked superseded 1 time

  @adr-0004-d3 @adr-0004-d7 @adr-0002-d6
  Scenario: Re-running the check on the new head doesn't grade the earlier round again
    Given the author replies with correct answers
    And the author pushes head "def456" touching "tokio.select"
    And the Soqrates check has run
    When the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 1 time
    And the history of "github:1001" has 2 graded answers
    And the questions were posted 2 times
    And the posted questions are about "tokio.select"

  @adr-0004-d3 @adr-0004-d7
  Scenario: Answers submitted by someone other than the author don't grade the earlier round
    Given "github:2002" submits correct answers to the questions
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 0 times
    And the history of "github:1001" has 0 graded answers
    And the posted questions are about "rust.ownership", "rust.traits", "tokio.select"
    And the latest posted session starts with the questions posted for head "abc123"
