-- 0.8.7 anti-hallucination P2 — per-message lint report.
--
-- Stores the JSON LintReport (niveau 0 heuristic unsourced_count/flagged_spans
-- + niveau 1 mechanical source verification sources/fabricated_count) computed
-- by core::anti_halluc::analyze at message finalize. Nullable: NULL means the
-- feature was off, the message had no agent output, or nothing was flagged.
ALTER TABLE messages ADD COLUMN lint_report TEXT;
