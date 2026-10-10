//! KT-1108 — a run's live progress for its reply bubble: the startup phase it
//! is in, its tool calls by category, how long it has been silent and the
//! inactivity delay in force.
//!
//! Every update comes from the code path that observed it (the ACP host, the
//! CLI's stream, the HTTP tool loop); nothing here advances on a timer. What
//! leaves is [`AgentRunProgress`]: phases, categories, counts and durations,
//! never a tool's name, argument or target, nor any agent text.
//!
//! [`publish`] turns the updates into frames: a phase, tool or stop change at
//! most every [`MIN_GAP`], a mere sign of life at most every [`BEAT_GAP`], so a
//! streaming answer never costs one frame per token.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use super::activity::{RecentActivity, ToolActivityUpdate};
use crate::models::{
    AgentRunPhase, AgentRunPhaseMark, AgentRunProgress, AgentRunStop, AgentType, WsMessage,
};

/// The shortest gap between two frames of one run.
pub const MIN_GAP: Duration = Duration::from_millis(500);
/// The longest a sign of life waits before a frame carries it.
pub const BEAT_GAP: Duration = Duration::from_secs(5);
/// Startup phases kept in the timeline; there are fewer kinds than this.
const TIMELINE_MAX: usize = 8;

/// What changed since the last frame: `significant` for a phase, a tool or a
/// stop, `beats` for a mere sign of life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Revision {
    significant: u64,
    beats: u64,
}

#[derive(Debug)]
struct State {
    phase: AgentRunPhase,
    phase_since: Instant,
    timeline: Vec<AgentRunPhaseMark>,
    mcp_servers: Option<u32>,
    recent: RecentActivity,
    tool_calls: u32,
    last_alive: Instant,
    idle_limit: Option<Duration>,
    stopped: Option<(AgentRunStop, Instant)>,
}

struct Shared {
    origin: Instant,
    state: Mutex<State>,
    revision: watch::Sender<Revision>,
    /// The number of the last frame published.
    seq: AtomicU32,
    /// Every significant state, in order, for tests that must see a short one.
    #[cfg(test)]
    history: Mutex<Vec<HistoryEntry>>,
}

/// One run's progress. Cloning shares it.
#[derive(Clone)]
pub struct RunProgress(Arc<Shared>);

impl std::fmt::Debug for RunProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunProgress").finish_non_exhaustive()
    }
}

impl Default for RunProgress {
    fn default() -> Self {
        Self::new()
    }
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

impl RunProgress {
    /// Starts the run's clock, in [`AgentRunPhase::Preparing`].
    pub fn new() -> Self {
        let origin = Instant::now();
        Self(Arc::new(Shared {
            origin,
            state: Mutex::new(State {
                phase: AgentRunPhase::Preparing,
                phase_since: origin,
                timeline: vec![AgentRunPhaseMark {
                    phase: AgentRunPhase::Preparing,
                    at_ms: 0,
                }],
                mcp_servers: None,
                recent: RecentActivity::default(),
                tool_calls: 0,
                last_alive: origin,
                idle_limit: None,
                stopped: None,
            }),
            revision: watch::channel(Revision::default()).0,
            seq: AtomicU32::new(0),
            #[cfg(test)]
            history: Mutex::default(),
        }))
    }

    /// Applies `change` unless the run already stopped, then wakes the
    /// publisher. `change` says whether the update is significant.
    fn update(&self, change: impl FnOnce(&mut State, Instant) -> Option<bool>) {
        let significant = {
            let Ok(mut state) = self.0.state.lock() else {
                return;
            };
            if state.stopped.is_some() {
                return;
            }
            let now = Instant::now();
            match change(&mut state, now) {
                Some(significant) => {
                    #[cfg(test)]
                    if significant {
                        if let Ok(mut history) = self.0.history.lock() {
                            history.push(HistoryEntry {
                                phase: state.phase,
                                limit_ms: state.idle_limit.map(millis),
                                silence_restarted: state.last_alive == now,
                            });
                        }
                    }
                    significant
                }
                None => return,
            }
        };
        self.0.revision.send_modify(|revision| {
            if significant {
                revision.significant += 1;
            } else {
                revision.beats += 1;
            }
        });
    }

    /// The run reached `phase`. A startup phase only ever moves forward, and
    /// never once the model has answered: a late report of an earlier step
    /// cannot rewind the bubble.
    pub fn phase(&self, phase: AgentRunPhase) {
        self.update(|state, now| {
            state.last_alive = now;
            if phase == state.phase
                || (phase.is_startup() && (!state.phase.is_startup() || phase < state.phase))
            {
                return Some(false);
            }
            set_phase(state, phase, now, self.0.origin);
            Some(true)
        });
    }

    /// Kronn sent the model a request, bounded by `limit` until its first
    /// output: the startup wait the first time, the wait for its next answer
    /// after a tool, reasoning or text. The silence restarts with the bound.
    pub fn request_sent(&self, limit: Option<Duration>) {
        self.update(|state, now| {
            state.last_alive = now;
            let phase = if state.phase.is_startup() {
                AgentRunPhase::WaitingModel
            } else {
                AgentRunPhase::WaitingNextAnswer
            };
            let changed = state.phase != phase || state.idle_limit != limit;
            if state.phase != phase {
                set_phase(state, phase, now, self.0.origin);
            }
            state.idle_limit = limit;
            Some(changed)
        });
    }

    /// A new bound starts now (the headers came, the body is read next): the
    /// silence restarts with it, and the bubble shows it at once.
    pub fn restart_deadline(&self, limit: Option<Duration>) {
        self.update(|state, now| {
            state.last_alive = now;
            state.idle_limit = limit;
            Some(true)
        });
    }

    /// Kronn waits before retrying a request: no inactivity bound runs meanwhile.
    pub fn backoff(&self) {
        self.idle_limit(None);
    }

    /// The significant states so far, in order; for tests.
    #[cfg(test)]
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.0.history.lock().map(|h| h.clone()).unwrap_or_default()
    }

    /// The session declares `count` MCP servers, which it starts while opening.
    pub fn mcp_servers(&self, count: usize) {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        self.update(|state, _| {
            let changed = state.mcp_servers != Some(count);
            state.mcp_servers = Some(count);
            changed.then_some(true)
        });
    }

    /// The silence after which Kronn stops the agent from now on.
    pub fn idle_limit(&self, limit: Option<Duration>) {
        self.update(|state, _| {
            let changed = state.idle_limit != limit;
            state.idle_limit = limit;
            changed.then_some(true)
        });
    }

    /// A sign of life with nothing to show.
    pub fn beat(&self) {
        self.update(|state, now| {
            state.last_alive = now;
            Some(false)
        });
    }

    /// The model is reasoning.
    pub fn thought(&self) {
        self.phase(AgentRunPhase::Thinking);
    }

    /// The model wrote part of its answer.
    pub fn text(&self) {
        self.phase(AgentRunPhase::Responding);
    }

    /// A tool call started or progressed: only its category is kept.
    pub fn tool(&self, update: &ToolActivityUpdate) {
        self.update(|state, now| {
            state.last_alive = now;
            if !state.recent.apply(update) {
                return Some(false);
            }
            state.tool_calls = state.tool_calls.saturating_add(1);
            if state.phase != AgentRunPhase::Tool {
                set_phase(state, AgentRunPhase::Tool, now, self.0.origin);
            }
            Some(true)
        });
    }

    /// The run ended. The first reason wins; nothing changes afterwards.
    pub fn stop(&self, reason: AgentRunStop) {
        self.update(|state, now| {
            state.stopped = Some((reason, now));
            state.idle_limit = None;
            Some(true)
        });
    }

    pub fn is_stopped(&self) -> bool {
        self.0
            .state
            .lock()
            .map(|state| state.stopped.is_some())
            .unwrap_or(true)
    }

    /// The progress as a frame carries it, measured now (or at the stop).
    pub fn snapshot(&self) -> AgentRunProgress {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = state.stopped.map_or_else(Instant::now, |(_, at)| at);
        AgentRunProgress {
            phase: state.phase,
            phase_ms: millis(now.saturating_duration_since(state.phase_since)),
            elapsed_ms: millis(now.saturating_duration_since(self.0.origin)),
            timeline: state.timeline.clone(),
            mcp_servers: state.mcp_servers,
            activity: state.recent.snapshot().entries,
            tool_calls: state.tool_calls,
            silent_ms: millis(now.saturating_duration_since(state.last_alive)),
            idle_limit_ms: state.idle_limit.map(millis),
            stopped: state.stopped.map(|(reason, _)| reason),
        }
    }

    /// The number of the last frame published; a snapshot carries it, so a
    /// reader never takes it for newer than a frame it already has.
    pub fn published_seq(&self) -> u32 {
        self.0.seq.load(Ordering::Acquire)
    }

    fn next_seq(&self) -> u32 {
        self.0.seq.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    }

    fn revisions(&self) -> watch::Receiver<Revision> {
        self.0.revision.subscribe()
    }
}

/// One significant state as it was recorded; for tests.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    pub phase: AgentRunPhase,
    pub limit_ms: Option<u32>,
    /// The update itself restarted the silence.
    pub silence_restarted: bool,
}

fn set_phase(state: &mut State, phase: AgentRunPhase, now: Instant, origin: Instant) {
    state.phase = phase;
    state.phase_since = now;
    if phase.is_startup() && state.timeline.len() < TIMELINE_MAX {
        state.timeline.push(AgentRunPhaseMark {
            phase,
            at_ms: millis(now.saturating_duration_since(origin)),
        });
    }
}

/// Stops the run as failed if it is dropped before a stop was reported, so the
/// publisher always ends and no bubble keeps counting.
pub struct StopOnDrop(pub RunProgress);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop(AgentRunStop::Failed);
    }
}

/// Hands `progress` to `emit` as frames numbered from 1, until the frame that
/// carries its stop. A phase, tool or stop change goes out at most every
/// [`MIN_GAP`]; a sign of life alone waits up to [`BEAT_GAP`].
pub async fn publish(progress: RunProgress, mut emit: impl FnMut(u32, AgentRunProgress)) {
    let mut revisions = progress.revisions();
    let mut sent: Option<Revision> = None;
    let mut sent_at = Instant::now();
    loop {
        let revision = *revisions.borrow_and_update();
        let due = match sent {
            None => true,
            Some(previous) => {
                previous.significant != revision.significant
                    || (previous.beats != revision.beats && sent_at.elapsed() >= BEAT_GAP)
            }
        };
        if due {
            let snapshot = progress.snapshot();
            let stopped = snapshot.stopped.is_some();
            emit(progress.next_seq(), snapshot);
            sent = Some(revision);
            sent_at = Instant::now();
            if stopped {
                return;
            }
            tokio::time::sleep(MIN_GAP).await;
            continue;
        }
        let beat_pending = sent.is_some_and(|previous| previous.beats != revision.beats);
        if beat_pending {
            tokio::select! {
                changed = revisions.changed() => if changed.is_err() { return },
                _ = tokio::time::sleep_until(sent_at + BEAT_GAP) => {}
            }
        } else if revisions.changed().await.is_err() {
            return;
        }
    }
}

/// Which reply a run belongs to: what every frame of it carries.
#[derive(Debug, Clone)]
pub struct RunIdentity {
    pub discussion_id: String,
    pub dispatch_id: Option<String>,
    pub trigger_message_id: Option<String>,
    pub agent_type: AgentType,
    pub run_id: String,
    /// When this launch started: a later attempt of the same dispatch has a later one.
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl RunIdentity {
    pub fn frame(&self, seq: u32, progress: AgentRunProgress) -> WsMessage {
        WsMessage::AgentRunProgress {
            discussion_id: self.discussion_id.clone(),
            dispatch_id: self.dispatch_id.clone(),
            trigger_message_id: self.trigger_message_id.clone(),
            agent_type: self.agent_type.clone(),
            run_id: self.run_id.clone(),
            started_at: self.started_at,
            seq,
            progress,
        }
    }
}

/// The runs in progress, so a page opened (or reconnected) mid-run can show
/// one without waiting for its next frame.
#[derive(Default)]
pub struct LiveRuns(Mutex<HashMap<String, (RunIdentity, RunProgress)>>);

impl LiveRuns {
    pub fn insert(&self, identity: RunIdentity, progress: RunProgress) {
        if let Ok(mut runs) = self.0.lock() {
            runs.insert(identity.run_id.clone(), (identity, progress));
        }
    }

    pub fn remove(&self, run_id: &str) {
        if let Ok(mut runs) = self.0.lock() {
            runs.remove(run_id);
        }
    }

    /// One frame per run of `discussion_id`, measured now. Reading changes
    /// nothing: neither the run's silence nor its frame numbers.
    pub fn snapshot(&self, discussion_id: &str) -> Vec<WsMessage> {
        let Ok(runs) = self.0.lock() else {
            return Vec::new();
        };
        let mut frames: Vec<_> = runs
            .values()
            .filter(|(identity, _)| identity.discussion_id == discussion_id)
            .map(|(identity, progress)| {
                (
                    identity.started_at,
                    identity.frame(progress.published_seq(), progress.snapshot()),
                )
            })
            .collect();
        frames.sort_by_key(|(started_at, _)| *started_at);
        frames.into_iter().map(|(_, frame)| frame).collect()
    }
}

/// Registers the run in `live`, then publishes it as frames on `emit` until
/// its stop, when it leaves `live`.
pub async fn publish_registered(
    live: Arc<LiveRuns>,
    identity: RunIdentity,
    progress: RunProgress,
    emit: impl Fn(WsMessage),
) {
    live.insert(identity.clone(), progress.clone());
    publish(progress, |seq, snapshot| {
        emit(identity.frame(seq, snapshot))
    })
    .await;
    live.remove(&identity.run_id);
}

#[cfg(test)]
#[path = "run_progress_tests.rs"]
mod tests;
