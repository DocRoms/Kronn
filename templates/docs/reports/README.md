# Reports

Dated, point-in-time snapshots — an audit run's own output, a benchmark
result, a one-off investigation. Real information, but tied to *when* it was
produced rather than to the current state of the code, so it never belongs in
the tiered context ([`../AGENTS.md`](../AGENTS.md) § 1): loading last month's
benchmark into every task's context would be silently stale advice.

## Convention

- Name files `YYYY-MM-DD-slug.md` — the date is load-bearing, not decoration.
- A report is never edited **by hand** after the fact — if the finding
  changed, add a new dated report and let the old one stand as history. A
  same-day automated rerun of the same slug (e.g. the audit's own
  `human-section-diff` reports) overwrites its own file, since only the
  latest proposal for that day matters — a new day always gets a new file.
- Nothing under `docs/reports/` is loaded by any tier. An agent reads one only
  when a task explicitly names it (e.g. "compare against
  `reports/2026-05-01-startup-latency.md`").
- A finding that is still true today and should shape ongoing work belongs in
  [`../inconsistencies-tech-debt.md`](../inconsistencies-tech-debt.md) or
  [`../decisions.md`](../decisions.md) instead — reports are the raw material,
  not the place to track open work.
