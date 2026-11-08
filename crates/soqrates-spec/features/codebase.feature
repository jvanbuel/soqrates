@adr-0008-d2 @adr-0008-d4 @adr-0001-d12
Feature: The examiner reads the code the change is about
  Every examiner step may read the codebase: finding the concepts, writing
  the questions and grading the answers. Each surface gives it the
  revision it asks about. In CI that is the pull request's head, from the
  checkout the action made, and a session is graded against the head it
  was asked for, even after a newer push. A hook gives it what is being committed or pushed. The MCP server
  has no codebase: the harness has the code, and the examiner sees only
  the diff.

  Scenario: A pull request's questions are written from the code at its head
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership"
    When the Soqrates check runs
    Then the examiner read "the code at abc123" to find the concepts
    And the examiner read "the code at abc123" to write the questions

  Scenario: A later round reads the code at its own head
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership"
    And the Soqrates check has run
    When the author pushes head "def456" touching "tokio.select"
    And the Soqrates check runs
    Then the examiner last read "the code at def456" to write the questions

  Scenario: A session is graded against the head it was asked for
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership"
    And the Soqrates check has run
    And the author pushes head "fix001" touching "rust.ownership"
    And the Soqrates check has run
    And the author replies with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the examiner read "the code at abc123" to grade the answers
    And the examiner never read "the code at fix001" to grade the answers

  Scenario Outline: A hook reads what it is about
    When "github:1001" runs the <hook> hook on a change touching "rust.traits"
    Then the examiner read "what the <hook> hook is about" to find the concepts
    And the examiner read "what the <hook> hook is about" to write the questions
    And the examiner read "what the <hook> hook is about" to grade the answers

    Examples:
      | hook       |
      | pre-commit |
      | pre-push   |

  @adr-0008-d5
  Scenario: Through MCP the examiner reads no code
    When an MCP client starts and answers a session for "github:1001" on a change touching "rust.traits"
    Then the examiner read no code
