@adr-0001-d6
Feature: Spaced repetition with FSRS
  Every concept an author is quizzed on becomes an FSRS review card. A
  correct answer pushes the next review further out than a wrong one.
  Concepts whose reviews are due are asked again in the author's next
  session, before new concepts and within the session's question budget.

  @adr-0001-d6
  Scenario: Every quizzed concept becomes a review card
    When "github:1001" answers questions about "rust.ownership", "rust.traits", "tokio.select" correctly
    Then "github:1001" has review cards for exactly "rust.ownership", "rust.traits", "tokio.select"

  @adr-0001-d6
  Scenario: A correct answer schedules the next review later than a wrong one
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.lifetimes"
    And "github:1001" answers the session as follows:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.lifetimes | wrong   |
    Then the next review of "rust.ownership" for "github:1001" is later than the next review of "rust.lifetimes"

  @adr-0001-d6
  Scenario Outline: Any correct grade schedules later than a wrong answer
    Given correct answers are graded <grade>
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.lifetimes"
    And "github:1001" answers the session as follows:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.lifetimes | wrong   |
    Then the next review of "rust.ownership" for "github:1001" is later than the next review of "rust.lifetimes"

    Examples:
      | grade |
      | Hard  |
      | Good  |
      | Easy  |

  @adr-0001-d6
  Scenario: Nothing is due right after a session
    When "github:1001" answers questions about "rust.ownership", "rust.traits" wrongly
    Then no reviews are due for "github:1001"

  @adr-0001-d6
  Scenario: A review falls due as time passes
    Given "github:1001" has answered questions about "rust.ownership" correctly
    When 30 days pass
    Then "rust.ownership" is due for review by "github:1001"

  @adr-0001-d6
  Scenario: Due reviews are listed most overdue first
    Given "github:1001" starts a session for a change touching "rust.ownership", "rust.lifetimes"
    And "github:1001" answers the session as follows:
      | concept        | answer  |
      | rust.ownership | correct |
      | rust.lifetimes | wrong   |
    When 30 days pass
    Then the reviews due for "github:1001" are "rust.lifetimes", "rust.ownership", in that order

  @adr-0001-d6
  Scenario: Due reviews are asked before new concepts
    Given "github:1001" has answered questions about "rust.ownership" correctly
    And 30 days pass
    When "github:1001" starts a session for a change touching "http.retries", "tokio.select"
    Then the session asks about "rust.ownership", "http.retries", "tokio.select", in that order

  @adr-0001-d6
  Scenario: The question budget includes due reviews
    Given the question budget is 3
    And "github:1001" has answered questions about "rust.ownership" correctly
    And 30 days pass
    When "github:1001" starts a session for a change touching "http.retries", "tokio.select", "serde.derive"
    Then the session asks 3 questions
    And the session asks about "rust.ownership", "http.retries", "tokio.select", in that order

  @adr-0001-d6 @adr-0002-d10
  Scenario: Due reviews never take every slot from the change
    Given the question budget is 3
    And "github:1001" has answered questions about "rust.ownership", "rust.traits" correctly
    And 30 days pass
    And the question budget is 2
    When "github:1001" starts a session for a change touching "http.retries", "tokio.select"
    Then the session asks 2 questions
    And the session asks about "rust.ownership", "http.retries", in that order

  @adr-0001-d6 @adr-0002-d10
  Scenario: A change with nothing new still asks one due review
    Given the question budget is 3
    And "github:1001" has answered questions about "rust.ownership", "rust.traits" correctly
    And 30 days pass
    And the question budget is 2
    And the examiner weighs "http.retries" as incidental
    When "github:1001" starts a session for a change touching "http.retries"
    Then the session asks 1 questions

  @adr-0001-d6
  Scenario: A due concept that the change also touches is asked only once
    Given "github:1001" has answered questions about "rust.ownership" correctly
    And 30 days pass
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "rust.ownership", "tokio.select", in that order

  @adr-0001-d6
  Scenario: Answering a due review reschedules it
    Given "github:1001" has answered questions about "rust.ownership" correctly
    And 30 days pass
    When "github:1001" starts a session for a change touching "tokio.select"
    And "github:1001" answers every question correctly
    Then no reviews are due for "github:1001"
    And the history of "github:1001" has 3 graded answers

  @adr-0001-d6
  Scenario: A change with no concepts and nothing due asks nothing
    When "github:1001" starts a session for an empty change
    Then the session asks 0 questions
