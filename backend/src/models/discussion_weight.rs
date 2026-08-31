//! Storage footprint of a discussion, split by what a cleanup can reclaim.
//! A single aggregate would say a discussion is heavy without saying what to
//! do about it, which is the only question the indicator has to answer.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Default amber threshold on the total footprint, in bytes. A 5 s / 480p
/// generated video weighs ~1.5 MB, so this leaves room for a dozen media.
pub const DEFAULT_AMBER_BYTES: u64 = 25 * 1024 * 1024;
/// Default red threshold on the total footprint, in bytes.
pub const DEFAULT_RED_BYTES: u64 = 100 * 1024 * 1024;

/// Where the bytes of one discussion live, ordered by recoverability.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DiscussionWeight {
    pub discussion_id: String,
    /// Attachment bytes held on disk. Reclaimable without losing the thread.
    pub disk_bytes: u64,
    /// Extracted document text kept in the database. Reclaimable, but the
    /// document search over those files goes with it.
    pub extracted_text_bytes: u64,
    /// Message content bytes. Not reclaimable without losing conversation.
    pub message_bytes: u64,
}

impl DiscussionWeight {
    pub fn new(discussion_id: impl Into<String>) -> Self {
        Self {
            discussion_id: discussion_id.into(),
            ..Default::default()
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.disk_bytes
            .saturating_add(self.extracted_text_bytes)
            .saturating_add(self.message_bytes)
    }

    /// Bytes a cleanup could drop without losing any conversation content.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.disk_bytes
    }

    /// Level is graded on the total, since that is what the discussion
    /// actually costs; `reclaimable_bytes` travels alongside so the UI can
    /// tell an actionable red from an unavoidable one.
    pub fn level(&self, thresholds: &WeightThresholds) -> WeightLevel {
        let total = self.total_bytes();
        if total >= thresholds.red_bytes {
            WeightLevel::Red
        } else if total >= thresholds.amber_bytes {
            WeightLevel::Amber
        } else {
            WeightLevel::Green
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WeightLevel {
    Green,
    Amber,
    Red,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WeightThresholds {
    pub amber_bytes: u64,
    pub red_bytes: u64,
}

impl Default for WeightThresholds {
    fn default() -> Self {
        Self {
            amber_bytes: DEFAULT_AMBER_BYTES,
            red_bytes: DEFAULT_RED_BYTES,
        }
    }
}

impl WeightThresholds {
    /// Only `0 < amber < red` grades anything meaningfully. Anything else
    /// falls back to the defaults as a whole rather than being silently
    /// patched, so a bad config never yields a half-plausible scale.
    pub fn validated(self) -> Self {
        let ok = self.amber_bytes > 0 && self.amber_bytes < self.red_bytes;
        if ok {
            self
        } else {
            Self::default()
        }
    }

    /// True when the pair was usable as supplied — lets a caller report the
    /// fallback instead of hiding it.
    pub fn is_valid(&self) -> bool {
        self.amber_bytes > 0 && self.amber_bytes < self.red_bytes
    }
}

/// One discussion's weight with its graded level, as served to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionWeightView {
    #[serde(flatten)]
    pub weight: DiscussionWeight,
    pub total_bytes: u64,
    pub reclaimable_bytes: u64,
    pub level: WeightLevel,
}

impl DiscussionWeightView {
    pub fn of(weight: DiscussionWeight, thresholds: &WeightThresholds) -> Self {
        let level = weight.level(thresholds);
        Self {
            total_bytes: weight.total_bytes(),
            reclaimable_bytes: weight.reclaimable_bytes(),
            level,
            weight,
        }
    }
}

/// Batch answer. `weights` is SPARSE and indexed by discussion id: an id that
/// was requested but holds nothing is absent, which the UI must render as
/// "empty" rather than as a zero it could confuse with a failed load.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionWeightsResponse {
    pub weights: std::collections::HashMap<String, DiscussionWeightView>,
    pub thresholds: WeightThresholds,
    /// True when the configured pair was unusable and the defaults took over.
    /// Surfaced rather than hidden, so a bad config is visible.
    pub thresholds_from_defaults: bool,
}

fn default_enabled() -> bool {
    true
}
fn default_amber() -> u64 {
    DEFAULT_AMBER_BYTES
}
fn default_red() -> u64 {
    DEFAULT_RED_BYTES
}

/// Sidebar weight indicator settings.
///
/// Deliberately not an `Option`: an absent section loads straight into a valid
/// effective state, so no caller has to interpret `None`. Each field carries
/// its own default too, so a PARTIAL section (`enabled = false` alone) keeps
/// usable thresholds instead of collapsing them to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionWeightConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_amber")]
    pub amber_bytes: u64,
    #[serde(default = "default_red")]
    pub red_bytes: u64,
}

impl Default for DiscussionWeightConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            amber_bytes: default_amber(),
            red_bytes: default_red(),
        }
    }
}

impl DiscussionWeightConfig {
    pub fn thresholds(&self) -> WeightThresholds {
        WeightThresholds {
            amber_bytes: self.amber_bytes,
            red_bytes: self.red_bytes,
        }
    }

    /// Pair actually applied, plus whether the stored one was unusable. The
    /// fallback is reported rather than hidden so a bad config stays visible.
    pub fn effective_thresholds(&self) -> (WeightThresholds, bool) {
        let stored = self.thresholds();
        (stored.validated(), !stored.is_valid())
    }

    /// Accepts a candidate pair only as a whole: a half-applied change would
    /// leave a scale that grades nothing meaningfully.
    pub fn with_thresholds(self, amber_bytes: u64, red_bytes: u64) -> Result<Self, String> {
        let candidate = WeightThresholds {
            amber_bytes,
            red_bytes,
        };
        if !candidate.is_valid() {
            return Err(format!(
                "thresholds must satisfy 0 < amber < red (got amber={amber_bytes}, red={red_bytes})"
            ));
        }
        Ok(Self {
            amber_bytes,
            red_bytes,
            ..self
        })
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn an_absent_section_loads_into_a_valid_effective_state() {
        // Older configs have no section at all: it must not need interpreting.
        let cfg: DiscussionWeightConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, DiscussionWeightConfig::default());
        assert!(cfg.enabled);
        let (t, fell_back) = cfg.effective_thresholds();
        assert!(!fell_back);
        assert_eq!(t.amber_bytes, DEFAULT_AMBER_BYTES);
        assert_eq!(t.red_bytes, DEFAULT_RED_BYTES);
    }

    #[test]
    fn a_partial_section_keeps_usable_thresholds() {
        // Per-field defaults matter here: without them `enabled = false`
        // alone would collapse both bounds to zero and grade nothing.
        let cfg: DiscussionWeightConfig = toml::from_str("enabled = false").unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.amber_bytes, DEFAULT_AMBER_BYTES);
        assert_eq!(cfg.red_bytes, DEFAULT_RED_BYTES);
        assert!(!cfg.effective_thresholds().1);
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg = DiscussionWeightConfig {
            enabled: false,
            amber_bytes: 1_000,
            red_bytes: 2_000,
        };
        let back: DiscussionWeightConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn a_stored_invalid_pair_falls_back_and_says_so() {
        let cfg: DiscussionWeightConfig =
            toml::from_str("amber_bytes = 900\nred_bytes = 100").unwrap();
        let (t, fell_back) = cfg.effective_thresholds();
        assert!(fell_back, "the fallback must be reported, not hidden");
        assert_eq!(t.amber_bytes, DEFAULT_AMBER_BYTES);
        assert_eq!(t.red_bytes, DEFAULT_RED_BYTES);
    }

    #[test]
    fn the_setter_refuses_an_invalid_pair_whole_rather_than_half_applying() {
        let cfg = DiscussionWeightConfig::default();
        for (amber, red) in [(900u64, 100u64), (0, 100), (50, 50)] {
            let err = cfg.with_thresholds(amber, red).unwrap_err();
            assert!(err.contains("0 < amber < red"), "unexpected message: {err}");
        }
        // The rejected attempts left the original untouched.
        assert_eq!(cfg, DiscussionWeightConfig::default());

        let updated = cfg.with_thresholds(10, 20).unwrap();
        assert_eq!(updated.amber_bytes, 10);
        assert_eq!(updated.red_bytes, 20);
        // The toggle is carried over, never reset by a threshold change.
        assert_eq!(updated.enabled, cfg.enabled);
    }
}
