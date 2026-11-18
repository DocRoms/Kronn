//! Tests of `crate::api::trust` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::strong_confirm::CONFIRM_HEADER;
use crate::api::trust::*;
use crate::core::trust_proof::{self, Binding, Channel, ProofError};
use crate::db::trust::{self, TrustOrigin};
use crate::models::{
    ApiErrorCode, ApiResponse, ConfirmTrustProofRequest, EffectiveTrust, SetTrustRequest,
    TrustAuditEntry, TrustEnrolment, TrustLevel, TrustOverview, TrustProofRequest, TrustProofView,
    TrustRecoveryCodes, TrustSandboxView, TrustScope, TrustSetting,
};
use crate::AppState;
use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
#[path = "../api/trust_gate_tests.rs"]
pub(crate) mod trust_gate_tests;
