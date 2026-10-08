//! KT-932 — a model that goes silent fails the run, and a run that stops frees
//! the model.
//!
//! Both halves are tested against [`SimulatedOllama`], a stand-in on a raw
//! socket. wiremock answers a request and forgets it; this one keeps watching
//! the connection, so a test can see what a real Ollama sees: whether the client
//! is still there. It also behaves like Ollama where it matters here — one
//! generation at a time on a loaded model, so a request queues behind the one
//! before it until that one finishes or its client hangs up.

use super::*;
use serial_test::serial;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore};

/// What the simulated model does with the next `/api/chat` request.
#[derive(Clone, Copy)]
enum Generation {
    /// Takes the request and never says anything: weights that never finish
    /// loading, or a connection that died while the machine slept.
    Mute,
    /// `chunks` tokens, then silence for ever, the connection left open.
    Dies { chunks: usize },
    /// Slow but alive: a first token after `first_after`, then one every
    /// `every`, `chunks` in all, then a normal end.
    Alive {
        first_after: Duration,
        every: Duration,
        chunks: usize,
    },
    /// Generates for ever. Only the client hanging up ends it.
    Endless,
    /// One token and a normal end.
    Quick,
    /// Answers at once with a call to `tool`, handing control to Kronn's own
    /// executor instead of generating further.
    ToolCall { tool: &'static str },
}

struct SimulatedOllama {
    base: String,
    port: u16,
    /// Generations that ended because their client was gone, not because they
    /// were finished.
    abandoned: Arc<AtomicUsize>,
    /// Pulsed once a generation has written its first token.
    started: Arc<Notify>,
}

impl SimulatedOllama {
    async fn start(script: Vec<Generation>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        let abandoned = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        // The model server's one runner slot.
        let slot = Arc::new(Semaphore::new(1));
        let (abandoned_for_server, started_for_server) = (abandoned.clone(), started.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(
                    stream,
                    slot.clone(),
                    script.clone(),
                    abandoned_for_server.clone(),
                    started_for_server.clone(),
                ));
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            port,
            abandoned,
            started,
        }
    }

    /// Wait until `count` generations have been abandoned by their client.
    async fn wait_abandoned(&self, count: usize, within: Duration) {
        let waited = tokio::time::timeout(within, async {
            while self.abandoned.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "the model server never saw its client hang up ({} abandoned, wanted {count}): \
             the generation is still running for nobody",
            self.abandoned.load(Ordering::SeqCst)
        );
    }

    /// The request that comes next. Behind a generation nobody reads it would
    /// wait for ever; with the model free it is answered at once.
    async fn assert_next_request_is_served(&self) {
        let client = reqwest::Client::new();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client
                .post(format!("{}/api/chat", self.base))
                .json(&serde_json::json!({"model": "sim-model", "stream": true}))
                .send(),
        )
        .await
        .expect("the next request queued behind a generation nobody reads")
        .expect("the model server answered");
        let body = tokio::time::timeout(Duration::from_secs(5), response.text())
            .await
            .expect("the answer arrived")
            .expect("readable");
        assert!(body.contains(r#""done":true"#), "{body}");
    }
}

async fn serve(
    stream: TcpStream,
    slot: Arc<Semaphore>,
    script: Arc<Mutex<VecDeque<Generation>>>,
    abandoned: Arc<AtomicUsize>,
    started: Arc<Notify>,
) {
    let (mut read, mut write) = stream.into_split();
    let Some(path) = read_request(&mut read).await else {
        return;
    };
    if path != "/api/chat" {
        // The runner's model probes (`/api/show`, `/api/version`, …): not
        // served, and it carries on with its portable defaults.
        let _ = write
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let generation = script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Generation::Quick);
    // One generation at a time. A client that gives up while it waits its turn
    // leaves the queue.
    let permit = tokio::select! {
        permit = slot.acquire_owned() => permit.expect("slot"),
        _ = hung_up(&mut read) => return,
    };
    let finished = tokio::select! {
        outcome = generate(&mut write, generation, &started) => outcome.is_ok(),
        _ = hung_up(&mut read) => false,
    };
    if !finished {
        abandoned.fetch_add(1, Ordering::SeqCst);
    }
    drop(permit);
}

/// Request head and body, and the path asked for.
async fn read_request(read: &mut OwnedReadHalf) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        let read_now = read.read(&mut chunk).await.ok()?;
        if read_now == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read_now]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let path = head.lines().next()?.split_whitespace().nth(1)?.to_owned();
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let read_now = read.read(&mut chunk).await.ok()?;
        if read_now == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read_now]);
    }
    Some(path)
}

/// Resolves when the client closes its end.
async fn hung_up(read: &mut OwnedReadHalf) {
    let mut sink = [0u8; 256];
    loop {
        match read.read(&mut sink).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn write_chunk(write: &mut OwnedWriteHalf, data: &str) -> std::io::Result<()> {
    write
        .write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes())
        .await?;
    write.flush().await
}

async fn write_token(write: &mut OwnedWriteHalf, text: &str) -> std::io::Result<()> {
    let line = format!(
        r#"{{"message":{{"role":"assistant","content":"{text}"}},"done":false}}{}"#,
        "\n"
    );
    write_chunk(write, &line).await
}

async fn generate(
    write: &mut OwnedWriteHalf,
    generation: Generation,
    started: &Notify,
) -> std::io::Result<()> {
    const HEAD: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n";
    match generation {
        Generation::Mute => std::future::pending().await,
        Generation::Dies { chunks } => {
            write.write_all(HEAD).await?;
            for index in 0..chunks {
                write_token(write, &format!("tok{index}")).await?;
                started.notify_one();
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            std::future::pending().await
        }
        Generation::Alive {
            first_after,
            every,
            chunks,
        } => {
            tokio::time::sleep(first_after).await;
            write.write_all(HEAD).await?;
            for index in 0..chunks {
                write_token(write, &format!("tok{index}")).await?;
                started.notify_one();
                tokio::time::sleep(every).await;
            }
            finish(write).await
        }
        Generation::Endless => {
            write.write_all(HEAD).await?;
            loop {
                write_token(write, "tok").await?;
                started.notify_one();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Generation::Quick => {
            write.write_all(HEAD).await?;
            write_token(write, "ok").await?;
            finish(write).await
        }
        Generation::ToolCall { tool } => {
            write.write_all(HEAD).await?;
            write_chunk(
                write,
                &format!(
                    r#"{{"message":{{"content":"","tool_calls":[{{"function":{{"name":"{tool}","arguments":{{}}}}}}]}},"done":false}}{}"#,
                    "\n"
                ),
            )
            .await?;
            started.notify_one();
            finish(write).await
        }
    }
}

async fn finish(write: &mut OwnedWriteHalf) -> std::io::Result<()> {
    write_chunk(
        write,
        concat!(
            r#"{"message":{"role":"assistant","content":""},"done":true,"#,
            r#""prompt_eval_count":5,"eval_count":2}"#,
            "\n"
        ),
    )
    .await?;
    write.write_all(b"0\r\n\r\n").await?;
    write.flush().await
}

/// What a test needs to start the native Ollama path against the simulation.
async fn start_native(
    base: &str,
    idle: Option<Duration>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
    format: Option<&serde_json::Value>,
) -> Result<AgentProcess, String> {
    start_ollama_http_with_idle(
        &AgentType::Ollama,
        "think about it",
        "",
        "sim-model",
        format,
        Some(base),
        None,
        None,
        None,
        None,
        cancel,
        None,
        None,
        None,
        idle,
        None,
        None,
    )
    .await
}

async fn drain(process: &mut AgentProcess) -> String {
    let mut text = String::new();
    while let Some(line) = process.next_line().await {
        text.push_str(&line);
    }
    text
}

fn stderr_of(process: &AgentProcess) -> String {
    process.stderr_capture.lock().unwrap().join("\n")
}

// ─── native HTTP: stop and cancel free the model (DoD 3) ─────────────────────

#[derive(Clone, Copy, Debug)]
enum Stop {
    /// The consumer kills the run: a stall, a size cap, a terminal signal.
    Kill,
    /// The caller's token fires: the Stop button, the batch budget.
    Cancel,
    /// The consumer walks away without a word: a timeout teardown.
    Drop,
}

#[tokio::test]
#[serial]
async fn stopping_a_native_ollama_run_frees_the_model_for_the_next_request() {
    for stop in [Stop::Kill, Stop::Cancel, Stop::Drop] {
        let ollama = SimulatedOllama::start(vec![Generation::Endless, Generation::Quick]).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut running = start_native(&ollama.base, None, Some(&cancel), None)
            .await
            .expect("the run starts");
        tokio::time::timeout(Duration::from_secs(5), ollama.started.notified())
            .await
            .expect("the model is generating");
        assert!(running.next_line().await.is_some(), "{stop:?}: tokens flow");

        match stop {
            Stop::Kill => running.kill().await,
            Stop::Cancel => cancel.cancel(),
            Stop::Drop => drop(running),
        }

        ollama.wait_abandoned(1, Duration::from_secs(5)).await;
        ollama.assert_next_request_is_served().await;
    }
}

// ─── native HTTP: a dead stream fails with its reason (DoD 1) ────────────────

#[tokio::test]
#[serial]
async fn a_native_ollama_stream_that_goes_mute_fails_the_run_and_frees_the_model() {
    let ollama =
        SimulatedOllama::start(vec![Generation::Dies { chunks: 2 }, Generation::Quick]).await;
    let started = std::time::Instant::now();
    let mut running = start_native(&ollama.base, Some(Duration::from_secs(1)), None, None)
        .await
        .expect("the run starts");

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert_eq!(text, "tok0tok1", "what arrived before the silence is kept");
    assert!(
        silence >= Duration::from_secs(1) && silence < Duration::from_secs(8),
        "the run ended on the silence, not before and not never: {silence:?}"
    );
    let status = running.child.wait().await.expect("lifeline");
    assert!(!status.success(), "a dead stream is a failed run");
    let reason = stderr_of(&running);
    assert!(
        reason
            .lines()
            .any(crate::agents::idle_watchdog::is_stall_reason),
        "the failure says why: {reason}"
    );
    assert!(reason.contains("Ollama sent no data for 1 s"), "{reason}");
    assert!(
        crate::workflows::steps::is_stall_error(&reason),
        "a step's on_timeout routing still recognises it: {reason}"
    );

    ollama.wait_abandoned(1, Duration::from_secs(5)).await;
    ollama.assert_next_request_is_served().await;
}

#[tokio::test]
#[serial]
async fn a_native_ollama_model_that_never_starts_fails_the_start_and_frees_the_model() {
    let ollama = SimulatedOllama::start(vec![Generation::Mute, Generation::Quick]).await;
    let started = std::time::Instant::now();
    let error = match start_native(&ollama.base, Some(Duration::from_secs(1)), None, None).await {
        Ok(_) => panic!("a model that says nothing cannot have started a run"),
        Err(error) => error,
    };

    assert!(
        started.elapsed() >= Duration::from_secs(1) && started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert!(
        crate::workflows::steps::is_stall_error(&error),
        "recognised as a stall: {error}"
    );
    assert!(
        error.contains("without ever sending a first token"),
        "{error}"
    );
    assert!(
        !error.contains("unreachable"),
        "the server was reached and said nothing: {error}"
    );

    ollama.wait_abandoned(1, Duration::from_secs(5)).await;
    ollama.assert_next_request_is_served().await;
}

// ─── native HTTP: a slow but living model is not cut (DoD 2) ─────────────────

#[tokio::test]
#[serial]
async fn a_slow_but_living_native_ollama_run_is_not_cut() {
    // A first token after 700 ms, then a token every 400 ms for 2.4 s: 3.1 s in
    // all against a 1 s delay. A budget on the whole run would have cut
    // it; a silence limit never sees one.
    let ollama = SimulatedOllama::start(vec![Generation::Alive {
        first_after: Duration::from_millis(700),
        every: Duration::from_millis(400),
        chunks: 6,
    }])
    .await;
    let mut running = start_native(&ollama.base, Some(Duration::from_secs(1)), None, None)
        .await
        .expect("the run starts");

    let text = drain(&mut running).await;

    assert_eq!(text, "tok0tok1tok2tok3tok4tok5");
    assert!(
        running.child.wait().await.expect("lifeline").success(),
        "{}",
        stderr_of(&running)
    );
    assert!(!stderr_of(&running).contains("stalled"));
    assert_eq!(ollama.abandoned.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[serial]
async fn a_request_that_is_not_streamed_is_allowed_its_whole_generation() {
    // A constrained-JSON request answers once, when it is finished: there is
    // nothing on the wire while it runs, and that says nothing about whether
    // the model is alive.
    let ollama = SimulatedOllama::start(vec![Generation::Alive {
        first_after: Duration::from_millis(1500),
        every: Duration::from_millis(10),
        chunks: 1,
    }])
    .await;
    let schema = serde_json::json!({"type": "object"});
    let mut running = start_native(
        &ollama.base,
        Some(Duration::from_secs(1)),
        None,
        Some(&schema),
    )
    .await
    .expect("a silent non-streamed request is not a dead one");

    assert_eq!(drain(&mut running).await, "tok0");
    assert!(running.child.wait().await.expect("lifeline").success());
}

// ─── native HTTP: Kronn's own tool execution is not model silence ────────────
//
// Review follow-up on KT-932, point 2 — confirming on the native HTTP path too
// that a LOCAL tool Kronn runs between two model requests (a build, a slow
// read) is not mistaken for the model going silent: nothing reads from the
// stream while the tool executes, so there is no watchdog running to trip.

/// A tool that simply takes a while, standing in for a build or a test run.
struct SlowTool {
    busy: Duration,
}

#[async_trait::async_trait]
impl crate::agents::tools::ToolExecutor for SlowTool {
    fn catalogue(&self) -> Vec<serde_json::Value> {
        vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "slow_tool",
                "description": "Takes a while, like a build or a test run.",
                "parameters": {"type": "object", "properties": {}},
            },
        })]
    }

    async fn execute(
        &self,
        call: &crate::agents::tools::ToolCall,
    ) -> crate::agents::tools::ToolOutcome {
        tokio::time::sleep(self.busy).await;
        crate::agents::tools::ToolOutcome {
            call: call.clone(),
            content: serde_json::json!({"ok": true}),
            ok: true,
        }
    }
}

#[tokio::test]
#[serial]
async fn a_local_tool_running_longer_than_the_idle_delay_is_not_mistaken_for_a_silent_model() {
    let idle = Duration::from_millis(300);
    let ollama = SimulatedOllama::start(vec![
        Generation::ToolCall { tool: "slow_tool" },
        Generation::Quick,
    ])
    .await;
    let executor: std::sync::Arc<dyn crate::agents::tools::ToolExecutor> =
        std::sync::Arc::new(SlowTool { busy: idle * 3 });
    let mut running = start_ollama_http_with_idle(
        &AgentType::Ollama,
        "run the slow tool",
        "",
        "sim-model",
        None,
        Some(&ollama.base),
        None,
        Some(executor),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(idle),
        None,
        None,
    )
    .await
    .expect("the run starts");

    let text = drain(&mut running).await;

    assert_eq!(
        text, "ok",
        "the tool's result fed back, and the model answered"
    );
    assert!(
        running.child.wait().await.expect("lifeline").success(),
        "{}",
        stderr_of(&running)
    );
    assert!(
        !stderr_of(&running).contains("stalled"),
        "{}",
        stderr_of(&running)
    );
}

// ─── ACP: the agent that holds the connection (the 30/09 incident) ───────────

/// An `opencode acp` stand-in, started with `python3`. On a prompt it does one
/// of three things, chosen by `FIXTURE_MODE`:
/// - `hold_ollama` — what OpenCode does with a local model: opens a connection
///   to the model server and waits on it, saying nothing over ACP;
/// - `hold_fd` — the same without a network: it and a helper it starts both hold
///   fd 3, a pipe the test keeps the other end of, and say nothing over ACP;
/// - `slow_alive` — a model that thinks for a long time, reporting each thought,
///   and only then answers.
#[cfg(unix)]
const ACP_AGENT: &str = r#"
import json, os, socket, subprocess, sys, time

def send(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()

def update(kind, text):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": "idle-session",
        "update": {"sessionUpdate": kind, "content": {"type": "text", "text": text}}}})

mode = os.environ["FIXTURE_MODE"]
if mode.startswith("mute_"):
    # Holds the line from the start, like a runtime whose MCP servers are up.
    subprocess.Popen(["sleep", "3600"], close_fds=False)
    os.write(3, b"up")
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize" and mode == "mute_initialize":
        continue
    if method == "session/new" and mode == "mute_session":
        continue
    if method == "session/new" and mode == "mute_error_session":
        send({"jsonrpc": "2.0", "id": message["id"],
              "error": {"code": -32603, "message": "MCP config rejected by the runtime"}})
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": 1,
            "agentCapabilities": {"sessionCapabilities": {}, "promptCapabilities": {},
                                  "sessionCancellation": {}}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"sessionId": "idle-session"}})
    elif method == "session/prompt":
        if mode == "hold_ollama":
            conn = socket.create_connection(("127.0.0.1", int(os.environ["OLLAMA_PORT"])))
            body = b'{"model":"sim-model","stream":true}'
            conn.sendall(b"POST /api/chat HTTP/1.1\r\nHost: sim\r\nContent-Length: %d\r\n\r\n%s"
                         % (len(body), body))
            while conn.recv(4096):
                pass
            time.sleep(3600)
        elif mode == "hold_fd":
            subprocess.Popen(["sleep", "3600"], close_fds=False)
            os.write(3, b"up")
            time.sleep(3600)
        elif mode == "tool_updates":
            # Two overlapping tools, progress only, never a word of text
            # until the answer: busy, not silent.
            for index in range(8):
                time.sleep(0.4)
                for call in ("build", "tests"):
                    send({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": "idle-session",
                        "update": {"sessionUpdate": "tool_call_update", "toolCallId": call,
                                   "title": call, "status": "in_progress"}}})
            for call in ("build", "tests"):
                send({"jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": "idle-session",
                    "update": {"sessionUpdate": "tool_call_update", "toolCallId": call,
                               "status": "completed"}}})
            update("agent_message_chunk", "built and tested")
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"stopReason": "end_turn"}})
        elif mode == "slow_alive":
            for index in range(5):
                time.sleep(0.5)
                update("agent_thought_chunk", "thinking %d" % index)
            update("agent_message_chunk", "the answer")
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"stopReason": "end_turn"}})
"#;

/// A pipe standing in for a connection to the model server: the agent's process
/// group inherits the write end as fd 3, the test keeps the read end. The read
/// end sees end-of-file only once EVERY holder of the write end is gone — what a
/// model server sees when its client hangs up.
#[cfg(unix)]
struct AgentLine {
    read: std::os::fd::RawFd,
    write: std::sync::atomic::AtomicI32,
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum LineState {
    /// Something was written.
    Data,
    /// Open, nothing to read.
    Quiet,
    /// Every holder is gone.
    Closed,
}

#[cfg(unix)]
impl AgentLine {
    fn open() -> Self {
        let mut fds = [0i32; 2];
        // SAFETY: plain pipe/fcntl calls on descriptors this function just made.
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe");
            // Neither end follows a spawn by accident; the agent gets its copy
            // from the explicit `dup2` in `start_acp_agent`.
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
            let flags = libc::fcntl(fds[0], libc::F_GETFL);
            libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        Self {
            read: fds[0],
            write: std::sync::atomic::AtomicI32::new(fds[1]),
        }
    }

    /// The test's own copy of the write end must go once the agent has its own,
    /// or the line could never close.
    fn release_write(&self) {
        let fd = self.write.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            // SAFETY: the descriptor is owned by `self` and closed exactly once.
            unsafe { libc::close(fd) };
        }
    }

    fn state(&self) -> LineState {
        let mut seen = LineState::Quiet;
        let mut buffer = [0u8; 64];
        loop {
            // SAFETY: reads into a local buffer from a descriptor `self` owns.
            let count = unsafe { libc::read(self.read, buffer.as_mut_ptr().cast(), buffer.len()) };
            match count {
                0 => return LineState::Closed,
                n if n > 0 => seen = LineState::Data,
                _ => return seen,
            }
        }
    }

    async fn wait_for(&self, wanted: LineState, within: Duration) -> bool {
        tokio::time::timeout(within, async {
            loop {
                // Data is consumed as it is seen; only the line closing is final.
                let state = self.state();
                if state == wanted || state == LineState::Closed {
                    return state == wanted;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(unix)]
impl Drop for AgentLine {
    fn drop(&mut self) {
        self.release_write();
        // SAFETY: owned by `self`, closed exactly once.
        unsafe { libc::close(self.read) };
    }
}

#[cfg(unix)]
#[derive(Default)]
struct AcpRun<'a> {
    mode: &'a str,
    ollama_port: Option<u16>,
    line: Option<&'a AgentLine>,
    idle: Option<Duration>,
    cancel: Option<&'a tokio_util::sync::CancellationToken>,
    timeouts: Option<crate::acp::AcpRequestTimeouts>,
}

#[cfg(unix)]
async fn start_acp_agent(run: AcpRun<'_>, project: &tempfile::TempDir) -> AgentProcess {
    try_start_acp_agent(run, project)
        .await
        .expect("the ACP run starts")
}

#[cfg(unix)]
async fn try_start_acp_agent(
    run: AcpRun<'_>,
    project: &tempfile::TempDir,
) -> Result<AgentProcess, String> {
    let mut command = tokio::process::Command::new("python3");
    command
        .args(["-c", ACP_AGENT])
        .env("FIXTURE_MODE", run.mode);
    if let Some(port) = run.ollama_port {
        command.env("OLLAMA_PORT", port.to_string());
    }
    if let Some(line) = run.line {
        let write = line.write.load(Ordering::SeqCst);
        // SAFETY: runs between fork and exec, and only calls async-signal-safe
        // functions (`dup2`, `fcntl`).
        unsafe {
            command.pre_exec(move || {
                if write == 3 {
                    // Already where it should be, but still close-on-exec.
                    libc::fcntl(3, libc::F_SETFD, 0);
                } else if libc::dup2(write, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut transport =
        crate::acp::AcpJsonRpcTransport::spawn(crate::acp::AcpAgent::OpenCode, command, false)
            .await
            .expect("the stand-in agent starts");
    if let Some(timeouts) = run.timeouts {
        transport = transport.with_request_timeouts(timeouts);
    }
    let transport: Arc<dyn crate::acp::AcpTransport> = Arc::new(transport);
    if let Some(line) = run.line {
        line.release_write();
    }
    let tokens = crate::models::setup::TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    };
    let _saved = crate::core::config::test_saved_access::set(&AgentType::OpenCode, true);
    start_agent_with_config(AgentStartConfig {
        idle_timeout: run.idle,
        cancel_token: run.cancel.cloned(),
        test_acp_transport: Some(transport),
        full_access: true,
        ..AgentStartConfig::new(
            &AgentType::OpenCode,
            project.path().to_str().unwrap(),
            "think about it",
            &tokens,
        )
    })
    .await
}

/// Startup budgets a test can wait out.
#[cfg(unix)]
fn short_timeouts() -> crate::acp::AcpRequestTimeouts {
    crate::acp::AcpRequestTimeouts {
        control: Duration::from_secs(1),
        session_setup: Duration::from_secs(2),
        prompt: Duration::from_secs(60),
    }
}

/// A runtime that never answers one startup phase: the start fails within
/// that phase's bound, names it, and its whole process group is gone.
#[cfg(unix)]
async fn a_mute_startup_phase_fails_fast(mode: &str, phase: &str, bound: Duration) -> String {
    let line = AgentLine::open();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers": {"Hang": {"command": "sh", "args": ["-c", "cat"]},
            "Ghost": {"command": "kronn-test-no-such-mcp-command"}}}"#,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let error = match try_start_acp_agent(
        AcpRun {
            mode,
            line: Some(&line),
            timeouts: Some(short_timeouts()),
            ..Default::default()
        },
        &project,
    )
    .await
    {
        Ok(_) => panic!("{mode}: a runtime that never answers must not start"),
        Err(error) => error,
    };
    let waited = started.elapsed();
    assert!(
        waited >= bound && waited < bound + Duration::from_secs(5),
        "{mode}: failed on its own bound: {waited:?}"
    );
    let timeout = crate::agents::acp_start::AcpStartFailure::from_error(&error)
        .unwrap_or_else(|| panic!("{mode}: a structured start timeout: {error}"));
    assert_eq!(
        serde_json::to_value(timeout.phase).unwrap(),
        serde_json::json!(phase)
    );
    assert!(
        line.wait_for(LineState::Closed, Duration::from_secs(10))
            .await,
        "{mode}: the runtime and what it started are killed"
    );
    error
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_runtime_that_never_answers_initialize_is_stopped_with_its_phase() {
    let error =
        a_mute_startup_phase_fails_fast("mute_initialize", "initialize", Duration::from_secs(1))
            .await;
    assert!(
        error.contains("OpenCode did not answer initialization within 1 s"),
        "{error}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_runtime_that_never_opens_its_session_names_the_project_servers() {
    let error =
        a_mute_startup_phase_fails_fast("mute_session", "session", Duration::from_secs(2)).await;
    let timeout = crate::agents::acp_start::AcpStartFailure::from_error(&error).unwrap();
    assert_eq!(
        timeout.servers,
        vec!["Hang".to_string()],
        "the project server is named; one whose command is missing was never declared"
    );
    assert!(
        error.contains("OpenCode did not open its session within 2 s"),
        "{error}"
    );
}

/// A run busy with tool updates for longer than the delay, with no text at
/// all until its answer, is not cut, and it tells consumers that its own
/// watchdog owns inactivity, so their text timers stand down.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_run_busy_with_tool_updates_outlives_the_delay() {
    let project = tempfile::tempdir().unwrap();
    let mut running = start_acp_agent(
        AcpRun {
            mode: "tool_updates",
            idle: Some(Duration::from_secs(1)),
            timeouts: Some(short_timeouts()),
            ..Default::default()
        },
        &project,
    )
    .await;
    assert!(running.activity_watched());
    let started = std::time::Instant::now();
    let text = drain(&mut running).await;
    assert!(
        started.elapsed() > Duration::from_secs(3),
        "busy past the 1 s delay"
    );
    assert_eq!(text, "built and tested");
    assert!(running.child.wait().await.expect("lifeline").success());
}

/// A runtime that answers `session/new` with an error is settled with that
/// error and its phase at once, never left to a silent retry.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_session_refused_by_the_runtime_is_a_settled_start_failure() {
    let line = AgentLine::open();
    let project = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let Err(error) = try_start_acp_agent(
        AcpRun {
            mode: "mute_error_session",
            line: Some(&line),
            timeouts: Some(short_timeouts()),
            ..Default::default()
        },
        &project,
    )
    .await
    else {
        panic!("a refused session must not start");
    };
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "no wait on a bound"
    );
    let failure = crate::agents::acp_start::AcpStartFailure::from_error(&error)
        .unwrap_or_else(|| panic!("a structured start failure: {error}"));
    assert_eq!(
        failure.phase,
        crate::agents::acp_start::AcpStartPhase::Session
    );
    assert!(
        failure
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("MCP config rejected by the runtime")),
        "{failure:?}"
    );
    assert!(
        line.wait_for(LineState::Closed, Duration::from_secs(10))
            .await
    );
}

/// A stop during startup ends the start at once and kills the runtime.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn stopping_during_startup_ends_the_start_and_kills_the_runtime() {
    let line = AgentLine::open();
    let project = tempfile::tempdir().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    let Err(error) = try_start_acp_agent(
        AcpRun {
            mode: "mute_session",
            line: Some(&line),
            cancel: Some(&cancel),
            timeouts: Some(crate::acp::AcpRequestTimeouts {
                session_setup: Duration::from_secs(60),
                ..short_timeouts()
            }),
            ..Default::default()
        },
        &project,
    )
    .await
    else {
        panic!("a stopped start must not start");
    };
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(error.contains("ACP start cancelled"), "{error}");
    assert!(
        line.wait_for(LineState::Closed, Duration::from_secs(10))
            .await
    );
}

/// An adapted turn that only reasons — thinking deltas, reasoning items —
/// for longer than the delay is alive, not silent: it is not cut.
#[cfg(unix)]
async fn a_reasoning_only_adapted_turn_outlives_the_delay(
    agent: &AgentType,
    transport: Arc<dyn crate::acp::AcpTransport>,
    project: &tempfile::TempDir,
    answer: &str,
) {
    let started = std::time::Instant::now();
    let mut running = run_acp_session(
        AcpSessionRequest {
            step_tools: None,
            agent_type: agent,
            work_dir: project.path(),
            prompt: "think",
            system_context: "",
            project_path: "",
            model_flag: None,
            reasoning_effort: None,
            parent_cancel: None,
            discussion_id: None,
            resume_id: None,
            session_store: None,
            fallback_prompt: None,
            provenance: None,
            activity: None,
            idle_timeout: Some(Duration::from_secs(1)),
        },
        transport,
    )
    .await
    .expect("the adapted turn starts");
    let text = drain(&mut running).await;
    assert!(
        started.elapsed() > Duration::from_secs(3),
        "{agent:?}: reasoned past the 1 s delay"
    );
    assert_eq!(text, answer, "{agent:?}: {}", stderr_of(&running));
    assert!(
        running.child.wait().await.expect("lifeline").success(),
        "{agent:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn a_claude_turn_that_only_thinks_is_not_cut() {
    let project = tempfile::tempdir().unwrap();
    let fixture = crate::acp::test_support::write_fixture_script(
        project.path(),
        r#"
cat >/dev/null
for i in 1 2 3 4 5 6 7 8; do
  sleep 0.4
  printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}'
done
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"thought it through"}}}'
printf '%s\n' '{"type":"result","subtype":"success","usage":{"input_tokens":1,"output_tokens":2}}'
"#,
    );
    let adapter = crate::acp::ClaudeAcpAdapter::new(
        None,
        None,
        false,
        None,
        crate::acp::AcpSessionScope::new(Some(project.path().to_path_buf()), "reasoning"),
    )
    .with_program(&fixture);
    a_reasoning_only_adapted_turn_outlives_the_delay(
        &AgentType::ClaudeCode,
        Arc::new(adapter),
        &project,
        "thought it through",
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn a_codex_turn_that_only_reasons_is_not_cut() {
    let project = tempfile::tempdir().unwrap();
    let fixture = crate::acp::test_support::write_fixture_script(
        project.path(),
        r#"
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"th-reasoning"}'
for i in 1 2 3 4 5 6 7 8; do
  sleep 0.4
  printf '%s\n' "{\"type\":\"item.updated\",\"item\":{\"id\":\"r1\",\"type\":\"reasoning\",\"text\":\"step $i\"}}"
done
printf '%s\n' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"reasoned it out"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":2}}'
"#,
    );
    let adapter = crate::acp::CodexAcpAdapter::new(
        None,
        None,
        false,
        None,
        None,
        crate::acp::AcpSessionScope::new(Some(project.path().to_path_buf()), "reasoning"),
    )
    .with_program(&fixture);
    a_reasoning_only_adapted_turn_outlives_the_delay(
        &AgentType::Codex,
        Arc::new(adapter),
        &project,
        "reasoned it out",
    )
    .await;
}

/// The prompt half: a session that opens, then never says a word. The run
/// fails on the configured delay with the no-first-token reason the discussion
/// turns into its own message, and the process group is gone.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_prompt_that_never_answers_fails_on_the_delay_and_is_killed() {
    let line = AgentLine::open();
    let project = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let mut running = start_acp_agent(
        AcpRun {
            mode: "mute_prompt",
            line: Some(&line),
            idle: Some(Duration::from_secs(1)),
            timeouts: Some(short_timeouts()),
            ..Default::default()
        },
        &project,
    )
    .await;
    assert!(drain(&mut running).await.is_empty());
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(!running.child.wait().await.expect("lifeline").success());
    let reason = running.captured_stderr_flushed().await.join("\n");
    assert!(reason.contains(idle_watchdog::NO_FIRST_TOKEN), "{reason}");
    assert!(
        line.wait_for(LineState::Closed, Duration::from_secs(10))
            .await,
        "the silent runtime is killed with its process group"
    );
}

/// DoD 3 for ACP, against the simulated Ollama — the agent is stopped with its
/// whole process group, so its connection to the model server closes and the
/// model is free again.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn stopping_an_acp_agent_frees_the_model_it_was_talking_to() {
    for stop in [Stop::Kill, Stop::Cancel, Stop::Drop] {
        let ollama = SimulatedOllama::start(vec![Generation::Endless, Generation::Quick]).await;
        let project = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut running = start_acp_agent(
            AcpRun {
                mode: "hold_ollama",
                ollama_port: Some(ollama.port),
                cancel: Some(&cancel),
                ..Default::default()
            },
            &project,
        )
        .await;
        tokio::time::timeout(Duration::from_secs(10), ollama.started.notified())
            .await
            .expect("the agent is talking to the model");

        match stop {
            Stop::Kill => running.kill().await,
            Stop::Cancel => cancel.cancel(),
            Stop::Drop => drop(running),
        }

        ollama.wait_abandoned(1, Duration::from_secs(10)).await;
        ollama.assert_next_request_is_served().await;
    }
}

/// DoD 1 for ACP, against the simulated Ollama — the incident itself. The agent
/// holds an open, mute connection to the model server; nothing on either side
/// would ever notice.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_agent_stuck_on_a_mute_model_fails_the_run_and_frees_the_model() {
    let ollama = SimulatedOllama::start(vec![Generation::Mute, Generation::Quick]).await;
    let project = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let mut running = start_acp_agent(
        AcpRun {
            mode: "hold_ollama",
            ollama_port: Some(ollama.port),
            idle: Some(Duration::from_secs(1)),
            ..Default::default()
        },
        &project,
    )
    .await;

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert!(text.is_empty(), "the agent never said anything: {text:?}");
    assert!(
        silence >= Duration::from_secs(1) && silence < Duration::from_secs(15),
        "ended on the silence, without anyone killing it by hand: {silence:?}"
    );
    let status = running.child.wait().await.expect("lifeline");
    assert!(!status.success(), "a stalled agent is a failed run");
    let reason = running.captured_stderr_flushed().await.join("\n");
    assert!(
        reason.contains("Agent stalled (no output for 1s)"),
        "{reason}"
    );
    assert!(reason.contains("OpenCode sent no data for 1 s"), "{reason}");

    ollama.wait_abandoned(1, Duration::from_secs(10)).await;
    ollama.assert_next_request_is_served().await;
}

/// DoD 3 for ACP, without a network — the same stop, observed on a pipe that the
/// agent AND the helper it started both hold. The line closes only when the
/// whole process group is gone, which is what lets a model server notice.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn stopping_an_acp_agent_closes_every_connection_its_process_group_held() {
    for stop in [Stop::Kill, Stop::Cancel, Stop::Drop] {
        let line = AgentLine::open();
        let project = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut running = start_acp_agent(
            AcpRun {
                mode: "hold_fd",
                line: Some(&line),
                cancel: Some(&cancel),
                ..Default::default()
            },
            &project,
        )
        .await;
        assert!(
            line.wait_for(LineState::Data, Duration::from_secs(10))
                .await,
            "{stop:?}: the agent is up and holds the line"
        );
        assert_eq!(line.state(), LineState::Quiet, "{stop:?}: still connected");

        match stop {
            Stop::Kill => running.kill().await,
            Stop::Cancel => cancel.cancel(),
            Stop::Drop => drop(running),
        }

        assert!(
            line.wait_for(LineState::Closed, Duration::from_secs(10))
                .await,
            "{stop:?}: the agent's connection is still open after the stop"
        );
    }
}

/// DoD 1 for ACP, without a network — the agent goes quiet while holding the
/// line. The run fails by itself, with its reason, and the line is released.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_acp_agent_that_goes_quiet_fails_the_run_and_releases_its_connection() {
    let line = AgentLine::open();
    let project = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let mut running = start_acp_agent(
        AcpRun {
            mode: "hold_fd",
            line: Some(&line),
            idle: Some(Duration::from_secs(1)),
            ..Default::default()
        },
        &project,
    )
    .await;

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert!(text.is_empty(), "{text:?}");
    assert!(
        silence >= Duration::from_secs(1) && silence < Duration::from_secs(15),
        "ended on the silence, nobody killed it by hand: {silence:?}"
    );
    assert!(!running.child.wait().await.expect("lifeline").success());
    let reason = running.captured_stderr_flushed().await.join("\n");
    assert!(
        reason.contains("Agent stalled (no output for 1s)"),
        "{reason}"
    );
    assert!(
        reason.contains("OpenCode sent no data for 1 s without ever sending a first token"),
        "{reason}"
    );
    assert!(
        crate::workflows::steps::is_stall_error(&reason),
        "a step's on_timeout routing recognises it: {reason}"
    );
    assert!(
        line.wait_for(LineState::Closed, Duration::from_secs(10))
            .await,
        "the stalled agent's connection must be released"
    );
}

/// DoD 2 for ACP — five thoughts, half a second apart, then the answer. The
/// answer is 2.5 s from the start and the only TEXT is at the very end: what
/// keeps the run alive against a 1 s delay is the thought frames, which carry
/// nothing to show and everything to prove.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn a_slow_but_living_acp_agent_is_not_cut() {
    let project = tempfile::tempdir().unwrap();
    let mut running = start_acp_agent(
        AcpRun {
            mode: "slow_alive",
            idle: Some(Duration::from_secs(1)),
            ..Default::default()
        },
        &project,
    )
    .await;

    let text = drain(&mut running).await;

    assert_eq!(text, "the answer");
    let status = running.child.wait().await.expect("lifeline");
    let diagnostics = running.captured_stderr_flushed().await.join("\n");
    assert!(status.success(), "{diagnostics}");
    assert!(!diagnostics.contains("stalled"), "{diagnostics}");
}

// ─── ACP: the host's own behaviour, on a scripted transport ──────────────────

#[derive(Clone, Copy)]
enum Turn {
    /// `proofs` signs of life 100 ms apart, then nothing for ever.
    GoesSilent { proofs: usize },
    /// `beats` signs of life `gap` apart, then the answer and a normal end.
    Alive { beats: usize, gap: Duration },
    /// A tool call starts, stays silent for `busy` (no ACP frame at all, as a
    /// real tool run produces none), reaches a terminal update, then the turn
    /// answers normally. KT-932 follow-up — `busy` outlives the model's own
    /// delay but not the tool's own (much wider) bound.
    ToolThenAnswer { name: &'static str, busy: Duration },
    /// A tool call starts and never reaches a terminal update: only the
    /// tool's own bound can end the turn, and the reason must name it.
    ToolNeverEnds { name: &'static str },
    /// A tool call starts, ends quickly, and then the MODEL itself goes
    /// silent for ever: the model's own (narrower) delay must be what ends
    /// the turn this time, not the tool bound.
    ToolEndsThenModelGoesSilent { name: &'static str },
}

struct ScriptedAcp {
    turn: Turn,
    cancelled: AtomicUsize,
    shut_down: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for ScriptedAcp {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
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

    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::OpenCode, "scripted-session")
    }

    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }

    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }

    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }

    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        events: mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        use crate::acp::AcpSessionEvent;
        match self.turn {
            Turn::GoesSilent { proofs } => {
                for _ in 0..proofs {
                    events.send(AcpSessionEvent::Activity).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                std::future::pending().await
            }
            Turn::Alive { beats, gap } => {
                for _ in 0..beats {
                    tokio::time::sleep(gap).await;
                    events.send(AcpSessionEvent::Activity).await.unwrap();
                }
                events
                    .send(AcpSessionEvent::TextDelta("the answer".into()))
                    .await
                    .unwrap();
                events.send(AcpSessionEvent::Completed).await.unwrap();
                Ok(())
            }
            Turn::ToolThenAnswer { name, busy } => {
                events
                    .send(AcpSessionEvent::ToolCall {
                        id: None,
                        name: name.into(),
                    })
                    .await
                    .unwrap();
                tokio::time::sleep(busy).await;
                events
                    .send(AcpSessionEvent::ToolCallEnded { id: None })
                    .await
                    .unwrap();
                events
                    .send(AcpSessionEvent::TextDelta("the answer".into()))
                    .await
                    .unwrap();
                events.send(AcpSessionEvent::Completed).await.unwrap();
                Ok(())
            }
            Turn::ToolNeverEnds { name } => {
                events
                    .send(AcpSessionEvent::ToolCall {
                        id: None,
                        name: name.into(),
                    })
                    .await
                    .unwrap();
                std::future::pending().await
            }
            Turn::ToolEndsThenModelGoesSilent { name } => {
                events
                    .send(AcpSessionEvent::ToolCall {
                        id: None,
                        name: name.into(),
                    })
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                events
                    .send(AcpSessionEvent::ToolCallEnded { id: None })
                    .await
                    .unwrap();
                std::future::pending().await
            }
        }
    }

    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        self.shut_down.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn start_scripted(
    turn: Turn,
    idle: Option<Duration>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> (AgentProcess, Arc<ScriptedAcp>) {
    let agent = Arc::new(ScriptedAcp {
        turn,
        cancelled: AtomicUsize::new(0),
        shut_down: AtomicUsize::new(0),
    });
    let project = tempfile::tempdir().unwrap();
    let tokens = crate::models::setup::TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    };
    let _saved = crate::core::config::test_saved_access::set(&AgentType::OpenCode, true);
    let running = start_agent_with_config(AgentStartConfig {
        idle_timeout: idle,
        cancel_token: cancel.cloned(),
        test_acp_transport: Some(agent.clone()),
        full_access: true,
        ..AgentStartConfig::new(
            &AgentType::OpenCode,
            project.path().to_str().unwrap(),
            "think about it",
            &tokens,
        )
    })
    .await
    .expect("the ACP run starts");
    (running, agent)
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never happened: {what}"));
}

/// DoD 1 — a turn that stops producing anything is cancelled on the agent and
/// ends the run with the reason, nobody having to kill anything by hand.
#[tokio::test]
async fn a_silent_acp_turn_is_cancelled_and_fails_with_its_reason() {
    let started = std::time::Instant::now();
    let (mut running, agent) = start_scripted(
        Turn::GoesSilent { proofs: 2 },
        Some(Duration::from_secs(1)),
        None,
    )
    .await;

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert!(text.is_empty());
    assert!(
        silence >= Duration::from_millis(900) && silence < Duration::from_secs(10),
        "{silence:?}"
    );
    let status = running.child.wait().await.expect("lifeline");
    assert!(!status.success(), "a stalled turn is a failed run");
    let reason = running.captured_stderr_flushed().await.join("\n");
    assert!(
        reason.contains("Agent stalled (no output for 1s)"),
        "{reason}"
    );
    assert!(
        reason.contains("OpenCode sent no data for 1 s after 2 event(s)"),
        "{reason}"
    );
    assert_eq!(
        agent.cancelled.load(Ordering::SeqCst),
        1,
        "the session was cancelled"
    );
    assert_eq!(
        agent.shut_down.load(Ordering::SeqCst),
        1,
        "and the agent shut down, which is what closes its connection to the model"
    );
}

/// DoD 2 — six signs of life 400 ms apart: 2.4 s against a 1 s delay. A budget on
/// the whole turn would have cut it; each sign restarts the silence instead.
#[tokio::test]
async fn a_living_acp_turn_is_not_cut_however_long_it_takes() {
    let (mut running, agent) = start_scripted(
        Turn::Alive {
            beats: 6,
            gap: Duration::from_millis(400),
        },
        Some(Duration::from_secs(1)),
        None,
    )
    .await;

    assert_eq!(drain(&mut running).await, "the answer");
    assert!(running.child.wait().await.expect("lifeline").success());
    assert!(!running
        .captured_stderr_flushed()
        .await
        .join("\n")
        .contains("stalled"));
    assert_eq!(agent.cancelled.load(Ordering::SeqCst), 0);
}

/// DoD 3 — a stop of any kind reaches the agent as a session cancel followed by
/// its shutdown, so the generation does not outlive the run.
#[tokio::test]
async fn stopping_an_acp_turn_cancels_the_session_and_shuts_the_agent_down() {
    for stop in [Stop::Kill, Stop::Cancel, Stop::Drop] {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (mut running, agent) =
            start_scripted(Turn::GoesSilent { proofs: 1 }, None, Some(&cancel)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        match stop {
            Stop::Kill => running.kill().await,
            Stop::Cancel => cancel.cancel(),
            Stop::Drop => drop(running),
        }

        wait_until(&format!("{stop:?}: the agent shut down"), || {
            agent.shut_down.load(Ordering::SeqCst) == 1
        })
        .await;
        assert_eq!(
            agent.cancelled.load(Ordering::SeqCst),
            1,
            "{stop:?}: the session was cancelled before the shutdown"
        );
    }
}

// ─── ACP: a long tool call is not mistaken for a dead model ──────────────────
//
// Review follow-up on KT-932 — OpenCode running `cargo test` or a 20-minute
// build emits no ACP frame for the whole run. Judging that silence by the
// model's own (much shorter) inactivity delay cut a healthy run. While a tool
// call is open, the watchdog now measures silence against the tool's own,
// much wider bound instead, and hands the clock back to the model the moment
// a terminal update closes it.

/// (a) — a tool call in progress longer than the model's own delay is not cut.
#[tokio::test(start_paused = true)]
async fn a_long_silent_acp_tool_call_is_not_mistaken_for_a_dead_model() {
    let model_idle = Duration::from_secs(60);
    let (mut running, agent) = start_scripted(
        Turn::ToolThenAnswer {
            name: "cargo test",
            // 5 minutes: longer than the 1-minute model delay, well inside
            // the 8-minute tool bound it scales to.
            busy: Duration::from_secs(5 * 60),
        },
        Some(model_idle),
        None,
    )
    .await;

    assert_eq!(drain(&mut running).await, "the answer");
    assert!(
        running.child.wait().await.expect("lifeline").success(),
        "{}",
        running.captured_stderr_flushed().await.join("\n")
    );
    assert!(!running
        .captured_stderr_flushed()
        .await
        .join("\n")
        .contains("stalled"));
    assert_eq!(agent.cancelled.load(Ordering::SeqCst), 0);
}

/// (b) — once the tool call has ended, the model's own delay applies again: a
/// model that then stays silent is still cut, with its usual (model-silence)
/// reason, not the tool bound and not naming the finished tool.
#[tokio::test(start_paused = true)]
async fn the_model_delay_resumes_once_the_tool_call_has_ended() {
    let started = tokio::time::Instant::now();
    let (mut running, agent) = start_scripted(
        Turn::ToolEndsThenModelGoesSilent { name: "cargo test" },
        Some(Duration::from_secs(60)),
        None,
    )
    .await;

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert!(text.is_empty());
    // Cut on the MODEL's 60s delay, not the much wider (8 min) tool bound the
    // 50ms-long tool call never came close to needing.
    assert_eq!(silence, Duration::from_secs(60) + Duration::from_millis(50));
    let status = running.child.wait().await.expect("lifeline");
    assert!(!status.success(), "a model gone silent is a failed run");
    let captured = running.captured_stderr_flushed().await.join("\n");
    // The `[acp-tool]cargo test` marker line is the tool call itself being
    // reported for display — it legitimately names the tool it ran, even
    // though that tool finished cleanly. What must NOT name it is the stall
    // reason line.
    let reason = captured
        .lines()
        .find(|line| crate::agents::idle_watchdog::is_stall_reason(line))
        .unwrap_or_else(|| panic!("no stall reason in: {captured}"));
    assert!(
        reason.contains("Agent stalled (no output for 60s)"),
        "{reason}"
    );
    assert!(
        reason.contains("OpenCode sent no data for 1 min"),
        "{reason}"
    );
    assert!(
        !reason.contains("cargo test"),
        "the tool had already finished, the model's own stall must not blame it: {reason}"
    );
    assert!(
        crate::workflows::steps::is_stall_error(reason),
        "a step's on_timeout routing still recognises it: {reason}"
    );
    assert_eq!(agent.cancelled.load(Ordering::SeqCst), 1);
}

/// (c) — a tool call that never reaches a terminal update is still bounded,
/// just by its own, much wider limit, and the failure names the tool.
#[tokio::test(start_paused = true)]
async fn a_tool_call_that_never_ends_is_cut_by_its_own_bound_and_names_it() {
    let started = tokio::time::Instant::now();
    let model_idle = Duration::from_secs(60);
    let tool_bound = crate::agents::idle_watchdog::tool_execution_timeout(model_idle);
    assert_eq!(
        tool_bound,
        Duration::from_secs(8 * 60),
        "sanity: 8x the model delay"
    );
    let (mut running, agent) = start_scripted(
        Turn::ToolNeverEnds { name: "cargo test" },
        Some(model_idle),
        None,
    )
    .await;

    let text = drain(&mut running).await;
    let silence = started.elapsed();

    assert!(text.is_empty());
    assert_eq!(
        silence, tool_bound,
        "the model's 60s delay never applied while the tool was open"
    );
    let status = running.child.wait().await.expect("lifeline");
    assert!(!status.success(), "a hung tool call is a failed run");
    let reason = running.captured_stderr_flushed().await.join("\n");
    assert!(
        reason.contains("Agent stalled (no output for 480s)"),
        "{reason}"
    );
    assert!(
        reason.contains(r#"OpenCode's "cargo test" tool call"#),
        "the reason names the tool, not the model: {reason}"
    );
    assert!(
        crate::workflows::steps::is_stall_error(&reason),
        "a step's on_timeout routing still recognises it: {reason}"
    );
    assert_eq!(agent.cancelled.load(Ordering::SeqCst), 1);
}
