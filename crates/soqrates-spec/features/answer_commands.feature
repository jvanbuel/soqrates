@adr-0004-d8 @adr-0004-d9 @adr-0001-d8
Feature: Answering on the pull request is forgiving and visible
  Authors answer in a comment that starts with `/soqrates answer`. The command
  forgives the philosopher's spelling `/socrates`, any letter case and a
  mention of the app Soqrates posts as, but never a bare `@soqrates`. A
  comment that starts like the command but isn't one gets a hint, once. An
  answer comment can be edited until its session is graded. The author sees
  when grading starts and when it is done.

  Background:
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"
    And Soqrates posts as the app "soqrates-review"
    And the Soqrates check has run

  @adr-0004-d8
  Scenario Outline: A forgiving answer command is graded like /soqrates answer
    Given the author comments "<command>" with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the results were posted on the pull request 1 time
    And the posted results have 2 correct answers

    Examples:
      | command                 |
      | /soqrates answer        |
      | /socrates answer        |
      | /SoQrates ANSWER        |
      | @soqrates-review answer |

  @adr-0004-d8
  Scenario Outline: A comment that isn't the answer command is not graded
    Given the author comments "<command>" with correct answers
    When the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 0 times

    Examples:
      | command                  |
      | @soqrates answer         |
      | @someone-else answer     |
      | /socrates answr          |
      | thanks, /soqrates answer |

  @adr-0004-d8
  Scenario: A mistyped command gets one hint, and an identical second comment gets none
    Given the author comments "/socrates answr" with correct answers
    When the Soqrates check runs
    And the Soqrates check runs
    And the author comments "/socrates answr" with correct answers
    And the Soqrates check runs
    Then 1 hint was posted
    And the check is Pending

  @adr-0004-d8
  Scenario: Each different mistyped command gets its own hint
    Given the author comments "/socrates answr" with correct answers
    And the author comments "/soqrates answers" with correct answers
    When the Soqrates check runs
    Then 2 hints were posted

  @adr-0004-d8
  Scenario: A comment that is no command at all gets no hint
    Given the author comments "Thanks for the questions!" with correct answers
    When the Soqrates check runs
    Then 0 hints were posted

  @adr-0004-d8
  Scenario: Fixing a mistyped command by editing it gets the answers graded
    Given the author comments "/socrates answr" with correct answers
    And the Soqrates check has run
    When the author edits that comment to "/socrates answer" with correct answers
    And the Soqrates check runs
    Then the check is Success
    And the posted results have 2 correct answers

  @adr-0004-d8
  Scenario: Editing an ungraded answer comment grades the edited text
    Given the author comments "/soqrates answer" with wrong answers
    When the author edits that comment to "/soqrates answer" with correct answers
    And the Soqrates check runs
    Then the check is Success
    And the posted results have 2 correct answers

  @adr-0004-d8
  Scenario: Editing an answer comment after grading changes nothing
    Given the author comments "/soqrates answer" with wrong answers
    And the Soqrates check has run
    When the author edits that comment to "/soqrates answer" with correct answers
    And the Soqrates check runs
    Then the check is Neutral
    And the results were posted on the pull request 1 time
    And the posted results have 0 correct answers

  @adr-0004-d9
  Scenario: The submitting comment is acknowledged, then shown as graded
    Given the author comments "/soqrates answer" with correct answers
    When the Soqrates check runs
    Then the author saw, in order:
      | what                                 |
      | 👀 on the answer comment             |
      | check Pending: Grading your answers… |
      | results                              |
      | ✅ on the answer comment             |
      | check Success                        |
    And the answer comment shows only ✅

  @adr-0004-d7 @adr-0004-d9
  Scenario: Answers graded before a push starts the next round are acknowledged too
    Given the author comments "/soqrates answer" with correct answers
    When the author pushes head "def456" touching "rust.ownership", "tokio.select"
    And the Soqrates check runs
    Then the results were posted on the pull request 1 time
    And the author saw, in order:
      | what                     |
      | 👀 on the answer comment |
      | results                  |
      | ✅ on the answer comment |
    And the answer comment shows only ✅
    And the check never said "Grading your answers…"

  @adr-0004-d9
  Scenario: Answers that are not picked up get no acknowledgement
    Given the author comments "/socrates answr" with correct answers
    When the Soqrates check runs
    Then the answer comment shows no reaction
    And the check never said "Grading your answers…"
