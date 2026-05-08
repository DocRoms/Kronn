#!/usr/bin/env python3
"""Seed a single discussion with 20 specific messages for the introspection
regression test.

Each User message carries a distinct "fact" that the agent (or the
introspection.perf.spec.ts test) must be able to retrieve verbatim.
Messages mix User and Agent roles, with summary_strategy=Off so that the
seeded transcript stays raw — anything missing from the agent context
forces it to call the kronn-internal MCP tools (`disc_get_message` /
`disc_summarize`).

Writes to /tmp/kronn-perf-sandbox/kronn.db (same sandbox as seed.py).
The user's real DB at ~/.config/kronn/kronn.db is NEVER touched.

Prints the disc_id on stdout so the spec can pick it up if needed —
though the test prefers a title-based lookup so the seed is idempotent.
"""
import sqlite3
import uuid
from datetime import datetime, timedelta, timezone

DB = "/tmp/kronn-perf-sandbox/kronn.db"

# Each tuple = (role, content). User messages set the "facts" that the
# spec asserts on. Agent messages reply briefly so the transcript reads
# naturally. Index 4 (the abc1234 commit fact) is the canonical assertion
# target — it's specific enough that the spec can do an exact substring
# match.
MESSAGES = [
    ("User", "Bonjour, je suis TestUser, dev sur ProjectAlpha."),
    ("Agent", "Bonjour TestUser, je suis prêt à t'aider."),
    ("User", "On bosse sur le ticket TA-7283 — la regression du layout sur la home."),
    ("Agent", "Compris : TA-7283, layout home regression."),
    ("User", "Le commit fautif est probablement abc1234 sur la branche feat/header-redesign."),
    ("Agent", "Noté. Tu veux que je vérifie le diff de abc1234 ?"),
    ("User", "Pas tout de suite. D'abord, regarde l'API Metrics — on doit la requêter avec apikey=demo-9876."),
    ("Agent", "OK, je note la clé apikey=demo-9876 pour les requêtes Metrics."),
    ("User", "Le seuil critique est 5000 vues, pas 10000 comme l'autre fois."),
    ("Agent", "Threshold = 5000 vues. Compris."),
    ("User", "On utilise Vibe pour les résumés, gpt-5-mini pour le code, Opus pour la review."),
    ("Agent", "Stack: Vibe (résumés) / gpt-5-mini (code) / Opus (review)."),
    ("User", "Le déploiement est sur https://stage.example.internal port 8443."),
    ("Agent", "Stage : stage.example.internal:8443."),
    ("User", "Notre slogan interne est 'Ship to learn, learn to ship'."),
    ("Agent", "Slogan noté."),
    ("User", "Ah aussi, le mot de passe du tracker — non je rigole, never share that."),
    ("Agent", "Bien vu. Aucun secret dans la transcription, c'est une bonne pratique."),
    ("User", "Bon, dernière info : la deadline est mardi 14 mai à 17h CET."),
    ("Agent", "Deadline : mardi 14 mai 17h CET. C'est noté."),
]

TITLE = "Introspection E2E test"


def now_iso(offset_min=0):
    t = datetime.now(timezone.utc) - timedelta(minutes=offset_min)
    return t.isoformat().replace("+00:00", "Z")


def main():
    conn = sqlite3.connect(DB)
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()

    # Idempotent: wipe any prior introspection-test discussion.
    cur.execute(
        "DELETE FROM messages WHERE discussion_id IN ("
        "SELECT id FROM discussions WHERE title = ?)",
        (TITLE,),
    )
    cur.execute("DELETE FROM discussions WHERE title = ?", (TITLE,))
    conn.commit()

    disc_id = str(uuid.uuid4())
    now = now_iso(0)
    started = now_iso(40)

    cur.execute(
        """INSERT INTO discussions
            (id, project_id, title, agent, language, participants_json,
             archived, message_count, skill_ids_json, profile_ids_json,
             directive_ids_json, workspace_mode, model_tier,
             pin_first_message, summary_strategy, introspection_call_count,
             created_at, updated_at, pinned)
           VALUES (?, NULL, ?, 'ClaudeCode', 'fr',
                   '["ClaudeCode"]',
                   0, ?, '[]', '[]', '[]', 'Direct', 'default',
                   0, 'Off', 0,
                   ?, ?, 0)""",
        (disc_id, TITLE, len(MESSAGES), started, now),
    )

    for i, (role, content) in enumerate(MESSAGES):
        ts = now_iso(40 - 2 * i)
        cur.execute(
            """INSERT INTO messages
                (id, discussion_id, role, content, agent_type, timestamp,
                 sort_order, tokens_used)
               VALUES (?, ?, ?, ?, ?, ?, ?, 0)""",
            (
                str(uuid.uuid4()),
                disc_id,
                role,
                content,
                "ClaudeCode" if role == "Agent" else None,
                ts,
                i,
            ),
        )

    conn.commit()
    conn.close()
    print(disc_id)


if __name__ == "__main__":
    main()
