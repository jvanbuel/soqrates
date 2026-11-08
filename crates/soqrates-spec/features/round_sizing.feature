@adr-0002-d10
Feature: A session is sized to the change and the author's knowledge
  The question budget is a cap, not a target. Concepts the change merely
  touches are not asked, nor are concepts the author knows and isn't due to
  review. Core concepts come before supporting ones. Due reviews come first,
  but the change keeps at least half of the slots.

  @adr-0002-d10
  Scenario: The default budget is five questions
    Given the default configuration
    When "github:1001" starts a session for a change touching "a.one", "b.two", "c.three", "d.four", "e.five", "f.six"
    Then the session asks 5 questions

  @adr-0002-d10
  Scenario: A concept the change merely touches is not asked
    Given the examiner weighs "tokio.select" as incidental
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "rust.ownership", in that order

  @adr-0002-d10
  Scenario: Core concepts are asked before supporting ones
    Given the examiner weighs "rust.ownership" as supporting
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "tokio.select", "rust.ownership", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: A concept the author knows and isn't due to review is not asked
    Given "github:1001" has answered questions about "rust.ownership" correctly
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "tokio.select", in that order

  @adr-0002-d10
  Scenario: A known concept is not asked even after a wrong answer, until it is due
    Given "github:1001" has answered questions about "rust.ownership" wrongly
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the session asks 0 questions

  @adr-0002-d10
  Scenario: A change in familiar territory asks nothing
    Given "github:1001" has answered questions about "rust.ownership", "rust.traits" correctly
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits"
    Then the session asks 0 questions

  @adr-0002-d10
  Scenario: A change whose concepts are all incidental asks nothing
    Given the examiner weighs "rust.ownership" as incidental
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the session asks 0 questions
