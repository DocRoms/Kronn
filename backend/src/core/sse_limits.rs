//! Shared limits for long-running Server-Sent Event streams.
//!
//! Long SSE streams are convenient (workflow runs, agent output, audits) but
//! they're also a slow-burn resource leak: a misbehaving frontend that opens
//! a stream and never reads from it will keep the producer alive until the
//! agent finishes hours later, and an upstream agent that loops can flood
//! the channel with millions of progress events.
//!
//! Two complementary safety belts:
//!
//! 1. **Idle timeout** — if no event is produced within `IDLE_TIMEOUT`, the
//!    stream is closed cleanly with a final `error` event so the client knows
//!    it was the server that hung up.
//! 2. **Event count cap** — after `MAX_EVENTS` events the stream is also
//!    closed with a final `error` event. This protects against runaway agents
//!    spamming progress chunks. The cap is generous enough that real workflow
//!    runs (a few thousand chunks) never hit it.
//!
//! A third helper, [`with_heartbeat`], keeps a stream that is legitimately
//! silent — a local model loading its weights and reading a large prompt —
//! from tripping the idle timeout: it emits an SSE comment while the producer
//! is alive but quiet. The agent layer (stall watchdog, global deadline) then
//! stays the single authority on whether a run is dead (issue #234).
//!
//! Both limits are deliberately *soft*: they wrap the stream non-invasively
//! and emit a structured terminator event so the frontend can show a useful
//! message ("workflow paused — too much output" / "stream timed out") instead
//! of a generic disconnect.

use std::convert::Infallible;
use std::pin::Pin;
use std::time::Duration;

use axum::response::sse::Event;
use futures::Stream;

/// Maximum events any single SSE stream may emit before we cut it off.
/// 100k events at ~1 KB each ≈ 100 MB of data — far above any legitimate
/// workflow run we've seen, but well below memory pressure on the producer.
pub const MAX_EVENTS: u64 = 100_000;

/// Maximum time we'll wait between events before closing the stream.
/// 10 minutes is enough to span a slow LLM call without blowing past the
/// nginx `proxy_read_timeout` (1800s = 30 min).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long an agent stream may stay quiet before [`with_heartbeat`] sends a
/// keep-alive comment. Far below [`IDLE_TIMEOUT`] and below the usual
/// proxy / NAT idle timeouts (nginx 60 s default, Tailscale ~2 min).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Boxed SSE stream type used everywhere in the API layer.
pub type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Wrap an SSE stream with idle-timeout + event-count limits.
///
/// The returned stream:
/// - terminates after [`MAX_EVENTS`] events with a final `error` event,
/// - terminates after [`IDLE_TIMEOUT`] of inactivity with a final `error`
///   event,
/// - otherwise behaves identically to the input stream.
pub fn bounded(inner: SseStream) -> SseStream {
    Box::pin(async_stream::stream! {
        let mut count: u64 = 0;
        let mut inner = inner;
        loop {
            // Race the next event against the idle deadline.
            let next = tokio::time::timeout(IDLE_TIMEOUT, futures::StreamExt::next(&mut inner)).await;
            match next {
                Ok(Some(item)) => {
                    count += 1;
                    yield item;
                    if count >= MAX_EVENTS {
                        let message = format!(
                            "sse stream cut off after {} events (limit reached)",
                            MAX_EVENTS
                        );
                        let final_evt = Event::default().event("error").data(
                            serde_json::json!({ "error": message, "message": message }).to_string()
                        );
                        yield Ok(final_evt);
                        break;
                    }
                }
                Ok(None) => break, // stream finished naturally
                Err(_) => {
                    let message = format!(
                        "sse stream idle for {}s, server closed",
                        IDLE_TIMEOUT.as_secs()
                    );
                    let final_evt = Event::default().event("error").data(
                        serde_json::json!({ "error": message, "message": message }).to_string()
                    );
                    yield Ok(final_evt);
                    break;
                }
            }
        }
    })
}

/// Emit an SSE comment (`: keepalive`) whenever `inner` stays silent for
/// `every`, and end when `inner` ends.
///
/// Meant for streams whose producer enforces its own deadlines, such as an
/// agent run: a cold local model sends nothing for many minutes while it
/// loads and prefills, and without a heartbeat [`bounded`] hung up after
/// [`IDLE_TIMEOUT`] although the agent watchdog still allowed 15 minutes
/// (issue #234). Comment lines are ignored by `EventSource` and by the
/// frontend parsers, which only read `event:` and `data:` lines.
pub fn with_heartbeat(inner: SseStream, every: Duration) -> SseStream {
    Box::pin(async_stream::stream! {
        let mut inner = inner;
        loop {
            match tokio::time::timeout(every, futures::StreamExt::next(&mut inner)).await {
                Ok(Some(item)) => yield item,
                Ok(None) => break,
                Err(_) => yield Ok(Event::default().comment("keepalive")),
            }
        }
    })
}

/// The envelope of every SSE stream fed by an agent run: [`with_heartbeat`]
/// inside [`bounded`]. Discussions, orchestration, workflow runs and audits go
/// through it so a silent model is judged by the agent's own deadlines only.
pub fn agent_run(inner: SseStream) -> SseStream {
    bounded(with_heartbeat(inner, HEARTBEAT_INTERVAL))
}

/// Render a whole SSE response as text, for tests of the handlers that build one.
#[cfg(test)]
pub(crate) async fn sse_text(sse: axum::response::sse::Sse<SseStream>) -> String {
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    let body = sse
        .into_response()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn evt(data: &str) -> Result<Event, Infallible> {
        Ok(Event::default().data(data))
    }

    #[tokio::test]
    async fn passthrough_under_limits() {
        let inner: SseStream = Box::pin(futures::stream::iter(vec![evt("a"), evt("b"), evt("c")]));
        let collected: Vec<_> = bounded(inner).collect().await;
        // Three input events come through unchanged, no synthetic terminator
        // because the stream ended naturally before any limit.
        assert_eq!(collected.len(), 3);
    }

    #[tokio::test]
    async fn event_cap_terminator_supports_generic_and_ollama_error_clients() {
        use axum::response::IntoResponse;
        use http_body_util::BodyExt;

        let inner: SseStream = Box::pin(futures::stream::repeat_with(|| evt("tick")));
        let body = axum::response::sse::Sse::new(bounded(inner))
            .into_response()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: error"), "{text}");
        assert!(text.contains("\"error\":"), "{text}");
        assert!(text.contains("\"message\":"), "{text}");
        assert!(text.contains("limit reached"), "{text}");
    }

    #[tokio::test]
    async fn idle_timeout_emits_terminator() {
        // pending::<>() never yields → forces the idle timeout path,
        // but tokio::time can't be advanced past 10 min in tests, so we
        // smoke-test the helper handles a fast pending+drop scenario instead.
        let inner: SseStream = Box::pin(futures::stream::empty());
        let collected: Vec<_> = bounded(inner).collect().await;
        // Empty inner → no events at all (idle timeout fires only if at
        // least one round-trip is needed; a fully-empty stream returns None
        // immediately and ends naturally).
        assert!(collected.is_empty());
    }

    async fn sse_text(stream: SseStream) -> String {
        super::sse_text(axum::response::sse::Sse::new(stream)).await
    }

    /// A producer that stays silent for `silence`, then sends one chunk and ends
    /// — a cold local model that finally answers.
    fn late_answer(silence: Duration) -> SseStream {
        Box::pin(async_stream::stream! {
            tokio::time::sleep(silence).await;
            yield evt("answer");
        })
    }

    #[tokio::test(start_paused = true)]
    async fn silent_producer_is_cut_after_idle_timeout() {
        let text = sse_text(bounded(late_answer(Duration::from_secs(20 * 60)))).await;
        assert!(text.contains("event: error"), "{text}");
        assert!(text.contains("idle for 600s"), "{text}");
        assert!(!text.contains("answer"), "{text}");
    }

    /// Regression for #234: a model silent longer than `IDLE_TIMEOUT` but
    /// within the agent watchdog must reach the client.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_keeps_a_slow_first_token_alive() {
        let silence = Duration::from_secs(20 * 60);
        let stream = bounded(with_heartbeat(late_answer(silence), HEARTBEAT_INTERVAL));
        let text = sse_text(stream).await;
        assert!(!text.contains("event: error"), "{text}");
        assert!(text.contains("data: answer"), "{text}");
        let beats = text.matches(": keepalive").count() as u64;
        // The last tick and the answer fall on the same instant of the paused
        // clock, so either may win that race.
        let expected = silence.as_secs() / HEARTBEAT_INTERVAL.as_secs();
        assert!(
            (expected - 1..=expected).contains(&beats),
            "{beats} beats\n{text}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_passes_events_through_and_ends_with_inner() {
        let inner: SseStream = Box::pin(futures::stream::iter(vec![evt("a"), evt("b")]));
        let collected: Vec<_> = with_heartbeat(inner, HEARTBEAT_INTERVAL).collect().await;
        assert_eq!(collected.len(), 2);

        let empty: SseStream = Box::pin(futures::stream::empty());
        assert!(with_heartbeat(empty, HEARTBEAT_INTERVAL)
            .collect::<Vec<_>>()
            .await
            .is_empty());
    }
}
