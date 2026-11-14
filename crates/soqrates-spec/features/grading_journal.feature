@adr-0002-d6
Feature: The grading journal holds the graded reviews
  Grading a session is written ahead in a journal. The journal holds the
  reviews that grading adds to the learner's profile, not the profile as
  it should look afterwards. Applying it records them on the profile as it
  is then, so another session of the learner graded at the same time
  loses nothing, and applying it again changes nothing.

  Scenario: Two sessions of one learner graded at the same moment both count
    Given "github:1001" has two pending sessions, about "rust.ownership" and about "rust.traits"
    When both sessions are graded at the same moment
    Then the history of "github:1001" has 2 graded answers
    And "github:1001" has review cards for exactly "rust.ownership", "rust.traits"

  Scenario Outline: Two sessions of one learner in <store> graded at the same moment both count
    Given Soqrates keeps state in <store> in a fresh directory
    And "github:1001" has two pending sessions, about "rust.ownership" and about "rust.traits"
    When both sessions are graded at the same moment
    Then the history of "github:1001" has 2 graded answers
    And "github:1001" has review cards for exactly "rust.ownership", "rust.traits"

    Examples:
      | store             |
      | a file store      |
      | a SQLite database |

  Scenario: Finishing a grading again and again records its reviews once
    Given "github:1001" starts a session for a change touching "rust.ownership", "rust.traits"
    And the process stops after 2 writes to the store
    When "github:1001" submits correct answers and the process stops
    Then the grading journal of the session is stored
    And the stored profile of "github:1001" has 2 graded answers
    When Soqrates tries to finish the grading 3 times
    Then the grading journal of the session is stored
    And the stored profile of "github:1001" has 2 graded answers
    When the store works again
    Then the history of "github:1001" has 2 graded answers
    And no grading journal is stored
    And the history of "github:1001" has 2 graded answers

  Scenario: A grading journal in the old format is still applied
    Given "github:1001" starts a session for a change touching "rust.ownership", "rust.traits"
    When Soqrates finds a grading journal in the old format for "github:1001"
    Then the history of "github:1001" has 2 graded answers
    And no grading journal is stored
    And the history of "github:1001" has 2 graded answers
