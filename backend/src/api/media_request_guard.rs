//! KT-1481 — one human request, one bounded set of generations.
//!
//! A media result wakes the agent through a fresh dispatch. Nothing used to tie
//! that dispatch back to the human sentence that started it, so every
//! notification looked like a new ask: an agent could answer each failure or
//! each success with another generation, varying a parameter until the idempotency
//! key no longer collapsed the call — ten jobs for one request, then a paid
//! provider with the references dropped.
//!
//! The identity of a request is the dispatch of the human message at the base of
//! the chain ([`request_root`]). Notifications and scheduled resumes join the
//! chain of the dispatch that armed them; a new human message starts a new root,
//! so the human can always ask again.
//!
//! Everything here is a function of the database, so it is the same after a
//! backend restart and testable without a model.

use anyhow::Result;
use rusqlite::Connection;

use crate::db::agent_dispatch::DispatchStatus;
use crate::db::media_jobs::MediaJob;
use crate::models::{MediaJobStatus, MediaModality};

/// Jobs one human request may launch, notifications included. Room for a
/// picture, a clip built on it and a few variations; far short of the ten
/// generations one sentence once produced. Asking again is a new request.
pub const MAX_JOBS_PER_REQUEST: usize = 6;

/// Id prefix of the message a durable resume (a media wake included) posts.
/// It is what tells a notification turn from a human one.
const RESUME_MESSAGE_PREFIX: &str = "agent-resume-result:";

/// Longest wake chain followed; a cycle can only come from corrupt rows.
const MAX_HOPS: usize = 32;

/// Who is asking, as the tool executor knows it.
pub struct Caller<'a> {
    pub dispatch_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
}

/// What the agent asks to generate. `connection_id` is the id the request's
/// connection name RESOLVED to — `None` when the agent named none, or the name
/// does not resolve (the generation then refuses it by itself).
pub struct Ask<'a> {
    pub modality: MediaModality,
    pub references: &'a [String],
    pub connection_id: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow {
        /// The request the new job belongs to; `None` when the caller has no
        /// request identity (nothing to bound it against).
        root: Option<String>,
        /// The connection earlier jobs of this request used for this modality.
        /// The generation must run there: choosing another is a provider
        /// change, which is the human's call.
        pinned_connection: Option<String>,
    },
    Refuse(String),
}

fn is_notification_message(message_id: &str) -> bool {
    message_id.starts_with(RESUME_MESSAGE_PREFIX)
}

/// The human request a dispatch belongs to: follows resume notifications back to
/// the dispatch that armed them, until a dispatch a human message started.
pub fn request_root(
    conn: &Connection,
    dispatch_id: Option<&str>,
    message_id: Option<&str>,
) -> Result<Option<String>> {
    let Some(start) = dispatch_id else {
        return Ok(message_id.map(str::to_string));
    };
    let mut current = start.to_string();
    for _ in 0..MAX_HOPS {
        let Some(job) = crate::db::agent_dispatch::get(conn, &current)? else {
            break;
        };
        let Some(resume_id) = job.trigger_message_id.strip_prefix(RESUME_MESSAGE_PREFIX) else {
            break;
        };
        let Some(parent) = crate::db::agent_jobs::get(conn, resume_id)?
            .and_then(|resume| resume.view.source_dispatch_job_id)
        else {
            break;
        };
        current = parent;
    }
    Ok(Some(current))
}

/// The id a connection name (id, alias or display name, or the local ComfyUI)
/// resolves to, so a name can be compared with the id a job recorded. `None`
/// when it names nothing or is ambiguous: the generation refuses that itself.
pub async fn resolve_connection_id(state: &crate::AppState, named: &str) -> Option<String> {
    use crate::db::external_api_connections::ConnectionReference;
    let named = named.trim().to_string();
    if named.is_empty() {
        return None;
    }
    if crate::api::comfyui::names_local(&named) {
        let config = state.config.read().await;
        return crate::api::comfyui::local_connection(&config.comfyui)
            .map(|connection| connection.id);
    }
    state
        .db
        .with_read_conn(move |conn| crate::db::external_api_connections::resolve(conn, &named))
        .await
        .ok()
        .and_then(|reference| match reference {
            ConnectionReference::Found(connection) => Some(connection.id),
            _ => None,
        })
}

/// Whether a dispatch is the answer to a Stop: its row is Cancelled.
pub fn dispatch_was_stopped(conn: &Connection, dispatch_id: &str) -> Result<bool> {
    Ok(crate::db::agent_dispatch::get(conn, dispatch_id)?
        .is_some_and(|job| job.status == DispatchStatus::Cancelled))
}

fn is_notification_turn(conn: &Connection, caller: &Caller<'_>) -> Result<bool> {
    if let Some(dispatch_id) = caller.dispatch_id {
        if let Some(job) = crate::db::agent_dispatch::get(conn, dispatch_id)? {
            return Ok(is_notification_message(&job.trigger_message_id));
        }
    }
    Ok(caller.message_id.is_some_and(is_notification_message))
}

fn failure_detail(job: &MediaJob) -> String {
    let error = job.last_error.as_deref().unwrap_or("no reason recorded");
    let error: String = error.chars().take(300).collect();
    format!("job {} {} ({error})", job.id, job.status.as_str())
}

/// Decides whether an agent's generation may start. Pure reads: the caller
/// stamps the new job with the returned root once it exists.
pub fn authorize(conn: &Connection, caller: &Caller<'_>, ask: &Ask<'_>) -> Result<Verdict> {
    if let Some(dispatch_id) = caller.dispatch_id {
        if dispatch_was_stopped(conn, dispatch_id)? {
            return Ok(Verdict::Refuse(
                "this turn was stopped by the human: no generation is started from it. \
                 Wait for a new request."
                    .into(),
            ));
        }
    }
    let Some(root) = request_root(conn, caller.dispatch_id, caller.message_id)? else {
        return Ok(Verdict::Allow {
            root: None,
            pinned_connection: None,
        });
    };
    let prior = crate::db::media_jobs::list_for_request_root(conn, &root)?;

    // A refused generation is final for the request. Retrying with another
    // parameter, another provider or fewer references is exactly the loop this
    // guard exists to end; the human decides what happens next.
    if let Some(broken) = prior.iter().find(|job| {
        matches!(
            job.status,
            MediaJobStatus::Failed | MediaJobStatus::TimedOut | MediaJobStatus::Cancelled
        )
    }) {
        return Ok(Verdict::Refuse(format!(
            "an earlier generation of this request did not complete: {}. Report that to the \
             user and stop. Do not retry, change the parameters, switch connection or drop \
             references on your own; a new generation needs a new request from the user, \
             who can also approve another provider.",
            failure_detail(broken)
        )));
    }

    if prior.len() >= MAX_JOBS_PER_REQUEST {
        return Ok(Verdict::Refuse(format!(
            "this request already started {} generations, the most one request may start. \
             Present what was produced and ask the user before generating more.",
            prior.len()
        )));
    }

    // A notification says a result is ready. Presenting it is the answer; the
    // only generation it can lead to is one that builds on that result (the
    // picture driving a clip). Anything else is a retake nobody asked for.
    if !prior.is_empty() && is_notification_turn(conn, caller)? {
        let builds_on_result = prior.iter().any(|job| {
            job.status == MediaJobStatus::Completed
                && job
                    .context_file_id
                    .as_deref()
                    .is_some_and(|asset| ask.references.iter().any(|given| given == asset))
        });
        if !builds_on_result {
            return Ok(Verdict::Refuse(
                "you were woken to be told a result: present it to the user instead of \
                 generating again. A further generation is only accepted here when it \
                 starts from that result (pass its context file id in `reference_asset_ids`); \
                 a different take is a new request from the user."
                    .into(),
            ));
        }
    }

    let same_modality: Vec<&MediaJob> = prior
        .iter()
        .filter(|job| job.modality == ask.modality)
        .collect();
    let pinned = same_modality.first().map(|job| job.connection_id.clone());
    if let (Some(pinned), Some(asked)) = (pinned.as_deref(), ask.connection_id) {
        if pinned != asked {
            return Ok(Verdict::Refuse(format!(
                "this request already generates on connection `{pinned}`; moving to `{asked}` \
                 changes the provider, and possibly the bill, which needs the user's approval. \
                 Ask the user, or keep `{pinned}`."
            )));
        }
    }
    let kept = same_modality
        .iter()
        .map(|job| job.params.reference_asset_ids.len())
        .max()
        .unwrap_or(0);
    if ask.references.len() < kept {
        return Ok(Verdict::Refuse(format!(
            "an earlier generation of this request used {kept} reference image(s) and this \
             one passes {}: dropping references changes what was asked and needs the user's \
             approval. Keep them, or ask the user.",
            ask.references.len()
        )));
    }

    Ok(Verdict::Allow {
        root: Some(root),
        pinned_connection: pinned,
    })
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::db::media_jobs::{self, NewMediaJob};
    use crate::models::MediaParams;
    use chrono::Utc;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, created_at, updated_at)
             VALUES ('d1', 'Media', 'Ollama', 'fr', ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn
    }

    /// A human message and its dispatch: a new request root.
    fn human_request(conn: &Connection, n: u32) -> String {
        let now = Utc::now().to_rfc3339();
        let message = format!("u{n}");
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, 'd1', 'User', 'an image', ?2, ?3, ?2)",
            rusqlite::params![message, now, i64::from(n)],
        )
        .unwrap();
        let dispatch = format!("dispatch-{n}");
        crate::db::agent_dispatch::enqueue(
            conn,
            crate::db::agent_dispatch::NewAgentDispatchJob {
                id: &dispatch,
                discussion_id: "d1",
                trigger_message_id: &message,
                trigger_sort_order: i64::from(n),
                dedupe_key: &format!("message:{message}"),
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        dispatch
    }

    /// The dispatch a media wake runs in, the way `settle_and_dispatch` creates it.
    fn notification_of(conn: &Connection, source: &str, n: u32) -> String {
        let resume = format!("resume-{n}");
        let message = format!("{RESUME_MESSAGE_PREFIX}{resume}");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, 'd1', 'User', 'notification', ?2, ?3, ?2)",
            rusqlite::params![message, now, 100 + i64::from(n)],
        )
        .unwrap();
        let dispatch = format!("wake-dispatch-{n}");
        crate::db::agent_dispatch::enqueue(
            conn,
            crate::db::agent_dispatch::NewAgentDispatchJob {
                id: &dispatch,
                discussion_id: "d1",
                trigger_message_id: &message,
                trigger_sort_order: 100 + i64::from(n),
                dedupe_key: &format!("agent-resume-completion:{resume}"),
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_resume_jobs
             (id, discussion_id, target_agent_json, source_dispatch_job_id, kind, status,
              dedupe_key, reason, scheduled_at, chain_depth, wake_budget,
              completion_dispatch_id, created_at, updated_at)
             VALUES (?1, 'd1', '\"Ollama\"', ?2, 'Wake', 'Completed', ?3, 'media', ?4, 1, 3, ?5, ?4, ?4)",
            rusqlite::params![resume, source, format!("media-wake:{n}"), now, dispatch],
        )
        .unwrap();
        dispatch
    }

    fn job(
        conn: &Connection,
        id: &str,
        root: &str,
        connection: &str,
        status: MediaJobStatus,
        refs: &[&str],
    ) {
        let now = Utc::now();
        let params = MediaParams {
            reference_asset_ids: refs.iter().map(|id| id.to_string()).collect(),
            ..Default::default()
        };
        media_jobs::insert(
            conn,
            NewMediaJob {
                id,
                modality: MediaModality::Image,
                connection_id: connection,
                model: "m",
                prompt: "p",
                params: &params,
                discussion_id: Some("d1"),
                message_id: None,
                project_id: None,
                scheduled_at: now,
                deadline_at: now,
            },
            now,
        )
        .unwrap();
        media_jobs::set_request_root(conn, id, root).unwrap();
        conn.execute(
            "UPDATE media_jobs SET status = ?2, context_file_id = ?3 WHERE id = ?1",
            rusqlite::params![id, status.as_str(), format!("asset-{id}")],
        )
        .unwrap();
    }

    fn ask<'a>(references: &'a [String], connection: Option<&'a str>) -> Ask<'a> {
        Ask {
            modality: MediaModality::Image,
            references,
            connection_id: connection,
        }
    }

    fn refused(verdict: Verdict) -> String {
        match verdict {
            Verdict::Refuse(reason) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_notification_joins_the_request_that_armed_it_and_a_human_message_starts_another() {
        let conn = database();
        let first = human_request(&conn, 1);
        let wake = notification_of(&conn, &first, 1);
        let second_wake = notification_of(&conn, &wake, 2);
        let again = human_request(&conn, 2);

        assert_eq!(
            request_root(&conn, Some(&wake), None).unwrap().as_deref(),
            Some(first.as_str())
        );
        assert_eq!(
            request_root(&conn, Some(&second_wake), None)
                .unwrap()
                .as_deref(),
            Some(first.as_str()),
            "a chain of wakes is still one request"
        );
        assert_eq!(
            request_root(&conn, Some(&again), None).unwrap().as_deref(),
            Some(again.as_str()),
            "a new human message is a new request"
        );
    }

    #[test]
    fn one_request_cannot_start_a_cascade_of_jobs() {
        let conn = database();
        let root = human_request(&conn, 1);
        let caller = Caller {
            dispatch_id: Some(&root),
            message_id: Some("u1"),
        };
        for n in 0..MAX_JOBS_PER_REQUEST {
            assert!(matches!(
                authorize(&conn, &caller, &ask(&[], Some("local:comfyui"))).unwrap(),
                Verdict::Allow { .. }
            ));
            job(
                &conn,
                &format!("j{n}"),
                &root,
                "local:comfyui",
                MediaJobStatus::Completed,
                &[],
            );
        }
        let reason = refused(authorize(&conn, &caller, &ask(&[], Some("local:comfyui"))).unwrap());
        assert!(reason.contains("most one request may start"), "{reason}");

        // The human asking again is a new request with a fresh allowance.
        let next = human_request(&conn, 2);
        let verdict = authorize(
            &conn,
            &Caller {
                dispatch_id: Some(&next),
                message_id: Some("u2"),
            },
            &ask(&[], Some("local:comfyui")),
        )
        .unwrap();
        assert!(matches!(verdict, Verdict::Allow { .. }));
    }

    #[test]
    fn a_deterministic_failure_is_not_retried_under_any_other_shape() {
        let conn = database();
        let root = human_request(&conn, 1);
        job(
            &conn,
            "j1",
            &root,
            "local:comfyui",
            MediaJobStatus::Failed,
            &["a", "b", "c"],
        );
        conn.execute(
            "UPDATE media_jobs SET last_error = 'graph refused: resolution_steps' WHERE id = 'j1'",
            [],
        )
        .unwrap();
        let wake = notification_of(&conn, &root, 1);
        let refs: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();

        // The same graph, another provider, fewer references, from the turn the
        // failure woke or from the original one: all end the same way.
        for caller in [
            Caller {
                dispatch_id: Some(&wake),
                message_id: None,
            },
            Caller {
                dispatch_id: Some(&root),
                message_id: None,
            },
        ] {
            for asked in [
                ask(&refs, Some("local:comfyui")),
                ask(&refs, Some("gemini")),
                ask(&[], Some("gemini")),
            ] {
                let reason = refused(authorize(&conn, &caller, &asked).unwrap());
                assert!(
                    reason.contains("graph refused: resolution_steps"),
                    "{reason}"
                );
                assert!(reason.contains("new request from the user"), "{reason}");
            }
        }
    }

    #[test]
    fn a_cancelled_job_ends_the_request_too() {
        let conn = database();
        let root = human_request(&conn, 1);
        job(&conn, "j1", &root, "c", MediaJobStatus::Cancelled, &[]);
        let caller = Caller {
            dispatch_id: Some(&root),
            message_id: None,
        };
        refused(authorize(&conn, &caller, &ask(&[], None)).unwrap());
    }

    #[test]
    fn a_success_notification_presents_the_result_and_only_builds_on_it() {
        let conn = database();
        let root = human_request(&conn, 1);
        job(
            &conn,
            "j1",
            &root,
            "local:comfyui",
            MediaJobStatus::Completed,
            &[],
        );
        let wake = notification_of(&conn, &root, 1);
        let caller = Caller {
            dispatch_id: Some(&wake),
            message_id: None,
        };

        let reason = refused(authorize(&conn, &caller, &ask(&[], None)).unwrap());
        assert!(reason.contains("present it"), "{reason}");

        // The picture driving the clip is the intended follow-up.
        let driving = vec!["asset-j1".to_string()];
        assert!(matches!(
            authorize(&conn, &caller, &ask(&driving, None)).unwrap(),
            Verdict::Allow { .. }
        ));

        // A turn a human message started is free to generate (up to the cap).
        let human = Caller {
            dispatch_id: Some(&root),
            message_id: None,
        };
        assert!(matches!(
            authorize(&conn, &human, &ask(&[], None)).unwrap(),
            Verdict::Allow { .. }
        ));
    }

    #[test]
    fn the_connection_and_the_references_of_a_request_cannot_be_swapped_silently() {
        let conn = database();
        let root = human_request(&conn, 1);
        job(
            &conn,
            "j1",
            &root,
            "local:comfyui",
            MediaJobStatus::Completed,
            &["a", "b", "c"],
        );
        let caller = Caller {
            dispatch_id: Some(&root),
            message_id: None,
        };
        let three: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();

        let reason = refused(authorize(&conn, &caller, &ask(&three, Some("gemini"))).unwrap());
        assert!(reason.contains("changes the provider"), "{reason}");
        let reason = refused(authorize(&conn, &caller, &ask(&three[..1], None)).unwrap());
        assert!(reason.contains("dropping references"), "{reason}");

        // Naming nothing keeps the request where it was, rather than letting the
        // default pick for it.
        assert_eq!(
            authorize(&conn, &caller, &ask(&three, None)).unwrap(),
            Verdict::Allow {
                root: Some(root.clone()),
                pinned_connection: Some("local:comfyui".into()),
            }
        );
    }

    #[test]
    fn a_stopped_turn_starts_nothing_even_when_it_still_runs() {
        let conn = database();
        let root = human_request(&conn, 1);
        crate::db::agent_dispatch::cancel_for_discussion(&conn, "d1").unwrap();
        let reason = refused(
            authorize(
                &conn,
                &Caller {
                    dispatch_id: Some(&root),
                    message_id: None,
                },
                &ask(&[], None),
            )
            .unwrap(),
        );
        assert!(reason.contains("stopped by the human"), "{reason}");
    }

    #[test]
    fn stop_disarms_the_callbacks_of_running_generations_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kronn.db");
        let root;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            crate::db::migrations::run(&conn).unwrap();
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, created_at, updated_at)
                 VALUES ('d1', 'Media', 'Ollama', 'fr', ?1, ?1)",
                [&now],
            )
            .unwrap();
            root = human_request(&conn, 1);
            job(&conn, "j1", &root, "c", MediaJobStatus::Running, &[]);
            job(&conn, "j2", &root, "c", MediaJobStatus::Running, &[]);
            let payload = format!(r#"{{"agent_type":"Ollama","source_dispatch_job_id":"{root}"}}"#);
            media_jobs::set_wake_request(&conn, "j1", &payload).unwrap();
            media_jobs::set_wake_request(&conn, "j2", &payload).unwrap();

            // Stop: the dispatch is cancelled and the armed wakes are dropped.
            crate::db::agent_dispatch::cancel_for_discussion(&conn, "d1").unwrap();
            assert_eq!(
                media_jobs::clear_wake_requests_for_discussion(&conn, "d1").unwrap(),
                2
            );
        }

        // The backend restarts; the generations settle late.
        let reopened = Connection::open(&path).unwrap();
        assert_eq!(
            media_jobs::take_wake_request(&reopened, "j1").unwrap(),
            None
        );
        assert_eq!(
            media_jobs::take_wake_request(&reopened, "j2").unwrap(),
            None
        );
        assert!(dispatch_was_stopped(&reopened, &root).unwrap());
    }

    #[test]
    fn a_wake_armed_in_the_race_with_stop_is_recognised_by_its_cancelled_dispatch() {
        // Armed after Stop cleared the column: the claim succeeds, but the
        // dispatch that asked for it is Cancelled, which is what the publisher
        // checks before scheduling anything.
        let conn = database();
        let root = human_request(&conn, 1);
        job(&conn, "j1", &root, "c", MediaJobStatus::Running, &[]);
        crate::db::agent_dispatch::cancel_for_discussion(&conn, "d1").unwrap();
        media_jobs::set_wake_request(&conn, "j1", "{}").unwrap();
        assert!(media_jobs::take_wake_request(&conn, "j1")
            .unwrap()
            .is_some());
        assert!(dispatch_was_stopped(&conn, &root).unwrap());
        // A second delivery of the same callback finds nothing to fire.
        assert_eq!(media_jobs::take_wake_request(&conn, "j1").unwrap(), None);
    }

    #[test]
    fn the_notification_states_the_outcome_and_asks_for_no_retry() {
        let conn = database();
        let root = human_request(&conn, 1);
        job(&conn, "ok", &root, "c", MediaJobStatus::Completed, &[]);
        job(&conn, "ko", &root, "c", MediaJobStatus::Failed, &[]);
        let completed = media_jobs::get(&conn, "ok").unwrap().unwrap();
        let failed = media_jobs::get(&conn, "ko").unwrap().unwrap();
        let success = crate::api::shared_runs::media_wake_reason(&completed);
        let failure = crate::api::shared_runs::media_wake_reason(&failed);
        assert!(success.contains("is ready") && success.contains("present it"));
        assert!(failure.contains("did not complete") && failure.contains("Do not retry"));
        for reason in [success, failure] {
            assert!(reason.contains("not a new request"), "{reason}");
            assert!(reason.chars().count() <= 1_000);
        }
    }

    #[test]
    fn the_guard_reads_the_same_answer_after_a_restart() {
        // Nothing here lives in memory: a second connection to the same file is
        // a restarted backend.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kronn.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            crate::db::migrations::run(&conn).unwrap();
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, created_at, updated_at)
                 VALUES ('d1', 'Media', 'Ollama', 'fr', ?1, ?1)",
                [&now],
            )
            .unwrap();
            let root = human_request(&conn, 1);
            job(&conn, "j1", &root, "c", MediaJobStatus::Failed, &[]);
        }
        let reopened = Connection::open(&path).unwrap();
        let caller = Caller {
            dispatch_id: Some("dispatch-1"),
            message_id: None,
        };
        refused(authorize(&reopened, &caller, &ask(&[], None)).unwrap());
    }
}
