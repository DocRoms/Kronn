# CLI session budget

The session budget's time axis is `active_hours`, not wall-clock age. The
backend reads timestamps from messages authored by the exact CLI session via
`message_cli_authors`, parses and sorts them chronologically in UTC across all
discussions, and sums the gaps between successive turns. Each gap is capped at
`max_inactive_gap_minutes`.

The default inactivity threshold is 30 minutes. This is a deliberate
compromise: it keeps short edit/test cycles in one work block, while treating a
long stop such as an overnight shutdown as inactivity. The threshold is a
field on `SessionBudget`, so callers can choose another value for a different
work pattern.

This estimate has an explicit limitation: turn timestamps under-count work that
happens without a posted message, such as a long compilation. The capped gap
is a practical correction, not a measurement of every minute spent working.
