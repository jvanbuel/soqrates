@adr-0002-d3 @adr-0002-d4 @adr-0002-d5 @adr-0002-d7 @adr-0002-d8
Feature: The learner profile aggregate
  A learner profile is everything Soqrates knows about one author. It
  changes only through its own operations: recording a review updates the
  concept's review card, appends to the history and re-evaluates the level,
  all at once, and returns domain events describing what happened. The
  level follows fixed rules: six consecutive correct reviews since the last
  level change promote the author, three consecutive incorrect ones demote
  them. The level decides the format of each question in a session. Due
  reviews come earliest due first, ties broken by concept ID.

  These scenarios use the aggregate and the policy directly: no engine, no
  ports, no async code.

  # -------------------------------------------------------------------------
  # D3: recording a review changes card, history and level together

  @adr-0002-d3
  Scenario: A new learner profile is empty and at level Novice
    Given a new learner profile for "github:1001"
    Then the learner profile belongs to "github:1001"
    And the learner profile is at level Novice
    And the learner profile has 0 review cards
    And the learner profile has 0 reviews in its history

  @adr-0002-d3 @adr-0002-d4
  Scenario Outline: An author may declare a starting level
    Given a learner profile for "github:1001" starting at <level>
    Then the learner profile belongs to "github:1001"
    And the learner profile is at level <level>
    And the learner profile has 0 review cards
    And the learner profile has 0 reviews in its history

    Examples:
      | level        |
      | Novice       |
      | Intermediate |
      | Expert       |

  @adr-0002-d3
  Scenario: Recording a review creates the review card and appends to the history
    Given a new learner profile for "github:1001"
    When a review of "rust.borrowing" graded Good is recorded for an open question
    Then the learner profile has 1 review card
    And the learner profile has a review card for "rust.borrowing" reviewed 1 time
    And the review card for "rust.borrowing" is due later than now
    And the learner profile has 1 review in its history
    And the last review in the history is of "rust.borrowing", graded Good, for an open question, recorded now

  @adr-0002-d3
  Scenario: Recording another review of the same concept reschedules the same review card
    Given a new learner profile for "github:1001"
    And a review of "rust.borrowing" graded Good is recorded
    And 3 days pass
    When a review of "rust.borrowing" graded Again is recorded
    Then the learner profile has 1 review card
    And the learner profile has a review card for "rust.borrowing" reviewed 2 times
    And the learner profile has 2 reviews in its history
    And the last review in the history is of "rust.borrowing", graded Again, for a multiple-choice question, recorded now

  @adr-0002-d3 @adr-0002-d4
  Scenario: The review that completes a streak updates card, history and level at once
    Given a new learner profile for "github:1001"
    And 5 reviews graded Good are recorded
    When a review of "rust.borrowing" graded Good is recorded
    Then the learner profile has 6 review cards
    And the learner profile has a review card for "rust.borrowing" reviewed 1 time
    And the learner profile has 6 reviews in its history
    And the last review in the history is of "rust.borrowing", graded Good, for a multiple-choice question, recorded now
    And the learner profile is at level Intermediate

  # -------------------------------------------------------------------------
  # D4: level rules

  @adr-0002-d4
  Scenario: The thresholds are six correct reviews to promote and three incorrect reviews to demote
    Then promotion takes 6 consecutive correct reviews and demotion takes 3 consecutive incorrect reviews

  @adr-0002-d4
  Scenario Outline: The level moves exactly at the thresholds
    Given a learner profile for "github:1001" starting at <start>
    When <count> reviews graded <grade> are recorded
    Then the learner profile is at level <level>

    Examples: promotion
      | start        | count | grade | level        |
      | Novice       | 5     | Good  | Novice       |
      | Novice       | 6     | Good  | Intermediate |
      | Intermediate | 5     | Good  | Intermediate |
      | Intermediate | 6     | Good  | Expert       |

    Examples: demotion
      | start        | count | grade | level        |
      | Expert       | 2     | Again | Expert       |
      | Expert       | 3     | Again | Intermediate |
      | Intermediate | 2     | Again | Intermediate |
      | Intermediate | 3     | Again | Novice       |

    Examples: the level stays within Novice and Expert
      | start        | count | grade | level        |
      | Novice       | 3     | Again | Novice       |
      | Novice       | 10    | Again | Novice       |
      | Expert       | 6     | Good  | Expert       |
      | Expert       | 20    | Good  | Expert       |

    Examples: Hard, Good and Easy all count as correct
      | start        | count | grade | level        |
      | Novice       | 6     | Hard  | Intermediate |
      | Novice       | 6     | Good  | Intermediate |
      | Novice       | 6     | Easy  | Intermediate |
      | Intermediate | 6     | Hard  | Expert       |
      | Intermediate | 6     | Easy  | Expert       |

  @adr-0002-d4
  Scenario: Correct grades of different kinds form one streak
    Given a new learner profile for "github:1001"
    When 2 reviews graded Hard are recorded
    And 2 reviews graded Good are recorded
    And 2 reviews graded Easy are recorded
    Then the learner profile is at level Intermediate

  @adr-0002-d4
  Scenario: A level change resets the streak
    Given a new learner profile for "github:1001"
    When 6 reviews graded Good are recorded
    Then the learner profile is at level Intermediate
    And the learner profile has 0 reviews since the last level change
    When 5 reviews graded Good are recorded
    Then the learner profile is at level Intermediate
    And the learner profile has 5 reviews since the last level change
    When 1 review graded Good is recorded
    Then the learner profile is at level Expert

  @adr-0002-d4
  Scenario: A demotion resets the streak
    Given a learner profile for "github:1001" starting at Expert
    When 3 reviews graded Again are recorded
    Then the learner profile is at level Intermediate
    When 2 reviews graded Again are recorded
    Then the learner profile is at level Intermediate
    When 1 review graded Again is recorded
    Then the learner profile is at level Novice

  @adr-0002-d4
  Scenario: An incorrect review breaks a correct streak
    Given a new learner profile for "github:1001"
    When 5 reviews graded Good are recorded
    And 1 review graded Again is recorded
    And 5 reviews graded Good are recorded
    Then the learner profile is at level Novice
    When 1 review graded Good is recorded
    Then the learner profile is at level Intermediate

  @adr-0002-d4
  Scenario: A correct review breaks an incorrect streak
    Given a learner profile for "github:1001" starting at Expert
    When 2 reviews graded Again are recorded
    And 1 review graded Hard is recorded
    And 2 reviews graded Again are recorded
    Then the learner profile is at level Expert

  @adr-0002-d4
  Scenario Outline: The level policy looks only at the trailing streak of the reviews since the last level change
    Then the next level of a <current> author after <reviews> is <expected>

    Examples:
      | current      | reviews                    | expected     |
      | Novice       | 0 Good                     | Novice       |
      | Novice       | 5 Good                     | Novice       |
      | Novice       | 6 Good                     | Intermediate |
      | Novice       | 1 Again, 6 Good            | Intermediate |
      | Novice       | 5 Good, 1 Again, 5 Good    | Novice       |
      | Novice       | 3 Easy, 3 Hard             | Intermediate |
      | Intermediate | 6 Good                     | Expert       |
      | Intermediate | 2 Again                    | Intermediate |
      | Intermediate | 3 Again                    | Novice       |
      | Intermediate | 2 Again, 1 Good, 2 Again   | Intermediate |
      | Intermediate | 5 Good, 3 Again            | Novice       |
      | Expert       | 3 Again                    | Intermediate |
      | Expert       | 6 Good                     | Expert       |
      | Novice       | 3 Again                    | Novice       |

  # -------------------------------------------------------------------------
  # D5: format rule

  @adr-0002-d5
  Scenario Outline: The format of every question follows the level
    Then questions for a <level> author with open questions <open> are <format>

    Examples:
      | level        | open | format          |
      | Novice       | off  | multiple-choice |
      | Novice       | on   | multiple-choice |
      | Intermediate | off  | multiple-choice |
      | Intermediate | on   | multiple-choice |
      | Expert       | off  | multiple-choice |
      | Expert       | on   | open            |

  @adr-0002-d5
  Scenario: Open questions are off by default
    Then open questions are off in the default configuration

  # -------------------------------------------------------------------------
  # D7: domain events emitted by the learner profile

  @adr-0002-d7
  Scenario: Recording a review emits ReviewRecorded and ReviewScheduled
    Given a new learner profile for "github:1001"
    When a review of "rust.borrowing" graded Good is recorded
    Then the last recorded review emitted exactly ReviewRecorded, ReviewScheduled
    And the ReviewRecorded event names "github:1001" and the review of "rust.borrowing" graded Good
    And the ReviewScheduled event for "rust.borrowing" is due when its review card is due

  @adr-0002-d7
  Scenario Outline: Every review emits ReviewScheduled with the new due date, whatever the grade
    Given a new learner profile for "github:1001"
    When a review of "rust.borrowing" graded <grade> is recorded
    Then the last recorded review emitted exactly ReviewRecorded, ReviewScheduled
    And the ReviewScheduled event for "rust.borrowing" is due when its review card is due

    Examples:
      | grade |
      | Again |
      | Hard  |
      | Good  |
      | Easy  |

  @adr-0002-d7
  Scenario: A review that does not change the level emits no LevelChanged
    Given a new learner profile for "github:1001"
    When 5 reviews graded Good are recorded
    Then no recorded review emitted LevelChanged

  @adr-0002-d7 @adr-0002-d4
  Scenario: The review that promotes the author also emits LevelChanged
    Given a new learner profile for "github:1001"
    And 5 reviews graded Good are recorded
    When a review of "rust.borrowing" graded Good is recorded
    Then the last recorded review emitted exactly ReviewRecorded, ReviewScheduled, LevelChanged
    And the LevelChanged event names "github:1001" and goes from Novice to Intermediate
    And the ReviewScheduled event for "rust.borrowing" is due when its review card is due

  @adr-0002-d7 @adr-0002-d4
  Scenario: The review that demotes the author also emits LevelChanged
    Given a learner profile for "github:1001" starting at Expert
    And 2 reviews graded Again are recorded
    When a review of "rust.borrowing" graded Again is recorded
    Then the last recorded review emitted exactly ReviewRecorded, ReviewScheduled, LevelChanged
    And the LevelChanged event names "github:1001" and goes from Expert to Intermediate

  @adr-0002-d7 @adr-0002-d4
  Scenario Outline: No LevelChanged at the edges of the level range
    Given a learner profile for "github:1001" starting at <level>
    When <count> reviews graded <grade> are recorded
    Then the learner profile is at level <level>
    And no recorded review emitted LevelChanged

    Examples:
      | level  | count | grade |
      | Novice | 6     | Again |
      | Expert | 12    | Good  |

  # -------------------------------------------------------------------------
  # D8: order of due reviews

  @adr-0002-d8
  Scenario: Nothing is due right after the reviews
    Given a new learner profile for "github:1001"
    And a review of "rust.borrowing" graded Good is recorded
    Then the learner profile has no due reviews

  @adr-0002-d8
  Scenario: Due reviews come earliest due first, even when the concept ID is later
    Given a new learner profile for "github:1001"
    And a review of "alpha.concept" graded Good is recorded
    And a review of "zeta.concept" graded Again is recorded
    When 30 days pass
    Then the due reviews are "zeta.concept", "alpha.concept", in that order

  @adr-0002-d8
  Scenario: An earlier review falls due earlier
    Given a new learner profile for "github:1001"
    And a review of "zeta.concept" graded Good is recorded
    And 1 day passes
    And a review of "alpha.concept" graded Good is recorded
    When 30 days pass
    Then the due reviews are "zeta.concept", "alpha.concept", in that order

  @adr-0002-d8
  Scenario: Reviews due at the same time are ordered by concept ID
    Given a new learner profile for "github:1001"
    And a review of "tokio.select" graded Good is recorded
    And a review of "rust.traits" graded Good is recorded
    And a review of "rust.borrowing" graded Good is recorded
    And a review of "serde.derive" graded Again is recorded
    When 30 days pass
    Then the due reviews are "serde.derive", "rust.borrowing", "rust.traits", "tokio.select", in that order

  @adr-0002-d8
  Scenario: Only concepts whose review card is due are due reviews
    Given a new learner profile for "github:1001"
    And a review of "rust.traits" graded Good is recorded
    And 30 days pass
    When a review of "rust.borrowing" graded Good is recorded
    Then the due reviews are "rust.traits", in that order
