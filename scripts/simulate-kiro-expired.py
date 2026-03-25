#!/usr/bin/env python3
"""Supprime le token OIDC de kiro-cli pour simuler une session expirée."""

import sqlite3
import shutil
from pathlib import Path

DB = Path.home() / ".local" / "share" / "kiro-cli" / "data.sqlite3"
BACKUP = Path("/tmp/kiro-data-backup.sqlite3")

if not DB.exists():
    print(f"❌ Base introuvable : {DB}")
    raise SystemExit(1)

shutil.copy2(DB, BACKUP)
print(f"✅ Backup → {BACKUP}")

conn = sqlite3.connect(str(DB))
cur = conn.execute("DELETE FROM auth_kv WHERE key = 'kirocli:odic:token'")
conn.commit()
conn.close()

if cur.rowcount:
    print(f"✅ Token supprimé ({cur.rowcount} row). Kiro n'est plus authentifié.")
else:
    print("⚠️  Aucun token trouvé (déjà supprimé ?).")

print(f"\n🔄 Pour restaurer : cp {BACKUP} {DB}")
print("🔄 Ou : kiro-cli login --use-device-flow")
