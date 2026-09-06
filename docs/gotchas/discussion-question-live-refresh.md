# Arbitration state must remain live

The question store is shared per discussion, including an initially empty
snapshot. New message IDs prompt a fresh read; the first subscriber also
revalidates a cached room. One five-second timer per subscribed room catches
streamed questions and answers recorded elsewhere. Focus, reconnect and return
to a visible tab refresh the same store; the last subscriber removes the timer
and event listeners. Overlapping requests are coalesced, and reads predating a
locally recorded answer cannot overwrite it.

[src: file: frontend/src/lib/discussionQuestions.ts:40-130]
[src: file: frontend/src/components/DiscussionQuestionBanner.tsx:14-25]
[src: file: frontend/src/pages/DiscussionsPage.tsx:4024-4027]

When the panel rail floats and message search is closed, the arbitration
banner reserves the rail's horizontal footprint for its reveal button. Opening
search moves the banner below the rail; the thread itself keeps its full width.
The browser regression checks hit testing at 360 px and 1200 px with both
search states, without opening a real discussion or answering its arbitration.

[src: file: frontend/src/pages/DiscussionsPage.css:3206-3219]
[src: file: frontend/e2e/specs/discussion-question-layout.spec.ts:13-53]
