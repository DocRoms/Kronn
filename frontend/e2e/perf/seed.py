#!/usr/bin/env python3
"""Seed the Kronn perf sandbox DB with 250 fake projects + 500 discussions.

Reads from /tmp/kronn-perf-sandbox/kronn.db (created by booting the backend
once with KRONN_DATA_DIR=/tmp/kronn-perf-sandbox).

User's real DB at ~/.config/kronn/kronn.db is NEVER touched.
"""
import sqlite3
import uuid
import random
from datetime import datetime, timedelta

DB = "/tmp/kronn-perf-sandbox/kronn.db"
NB_PROJECTS = 250
NB_DISCUSSIONS = 500
NB_MSGS_PER_DISC_MIN = 0
NB_MSGS_PER_DISC_MAX = 30  # mostly small, some big

# Project name pool — varied lengths, accents, multiple words
NAME_PARTS = [
    "frontend", "backend", "api", "core", "service", "platform", "studio",
    "engine", "gateway", "cli", "sdk", "lib", "ops", "infra", "auth",
    "billing", "analytics", "dashboard", "mobile", "ios", "android",
    "rag", "etl", "workflow", "pipeline", "scraper", "crawler", "indexer",
    "checkout", "payments", "users", "search", "feed", "media", "video",
    "podcast", "newsroom", "live", "weather", "sports", "crypto", "stocks",
    "ai-assistant", "chatbot", "summarizer", "translator", "transcriber",
    "data-lake", "ml-pipeline", "feature-store", "embeddings", "rag-eval",
    "compliance", "audit-tool", "kpi-tracker", "feature-flags", "telemetry",
]
SUFFIXES = ["v2", "next", "legacy", "exp", "lab", "prod", "beta", ""]

PROJ_PATHS = [
    "/home/priol/Repositories/perf-test/{name}",
]

AUDIT_STATUSES = [
    "NoTemplate", "TemplateInstalled", "Audited", "Bootstrapped", "Validated"
]

AGENTS = ["ClaudeCode", "Codex", "GeminiCli", "Kiro", "Vibe", "Ollama"]

DISC_TITLES = [
    "Audit checkpoint",
    "Migration analysis",
    "Bug hunt: {} crash",
    "Feature scoping — {}",
    "Refactor sprint",
    "Performance regression",
    "Code review: PR #{}",
    "Architecture decision",
    "Release prep",
    "On-call investigation {}",
    "Vendor integration: {}",
    "Quick prompt: {}",
    "Briefing — {}",
    "Validation discussion",
    "Tech debt cleanup",
    "Retro: incident {}",
]


def now_iso(offset_days=0):
    return (datetime.utcnow() - timedelta(days=offset_days)).isoformat() + "Z"


def gen_proj_name(i):
    parts = random.sample(NAME_PARTS, k=random.randint(1, 3))
    suffix = random.choice(SUFFIXES)
    name = "-".join(parts)
    if suffix:
        name = f"{name}-{suffix}"
    return f"{name}-{i:03d}"


def gen_disc_title(idx):
    template = random.choice(DISC_TITLES)
    if "{}" in template:
        return template.format(random.choice([
            "EW-1234", "infra", "billing", "auth", "search-v2", "ios-12.4",
            "P0", "Q3-OKR", "podcast-pipeline", "homepage-A11Y",
        ]))
    return f"{template} #{idx}"


def main():
    conn = sqlite3.connect(DB)
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()

    # Wipe any prior seed data (NOT the user's real DB — this is the sandbox!)
    cur.execute("DELETE FROM messages")
    cur.execute("DELETE FROM discussions")
    cur.execute("DELETE FROM projects")
    conn.commit()

    # ─── Projects ─────────────────────────────────────────────────────
    project_ids = []
    for i in range(NB_PROJECTS):
        pid = str(uuid.uuid4())
        name = gen_proj_name(i)
        path = f"/home/priol/Repositories/perf-test/{name}"
        audit = random.choice(AUDIT_STATUSES)
        ai_config = '{"detected":' + ('true' if audit != "NoTemplate" else 'false') + ',"configs":[]}'
        created = now_iso(random.randint(0, 365))
        updated = now_iso(random.randint(0, 30))

        cur.execute(
            """INSERT INTO projects
               (id, name, path, repo_url, token_override_json,
                ai_config_json, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
            (pid, name, path, None, None, ai_config, created, updated),
        )
        project_ids.append(pid)

    # ─── Discussions + messages ───────────────────────────────────────
    # Concentrate ~30% of discussions on the FIRST 5 projects so we can
    # test the per-project loose-disc cap (default PROJECT_LOOSE_LIMIT=10
    # with "+N more" expand). The remaining 70% spread across all 250
    # projects so we still cover the cross-project rendering path.
    hot_projects = project_ids[:5]
    for i in range(NB_DISCUSSIONS):
        did = str(uuid.uuid4())
        # ~30% on hot projects, ~55% on random project, ~15% global
        r = random.random()
        if r < 0.30:
            pid = random.choice(hot_projects)
        elif r < 0.85:
            pid = random.choice(project_ids)
        else:
            pid = None
        title = gen_disc_title(i)
        agent = random.choice(AGENTS)
        created = now_iso(random.randint(0, 90))
        updated = now_iso(random.randint(0, 30))

        cur.execute(
            """INSERT INTO discussions
               (id, project_id, title, agent, language,
                participants_json, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
            (did, pid, title, agent, "fr", "[]", created, updated),
        )

        # Some discussions get many messages, some get few — realistic mix.
        nb_msgs = random.randint(NB_MSGS_PER_DISC_MIN, NB_MSGS_PER_DISC_MAX)
        for j in range(nb_msgs):
            mid = str(uuid.uuid4())
            role = "User" if j % 2 == 0 else "Agent"
            content = (
                f"Message {j} of discussion {i}. "
                + ("lorem ipsum dolor sit amet, " * random.randint(2, 30))
                + ("\n\nWith some détails accentués et un emoji 🚀" if random.random() < 0.3 else "")
            )
            ts = now_iso(random.randint(0, 30))
            cur.execute(
                """INSERT INTO messages
                   (id, discussion_id, role, content, agent_type, timestamp, sort_order)
                   VALUES (?, ?, ?, ?, ?, ?, ?)""",
                (mid, did, role, content, agent if role == "Agent" else None, ts, j),
            )

    conn.commit()

    # Verify
    p_count = cur.execute("SELECT COUNT(*) FROM projects").fetchone()[0]
    d_count = cur.execute("SELECT COUNT(*) FROM discussions").fetchone()[0]
    m_count = cur.execute("SELECT COUNT(*) FROM messages").fetchone()[0]
    db_size = cur.execute("SELECT page_count*page_size FROM pragma_page_count(), pragma_page_size()").fetchone()[0]

    print(f"Seeded sandbox DB at {DB}")
    print(f"  Projects:    {p_count}")
    print(f"  Discussions: {d_count}")
    print(f"  Messages:    {m_count}")
    print(f"  DB size:     {db_size / 1024:.0f} KB")

    conn.close()


if __name__ == "__main__":
    main()
