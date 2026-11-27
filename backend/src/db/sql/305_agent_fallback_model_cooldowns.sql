-- KT-2048 — a cooldown can be scoped to one model of an agent. A provider
-- refusal that names only its model ("You've hit your Fable limit … Switch to
-- another model") cools that (agent, model) pair; the other models of the same
-- account keep answering. `model = ''` is the agent-wide cooldown (every model
-- refused: session, weekly or spend limit), which is what every row was before.
CREATE TABLE IF NOT EXISTS agent_fallback_cooldowns_scoped (
    agent     TEXT NOT NULL,
    model     TEXT NOT NULL DEFAULT '',
    until     TEXT NOT NULL,
    reason    TEXT NOT NULL,
    noted_at  TEXT NOT NULL,
    PRIMARY KEY (agent, model)
);
INSERT INTO agent_fallback_cooldowns_scoped (agent, model, until, reason, noted_at)
    SELECT agent, '', until, reason, noted_at FROM agent_fallback_cooldowns;
DROP TABLE agent_fallback_cooldowns;
ALTER TABLE agent_fallback_cooldowns_scoped RENAME TO agent_fallback_cooldowns;
