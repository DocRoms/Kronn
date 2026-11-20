//! KT-1843 — custom directives and profiles travel with an export, and an
//! import reuses the local one that says the same thing instead of copying it.
use std::collections::{BTreeSet, HashMap};

use crate::models::{
    AgentProfile, BindingImportDecision, BindingKind, Directive, QuickApi, QuickPrompt,
    SkillImportAction, Workflow,
};

use super::{directives, profiles};

const MAX_BUNDLED: usize = 200;
const MAX_LINE_BYTES: usize = 512;
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// The custom directives among `ids` that exist here, ordered by id.
pub fn bundle_directives<'a>(ids: impl IntoIterator<Item = &'a String>) -> Vec<Directive> {
    let wanted: BTreeSet<&String> = ids.into_iter().collect();
    let mut bundled: Vec<Directive> = directives::list_all_directives()
        .into_iter()
        .filter(|directive| !directive.is_builtin && wanted.contains(&directive.id))
        .collect();
    bundled.sort_by(|a, b| a.id.cmp(&b.id));
    bundled
}

/// The custom profiles among `ids` that exist here, ordered by id.
pub fn bundle_profiles<'a>(ids: impl IntoIterator<Item = &'a String>) -> Vec<AgentProfile> {
    let wanted: BTreeSet<&String> = ids.into_iter().collect();
    let mut bundled: Vec<AgentProfile> = profiles::list_all_profiles()
        .into_iter()
        .filter(|profile| !profile.is_builtin && wanted.contains(&profile.id))
        .collect();
    bundled.sort_by(|a, b| a.id.cmp(&b.id));
    bundled
}

/// The custom directives and profiles the exported workflows, Quick Prompts
/// and Quick APIs bind, for the file.
pub fn bundle_for(
    workflows: &[&Workflow],
    quick_prompts: &[&QuickPrompt],
    quick_apis: &[&QuickApi],
) -> (Vec<Directive>, Vec<AgentProfile>) {
    let steps = workflows
        .iter()
        .flat_map(|workflow| workflow.steps.iter().chain(&workflow.on_failure));
    let directive_ids = steps
        .clone()
        .flat_map(|step| &step.directive_ids)
        .chain(
            quick_prompts
                .iter()
                .flat_map(|prompt| &prompt.directive_ids),
        )
        .chain(quick_apis.iter().flat_map(|api| &api.directive_ids));
    let profile_ids = steps
        .flat_map(|step| &step.profile_ids)
        .chain(quick_prompts.iter().flat_map(|prompt| &prompt.profile_ids))
        .chain(quick_apis.iter().flat_map(|api| &api.profile_ids));
    (
        bundle_directives(directive_ids),
        bundle_profiles(profile_ids),
    )
}

/// Two directives that tell an agent the same thing, whatever they are called.
fn same_directive(a: &Directive, b: &Directive) -> bool {
    a.category == b.category && a.content.trim() == b.content.trim()
}

/// Two profiles that give an agent the same persona, whatever they are called.
fn same_profile(a: &AgentProfile, b: &AgentProfile) -> bool {
    a.category == b.category
        && a.role.trim() == b.role.trim()
        && a.persona_prompt.trim() == b.persona_prompt.trim()
        && a.default_engine == b.default_engine
}

/// A frontmatter value: one line, so it cannot add a key of its own.
fn line(kind: &str, id: &str, field: &str, value: &str, required: bool) -> Result<(), String> {
    let invalid = value.len() > MAX_LINE_BYTES
        || value.chars().any(char::is_control)
        || (required && value.trim().is_empty());
    if invalid {
        return Err(format!("Bundled {kind} `{id}` has an invalid {field}"));
    }
    Ok(())
}

fn text(kind: &str, id: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "Bundled {kind} `{id}` is empty or exceeds {} KiB",
            MAX_TEXT_BYTES / 1024
        ));
    }
    Ok(())
}

fn source_ids<'a>(kind: &str, ids: impl Iterator<Item = &'a String>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !id.starts_with("custom-") || id.len() > 128 || id.chars().any(char::is_control) {
            return Err(format!("A bundled {kind} has an invalid id"));
        }
        if !seen.insert(id) {
            return Err(format!("Bundled {kind} `{id}` appears twice"));
        }
    }
    if seen.len() > MAX_BUNDLED {
        return Err(format!("More than {MAX_BUNDLED} bundled {kind}s"));
    }
    Ok(())
}

fn check(
    bundled_directives: &[Directive],
    bundled_profiles: &[AgentProfile],
) -> Result<(), String> {
    source_ids("directive", bundled_directives.iter().map(|d| &d.id))?;
    source_ids("profile", bundled_profiles.iter().map(|p| &p.id))?;
    for directive in bundled_directives {
        let id = &directive.id;
        line("directive", id, "name", &directive.name, true)?;
        line(
            "directive",
            id,
            "description",
            &directive.description,
            false,
        )?;
        line("directive", id, "icon", &directive.icon, false)?;
        text("directive", id, &directive.content)?;
        for conflict in &directive.conflicts {
            let listed = !conflict.is_empty()
                && conflict
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !listed || conflict.len() > 128 {
                return Err(format!("Bundled directive `{id}` has an invalid conflict"));
            }
        }
    }
    for profile in bundled_profiles {
        let id = &profile.id;
        line("profile", id, "name", &profile.name, true)?;
        line("profile", id, "persona name", &profile.persona_name, false)?;
        line("profile", id, "role", &profile.role, false)?;
        line("profile", id, "avatar", &profile.avatar, false)?;
        line("profile", id, "color", &profile.color, false)?;
        if profile.color.contains('"') {
            return Err(format!("Bundled profile `{id}` has an invalid color"));
        }
        if let Some(engine) = &profile.default_engine {
            line("profile", id, "default engine", engine, true)?;
        }
        text("profile", id, &profile.persona_prompt)?;
    }
    Ok(())
}

/// The local custom entry `same` as the bundled one: its own id first, then
/// the smallest, so two imports of one file settle on the same entry.
fn local_match<'a, T>(
    local: &'a [T],
    source_id: &str,
    id: impl Fn(&T) -> &String,
    same: impl Fn(&T) -> bool,
) -> Option<&'a String> {
    local
        .iter()
        .filter(|entry| same(entry))
        .map(id)
        .min_by_key(|id| (id.as_str() != source_id, id.as_str()))
}

fn custom_directives() -> Vec<Directive> {
    directives::list_all_directives()
        .into_iter()
        .filter(|directive| !directive.is_builtin)
        .collect()
}

fn custom_profiles() -> Vec<AgentProfile> {
    profiles::list_all_profiles()
        .into_iter()
        .filter(|profile| !profile.is_builtin)
        .collect()
}

/// What an import does with each bundled directive and profile: reuse the
/// local custom one that says the same thing, else create it. Writes nothing.
pub fn plan(
    bundled_directives: &[Directive],
    bundled_profiles: &[AgentProfile],
) -> Result<Vec<BindingImportDecision>, String> {
    check(bundled_directives, bundled_profiles)?;
    let local_directives = custom_directives();
    let local_profiles = custom_profiles();
    let decision = |kind, source_id: &String, name: &String, existing: Option<&String>| {
        BindingImportDecision {
            kind,
            source_id: source_id.clone(),
            name: name.clone(),
            action: match existing {
                Some(_) => SkillImportAction::Reuse,
                None => SkillImportAction::Create,
            },
            existing_id: existing.cloned(),
        }
    };
    let mut decisions = Vec::new();
    for (index, directive) in bundled_directives.iter().enumerate() {
        // The file may hold the same directive under two ids: one copy.
        let earlier = bundled_directives[..index]
            .iter()
            .find(|earlier| same_directive(earlier, directive));
        let existing = local_match(
            &local_directives,
            &directive.id,
            |d| &d.id,
            |d| same_directive(d, directive),
        );
        let mut entry = decision(
            BindingKind::Directive,
            &directive.id,
            &directive.name,
            existing,
        );
        if existing.is_none() && earlier.is_some() {
            entry.action = SkillImportAction::Reuse;
        }
        decisions.push(entry);
    }
    for (index, profile) in bundled_profiles.iter().enumerate() {
        let earlier = bundled_profiles[..index]
            .iter()
            .find(|earlier| same_profile(earlier, profile));
        let existing = local_match(
            &local_profiles,
            &profile.id,
            |p| &p.id,
            |p| same_profile(p, profile),
        );
        let mut entry = decision(BindingKind::Profile, &profile.id, &profile.name, existing);
        if existing.is_none() && earlier.is_some() {
            entry.action = SkillImportAction::Reuse;
        }
        decisions.push(entry);
    }
    Ok(decisions)
}

/// Source id → local id of every bundled directive and profile.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BindingIds {
    pub directives: HashMap<String, String>,
    pub profiles: HashMap<String, String>,
}

impl BindingIds {
    /// Rewrites the bundled ids a step or a Quick object binds; others stay.
    pub fn remap(&self, profile_ids: &mut [String], directive_ids: &mut [String]) {
        for id in profile_ids {
            if let Some(target) = self.profiles.get(id) {
                *id = target.clone();
            }
        }
        for id in directive_ids {
            if let Some(target) = self.directives.get(id) {
                *id = target.clone();
            }
        }
    }
}

/// The ids a preview can tell: those of the local entries reused.
pub fn predicted_ids(decisions: &[BindingImportDecision]) -> BindingIds {
    let mut ids = BindingIds::default();
    for decision in decisions {
        let Some(existing) = &decision.existing_id else {
            continue;
        };
        let map = match decision.kind {
            BindingKind::Directive => &mut ids.directives,
            BindingKind::Profile => &mut ids.profiles,
        };
        map.insert(decision.source_id.clone(), existing.clone());
    }
    ids
}

/// A display name no entry uses yet: `name`, else `name (2)`, `name (3)`…
fn free_name(name: &str, taken: &BTreeSet<String>) -> String {
    let name = name.trim();
    if !taken.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .expect("an unbounded range always yields a free name")
}

/// Reuse or create every bundled directive and profile, as [`plan`] says.
/// They are files, outside any database transaction: one created before a
/// later failure stays, and the next import of the same file reuses it.
pub fn apply(
    bundled_directives: &[Directive],
    bundled_profiles: &[AgentProfile],
) -> Result<BindingIds, String> {
    check(bundled_directives, bundled_profiles)?;
    let mut ids = BindingIds::default();
    let mut created = Vec::new();
    for directive in bundled_directives {
        // Read again each time: the entry just created counts.
        let all = directives::list_all_directives();
        let local: Vec<Directive> = all.iter().filter(|d| !d.is_builtin).cloned().collect();
        let existing = local_match(
            &local,
            &directive.id,
            |d| &d.id,
            |d| same_directive(d, directive),
        );
        let target = match existing {
            Some(existing) => existing.clone(),
            None => {
                let taken = all.iter().map(|d| d.name.trim().to_lowercase()).collect();
                let builtin_conflicts: Vec<String> = directive
                    .conflicts
                    .iter()
                    .filter(|id| !id.starts_with("custom-"))
                    .cloned()
                    .collect();
                let id = directives::save_custom_directive(
                    &free_name(&directive.name, &taken),
                    &directive.description,
                    &directive.icon,
                    &directive.category,
                    directive.content.trim(),
                    &builtin_conflicts,
                )?;
                created.push((id.clone(), directive));
                id
            }
        };
        ids.directives.insert(directive.id.clone(), target);
    }
    // A conflict with another bundled directive names its local id.
    for (id, source) in created {
        let conflicts: Vec<String> = source
            .conflicts
            .iter()
            .filter_map(|conflict| match conflict.starts_with("custom-") {
                true => ids.directives.get(conflict).cloned(),
                false => Some(conflict.clone()),
            })
            .collect();
        if conflicts.len()
            == source
                .conflicts
                .iter()
                .filter(|c| !c.starts_with("custom-"))
                .count()
        {
            continue;
        }
        let Some(saved) = directives::get_directive(&id) else {
            continue;
        };
        directives::update_custom_directive(
            &id,
            &saved.name,
            &saved.description,
            &saved.icon,
            &saved.category,
            &saved.content,
            &conflicts,
        )?;
    }
    for profile in bundled_profiles {
        let all = profiles::list_all_profiles();
        let local: Vec<AgentProfile> = all.iter().filter(|p| !p.is_builtin).cloned().collect();
        let existing = local_match(&local, &profile.id, |p| &p.id, |p| same_profile(p, profile));
        let target = match existing {
            Some(existing) => existing.clone(),
            None => {
                let taken = all.iter().map(|p| p.name.trim().to_lowercase()).collect();
                profiles::save_custom_profile(&profiles::CustomProfileData {
                    name: &free_name(&profile.name, &taken),
                    persona_name: &profile.persona_name,
                    role: &profile.role,
                    avatar: &profile.avatar,
                    color: &profile.color,
                    category: &profile.category,
                    persona_prompt: profile.persona_prompt.trim(),
                    default_engine: profile.default_engine.as_deref(),
                })?
            }
        };
        ids.profiles.insert(profile.id.clone(), target);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DirectiveCategory, ProfileCategory};
    use serial_test::serial;

    /// A scratch data directory for the test, put back on drop.
    struct Scratch {
        _dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl Scratch {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
            crate::core::child_env::set_var("KRONN_DATA_DIR", dir.path());
            Self {
                _dir: dir,
                previous,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => crate::core::child_env::set_var("KRONN_DATA_DIR", value),
                None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
            }
        }
    }

    fn directive(id: &str, name: &str, content: &str) -> Directive {
        Directive {
            id: id.into(),
            name: name.into(),
            description: "How to answer".into(),
            icon: "T".into(),
            category: DirectiveCategory::Output,
            content: content.into(),
            is_builtin: false,
            conflicts: vec![],
            token_estimate: 0,
            source_url: None,
        }
    }

    fn profile(id: &str, name: &str, prompt: &str) -> AgentProfile {
        AgentProfile {
            id: id.into(),
            name: name.into(),
            persona_name: "Cap".into(),
            role: "Ships".into(),
            avatar: "C".into(),
            color: "#123456".into(),
            category: ProfileCategory::Technical,
            persona_prompt: prompt.into(),
            default_engine: None,
            is_builtin: false,
            token_estimate: 0,
        }
    }

    fn local_directives() -> Vec<(String, String, String)> {
        let mut found: Vec<_> = custom_directives()
            .into_iter()
            .map(|d| (d.id, d.name, d.content))
            .collect();
        found.sort();
        found
    }

    fn actions(decisions: &[BindingImportDecision]) -> Vec<(SkillImportAction, Option<String>)> {
        decisions
            .iter()
            .map(|d| (d.action, d.existing_id.clone()))
            .collect()
    }

    #[test]
    #[serial]
    fn a_directive_is_created_once_and_reused_whatever_it_is_called_afterwards() {
        let _scratch = Scratch::new();
        let bundled = [directive(
            "custom-house",
            "House style",
            "Answer in a table.",
        )];
        assert_eq!(
            actions(&plan(&bundled, &[]).unwrap()),
            [(SkillImportAction::Create, None)]
        );
        assert!(local_directives().is_empty(), "a plan writes nothing");

        let first = apply(&bundled, &[]).unwrap();
        let local = first.directives["custom-house"].clone();
        assert_eq!(
            local_directives(),
            [(
                local.clone(),
                "House style".into(),
                "Answer in a table.".into()
            )]
        );

        // The same file again, a renamed source, a renamed local copy.
        assert_eq!(
            actions(&plan(&bundled, &[]).unwrap()),
            [(SkillImportAction::Reuse, Some(local.clone()))]
        );
        assert_eq!(apply(&bundled, &[]).unwrap(), first);
        let renamed = [directive(
            "custom-other-id",
            "Tables",
            " Answer in a table.\n",
        )];
        assert_eq!(
            apply(&renamed, &[]).unwrap().directives["custom-other-id"],
            local
        );
        directives::update_custom_directive(
            &local,
            "Our style",
            "",
            "T",
            &DirectiveCategory::Output,
            "Answer in a table.",
            &[],
        )
        .unwrap();
        assert_eq!(apply(&bundled, &[]).unwrap(), first);
        assert_eq!(local_directives().len(), 1, "never a second copy");
    }

    #[test]
    #[serial]
    fn a_directive_that_says_something_else_is_a_new_one_under_a_free_name() {
        let _scratch = Scratch::new();
        apply(
            &[directive(
                "custom-house",
                "House style",
                "Answer in a table.",
            )],
            &[],
        )
        .unwrap();
        let other = [
            directive("custom-house", "House style", "Answer in prose."),
            // Another category is another directive, even with the same text.
            Directive {
                category: DirectiveCategory::Language,
                ..directive("custom-lang", "House style", "Answer in a table.")
            },
        ];
        let ids = apply(&other, &[]).unwrap();
        let names: Vec<String> = local_directives()
            .into_iter()
            .map(|(_, name, _)| name)
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        for name in ["House style", "House style (2)", "House style (3)"] {
            assert!(names.iter().any(|n| n == name), "{name} missing: {names:?}");
        }
        // Each import of that file lands on the copies it made.
        assert_eq!(apply(&other, &[]).unwrap(), ids);
        assert_eq!(local_directives().len(), 3);
    }

    #[test]
    #[serial]
    fn two_bundled_entries_that_say_the_same_thing_make_one_local_entry() {
        let _scratch = Scratch::new();
        let twice = [
            directive("custom-a", "A", "Be brief."),
            directive("custom-b", "B", "Be brief."),
        ];
        assert_eq!(
            actions(&plan(&twice, &[]).unwrap()),
            [
                (SkillImportAction::Create, None),
                (SkillImportAction::Reuse, None)
            ]
        );
        let ids = apply(&twice, &[]).unwrap();
        assert_eq!(ids.directives["custom-a"], ids.directives["custom-b"]);
        assert_eq!(local_directives().len(), 1);

        let personas = [
            profile("custom-a", "A", "You ship carefully."),
            profile("custom-b", "B", "You ship carefully."),
        ];
        let ids = apply(&[], &personas).unwrap();
        assert_eq!(ids.profiles["custom-a"], ids.profiles["custom-b"]);
        assert_eq!(custom_profiles().len(), 1);
    }

    #[test]
    #[serial]
    fn a_profile_is_reused_when_its_persona_is_the_same_and_created_otherwise() {
        let _scratch = Scratch::new();
        let bundled = [profile(
            "custom-cap",
            "Release captain",
            "You ship carefully.",
        )];
        let first = apply(&[], &bundled).unwrap();
        let local = first.profiles["custom-cap"].clone();
        let saved = profiles::get_profile(&local).unwrap();
        assert_eq!(
            (
                saved.name.as_str(),
                saved.role.as_str(),
                saved.persona_prompt.as_str()
            ),
            ("Release captain", "Ships", "You ship carefully.")
        );
        assert_eq!(
            actions(&plan(&[], &bundled).unwrap()),
            [(SkillImportAction::Reuse, Some(local.clone()))]
        );
        // Another name or look is the same persona; another role, prompt,
        // category or engine is not.
        let same = AgentProfile {
            avatar: "X".into(),
            color: "#000000".into(),
            persona_name: "Zed".into(),
            ..profile("custom-x", "Captain", "You ship carefully.")
        };
        assert_eq!(apply(&[], &[same]).unwrap().profiles["custom-x"], local);
        let variants = [
            AgentProfile {
                role: "Reviews".into(),
                ..profile("custom-1", "Release captain", "You ship carefully.")
            },
            profile("custom-2", "Release captain", "You ship fast."),
            AgentProfile {
                category: ProfileCategory::Business,
                ..profile("custom-3", "Release captain", "You ship carefully.")
            },
            AgentProfile {
                default_engine: Some("codex".into()),
                ..profile("custom-4", "Release captain", "You ship carefully.")
            },
        ];
        let ids = apply(&[], &variants).unwrap();
        let distinct: BTreeSet<&String> = ids.profiles.values().collect();
        assert_eq!(distinct.len(), 4);
        assert!(!distinct.contains(&local));
        assert_eq!(custom_profiles().len(), 5);
        assert_eq!(apply(&[], &variants).unwrap(), ids);
        assert_eq!(custom_profiles().len(), 5, "never a second copy");
    }

    #[test]
    #[serial]
    fn a_conflict_with_another_bundled_directive_names_its_local_copy() {
        let _scratch = Scratch::new();
        // The local Kronn already uses the source's slug for something else.
        directives::save_custom_directive(
            "Prose",
            "",
            "T",
            &DirectiveCategory::Output,
            "Unrelated.",
            &[],
        )
        .unwrap();
        let bundled = [
            Directive {
                conflicts: vec![
                    "custom-prose".into(),
                    "json-output".into(),
                    "custom-absent".into(),
                ],
                ..directive("custom-table", "Table", "Answer in a table.")
            },
            directive("custom-prose", "Prose", "Answer in prose."),
        ];
        let ids = apply(&bundled, &[]).unwrap();
        let prose = ids.directives["custom-prose"].clone();
        assert_ne!(prose, "custom-prose", "the slug was taken");
        let table = directives::get_directive(&ids.directives["custom-table"]).unwrap();
        assert_eq!(table.conflicts, [prose, "json-output".to_string()]);
        assert_eq!(table.content, "Answer in a table.");
    }

    #[test]
    #[serial]
    fn a_file_that_could_write_another_frontmatter_key_or_an_empty_entry_is_refused() {
        let _scratch = Scratch::new();
        // Refused by the plan and by the import alike, or accepted by both.
        let d = |change: fn(&mut Directive)| {
            let mut entry = directive("custom-x", "X", "Body.");
            change(&mut entry);
            let planned = plan(std::slice::from_ref(&entry), &[]).is_ok();
            assert_eq!(apply(&[entry], &[]).is_ok(), planned);
            planned
        };
        let p = |change: fn(&mut AgentProfile)| {
            let mut entry = profile("custom-x", "X", "Body.");
            change(&mut entry);
            let planned = plan(&[], std::slice::from_ref(&entry)).is_ok();
            assert_eq!(apply(&[], &[entry]).is_ok(), planned);
            planned
        };
        assert!(d(|_| {}));
        assert!(!d(|e| e.name = "X\nbuiltin: true".into()));
        assert!(!d(|e| e.name = "  ".into()));
        assert!(!d(|e| e.description = "a\rb".into()));
        assert!(!d(|e| e.icon = "a\nb".into()));
        assert!(!d(|e| e.content = " \n".into()));
        assert!(!d(|e| e.content = "x".repeat(MAX_TEXT_BYTES + 1)));
        assert!(!d(|e| e.conflicts = vec!["a]\nname: z".into()]));
        assert!(!d(|e| e.id = "token-saver".into()));
        assert!(!d(|e| e.id = format!("custom-{}", "x".repeat(130))));
        assert!(p(|_| {}));
        assert!(!p(|e| e.name = "X\nrole: root".into()));
        assert!(!p(|e| e.role = "a\nb".into()));
        assert!(!p(|e| e.persona_name = "a\nb".into()));
        assert!(!p(|e| e.avatar = "a\nb".into()));
        assert!(!p(|e| e.color = "\"\nname: z".into()));
        assert!(!p(|e| e.color = "a\"b".into()));
        assert!(!p(|e| e.default_engine = Some("a\nb".into())));
        assert!(!p(|e| e.persona_prompt = "".into()));
        assert!(!p(|e| e.id = "architect".into()));
        // Nothing but the two valid entries was written.
        assert_eq!(local_directives().len(), 1);
        assert_eq!(custom_profiles().len(), 1);

        let twice = [
            directive("custom-x", "X", "One."),
            directive("custom-x", "Y", "Two."),
        ];
        assert!(plan(&twice, &[]).is_err());
        let many: Vec<Directive> = (0..=MAX_BUNDLED)
            .map(|n| directive(&format!("custom-{n}"), "N", "Body."))
            .collect();
        assert!(plan(&many, &[]).is_err());
    }

    #[test]
    #[serial]
    fn the_file_carries_the_custom_entries_the_exported_objects_bind_and_only_those() {
        let _scratch = Scratch::new();
        let ids = apply(
            &[
                directive("custom-step", "Step", "One."),
                directive("custom-failure", "Failure", "Two."),
                directive("custom-prompt", "Prompt", "Three."),
                directive("custom-api", "Api", "Four."),
                directive("custom-unbound", "Unbound", "Five."),
            ],
            &[
                profile("custom-step", "Step", "One."),
                profile("custom-prompt", "Prompt", "Two."),
                profile("custom-api", "Api", "Three."),
                profile("custom-unbound", "Unbound", "Four."),
            ],
        )
        .unwrap();
        let workflow: Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-1", "name": "Review", "project_id": null,
            "trigger": {"type": "Manual"}, "actions": [],
            "safety": {"sandbox": false, "max_files": null, "max_lines": null,
                       "require_approval": false},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "steps": [{"name": "review",
                       "directive_ids": ["custom-step", "token-saver", "custom-deleted"],
                       "profile_ids": ["custom-step", "architect"]}],
            "on_failure": [{"name": "alert", "directive_ids": ["custom-failure"]}],
        }))
        .unwrap();
        let prompt: QuickPrompt = serde_json::from_value(serde_json::json!({
            "id": "qp-1", "name": "Daily", "icon": "x", "prompt_template": "Go",
            "variables": [], "agent": "ClaudeCode", "project_id": null,
            "directive_ids": ["custom-prompt"], "profile_ids": ["custom-prompt"],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        let mut api: QuickApi = serde_json::from_value(serde_json::json!({
            "id": "qa-1", "name": "Api", "icon": "x", "description": "", "project_id": null,
            "api_plugin_slug": "x", "api_config_id": "c", "api_endpoint_path": "/",
            "variables": [],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        api.directive_ids = vec!["custom-api".into()];
        api.profile_ids = vec!["custom-api".into()];
        assert_eq!(ids.directives["custom-step"], "custom-step");

        let (found_directives, found_profiles) = bundle_for(&[&workflow], &[&prompt], &[&api]);
        assert_eq!(
            found_directives
                .into_iter()
                .map(|d| d.id)
                .collect::<Vec<_>>(),
            [
                "custom-api",
                "custom-failure",
                "custom-prompt",
                "custom-step"
            ]
        );
        assert_eq!(
            found_profiles.into_iter().map(|p| p.id).collect::<Vec<_>>(),
            ["custom-api", "custom-prompt", "custom-step"]
        );
    }

    #[test]
    fn remap_rewrites_the_bundled_ids_and_leaves_the_others() {
        let ids = BindingIds {
            directives: HashMap::from([("custom-a".to_string(), "custom-a-2".to_string())]),
            profiles: HashMap::from([("custom-p".to_string(), "custom-q".to_string())]),
        };
        let mut profile_ids = vec![
            "custom-p".to_string(),
            "architect".to_string(),
            "custom-a".to_string(),
        ];
        let mut directive_ids = vec![
            "token-saver".to_string(),
            "custom-a".to_string(),
            "custom-p".to_string(),
        ];
        ids.remap(&mut profile_ids, &mut directive_ids);
        assert_eq!(profile_ids, ["custom-q", "architect", "custom-a"]);
        assert_eq!(directive_ids, ["token-saver", "custom-a-2", "custom-p"]);

        let decisions = [
            BindingImportDecision {
                kind: BindingKind::Directive,
                source_id: "custom-a".into(),
                name: "A".into(),
                action: SkillImportAction::Reuse,
                existing_id: Some("custom-a-2".into()),
            },
            BindingImportDecision {
                kind: BindingKind::Profile,
                source_id: "custom-p".into(),
                name: "P".into(),
                action: SkillImportAction::Reuse,
                existing_id: Some("custom-q".into()),
            },
            BindingImportDecision {
                kind: BindingKind::Profile,
                source_id: "custom-new".into(),
                name: "N".into(),
                action: SkillImportAction::Create,
                existing_id: None,
            },
        ];
        assert_eq!(predicted_ids(&decisions), ids);
    }
}
