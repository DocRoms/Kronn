//! Wire types for the NVIDIA provider card (KT-337).
//!
//! The shape differs from LiteLLM's on one point that matters: a catalogue entry
//! carries a *verification state*. NVIDIA's `/v1/models` answers 200 without a key
//! and lists everything the service knows — including models this account cannot
//! call (404), models past end of life (410), and models that never answer. So the
//! UI must be able to show "listed" and "actually usable" as different things,
//! otherwise a user assigns a tier to a model that fails on first use.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One catalogue entry, plus what a real probe said about it (if one ran).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NvidiaModel {
    /// The id to send as `model`, e.g. `meta/llama-3.1-8b-instruct`.
    pub id: String,
    /// Vendor prefix of the id (`meta`, `nvidia`, `deepseek-ai`…). Taken from the
    /// id rather than `owned_by`, which is the same information without the risk
    /// of the two disagreeing.
    pub vendor: String,
    /// Last known probe verdict for this id, `None` when never probed. The UI
    /// gates tier assignment on this being `Usable`.
    pub probe: Option<NvidiaProbeVerdict>,
}

/// What a real invocation established. Kept distinct from a plain boolean because
/// the reasons are not interchangeable: `Retired` and `NotEntitled` are dead ends,
/// while `NoAnswer` may be a cold start worth retrying.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NvidiaProbeVerdict {
    /// Answered — safe to assign to a tier.
    Usable,
    /// `404 … not found for account`: listed by the service, not granted here.
    NotEntitled,
    /// `410`: the service retired it.
    Retired,
    /// Any other error status.
    Refused,
    /// No answer within the deadline. NOT a refusal: retrying may succeed.
    NoAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NvidiaModelsResponse {
    pub models: Vec<NvidiaModel>,
    /// Endpoint the catalogue came from, so the card can show which service it is
    /// talking to (the hosted endpoint, or a self-hosted NIM).
    pub endpoint: String,
    /// Whether a key is configured. The catalogue lists fine without one, which
    /// would otherwise look like a working setup right up to the first real call.
    pub has_key: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct NvidiaProbeRequest {
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NvidiaProbeResponse {
    pub model: String,
    pub verdict: NvidiaProbeVerdict,
    /// Human-readable explanation for the card. Never the raw upstream body when
    /// it could carry account identifiers.
    pub detail: String,
}
