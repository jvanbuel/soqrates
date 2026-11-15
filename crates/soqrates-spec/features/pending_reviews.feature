@adr-0002-d10
Feature: A due review is asked in one pending session at a time
  A concept whose review is due is asked in the first session opened while
  it is due. Other sessions of the same learner leave it out while that
  session is pending or being graded, so several open pull requests don't
  each ask it. Once that session is graded the usual schedule applies; if it
  is discarded or superseded without being graded, or left unanswered for
  longer than the hold, the review is free again.

  Background:
    Given "github:1001" has answered questions about "rust.ownership" correctly
    And 30 days pass
    And "github:1001" starts a session for a change touching "http.retries"

  @adr-0002-d10 @adr-0001-d6
  Scenario: The first pending session asks the due review
    Then the session asks about "rust.ownership", "http.retries", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: A second session opened before the first is graded leaves the review out
    When "github:1001" starts a session for a change touching "tokio.select"
    Then the session asks about "tokio.select", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: A held review is left out even when the change touches it
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "tokio.select", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: A review held by a session that is discarded is free again
    When session 2 of "github:1001" is discarded
    And "github:1001" starts a session for a change touching "tokio.select"
    Then the session asks about "rust.ownership", "tokio.select", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: Once the holding session is graded, the usual schedule applies
    When "github:1001" answers session 2 correctly
    And "github:1001" starts a session for a change touching "rust.ownership", "tokio.select"
    Then the session asks about "tokio.select", in that order

  @adr-0002-d10 @adr-0001-d6
  Scenario: A session left unanswered longer than the hold lets the review go
    When 4 days pass
    And "github:1001" starts a session for a change touching "tokio.select"
    Then the session asks about "rust.ownership", "tokio.select", in that order

  @adr-0002-d10 @adr-0003-d1
  Scenario: The hold applies across the learner's linked identities
    Given identities "github:1001" and "gitlab:2002" belong to the same learner "Mira"
    When "gitlab:2002" starts a session for a change touching "tokio.select"
    Then the session asks about "tokio.select", in that order
