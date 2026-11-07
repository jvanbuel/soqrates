@adr-0003-d1
Feature: One learner, many identities
  A learner can show up under several identities: an identity on the
  consultancy's GitHub, another on a client's self-hosted GitLab, and
  different commit e-mails in local repositories. All their reviews must
  land on one learner profile, so that spaced repetition and the level work
  across forges. Identities are linked only on proof, and nothing
  confidential crosses from one organisation to another.

  Background:
    Given the forge "github.com" is administered by organisation "consultancy"
    And the forge "gitlab.consultancy.example" is administered by organisation "consultancy"
    And the forge "gitlab.client.example" is administered by organisation "client"
    And all forges use the same learner store

  # --- D1: profiles belong to learners -------------------------------------

  @adr-0003-d1 @wip
  Scenario: The first change from an unknown identity creates a new learner
    When identity "github.com:1001" opens a pull request touching "rust.async.cancellation"
    Then identity "github.com:1001" belongs to a new learner
    And that learner's level is Novice

  @adr-0003-d1 @wip
  Scenario: An identity keeps its learner when the username changes
    Given identity "github.com:1001" with username "mira" belongs to learner "Mira"
    When the username of identity "github.com:1001" changes to "mira-k"
    And identity "github.com:1001" opens a pull request touching "rust.traits"
    Then the session is for learner "Mira"

  @adr-0003-d1 @wip
  Scenario: A reused username does not inherit the old learner
    Given identity "github.com:1001" with username "mira" belongs to learner "Mira"
    And identity "github.com:1001" is renamed to "mira-k"
    When a new identity "github.com:2002" takes the username "mira" and opens a pull request
    Then identity "github.com:2002" belongs to a new learner

  @adr-0003-d1 @wip
  Scenario: Unlinked identities have separate learner profiles
    Given identity "github.com:1001" belongs to learner "Mira"
    When identity "gitlab.client.example:678" opens a pull request touching "rust.async.cancellation"
    Then identity "gitlab.client.example:678" belongs to a new learner
    And learner "Mira" has no review card for "rust.async.cancellation"

  # --- D2: linking requires proof -------------------------------------------

  @adr-0003-d2 @wip
  Scenario: A learner links two identities with a link code
    Given identity "github.com:1001" belongs to learner "Mira"
    And identity "gitlab.client.example:678" belongs to learner "Mira at client"
    When identity "github.com:1001" requests a link code
    And identity "gitlab.client.example:678" presents that link code
    Then identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner

  @adr-0003-d2 @wip
  Scenario Outline: A link code is rejected unless it is fresh, unused and presented by the other identity
    Given identity "github.com:1001" belongs to learner "Mira"
    And identity "github.com:1001" requested a link code
    And <circumstance>
    When identity "<presenter>" presents that link code
    Then the link is rejected because the code is <reason>
    And identity "github.com:1001" is linked only to itself

    Examples:
      | circumstance                                                     | presenter                 | reason                |
      | 16 minutes have passed                                           | gitlab.client.example:678 | expired               |
      | identity "gitlab.client.example:999" already presented that code | gitlab.client.example:678 | already used          |
      | nothing else has happened                                        | github.com:1001           | from the same identity |

  @adr-0003-d2 @wip
  Scenario: Matching commit e-mails never link identities
    Given identity "github.com:1001" with verified e-mail "mira@consultancy.example" belongs to learner "Mira"
    When a local commit by "mira@consultancy.example" is checked by the pre-commit hook in a repository without a configured identity
    Then the hook resolves to a learner other than "Mira"
    And identity "email:mira@consultancy.example" is unverified

  @adr-0003-d2 @wip
  Scenario: An administrator links identities on forges their organisation administers
    Given identity "github.com:1001" belongs to learner "Mira"
    When organisation "consultancy" declares that "github.com:1001" and "gitlab.consultancy.example:55" are the same learner
    Then identities "github.com:1001" and "gitlab.consultancy.example:55" belong to the same learner

  @adr-0003-d2 @wip
  Scenario: An administrator cannot link an identity on another organisation's forge
    Given identity "github.com:1001" belongs to learner "Mira"
    When organisation "consultancy" declares that "github.com:1001" and "gitlab.client.example:678" are the same learner
    Then the declaration is rejected because "gitlab.client.example" is not administered by "consultancy"

  @adr-0003-d2 @wip
  Scenario: An identity that already belongs to another learner cannot be linked
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And identity "github.com:3003" belongs to learner "Sam"
    When identity "github.com:3003" requests a link code
    And identity "gitlab.client.example:678" presents that link code
    Then the link is rejected because "gitlab.client.example:678" already belongs to another learner

  # --- D3: linking merges by replaying reviews ------------------------------

  @adr-0003-d3 @wip
  Scenario: Reviews from both forges are merged into one review card
    Given learner "Mira" with identity "github.com:1001" reviewed "rust.async.cancellation" as Good on day 1
    And learner "Mira at client" with identity "gitlab.client.example:678" reviewed "rust.async.cancellation" as Good on day 4
    When the two identities are linked
    Then the review card for "rust.async.cancellation" equals that of one learner profile that recorded both reviews in time order
    And the history contains 2 reviews of "rust.async.cancellation"

  @adr-0003-d3 @wip
  Scenario: Correct streaks on both forges add up to a promotion
    Given learner "Mira" with identity "github.com:1001" had 3 correct reviews on days 1 to 3
    And learner "Mira at client" with identity "gitlab.client.example:678" had 3 correct reviews on days 4 to 6
    When the two identities are linked
    Then the linked learner's level is Intermediate

  @adr-0003-d3 @wip
  Scenario: Unlinking splits the learner profile by identity
    Given learner "Mira" with identity "github.com:1001" reviewed "rust.traits" as Good on day 1
    And learner "Mira at client" with identity "gitlab.client.example:678" reviewed "tokio.select" as Again on day 2
    And the two identities are linked
    When identity "gitlab.client.example:678" is unlinked
    Then the learner of "github.com:1001" has review cards for exactly "rust.traits"
    And the learner of "gitlab.client.example:678" has review cards for exactly "tokio.select"

  @adr-0003-d3 @wip
  Scenario: A due review is asked on whichever forge the learner works next
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And identity "github.com:1001" answered a question about "rust.async.cancellation" wrongly
    When 30 days pass
    And identity "gitlab.client.example:678" opens a pull request touching "serde.derive"
    Then the first question is about "rust.async.cancellation"

  # --- D4: shared learner store ---------------------------------------------

  @adr-0003-d4 @wip
  Scenario: Without the learner store, a forge falls back to its own profile
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And the forge "gitlab.client.example" cannot reach the learner store
    When identity "gitlab.client.example:678" opens a pull request touching "rust.traits"
    And the Soqrates check runs
    Then the check is Pending
    And the check output says the learner profile is local to "gitlab.client.example"
    And learner "Mira" in the learner store is unchanged

  # --- D5: only general concepts cross organisations ------------------------

  @adr-0003-d5 @wip
  Scenario: A session at the client stores project concepts only at the client
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And the examiner classifies "rust.async.cancellation" as General
    And the examiner classifies "client.billing.invoice-rounding" as Project
    When identity "gitlab.client.example:678" answers questions on pull request "client/billing#42" about "rust.async.cancellation" and "client.billing.invoice-rounding"
    Then the learner store has a review of "rust.async.cancellation" for learner "Mira"
    And the learner store has no review of "client.billing.invoice-rounding"
    And the store of organisation "client" has a review of "client.billing.invoice-rounding"
    And the learner store contains none of "client/billing", "#42", the question text or the answers

  @adr-0003-d5 @wip
  Scenario: A project concept from the client is never asked on the consultancy's forge
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And identity "gitlab.client.example:678" answered a question about Project concept "client.billing.invoice-rounding" wrongly
    When 30 days pass
    And identity "github.com:1001" opens a pull request touching "rust.traits"
    Then no question is about "client.billing.invoice-rounding"

  # --- D6: the learner is in control ----------------------------------------

  @adr-0003-d6 @wip
  Scenario: A learner lists and deletes their data
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    When identity "github.com:1001" lists its learner's identities
    Then the list is exactly "github.com:1001", "gitlab.client.example:678"
    When identity "github.com:1001" deletes its learner profile
    Then no store contains reviews or links for learner "Mira"

  @adr-0003-d6 @wip
  Scenario: Other people never see another learner's identities
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And identity "github.com:1001" has a graded session on pull request "consultancy/app#7"
    When the Soqrates check output for "consultancy/app#7" is shown
    Then it does not mention "gitlab.client.example"

  # --- D7: local hooks use a configured identity ----------------------------

  @adr-0003-d7 @wip
  Scenario: Local repositories with different commit e-mails resolve to the same learner
    Given identities "github.com:1001" and "gitlab.client.example:678" belong to the same learner "Mira"
    And a repository with remote host "github.com", commit e-mail "mira@consultancy.example" and configured identity "github.com:1001"
    And a repository with remote host "gitlab.client.example", commit e-mail "mira.ext@client.example" and configured identity "gitlab.client.example:678"
    When the pre-commit hook runs in each repository
    Then both hooks resolve to learner "Mira"
