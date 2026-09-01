//! Media generation (image / video) over HTTP providers.
//!
//! Two provider facts drive this whole model, both measured rather than
//! assumed on `bytedance/seedance-2.0-mini` (5 s, "480p", 16:9):
//!   * the requested resolution is NOT what comes back (864x496), so rendered
//!     dimensions are read from the produced file, never from the request;
//!   * the billed cost is NOT rate x duration (0.0708932 USD against 0.0678
//!     implied) and the usage payload carries no token count, so the declared
//!     cost is persisted verbatim in its own counter.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What the model outputs. The execution family is the same for both, so this
/// belongs to the job and its result — not to a separate run kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MediaModality {
    Image,
    Video,
}

impl MediaModality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MediaJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl MediaJobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "timed_out" => Some(Self::TimedOut),
            _ => None,
        }
    }

    /// A job still owed work. Terminal states are never re-claimed.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    /// Maps to the shared run vocabulary consumed by `RunStatusCard`. Typed
    /// rather than a string: a typo in a status name would otherwise reach
    /// the UI as an unknown state.
    pub fn shared_run_status(self) -> crate::models::SharedRunStatus {
        use crate::models::SharedRunStatus as S;
        match self {
            Self::Pending => S::Queued,
            Self::Running => S::Running,
            Self::Completed => S::Success,
            Self::Failed => S::Failed,
            Self::Cancelled => S::Cancelled,
            Self::TimedOut => S::Timeout,
        }
    }
}

/// Generation parameters as REQUESTED. The provider may honour them loosely,
/// so nothing downstream may treat these as describing the output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generate_audio: Option<bool>,
}

/// Cost of one generation, as the provider declared it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaCost {
    /// Verbatim provider value. Recomputing it from a published rate drifts
    /// from the actual invoice.
    pub cost_usd: f64,
    /// Bring-your-own-key generations can legitimately cost nothing; keeping
    /// the flag stops a zero from looking like a measurement failure.
    pub is_byok: bool,
}

/// What actually came back, read from the produced file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaRendered {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Coarse provider phase, for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MediaPhase {
    Submitting,
    Polling,
    Downloading,
    Persisting,
}

/// Versioned payload published on the shared run. `progress` is absent unless
/// the provider actually measures it — an invented percentage is worse than
/// none, because it looks authoritative.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MediaRunResult {
    pub schema_version: u32,
    pub modality: MediaModality,
    pub phase: MediaPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_byok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_duration_ms: Option<u64>,
}

pub const MEDIA_RUN_RESULT_SCHEMA_VERSION: u32 = 1;

impl MediaRunResult {
    pub fn new(modality: MediaModality, phase: MediaPhase) -> Self {
        Self {
            schema_version: MEDIA_RUN_RESULT_SCHEMA_VERSION,
            modality,
            phase,
            progress: None,
            generation_id: None,
            asset_id: None,
            message_id: None,
            cost_usd: None,
            is_byok: None,
            width: None,
            height: None,
            media_duration_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_through_their_db_form() {
        for status in [
            MediaJobStatus::Pending,
            MediaJobStatus::Running,
            MediaJobStatus::Completed,
            MediaJobStatus::Failed,
            MediaJobStatus::Cancelled,
            MediaJobStatus::TimedOut,
        ] {
            assert_eq!(MediaJobStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(MediaJobStatus::parse("Running"), None, "parsing is exact");
    }

    #[test]
    fn only_pending_and_running_are_claimable() {
        assert!(MediaJobStatus::Pending.is_active());
        assert!(MediaJobStatus::Running.is_active());
        for done in [
            MediaJobStatus::Completed,
            MediaJobStatus::Failed,
            MediaJobStatus::Cancelled,
            MediaJobStatus::TimedOut,
        ] {
            assert!(!done.is_active());
        }
    }

    #[test]
    fn shared_run_mapping_matches_the_agreed_vocabulary() {
        // Compared on the wire form, which is what RunStatusCard reads.
        let wire = |s: MediaJobStatus| serde_json::to_string(&s.shared_run_status()).unwrap();
        assert_eq!(wire(MediaJobStatus::Pending), "\"queued\"");
        assert_eq!(wire(MediaJobStatus::Running), "\"running\"");
        assert_eq!(wire(MediaJobStatus::Completed), "\"success\"");
        assert_eq!(wire(MediaJobStatus::Failed), "\"failed\"");
        assert_eq!(wire(MediaJobStatus::Cancelled), "\"cancelled\"");
        assert_eq!(wire(MediaJobStatus::TimedOut), "\"timeout\"");
    }

    #[test]
    fn media_is_a_single_shared_run_kind() {
        // Guards the contract: no per-modality kind may appear.
        let kind = serde_json::to_string(&crate::models::SharedRunKind::Media).unwrap();
        assert_eq!(kind, "\"media\"");
    }

    #[test]
    fn a_fresh_run_result_carries_no_invented_progress() {
        let r = MediaRunResult::new(MediaModality::Video, MediaPhase::Polling);
        assert_eq!(r.schema_version, 1);
        assert!(r.progress.is_none(), "progress exists only when measured");
        // Omitted rather than serialised as null, so consumers cannot read a
        // placeholder as a real measurement.
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("progress"), "got {json}");
    }

    #[test]
    fn modality_serialises_lowercase_for_the_wire_and_the_db() {
        assert_eq!(MediaModality::Video.as_str(), "video");
        assert_eq!(
            serde_json::to_string(&MediaModality::Image).unwrap(),
            "\"image\""
        );
        assert_eq!(MediaModality::parse("video"), Some(MediaModality::Video));
        assert_eq!(MediaModality::parse("Video"), None);
    }
}
