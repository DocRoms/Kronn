# Orchestration messages — typed kind, fold, unread (KT-1330)

A campaign room fills with what the orchestrator posts itself: review requests,
`Delivery accepted` summaries, terminal events, integration bounces, timeouts,
worker-undelivered alerts. They are `User`/`System` messages authored as
`Orchestrateur`, so the role cannot tell them from a human's.

## The typed signal

`messages.kind` (migration `268_message_kind`, model `MessageKind` in
`backend/src/models/discussions.rs`) is `orchestration` when the orchestrator
posted the message. It is set where the backend posts: `orchestrator_message()`
and `build_control_offer_message()` in `api/orchestration.rs` (which also covers
`api/delivery_publication.rs`), and the room posts in `db/orchestration.rs`
(terminal, timeout, integration refusal, campaign gate, worker return).

`NULL` means an ordinary message **or** a row stored before the column existed.
Readers resolve it with `MessageKind::resolve`: a stored value wins; an unset
one falls back to the legacy id prefix (`orch-…`, `delivery-summary:…`). So old
rooms fold with no data migration. The field is omitted on the wire while
`standard`, and exported to `frontend/src/types/generated.ts` via `make typegen`.

## Display only

The kind never filters anything server-side. `list_messages`, the bridge, the
MCP read paths and prompt construction keep every message; the agents see what
they always saw.

## The fold (frontend)

`Hide orchestration messages` sits beside `Hide notes` in the composer toolbar
(`ChatInput`), persisted like it: a global `localStorage` flag
`kronn:showOrchestrationMessages` (synced via `SYNCED_KEYS`, default shown,
`aria-pressed` = shown). Each run of consecutive hidden messages becomes one
`OrchestrationFoldMarker` (`N orchestration message(s) hidden — show`). The pure
rules are in `frontend/src/lib/orchestrationFold.ts`.

Never folded: a message with a `kronn-question` fence, a campaign gate parked on
a human (`orch-campaign-gate:…:awaiting:…`), a message with an open discussion
action, and the message a search hit or deep link points at. Notes, tool-call
groups, human and agent messages end a run; so does a change of calendar day.

## Unread

Hidden — or shown — orchestration messages do **not** raise the unread badge.
The badge basis is `non_system_message_count`, which now excludes both
`System` rows and orchestration ones (`DiscussionMessage::counts_as_unread`, and
the matching SQL in `DISC_SELECT_COLS`). The fold is a per-browser preference
while the count is server-side, so excluding them always keeps the badge
consistent between browsers and toggle states. Trade-off: a room whose only news
is an orchestration message shows no badge; a gate that needs a human is still
announced by its steering card.
