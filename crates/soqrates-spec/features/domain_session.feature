@adr-0002-d3 @adr-0002-d6 @adr-0002-d7
Feature: The session aggregate
  A session holds the questions put to one author about one change. It is
  opened with at most the question budget and never asks twice about the
  same concept. Answers must refer to its questions, at most one per
  question; an unanswered question is graded Again. A session passes when
  the fraction of correct grades is at least the pass threshold, and an
  empty session passes. Concluding a session consumes it and emits
  SessionGraded, so it is graded at most once: the engine answers a second
  submission, like a submission to an unknown session, with NotFound.

  The aggregate scenarios use Session directly. Only the scenarios about the
  engine boundary go through Soqrates::submit.

  # -------------------------------------------------------------------------
  # D3 and D6: opening a session

  @adr-0002-d3 @adr-0002-d6
  Scenario: An opened session keeps its author and its questions in order
    Given multiple-choice questions about "rust.borrowing", "rust.traits", "tokio.select"
    When a session for "github:1001" is opened with at most 3 questions
    Then the session is opened
    And the session belongs to "github:1001" and asks about "rust.borrowing", "rust.traits", "tokio.select", in that order

  @adr-0002-d6
  Scenario Outline: A session holds at most the question budget
    Given multiple-choice questions about <concepts>
    When a session for "github:1001" is opened with at most <budget> questions
    Then <outcome>

    Examples:
      | concepts                                                 | budget | outcome                           |
      | "rust.borrowing", "rust.traits"                          | 3      | the session is opened             |
      | "rust.borrowing", "rust.traits", "tokio.select"          | 3      | the session is opened             |
      | "rust.borrowing", "rust.traits", "tokio.select", "serde" | 3      | opening the session fails as invalid |
      | "rust.borrowing", "rust.traits"                          | 1      | opening the session fails as invalid |

  @adr-0002-d6
  Scenario: A session holds no two questions about the same concept
    Given a multiple-choice and an open question about "rust.borrowing"
    When a session for "github:1001" is opened with at most 3 questions
    Then opening the session fails as invalid

  @adr-0002-d6
  Scenario: A session about no concepts can be opened
    Given no questions
    When a session for "github:1001" is opened with at most 3 questions
    Then the session is opened

  # -------------------------------------------------------------------------
  # D6: answers

  @adr-0002-d6
  Scenario Outline: Answers must refer to the session's questions, at most one per question
    Given a session for "github:1001" opened with questions about "rust.borrowing", "rust.traits", "tokio.select"
    When the answers to the questions about <answered> are validated
    Then the answers are <verdict>

    Examples:
      | answered                                        | verdict             |
      | "rust.borrowing", "rust.traits", "tokio.select" | accepted            |
      | "tokio.select", "rust.borrowing"                | accepted            |
      | "rust.traits"                                   | accepted            |
      | "rust.borrowing", "serde.derive"                | rejected as invalid |
      | "serde.derive"                                  | rejected as invalid |
      | "rust.borrowing", "rust.borrowing"              | rejected as invalid |
      | "rust.traits", "tokio.select", "rust.traits"    | rejected as invalid |

  @adr-0002-d6
  Scenario: An unanswered question is graded Again
    Given a session for "github:1001" opened with questions about "rust.borrowing", "rust.traits", "tokio.select"
    When the session is concluded with pass threshold 0.6 and grades:
      | concept        | grade |
      | rust.borrowing | Good  |
      | tokio.select   | Easy  |
    Then the session grades are, in question order:
      | concept        | grade |
      | rust.borrowing | Good  |
      | rust.traits    | Again |
      | tokio.select   | Easy  |
    And the session passes

  @adr-0002-d6
  Scenario: A session with no grades at all grades every question Again
    Given a session for "github:1001" opened with questions about "rust.borrowing", "rust.traits"
    When the session is concluded with pass threshold 0.5 and no grades
    Then the session grades are, in question order:
      | concept        | grade |
      | rust.borrowing | Again |
      | rust.traits    | Again |
    And the session fails

  # -------------------------------------------------------------------------
  # D6: passing

  @adr-0002-d6
  Scenario Outline: A session passes when the fraction of correct grades reaches the pass threshold
    Given a session for "github:1001" opened with questions about "a.one", "a.two", "a.three", "a.four", "a.five"
    When the session is concluded with pass threshold <threshold>, <correct> questions graded <grade> and the rest graded Again
    Then the session <result>

    Examples:
      | threshold | correct | grade | result |
      | 0.6       | 3       | Good  | passes |
      | 0.6       | 2       | Good  | fails  |
      | 0.6       | 3       | Hard  | passes |
      | 0.6       | 3       | Easy  | passes |
      | 0.8       | 4       | Good  | passes |
      | 0.8       | 3       | Good  | fails  |
      | 1.0       | 5       | Good  | passes |
      | 1.0       | 4       | Good  | fails  |
      | 0.0       | 0       | Good  | passes |

  @adr-0002-d6
  Scenario: Unanswered questions count against the pass threshold
    Given a session for "github:1001" opened with questions about "a.one", "a.two", "a.three", "a.four", "a.five"
    When the session is concluded with pass threshold 0.6 and grades:
      | concept | grade |
      | a.one   | Good  |
      | a.two   | Good  |
    Then the session fails

  @adr-0002-d6
  Scenario Outline: An empty session passes
    Given a session for "github:1001" opened with no questions
    When the session is concluded with pass threshold <threshold> and no grades
    Then the session passes

    Examples:
      | threshold |
      | 0.0       |
      | 0.6       |
      | 1.0       |

  # -------------------------------------------------------------------------
  # D6: the engine boundary

  @adr-0002-d6
  Scenario: A session is graded at most once
    Given "github:1001" starts a session for a change touching "rust.borrowing", "rust.traits"
    When "github:1001" answers every question correctly
    And "github:1001" submits the same answers to that session again
    Then the submission fails as not found

  @adr-0002-d6
  Scenario: Submitting answers to an unknown session fails
    When answers are submitted to the unknown session "local:github:1001:404"
    Then the submission fails as not found

  @adr-0002-d6
  Scenario: The engine rejects an answer to a question that is not in the session
    Given "github:1001" starts a session for a change touching "rust.borrowing", "rust.traits"
    When "github:1001" submits an answer to a question that is not in the session
    Then the submission fails as invalid

  @adr-0002-d6
  Scenario: The engine rejects two answers to one question
    Given "github:1001" starts a session for a change touching "rust.borrowing", "rust.traits"
    When "github:1001" submits two answers to the first question of the session
    Then the submission fails as invalid

  # -------------------------------------------------------------------------
  # D7: domain events

  @adr-0002-d7
  Scenario Outline: Concluding a session emits SessionGraded
    Given a session for "github:1001" opened with questions about "a.one", "a.two", "a.three", "a.four", "a.five"
    When the session is concluded with pass threshold 0.6, <correct> questions graded Good and the rest graded Again
    Then the SessionGraded event reports <correct> correct out of 5 and <passed>

    Examples:
      | correct | passed |
      | 0       | failed |
      | 2       | failed |
      | 3       | passed |
      | 5       | passed |

  @adr-0002-d7 @adr-0002-d6
  Scenario: Concluding an empty session emits SessionGraded with nothing to grade
    Given a session for "github:1001" opened with no questions
    When the session is concluded with pass threshold 0.6 and no grades
    Then the SessionGraded event reports 0 correct out of 0 and passed

  @adr-0002-d7
  Scenario: The session outcome carries the events of every review and one SessionGraded
    Given the pass threshold is 0.6
    When "github:1001" starts a session for a change touching "rust.borrowing", "rust.traits", "tokio.select"
    And "github:1001" answers the session as follows:
      | concept        | answer  |
      | rust.borrowing | correct |
      | rust.traits    | wrong   |
      | tokio.select   | correct |
    Then the outcome events hold 3 ReviewRecorded events
    And the outcome events hold 3 ReviewScheduled events
    And the outcome events record a review of each of "rust.borrowing", "rust.traits", "tokio.select"
    And the ReviewScheduled events in the outcome match the review cards of "github:1001"
    And the outcome events hold 0 LevelChanged events
    And the outcome events hold exactly one SessionGraded event, reporting 2 correct out of 3 and passed

  @adr-0002-d7 @adr-0002-d4
  Scenario: The session outcome carries LevelChanged when the session changes the level
    Given the question budget is 6
    When "github:1001" starts a session for a change touching "c.one", "c.two", "c.three", "c.four", "c.five", "c.six"
    And "github:1001" answers every question correctly
    Then the outcome events hold 6 ReviewRecorded events
    And the outcome events hold 1 LevelChanged event
    And the outcome events hold a LevelChanged event from Novice to Intermediate
    And the outcome events hold exactly one SessionGraded event, reporting 6 correct out of 6 and passed
