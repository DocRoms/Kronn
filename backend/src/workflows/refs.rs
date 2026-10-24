//! Named resource references of a workflow (KT-1448): `refs.<name>` names a
//! Page, workflow, Quick Prompt / API / Exec or skill by id. Steps render it as
//! `{{refs.<name>}}`; Exec steps also get `KRONN_REF_<NAME>`. Export bundles the
//! objects and import remaps every ref to the new id.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;

use crate::models::{ResourceRefKind, WorkflowRefs, WorkflowStep};

pub const MAX_REFS: usize = 32;
const MAX_NAME_TAIL: usize = 40;
const MAX_ID_LEN: usize = 128;

/// `[a-z][a-z0-9_]{0,40}`.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= MAX_NAME_TAIL + 1
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `[A-Za-z0-9][A-Za-z0-9_-]{0,127}`: no character a shell, an interpreter
/// or a URL reads as syntax, and never an option (`-x`), which is what lets
/// inline code take `{{refs.<name>}}`.
pub fn is_safe_id(id: &str) -> bool {
    id.len() <= MAX_ID_LEN
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The `refs.<name>` template path of a valid ref name, if `path` is one.
pub fn ref_name_of_path(path: &str) -> Option<&str> {
    path.strip_prefix("refs.")
        .filter(|name| is_valid_name(name))
}

/// `KRONN_REF_<NAME>`, the Exec environment variable of ref `name`.
pub fn env_name(name: &str) -> String {
    format!("KRONN_REF_{}", name.to_ascii_uppercase())
}

/// Names, count and id charset; what does not need the database.
pub fn validate_syntax(refs: &WorkflowRefs) -> Result<(), String> {
    if refs.len() > MAX_REFS {
        return Err(format!("Too many refs ({}, max {MAX_REFS})", refs.len()));
    }
    for (name, reference) in refs {
        if !is_valid_name(name) {
            return Err(format!(
                "Ref name `{name}` is invalid: use a lowercase letter, then lowercase letters, digits or `_` (41 characters max)"
            ));
        }
        if !is_safe_id(&reference.id) {
            return Err(format!(
                "Ref `{name}`: id `{}` is not a Kronn id (a letter or digit, then letters, digits, `_` or `-`)",
                reference.id
            ));
        }
    }
    Ok(())
}

fn collect_strings<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => out.push(text),
        serde_json::Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// Every `refs.<name>` the steps render, by name.
pub fn used_names(steps: &[WorkflowStep]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for step in steps {
        let Ok(value) = serde_json::to_value(step) else {
            continue;
        };
        let mut texts = Vec::new();
        collect_strings(&value, &mut texts);
        for text in texts {
            for path in crate::workflows::template::placeholder_paths(text).unwrap_or_default() {
                if let Some(name) = path.strip_prefix("refs.") {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

/// A step that renders `{{refs.<name>}}` names a declared ref.
pub fn validate_usage(
    steps: &[WorkflowStep],
    on_failure: &[WorkflowStep],
    refs: &WorkflowRefs,
) -> Result<(), String> {
    let mut used = used_names(steps);
    used.extend(used_names(on_failure));
    let missing: Vec<String> = used
        .into_iter()
        .filter(|name| !refs.contains_key(name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Steps use undeclared refs: {}. Declare them under the workflow's refs.",
            missing
                .iter()
                .map(|name| format!("`refs.{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Every ref exists and is global or in `project_id`, like the Quick Execs
/// a step names (save-time check).
pub fn validate_in_db(
    conn: &Connection,
    refs: &WorkflowRefs,
    project_id: Option<&str>,
) -> anyhow::Result<Result<(), String>> {
    for (name, reference) in refs {
        let id = reference.id.as_str();
        let owner: Option<Option<String>> = match reference.kind {
            ResourceRefKind::Page => conn
                .query_row(
                    "SELECT project_id FROM live_pages WHERE id = ?1",
                    [id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?,
            ResourceRefKind::Workflow => {
                crate::db::workflows::get_workflow(conn, id)?.map(|w| w.project_id)
            }
            ResourceRefKind::QuickPrompt => {
                crate::db::quick_prompts::get_quick_prompt(conn, id)?.map(|q| q.project_id)
            }
            ResourceRefKind::QuickApi => {
                crate::db::quick_apis::get_quick_api(conn, id)?.map(|q| q.project_id)
            }
            ResourceRefKind::QuickExec => {
                crate::db::quick_execs::get_quick_exec(conn, id)?.map(|q| q.project_id)
            }
            ResourceRefKind::Skill => crate::core::skills::get_skill(id).map(|s| s.project_id),
        };
        let kind = reference.kind.as_str();
        let Some(owner) = owner else {
            return Ok(Err(format!("Ref `{name}`: {kind} `{id}` not found")));
        };
        if owner
            .as_deref()
            .is_some_and(|owner| Some(owner) != project_id)
        {
            return Ok(Err(format!(
                "Ref `{name}`: {kind} `{id}` belongs to another project"
            )));
        }
    }
    Ok(Ok(()))
}

/// The ids the refs of `kind` name.
pub fn ids_of(refs: &WorkflowRefs, kind: ResourceRefKind) -> impl Iterator<Item = &String> {
    refs.values()
        .filter(move |reference| reference.kind == kind)
        .map(|reference| &reference.id)
}

/// Source id → imported id, per kind, for [`remap`]. `skills: None` checks
/// skill refs without rewriting them (their ids are known only once the
/// skills are written).
pub struct RefRemaps<'a> {
    pub pages: &'a HashMap<String, String>,
    pub workflows: &'a HashMap<String, String>,
    pub quick_prompts: &'a HashMap<String, String>,
    pub quick_apis: &'a HashMap<String, String>,
    pub quick_execs: &'a HashMap<String, String>,
    pub bundled_skills: &'a BTreeSet<String>,
    pub skills: Option<&'a HashMap<String, String>>,
}

/// Point every ref at its imported object. A ref to an object the bundle
/// does not carry fails: it would name an object of the exporting Kronn.
/// Builtin skills keep their id, the same everywhere.
pub fn remap(
    workflow_name: &str,
    refs: &mut WorkflowRefs,
    maps: &RefRemaps<'_>,
) -> Result<(), String> {
    for (name, reference) in refs.iter_mut() {
        let table = match reference.kind {
            ResourceRefKind::Page => maps.pages,
            ResourceRefKind::Workflow => maps.workflows,
            ResourceRefKind::QuickPrompt => maps.quick_prompts,
            ResourceRefKind::QuickApi => maps.quick_apis,
            ResourceRefKind::QuickExec => maps.quick_execs,
            ResourceRefKind::Skill => {
                if !reference.id.starts_with("custom-") {
                    continue;
                }
                if !maps.bundled_skills.contains(&reference.id) {
                    return Err(missing(workflow_name, name, reference.kind, &reference.id));
                }
                if let Some(target) = maps.skills.and_then(|skills| skills.get(&reference.id)) {
                    reference.id = target.clone();
                }
                continue;
            }
        };
        match table.get(&reference.id) {
            Some(target) => reference.id = target.clone(),
            None => return Err(missing(workflow_name, name, reference.kind, &reference.id)),
        }
    }
    Ok(())
}

fn missing(workflow: &str, name: &str, kind: ResourceRefKind, id: &str) -> String {
    format!(
        "Workflow « {workflow} »: ref `{name}` names the {} `{id}`, which the file does not carry. Export the source workflow again with this Kronn version.",
        kind.as_str()
    )
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::ResourceRef;

    fn refs(entries: &[(&str, ResourceRefKind, &str)]) -> WorkflowRefs {
        entries
            .iter()
            .map(|(name, kind, id)| {
                (
                    name.to_string(),
                    ResourceRef {
                        kind: *kind,
                        id: id.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn names_and_ids_follow_their_charset() {
        for good in ["a", "board", "page_2", &format!("a{}", "b".repeat(40))] {
            assert!(is_valid_name(good), "{good}");
        }
        for bad in [
            "",
            "A",
            "2a",
            "_a",
            "a-b",
            "a.b",
            "é",
            &format!("a{}", "b".repeat(41)),
        ] {
            assert!(!is_valid_name(bad), "{bad}");
        }
        assert!(is_safe_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_safe_id("custom-review_2"));
        for bad in [
            "",
            "-rf",
            "_a",
            "a b",
            "a;b",
            "$(x)",
            "a'b",
            "repository:p:x",
            "ü",
        ] {
            assert!(!is_safe_id(bad), "{bad}");
        }
        assert!(validate_syntax(&refs(&[("board", ResourceRefKind::Page, "p-1")])).is_ok());
        assert!(validate_syntax(&refs(&[("Board", ResourceRefKind::Page, "p-1")])).is_err());
        assert!(validate_syntax(&refs(&[("board", ResourceRefKind::Page, "p 1")])).is_err());
        assert_eq!(env_name("board_2"), "KRONN_REF_BOARD_2");
        assert_eq!(ref_name_of_path("refs.board"), Some("board"));
        assert_eq!(ref_name_of_path("refs.Bad"), None);
    }

    #[test]
    fn steps_use_only_declared_refs() {
        let step: WorkflowStep = serde_json::from_value(serde_json::json!({
            "name": "s", "step_type": {"type": "Exec"},
            "exec_command": "echo", "exec_args": ["{{refs.board}}", "{{refs.other|sh}}"]
        }))
        .unwrap();
        let declared = refs(&[("board", ResourceRefKind::Page, "p")]);
        let error = validate_usage(std::slice::from_ref(&step), &[], &declared).unwrap_err();
        assert!(error.contains("`refs.other`"), "{error}");
        let both = refs(&[
            ("board", ResourceRefKind::Page, "p"),
            ("other", ResourceRefKind::Workflow, "w"),
        ]);
        assert!(validate_usage(&[], std::slice::from_ref(&step), &both).is_ok());
    }

    #[test]
    fn remap_rewrites_bundled_ids_and_refuses_missing_ones() {
        let pages = HashMap::from([("p-old".to_string(), "p-new".to_string())]);
        let empty = HashMap::new();
        let skills = HashMap::from([("custom-a".to_string(), "custom-a-2".to_string())]);
        let bundled = BTreeSet::from(["custom-a".to_string()]);
        let maps = RefRemaps {
            pages: &pages,
            workflows: &empty,
            quick_prompts: &empty,
            quick_apis: &empty,
            quick_execs: &empty,
            bundled_skills: &bundled,
            skills: Some(&skills),
        };
        let mut ok = refs(&[
            ("board", ResourceRefKind::Page, "p-old"),
            ("skill", ResourceRefKind::Skill, "custom-a"),
            ("builtin", ResourceRefKind::Skill, "rust"),
        ]);
        remap("wf", &mut ok, &maps).unwrap();
        assert_eq!(ok["board"].id, "p-new");
        assert_eq!(ok["skill"].id, "custom-a-2");
        assert_eq!(ok["builtin"].id, "rust");

        for (kind, id) in [
            (ResourceRefKind::Workflow, "w-old"),
            (ResourceRefKind::Skill, "custom-gone"),
        ] {
            let mut bad = refs(&[("x", kind, id)]);
            let error = remap("Nightly", &mut bad, &maps).unwrap_err();
            assert!(error.contains("Nightly") && error.contains(id), "{error}");
        }
    }
}
