//! As `uplink`, for the calls that carry the application state.

use crate::agents::tools::ToolExecutor;
use crate::models::{AgentType, ApiResponse, CreateDiscussionRequest, Discussion};
use crate::workflows::step_room::StepRoomMember;
use crate::AppState;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type Async<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Creates a discussion as the HTTP endpoint does.
pub struct DiscussionCreate(
    pub fn(AppState, CreateDiscussionRequest) -> Async<'static, ApiResponse<Discussion>>,
);
inventory::collect!(DiscussionCreate);

/// Offers an escalated execution to the next agent of its fallback chain.
pub struct EscalatedFallback(
    pub for<'a> fn(&'a AppState, &'a str, Option<Vec<AgentType>>) -> Async<'a, anyhow::Result<bool>>,
);
inventory::collect!(EscalatedFallback);

/// The Kronn tools of a workflow Agent step:
/// (state, project, run, step name, room member).
pub struct WorkflowRoomTools(
    pub  fn(
        AppState,
        Option<String>,
        String,
        String,
        Option<StepRoomMember>,
    ) -> Arc<dyn ToolExecutor>,
);
inventory::collect!(WorkflowRoomTools);

/// The Kronn tools of a step run from its Test button:
/// (state, project, step name, dry run).
pub struct WorkflowTestTools(
    pub fn(AppState, Option<String>, String, bool) -> Arc<dyn ToolExecutor>,
);
inventory::collect!(WorkflowTestTools);
