//! Pure scope router for continual learning (spec §7).
//!
//! `Preference` → `User` (writes `~/.kronn/user-context/learnings.md`).
//! `Fact`/`Inference` with a project → `Project` (writes `docs/learnings.md`).
//! Else → `User`. Promotion targets a DEDICATED file, never an audited file
//! (avoids invalidating drift checksums). No I/O here — pure decision.

use crate::models::learnings::{LearningKind, LearningScope};

pub fn route_scope(kind: LearningKind, project_id: Option<&str>) -> LearningScope {
    match kind {
        LearningKind::Preference => LearningScope::User,
        LearningKind::Fact | LearningKind::Inference => {
            if project_id.is_some() {
                LearningScope::Project
            } else {
                LearningScope::User
            }
        }
    }
}

/// The dedicated promotion file for a scope (never an audited file).
pub fn promotion_target(scope: LearningScope) -> &'static str {
    match scope {
        LearningScope::Project => "docs/learnings.md",
        LearningScope::User => "learnings.md", // under ~/.kronn/user-context/
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_always_user() {
        assert_eq!(
            route_scope(LearningKind::Preference, Some("p1")),
            LearningScope::User
        );
        assert_eq!(
            route_scope(LearningKind::Preference, None),
            LearningScope::User
        );
    }

    #[test]
    fn fact_inference_route_on_project_presence() {
        assert_eq!(
            route_scope(LearningKind::Fact, Some("p1")),
            LearningScope::Project
        );
        assert_eq!(route_scope(LearningKind::Fact, None), LearningScope::User);
        assert_eq!(
            route_scope(LearningKind::Inference, Some("p1")),
            LearningScope::Project
        );
        assert_eq!(
            route_scope(LearningKind::Inference, None),
            LearningScope::User
        );
    }

    #[test]
    fn targets_are_dedicated_files() {
        assert_eq!(
            promotion_target(LearningScope::Project),
            "docs/learnings.md"
        );
        assert_eq!(promotion_target(LearningScope::User), "learnings.md");
    }
}
