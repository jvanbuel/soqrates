@adr-0004-d1
Feature: Feedback after grading
  A learning check is most useful after the answer: the author learns why an
  answer was right or wrong, and when the concept comes back. Every graded
  session carries a result per question, which the PR, the terminal and the
  MCP client present.

  @adr-0004-d1
  Scenario: Each question gets a result, in question order
    Given "github:1001" starts a session for a change touching "rust.ownership", "rust.traits"
    When "github:1001" answers every question correctly
    Then there is one result per question, in question order

  @adr-0004-d1
  Scenario Outline: A multiple-choice result shows the answer, the correct option and the explanation
    Given "github:1001" starts a session for a change touching "rust.ownership"
    When "github:1001" answers every question <how>
    Then the result for "rust.ownership" is <verdict>
    And the result for "rust.ownership" shows the author's answer
    And the result for "rust.ownership" names the correct option
    And the result for "rust.ownership" explains the answer with the question's explanation

    Examples:
      | how       | verdict |
      | correctly | correct |
      | wrongly   | wrong   |

  @adr-0004-d1
  Scenario Outline: An open result carries the examiner's feedback
    Given "github:1001" is a known Expert author
    And "github:1001" starts a session for a change touching "rust.traits"
    When "github:1001" answers every question <how>
    Then the result for "rust.traits" is <verdict>
    And the result for "rust.traits" names no correct option
    And the result for "rust.traits" carries the examiner's feedback for a <verdict> answer

    Examples:
      | how       | verdict |
      | correctly | correct |
      | wrongly   | wrong   |

  @adr-0004-d1
  Scenario: An unanswered question says so and still explains the answer
    Given "github:1001" starts a session for a change touching "rust.ownership", "rust.traits"
    When "github:1001" answers only the question about "rust.ownership" correctly
    Then the result for "rust.traits" is wrong
    And the result for "rust.traits" says it was not answered and explains the answer

  @adr-0004-d1 @adr-0004-d6
  Scenario: A result says when its concept is next due for review
    Given "github:1001" starts a session for a change touching "rust.ownership"
    When "github:1001" answers every question wrongly
    Then the result for "rust.ownership" is due again when its review card is due

  @adr-0004-d1 @adr-0004-d5 @adr-0001-d8
  Scenario: The results are posted on the pull request once
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"
    And the Soqrates check has run
    And the author replies with correct answers
    When the Soqrates check runs
    And the Soqrates check runs
    Then the results were posted on the pull request 1 time
    And the posted results are for the current head
