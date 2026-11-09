@adr-0001-d7
Feature: Question format adapts to the author's level
  Each profile has a level: Novice, Intermediate or Expert. New authors
  start as Novice. Novices and intermediates get multiple-choice questions.
  Experts get open questions when open questions are enabled; they are off
  by default, so then everyone gets multiple choice. A session never
  alternates formats. The level goes up after sustained correct answers and
  down after sustained wrong ones.

  @adr-0001-d7
  Scenario: A new author starts as a Novice and gets multiple-choice questions
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select"
    Then every question in the session is multiple-choice
    And the profile of "github:1001" is at level Novice

  @adr-0001-d7
  Scenario: By default an expert gets multiple-choice questions too
    Given "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select"
    Then every question in the session is multiple-choice

  @adr-0001-d7
  Scenario: With open questions enabled, an expert gets open questions
    Given open questions are enabled
    And "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select"
    Then every question in the session is open

  @adr-0001-d7
  Scenario: An intermediate gets multiple-choice questions, never a mix
    Given open questions are enabled
    And the question budget is 4
    And "github:1001" is a known Intermediate author
    When "github:1001" starts a session for a change touching "rust.ownership", "rust.traits", "tokio.select", "serde.derive"
    Then every question in the session is multiple-choice

  @adr-0001-d7
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

  @adr-0001-d7
  Scenario Outline: The format policy per level
    Then questions for a <level> author with open questions <open> are <format>

    Examples:
      | level        | open | format          |
      | Novice       | off  | multiple-choice |
      | Novice       | on   | multiple-choice |
      | Intermediate | off  | multiple-choice |
      | Intermediate | on   | multiple-choice |
      | Expert       | off  | multiple-choice |
      | Expert       | on   | open            |

  @adr-0001-d7
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

  @adr-0001-d7
  Scenario: A novice who keeps answering correctly is promoted
    When "github:1001" answers 7 sessions of 3 new concepts correctly
    Then the profile of "github:1001" is at a level higher than Novice
    And the last outcome reports the level stored in the profile of "github:1001"

  @adr-0001-d7
  Scenario: An expert who keeps answering wrongly is demoted and starts getting multiple-choice questions
    Given "github:1001" is a known Expert author
    When "github:1001" answers 7 sessions of 3 new concepts wrongly
    Then the profile of "github:1001" is at a level lower than Expert
    And the last outcome reports the level stored in the profile of "github:1001"
    When "github:1001" starts a session for a change touching "late.one", "late.two", "late.three"
    Then the session contains at least one multiple-choice question

  @adr-0001-d7
  Scenario: One wrong answer does not demote an expert
    Given "github:1001" is a known Expert author
    When "github:1001" answers questions about "rust.ownership" wrongly
    Then the profile of "github:1001" is at level Expert
