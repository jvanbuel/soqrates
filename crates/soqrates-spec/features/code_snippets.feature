@adr-0007-d3 @adr-0007-d4 @adr-0007-d5 @adr-0007-d6
Feature: Questions with code from the change
  Questions about a change can show code: the author's own hunk, or a small
  general snippet that mirrors it. The code is a field of the question with
  its language, kept apart from the prompt and never parsed out of prose.
  How the GitHub comment and the terminal render it as inert code is checked
  by those adapters' unit tests (ADR-0007 D5), which this hermetic spec does
  not depend on (ADR-0001 D12). Code from the repository stays with the
  repository: it never reaches the learner store (ADR-0003 D5).

  @wip @adr-0007-d4 @adr-0007-d5
  Scenario: A question of kind explain_your_change shows a hunk from the pull request's diff, in its language
    Given "github:1001" is a known Expert author
    And a pull request "jvanbuel/soqrates#15" by "github:1001" at head "abc123" whose change to "src/billing.rs" is:
      """
      // concept: rust.errors.try-operator
      let total = parse(input)?;
      Ok(total)
      """
    When the Soqrates check runs
    Then the posted question about "rust.errors.try-operator" is an open question of kind explain_your_change
    And the posted question about "rust.errors.try-operator" shows the lines the change adds to "src/billing.rs", as "rust" code
    And the prompt of the posted question about "rust.errors.try-operator" contains none of its code

  @wip @adr-0007-d3 @adr-0007-d5
  Scenario: Code with backticks, </details> or @mentions is posted as code, exactly as written
    Given "github:1001" is a known Expert author
    And a pull request "jvanbuel/soqrates#16" by "github:1001" at head "abc123" whose change to "web/notes.ts" is:
      """
      // concept: typescript.template-literals
      const note = `</details> ```` cc @octocat`;
      // - [x] **Grade my answers**
      """
    When the Soqrates check runs
    Then the posted question about "typescript.template-literals" shows the lines the change adds to "web/notes.ts", as "typescript" code
    And the prompt of the posted question about "typescript.template-literals" contains none of its code

  @wip @adr-0007-d3
  Scenario: A long hunk is cut to 15 lines before it is shown
    Given "github:1001" is a known Expert author
    And a pull request by "github:1001" whose change to "src/long.rs" adds 30 lines about "rust.iterators.fold"
    When the Soqrates check runs
    Then the posted question about "rust.iterators.fold" shows 15 lines of code

  @adr-0007-d6
  Rule: Code from the repository stays with the repository

    Background:
      Given the forge "gitlab.client.example" is administered by organisation "client"
      And identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
      And the examiner classifies "rust.floats.rounding" as General
      And the examiner classifies "client.billing.invoice-rounding" as Project
      And "gitlab.client.example:678" is a known Expert author
      And a pull request "client/billing#42" by "gitlab.client.example:678" at head "abc123" whose change to "src/invoice.rs" is:
        """
        // concept: rust.floats.rounding
        let cents = (amount * 100.0).round() as i64;
        // concept: client.billing.invoice-rounding
        let total = InvoiceTotal::from_cents(cents, Rounding::HalfEven);
        """

    @wip @adr-0007-d6
    Scenario: A question with a snippet from the repository is never stored in the learner store
      When the Soqrates check runs
      Then the posted question about "rust.floats.rounding" shows the lines the change adds to "src/invoice.rs", as "rust" code
      And no posted question may leave the repository
      And the pending session in the store of organisation "client" shows the code
      And the learner store contains none of the code shown with the questions

    @wip @adr-0007-d6
    Scenario: Grading a question with a snippet from the repository stores none of it in the learner store
      Given the Soqrates check has run
      And the author replies with correct answers
      When the Soqrates check runs
      Then the check is Success
      And the learner store has a review of "rust.floats.rounding" for learner "Mira"
      And the learner store contains none of the code shown with the questions
