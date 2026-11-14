@adr-0004-d11 @adr-0004-d3
Feature: Open questions are answered inside the question comment
  Each open question has an answer field in the question comment. The author
  edits the comment, types the answer into the field, ticks the boxes of the
  multiple-choice questions and then ticks "Grade my answers", so everything
  is answered in one place. Like the boxes, the fields count only when the
  author edited the comment last. An answer comment stays for authors who
  can't edit the question comment, and wins over the field of a question it
  answers.

  Background:
    Given open questions are enabled
    And "github:1001" is a known Expert author
    And a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership", "rust.traits"
    And the Soqrates check has run

  @adr-0004-d11 @adr-0004-d9
  Scenario: An open answer typed into the question comment's answer field is graded
    Given the author types correct answers into the question comment and ticks "Grade my answers"
    When the Soqrates check runs
    Then every posted question is an open question
    And the check is Success
    And the results were posted on the pull request 1 time
    And the posted results have 2 correct answers
    And the author saw, in order:
      | what                                 |
      | 👀 on the question comment           |
      | check Pending: Grading your answers… |
      | results                              |
      | ✅ on the question comment           |
      | check Success                        |
    And the question comment shows only ✅

  @adr-0004-d11 @adr-0004-d3
  Scenario: An open answer typed into the answer field by someone other than the author is ignored
    Given "github:2002" types correct answers into the question comment and ticks "Grade my answers"
    When the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 0 times
    And the history of "github:1001" has 0 graded answers
    And the question comment shows no reaction

  @adr-0004-d11
  Scenario: Answers typed into the answer fields are not graded until they are submitted
    Given the author types correct answers into the question comment without ticking "Grade my answers"
    When the Soqrates check runs
    Then the check is Pending
    And the results were posted on the pull request 0 times

  @adr-0004-d11 @adr-0004-d3
  Scenario: An answer comment wins over the answer field of the question it answers and submits the others
    Given the author types correct answers into the question comment without ticking "Grade my answers"
    And the author comments "/soqrates answer" answering only question 1 wrongly
    When the Soqrates check runs
    Then the results were posted on the pull request 1 time
    And the posted result for question 1 is wrong
    And the posted result for question 2 is correct

  @adr-0004-d11 @adr-0004-d3
  Scenario: The author's answer comment still counts when someone else edited the question comment last
    Given "github:2002" types wrong answers into the question comment and ticks "Grade my answers"
    And the author comments "/soqrates answer" with correct answers
    When the Soqrates check runs
    Then the check is Success
    And the posted results have 2 correct answers
