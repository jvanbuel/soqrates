@adr-0001-d8 @adr-0001-d4 @adr-0001-d12
Feature: The CI check is an asynchronous round trip
  In CI the questions cannot be answered at the terminal. The first run on a
  PR head creates a session, posts the questions as a PR comment and sets the
  check to Pending. A later run reads the answers, grades them, updates the
  profile and sets the check to Success or Failure (Neutral in Advisory mode).
  A new push starts a new session for the new head. All forge interaction goes
  through the ScmProvider port, played here by a hermetic fake forge, with a
  scripted examiner and a fake clock.

  Background:
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"

  @adr-0001-d8 @adr-0001-d4
  Scenario: The first run posts the questions and leaves the check pending
    When the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And the posted questions are about "rust.ownership", "rust.traits"
    And the latest posted session is for the current head
    And a pending session is stored for the current head

  @adr-0001-d8 @adr-0001-d4
  Scenario: A run before the author replies keeps the check pending and does not post again
    Given the Soqrates check has run
    When the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time
    And a pending session is stored for the current head

  @adr-0001-d8 @adr-0001-d4
  Scenario: A run after the author replies grades the answers and updates the profile
    Given the Soqrates check has run
    And the author replies with correct answers
    When the Soqrates check runs
    Then the check is Success
    And "github:1001" has review cards for exactly "rust.ownership", "rust.traits"
    And the history of "github:1001" has 2 graded answers
    And no pending session is stored for the current head

  @adr-0001-d8 @adr-0001-d4
  Scenario Outline: The final status depends on the answers and the mode
    Given Soqrates runs in <mode> mode
    And the Soqrates check has run
    And the author replies with <answers> answers
    When the Soqrates check runs
    Then the check is <status>

    Examples:
      | mode     | answers | status  |
      | Blocking | correct | Success |
      | Blocking | wrong   | Failure |
      | Advisory | correct | Success |
      | Advisory | wrong   | Neutral |

  @adr-0001-d8 @adr-0001-d4
  Scenario Outline: The pass threshold decides between passing and failing
    Given Soqrates runs in Blocking mode
    And the pass threshold is 0.6
    And a pull request "jvanbuel/soqrates#8" by "github:1001" at head "fff000" touching "rust.ownership", "rust.traits", "tokio.select"
    And the Soqrates check has run
    And the author replies with answers:
      | concept        | answer   |
      | rust.ownership | <first>  |
      | rust.traits    | <second> |
      | tokio.select   | <third>  |
    When the Soqrates check runs
    Then the check is <status>

    Examples:
      | first   | second  | third   | status  |
      | correct | correct | correct | Success |
      | correct | correct | wrong   | Success |
      | correct | wrong   | wrong   | Failure |
      | wrong   | wrong   | wrong   | Failure |

  @adr-0001-d8 @adr-0001-d4
  Scenario: Advisory is the default mode for the check
    Given the default configuration
    And the Soqrates check has run
    And the author replies with wrong answers
    When the Soqrates check runs
    Then the check is Neutral

  @adr-0001-d8 @adr-0001-d4
  Scenario: A new push starts a new session for the new head
    Given the Soqrates check has run
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the questions were posted 2 times
    And the latest posted session is for the current head
    And the posted questions are about "tokio.select"
    And a pending session is stored for the current head

  @adr-0001-d8 @adr-0001-d4
  Scenario: Answers to an earlier head do not complete the session for a new head
    Given the Soqrates check has run
    And the author replies with correct answers
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending

  @adr-0001-d8
  Scenario: The session key is tied to the pull request and its head commit
    Then the session key is the same each time it is derived for the current head
    And the session key for head "abc123" differs from the session key for head "def456"
    And the session key for pull request "jvanbuel/soqrates#7" differs from the one for "jvanbuel/soqrates#9" at the same head

  @adr-0001-d8 @adr-0001-d4
  Scenario: Pull requests by different authors are graded independently
    Given Soqrates runs in Blocking mode
    And the Soqrates check has run
    And the author replies with correct answers
    And the Soqrates check has run
    And a pull request "jvanbuel/soqrates#9" by "github:2002" at head "999aaa" touching "rust.ownership"
    And the Soqrates check has run
    And the author replies with wrong answers
    When the Soqrates check runs
    Then the check is Failure
    And the history of "github:1001" has 2 graded answers
    And the history of "github:2002" has 1 graded answer

  @adr-0001-d8 @adr-0002-d6
  Scenario: Running the check again after grading reports the result without asking again
    Given the Soqrates check has run
    And the author replies with correct answers
    And the Soqrates check has run
    When the Soqrates check runs
    Then the check is Success
    And the questions were posted 1 time
    And the history of "github:1001" has 2 graded answers

  @adr-0001-d8
  Scenario: When the questions cannot be posted, the next run asks again
    Given posting the questions fails once
    When the Soqrates check runs and fails
    Then no pending session is stored for the current head
    When the Soqrates check runs
    Then the check is Pending
    And the questions were posted 1 time

  @adr-0001-d8 @adr-0002-d6
  Scenario: When saving the reviews fails, the answers are graded exactly once on the next run
    Given the Soqrates check has run
    And the author replies with correct answers
    And saving the learner profile fails once
    When the Soqrates check runs and fails
    Then a pending session is stored for the current head
    When the Soqrates check runs
    Then the check is Success
    And the questions were posted 1 time
    And the history of "github:1001" has 2 graded answers

  @adr-0002-d6
  Scenario: Two simultaneous submissions grade the session once
    Given the Soqrates check has run
    When the author's correct answers are submitted twice at the same time
    Then the history of "github:1001" has 2 graded answers
    And no pending session is stored for the current head

  @adr-0004-d7 @adr-0001-d8
  Scenario: A new push freezes the unanswered question comment of the earlier head
    Given the Soqrates check has run
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And the question comment for head "abc123" was marked superseded 1 time
    And it points to the question comment for the current head and shows its own questions
    And no pending session is stored for head "abc123"
    And a pending session is stored for the current head

  @adr-0004-d7 @adr-0001-d8
  Scenario: Answering a superseded question comment has no effect
    Given the Soqrates check has run
    And the author pushes head "def456" touching "tokio.select"
    And the Soqrates check has run
    When the author answers the questions for head "abc123" correctly
    And the Soqrates check runs
    Then the check is Pending
    And the history of "github:1001" has 0 graded answers
    And the question comment for head "abc123" was marked superseded 1 time

  @adr-0004-d7 @adr-0004-d5
  Scenario: A graded question comment is not superseded by a new push
    Given the Soqrates check has run
    And the author replies with correct answers
    And the Soqrates check has run
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the check is Pending
    And no question comment was marked superseded

  @adr-0004-d7
  Scenario: When freezing the earlier question comment fails, the next run freezes it
    Given the Soqrates check has run
    And the author pushes head "def456" touching "tokio.select"
    And freezing superseded question comments fails once
    When the Soqrates check runs and fails
    Then a pending session is stored for the current head
    And a pending session is stored for head "abc123"
    When the Soqrates check runs
    Then the check is Pending
    And the questions were posted 2 times
    And the question comment for head "abc123" was marked superseded 1 time
    And no pending session is stored for head "abc123"
