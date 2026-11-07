@adr-0001-d7
Feature: Question format adapts to the author's level
  Each profile has a level: Novice, Intermediate or Expert. New authors
  start as Novice. Novices get multiple-choice questions, experts get open
  questions, and intermediates get a mix. The level goes up after sustained
  correct answers and down after sustained wrong ones.

  @adr-0001-d7 @wip
  Scenario: A new author starts as a Novice and gets multiple-choice questions
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select"
    Then every question in the session is multiple-choice
    And the profile of "github:1001" is at level Novice

  @adr-0001-d7 @wip
  Scenario: An expert gets open questions
    Given "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select"
    Then every question in the session is open

  @adr-0001-d7 @wip
  Scenario: An intermediate gets a mix of question formats
    Given the question budget is 4
    And "github:1001" is a known Intermediate author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select", "serde.derive"
    Then the session mixes multiple-choice and open questions

  @adr-0001-d7 @wip
  Scenario Outline: Session questions follow the format policy for the author's level
    Given the question budget is 4
    And "github:1001" is a known <level> author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select", "serde.derive"
    Then the question formats follow the policy for <level> authors

    Examples:
      | level        |
      | Novice       |
      | Intermediate |
      | Expert       |

  @adr-0001-d7 @wip
  Scenario Outline: The format policy per level
    Then the first <count> questions for a <level> author are <formats>

    Examples:
      | level        | count | formats                       |
      | Novice       | 10    | all multiple-choice           |
      | Expert       | 10    | all open                      |
      | Intermediate | 4     | a mix of multiple-choice and open |

  @adr-0001-d7 @wip
  Scenario Outline: The level only moves after sustained answers
    Given a <current> author with a history of <count> <kind> answers
    When their level is recomputed
    Then the recomputed level is <expected>

    Examples:
      | current      | count | kind    | expected                |
      | Novice       | 1     | correct | Novice                  |
      | Novice       | 20    | correct | higher than Novice      |
      | Intermediate | 20    | correct | higher than Intermediate |
      | Expert       | 20    | correct | Expert                  |
      | Expert       | 1     | wrong   | Expert                  |
      | Expert       | 20    | wrong   | lower than Expert       |
      | Intermediate | 20    | wrong   | lower than Intermediate |
      | Novice       | 20    | wrong   | Novice                  |

  @adr-0001-d7 @wip
  Scenario: A novice who keeps answering correctly is promoted and starts getting open questions
    When "github:1001" answers 7 sessions of 3 new concepts correctly
    Then the profile of "github:1001" is at a level higher than Novice
    And the last outcome reports the level stored in the profile of "github:1001"
    When "github:1001" starts a session for a change touching "late.one", "late.two", "late.three"
    Then the session contains at least one open question

  @adr-0001-d7 @wip
  Scenario: An expert who keeps answering wrongly is demoted and starts getting multiple-choice questions
    Given "github:1001" is a known Expert author
    When "github:1001" answers 7 sessions of 3 new concepts wrongly
    Then the profile of "github:1001" is at a level lower than Expert
    And the last outcome reports the level stored in the profile of "github:1001"
    When "github:1001" starts a session for a change touching "late.one", "late.two", "late.three"
    Then the session contains at least one multiple-choice question

  @adr-0001-d7 @wip
  Scenario: One wrong answer does not demote an expert
    Given "github:1001" is a known Expert author
    When "github:1001" answers questions about "rust.ownership" wrongly
    Then the profile of "github:1001" is at level Expert
