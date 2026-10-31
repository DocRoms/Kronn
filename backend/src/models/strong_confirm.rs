//! KT-1496 — the grant the screen obtains once the code is typed.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A code, to exchange for a grant on one route and one object.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct StrongGrantRequest {
    /// The HTTP method of the protected call, e.g. `DELETE`.
    pub method: String,
    /// The path of the protected call, e.g. `/api/mcps/configs/abc/reveal`.
    pub path: String,
    /// The current code of the authenticator, or a recovery code. The
    /// `X-Kronn-Confirm` header works as well.
    #[serde(default)]
    pub code: Option<String>,
}

/// A single-use grant for the one call it names.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StrongGrant {
    /// Sent as `X-Kronn-Grant` on the protected call.
    pub grant: String,
    pub expires_in_secs: u32,
}
