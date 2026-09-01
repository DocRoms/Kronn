#!/usr/bin/env bash
# Reproducible measurement for the discussion storage-weight queries (KT-541).
#
# Answers one question: does bounding the batch actually matter, or is the
# global aggregate good enough? It prints the query PLAN (indexed SEARCH vs
# full SCAN) alongside net timings, because the plan is what predicts how the
# cost grows — the timings alone look close on a small database.
#
# Read-only: opens the live database with mode=ro, so it is safe to run while
# the backend serves traffic.
#
# Usage:
#   scripts/measure-discussion-weight.sh [path-to-kronn.db]
set -uo pipefail

DB="${1:-$HOME/Library/Application Support/com.kronn.kronn/kronn.db}"
if [ ! -r "$DB" ]; then
  echo "database not readable: $DB" >&2
  echo "pass the path explicitly: $0 /path/to/kronn.db" >&2
  exit 1
fi
URI="file:${DB}?mode=ro"

SUM_MESSAGES="SUM(LENGTH(CAST(content AS BLOB)))"

echo "# Discussion weight — query plan and cost"
echo
echo "## Volume"
sqlite3 "$URI" "SELECT 'discussions', COUNT(*) FROM discussions
  UNION ALL SELECT 'messages', COUNT(*) FROM messages
  UNION ALL SELECT 'context_files', COUNT(*) FROM context_files;"

IDS_20=$(sqlite3 "$URI" "SELECT group_concat(quote(id), ',') FROM (SELECT id FROM discussions LIMIT 20);")
IDS_200=$(sqlite3 "$URI" "SELECT group_concat(quote(id), ',') FROM (SELECT id FROM discussions LIMIT 200);")
if [ -z "$IDS_20" ]; then
  echo "no discussions to measure" >&2
  exit 1
fi

echo
echo "## Query plans"
echo "### messages, bounded batch (what the endpoint does)"
sqlite3 "$URI" "EXPLAIN QUERY PLAN SELECT discussion_id, $SUM_MESSAGES FROM messages
  WHERE discussion_id IN ($IDS_200) GROUP BY discussion_id;"
echo "### context_files, bounded batch"
sqlite3 "$URI" "EXPLAIN QUERY PLAN SELECT discussion_id,
    SUM(CASE WHEN disk_path IS NOT NULL THEN original_size ELSE 0 END),
    SUM(extracted_size)
  FROM context_files WHERE discussion_id IN ($IDS_200) GROUP BY discussion_id;"
echo "### messages, global aggregate (the endpoint REFUSES this shape)"
sqlite3 "$URI" "EXPLAIN QUERY PLAN SELECT discussion_id, $SUM_MESSAGES FROM messages
  GROUP BY discussion_id;"

echo
echo "## Net timings (process start-up subtracted, best of 5)"
python3 - "$URI" "$IDS_20" "$IDS_200" <<'PYEOF'
import subprocess, sys, time

uri, ids20, ids200 = sys.argv[1], sys.argv[2], sys.argv[3]
SUM = "SUM(LENGTH(CAST(content AS BLOB)))"

def best(sql, runs=5):
    timings = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run(["sqlite3", uri, sql], stdout=subprocess.DEVNULL, check=True)
        timings.append(time.perf_counter() - start)
    return min(timings) * 1000

baseline = best("SELECT 1;")
cases = [
    ("20 ids (a sidebar page)", f"SELECT discussion_id, {SUM} FROM messages WHERE discussion_id IN ({ids20}) GROUP BY discussion_id;"),
    ("200 ids (the cap)", f"SELECT discussion_id, {SUM} FROM messages WHERE discussion_id IN ({ids200}) GROUP BY discussion_id;"),
    ("global aggregate (refused)", f"SELECT discussion_id, {SUM} FROM messages GROUP BY discussion_id;"),
]
print(f"  {'sqlite3 start-up (baseline)':32} {baseline:6.1f} ms")
for label, sql in cases:
    print(f"  {label:32} {max(0.0, best(sql) - baseline):6.1f} ms net")
PYEOF

echo
echo "The bounded batch grows with what is displayed; the global aggregate grows"
echo "with the table. That is why the endpoint has no unbounded form."
