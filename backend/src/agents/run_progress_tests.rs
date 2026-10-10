use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::*;
use crate::acp::{AcpError, AcpSessionEvent};
use crate::agents::runner::{start_agent_with_config, AgentStartConfig};
use crate::models::{ActivityCategory, AgentType};

type Frames = Arc<Mutex<Vec<(u32, AgentRunProgress)>>>;

fn collect(progress: &RunProgress) -> (Frames, tokio::task::JoinHandle<()>) {
    let frames: Frames = Arc::default();
    let sink = frames.clone();
    let task = tokio::spawn(publish(progress.clone(), move |seq, frame| {
        sink.lock().unwrap().push((seq, frame));
    }));
    (frames, task)
}

/// The phases the frames went through, consecutive repeats folded.
fn phases(frames: &Frames) -> Vec<AgentRunPhase> {
    let mut seen: Vec<AgentRunPhase> = Vec::new();
    for (_, frame) in frames.lock().unwrap().iter() {
        if seen.last() != Some(&frame.phase) {
            seen.push(frame.phase);
        }
    }
    seen
}

fn timeline(frame: &AgentRunProgress) -> Vec<AgentRunPhase> {
    frame.timeline.iter().map(|mark| mark.phase).collect()
}

/// Every string an agent controls, as a tool call can carry it.
const SECRETS: [&str; 6] = [
    "/etc/kronn-secret",
    "notes-hunter2",
    "https://",
    "token=",
    "Read the private",
    "read_secret_file",
];

fn assert_no_leak(frames: &Frames) {
    for (_, frame) in frames.lock().unwrap().iter() {
        let json = serde_json::to_string(frame).unwrap();
        for secret in SECRETS {
            assert!(!json.contains(secret), "{secret} leaked: {json}");
        }
    }
}

/// An ACP tool call stuffed with targets: its title, raw input and locations.
fn leaky_tool_call() -> serde_json::Value {
    serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "kind": "read",
        "title": "Read the private /etc/kronn-secret/notes-hunter2.md",
        "rawInput": {
            "path": "/etc/kronn-secret/notes-hunter2.md",
            "url": "https://example.test/x?token=abc"
        },
        "locations": [{"path": "/etc/kronn-secret/notes-hunter2.md"}]
    })
}

#[test]
fn a_startup_phase_never_moves_back_nor_after_the_first_output() {
    let progress = RunProgress::new();
    progress.phase(AgentRunPhase::OpeningSession);
    progress.phase(AgentRunPhase::Initializing);
    assert_eq!(progress.snapshot().phase, AgentRunPhase::OpeningSession);
    progress.text();
    progress.phase(AgentRunPhase::WaitingModel);
    let snapshot = progress.snapshot();
    assert_eq!(snapshot.phase, AgentRunPhase::Responding);
    assert_eq!(
        timeline(&snapshot),
        vec![AgentRunPhase::Preparing, AgentRunPhase::OpeningSession]
    );
}

#[test]
fn nothing_changes_after_the_stop() {
    let progress = RunProgress::new();
    progress.idle_limit(Some(Duration::from_secs(60)));
    progress.stop(AgentRunStop::Idle);
    let stopped = progress.snapshot();
    progress.text();
    progress.tool(&ToolActivityUpdate::named(None, "Bash"));
    progress.stop(AgentRunStop::Finished);
    std::thread::sleep(Duration::from_millis(20));
    let later = progress.snapshot();
    assert_eq!(
        later, stopped,
        "a stopped run is frozen, its clock included"
    );
    assert_eq!(later.stopped, Some(AgentRunStop::Idle));
    assert_eq!(later.idle_limit_ms, None, "no countdown once stopped");
}

#[tokio::test(start_paused = true)]
async fn the_silence_restarts_only_on_real_activity() {
    let progress = RunProgress::new();
    progress.phase(AgentRunPhase::WaitingModel);
    tokio::time::advance(Duration::from_secs(7)).await;
    // Reading the progress, or publishing it, is not activity.
    let _ = progress.snapshot();
    assert_eq!(progress.snapshot().silent_ms, 7_000);
    progress.beat();
    assert_eq!(progress.snapshot().silent_ms, 0);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(progress.snapshot().silent_ms, 2_000);
    assert_eq!(progress.snapshot().elapsed_ms, 9_000);
}

/// A token storm costs a bounded number of frames, each with a higher number,
/// and the stop is always the last one.
#[tokio::test(start_paused = true)]
async fn frames_are_coalesced_numbered_and_end_with_the_stop() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    tokio::task::yield_now().await;
    progress.phase(AgentRunPhase::WaitingModel);
    for _ in 0..200 {
        // 20 s of streaming, one token every 100 ms.
        progress.text();
        tokio::time::advance(Duration::from_millis(100)).await;
    }
    progress.tool(&ToolActivityUpdate::named(Some("c1".into()), "Bash"));
    tokio::time::advance(Duration::from_millis(600)).await;
    progress.stop(AgentRunStop::Finished);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the publisher ends with the stop")
        .unwrap();

    let frames = frames.lock().unwrap();
    assert!(
        frames.len() <= 12,
        "{} frames for 200 tokens: not coalesced",
        frames.len()
    );
    assert!(frames.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let (_, last) = frames.last().unwrap();
    assert_eq!(last.stopped, Some(AgentRunStop::Finished));
    assert!(frames
        .iter()
        .any(|(_, frame)| frame.phase == AgentRunPhase::Tool));
    assert!(frames
        .iter()
        .any(|(_, frame)| frame.phase == AgentRunPhase::Responding));
}

#[tokio::test]
async fn dropping_the_guard_stops_the_publisher() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    drop(StopOnDrop(progress.clone()));
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the publisher ends")
        .unwrap();
    assert_eq!(
        frames.lock().unwrap().last().unwrap().1.stopped,
        Some(AgentRunStop::Failed)
    );
}

/// The no-leak rule: a tool call with a file path, a URL and a token in its
/// title and arguments leaves as its category, and only that.
#[tokio::test]
async fn a_tool_call_with_targets_leaves_as_its_category_only() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    let update = ToolActivityUpdate::from_acp(&leaky_tool_call()).unwrap();
    progress.tool(&update);
    progress.tool(&ToolActivityUpdate::named(None, "read_secret_file"));
    progress.tool(&ToolActivityUpdate::named(
        None,
        "mcp__github__create_issue",
    ));
    progress.stop(AgentRunStop::Finished);
    task.await.unwrap();

    let last = frames.lock().unwrap().last().unwrap().1.clone();
    let categories: Vec<_> = last.activity.iter().map(|entry| entry.category).collect();
    assert_eq!(
        categories,
        vec![
            ActivityCategory::Mcp,
            ActivityCategory::Other,
            ActivityCategory::Read
        ]
    );
    assert_no_leak(&frames);
    let json = serde_json::to_string(&last).unwrap();
    assert!(!json.contains("github"), "{json}");
}

// ─── The ACP host's own phases, on a scripted transport ─────────────────────

#[derive(Clone, Copy)]
enum Script {
    /// A tool call full of targets, a thought, then the answer.
    Answers,
    /// `session/new` never answers within its bound.
    SessionBlocks,
    /// The prompt is sent and nothing ever comes back.
    Silent,
}

struct ScriptedRuntime(Script);

#[async_trait::async_trait]
impl crate::acp::AcpTransport for ScriptedRuntime {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, AcpError> {
        use crate::acp::AcpCapability;
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                AcpCapability::Sessions,
                AcpCapability::Streaming,
                AcpCapability::Cancellation,
                AcpCapability::McpInjection,
            ]),
        })
    }

    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, AcpError> {
        if let Script::SessionBlocks = self.0 {
            return Err(AcpError::Timeout("session/new".into()));
        }
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::OpenCode, "progress-session")
    }

    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }

    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }

    async fn resume_session(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }

    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        match self.0 {
            Script::Answers => {
                let call = leaky_tool_call();
                events
                    .send(AcpSessionEvent::ToolCall {
                        id: Some("call-1".into()),
                        name: call["title"].as_str().unwrap().into(),
                    })
                    .await
                    .unwrap();
                events
                    .send(AcpSessionEvent::ToolActivity(
                        ToolActivityUpdate::from_acp(&call).unwrap(),
                    ))
                    .await
                    .unwrap();
                tokio::time::sleep(MIN_GAP * 2).await;
                events
                    .send(AcpSessionEvent::ToolCallEnded {
                        id: Some("call-1".into()),
                    })
                    .await
                    .unwrap();
                events.send(AcpSessionEvent::Thought).await.unwrap();
                tokio::time::sleep(MIN_GAP * 2).await;
                events
                    .send(AcpSessionEvent::TextDelta("the answer".into()))
                    .await
                    .unwrap();
                events.send(AcpSessionEvent::Completed).await.unwrap();
                Ok(())
            }
            Script::SessionBlocks => unreachable!("no session, no prompt"),
            Script::Silent => std::future::pending().await,
        }
    }

    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), AcpError> {
        Ok(())
    }
}

async fn launch(
    script: Script,
    idle: Duration,
    progress: &RunProgress,
) -> Result<crate::agents::runner::AgentProcess, String> {
    let project = tempfile::tempdir().unwrap();
    let tokens = crate::models::setup::TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    };
    let _saved = crate::core::config::test_saved_access::set(&AgentType::OpenCode, true);
    start_agent_with_config(AgentStartConfig {
        idle_timeout: Some(idle),
        test_acp_transport: Some(Arc::new(ScriptedRuntime(script))),
        full_access: true,
        run_progress: Some(progress.clone()),
        ..AgentStartConfig::new(
            &AgentType::OpenCode,
            project.path().to_str().unwrap(),
            "hello",
            &tokens,
        )
    })
    .await
}

async fn drain(process: &mut crate::agents::runner::AgentProcess) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while process.next_line().await.is_some() {}
    })
    .await
    .expect("the run ends");
}

#[tokio::test]
async fn an_acp_run_reports_its_phases_in_order_then_its_tools_by_category() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    let mut process = launch(Script::Answers, Duration::from_secs(30), &progress)
        .await
        .expect("the run starts");
    drain(&mut process).await;
    assert!(process.child.wait().await.unwrap().success());
    progress.stop(AgentRunStop::Finished);
    task.await.unwrap();

    let last = frames.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(
        timeline(&last),
        vec![
            AgentRunPhase::Preparing,
            AgentRunPhase::Initializing,
            AgentRunPhase::OpeningSession,
            AgentRunPhase::WaitingModel,
        ]
    );
    assert!(
        last.mcp_servers.is_some(),
        "the session's servers are counted"
    );
    let seen = phases(&frames);
    let order = |phase| seen.iter().position(|seen| *seen == phase);
    assert!(order(AgentRunPhase::Tool) < order(AgentRunPhase::Thinking));
    assert!(order(AgentRunPhase::Thinking) < order(AgentRunPhase::Responding));
    assert_eq!(last.tool_calls, 1);
    assert_eq!(last.activity[0].category, ActivityCategory::Read);
    assert_eq!(last.stopped, Some(AgentRunStop::Finished));
    assert_no_leak(&frames);
}

#[tokio::test]
async fn a_session_that_does_not_open_stops_on_its_bound() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    let error = launch(Script::SessionBlocks, Duration::from_secs(30), &progress)
        .await
        .err()
        .expect("the start fails");
    let failure = crate::agents::acp_start::AcpStartFailure::from_error(&error).unwrap();
    assert_eq!(
        failure.phase,
        crate::agents::acp_start::AcpStartPhase::Session
    );
    // While it was opening, the bubble counted down the session bound.
    let opening = progress.snapshot();
    assert_eq!(opening.phase, AgentRunPhase::OpeningSession);
    assert_eq!(
        opening.idle_limit_ms,
        Some(millis(crate::acp::SESSION_SETUP_TIMEOUT))
    );
    progress.stop(AgentRunStop::TimedOut);
    task.await.unwrap();
    let seen = phases(&frames);
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
    let last = frames.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(
        timeline(&last),
        vec![
            AgentRunPhase::Preparing,
            AgentRunPhase::Initializing,
            AgentRunPhase::OpeningSession,
        ]
    );
    assert_eq!(last.phase, AgentRunPhase::OpeningSession);
    assert_eq!(last.stopped, Some(AgentRunStop::TimedOut));
}

#[tokio::test]
async fn a_silent_run_counts_down_then_is_stopped_for_inactivity() {
    let progress = RunProgress::new();
    let (frames, task) = collect(&progress);
    let idle = Duration::from_millis(1_200);
    let mut process = launch(Script::Silent, idle, &progress)
        .await
        .expect("the run starts");
    drain(&mut process).await;
    assert!(!process.child.wait().await.unwrap().success());
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the stop ends the frames")
        .unwrap();

    let frames_seen = frames.lock().unwrap().clone();
    let waiting = frames_seen
        .iter()
        .find(|(_, frame)| frame.phase == AgentRunPhase::WaitingModel)
        .expect("a frame while waiting for the model");
    assert_eq!(waiting.1.idle_limit_ms, Some(1_200));
    let (_, last) = frames_seen.last().unwrap();
    assert_eq!(last.stopped, Some(AgentRunStop::Idle));
    assert_eq!(last.phase, AgentRunPhase::WaitingModel);
    assert!(last.silent_ms >= 1_000, "{}", last.silent_ms);
}

// ─── The snapshot a page reads when it opens or reconnects mid-run ──────────

fn identity(run_id: &str) -> RunIdentity {
    RunIdentity {
        discussion_id: "d1".into(),
        dispatch_id: Some("job-1".into()),
        trigger_message_id: Some("u1".into()),
        agent_type: AgentType::OpenCode,
        run_id: run_id.into(),
        started_at: chrono::Utc::now(),
    }
}

fn snapshot_frame(live: &LiveRuns) -> (u32, AgentRunProgress) {
    match live.snapshot("d1").pop().expect("the run is listed") {
        WsMessage::AgentRunProgress { seq, progress, .. } => (seq, progress),
        other => panic!("{other:?}"),
    }
}

/// A page opened during a long silence sees the phase and the countdown at
/// once, with the number of the last frame sent so it never outranks a newer
/// live frame; and the run leaves the list with its stop.
#[tokio::test]
async fn a_run_is_readable_mid_silence_and_leaves_with_its_stop() {
    let live = Arc::new(LiveRuns::default());
    let progress = RunProgress::new();
    let sent: Arc<Mutex<Vec<u32>>> = Arc::default();
    let sink = sent.clone();
    let task = tokio::spawn(publish_registered(
        live.clone(),
        identity("run-1"),
        progress.clone(),
        move |frame| {
            if let WsMessage::AgentRunProgress { seq, .. } = frame {
                sink.lock().unwrap().push(seq);
            }
        },
    ));
    progress.phase(AgentRunPhase::WaitingModel);
    progress.idle_limit(Some(Duration::from_secs(300)));
    tokio::time::sleep(MIN_GAP * 3).await;

    let (seq, frame) = snapshot_frame(&live);
    assert_eq!(seq, *sent.lock().unwrap().last().unwrap());
    assert_eq!(frame.phase, AgentRunPhase::WaitingModel);
    assert_eq!(frame.idle_limit_ms, Some(300_000));
    assert!(live.snapshot("other-discussion").is_empty());

    progress.stop(AgentRunStop::Cancelled);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(live.snapshot("d1").is_empty());
}

/// Reading the snapshot is not activity: the silence and the frame numbers
/// go on as if nobody had looked.
#[tokio::test(start_paused = true)]
async fn reading_the_snapshot_moves_neither_the_silence_nor_the_frame_numbers() {
    let live = LiveRuns::default();
    let progress = RunProgress::new();
    progress.phase(AgentRunPhase::WaitingModel);
    live.insert(identity("run-1"), progress.clone());
    tokio::time::advance(Duration::from_secs(4)).await;
    let (seq_before, first) = snapshot_frame(&live);
    tokio::time::advance(Duration::from_secs(3)).await;
    let (seq_after, second) = snapshot_frame(&live);
    assert_eq!(first.silent_ms, 4_000);
    assert_eq!(
        second.silent_ms, 7_000,
        "the read did not restart the silence"
    );
    assert_eq!(seq_before, seq_after);
    assert_eq!(progress.snapshot().silent_ms, 7_000);
}

/// The snapshot route is not open to an agent's bridge token.
#[test]
fn the_snapshot_route_is_not_on_the_bridge_token_list() {
    assert!(
        crate::core::bridge_token::route_for("GET", "/api/discussions/{id}/run-progress").is_none()
    );
}

/// After a tool, reasoning or text, a new request waits for the model's next
/// answer without reopening the startup steps.
#[test]
fn a_request_after_output_waits_for_the_next_answer() {
    let progress = RunProgress::new();
    progress.request_sent(Some(Duration::from_secs(60)));
    assert_eq!(progress.snapshot().phase, AgentRunPhase::WaitingModel);
    progress.tool(&ToolActivityUpdate::named(None, "Bash"));
    progress.request_sent(Some(Duration::from_secs(30)));
    let next = progress.snapshot();
    assert_eq!(next.phase, AgentRunPhase::WaitingNextAnswer);
    assert_eq!(next.idle_limit_ms, Some(30_000));
    assert_eq!(
        timeline(&next),
        vec![AgentRunPhase::Preparing, AgentRunPhase::WaitingModel]
    );
    progress.backoff();
    assert_eq!(progress.snapshot().idle_limit_ms, None);
}
