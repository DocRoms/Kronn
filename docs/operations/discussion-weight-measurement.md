# Discussion weight — query plan and cost

## Volume
discussions|602
messages|26960
context_files|28

## Query plans
### messages, bounded batch (what the endpoint does)
QUERY PLAN
`--SEARCH messages USING INDEX idx_messages_discussion (discussion_id=?)
### context_files, bounded batch
QUERY PLAN
`--SEARCH context_files USING INDEX idx_context_files_discussion (discussion_id=?)
### messages, global aggregate (the endpoint REFUSES this shape)
QUERY PLAN
`--SCAN messages USING INDEX idx_messages_discussion

## Net timings (process start-up subtracted, best of 5)
  sqlite3 start-up (baseline)         4.7 ms
  20 ids (a sidebar page)             0.0 ms net
  200 ids (the cap)                   4.3 ms net
  global aggregate (refused)          9.9 ms net

The bounded batch grows with what is displayed; the global aggregate grows
with the table. That is why the endpoint has no unbounded form.
