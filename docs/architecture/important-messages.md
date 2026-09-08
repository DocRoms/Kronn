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

Today, only the orchestrator's own CLI/bridge session can publish. A worker
cannot create, prepare or publish a card — including by calling the ingest
path directly with a forged `author_kind`, an empty session, or a different
target room. Authority is resolved from the CALLER's own durable identity,
never from a field the payload claims:

- `disc_append` (the CLI/bridge channel) resolves the caller's session by
  `session_id` alone — no `disc_id` filter, so a worker's session is found and
  refused even when it targets its parent room, and no gate on "one live Agent
  message" so a bulk import can't slip past by shape. An identity this cannot
  verify (missing session, one that no longer resolves) refuses; there is no
  legitimate "unknown caller" case on this endpoint, and no legitimate human
  turn either.
  [src: file: backend/src/api/disc_source.rs:749-825]
  [src: file: backend/src/db/discussion_important.rs:458-502]

**Human publication is not yet wired — deliberately.** `send_message` (the
human chat composer) takes no caller identity at all (`State`/`Path`/`Json`
only), and Kronn's whole trust model treats every LOCAL caller as the human
(`auth_middleware`'s loopback bypass exists because a self-hosted instance
assumes "the user is always on the same machine") — a worker calling this
endpoint directly is indistinguishable from the browser. An earlier version
of this lot granted `ImportantPublisher::Human` from this endpoint on the
assumption that "no session/role field to spoof" made it safe; it does not,
because there is no verification of ANY KIND, which is a weaker guarantee
than the one already rejected for `disc_append`. A human decision
(`kt619-human-publication-boundary`, answered `verified-publication`)
confirmed: ship nothing rather than a label-only grant. `send_message` now
logs and refuses every `kronn-important` fence; the fence stays in the
transcript and the card renders as "not recorded" — the same honest failure
mode as a malformed spec.
[src: file: backend/src/api/discussions/messaging.rs:671-693]
[src: file: backend/src/lib.rs:389-453]

A card also attaches only to the message the caller just wrote:
`is_newest_message` checks the message about to receive a card is the newest
in its discussion, so an old ordinary message cannot be converted after the
fact. `UNIQUE(message_id)` alone would only stop a SECOND card on a message
that already had one — it says nothing about the first.
[src: file: backend/src/db/discussion_important.rs:311-328]

**Scope of the guarantee**: this is enforced in the service. A privileged
SQL client writing directly to `discussion_important_messages` is not
constrained by any of it — the `author_kind` CHECK constrains the *value*, not
who chose it.
[src: file: backend/src/db/sql/172_discussion_important_messages.sql:6-13]

## Distinction from `delivery_summary`

A worker's accepted delivery report stays a `delivery_summary` message. If the
orchestrator judges a fact in that delivery important, it publishes a
SEPARATE important card that references the delivery (via `references`)
instead of mutating the delivery message's type. The note channel
(`channel: "note"`) is excluded from important-card ingestion entirely — a
note is a private aside, not a steering event.

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

- **The human half of DoD `ddf34a4e` is not met yet.** The contract asks for
  an explicit human publish path; today there is none — `send_message` refuses
  every `kronn-important` fence rather than ship an unverified one (see
  "Who can publish" above). This is a deliberate, human-approved deferral
  (`kt619-human-publication-boundary` → `verified-publication`), not an
  oversight: Kronn has no existing verified-human primitive distinct from "any
  local caller" to hook into, and inventing a weak one was explicitly
  rejected. A follow-up needs its own design (e.g. a confirmation tied to
  content hash + room + nonce + expiry, or another human-approved mechanism)
  and its own arbitration before implementation.
- A bulk `disc_append` (several messages in one call) reports only the LAST
  message's `ImportantIngest` in the response; earlier messages' outcomes are
  not summed into it. The database is still authoritative — nothing is lost —
  but a caller reading only the response for a multi-message append could
  under-count refusals/publications from earlier messages in the same call.
- The onboarding tour's `important-messages` step is `optionalWhenMissing`:
  the seeded demo discussion carries no card, so a brand-new user skips it
  rather than seeing a live example.
