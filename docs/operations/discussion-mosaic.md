# Discussion mosaic

Select **2 to 12 discussions** from the discussion sidebar's selection menu,
then choose **Open in a mosaic**. The new tab stores its ordered selection and
layout in the URL (`#discussions/mosaic?discussion=…&discussion=…&layout=…`).
Two and three tiles offer the same layout presets as the Artifact mosaic; larger
selections use the automatic grid. On narrow screens the tiles stack vertically.

Each tile links to the full discussion and shows the plan's completed/active
counts, in-progress, ready, blocked and idea buckets, plus the separate later
count. Counts follow the same status and active-blocker precedence as the full
plan. They are aggregated in SQLite without loading task descriptions or DoD.
Message author, CLI ordinal and model labels come from stored message metadata.
A configured native agent in the header is not evidence that a joined CLI is
currently working. No activity is inferred from CLI presence.

The monitor itself is read-only. It does not launch or resume agents,
acknowledge room cursors, or update the discussion unread state. Markdown
proposals remain text; the preview has no action cards or iframe and omits image
fetching. Open the full discussion to interact with its contents.

## Writing to one tile

Clicking a tile (or its reply button) selects it: the tile gets an accent
outline and the collapsible input at the bottom names it. Nothing can be sent
without a selection. The input is the discussion's own `ChatInput`, with its
mentions, `@all`, joined CLIs and per-discussion draft; changing tile keeps
each draft in its discussion. The selected discussion is loaded once on
selection, and the input appears only when that exact record is loaded.
Each message first reaches the same local outbox as the discussion page, with
a stable UUID; the draft is cleared at that point. The outbox then sends it with
`defer_dispatch`, so the server persists it and runs the agent after the current
run, and closing the tab does not cut the reply. A failed send stays listed
under the input and retries with the same UUID (the server deduplicates it);
it can be retried now or discarded. An entry still waiting when another tile is
selected resumes when its discussion is open again, here or in the full page.
Notes are sent on their own channel, never through the dispatch outbox, and
are restored as a draft if refused. Voice conversation, read-aloud, debates and
`kronn-important` publication stay in the full discussion and are hidden here.

## Bounded refresh contract

`GET /api/discussions/monitor?ids=<comma-separated ids>` accepts 1–12 unique
IDs (maximum 128 bytes each and 4096 bytes of selection). It returns one
`DiscussionMonitorItem` per selected ID, in selection order. A missing or
unreadable discussion has its own `not_found` / `unavailable` error; other tiles
continue loading. Global failures preserve the previous preview with a visible
refresh error.

Every successful preview includes at most 8 messages, each limited to 2048
Unicode scalar values, plus the **last 4096 characters** of a persisted partial
response. The partial response is a saved checkpoint, not a token stream or a
promise that an agent is still running. Truncation is visible. Full transcripts,
attachments and task details are never hydrated by this endpoint.

The view makes one batch request at a time. It shares the application's single
WebSocket connection and coalesces events for selected discussions, refreshing
within 250 ms without allowing a continuous stream to postpone the request.
A 5-second scoped poll also catches persisted checkpoints and CLI messages;
requests time out after 15 seconds. Hidden tabs pause polling, visibility and
reconnection catch up, and unmount cancels the current request and timers.
Each tile follows its newest messages independently; scrolling upward pauses
following until the user returns to the bottom or uses the follow button.

## Validation

API integration tests exercise oversized content, Unicode, duplicate/missing/
deleted/corrupt rooms and 48 combinations of plan status, placement and blocker
state against the full-plan projection. Frontend tests cover URL navigation,
selection limits, abort/visibility/reconnect/throttling, plan rendering, scroll
independence and separation from comparison actions. Browser qualification uses
synthetic responses; it does not replace the final real-data performance audit.
