//! KT-932 — inactivity watchdog for a model stream.
//!
//! A local model can stay silent for a long time and still be alive: loading
//! tens of gigabytes of weights, then reading a long prompt, both happen before
//! the first token. What it cannot be is silent *for ever*: the machine slept,
//! Ollama's runner went away, a NAT dropped the flow — the TCP connection stays
//! open and mute, and neither end ever notices. A wall-clock budget for the whole
//! run cannot tell those two apart, so the signal here is *progress*: every byte,
//! ACP frame or token that arrives restarts the clock, and only a silence of the
//! full delay ends the run.
//!
//! Two shapes, one policy:
//! - the native HTTP stream reads bytes one at a time, so [`next_within`] bounds
//!   each read — no shared state needed;
//! - an ACP turn is one long `session/prompt` future fed by another task, so
//!   [`IdleWatchdog`] carries the progress across: the producer calls
//!   [`IdleWatchdog::beat`], the owner awaits [`IdleWatchdog::expired`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::time::Instant;

/// How long a model may stay silent before the run is declared dead.
///
/// It has to let the FIRST token of a large local model through, cold: weights
/// read from disk, then the whole prompt processed, with nothing on the wire
/// until both are done. Fifteen minutes is also the floor the streaming layer
/// already grants any agent that writes its answer at the end
/// (`NON_STREAMING_STALL_TIMEOUT`), so the two agree on what "silent but maybe
/// alive" means. Callers that carry an operator's setting pass that instead:
/// the discussion's "Agent inactivity timeout" and a workflow step's
/// `stall_timeout_secs` both become the delay of this watchdog.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Leads every explicit stall reason. It is the same wording the workflow step
/// watchdog has always used, so `is_stall_error` and a step's `on_timeout`
/// routing keep recognising a stalled run whichever watchdog fired first.
const STALL_PREFIX: &str = "Agent stalled (no output for";

struct Shared {
    origin: Instant,
    /// Microseconds since `origin` of the latest progress.
    last_beat_us: AtomicU64,
    beats: AtomicU64,
}

/// Progress clock shared between whoever sees the model's output and whoever
/// owns the run. Cloning shares the clock.
#[derive(Clone)]
pub struct IdleWatchdog {
    shared: Arc<Shared>,
    limit: Duration,
}

impl IdleWatchdog {
    /// Starts counting now: a model that never produces anything is as dead as
    /// one that stops halfway.
    pub fn new(limit: Duration) -> Self {
        Self {
            shared: Arc::new(Shared {
                origin: Instant::now(),
                last_beat_us: AtomicU64::new(0),
                beats: AtomicU64::new(0),
            }),
            limit,
        }
    }

    pub fn limit(&self) -> Duration {
        self.limit
    }

    /// Something arrived. Restarts the silence.
    pub fn beat(&self) {
        let elapsed = self.shared.origin.elapsed().as_micros();
        self.shared.last_beat_us.store(
            u64::try_from(elapsed).unwrap_or(u64::MAX),
            Ordering::Release,
        );
        self.shared.beats.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times progress was reported — what a stall message says the
    /// model had done before it went quiet.
    pub fn beats(&self) -> u64 {
        self.shared.beats.load(Ordering::Relaxed)
    }

    /// Resolves once nothing has been reported for the full limit. Never
    /// resolves while progress keeps coming.
    ///
    /// The deadline is recomputed from the latest beat after every sleep, so a
    /// beat just before the deadline buys a whole new delay rather than being
    /// rounded up to two.
    pub async fn expired(&self) {
        loop {
            let last = self.shared.last_beat_us.load(Ordering::Acquire);
            let deadline = self.shared.origin + Duration::from_micros(last) + self.limit;
            tokio::time::sleep_until(deadline).await;
            if self.shared.last_beat_us.load(Ordering::Acquire) == last {
                return;
            }
        }
    }
}

/// The stream stayed silent for the whole limit.
#[derive(Debug, PartialEq, Eq)]
pub struct Silent;

/// The next item of `stream`, or [`Silent`] once `limit` passes without one.
/// `None` for the limit waits as long as it takes: a response that is not
/// streamed arrives in one piece when the generation is over, so silence while
/// it runs says nothing about whether the model is alive.
pub async fn next_within<S>(
    stream: &mut S,
    limit: Option<Duration>,
) -> Result<Option<S::Item>, Silent>
where
    S: Stream + Unpin,
{
    match limit {
        Some(limit) => tokio::time::timeout(limit, stream.next())
            .await
            .map_err(|_| Silent),
        None => Ok(stream.next().await),
    }
}

/// `15 min`, `90 s` — whichever reads naturally for the delay.
fn spoken(limit: Duration) -> String {
    let secs = limit.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// The failure a silent stream ends with. Says what stopped, for how long, how
/// far the model had got, and what Kronn did about it — the operator reading a
/// red step must not have to guess whether this was a crash or a decision.
///
/// `progress` completes "…before it went quiet": `"after 42 chunks"`, or
/// `"without ever sending a first token"`.
pub fn stall_reason(what: &str, limit: Duration, progress: &str) -> String {
    format!(
        "{STALL_PREFIX} {}s): {what} sent no data for {} {progress}. Kronn cancelled the \
         generation so the model is free for the next request. If this model is only slow to \
         start, raise the agent inactivity timeout (Config > Server, or the step's \
         `stall_timeout_secs`).",
        limit.as_secs(),
        spoken(limit),
    )
}

/// Whether `text` is a failure produced by [`stall_reason`].
pub fn is_stall_reason(text: &str) -> bool {
    text.starts_with(STALL_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn silence_for_the_whole_limit_expires() {
        let watchdog = IdleWatchdog::new(Duration::from_secs(60));
        let started = Instant::now();
        watchdog.expired().await;
        assert_eq!(started.elapsed(), Duration::from_secs(60));
        assert_eq!(watchdog.beats(), 0, "nothing ever arrived");
    }

    #[tokio::test(start_paused = true)]
    async fn progress_restarts_the_clock_and_only_the_last_silence_counts() {
        let watchdog = IdleWatchdog::new(Duration::from_secs(60));
        let started = Instant::now();
        let producer = watchdog.clone();
        tokio::spawn(async move {
            // Slow but alive: one token every 50 s for five minutes. Wall-clock
            // alone would have cut this at the first minute.
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_secs(50)).await;
                producer.beat();
            }
        });
        watchdog.expired().await;
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(6 * 50 + 60),
            "300 s of life, then a full 60 s of silence"
        );
        assert_eq!(watchdog.beats(), 6);
    }

    #[tokio::test(start_paused = true)]
    async fn a_beat_just_before_the_deadline_buys_a_whole_new_delay() {
        let watchdog = IdleWatchdog::new(Duration::from_secs(60));
        let started = Instant::now();
        let producer = watchdog.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(59)).await;
            producer.beat();
        });
        watchdog.expired().await;
        assert_eq!(started.elapsed(), Duration::from_secs(59 + 60));
    }

    /// DoD 2 — the default lets a large model's cold first token through and
    /// still ends a run that has really stopped.
    #[tokio::test(start_paused = true)]
    async fn the_default_lets_a_cold_first_token_through_and_ends_a_dead_stream() {
        let watchdog = IdleWatchdog::new(DEFAULT_IDLE_TIMEOUT);
        let started = Instant::now();
        let producer = watchdog.clone();
        tokio::spawn(async move {
            // Weights loading and the prompt being read: 14 minutes of nothing,
            // then the first token, then one every 5 minutes for an hour.
            tokio::time::sleep(Duration::from_secs(14 * 60)).await;
            for _ in 0..13 {
                producer.beat();
                tokio::time::sleep(Duration::from_secs(5 * 60)).await;
            }
        });
        watchdog.expired().await;
        // Last beat at 14 min + 12 × 5 min = 74 min, then silence for the limit.
        assert_eq!(
            started.elapsed(),
            Duration::from_secs((14 + 12 * 5) * 60) + DEFAULT_IDLE_TIMEOUT,
            "an hour of slow generation was never cut; the silence after it was"
        );
        assert!(DEFAULT_IDLE_TIMEOUT >= Duration::from_secs(10 * 60));
    }

    #[tokio::test(start_paused = true)]
    async fn next_within_gives_up_on_a_silent_stream_and_waits_when_unbounded() {
        let mut silent = futures::stream::pending::<u8>();
        assert_eq!(
            next_within(&mut silent, Some(Duration::from_secs(30))).await,
            Err(Silent)
        );

        let mut slow = Box::pin(async_stream::stream! {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            yield 7u8;
        });
        assert_eq!(
            next_within(&mut slow, None).await,
            Ok(Some(7)),
            "a response that is not streamed is allowed its hour"
        );

        let mut finished = futures::stream::empty::<u8>();
        assert_eq!(
            next_within(&mut finished, Some(Duration::from_secs(30))).await,
            Ok(None)
        );
    }

    #[test]
    fn the_reason_names_the_delay_and_is_recognised_as_a_stall() {
        let reason = stall_reason(
            "Ollama",
            Duration::from_secs(900),
            "without ever sending a first token",
        );
        assert!(is_stall_reason(&reason), "{reason}");
        assert!(reason.contains("no output for 900s"), "{reason}");
        assert!(
            reason.contains("Ollama sent no data for 15 min"),
            "{reason}"
        );
        assert!(
            reason.contains("without ever sending a first token"),
            "{reason}"
        );
        assert!(reason.contains("inactivity timeout"), "{reason}");

        let short = stall_reason("OpenCode", Duration::from_secs(90), "after 3 events");
        assert!(
            short.contains("sent no data for 90 s after 3 events"),
            "{short}"
        );
        assert!(!is_stall_reason("Agent exited with exit code 1"));
    }
}
