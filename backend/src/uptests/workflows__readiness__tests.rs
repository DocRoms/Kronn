//! Tests of `crate::workflows::readiness` that reach into the layers above it.
#![allow(unused_imports)]
use crate::models::{
    AgentType, BlockedQuickExec, QuickExec, StepType, Workflow, WorkflowBlocker,
    WorkflowBlockerKind, WorkflowReadiness, WorkflowStep, WorkflowWarning,
};
use crate::workflows::doctrine_warnings::{self, ConfiguredApi};
use crate::workflows::readiness::*;
use std::collections::{HashMap, HashSet};
#[path = "../workflows/readiness_test.rs"]
pub(crate) mod tests;
