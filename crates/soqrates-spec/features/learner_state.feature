@adr-0001-d5
Feature: Per-author learner state
  Soqrates keeps a learner profile per author, keyed by a stable author ID
  such as "github:12345" or "email:<address>". Profiles and pending
  sessions are persisted through the StateStore port. Profiles of different
  authors never affect each other, and a profile survives across sessions.

  @adr-0001-d5
  Scenario: An author Soqrates has never seen gets a fresh Novice profile
    When I look up the profile of "github:1001"
    Then the profile belongs to "github:1001"
    And the profile of "github:1001" is at level Novice
    And "github:1001" has no review cards
    And the history of "github:1001" has 0 graded answers

  @adr-0001-d5
  Scenario: A profile remembers the concepts an author was quizzed on
    When "github:1001" answers questions about "rust.ownership", "rust.traits" correctly
    Then "github:1001" has review cards for exactly "rust.ownership", "rust.traits"
    And the history of "github:1001" has 2 graded answers

  @adr-0001-d5
  Scenario: A profile survives across sessions
    Given "github:1001" has answered questions about "rust.ownership" correctly
    When "github:1001" answers questions about "tokio.select" wrongly
    Then "github:1001" has review cards for exactly "rust.ownership", "tokio.select"
    And the history of "github:1001" has 2 graded answers

  @adr-0001-d5
  Scenario: Quizzing one author leaves another author's profile untouched
    Given "github:1001" has answered questions about "rust.ownership", "rust.traits" wrongly
    When I look up the profile of "github:2002"
    Then "github:2002" has no review cards
    And the history of "github:2002" has 0 graded answers
    And the profile of "github:2002" is at level Novice

  @adr-0001-d5
  Scenario: Another author's level does not change the questions I get
    Given "github:1001" is a known Expert author
    When "github:2002" starts a session for a change touching "rust.ownership", "rust.traits"
    Then every question in the session is multiple-choice

  @adr-0001-d5
  Scenario: Due reviews belong to the author who was quizzed
    Given "github:1001" has answered questions about "rust.ownership" correctly
    When 30 days pass
    Then "rust.ownership" is due for review by "github:1001"
    And no reviews are due for "github:2002"

  @adr-0001-d5
  Scenario: A started session is pending in the store until it is answered
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the session is stored as pending
    When "github:1001" answers every question correctly
    Then the session is no longer stored as pending

  @adr-0001-d5 @adr-0005-d1
  Scenario Outline: A profile survives a restart when kept in <store>
    Given Soqrates keeps state in <store> in a fresh directory
    And "<author>" has answered questions about "rust.ownership", "rust.traits" correctly
    When Soqrates restarts with the same state directory
    Then the profile belongs to "<author>"
    And "<author>" has review cards for exactly "rust.ownership", "rust.traits"
    And the history of "<author>" has 2 graded answers

    Examples:
      | store             | author                                  |
      | a file store      | github:12345                            |
      | a file store      | email:jan@example.com                   |
      | a file store      | email:jan.de.vries+soqrates@example.com |
      | a SQLite database | github:12345                            |
      | a SQLite database | email:jan@example.com                   |
      | a SQLite database | email:jan.de.vries+soqrates@example.com |

  @adr-0001-d5 @adr-0005-d1
  Scenario Outline: A pending session survives a restart when kept in <store>
    Given Soqrates keeps state in <store> in a fresh directory
    And "github:1001" starts a session for a change touching "rust.ownership"
    When Soqrates restarts with the same state directory
    Then the session is stored as pending
    When "github:1001" answers every question correctly
    Then "github:1001" has review cards for exactly "rust.ownership"

    Examples:
      | store             |
      | a file store      |
      | a SQLite database |

  @adr-0001-d5 @adr-0003-d1
  Scenario: The file store keeps one profile document per learner
    Given Soqrates keeps state in a file store in a fresh directory
    And "github:1001" has answered questions about "rust.ownership" correctly
    And "email:jan@example.com" has answered questions about "rust.traits" wrongly
    Then the state directory holds exactly 2 learner profile documents
    And no learner profile document's file name reveals "github:1001" or "email:jan@example.com"
    And no JSON document in the state directory mentions both "github:1001" and "email:jan@example.com"

  @adr-0001-d5 @adr-0005-d1
  Scenario Outline: Authors in <store> do not interfere across a restart
    Given Soqrates keeps state in <store> in a fresh directory
    And "github:1001" has answered questions about "rust.ownership" correctly
    And "github:2002" has answered questions about "rust.traits" wrongly
    When Soqrates restarts with the same state directory
    Then "github:1001" has review cards for exactly "rust.ownership"
    And "github:2002" has review cards for exactly "rust.traits"

    Examples:
      | store             |
      | a file store      |
      | a SQLite database |
