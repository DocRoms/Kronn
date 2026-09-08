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

Only the orchestrator's own session or a human can publish. A worker cannot
create, prepare or publish a card — including by calling the ingest path
directly with a forged `author_kind`, an empty session, or a different target
room. Authority is resolved from the CALLER's own durable identity, never from
a field the payload claims:

- `disc_append` (the CLI/bridge channel) resolves the caller's session by
  `session_id` alone — no `disc_id` filter, so a worker's session is found and
  refused even when it targets its parent room, and no gate on "one live Agent
  message" so a bulk import can't slip past by shape. An identity this cannot
  verify (missing session, one that no longer resolves) refuses; there is no
  legitimate "unknown caller" case on this endpoint, and no legitimate human
  turn either.
  [src: file: backend/src/api/disc_source.rs:749-825]
  [src: file: backend/src/db/discussion_important.rs:458-502]
- `send_message` (the authenticated human composer) is the one legitimate
  `Human` publisher — this endpoint has no session or claimed role to spoof,
  so it constructs `ImportantPublisher::Human` directly.
  [src: file: backend/src/api/discussions/messaging.rs:671-697]

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
filter, and previous/next navigation that scrolls the transcript to a card
without re-rendering or reordering it, so the reader's scroll position is the
only thing that moves. `ImportantMessageCard` renders the DURABLE row for a
`kronn-important` fence in the transcript, never the fence text itself; a
fence that produced no row (refused or malformed) says so instead of
rendering raw JSON.
[src: file: frontend/src/components/ImportantMessageCard.tsx:1-204]
[src: file: frontend/src/lib/importantMessages.ts:1-98]

The bar/card store is refreshed on the WebSocket `chat_message`/
`message_revised` event and on the human composer's own accepted receipt —
both the transcript reload and the important-messages store need their own
invalidation, since they are separate caches.
[src: file: frontend/src/pages/DiscussionsPage.tsx:1543-1554]
[src: file: frontend/src/pages/DiscussionsPage.tsx:2706-2729]

## Known limitations

- A bulk `disc_append` (several messages in one call) reports only the LAST
  message's `ImportantIngest` in the response; earlier messages' outcomes are
  not summed into it. The database is still authoritative — nothing is lost —
  but a caller reading only the response for a multi-message append could
  under-count refusals/publications from earlier messages in the same call.
- The onboarding tour's `important-messages` step is `optionalWhenMissing`:
  the seeded demo discussion carries no card, so a brand-new user skips it
  rather than seeing a live example.
