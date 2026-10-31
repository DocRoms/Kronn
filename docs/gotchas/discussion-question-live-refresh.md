# Arbitration state must remain live

The question store is shared per discussion, including an initially empty
snapshot. New message IDs prompt a fresh read; the first subscriber also
revalidates a cached room. One five-second timer per subscribed room catches
streamed questions and answers recorded elsewhere. Focus, reconnect and return
to a visible tab refresh the same store; the last subscriber removes the timer
and event listeners. Overlapping requests are coalesced, and reads predating a
locally recorded answer cannot overwrite it.

[src: file: frontend/src/lib/discussionQuestions.ts:33-143]
[src: file: frontend/src/components/DiscussionQuestionBanner.tsx:14-25]
[src: file: frontend/src/pages/DiscussionsPage.tsx:4562-4566#DiscussionQuestionBanner]

Nothing floats over the conversation any more (KT-1344): the panel rail lives
in the side panel's own column and the in-conversation search was removed, so
the arbitration banner keeps its reveal button inside the conversation column.
The browser regression checks hit testing at 360 px and 1200 px, without opening
a real discussion or answering its arbitration.

[src: file: frontend/e2e/specs/discussion-question-layout.spec.ts:16-42]
