//! Media generation as a native tool, for a discussion agent and for a workflow
//! Agent step (KT-1605).
//!
//! One body serves both: the validation, the request guard (KT-1481), the
//! connection resolution and the idempotency key are the same code, whoever
//! asks. They differ only in where the asset lands ([`MediaOrigin`]).
//!
//! A discussion agent's asset lands in its own room, and the room is woken when
//! it asked. A workflow step has no room, so Kronn opens one in the workflow's
//! project, links it to the run (the run's discussions list it, and deleting the
//! run deletes it) and reuses it for the next generation of the same step. The
//! step waits with `media_job_status`, and a finished asset is also copied into
//! the run's artifacts directory, where a later step reads it.
use super::*;
use crate::models::MediaJobStatus;

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_media_tests.rs"]
mod tests;

/// Longest one `media_job_status` call blocks. The media worker keeps running
/// past it: the step calls again, or the asset is simply found in the run.
const MAX_WAIT_SECS: u64 = 300;
#[cfg(not(test))]
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(test)]
const POLL_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// Where a generation comes from, and so where its asset goes.
pub(super) struct MediaOrigin {
    /// The room the asset lands in; `None` asks Kronn to open one.
    pub discussion_id: Option<String>,
    /// What the derived idempotency key is scoped to: a room, or a workflow step.
    pub key_scope: String,
    /// The human message the request counts against (the guard's root when
    /// there is no dispatch).
    pub message_id: Option<String>,
    pub dispatch_id: Option<String>,
    /// The agent to wake when the job settles; `None` when there is nobody to wake.
    pub wake_agent: Option<AgentType>,
    /// The references as the room holds them, when a workflow step's were
    /// brought into it; `None` takes the ids the agent named.
    pub references: Option<Vec<String>>,
}

/// What a workflow step's generations are keyed on (KT-1605).
#[derive(Default)]
pub(super) struct StepMedia {
    pub step_name: String,
    /// Set for the Test button: one press, never shared with another press,
    /// workflow or project.
    pub test_invocation: Option<String>,
    /// The press is a dry run: nothing may be billed.
    pub dry_run: bool,
}

impl KronnToolExecutor {
    pub(super) async fn generate_media(&self, call: &ToolCall, origin: MediaOrigin) -> ToolOutcome {
        let wake_requested = call
            .arguments
            .get("wake_when_ready")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if wake_requested && origin.wake_agent.is_none() {
            // Said before anything is billed, not after.
            return fail(
                call,
                "this step has no room to wake: start the generation without `wake_when_ready` and wait for it with `media_job_status` and `wait_secs`",
            );
        }
        // Let the media handler resolve an absent connection or report ambiguous providers.
        let connection_id = required_string(call, "connection_id");
        let Some(prompt) = required_string(call, "prompt") else {
            return fail(call, "missing required field `prompt`");
        };
        let modality = match call
            .arguments
            .get("modality")
            .and_then(serde_json::Value::as_str)
        {
            Some("image") => crate::models::MediaModality::Image,
            Some("video") => crate::models::MediaModality::Video,
            // Named rather than defaulted: generating the wrong modality
            // is a billed mistake the operator did not ask for.
            other => {
                return fail(
                    call,
                    format!(
                        "`modality` must be \"image\" or \"video\", got {}",
                        other.unwrap_or("nothing")
                    ),
                )
            }
        };
        let references: Vec<String> = origin
            .references
            .clone()
            .unwrap_or_else(|| named_references(call));
        // KT-1481 — bounded per human request: no cascade after a
        // notification, no retry of a refusal, no silent provider
        // switch or loss of references (KT-1420), nothing after Stop.
        let named_connection = match connection_id.as_deref() {
            Some(named) => {
                crate::api::media_request_guard::resolve_connection_id(&self.state, named).await
            }
            None => None,
        };
        let verdict = {
            let dispatch_id = origin.dispatch_id.clone();
            let message_id = origin.message_id.clone();
            let references = references.clone();
            self.state
                .db
                .with_read_conn(move |conn| {
                    crate::api::media_request_guard::authorize(
                        conn,
                        &crate::api::media_request_guard::Caller {
                            dispatch_id: dispatch_id.as_deref(),
                            message_id: message_id.as_deref(),
                        },
                        &crate::api::media_request_guard::Ask {
                            modality,
                            references: &references,
                            connection_id: named_connection.as_deref(),
                        },
                    )
                })
                .await
        };
        let (request_root, connection_id) = match verdict {
            Ok(crate::api::media_request_guard::Verdict::Allow {
                root,
                pinned_connection,
            }) => (root, pinned_connection.or(connection_id)),
            Ok(crate::api::media_request_guard::Verdict::Refuse(reason)) => {
                return fail(call, reason)
            }
            Err(error) => return fail(call, format!("could not check this generation: {error}")),
        };
        let request = crate::api::media::GenerateMediaRequest {
            // Derived from the request when the agent gives none.
            // Without this the server falls back to a random id, so two
            // identical asks are two jobs and two charges — which is
            // what a wake-and-retry loop produced: the same picture
            // generated twice, six minutes apart, billed twice.
            //
            // An agent that wants a genuinely different take says so by
            // passing its own key, or by changing the prompt.
            idempotency_key: Some(
                call.arguments
                    .get("idempotency_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        derived_media_idempotency_key(&origin.key_scope, &call.arguments)
                    }),
            ),
            connection_id,
            expected_model: required_string(call, "expected_model"),
            modality,
            prompt,
            // The asset belongs to the room the agent is answering in.
            discussion_id: origin.discussion_id.clone(),
            message_id: None,
            duration_secs: call
                .arguments
                .get("duration_secs")
                .and_then(serde_json::Value::as_u64)
                .map(|value| value as u32),
            resolution: call
                .arguments
                .get("resolution")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            aspect_ratio: call
                .arguments
                .get("aspect_ratio")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            generate_audio: call
                .arguments
                .get("generate_audio")
                .and_then(serde_json::Value::as_bool),
            // Hardcoded to None until 0.13.0, which is why an agent
            // could generate an image and a video but never join them.
            reference_asset_ids: call
                .arguments
                .get("reference_asset_ids")
                .and_then(serde_json::Value::as_array)
                .map(|_| references.clone()),
            reference_asset_id: None,
            reference_mode: match call
                .arguments
                .get("reference_mode")
                .and_then(serde_json::Value::as_str)
            {
                Some("first_frame") => Some(crate::models::MediaReferenceMode::FirstFrame),
                Some("last_frame") => Some(crate::models::MediaReferenceMode::LastFrame),
                Some("reference") => Some(crate::models::MediaReferenceMode::Reference),
                // Left to the server's own default rather than guessed
                // here, so the agent path and the UI agree.
                _ => None,
            },
        };
        let response = crate::api::media::generate(
            axum::extract::State(self.state.clone()),
            None,
            axum::Json(request),
        )
        .await;

        // The job now belongs to the human request that asked for it, so
        // the next notification of this chain counts against the same one.
        if let (Some(root), Some(queued)) = (request_root, response.0.data.as_ref()) {
            let job = queued.job_id.clone();
            if let Err(error) = self
                .state
                .db
                .with_conn(move |conn| crate::db::media_jobs::set_request_root(conn, &job, &root))
                .await
            {
                tracing::warn!("media job could not be tied to its request: {error}");
            }
        }

        // Recorded AFTER the job exists, and only when asked. Writing it
        // into the generation request would carry caller identity into
        // the body sent to the provider.
        if let (true, Some(wake_agent)) = (wake_requested, origin.wake_agent.as_ref()) {
            if let Some(job_id) = response.0.data.as_ref().map(|queued| queued.job_id.clone()) {
                let payload = serde_json::json!({
                    "agent_type": wake_agent,
                    "source_dispatch_job_id": origin.dispatch_id,
                })
                .to_string();
                let job = job_id.clone();
                if let Err(error) = self
                    .state
                    .db
                    .with_conn(move |conn| {
                        crate::db::media_jobs::set_wake_request(conn, &job, &payload)
                    })
                    .await
                {
                    // The generation is already queued and billed. Say
                    // the wake was not armed rather than let the agent
                    // wait for a call that will never come.
                    return fail(
                        call,
                        format!(
                            "the generation started ({job_id}) but the wake could not be \
                             armed: {error}. Poll media_job_status instead."
                        ),
                    );
                }
            }
        }

        // KT-1418 — a refused generation is a failed call. Reported as
        // `ok`, it never fed the runner's error circuit: an agent that
        // kept passing an unusable reference called this 13 times,
        // each time getting the same refusal as if it had succeeded.
        let succeeded = response.0.success;
        match serde_json::to_value(response.0) {
            Ok(value) if succeeded => ok(call, value),
            Ok(value) => ToolOutcome {
                call: call.clone(),
                content: value,
                ok: false,
            },
            Err(error) => fail(call, error.to_string()),
        }
    }

    /// `media_job_status`, as a discussion agent reads it.
    pub(super) async fn read_media_job(&self, call: &ToolCall) -> ToolOutcome {
        let Some(job_id) = required_string(call, "job_id") else {
            return fail(call, "missing required field `job_id`");
        };
        let response = crate::api::media::get_job(
            axum::extract::State(self.state.clone()),
            axum::extract::Path(job_id),
        )
        .await;
        match serde_json::to_value(response.0) {
            Ok(value) => ok(call, value),
            Err(error) => fail(call, error.to_string()),
        }
    }

    /// `media_generate` and `media_job_status` in a workflow Agent step. The
    /// result is the job itself, not the HTTP envelope around it.
    pub(super) async fn workflow_media_tool(&self, call: &ToolCall) -> ToolOutcome {
        let Some(run_id) = self.workflow_run_id.clone() else {
            return fail(call, "media tools of a workflow step need a workflow run");
        };
        let outcome = match call.name.as_str() {
            "media_generate" => self.workflow_generate_media(call, &run_id).await,
            _ => self.workflow_media_status(call, &run_id).await,
        };
        flatten_envelope(outcome)
    }

    /// The guard's request for this step, and the prefix of every request of
    /// its run. A Test press has no run: it is its own request, keyed on its
    /// project and a fresh id, so no other press shares its cap or its room.
    fn media_roots(&self, run_id: &str) -> (String, String) {
        match &self.step_media.test_invocation {
            Some(press) => {
                let root = format!(
                    "workflow-step-test:{}:{press}",
                    self.project_id.as_deref().unwrap_or("-")
                );
                (root.clone(), root)
            }
            None => (
                format!("workflow-step:{}", self.actor_id),
                format!("workflow-step:Workflow {run_id} · "),
            ),
        }
    }

    async fn workflow_generate_media(&self, call: &ToolCall, run_id: &str) -> ToolOutcome {
        if self.step_media.dry_run {
            // Before anything is queued: a dry run only adds a preamble to the
            // prompt, it does not stop a billed call.
            return fail(
                call,
                "this step runs as a dry run: media_generate would start a billed generation, so nothing is generated. Describe the generation you would start (modality, prompt, references) instead",
            );
        }
        if call
            .arguments
            .get("wake_when_ready")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return fail(
                call,
                "this step has no room to wake: start the generation without `wake_when_ready` and wait for it with `media_job_status` and `wait_secs`",
            );
        }
        // One request per run and step: the guard bounds how many generations a
        // step may start, refuses to retry a failed one and keeps its connection.
        let (root, run_prefix) = self.media_roots(run_id);
        // The checks below and the job they allow are one step: two calls of a
        // run launched together would otherwise both pass a cap one of them
        // fills, or each open its own room.
        let lock = run_lock(&run_prefix);
        let _launching = lock.lock().await;

        let counts = {
            let (root, prefix) = (root.clone(), run_prefix.clone());
            self.state
                .db
                .with_read_conn(move |conn| {
                    Ok((
                        crate::db::media_jobs::list_for_request_root(conn, &root)?,
                        crate::db::media_jobs::count_for_request_root_prefix(conn, &prefix)?,
                    ))
                })
                .await
        };
        let (prior, run_total) = match counts {
            Ok(counts) => counts,
            Err(error) => return fail(call, format!("could not check this generation: {error}")),
        };
        let cap = run_cap();
        if run_total >= cap {
            return fail(
                call,
                format!(
                    "this workflow run already started {run_total} generations, the most one run may start ({cap}; {RUN_CAP_ENV} raises it). Use what was produced"
                ),
            );
        }
        let (room, created) = match prior.into_iter().find_map(|job| job.discussion_id) {
            Some(room) => (room, false),
            None => match self.open_step_room(run_id).await {
                Ok(room) => (room, true),
                Err(error) => {
                    return fail(
                        call,
                        format!("could not open the room of this step: {error}"),
                    )
                }
            },
        };

        let named = named_references(call);
        let references = if named.is_empty() {
            None
        } else {
            match self.bring_references(run_id, &room, &named).await {
                Ok(brought) => Some(brought),
                Err(refusal) => {
                    if created {
                        self.discard_room(&room).await;
                    }
                    return fail(call, refusal);
                }
            }
        };
        let outcome = self
            .generate_media(
                call,
                MediaOrigin {
                    discussion_id: Some(room.clone()),
                    key_scope: root.clone(),
                    message_id: Some(root),
                    dispatch_id: None,
                    wake_agent: None,
                    references,
                },
            )
            .await;
        if created && !outcome.ok {
            self.discard_room(&room).await;
        }
        outcome
    }

    /// The room the step's assets land in, listed with the run and deleted
    /// with it. A Test press has no run to list it under.
    async fn open_step_room(&self, run_id: &str) -> anyhow::Result<String> {
        let title: String = format!("Media · {}", self.step_media.step_name)
            .chars()
            .take(60)
            .collect();
        let project = self.project_id.clone();
        let run = self
            .step_media
            .test_invocation
            .is_none()
            .then(|| run_id.to_string());
        self.state
            .db
            .with_conn(move |conn| {
                crate::api::media::insert_step_media_room(conn, &title, project, run.as_deref())
            })
            .await
    }

    /// Takes back a room this call opened and nothing came of: a refusal must
    /// not leave an empty room in the run.
    async fn discard_room(&self, room: &str) {
        let room = room.to_string();
        let _ = self
            .state
            .db
            .with_conn(move |conn| {
                let used: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM media_jobs WHERE discussion_id = ?1)",
                    [&room],
                    |row| row.get(0),
                )?;
                if !used {
                    crate::db::discussions::delete_discussion(conn, &room)?;
                }
                Ok(())
            })
            .await;
    }

    async fn workflow_media_status(&self, call: &ToolCall, run_id: &str) -> ToolOutcome {
        let Some(job_id) = required_string(call, "job_id") else {
            return fail(call, "missing required field `job_id`");
        };
        let wait = call
            .arguments
            .get("wait_secs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(MAX_WAIT_SECS);
        let started = std::time::Instant::now();
        let outcome = loop {
            let outcome = self
                .read_media_job(&ToolCall {
                    arguments: json!({ "job_id": job_id }),
                    ..call.clone()
                })
                .await;
            let status = outcome.content["data"]["status"]
                .as_str()
                .and_then(|status| serde_json::from_value::<MediaJobStatus>(json!(status)).ok());
            let settled = status.is_some_and(MediaJobStatus::is_terminal);
            if settled || !outcome.ok || started.elapsed().as_secs() >= wait {
                break outcome;
            }
            tokio::time::sleep(POLL_EVERY).await;
        };
        let mut outcome = flatten_envelope(outcome);
        if !outcome.ok {
            return outcome;
        }
        let settled = outcome.content["status"]
            .as_str()
            .and_then(|status| serde_json::from_value::<MediaJobStatus>(json!(status)).ok())
            .is_some_and(MediaJobStatus::is_terminal);
        outcome.content["settled"] = json!(settled);
        if !settled {
            outcome.content["note"] = json!(
                "still generating: call media_job_status again with `wait_secs` (up to 300). Do not start the generation again."
            );
        }
        if outcome.content["status"] == "completed" {
            if let Some(file) = outcome.content["context_file_id"]
                .as_str()
                .map(str::to_string)
            {
                outcome.content["asset"] = self.run_asset(run_id, &job_id, &file).await;
            }
        }
        outcome
    }

    /// The finished asset as a workflow step uses it: its context file, and a
    /// copy in the run's artifacts directory, which later steps read.
    async fn run_asset(&self, run_id: &str, job_id: &str, file_id: &str) -> Value {
        let lookup = file_id.to_string();
        let file = self
            .state
            .db
            .with_read_conn(move |conn| {
                Ok(crate::db::discussions::get_context_file(conn, &lookup)?)
            })
            .await
            .ok()
            .flatten();
        let Some(file) = file else {
            return json!({ "context_file_id": file_id });
        };
        let mut asset = json!({
            "context_file_id": file.id,
            "discussion_id": file.discussion_id,
            "filename": file.filename,
            "mime_type": file.mime_type,
        });
        // A test run of one step has no run directory to fill.
        if let (Some(source), true) = (file.disk_path.clone(), run_id != TEST_STEP_RUN_ID) {
            let (run, job, name) = (
                run_id.to_string(),
                job_id.to_string(),
                file.filename.clone(),
            );
            let copied =
                tokio::task::spawn_blocking(move || copy_into_run(&run, &job, &name, &source))
                    .await
                    .map_err(std::io::Error::other)
                    .and_then(|copied| copied);
            match copied {
                Ok(path) => asset["artifact_path"] = json!(path.display().to_string()),
                Err(error) => {
                    tracing::warn!("media asset could not be copied to its run: {error}");
                    asset["artifact_note"] = json!(
                        "the asset is in the run's room but could not be copied to the run's artifacts directory"
                    );
                }
            }
        }
        asset
    }

    /// Brings every picture a step names into its room, where the generation
    /// resolves it as it does in a discussion. A reference is one of, in this
    /// order: an image already in the step's room (id or filename); a context
    /// file of the run's rooms or of the project's discussions (the brand
    /// image a launch variable names); a job of THIS run, by id or by the
    /// artifact path `media_job_status` returned; a picture file of the
    /// project. Nothing of another run, project or discussion is reachable.
    async fn bring_references(
        &self,
        run_id: &str,
        room: &str,
        named: &[String],
    ) -> Result<Vec<String>, String> {
        let mut brought = Vec::with_capacity(named.len());
        for reference in named {
            brought.push(self.bring_reference(run_id, room, reference.trim()).await?);
        }
        Ok(brought)
    }

    async fn bring_reference(
        &self,
        run_id: &str,
        room: &str,
        reference: &str,
    ) -> Result<String, String> {
        if reference.is_empty() {
            return Ok(String::new());
        }
        let (named, home) = (reference.to_string(), room.to_string());
        let found = self
            .state
            .db
            .with_read_conn(move |conn| {
                let file = crate::db::discussions::get_context_file(conn, &named)?;
                let owner = match &file {
                    Some(file) => Some(discussion_scope(conn, &file.discussion_id)?),
                    None => None,
                };
                let in_room_by_name = crate::db::discussions::list_context_files(conn, &home)?
                    .iter()
                    .any(|file| file.filename == named && file.mime_type.starts_with("image/"));
                Ok(Found {
                    file,
                    owner,
                    in_room_by_name,
                    job: job_root(conn, &named)?,
                })
            })
            .await
            .map_err(|error| format!("failed to read the reference: {error}"))?;
        if found.in_room_by_name
            || found
                .file
                .as_ref()
                .is_some_and(|file| file.discussion_id == room)
        {
            return Ok(reference.to_string());
        }
        if let Some(file) = found.file {
            // Any discussion of the project is a source, rooms shared with
            // peers included: fine for one account. KT-1235 (multi-account)
            // must revisit this scope.
            let (project, run) = found.owner.unwrap_or_default();
            let ours = run.as_deref() == Some(run_id)
                || (self.project_id.is_some() && project == self.project_id);
            if !ours {
                return Err(OUT_OF_SCOPE.into());
            }
            return self.copy_into_room(room, &file.id, &file).await;
        }
        if let Some((job, root)) = found.job {
            return self.bring_job_asset(run_id, room, &job, root).await;
        }
        if let Some(job) = self.artifact_job(run_id, reference) {
            let lookup = job.clone();
            let found = self
                .state
                .db
                .with_read_conn(move |conn| job_root(conn, &lookup))
                .await
                .map_err(|error| format!("failed to read the reference: {error}"))?;
            return match found {
                Some((job, root)) => self.bring_job_asset(run_id, room, &job, root).await,
                None => Err(format!(
                    "no generation of this run produced `{reference}`: pass the job id or the `artifact_path` media_job_status returned"
                )),
            };
        }
        self.bring_project_file(room, reference).await
    }

    /// The finished asset of a job of THIS run, brought into the step's room.
    async fn bring_job_asset(
        &self,
        run_id: &str,
        room: &str,
        job: &str,
        root: Option<String>,
    ) -> Result<String, String> {
        let (_, run_prefix) = self.media_roots(run_id);
        if !root.is_some_and(|root| root.starts_with(&run_prefix)) {
            return Err(OUT_OF_SCOPE.into());
        }
        let lookup = job.to_string();
        let asset = self
            .state
            .db
            .with_read_conn(move |conn| {
                let Some(job) = crate::db::media_jobs::get(conn, &lookup)? else {
                    return Ok(None);
                };
                let file = match job.context_file_id.as_deref() {
                    Some(id) => crate::db::discussions::get_context_file(conn, id)?,
                    None => None,
                };
                Ok(Some((job.status, file)))
            })
            .await
            .map_err(|error| format!("failed to read the reference: {error}"))?;
        match asset {
            Some((MediaJobStatus::Completed, Some(file))) => {
                if file.discussion_id == room {
                    return Ok(file.id);
                }
                self.copy_into_room(room, &file.id, &file).await
            }
            Some((status, _)) => Err(format!(
                "generation {job} is {}: wait for it with media_job_status and `wait_secs` before using it as a reference",
                status.as_str()
            )),
            None => Err(OUT_OF_SCOPE.into()),
        }
    }

    /// The job id an artifact path of THIS run names: `media_job_status`
    /// returns `<run artifacts dir>/<job id>.<ext>`.
    fn artifact_job(&self, run_id: &str, reference: &str) -> Option<String> {
        let path = std::path::Path::new(reference);
        if !path.is_absolute() {
            return None;
        }
        let artifacts = crate::workflows::run_artifacts::dir_for(run_id)?;
        let parent = path.parent()?;
        let same = parent == artifacts
            || matches!(
                (parent.canonicalize(), artifacts.canonicalize()),
                (Ok(parent), Ok(artifacts)) if parent == artifacts
            );
        same.then(|| path.file_stem()?.to_str().map(str::to_string))
            .flatten()
    }

    /// A picture file of the project, named relative to its root. It is read
    /// through directory fds (no link is followed, a FIFO cannot hold the
    /// open) and must be a picture by its first bytes. Anything that matches
    /// no file gets the one scope refusal.
    async fn bring_project_file(&self, room: &str, reference: &str) -> Result<String, String> {
        let Some(project_id) = self.project_id.clone() else {
            return Err(OUT_OF_SCOPE.into());
        };
        let path = std::path::PathBuf::from(reference);
        let root = self
            .state
            .db
            .with_read_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
            .await
            .map_err(|error| format!("failed to read the project: {error}"))?
            .map(|project| crate::core::scanner::resolve_host_path(&project.path));
        let Some(root) = root else {
            return Err(OUT_OF_SCOPE.into());
        };
        let read = tokio::task::spawn_blocking(move || read_project_image(&root, &path))
            .await
            .map_err(|error| error.to_string())?;
        match read? {
            None => Err(OUT_OF_SCOPE.into()),
            Some(image) => {
                // Keyed on the content: the same picture is one copy, a changed
                // file is a new one.
                let key = format!(
                    "project-file:{}",
                    crate::core::repository_resources::sha256(&image.bytes)
                );
                self.store_in_room(room, &key, image.mime, &image.bytes)
                    .await
            }
        }
    }

    /// A context file of the run or project, copied into the step's room.
    async fn copy_into_room(
        &self,
        room: &str,
        key: &str,
        file: &crate::models::ContextFile,
    ) -> Result<String, String> {
        if !file.mime_type.starts_with("image/") {
            return Err(format!(
                "the reference asset is not an image ({}): a reference is a picture",
                file.mime_type
            ));
        }
        if file.original_size > crate::api::media::MAX_REFERENCE_BYTES {
            return Err(too_large(file.original_size));
        }
        let Some(source) = file.disk_path.clone() else {
            return Err(
                "the reference asset has no stored file — only an uploaded image can be used"
                    .into(),
            );
        };
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(source))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("failed to read the reference image: {error}"))?;
        let mime = image_mime(&file.mime_type);
        self.store_in_room(room, key, mime, &bytes).await
    }

    /// Stores a picture in the step's room as a context file, once per source:
    /// the name is derived from the source, so naming it twice finds the copy.
    async fn store_in_room(
        &self,
        room: &str,
        key: &str,
        mime: &'static str,
        bytes: &[u8],
    ) -> Result<String, String> {
        if bytes.len() as u64 > crate::api::media::MAX_REFERENCE_BYTES {
            return Err(too_large(bytes.len() as u64));
        }
        let extension = mime.trim_start_matches("image/");
        let digest = crate::core::repository_resources::sha256(key.as_bytes());
        let filename = format!("ref-{}.{extension}", &digest[..12]);
        let (home, name) = (room.to_string(), filename.clone());
        let existing = self
            .state
            .db
            .with_read_conn(move |conn| {
                Ok(crate::db::discussions::list_context_files(conn, &home)?
                    .into_iter()
                    .find(|file| file.filename == name)
                    .map(|file| file.id))
            })
            .await
            .map_err(|error| format!("failed to read the room: {error}"))?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let dirs =
            crate::core::context_files::discussion_attachment_dirs(&self.state.db, room).await;
        let disk_path = crate::core::context_files::save_file_to_dir_or_disk(
            &dirs.durable,
            &id,
            &filename,
            bytes,
        )
        .map_err(|error| format!("failed to store the reference image: {error}"))?;
        if let Some(agent_dir) = dirs.agent.as_deref() {
            crate::core::context_files::agent_readable_path(&disk_path, agent_dir);
        }
        let (stored, home, size) = (id.clone(), room.to_string(), bytes.len() as u64);
        self.state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::insert_context_file(
                    conn,
                    &stored,
                    &home,
                    &filename,
                    mime,
                    size,
                    "",
                    Some(&disk_path),
                )?;
                Ok(())
            })
            .await
            .map_err(|error| format!("failed to store the reference image: {error}"))?;
        Ok(id)
    }
}

/// What a reference name resolved to, read in one go.
struct Found {
    file: Option<crate::models::ContextFile>,
    /// The (project, workflow run) of the discussion holding `file`.
    owner: Option<(Option<String>, Option<String>)>,
    in_room_by_name: bool,
    /// The job of that id, and the request it was launched under.
    job: Option<(String, Option<String>)>,
}

/// One refusal for "absent" and "elsewhere", so it does not say which ids exist.
const OUT_OF_SCOPE: &str = "the reference is not in this workflow run's scope: use a picture of this run's rooms, of this project's discussions or files, or a generation of this run (job id or `artifact_path`)";

fn too_large(bytes: u64) -> String {
    format!(
        "the reference image is too large ({} MB); the ceiling is {} MB",
        bytes / (1024 * 1024),
        crate::api::media::MAX_REFERENCE_BYTES / (1024 * 1024)
    )
}

/// The project and workflow run a discussion belongs to.
fn discussion_scope(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT project_id, workflow_run_id FROM discussions WHERE id = ?1",
            [discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or_default())
}

/// A media job's id and the request root it was launched under.
fn job_root(
    conn: &rusqlite::Connection,
    job_id: &str,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT id, request_root_id FROM media_jobs WHERE id = ?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// The mime of a picture Kronn passes on; anything else was refused before.
fn image_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/webp" => "image/webp",
        "image/gif" => "image/gif",
        _ => "image/png",
    }
}

/// The picture file `reference` names under `root`. Every directory of the
/// path is opened from the one before it with `O_NOFOLLOW | O_DIRECTORY`, and
/// the leaf with `O_NOFOLLOW | O_NONBLOCK`, so a directory swapped for a link
/// between the check and the read is refused, and a FIFO is refused at its
/// `fstat` instead of blocking the open. `Ok(None)`: no such file.
fn read_project_image(
    root: &std::path::Path,
    reference: &std::path::Path,
) -> Result<Option<ProjectImage>, String> {
    use crate::workflows::run_artifacts::fd::Dir;
    use std::path::Component;
    let link =
        || "a project file stays inside the project: a link on its path is refused".to_string();
    let mut names = Vec::new();
    for part in reference.components() {
        match part {
            Component::Normal(name) => match name.to_str() {
                Some(name) => names.push(name),
                None => return Ok(None),
            },
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(
                    "a project file is named from the project's root and stays inside it: no `..`"
                        .into(),
                )
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "a project file is named from the project's root, not by an absolute path"
                        .into(),
                )
            }
        }
    }
    let Some((leaf, directories)) = names.split_last() else {
        return Ok(None);
    };
    let walk = |error: std::io::Error| {
        if Dir::is_link_refusal(&error) {
            link()
        } else if Dir::is_not_a_directory(&error) {
            "a project file's path runs through something that is not a directory".to_string()
        } else {
            format!("failed to read the project file: {error}")
        }
    };
    let Some(directory) = Dir::open_under(root, directories, false).map_err(walk)? else {
        return Ok(None);
    };
    let file = match directory.open_regular(leaf) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            return Err("a project file used as a reference must be a regular picture file".into())
        }
        Err(error) => return Err(walk(error)),
    };
    let size = file
        .metadata()
        .map_err(|error| format!("failed to read the project file: {error}"))?
        .len();
    if size > crate::api::media::MAX_REFERENCE_BYTES {
        return Err(too_large(size));
    }
    let bytes = read_capped(file, size)?;
    let Some(mime) = crate::agents::vision::sniff_image_mime(&bytes) else {
        return Err(
            "a project file used as a reference must be a png, jpg, webp or gif picture".into(),
        );
    };
    Ok(Some(ProjectImage { mime, bytes }))
}

/// Reads at most `MAX_REFERENCE_BYTES`: a file that grew between the `fstat`
/// and the read comes back at MAX + 1 and is refused, never accepted.
fn read_capped(reader: impl std::io::Read, size_hint: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes =
        Vec::with_capacity(size_hint.min(crate::api::media::MAX_REFERENCE_BYTES) as usize);
    reader
        .take(crate::api::media::MAX_REFERENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read the project file: {error}"))?;
    if bytes.len() as u64 > crate::api::media::MAX_REFERENCE_BYTES {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(bytes)
}

/// A picture file of the project, as read.
struct ProjectImage {
    mime: &'static str,
    bytes: Vec<u8>,
}

/// The ids the agent named in `reference_asset_ids`.
fn named_references(call: &ToolCall) -> Vec<String> {
    call.arguments
        .get("reference_asset_ids")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Generations a single workflow run may start, whatever its steps; the
/// per-step ceiling of the request guard sits under it. A value above
/// [`MAX_RUN_CAP`] is lowered to it, so a typo cannot lift the guard for good.
pub const RUN_CAP_ENV: &str = "KRONN_WORKFLOW_MEDIA_JOBS_PER_RUN";
const DEFAULT_RUN_CAP: usize = 12;
pub const MAX_RUN_CAP: usize = 100;

fn run_cap() -> usize {
    bounded_run_cap(crate::core::child_env::var(RUN_CAP_ENV).ok().as_deref())
}

fn bounded_run_cap(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|cap| *cap > 0)
        .map_or(DEFAULT_RUN_CAP, |cap| cap.min(MAX_RUN_CAP))
}

/// One lock per run (per press for a Test): the cap checks and the job insert
/// of its steps run one at a time. Entries nobody holds are dropped.
fn run_lock(run_prefix: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::Mutex<
        Option<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::Mutex::new(None);
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let locks = locks.get_or_insert_with(Default::default);
    locks.retain(|_, lock| std::sync::Arc::strong_count(lock) > 1);
    locks.entry(run_prefix.to_string()).or_default().clone()
}

/// `<job>.<extension>` in the run's artifacts directory. Written through the
/// run directory's fd (KT-1585): an Exec step can write there, so a link
/// planted at the name is replaced, never written through.
fn copy_into_run(
    run_id: &str,
    job_id: &str,
    filename: &str,
    source: &str,
) -> std::io::Result<std::path::PathBuf> {
    let safe = |text: &str, max: usize| -> String {
        text.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .take(max)
            .collect()
    };
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| safe(extension, 8))
        .filter(|extension| !extension.is_empty())
        .unwrap_or_else(|| "bin".into());
    let name = format!("{}.{extension}", safe(job_id, 80));
    let directory = crate::workflows::run_artifacts::ensure(run_id)?;
    // The source is Kronn's own stored asset, not a path a step chose.
    let mut file = std::fs::File::open(source)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("the asset is not a regular file"));
    }
    crate::workflows::run_artifacts::open_run_dir(&directory)?.write_from(&name, &mut file)?;
    Ok(directory.join(name))
}

/// `{success, data}` from a handler becomes `data`; a refusal keeps its message.
fn flatten_envelope(mut outcome: ToolOutcome) -> ToolOutcome {
    if outcome.content.get("success").is_none() {
        return outcome;
    }
    outcome.content = if outcome.content["success"] == true {
        outcome.content["data"].take()
    } else {
        let message = outcome.content["error"]
            .as_str()
            .unwrap_or("the media request failed")
            .to_string();
        json!({ "error": message })
    };
    outcome
}

/// The two tools as a workflow Agent step is offered them: the discussion
/// declarations minus what needs a room to wake, plus the wait a step needs.
pub(super) fn workflow_declarations() -> Vec<Value> {
    orchestration_tool_catalogue()
        .into_iter()
        .filter_map(|mut tool| {
            let name = tool["function"]["name"].as_str()?.to_string();
            match name.as_str() {
                "media_generate" => {
                    tool["function"]["parameters"]["properties"]
                        .as_object_mut()?
                        .remove("wake_when_ready");
                    let description = tool["function"]["description"].as_str()?;
                    let (kept, _) = description.split_once(" The asset lands")?;
                    tool["function"]["description"] = json!(format!(
                        "{kept} The asset lands in a room Kronn opens for this run and, once ready, in the run's artifacts directory: wait for it with `media_job_status` and `wait_secs`."
                    ));
                    tool["function"]["parameters"]["properties"]["reference_asset_ids"]
                        ["description"] = json!(
                        "Pictures the generation starts from, each one of: the `job_id` of an image an earlier step of this run generated, or the `artifact_path` media_job_status returned for it; the context file id of a picture in this run's rooms or this project's discussions (a brand image a launch variable names); or the path of a png/jpg/webp/gif file inside the project, relative to its root. Nothing of another run or project is reachable. Pass `reference_mode` with it: `first_frame`/`last_frame` pin ONE picture of a video, `reference` takes several, up to agent_list's effective max_input_references (reference_support.blocker names a missing adapter or ComfyUI dependency, not a model limit)."
                    );
                }
                "media_job_status" => {
                    tool["function"]["description"] = json!(
                        "Read one media generation job by id: status, model and the asset once it is ready. `wait_secs` (up to 300) blocks until it settles; an unsettled answer says to call again, never to generate again. A finished asset comes with `artifact_path`, a file in the run's artifacts directory."
                    );
                    tool["function"]["parameters"]["properties"]["wait_secs"] = json!({
                        "type": "integer",
                        "description": "Wait up to this many seconds for the job to settle. 0 reads it once."
                    });
                }
                _ => return None,
            }
            Some(tool)
        })
        .collect()
}
