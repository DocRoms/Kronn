#!/usr/bin/env python3
"""Seed ONE discussion carrying a ~2.7 MB agent message into the perf sandbox.

Reproduces the 2026-06-23 crash: a killed Codex run persisted a 2.4 MB
stderr/reasoning dump as its reply; opening that discussion pushed the content
through ReactMarkdown + syntax highlight (super-linear) and crashed the browser
tab. `MarkdownContent` now renders anything past ~200 KB as plain text — this
fixture + `large-message-render.perf.spec.ts` pin that, in a real browser.

Inserts directly into SQLite (like seed.py), so it exercises the case where a
huge message ALREADY exists in the DB regardless of how it got there (legacy
rows, a path that bypassed the backend cap, etc.). Appends to the sandbox —
run after seed.py or standalone. Idempotent.

DB: /tmp/kronn-perf-sandbox/kronn.db (never the user's real DB).
"""
import sqlite3
import uuid
from datetime import datetime

DB = "/tmp/kronn-perf-sandbox/kronn.db"
# Fixed id so re-running is idempotent and the spec can target it.
DISC_ID = "perf0000-large-msg0-0000-000000000000"
TITLE = "PERF Large message (multi-MB)"


def now():
    return datetime.utcnow().isoformat() + "Z"


def main():
    conn = sqlite3.connect(DB)
    cur = conn.cursor()

    cur.execute("DELETE FROM messages WHERE discussion_id = ?", (DISC_ID,))
    cur.execute("DELETE FROM discussions WHERE id = ?", (DISC_ID,))

    cur.execute(
        """INSERT INTO discussions
           (id, project_id, title, agent, language,
            participants_json, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
        (DISC_ID, None, TITLE, "Codex", "fr", "[]", now(), now()),
    )

    # ~2.7 MB. Shaped like the real killed-Codex dump (error prefix + a
    # leading markdown header that, PRE-guard, would have built a giant DOM)
    # and laced with accents/emoji to exercise the UTF-8 char-boundary path.
    big = (
        "[Agent exited with error] (exit code: None)\n\n# DUMP\n\n"
        + ("ligne de raisonnement très détaillée éàü 🚀 " * 60_000)
    )

    cur.execute(
        """INSERT INTO messages
           (id, discussion_id, role, content, agent_type, timestamp, sort_order)
           VALUES (?, ?, ?, ?, ?, ?, ?)""",
        (str(uuid.uuid4()), DISC_ID, "User", "Triage EW-XXXX (read-only)", None, now(), 0),
    )
    cur.execute(
        """INSERT INTO messages
           (id, discussion_id, role, content, agent_type, timestamp, sort_order)
           VALUES (?, ?, ?, ?, ?, ?, ?)""",
        (str(uuid.uuid4()), DISC_ID, "Agent", big, "Codex", now(), 1),
    )

    conn.commit()
    print(f"Seeded disc {DISC_ID!r} '{TITLE}' with a {len(big.encode()) // 1024} KB agent message.")


if __name__ == "__main__":
    main()
