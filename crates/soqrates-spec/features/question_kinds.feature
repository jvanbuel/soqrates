@adr-0007-d1 @adr-0007-d2
Feature: Questions of different kinds
  A question has a kind as well as a format: a scenario in prose, predict
  the outcome of a snippet, spot the bug in it, choose the fix, or explain
  your own change. The format follows the level (ADR-0002 D5). The examiner
  chooses the kind per concept, from the kinds the author's level allows,
  and a round mixes kinds. The scripted examiner takes the first kind it is
  offered.

  @adr-0007-d2
  Scenario Outline: The author's level steers the kind of question
    Given open questions are enabled
    And "github:1001" is a known <level> author
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" is <format> question of kind <kind>

    Examples:
      | level        | format            | kind                |
      | Novice       | a multiple-choice | predict_outcome     |
      | Intermediate | a multiple-choice | spot_the_bug        |
      | Expert       | an open           | explain_your_change |

  @adr-0007-d2 @adr-0007-d3
  Scenario: A novice predicts the outcome of a general snippet
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" is a multiple-choice question of kind predict_outcome
    And the question about "rust.ownership" shows a snippet the examiner wrote, as "rust" code
    And the prompt of the question about "rust.ownership" contains none of its code

  @adr-0007-d2 @adr-0007-d4
  Scenario: An expert explains their own change, in an open question
    Given open questions are enabled
    And "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" is an open question of kind explain_your_change
    And the question about "rust.ownership" shows the author's own code

  @adr-0007-d2
  Scenario: A novice's round mixes kinds
    Given the question budget is 4
    When "github:1001" starts a session for a change touching "a.one", "b.two", "c.three", "d.four"
    Then the kinds of the questions are predict_outcome, spot_the_bug, scenario, choose_the_fix, in that order

  @adr-0007-d2
  Scenario: An expert's round mixes kinds
    Given "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "a.one", "b.two", "c.three"
    Then the kinds of the questions are explain_your_change, spot_the_bug, scenario, in that order

  @adr-0007-d2 @adr-0007-d3
  Scenario: Choosing the fix is multiple choice, with code options
    Given the question budget is 4
    When "github:1001" starts a session for a change touching "a.one", "b.two", "c.three", "d.four"
    Then the question about "d.four" is a multiple-choice question of kind choose_the_fix
    And the options of the question about "d.four" are "rust" code

  @adr-0007-d2 @adr-0007-d4
  Scenario: A due review has no hunk of the change, so it is not asked as explain_your_change
    Given open questions are enabled
    And "github:1001" is a known Expert author
    And "github:1001" has answered questions about "rust.ownership" wrongly
    And 30 days pass
    When "github:1001" starts a session for a change touching "tokio.select"
    Then the question about "rust.ownership" is an open question of kind spot_the_bug
    And the question about "tokio.select" is an open question of kind explain_your_change

  @adr-0007-d1
  Scenario: A question carried to a new push keeps its kind and its code
    Given open questions are enabled
    And "github:1001" is a known Expert author
    And a pull request "jvanbuel/soqrates#15" by "github:1001" at head "abc123" touching "rust.ownership"
    And the Soqrates check has run
    When the author pushes head "def456" touching "rust.ownership", "rust.traits"
    And the Soqrates check runs
    Then the posted question about "rust.ownership" is an open question of kind explain_your_change
    And the posted question about "rust.ownership" is asked as it was for head "abc123"
