//! Deterministic launch context threaded from a source discussion into the
//! Quick Prompt, Quick API, Quick Exec and Workflow runners a `kronn-action`
//! proposal may launch (KT-476).
//!
//! A launch always prefers a target's own declared project. This context
//! only fills the gap for a GLOBAL target so it resolves the same project
//! environment/worktree and the source discussion's retention override that
//! a human would get triggering it directly from that project — instead of
//! silently falling back to no project (temp dir, no env) as every runner
//! did before this existed.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct LaunchContext {
    pub discussion_id: Option<String>,
    pub project_id: Option<String>,
    pub context: HashMap<String, String>,
}

impl LaunchContext {
    pub fn from_discussion(discussion_id: String, project_id: Option<String>) -> Self {
        Self {
            discussion_id: Some(discussion_id),
            project_id,
            context: HashMap::new(),
        }
    }

    /// A target's own declared project always wins; a global target falls
    /// back to this context's project (e.g. the discussion that proposed it).
    pub fn effective_project_id<'a>(
        &'a self,
        target_project_id: Option<&'a str>,
    ) -> Option<&'a str> {
        target_project_id.or(self.project_id.as_deref())
    }
}
