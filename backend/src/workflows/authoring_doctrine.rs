//! KT-1216 — the one authoring doctrine every agent reads before composing a
//! workflow. It lives in `authoring_doctrine.md` and reaches agents by two
//! doors that cannot drift because both read that file: the `authoring_doctrine`
//! field of `workflow_step_schema`, and the `workflow-architect` guide, whose
//! source marks where it goes. The step-type list is rendered from
//! [`StepType::ALL`] the same way, wherever Kronn writes it out.
use crate::models::StepType;

/// Short design doctrine: decision tree, equal-cost rules, ApiCall limits.
pub const AUTHORING_DOCTRINE: &str = include_str!("authoring_doctrine.md");

/// Where the guide's source takes the doctrine in.
pub const DOCTRINE_MARKER: &str = "<!-- kronn:authoring-doctrine -->";
/// Where the guide's source takes the step-type names in.
pub const STEP_TYPES_MARKER: &str = "<!-- kronn:step-types -->";
/// Where the guide's source takes the step-type count in.
pub const STEP_TYPE_COUNT_MARKER: &str = "<!-- kronn:step-type-count -->";

/// The step types as `` `Agent`, `ApiCall`, … ``, from the code.
pub fn step_types_inline() -> String {
    StepType::names()
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A guide's source with its markers replaced by what the code owns.
pub fn render_guide(source: &str) -> String {
    source
        .replace(DOCTRINE_MARKER, AUTHORING_DOCTRINE.trim_end())
        .replace(STEP_TYPES_MARKER, &step_types_inline())
        .replace(STEP_TYPE_COUNT_MARKER, &StepType::ALL.len().to_string())
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "authoring_doctrine_test.rs"]
mod tests;
