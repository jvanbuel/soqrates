@adr-0011-d1 @adr-0011-d2
Feature: Authors give feedback on graded questions
  After grading, the author can say that a question or its answer key is
  wrong or unclear, not about the change, or good, with `/soqrates
  feedback <question> <kind> [why]` on the pull request or at the terminal
  after a local session. The feedback is kept with the question as it was asked, the
  author's answer and the grade, in the store of the author's sessions,
  never in the learner store. Grades stay as they are. A dispute keeps a
  question from being asked again from the question bank until a
  maintainer resolves it.

  @adr-0011-d1 @adr-0011-d2
  Scenario: The author's feedback is kept with the question, the answer and the grade
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership", "rust.traits" were graded
    When the author comments "/soqrates feedback 2 disagree option b is also correct"
    And the Soqrates check runs
    Then 1 feedback note was posted, saying the feedback was recorded
    And question 2 of the graded session has 1 "disagree" feedback saying "option b is also correct"
    And question 2 of the graded session is kept with the author's answer and the grade Again

  @adr-0011-d4
  Scenario: Feedback from someone other than the PR author is ignored
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership", "rust.traits" were graded
    When "github:2002" comments "/soqrates feedback 2 disagree option b is also correct"
    And the Soqrates check runs
    Then no feedback note was posted
    And no question of the graded session has feedback

  @adr-0011-d2
  Scenario: Feedback on a question that was never graded is rejected with a short note
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership", "rust.traits" were graded
    When the author comments "/soqrates feedback 3 unclear"
    And the Soqrates check runs
    Then 1 feedback note was posted, saying the feedback was not recorded
    And no question of the graded session has feedback

  @adr-0011-d2
  Scenario: Feedback before any results is rejected with a short note
    Given a pull request "jvanbuel/soqrates#7" by "github:1001" at head "abc123" touching "rust.ownership"
    And the Soqrates check has run
    When the author comments "/soqrates feedback 1 disagree"
    And the Soqrates check runs
    Then 1 feedback note was posted, saying there are no graded results yet
    And the check is Pending

  @adr-0011-d2
  Scenario: The same feedback comment counts once, however often the check runs
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership", "rust.traits" were graded
    When the author comments "/socrates FEEDBACK 1 good"
    And the Soqrates check runs
    And the Soqrates check runs
    Then 1 feedback note was posted, saying the feedback was recorded
    And question 1 of the graded session has 1 "good" feedback

  @adr-0011-d1
  Scenario: A project question's feedback is kept in the organisation's store, never in the learner store
    Given the forge "github.com" is administered by organisation "acme"
    And the examiner classifies "acme.billing" as Project
    And the author's wrong answers to a pull request by "github.com:1001" touching "acme.billing" were graded
    When the author comments "/soqrates feedback 1 unclear which field is meant?"
    And the Soqrates check runs
    Then the store of organisation "acme" keeps 1 "unclear" feedback on question 1
    And the learner store keeps no asked question

  @adr-0011-d6
  Scenario: A question with open disagree feedback is not reused from the bank
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership" were graded
    And the question about "rust.ownership" came from the question bank
    When the author comments "/soqrates feedback 1 disagree the answer key is wrong"
    And the Soqrates check runs
    And "github:1002" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" is not the disputed bank question
    And 2 questions were written about "rust.ownership"

  @adr-0011-d7
  Scenario: A rejected dispute lets the question be reused, and no grade changes
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership" were graded
    And the author comments "/soqrates feedback 1 disagree the answer key is wrong"
    And the Soqrates check has run
    When a maintainer rejects that feedback
    And "github:1002" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" comes from the question bank
    And question 1 of the graded session is kept with the author's answer and the grade Again

  @adr-0011-d7
  Scenario: An accepted dispute retires the bank question
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership" were graded
    And the author comments "/soqrates feedback 1 disagree the answer key is wrong"
    And the Soqrates check has run
    When a maintainer accepts that feedback
    Then the bank question about "rust.ownership" is retired
    And question 1 of the graded session is kept with the author's answer and the grade Again

  @adr-0011-d8
  Scenario: Deleting a learner deletes their feedback
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership" were graded
    And the author comments "/soqrates feedback 1 disagree the answer key is wrong"
    And the Soqrates check has run
    When identity "github:1001" deletes its learner profile
    Then no store keeps an asked question
    And no bank question about "rust.ownership" is disputed

  @adr-0011-d4
  Scenario: Feedback stays with a learner whose identity is linked to another learner
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership" were graded
    And the author comments "/soqrates feedback 1 disagree the answer key is wrong"
    And the Soqrates check has run
    And identity "gitlab:678" requested a link code
    When identity "github:1001" presents that link code
    Then question 1 of the graded session has 1 "disagree" feedback
    When a maintainer rejects that feedback
    Then no bank question about "rust.ownership" is disputed

  @adr-0011-d9
  Scenario: The export with feedback includes the question, the answer, the grade and every feedback entry
    Given the author's wrong answers to a pull request by "github:1001" touching "rust.ownership", "rust.traits" were graded
    And the author comments "/soqrates feedback 1 unclear which borrow is meant?"
    And the author comments "/soqrates feedback 2 good"
    And the Soqrates check has run
    When the asked questions are exported with feedback
    Then the export has 2 lines
    And line 1 of the export has the question, the author's answer, the grade Again and 1 "unclear" feedback saying "which borrow is meant?"
    And line 2 of the export has 1 "good" feedback
    And the export never names "github:1001" or the session

  @adr-0011-d2
  Scenario: The author gives feedback at the terminal after a local session
    Given the author will answer wrongly at the terminal
    And the author will type "1 wrong-explanation it argues about an option not shown" at the feedback prompt
    When "github:1001" runs the pre-commit hook on a change touching "rust.ownership"
    Then the author was told the feedback was recorded
    And question 1 of the graded session has 1 "wrong-explanation" feedback saying "it argues about an option not shown"

  @adr-0011-d2
  Scenario: Pressing Enter at the feedback prompt skips it
    Given the author will answer wrongly at the terminal
    When "github:1001" runs the pre-commit hook on a change touching "rust.ownership"
    Then the author was told nothing about feedback
    And no question of the graded session has feedback
