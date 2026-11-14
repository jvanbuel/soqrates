# ADR-0003: Learner identity across forges

- **Status:** Accepted
- **Date:** 2026-11-07
- **Refines:** ADR-0001 D5 (state keyed by author), ADR-0002 (glossary,
  `LearnerProfile`)

## Context

So far a learner profile is keyed by the identity that authored a change:
`github:12345`, or `email:<address>` for local hooks. In practice one person
often has several identities, and each gets its own profile:

> **Example: a consultancy.** Mira works for a consultancy. She has a
> GitHub account in the consultancy's organisation for internal work, and a
> GitLab account on a client's self-hosted GitLab for her current
> engagement. Her local commits use `mira@consultancy.example` in one
> repository and `mira.ext@client.example` in the other.

With one profile per identity, Soqrates treats Mira as two novices. It asks
her again about concepts she already knows, it schedules the same concept
twice, and her level drifts apart between forges. Spaced repetition only
works if all of her reviews land on one schedule.

Three forces make this more than a lookup table:

1. **Trust.** A commit e-mail is whatever `git config user.email` says, so
   anyone can claim it. Usernames can be renamed and then reused by someone
   else. If linking were automatic, someone could read, or pollute, another
   person's profile. A profile is sensitive: it lists what someone does
   *not* know.
2. **Confidentiality across organisations.** The client must not learn
   anything about the consultancy's code, and the consultancy must not
   receive the client's code. Concept IDs such as
   `client.billing.invoice-rounding`, repository names, question text and
   answers can all leak proprietary information.
3. **Where state lives.** Each forge's CI may use its own store, for example
   a state branch in the client's repository. Linking identities is only
   useful if both forges reach the same profile.

## Glossary additions

This ADR adds the terms Learner, Identity, Verification, Link, Link code,
Learner store and Concept scope to the [ADR-0002 glossary](0002-domain-model.md#glossary),
and narrows "Author" to the identity behind a change.

## Decisions

### D1: Profiles belong to learners, not to identities
**Enforcement:** automated

A learner profile is keyed by an opaque, generated `LearnerId`. An identity
is the forge host plus the forge's immutable account ID (e.g.
`github.com:12345`, `gitlab.client.example:678`), or an e-mail address.
Usernames never identify anyone, because they can be renamed and reused.

The first time Soqrates sees an identity, it creates a new learner linked
to it. After that, the identity always resolves to that learner, even when
the account's username changes.

### D2: Linking requires proof of control of both identities
**Enforcement:** automated

There are two ways to link identities:

- **By the learner**, with a link code. The learner asks for a code while
  acting as identity A (CLI, MCP or a PR comment command), then presents it
  while acting as identity B. A code is single-use, expires after 15
  minutes, and is rejected if presented by the identity that requested it.
- **By an administrator**, through declared mappings in the Soqrates
  configuration of an organisation. This is allowed only between identities
  on forges that the organisation administers. A consultancy can map its
  own GitHub and its own GitLab, but not the client's GitLab.

Nothing else links identities. In particular, matching commit e-mails,
usernames or display names never links them. An unverified identity can be
the second half of a learner-initiated link only, because presenting the
code from it is the proof.

An identity has at most one link. Because every identity gets a learner
when it is first seen (D1), linking works like this:

- If the identity is the only identity of its learner, that learner is
  merged into the other one (D3).
- If its learner has other identities too, the link is rejected. The
  identity must be unlinked first.

### D3: Linking merges by replaying reviews
**Enforcement:** automated

When two learners are linked into one, their reviews are merged in time
order and replayed through `LearnerProfile::record` (ADR-0002 D3),
starting from the lower of the two declared starting levels. The
resulting review cards and level are exactly what they would have been if
every review had happened on one profile. This decision reuses the domain
rules; it does not add merge rules of its own.

Each review remembers the identity it came through. Unlinking an identity
splits the profile by replaying each part separately, so unlinking undoes a
link.

### D4: Linked learners need a shared learner store
**Enforcement:** automated

Identities are resolved through the `StateStore`. Profiles of linked
learners live in a learner store that the CI of each linked forge (and the
local hooks) are configured to use. Who owns the learner store is a
deployment choice, and Soqrates supports both:

- **Per learner:** the learner owns it, for example a private repository
  or a personal hosted space, and keeps it when they change employer.
- **Per organisation:** an organisation, such as a consultancy, runs one
  learner store for its people.

The domain does not depend on the choice. Where a forge's CI cannot reach the
learner store, Soqrates falls back to a profile local to that forge, says
so in the check output, and never fails the check because of it.

The store router (`app/stores`) is the one place that implements this
fallback: it falls back for placing a learner, and for looking a session,
grading journal or graded record up across the stores.

### D5: Only general concepts cross organisations
**Enforcement:** automated

The examiner gives every concept a `ConceptScope`. A profile in the learner
store, reachable from more than one organisation, stores reviews of
`General` concepts only. Reviews of `Project` concepts stay in the store of
the organisation whose code they came from. The learner store never
contains repository names, diffs, question text, answers or PR references.
It holds concept IDs, grades, formats, timestamps and the identity each
review came through, and nothing else.
Code snippets shown with questions are question text: they never reach
it either (ADR-0007 D6).

A `Project` concept is never reclassified as `General`, not even when an
engagement ends.

### D6: The learner is in control
**Enforcement:** automated

Linking is opt-in. A learner can list their identities, unlink any of them
(D3) and delete their profile. Deleting removes every review and every
link. Other people, including PR reviewers, see a check's outcome but never
another learner's profile, links or identities.

### D7: Local hooks use a configured identity
**Enforcement:** automated

A local hook resolves the learner from the identity configured for the
repository's remote host (`soqrates identity use gitlab.client.example:678`).
It falls back to the commit e-mail as an unverified identity. Two
repositories with different commit e-mails resolve to the same learner if
their configured identities are linked.

## Consequences

- One person, one schedule: reviews on every forge feed the same FSRS
  cards and the same level, which is the point of spaced repetition.
- Merging is replaying (D3), so it needs no extra domain rules, and an
  unlink can be undone and redone deterministically. The cost is that
  reviews have to be stored with their time and identity (they already
  are, apart from the identity).
- Concept scope (D5) changes the `Examiner` contract: it must classify
  concepts. A misclassified project concept would leak an identifier, so
  the examiner should err towards `Project`.
- The learner store is a new piece of infrastructure to run. The fallback
  to per-forge profiles (D4) keeps Soqrates usable without it.
- Supersedes ADR-0001 D5's "keyed by a stable `AuthorId`": the key is now
  `LearnerId`, and `AuthorId` becomes an `Identity`.

## Resolved questions

- *Who owns the learner store?* Either the learner or an organisation,
  configurable per deployment (D4).
- *Can a project concept be kept after an engagement by reclassifying it?*
  No, never (D5).
