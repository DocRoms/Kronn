use crate::models::LiteLlmModelFailure;
use rusqlite::{params, Connection, Result};

const MAX_ERROR_BYTES: usize = 8_000;

fn bounded_error(error: &str) -> &str {
    if error.len() <= MAX_ERROR_BYTES {
        return error;
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    &error[..end]
}

pub fn record(
    conn: &Connection,
    endpoint: &str,
    model: &str,
    status_code: u16,
    error_message: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO lite_llm_model_failures (
             endpoint, model, status_code, error_message
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(endpoint, model) DO UPDATE SET
             status_code = excluded.status_code,
             error_message = excluded.error_message,
             last_failed_at = datetime('now'),
             failure_count = lite_llm_model_failures.failure_count + 1",
        params![endpoint, model, status_code, bounded_error(error_message)],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, endpoint: &str) -> Result<Vec<LiteLlmModelFailure>> {
    let mut statement = conn.prepare(
        "SELECT model, status_code, error_message, first_failed_at,
                last_failed_at, failure_count
         FROM lite_llm_model_failures
         WHERE endpoint = ?1
         ORDER BY last_failed_at DESC, model ASC",
    )?;
    let rows = statement
        .query_map([endpoint], |row| {
            Ok(LiteLlmModelFailure {
                model: row.get(0)?,
                status_code: row.get::<_, u16>(1)?,
                error_message: row.get(2)?,
                first_failed_at: row.get(3)?,
                last_failed_at: row.get(4)?,
                failure_count: row.get::<_, u32>(5)?,
            })
        })?
        .collect();
    rows
}

pub fn clear(conn: &Connection, endpoint: &str, model: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM lite_llm_model_failures WHERE endpoint = ?1 AND model = ?2",
        params![endpoint, model],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_updates_lists_and_clears_a_model_failure() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();

        record(&conn, "http://proxy-a", "model-a", 404, "first error").unwrap();
        record(&conn, "http://proxy-a", "model-a", 422, "latest error").unwrap();
        record(&conn, "http://proxy-b", "model-a", 404, "other proxy").unwrap();

        let failures = list(&conn, "http://proxy-a").unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].model, "model-a");
        assert_eq!(failures[0].status_code, 422);
        assert_eq!(failures[0].error_message, "latest error");
        assert_eq!(failures[0].failure_count, 2);

        assert!(clear(&conn, "http://proxy-a", "model-a").unwrap());
        assert!(list(&conn, "http://proxy-a").unwrap().is_empty());
        assert_eq!(list(&conn, "http://proxy-b").unwrap().len(), 1);
    }

    #[test]
    fn bounds_large_error_payloads_on_a_utf8_boundary() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        record(&conn, "http://proxy", "model", 404, &"é".repeat(10_000)).unwrap();
        let failure = list(&conn, "http://proxy").unwrap().remove(0);
        assert!(failure.error_message.len() <= MAX_ERROR_BYTES);
        assert!(failure
            .error_message
            .is_char_boundary(failure.error_message.len()));
    }
}
