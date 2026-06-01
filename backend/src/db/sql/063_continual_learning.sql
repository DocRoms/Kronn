-- 0.9.0 — Continual Learning. A `learning` is an agent-proposed durable fact /
-- preference / inference, emitted via the typed MCP tool `learning_propose`,
-- gated by evidence verification + a human, then promoted into a dedicated
-- learnings file. See docs/research/continual-learning-0.9.0-spec.md.
--
-- `faithfulness` = the Gate-2 verdict (claim ⊨ evidence): NULL when the checker
-- is `off`, else 'entailment'|'neutral'|'contradiction'. Posture B: informative,
-- never auto-blocking — the human reads it in the validation modal.
CREATE TABLE learnings (
  id                TEXT PRIMARY KEY,
  claim             TEXT NOT NULL,
  evidence_json     TEXT NOT NULL,                  -- JSON array, MUST be non-empty
  kind              TEXT NOT NULL,                  -- 'fact' | 'preference' | 'inference'
  status            TEXT NOT NULL DEFAULT 'pending',-- pending|promoting|validated|rejected|stale|promoted
  scope             TEXT,                           -- 'user' | 'project' (NULL until routed)
  confidence        REAL,                           -- self-scored, server applies a haircut
  faithfulness      TEXT,                           -- Gate-2 verdict (NULL if checker off)
  discussion_id     TEXT,
  project_id        TEXT,
  source_agent      TEXT,
  promoted_target   TEXT,                           -- file path the learning was written to
  created_at        TEXT NOT NULL,
  last_validated_at TEXT,
  validated_by      TEXT,                           -- 'human' | 'rule:<name>'
  FOREIGN KEY (discussion_id) REFERENCES discussions(id) ON DELETE SET NULL,
  FOREIGN KEY (project_id)    REFERENCES projects(id)    ON DELETE SET NULL
);
CREATE INDEX idx_learnings_status ON learnings(status);
CREATE INDEX idx_learnings_disc   ON learnings(discussion_id);
CREATE INDEX idx_learnings_stale  ON learnings(status, last_validated_at);
-- Dedup: at most ONE non-rejected row per (kind, scope, claim). The partial
-- predicate excludes 'rejected' on purpose — a rejected claim CAN be re-proposed
-- so the negative-learning 3-strike counter (learning_rejections) actually
-- accumulates instead of being pre-empted by the dedup. COALESCE so NULL scope
-- dedups too.
CREATE UNIQUE INDEX idx_learnings_dedup
  ON learnings(kind, COALESCE(scope, ''), claim)
  WHERE status != 'rejected';

-- Negative learning (safeguard #6a): a claim rejected >=3 times auto-rejects the 4th.
CREATE TABLE learning_rejections (
  claim_hash TEXT PRIMARY KEY,
  reason     TEXT,
  count      INTEGER NOT NULL DEFAULT 1,
  last_at    TEXT NOT NULL
);
