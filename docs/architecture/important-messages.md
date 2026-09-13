# Important messages (steering cards)

An important message is a persisted, indexed object distinct from a message's
text — `message_kind: important` in the contract, `discussion_important_messages`
in storage — not a formatting convention, a colour or an emoji. Publication
mints a NEW message and its card row together, in one transaction; no service
path attaches a card to a message that already exists, so a `delivery_summary`
never becomes an important card retroactively.
[src: file: backend/src/db/sql/172_discussion_important_messages.sql:1-53]
[src: file: backend/src/db/discussion_important.rs:263-328]

## The template

A publisher submits a bounded `ImportantSpec` (a `kronn-important` fence, the
same mechanism as `kronn-question`): a closed `category` (`decision`,
`scope_change`, `dod_waiver`, `blocking_alert`, `human_action_required`,
`accepted_delivery`), a `dedup_key`, `title`, `highlight`, optional `context`,
`impact`, `action_required` (`required` plus `action`/`owner`/`due`, or an
explicit "none"), and `references` (task, DoD, execution, agent, commit,
artifact — every field optional). `author`, `created_at` and `schema_version`
are NOT accepted from the payload — `#[serde(deny_unknown_fields)]` fails the
whole parse if a caller tries to inject them; Kronn derives them server-side.
[src: file: backend/src/db/discussion_important.rs:34-161]

## Who can publish

**The contract lives in
[`important-message-publication-authority.md`](important-message-publication-authority.md)**,
decided 2026-09-09. In short: a card needs an enrolled grant and a single-use
proof, both held rather than named, and neither reachable by inviting, joining,
transferring or rebinding.

Two consequences worth carrying here:

- **an install with no grant publishes no card IN ANYBODY'S NAME.** That is
  every install today, and it is deliberate. It does not mean a silent product:
  Kronn's own orchestration events — a failed/cancelled execution, a review that
  requests changes, a campaign parked on a human — publish with no credential at
  all, because there is no caller to authenticate. The gate is on publication in
  one's own name, never on the backend reporting what it just did;
- **the bootstrap is rotatable and recoverable.** An operator who still holds it
  rotates through `POST /api/human-credentials/admin/rotate` (which answers with
  a path, never a secret); one who lost it leaves a `recover-admin-secret` file
  in the private directory and restarts;
- **a worker never publishes**, even holding a valid grant and a valid proof —
  a principal delegated afterwards is refused while it is working, read from its
  exact active assignment rather than from the room it sits in.

What it proves is an API identity, not physical presence, and OS-level theft of
a secret is outside the guarantee by decision.

## Distinction from `delivery_summary`

A worker's accepted delivery report stays a `delivery_summary` message. If the
orchestrator judges a fact in that delivery important, it publishes a
SEPARATE important card that references the delivery (via `references`)
instead of mutating the delivery message's type. The note channel
(`channel: "note"`) is excluded from important-card ingestion entirely — a
note is a private aside, not a steering event.

An ordinary `Done` transition keeps its durable notification without minting
an `accepted_delivery` card automatically. That category is an explicit
publication decision, not an automatic promotion of every accepted report.

## Deduplication and replay

Publication is keyed by `dedup_key`, the stable identity of the fact being
reported (not the message id). A structured event that fires again on restart
or resume collapses onto the existing row instead of creating a duplicate
card; `ingest_message_important` reports `deduplicated` back to the caller so
this is visible, not a silent no-op.
[src: file: backend/src/db/discussion_important.rs:561-610]

Only the FIRST `kronn-important` fence in a message publishes; extras are
refused (`refused_extra`) rather than silently collapsed by the unique index —
one card is one message. A fence nested inside a fenced example (for
instance, one showing the format) never publishes.

## Frontend: filter, counter, navigation

`ImportantMessagesBar` renders a counter (`total_all`, the discussion-wide
count — it does not drop while a category filter is active), a category
filter, and previous/next navigation. Jumping to a card goes through the
transcript's own shared navigation callback (`onNavigate`, the same
`handleReplyNavigate` a reply-jump uses), so it releases stick-to-bottom and
highlights the target the way every other jump in the page does, instead of
reimplementing scroll/focus against a raw `querySelector`. The counter is
itself a button: with one card, or a filter narrowed to one, both arrows are
disabled and the counter is the only way left to reach it.
[src: file: frontend/src/components/ImportantMessageCard.tsx:116-236]

`ImportantMessageCard` renders the DURABLE row for a `kronn-important` fence
in the transcript, never the fence text itself; a fence that produced no row
(refused or malformed) says so instead of rendering raw JSON.

Server-authored orchestration events carry no Markdown fence. Their durable
card is rendered beside the folded report, visible without opening its details.
Only a matching row with `source_kind: orchestration` enables that path; an
orchestrator display label or ordinary notification alone renders no card and
no false refusal notice.
[src: file: frontend/src/components/ImportantMessageCard.tsx:1-114]
[src: file: frontend/src/lib/importantMessages.ts:1-98]

The bar re-fetches when the transcript's newest durable message id changes
(`messageRevision`, the same idiom `DiscussionQuestionBanner` uses) — one
fetch per arrival, not one per render or per bubble, and never stuck on the
first GET while a new card lands.
[src: file: frontend/src/components/ImportantMessageCard.tsx:156-162]
[src: file: frontend/src/pages/DiscussionsPage.tsx:4052-4059]

## Tests

- Backend model, dedup/replay/restart, categories: `backend/src/db/discussion_important/tests.rs`.
- Authorization at the real handler (role/System bypass, bulk import, missing/invalid session, parent-room targeting): `backend/src/api/disc_source.rs` (`disc_append_*` tests) and `backend/src/api/discussions/messaging.rs` (`send_message_does_not_publish_an_unverified_human_card`).
- Frontend behaviour (filter, counter, navigation, refetch-on-arrival): `frontend/src/components/__tests__/ImportantMessageCard.test.tsx`.
- E2E layout (no horizontal overflow, click targets, keyboard focus ring) at phone and desktop widths: `frontend/e2e/specs/important-message-card-layout.spec.ts`.

## Known limitations

- **What the authority proves is an API identity, not a person.** A `human`
  grant is held by whoever holds its secret. The arbitration that chose this
  (`kt619-human-credential-bootstrap` → `dedicated-human-credential`) excluded
  OS-level secret theft and direct database writes from scope, so an operator
  whose grant file is readable has given it away and nothing here notices.
- **Every existing install publishes nothing IN A CALLER'S NAME until it is
  bootstrapped.** No grant exists anywhere, and there is no trust-on-first-use
  to fall back on. Deliberate, and a real behaviour change rather than a silent
  tightening. Server-authored steering cards are unaffected and need no
  bootstrap.
- A bulk `disc_append` (several messages in one call) reports only the LAST
  message's `ImportantIngest` in the response; earlier messages' outcomes are
  not summed into it. The database is still authoritative — nothing is lost —
  but a caller reading only the response for a multi-message append could
  under-count refusals/publications from earlier messages in the same call.
- The onboarding tour's `important-messages` step is `optionalWhenMissing`:
  the seeded demo discussion carries no card, so a brand-new user skips it
  rather than seeing a live example.
