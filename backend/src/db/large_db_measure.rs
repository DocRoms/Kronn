//! Measurements on a generated large database (KT-1019, KT-984), through the
//! real migrations, queries, backup, retention and compaction code.
//!
//! Ignored by default; it writes several gigabytes. Run it with
//! `KRONN_MEASURE_DIR=<empty dir> [KRONN_MEASURE_MB=4096] cargo test --lib
//! large_db_measure -- --ignored --nocapture`. Times are with a warm page
//! cache: the database was just written.
use std::path::Path;
use std::time::Instant;

use rusqlite::{params, Connection};

/// Deterministic pseudo-random sequence, so two runs build the same base.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let value = f();
    eprintln!("{label:<58} {:>8.2} s", started.elapsed().as_secs_f64());
    value
}

/// Agent-like output: prose and code, `len` bytes.
fn output(rng: &mut Lcg, len: usize) -> String {
    const CHUNKS: [&str; 4] = [
        "L'agent a relu le module et propose de découper la fonction en trois étapes. ",
        "fn compact(conn: &Connection) -> Result<usize> { let n = conn.execute(SQL, [])?; Ok(n) }\n",
        "| fichier | lignes | statut |\n|---|---|---|\n| src/db/mod.rs | 412 | modifié |\n",
        "Tests: 128 passed, 0 failed. Next step: open the pull request and request a review.\n",
    ];
    let mut text = String::with_capacity(len + 128);
    while text.len() < len {
        text.push_str(CHUNKS[rng.below(4) as usize]);
    }
    text
}

fn generate(conn: &Connection, target_bytes: u64) -> (usize, u64) {
    let mut rng = Lcg(0x4b52_4f4e);
    for index in 0..20 {
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES (?1, ?2, '\"Manual\"', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            params![format!("wf-{index}"), format!("Workflow {index}")],
        )
        .unwrap();
    }
    let now = chrono::Utc::now();
    let mut written = 0u64;
    let mut runs = 0usize;
    while written < target_bytes {
        conn.execute_batch("BEGIN").unwrap();
        for _ in 0..50 {
            let kind = rng.below(100);
            let (status, run_type) = match kind {
                0..=84 => ("Success", "linear"),
                85..=89 => ("Failed", "linear"),
                90..=92 => ("Success", "batch"),
                93..=94 => ("Success", "compare"),
                95..=96 => ("Interrupted", "linear"),
                _ => ("Success", "subworkflow"),
            };
            let steps = if run_type == "linear" || run_type == "subworkflow" {
                3 + rng.below(8) as usize
            } else {
                1
            };
            let mut results = Vec::with_capacity(steps);
            for step in 0..steps {
                // Mostly a few KB to tens of KB, sometimes a full review report.
                let len = if rng.below(10) == 0 {
                    200_000 + rng.below(400_000) as usize
                } else {
                    2_000 + rng.below(60_000) as usize
                };
                let len = if run_type == "linear" || run_type == "subworkflow" {
                    len
                } else {
                    300
                };
                results.push(serde_json::json!({
                    "step_name": format!("Étape {step}"), "status": "Success",
                    "output": output(&mut rng, len), "tokens_used": 1000 + step,
                    "duration_ms": 1200 + step,
                }));
            }
            let payload = serde_json::Value::Array(results).to_string();
            written += payload.len() as u64;
            let age_days = rng.below(180) as i64;
            let finished = (now - chrono::Duration::days(age_days)).to_rfc3339();
            let parent = if run_type == "subworkflow" && runs > 0 {
                Some(format!("run-{:06}", rng.below(runs as u64)))
            } else {
                None
            };
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
                     started_at, finished_at, run_type, parent_run_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)",
                params![
                    format!("run-{runs:06}"),
                    format!("wf-{}", rng.below(20)),
                    status,
                    payload,
                    (rng.below(200_000)) as i64,
                    finished,
                    run_type,
                    parent
                ],
            )
            .unwrap();
            runs += 1;
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    (runs, written)
}

const OLD_LAST_RUNS_SQL: &str = "SELECT wr.id, wr.workflow_id, wr.status, wr.trigger_context,
        CASE WHEN json_valid(wr.step_results_json)
             THEN (SELECT json_group_array(json_set(value, '$.output', ''))
                   FROM json_each(wr.step_results_json))
             ELSE '[]' END,
        wr.tokens_used, wr.started_at, wr.finished_at
 FROM workflow_runs wr
 INNER JOIN (SELECT workflow_id, MAX(started_at) AS max_started
             FROM workflow_runs GROUP BY workflow_id) latest
   ON wr.workflow_id = latest.workflow_id AND wr.started_at = latest.max_started";

fn count_rows(conn: &Connection, sql: &str) -> usize {
    let mut statement = conn.prepare(sql).unwrap();
    let mut rows = statement.query([]).unwrap();
    let mut n = 0;
    while rows.next().unwrap().is_some() {
        n += 1;
    }
    n
}

#[test]
#[ignore = "writes several gigabytes; run by hand with KRONN_MEASURE_DIR"]
fn large_db_measure() {
    let Ok(dir) = crate::core::child_env::var("KRONN_MEASURE_DIR") else {
        eprintln!("KRONN_MEASURE_DIR is not set; nothing measured");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let target_mb: u64 = crate::core::child_env::var("KRONN_MEASURE_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kronn.db");
    for name in [
        "kronn.db",
        "kronn.db-wal",
        "kronn.db-shm",
        "kronn.db.backup",
        "copy.db",
        "snapshot.db",
        "snapshot-after.db",
    ] {
        let _ = std::fs::remove_file(dir.join(name));
    }

    let conn = Connection::open(&path).unwrap();
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .unwrap();
    super::migrations::run_through(&conn, "211_ui_preferences").unwrap();
    let (runs, payload) = timed("generate (0.14.2 schema)", || {
        generate(&conn, target_mb * 1024 * 1024)
    });
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    eprintln!(
        "runs {runs}, payload {:.0} MB, file {:.0} MB",
        mb(payload),
        mb(file_len(&path))
    );

    timed("0.14.2 stats: SUM(tokens_used)", || {
        conn.query_row(crate::api::stats::WORKFLOW_TOKENS_SQL, [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    });
    timed("0.14.2 workflow list: last runs with json_each", || {
        count_rows(&conn, OLD_LAST_RUNS_SQL)
    });

    timed("file copy, std::fs::copy (APFS clones it)", || {
        std::fs::copy(&path, dir.join("copy.db")).unwrap()
    });
    let _ = std::fs::remove_file(dir.join("copy.db"));
    timed(
        "file copy, byte for byte (what ext4 in Docker does)",
        || {
            let mut from =
                std::io::BufReader::with_capacity(8 << 20, std::fs::File::open(&path).unwrap());
            let mut to = std::io::BufWriter::with_capacity(
                8 << 20,
                std::fs::File::create(dir.join("copy.db")).unwrap(),
            );
            std::io::copy(&mut from, &mut to).unwrap();
            std::io::Write::flush(&mut to).unwrap();
            to.get_ref().sync_all().unwrap();
        },
    );
    let _ = std::fs::remove_file(dir.join("copy.db"));
    timed("pre-migration backup (copy + sync + rename)", || {
        super::migrations::backup_before_migration(&path, |_| Ok(u64::MAX)).unwrap()
    });

    timed("migration 212 (summary index)", || {
        super::migrations::run_through(&conn, "212_workflow_runs_summary_index").unwrap()
    });
    timed("migration 213 (retention column + partial index)", || {
        super::migrations::run(&conn).unwrap()
    });
    timed("0.14.3 stats: SUM(tokens_used)", || {
        conn.query_row(crate::api::stats::WORKFLOW_TOKENS_SQL, [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    });
    timed("0.14.3 stats: top workflows", || {
        count_rows(&conn, crate::api::stats::TOP_WORKFLOWS_SQL)
    });
    timed("0.14.3 workflow list: last run summaries", || {
        count_rows(&conn, super::workflows::LAST_RUN_SUMMARIES_SQL)
    });

    let table: i64 = conn
        .query_row(
            "SELECT SUM(pgsize) FROM dbstat WHERE name = 'workflow_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!(
        "  workflow_runs table before retention: {:.0} MB",
        mb(table as u64)
    );
    let snapshot = dir.join("snapshot.db");
    let size = timed("VACUUM INTO backup, before retention", || {
        crate::core::backup::snapshot_database(&path, &snapshot, |_| Ok(u64::MAX)).unwrap()
    });
    eprintln!("  backup size {:.0} MB", mb(size));
    let _ = std::fs::remove_file(&snapshot);

    let cutoff = super::run_retention::cutoff(chrono::Utc::now(), 30);
    let mut chunks = 0;
    let mut slowest = 0f64;
    let trimmed = timed("retention: trim outputs older than 30 days", || {
        let mut total = 0;
        loop {
            let started = Instant::now();
            let n = super::run_retention::compact_run_payloads_chunk(
                &conn,
                &cutoff,
                super::run_retention::CHUNK_ROWS,
            )
            .unwrap();
            slowest = slowest.max(started.elapsed().as_secs_f64());
            chunks += 1;
            total += n;
            if n < super::run_retention::CHUNK_ROWS {
                return total;
            }
        }
    });
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let free: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    eprintln!(
        "  trimmed {trimmed} runs in {chunks} chunks (slowest {slowest:.3} s); file {:.0} MB, free {:.0} MB",
        mb(file_len(&path)),
        mb(free as u64 * 4096)
    );
    let size = timed("VACUUM INTO backup, after retention", || {
        crate::core::backup::snapshot_database(&path, &dir.join("snapshot-after.db"), |_| {
            Ok(u64::MAX)
        })
        .unwrap()
    });
    eprintln!("  backup size {:.0} MB", mb(size));
    let _ = std::fs::remove_file(dir.join("snapshot-after.db"));
    let before = file_len(&path);
    timed("compaction: VACUUM + checkpoint", || {
        conn.execute_batch("VACUUM;").unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    });
    eprintln!(
        "  file {:.0} MB -> {:.0} MB",
        mb(before),
        mb(file_len(&path))
    );
    let table: i64 = conn
        .query_row(
            "SELECT SUM(pgsize) FROM dbstat WHERE name = 'workflow_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!("  workflow_runs table after: {:.0} MB", mb(table as u64));
}
