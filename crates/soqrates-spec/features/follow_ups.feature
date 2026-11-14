@adr-0010-d1 @adr-0010-d2 @adr-0001-d8 @adr-0001-d12
Feature: Ask a new question for every wrong answer
  After grading, each concept answered wrongly gets a new question about
  it, from another angle, in a follow-up round on the same commit. The
  check stays Pending while a follow-up round is open, and is set from the
  result after the final round, which counts each concept with its latest
  answer. Only the first answer is recorded as a review. A new push
  supersedes an open follow-up round like any question comment.

  Background:
    Given a pull request "jvanbuel/soqrates#9" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits", "tokio.select"
    And Soqrates runs in Blocking mode
    And Soqrates asks at most 2 follow-up rounds
    And the Soqrates check has run

  @adr-0010-d3 @adr-0010-d4
  Scenario: Two wrong answers out of three get a follow-up round with two new questions
    Given the author replies with answers:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.traits    | wrong   |
      | tokio.select   | wrong   |
    When the Soqrates check runs
    Then the check is Pending
    And the check says "follow-up question"
    And the results were posted on the pull request 1 time
    And the questions were posted 2 times
    And the latest posted session is follow-up round 2 of head "abc123"
    And the posted questions are about "rust.traits", "tokio.select"
    And the posted questions are new multiple-choice questions, written knowing the wrong answers they follow

  @adr-0010-d4 @adr-0010-d5
  Scenario: A follow-up answered correctly asks no further round and sets the check
    Given the author replies with answers:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.traits    | wrong   |
      | tokio.select   | wrong   |
    And the Soqrates check has run
    And the author replies with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the check says "3 of 3 answers correct: passed after a follow-up"
    And the results were posted on the pull request 2 times
    And the questions were posted 2 times
    And the history of "github:1001" has 3 graded answers
    And the latest review of "rust.traits" by "github:1001" is graded Again

  @adr-0010-d4
  Scenario: Running the check again after the follow-up reports its result
    Given the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with correct answers
    And the Soqrates check has run
    When the Soqrates check runs
    Then the check is Success
    And the check says "passed after a follow-up"
    And the questions were posted 2 times

  @adr-0010-d4
  Scenario: A follow-up that still leaves too few concepts right fails the check
    Given the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with answers:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.traits    | wrong   |
      | tokio.select   | wrong   |
    And the Soqrates check has run
    Then the check is Pending
    And the latest posted session is follow-up round 3 of head "abc123"
    And the posted questions are about "rust.traits", "tokio.select"
    When the author replies with wrong answers
    And the Soqrates check runs
    Then the check is Failure
    And the check says "1 of 3 answers correct: not passed"
    And the questions were posted 3 times
    And the history of "github:1001" has 3 graded answers

  Scenario: A follow-up answered wrongly at the round limit asks no further round
    Given Soqrates asks at most 1 follow-up round
    And the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with wrong answers
    When the Soqrates check runs
    Then the check is Failure
    And the check says "0 of 3 answers correct: not passed"
    And the questions were posted 2 times
    And the results were posted on the pull request 2 times

  Scenario: All answers correct asks no follow-up
    Given the author replies with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the check says "3 of 3 answers correct: passed."
    And the questions were posted 1 time

  Scenario: Without follow-up rounds the result stands at once
    Given Soqrates asks no follow-up rounds
    And the author replies with wrong answers
    When the Soqrates check runs
    Then the check is Failure
    And the questions were posted 1 time

  Scenario: Two follow-up rounds are asked by default
    Given the default configuration
    And the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with wrong answers
    When the Soqrates check runs
    Then the check is Neutral
    And the questions were posted 3 times

  @adr-0010-d6 @adr-0004-d7
  Scenario: A new push while a follow-up is open supersedes it
    Given the author replies with wrong answers
    And the Soqrates check has run
    When the author pushes head "def456" touching "serde.derive"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 3 times
    And the latest posted session is for the current head
    And the posted questions are about "serde.derive"
    And follow-up round 2 of head "abc123" was marked superseded 1 time
    And no pending session is stored for follow-up round 2 of head "abc123"

  @adr-0010-d6 @adr-0002-d9
  Scenario: A push with nothing new keeps the follow-up open
    Given the author replies with wrong answers
    And the Soqrates check has run
    When the author pushes head "fix001" touching "rust.ownership"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 2 times
    And no question comment was marked superseded
    When the author replies with correct answers
    And the Soqrates check runs
    Then the check is Success
    And the check says "passed after a follow-up"

  @adr-0010-d6 @adr-0004-d7
  Scenario: Follow-up answers submitted before a push are graded first, with no further round
    Given the author replies with wrong answers
    And the Soqrates check has run
    And the author replies with wrong answers
    When the author pushes head "def456" touching "serde.derive"
    And the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 2 times
    And the questions were posted 3 times
    And the posted questions are about "serde.derive"
    And no question comment was marked superseded
