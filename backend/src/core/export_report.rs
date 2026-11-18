//! KT-1843 — what an export names and does not carry: a file meant to be
//! handed to someone else is complete, or says exactly what it leaves out.
use std::collections::{BTreeMap, BTreeSet};

use crate::models::{QuickApi, QuickPrompt, Workflow, WorkflowStep};

pub use crate::models::NotCarried;

/// The ids of this Kronn's saved objects an id pasted in a step can name.
#[derive(Debug, Default)]
pub struct KnownIds(BTreeMap<String, &'static str>);

impl KnownIds {
    /// Reads ids only: an export must not fail on a row another reader
    /// would refuse (a page whose first revision is not written yet).
    pub fn load(conn: &rusqlite::Connection) -> anyhow::Result<Self> {
        let mut known = BTreeMap::new();
        for (table, kind) in [
            ("live_pages", "page"),
            ("workflows", "workflow"),
            ("quick_prompts", "quick_prompt"),
            ("quick_apis", "quick_api"),
            ("quick_execs", "quick_exec"),
        ] {
            let mut rows = conn.prepare(&format!("SELECT id FROM {table}"))?;
            for id in rows.query_map([], |row| row.get::<_, String>(0))? {
                known.insert(id?, kind);
            }
        }
        Ok(Self(known))
    }

    #[cfg(test)]
    pub fn of(ids: &[(&str, &'static str)]) -> Self {
        Self(
            ids.iter()
                .map(|(id, kind)| (id.to_string(), *kind))
                .collect(),
        )
    }
}

/// What the file holds: the bundled skills, and every other bundled object
/// (the exported root included).
#[derive(Debug, Default)]
pub struct Carried {
    pub skills: BTreeSet<String>,
    pub directives: BTreeSet<String>,
    pub profiles: BTreeSet<String>,
    pub objects: BTreeSet<String>,
}

/// Every `8-4-4-4-12` hexadecimal id written in `text`.
fn uuids_in(text: &str) -> Vec<&str> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let bytes = text.as_bytes();
    let hex = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_hexdigit);
    let mut found = Vec::new();
    let mut start = 0;
    while start + 36 <= bytes.len() {
        let mut at = start;
        let whole = GROUPS.iter().enumerate().all(|(index, len)| {
            let digits = (at..at + len).all(hex);
            at += len;
            let dash = index == 4 || bytes.get(at) == Some(&b'-');
            at += 1;
            digits && dash
        });
        // An id inside a longer run of hex digits is another value.
        if whole && !(start > 0 && hex(start - 1)) && !hex(start + 36) {
            found.push(&text[start..start + 36]);
            start += 36;
        } else {
            start += 1;
        }
    }
    found
}

fn custom_directive(id: &str) -> bool {
    crate::core::directives::get_directive(id).is_none_or(|directive| !directive.is_builtin)
}

fn custom_profile(id: &str) -> bool {
    crate::core::profiles::get_profile(id).is_none_or(|profile| !profile.is_builtin)
}

struct Report<'a> {
    carried: &'a Carried,
    found: BTreeSet<NotCarried>,
}

impl Report<'_> {
    fn add(&mut self, kind: &str, id: &str, named_in: &str) {
        self.found.insert(NotCarried {
            kind: kind.to_string(),
            id: id.to_string(),
            named_in: named_in.to_string(),
        });
    }

    fn skill(&mut self, id: &str, named_in: &str) {
        if id.starts_with("custom-") && !self.carried.skills.contains(id) {
            self.add("skill", id, named_in);
        }
    }

    fn bindings(&mut self, profiles: &[String], directives: &[String], named_in: &str) {
        for id in profiles.iter().filter(|id| custom_profile(id)) {
            if !self.carried.profiles.contains(id) {
                self.add("profile", id, named_in);
            }
        }
        for id in directives.iter().filter(|id| custom_directive(id)) {
            if !self.carried.directives.contains(id) {
                self.add("directive", id, named_in);
            }
        }
    }

    fn step(&mut self, step: &WorkflowStep, named_in: &str, known: &KnownIds) {
        for id in &step.skill_ids {
            self.skill(id, named_in);
        }
        for shadow in &step.shadows {
            for id in shadow.skills.values().filter(|id| !id.is_empty()) {
                self.skill(id, named_in);
            }
        }
        self.bindings(&step.profile_ids, &step.directive_ids, named_in);
        let Ok(value) = serde_json::to_value(step) else {
            return;
        };
        let mut texts = Vec::new();
        crate::workflows::refs::collect_texts(&value, &mut texts);
        for id in texts.into_iter().flat_map(uuids_in) {
            let id = id.to_ascii_lowercase();
            if let Some(kind) = known.0.get(&id) {
                if !self.carried.objects.contains(&id) {
                    self.add(kind, &id, named_in);
                }
            }
        }
    }
}

/// What `workflows`, `quick_prompts` and `quick_apis` (the exported ones,
/// bundled included) name that the file does not hold: a custom skill,
/// directive or profile that no longer exists, and the id of a saved Page,
/// workflow or Quick object pasted in a step's text. Whole known ids only.
pub fn not_carried(
    workflows: &[&Workflow],
    quick_prompts: &[&QuickPrompt],
    quick_apis: &[&QuickApi],
    carried: &Carried,
    known: &KnownIds,
) -> Vec<NotCarried> {
    let mut report = Report {
        carried,
        found: BTreeSet::new(),
    };
    for workflow in workflows {
        for id in
            crate::workflows::refs::ids_of(&workflow.refs, crate::models::ResourceRefKind::Skill)
        {
            report.skill(id, &workflow.name);
        }
        for step in workflow.steps.iter().chain(&workflow.on_failure) {
            report.step(step, &format!("{} › {}", workflow.name, step.name), known);
        }
    }
    for prompt in quick_prompts {
        for id in &prompt.skill_ids {
            report.skill(id, &prompt.name);
        }
        report.bindings(&prompt.profile_ids, &prompt.directive_ids, &prompt.name);
    }
    for api in quick_apis {
        report.bindings(&api.profile_ids, &api.directive_ids, &api.name);
    }
    report.found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ids_are_read_from_a_page_the_page_reader_refuses() {
        let db = crate::db::Database::open_in_memory().unwrap();
        let known = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO live_pages(id, project_id, title, slug, created_at, updated_at) \
                     VALUES ('page-1', NULL, 'P', 'p', '2026-11-18T00:00:00Z', '2026-11-18T00:00:00Z')",
                    [],
                )?;
                assert!(
                    crate::db::live_pages::list_live_pages(conn).is_err(),
                    "the fixture is a page without a revision"
                );
                KnownIds::load(conn)
            })
            .await
            .unwrap();
        assert_eq!(known.0.get("page-1"), Some(&"page"));
    }

    #[test]
    fn uuids_are_found_whole_wherever_a_text_writes_them() {
        let id = "4339ea95-1c2d-4e5f-8a9b-0123456789ab";
        assert_eq!(uuids_in(&format!("WF = '{id}'")), [id]);
        assert_eq!(uuids_in(&format!("{id},{id}")), [id, id]);
        assert_eq!(uuids_in(&format!("/api/pages/{id}/data")), [id]);
        // A longer hexadecimal run, a truncated id, no id at all.
        assert!(uuids_in(&format!("a{id}")).is_empty());
        assert!(uuids_in(&format!("{id}0")).is_empty());
        assert!(uuids_in(&id[..35]).is_empty());
        assert!(uuids_in("4339ea95_1c2d_4e5f_8a9b_0123456789ab").is_empty());
        assert!(uuids_in("").is_empty());
    }

    const OTHER: &str = "4339ea95-1c2d-4e5f-8a9b-0123456789ab";
    const PAGE: &str = "0b6f6a52-5a0f-4a43-9c2e-0f5e6c0f7a11";

    fn workflow(extra: serde_json::Value) -> Workflow {
        let mut value = serde_json::json!({
            "id": "wf-1", "name": "Review", "project_id": null,
            "trigger": {"type": "Manual"}, "steps": [], "actions": [],
            "safety": {"sandbox": false, "max_files": null, "max_lines": null,
                       "require_approval": false},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        });
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }

    fn prompt(extra: serde_json::Value) -> QuickPrompt {
        let mut value = serde_json::json!({
            "id": "qp-1", "name": "Daily", "icon": "x", "prompt_template": "Go",
            "variables": [], "agent": "ClaudeCode", "project_id": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        });
        for (key, field) in extra.as_object().unwrap() {
            value[key] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }

    fn item(kind: &str, id: &str, named_in: &str) -> NotCarried {
        NotCarried {
            kind: kind.into(),
            id: id.into(),
            named_in: named_in.into(),
        }
    }

    fn carried(skills: &[&str], objects: &[&str]) -> Carried {
        Carried {
            skills: skills.iter().map(|id| id.to_string()).collect(),
            objects: objects.iter().map(|id| id.to_string()).collect(),
            ..Carried::default()
        }
    }

    #[test]
    fn a_custom_skill_the_file_does_not_hold_is_reported_wherever_it_is_bound() {
        let wf = workflow(serde_json::json!({
            "refs": {"rubric": {"kind": "skill", "id": "custom-ref"}},
            "steps": [{"name": "review", "skill_ids": ["custom-gone", "custom-kept", "rust"],
                       "shadows": [{"id": "s1", "skills": {"custom-kept": "custom-shadow", "rust": ""}}]}],
            "on_failure": [{"name": "alert", "skill_ids": ["custom-late"]}],
        }));
        let qp = prompt(serde_json::json!({"skill_ids": ["custom-qp", "custom-kept"]}));
        let found = not_carried(
            &[&wf],
            &[&qp],
            &[],
            &carried(&["custom-kept"], &[]),
            &KnownIds::default(),
        );
        assert_eq!(
            found,
            [
                item("skill", "custom-gone", "Review › review"),
                item("skill", "custom-late", "Review › alert"),
                item("skill", "custom-qp", "Daily"),
                item("skill", "custom-ref", "Review"),
                item("skill", "custom-shadow", "Review › review"),
            ]
        );
    }

    #[test]
    fn custom_directives_and_profiles_are_reported_and_builtin_ones_are_not() {
        let wf = workflow(serde_json::json!({
            "steps": [{"name": "review",
                       "directive_ids": ["token-saver", "custom-tone"],
                       "profile_ids": ["architect", "custom-lead"]}],
        }));
        let qp = prompt(serde_json::json!({
            "directive_ids": ["json-output", "custom-table"],
            "profile_ids": ["tech-lead", "custom-pm"],
        }));
        let found = not_carried(
            &[&wf],
            &[&qp],
            &[],
            &Carried::default(),
            &KnownIds::default(),
        );
        assert_eq!(
            found,
            [
                item("directive", "custom-table", "Daily"),
                item("directive", "custom-tone", "Review › review"),
                item("profile", "custom-lead", "Review › review"),
                item("profile", "custom-pm", "Daily"),
            ]
        );

        // A Quick API binds them too, and what the file holds is not reported.
        let api: QuickApi = serde_json::from_value(serde_json::json!({
            "id": "qa-1", "name": "Metrics", "icon": "x", "description": "",
            "project_id": null, "api_plugin_slug": "x", "api_config_id": "c",
            "api_endpoint_path": "/", "variables": [],
            "directive_ids": ["custom-api", "custom-tone"], "profile_ids": ["custom-ops"],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        let held = Carried {
            directives: ["custom-tone", "custom-table"].map(String::from).into(),
            profiles: ["custom-lead", "custom-ops"].map(String::from).into(),
            ..Carried::default()
        };
        assert_eq!(
            not_carried(&[&wf], &[&qp], &[&api], &held, &KnownIds::default()),
            [
                item("directive", "custom-api", "Metrics"),
                item("profile", "custom-pm", "Daily"),
            ]
        );
    }

    #[test]
    fn a_saved_object_a_step_text_names_is_reported_unless_the_file_holds_it() {
        let upper = OTHER.to_ascii_uppercase();
        let wf = workflow(serde_json::json!({
            "steps": [
                {"name": "ctx", "prompt_template": format!("Trigger {upper} then read {PAGE}")},
                {"name": "fail", "prompt_template": "Unknown 00000000-0000-4000-8000-000000000000"},
            ],
            "on_failure": [{"name": "alert", "prompt_template": format!("See {PAGE}")}],
        }));
        let known = KnownIds::of(&[(OTHER, "workflow"), (PAGE, "page")]);
        assert_eq!(
            not_carried(&[&wf], &[], &[], &Carried::default(), &known),
            [
                item("page", PAGE, "Review › alert"),
                item("page", PAGE, "Review › ctx"),
                item("workflow", OTHER, "Review › ctx"),
            ]
        );
        assert_eq!(
            not_carried(&[&wf], &[], &[], &carried(&[], &[PAGE]), &known),
            [item("workflow", OTHER, "Review › ctx")]
        );
        assert!(not_carried(&[&wf], &[], &[], &carried(&[], &[PAGE, OTHER]), &known).is_empty());
    }
}
