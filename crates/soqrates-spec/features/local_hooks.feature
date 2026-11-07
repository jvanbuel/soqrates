@adr-0001-d9
Feature: Local hooks share one entry point
  "soqrates hook pre-commit" (git) and "soqrates hook pre-push" (jj) run the
  same local workflow: quiz the author at the terminal through the Prompter
  port, then allow or block. The default is Advisory: report the result and
  always allow. In Blocking mode a failed quiz blocks the commit or push. An
  empty diff never asks any questions.

  @adr-0001-d9 @wip
  Scenario Outline: In the default Advisory mode a failed quiz is reported but allowed
    Given the default configuration
    And the author will answer wrongly at the terminal
    When "email:jan@example.com" runs the <hook> hook on a change touching "rust.ownership", "rust.traits"
    Then the hook allows it
    And the author was asked 2 questions at the terminal
    And a failed result was reported to the author

    Examples:
      | hook       |
      | pre-commit |
      | pre-push   |

  @adr-0001-d9 @wip
  Scenario Outline: In Blocking mode a failed quiz blocks
    Given Soqrates runs in Blocking mode
    And the author will answer wrongly at the terminal
    When "email:jan@example.com" runs the <hook> hook on a change touching "rust.ownership", "rust.traits"
    Then the hook blocks it
    And a failed result was reported to the author

    Examples:
      | hook       |
      | pre-commit |
      | pre-push   |

  @adr-0001-d9 @wip
  Scenario Outline: In Blocking mode a passed quiz allows
    Given Soqrates runs in Blocking mode
    And the author will answer correctly at the terminal
    When "email:jan@example.com" runs the <hook> hook on a change touching "rust.ownership", "rust.traits"
    Then the hook allows it
    And a passed result was reported to the author

    Examples:
      | hook       |
      | pre-commit |
      | pre-push   |

  @adr-0001-d9 @wip
  Scenario Outline: An empty change never asks any questions
    Given Soqrates runs in Blocking mode
    And the author will answer wrongly at the terminal
    When "email:jan@example.com" runs the <hook> hook on <change>
    Then the hook allows it
    And the author was asked 0 questions at the terminal
    And the history of "email:jan@example.com" has 0 graded answers

    Examples:
      | hook       | change                            |
      | pre-commit | an empty change                   |
      | pre-push   | an empty change                   |
      | pre-commit | a change with only whitespace     |
      | pre-push   | a change with only whitespace     |

  @adr-0001-d9 @wip
  Scenario Outline: A hook updates the author's learner profile
    Given the author will answer correctly at the terminal
    When "email:jan@example.com" runs the <hook> hook on a change touching "rust.ownership", "rust.traits"
    Then "email:jan@example.com" has review cards for exactly "rust.ownership", "rust.traits"
    And the history of "email:jan@example.com" has 2 graded answers

    Examples:
      | hook       |
      | pre-commit |
      | pre-push   |

  @adr-0001-d9 @wip
  Scenario: pre-commit and pre-push behave the same
    Given Soqrates runs in Blocking mode
    And the author will answer wrongly at the terminal
    When "email:jan@example.com" runs the pre-commit hook on a change touching "rust.ownership", "rust.traits"
    And "email:piet@example.com" runs the pre-push hook on a change touching "rust.ownership", "rust.traits"
    Then both hooks made the same decision
    And both hooks asked the same questions

  @adr-0001-d9 @wip
  Scenario: A hook asks due reviews before the concepts of the change
    Given the author will answer correctly at the terminal
    And "email:jan@example.com" ran the pre-commit hook on a change touching "rust.ownership"
    When 30 days pass
    And "email:jan@example.com" runs the pre-push hook on a change touching "tokio.select"
    Then the author was asked about "rust.ownership", "tokio.select", in that order
