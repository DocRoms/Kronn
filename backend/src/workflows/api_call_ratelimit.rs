//! Per-plugin rate limiting for `StepType::ApiCall`.
//!
//! Why this exists: a `BatchQuickPrompt` fan-out that runs an `ApiCall` step
//! against Jira for 50 issues will burst 50 requests in milliseconds. Jira
//! Cloud caps at ~10 req/s/user and starts returning 429 with `Retry-After`
//! after the burst window. Cloudflare's GraphQL is even stricter (cost-based
//! per `account_id`). Without a client-side bucket, every batch run risks a
//! cascading failure: half the requests retry, the rest timeout, and the
//! workflow ends with mostly-empty extractions.
//!
//! Design: a process-global `HashMap<(plugin_slug, config_id), RateLimiter>`
//! lazy-initialised on first use. The bucket lives next to the engine, NOT
//! per-step, so two parallel `ApiCall` steps targeting the same Jira instance
//! share the same quota (which is the whole point — the API server doesn't
//! care which step issued the request, it counts by token). Tokio + governor
//! integrate cleanly: `until_ready().await` yields the runtime until the
//! next slot is available, no busy-wait.
//!
//! Rates are declarative in [`default_rate_per_second`]. Adding a new plugin
//! defaults to "unbounded" (no rate limit) — explicit decisions only. We'd
//! rather miss a limit and 429 once than throttle a plugin that doesn't need
//! it.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::LazyLock;

use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use tokio::sync::Mutex as TokioMutex;

/// Concrete `RateLimiter` type alias to keep `HashMap` ergonomic.
type AppRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Process-global per-(plugin, config) bucket map. Tokio mutex so the
/// async acquire path doesn't block worker threads on the hash lookup.
/// LazyLock = no init at module load — first plugin call creates it.
static BUCKETS: LazyLock<TokioMutex<HashMap<String, Arc<AppRateLimiter>>>> =
    LazyLock::new(|| TokioMutex::new(HashMap::new()));

/// Default request-per-second budget for a plugin slug. `None` = no limit.
///
/// Numbers come from each provider's published soft caps with a ~20% safety
/// margin so a burst doesn't tip us over the actual server-side limit
/// (Jira returns 429 after sustained 10 req/s; we cap at 8). Conservative
/// on purpose — easier to relax than to debug a workflow that 429s in
/// production after weeks of fine.
pub fn default_rate_per_second(plugin_slug: &str) -> Option<u32> {
    match plugin_slug {
        // Atlassian Cloud: docs say 10 req/s per user. 8 keeps us safely
        // under during a batch fan-out without starving the worker.
        "jira" | "atlassian" | "confluence" => Some(8),
        // Cloudflare GraphQL is cost-based per account. 3 req/s is the
        // empirical floor that doesn't trip "RATE_LIMIT" responses on
        // analytics-heavy queries.
        "cloudflare" => Some(3),
        // Adobe Analytics IMS: 25 req/s, but the OAuth2 round-trip burns
        // budget too. 10 leaves headroom.
        "adobe-analytics" => Some(10),
        // Google Search: 100 queries/day on the free tier — rate limit
        // alone won't save us, but we still cap to avoid burning the
        // daily quota during a noisy workflow.
        "google-search" => Some(2),
        // Chartbeat: published as "unbounded" for paying customers. Skip.
        "chartbeat" => None,
        // GitHub: 5000 req/h authenticated = ~1.4 req/s sustainable. We
        // cap at 1 req/s to stay clear during bursts; long workflows are
        // the danger here, not bursts.
        "github" => Some(1),
        // Anything not listed: unbounded. Add an entry when you observe
        // 429s — explicit decisions only.
        _ => None,
    }
}

/// Acquire a rate-limit slot for `(plugin_slug, config_id)`. Returns
/// immediately when the plugin has no configured rate; otherwise yields
/// the runtime until the bucket has a token.
///
/// `config_id` is part of the key because two configurations of the same
/// plugin (two Jira instances) hit different servers and don't share a
/// quota. Same plugin + same config = same bucket = parallel steps share
/// the budget, which matches what the server enforces.
pub async fn acquire_slot(plugin_slug: &str, config_id: &str) {
    let Some(rate) = default_rate_per_second(plugin_slug) else {
        return;
    };
    let key = bucket_key(plugin_slug, config_id);

    // Get-or-insert the limiter under a short-lived lock, then release
    // before awaiting — holding the map mutex across `.await` would
    // serialise every other plugin's acquire call.
    let limiter = {
        let mut map = BUCKETS.lock().await;
        match map.get(&key) {
            Some(l) => l.clone(),
            None => {
                let new_limiter = Arc::new(RateLimiter::direct(quota_for_rate(rate)));
                map.insert(key.clone(), new_limiter.clone());
                new_limiter
            }
        }
    };

    limiter.until_ready().await;
}

/// Wait-or-skip variant for tests + diagnostic UIs: returns `true` when
/// a slot was available immediately, `false` when we'd have to wait.
/// Production code should use [`acquire_slot`] which actually blocks.
#[cfg(test)]
pub async fn try_acquire_slot(plugin_slug: &str, config_id: &str) -> bool {
    let Some(rate) = default_rate_per_second(plugin_slug) else {
        return true;
    };
    let key = bucket_key(plugin_slug, config_id);

    let limiter = {
        let mut map = BUCKETS.lock().await;
        match map.get(&key) {
            Some(l) => l.clone(),
            None => {
                let new_limiter = Arc::new(RateLimiter::direct(quota_for_rate(rate)));
                map.insert(key.clone(), new_limiter.clone());
                new_limiter
            }
        }
    };

    limiter.check().is_ok()
}

fn bucket_key(plugin_slug: &str, config_id: &str) -> String {
    // Pipe is fine — neither slugs nor uuid-style config ids contain it.
    format!("{plugin_slug}|{config_id}")
}

fn quota_for_rate(rate_per_sec: u32) -> Quota {
    // `NonZeroU32::new(0)` returns None — we already handle the `None`
    // case in `default_rate_per_second`, so unwrap_or(1) is just a
    // defense against future refactors that might lose the guard.
    Quota::per_second(NonZeroU32::new(rate_per_sec).unwrap_or(NonZeroU32::new(1).unwrap()))
        .allow_burst(NonZeroU32::new(rate_per_sec).unwrap_or(NonZeroU32::new(1).unwrap()))
}

/// Reset the global state — test-only. Avoids cross-test pollution when
/// two cases hit the same plugin slug.
#[cfg(test)]
pub async fn reset_buckets() {
    BUCKETS.lock().await.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn default_rate_for_known_plugins_matches_expected_floors() {
        // Regression guard — these numbers feed every production workflow.
        // A bump here without a comment explaining why = treat as a bug.
        assert_eq!(default_rate_per_second("jira"), Some(8));
        assert_eq!(default_rate_per_second("atlassian"), Some(8));
        assert_eq!(default_rate_per_second("confluence"), Some(8));
        assert_eq!(default_rate_per_second("cloudflare"), Some(3));
        assert_eq!(default_rate_per_second("adobe-analytics"), Some(10));
        assert_eq!(default_rate_per_second("google-search"), Some(2));
        assert_eq!(default_rate_per_second("github"), Some(1));
    }

    #[test]
    fn unknown_plugin_is_unbounded_by_default() {
        // Adding a new plugin shouldn't accidentally throttle it. We
        // require an explicit entry — silent throttling is harder to
        // debug than a 429 (which at least tells you which API is mad).
        assert_eq!(default_rate_per_second("brand-new-plugin"), None);
        assert_eq!(default_rate_per_second(""), None);
        assert_eq!(default_rate_per_second("chartbeat"), None);
    }

    #[test]
    fn bucket_key_separates_plugin_and_config() {
        // Two Jira instances on the same project use different keys → don't
        // share the quota. Each instance hits its own server.
        let k1 = bucket_key("jira", "cfg-staging");
        let k2 = bucket_key("jira", "cfg-prod");
        assert_ne!(k1, k2);
        // Same instance accessed twice → same bucket → shared quota.
        assert_eq!(k1, bucket_key("jira", "cfg-staging"));
    }

    #[tokio::test]
    #[serial]
    async fn acquire_slot_returns_immediately_for_unbounded_plugin() {
        reset_buckets().await;
        // Chartbeat has no rate limit configured. Acquire must not
        // create a bucket nor block.
        let start = std::time::Instant::now();
        acquire_slot("chartbeat", "cfg-1").await;
        assert!(
            start.elapsed().as_millis() < 50,
            "unbounded plugin should acquire instantly, took {}ms",
            start.elapsed().as_millis()
        );
        let map_size = BUCKETS.lock().await.len();
        assert_eq!(
            map_size, 0,
            "no bucket should be created for an unbounded plugin"
        );
    }

    #[tokio::test]
    #[serial]
    async fn acquire_slot_creates_bucket_for_rate_limited_plugin() {
        reset_buckets().await;
        acquire_slot("jira", "cfg-test").await;
        let map = BUCKETS.lock().await;
        assert!(
            map.contains_key("jira|cfg-test"),
            "bucket should be created on first call"
        );
    }

    #[tokio::test]
    #[serial]
    async fn try_acquire_slot_first_call_succeeds_then_burst_exhausts() {
        reset_buckets().await;
        // Cloudflare = 3/s with burst=3. First 3 calls fit in the burst.
        // The 4th must reject (would have to wait).
        assert!(try_acquire_slot("cloudflare", "cfg-burst").await);
        assert!(try_acquire_slot("cloudflare", "cfg-burst").await);
        assert!(try_acquire_slot("cloudflare", "cfg-burst").await);
        // 4th — burst exhausted, should be false.
        let fourth = try_acquire_slot("cloudflare", "cfg-burst").await;
        assert!(
            !fourth,
            "4th call within 1s on a 3/s bucket should be rate-limited"
        );
    }

    #[tokio::test]
    #[serial]
    async fn separate_configs_dont_share_a_bucket() {
        reset_buckets().await;
        // Burst-exhaust one config — the other config of the same plugin
        // must still let calls through (different Jira instances).
        for _ in 0..3 {
            try_acquire_slot("cloudflare", "cfg-A").await;
        }
        // cfg-A is exhausted, but cfg-B has its own fresh bucket.
        assert!(
            try_acquire_slot("cloudflare", "cfg-B").await,
            "second config must not be throttled by the first config's burst"
        );
    }

    #[tokio::test]
    #[serial]
    async fn acquire_slot_unblocks_after_token_replenish() {
        reset_buckets().await;
        // 1 req/s plugin: burst-exhaust, then `acquire_slot` should yield
        // and resolve roughly within 1s. We cap at 2s to absorb CI jitter.
        for _ in 0..1 {
            try_acquire_slot("github", "cfg-replenish").await;
        }
        let start = std::time::Instant::now();
        acquire_slot("github", "cfg-replenish").await;
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() <= 2000,
            "acquire_slot should unblock within ~1s for a 1/s plugin, took {}ms",
            elapsed.as_millis()
        );
    }
}
