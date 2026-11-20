//! Tests of `crate::agents::runner::runner_test` that reach into the layers above it.
#![allow(unused_imports)]
use crate::agents::idle_watchdog::{self, IdleWatchdog};
use crate::agents::ollama_memory::{
    ceiling_for_model, kv_shape_from_show, kv_shape_from_store, CeilingBasis, CeilingInputs,
    KvShape, MachineFacts, ModelCeiling,
};
use crate::agents::provenance::{self, AgentProvenanceCapture};
use crate::agents::runner::runner_test::*;
use crate::agents::runner::*;
use crate::agents::wsl::platform_agent_invocation;
#[allow(unused_imports)]
use crate::agents::wsl::{agent_runs_in_wsl, is_wsl};
use crate::core::child_env::{
    RoomAgentBridgeContext, TaskWorkerBridgeContext, WorkflowStepBridgeContext,
};
use crate::core::cmd::{async_cmd, async_cmd_with, Priority};
use crate::models::{AgentType, ModelTier, ModelTiersConfig, Skill, TokensConfig};
use crate::models::{PromptCacheUsage, ReportedUsage};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[path = "../agents/runner_test_tests_up.rs"]
pub(crate) mod tests;
