@adr-0002-d6
Feature: Runs that write the same record at the same moment lose nothing
  Several runs of Soqrates can work for one learner or one pull request at
  once: two pushes, two pull requests, a hook next to the CI check. Records
  that more than one of them writes are changed with an update that builds
  on what is stored when it saves, and a session is opened only if no run
  opened it first, so no run's work overwrites another's. Here the store
  holds back the first of two writes of one record until the second
  arrives, so both runs read it before either saves.

  @adr-0002-d6 @adr-0009-d5
  Scenario: Two rounds composed at the same moment get their own numbers and keep what each showed
    When "github:1001" starts sessions for changes touching "rust.ownership" and "rust.traits" at the same moment
    Then what "github:1001" was shown holds the bank questions of both sessions, from different rounds

  @adr-0002-d6 @adr-0002-d9
  Scenario: Rounds of two pushes recorded at the same moment are both kept
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership"
    When rounds for heads "abc123" and "def456" of the pull request are recorded at the same moment
    Then the pull request has rounds for heads "abc123", "def456"

  @adr-0002-d6 @adr-0001-d8
  Scenario: Two check runs for one head at the same moment post the questions once
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"
    When the Soqrates check runs twice at the same moment
    Then the check is Pending
    And the questions were posted 1 time
    And the pending session stored for the current head is the one posted

  @adr-0002-d6
  Scenario: A session that was graded meanwhile is not opened again
    Given "github:1001" has answered questions about "rust.ownership" correctly
    Then opening session 1 of "github:1001" again opens nothing, and it stays graded
