# KT-661 — Long discussion performance audit

## Scope and evidence

This is a source audit of the discussion detail opening, transcript rendering,
scrolling, streaming, and in-transcript search paths. It contains no user-room
transcript, production-code change, database/configuration change, or runtime
benchmark.

The source tree has no representative synthetic small/medium/2,000+-message
performance fixture or benchmark identified in this audit. Consequently, there
are no timing, memory, or rendering-FPS claims in this document; source cost
alone does not establish a user-visible duration.

## Observed paths and count-dependent work

### Opening and refresh

1. `GET /api/discussions/{id}` invokes `get_discussion`, then additionally
   obtains active dispatches, a partial response, and the message-target map.
   [src: file: backend/src/api/discussions/crud.rs:92-131#load_detail]
2. `get_discussion` loads every message for the selected discussion and derives
   both message counters by iterating that returned array.
   [src: file: backend/src/db/discussions.rs:791-801#list_messages]
3. `list_messages` selects the message content and other message fields for one
   discussion, ordered by `sort_order, timestamp`; it has no `LIMIT` or
   pagination clause. [src: file: backend/src/db/discussions.rs:2591-2658#list_messages]
   The `(discussion_id, sort_order)` unique index supports the first ordering
   key. [src: file: backend/src/db/sql/082_message_sequence.sql:14-15#idx_messages_discussion_sort_order_unique]
4. The frontend polls `GET /api/discussions/{id}/poll` on active-discussion
   changes, immediately and then every five seconds.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:1290-1298#fetchActive]
   [src: file: frontend/src/pages/DiscussionsPage.tsx:1091-1124#reloadDiscussion]
   The server builds the complete detail on every poll and omits it only when
   its revision matches the one the client holds; a changed response replaces
   that discussion's cached detail object.
   [src: file: backend/src/api/discussions/crud.rs:195-225#poll]
   [src: file: frontend/src/pages/DiscussionsPage.tsx:1063-1064#reconcileLoadedDiscussion]
5. The detail response also queries all message targets belonging to the
   discussion and groups them in a map.
   [src: file: backend/src/db/discussions.rs:3059-3103#list_discussion_message_targets]
   This means the refresh payload and
   server-side deserialization can grow with both messages and addressed
   targets, independently of visible transcript rows.

### Transcript render and interaction

1. The active transcript is rendered as the complete `activeDiscussion.messages`
   collection inside one scrolling container; this code does not window or
   paginate visible message rows.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:4797-4817#transcriptElements]
   Each normal item creates a
   `MessageBubble`; tool groups create `ToolCallsGroup`.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:4132-4190#MessageBubble]
2. Before mapping rows, the page filters notes, projects messages into
   conversation order and builds lookup maps, memoized per transcript change;
   the memoized element list then makes several more passes to calculate
   metadata and reply/turn anchors.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:3397-3414#transcriptIndex]
   [src: file: frontend/src/pages/DiscussionsPage.tsx:3955-4038#transcriptElements]
3. The conversation-order projection maps every message and sorts the resulting
   array; resolving a reply chain can walk up to 16 links for an eligible row.
   [src: file: frontend/src/lib/messageTargets.ts:319-367#messagesInConversationOrder]
   Thus the sort is a
   demonstrated count-dependent operation, while its user-visible cost still
   requires measurement.
4. A `MessageBubble` passes normal text through `MessageBody`, which splits
   injected context and sends the content to `ReactMarkdown` with
   `remark-gfm` and `remark-emoji`.
   [src: file: frontend/src/components/MessageBubble.tsx:2295-2343#MessageBody]
   [src: file: frontend/src/components/MessageBubble.tsx:2246-2259#ReactMarkdown]
   Each message first renders as plain text and upgrades to Markdown once it is
   near the viewport or the browser is idle, so every message is still parsed.
   [src: file: frontend/src/components/MessageBubble.tsx:2196-2228#canDefer]
   There is a per-message
   200,000-character fallback that avoids this Markdown path for very large
   individual messages; it does not cap the count of ordinary messages.
   [src: file: frontend/src/components/MessageBubble.tsx:2005-2013#MAX_MARKDOWN_CHARS]
5. In-transcript search walks each rendered message row, then scans each row's
   rendered text nodes. For every match, the range helper locates start and end
   nodes by linear `find` calls.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:1403-1436#findRenderedTextRanges]
   [src: file: frontend/src/lib/discussionMessageSearch.ts:3-39#findRenderedTextRanges]
6. The active-detail change also triggers an effect that filters the complete
   array to mark the discussion seen.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:2086-2100#markDiscussionSeen]
   The code-count refresh
   effect is similarly keyed to the message-array length.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:2235-2262#setPendingFilesCount]

### Scrolling and streaming

1. On initial settling of a loaded discussion, `scrollToBottomSettled` writes
   `scrollTop = scrollHeight` on animation frames for up to 700 ms; the effect
   is reached after a non-empty message list loads.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:2158-2197#bottomSettleRafRef]
   [src: file: frontend/src/pages/DiscussionsPage.tsx:2240-2254#lastSettledDiscIdRef]
2. For streaming content while stuck to the bottom, the page calls
   `scrollIntoView` no more often than once per 250 ms.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:2207-2217#lastScrollRef]
3. Incoming stream chunks are buffered and flush at most once per animation
   frame into the discussion streaming map.
   [src: file: frontend/src/hooks/useRafBatchedStream.ts:20-49#useRafBatchedStream]
   The active streaming text is
   deferred before being passed to the streaming reply renderer.
   [src: file: frontend/src/pages/DiscussionsPage.tsx:1697#deferredStreamingText]
   [src: file: frontend/src/pages/DiscussionsPage.tsx:4049-4057#deferredStreamingText]

## Findings and priorities

| Priority | Classification | Evidence-backed finding | Recommendation / risk |
| --- | --- | --- | --- |
| P0 | Observed | Every active-detail refresh loads the complete transcript server-side and returns all of it whenever anything changed, and active rooms refresh every 5 seconds. | I recommend testing an incremental or paged detail contract first. Risks: preserving bottom anchoring, global/in-room search result navigation, reply links, tool-group boundaries, and a stream that becomes durable between pages. |
| P0 | Observed | The DOM contains a row projection for the full message list, with Markdown processing per ordinary message. | I recommend evaluating anchored windowing/virtualization only behind parity tests. Risks: variable Markdown/media heights, focus and screen-reader semantics, search highlights for unmounted rows, date separators, collapsed tool cards, and `scrollIntoView` targets. |
| P1 | Observed | Each transcript change recomputes collection projections and maps; the ordering projection sorts the message collection. | I recommend profiling the existing memoized derived transcript data after a baseline. Risk: stale maps or incorrect order when a late reply belongs to an older turn. |
| P1 | Observed | In-transcript search scans rendered DOM across all mounted rows and builds ranges. | I recommend keeping server search as the cross-room search path and measuring a bounded/visible-window in-room highlighting design. Risk: Markdown-visible text and cross-node matches must remain correct. |
| P2 | Observed mitigation | Streaming updates are animation-frame batched, the Markdown input is deferred, and stream auto-scroll is throttled. | I recommend retaining these behaviours in every experiment. Risk: changing batching or anchoring can produce dropped-looking output, delayed text, or unwanted scroll jumps. |

The priority labels are recommendations based on demonstrated count-dependent
work and functional blast radius, not measured latency rankings.

## Validation plan before and after any implementation

Create an isolated, deterministic fixture that contains no real discussion
content. Run each scenario in a fresh browser/profile and record browser
version, hardware/OS, build revision, repetitions, medians, and spread.

| Fixture | Shape | Repetitions and observations |
| --- | --- | --- |
| Small | 20 synthetic messages, short plain and Markdown bodies | At least 10 cold opens and 10 active refreshes; capture request bytes, detail parse/commit timing, long-task count, DOM-row count, heap, and bottom-anchor correctness. |
| Medium | 200 synthetic messages, deterministic mix of user/agent/system, tool groups, replies, and date boundaries | Same collection, plus in-room search and navigation to an early, middle, and late result. |
| Long | 2,000+ synthetic messages, same deterministic shape; include ordinary Markdown, collapsed tools, a bounded number of attachments/cards, and reply chains no deeper than the supported 16-hop traversal | Same collection, plus cold-open settle, scroll from top/middle/bottom, streamed response arrival while at bottom and while reading history, and resize/font/image late-layout checks. |

For every candidate change, retain the following acceptance checks:

- Initial open reaches the latest message when no explicit search target is
  active, and does not override a user who has scrolled away.
- A streamed reply remains visible in order, is not duplicated by the durable
  reload, and does not force-scroll a reader of older rows.
- Searching rendered Markdown finds text across adjacent text nodes and moves to
  hits whether the hit is initially mounted or not.
- Tool groups, date separators, reply/backlink navigation, cards, and note
  visibility preserve their present relative order.
- Compare small, medium, and long fixture metrics before/after; reject a change
  that improves the long fixture by regressing the small/medium interaction or
  any acceptance check.

## Limits

- No timing is claimed: no source-level reading can substitute for browser,
  database, and payload measurements on a representative fixture.
- This audit did not open, read, export, or copy a user discussion transcript.
- This audit did not run a Rust build, start/restart a provider or MAIN, modify
  production/test/gate code, or add an optimization to a release.
