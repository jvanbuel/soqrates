@adr-0009-d4
Feature: A question bank
  Soqrates keeps the questions it writes, keyed by concept, and asks them
  again in later rounds instead of writing every question from scratch.
  A round looks in the question bank first and asks the examiner for a new
  question only when nothing there suits. Only checked questions about
  general concepts that show no code from the change are banked, and a
  learner is not shown the same bank question twice within a few rounds.
  The scripted examiner writes a new variant each time it is asked about a
  concept again.

  @adr-0009-d4 @adr-0009-d5
  Scenario: A second round about a concept already in the bank reuses an unseen stored question
    Given the question bank holds 2 questions about "rust.ownership"
    And "github:1001" has answered questions about "rust.ownership" wrongly
    And 30 days pass
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" is stored question 2 from the question bank
    And no question was written about "rust.ownership"

  @adr-0009-d4
  Scenario: A question is written only when the bank has nothing suitable, and then banked
    When "github:1001" starts a session for a change touching "rust.ownership"
    Then 1 question was written about "rust.ownership"
    And the question bank now holds 1 question about "rust.ownership"
    When "github:1002" starts a session for a change touching "rust.ownership"
    Then the question about "rust.ownership" comes from the question bank
    And 1 question was written about "rust.ownership"

  @adr-0009-d5
  Scenario: A learner is never shown the same question twice within N rounds
    Given a learner is shown a bank question at most once within 3 rounds
    And the question bank holds 2 questions about "rust.ownership"
    When "github:1001" has 6 rounds about "rust.ownership"
    Then "github:1001" was never shown the same question twice within 3 rounds
    And 4 of those rounds asked a question from the question bank

  @adr-0009-d2 @adr-0009-d3
  Scenario: A question that quotes repository code is not added to a shared bank
    Given the deployment shares one question bank across organisations
    And the examiner classifies "acme.billing" as Project
    And "github:1001" is a known Expert author
    When "github:1001" starts a session for a change touching "rust.ownership", "tokio.select", "acme.billing"
    Then the question about "rust.ownership" shows the author's own code
    And the shared question bank holds no question about "rust.ownership"
    And the shared question bank holds no question about "acme.billing"
    And the shared question bank holds 1 question about "tokio.select"

  @adr-0009-d2
  Scenario: Each organisation's question bank lives in its own store, never in the learner store
    Given the forge "github.com" is administered by organisation "acme"
    When "github.com:1001" starts a session for a change touching "rust.ownership"
    Then the question bank of organisation "acme" holds 1 question about "rust.ownership"
    And the learner store holds no question about "rust.ownership"

  @adr-0009-d2
  Scenario: An author outside every organisation gets no bank in the shared learner store
    Given the forge "github.com" is administered by organisation "acme"
    When "email:ada@example.com" starts a session for a change touching "rust.ownership"
    Then 1 question was written about "rust.ownership"
    And the learner store holds no question about "rust.ownership"
