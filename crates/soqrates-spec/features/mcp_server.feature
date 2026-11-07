@adr-0001-d10
Feature: The core is exposed as an MCP server
  soqrates-mcp exposes the core through MCP with exactly these tools:
  start_session, submit_answers, get_learner_profile and due_reviews. Other
  harnesses get the same learner profiles and scheduling as the Soqrates agent.

  @adr-0001-d10 @wip
  Scenario: The MCP server exposes exactly the four core tools
    When I list the MCP tools
    Then the MCP tools are exactly:
      | tool                |
      | start_session       |
      | submit_answers      |
      | get_learner_profile |
      | due_reviews         |
    And no MCP tool is listed twice

  @adr-0001-d10 @wip
  Scenario Outline: Each core operation is reachable as an MCP tool
    When I list the MCP tools
    Then the MCP server exposes the tool "<tool>"

    Examples:
      | tool                |
      | start_session       |
      | submit_answers      |
      | get_learner_profile |
      | due_reviews         |

  @adr-0001-d10 @wip @blocked
  Scenario: An MCP client gets the same learner profile as the engine
    Given "github:1001" has answered questions about "rust.ownership" correctly
    When an MCP client calls "get_learner_profile" for "github:1001"
    Then the MCP result matches the engine's profile of "github:1001"
