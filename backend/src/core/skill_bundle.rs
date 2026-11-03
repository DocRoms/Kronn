//! Custom skills carried by workflow and Page exports (KT-1446). A bundle
//! holds each referenced custom skill's file; an import reuses an identical
//! local skill, creates a missing one, or settles a same-name conflict as the
//! importer chose. Builtin and repository skills travel as plain references.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use crate::models::{
    BundledSkill, SkillConflictResolution, SkillImportAction, SkillImportChoice,
    SkillImportDecision,
};

/// Larger than any hand-written skill; bounds what an import writes to disk.
const MAX_SKILL_BYTES: usize = 256 * 1024;

pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Indexes of the opening and closing `---` lines of the frontmatter.
fn frontmatter(lines: &[&str]) -> Option<(usize, usize)> {
    let open = lines.iter().position(|line| !line.trim().is_empty())?;
    if lines[open].trim() != "---" {
        return None;
    }
    let close = (open + 1..lines.len()).find(|&i| lines[i].trim_end().starts_with("---"))?;
    Some((open, close))
}

fn indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn entry_key(line: &str) -> Option<&str> {
    line.trim().split_once(':').map(|(key, _)| key.trim())
}

fn line_end(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// The skill file without `allowed-tools`: that list pre-approves tools in the
/// agents' native skill files, a choice an imported file never makes here.
/// Fails when the stripped file still declares tools, whatever its spelling.
pub fn without_allowed_tools(raw: &str) -> Result<String, String> {
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let stripped = match frontmatter(&lines) {
        None => raw.to_string(),
        Some((open, close)) => {
            let mut out = String::with_capacity(raw.len());
            let mut index = 0;
            while index < lines.len() {
                let line = lines[index];
                if index > open
                    && index < close
                    && !indented(line)
                    && entry_key(line) == Some("allowed-tools")
                {
                    // The key's value may continue on indented or list lines.
                    index += 1;
                    while index < close
                        && (indented(lines[index]) || lines[index].trim_start().starts_with('-'))
                    {
                        index += 1;
                    }
                    continue;
                }
                out.push_str(line);
                index += 1;
            }
            out
        }
    };
    match super::skills::parse_skill_markdown("custom-import-check", &stripped, false) {
        Some(skill) if skill.allowed_tools.is_some() => {
            Err("An imported skill cannot pre-approve tools (`allowed-tools`)".into())
        }
        _ => Ok(stripped),
    }
}

/// The skill file without its project scope: a scope names a project of the
/// exporting Kronn, meaningless elsewhere.
pub fn portable_content(raw: &str) -> String {
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let Some((open, close)) = frontmatter(&lines) else {
        return raw.to_string();
    };
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if index > open && index < close && !indented(line) && line.trim() == "metadata:" {
            let mut end = index + 1;
            let mut kept = Vec::new();
            while end < close && (lines[end].trim().is_empty() || indented(lines[end])) {
                if entry_key(lines[end]) != Some(super::skills::KRONN_PROJECT_KEY) {
                    kept.push(lines[end]);
                }
                end += 1;
            }
            if kept.iter().any(|entry| !entry.trim().is_empty()) {
                out.push(line);
                out.extend(kept);
            }
            index = end;
            continue;
        }
        out.push(line);
        index += 1;
    }
    out.concat()
}

/// `raw` (a portable file) scoped to `project_id`, or unchanged without one.
fn with_project(raw: &str, project_id: Option<&str>) -> String {
    let Some(project) = project_id.filter(|p| !p.is_empty()) else {
        return raw.to_string();
    };
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let Some((open, close)) = frontmatter(&lines) else {
        return raw.to_string();
    };
    let quoted = crate::core::agent_skill::quoted(project);
    let key = super::skills::KRONN_PROJECT_KEY;
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    if let Some(at) =
        (open + 1..close).find(|&i| !indented(lines[i]) && lines[i].trim() == "metadata:")
    {
        let end = line_end(lines[at]);
        out.insert(at + 1, format!("  {key}: {quoted}{end}"));
    } else {
        let end = line_end(lines[close - 1]);
        out.insert(close, format!("metadata:{end}  {key}: {quoted}{end}"));
    }
    out.concat()
}

/// `raw` with its display name set to `name`, when the file says it in a way
/// this can rewrite; checked by parsing the result.
fn renamed(raw: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let (open, close) = frontmatter(&lines)?;
    let quoted = crate::core::agent_skill::quoted(name);
    let kronn_name = super::skills::KRONN_NAME_KEY;
    let at = (open + 1..close)
        .find(|&i| indented(lines[i]) && entry_key(lines[i]) == Some(kronn_name))
        .map(|i| (i, format!("  {kronn_name}: {quoted}{}", line_end(lines[i]))))
        .or_else(|| {
            (open + 1..close)
                .find(|&i| !indented(lines[i]) && entry_key(lines[i]) == Some("name"))
                .map(|i| (i, format!("name: {quoted}{}", line_end(lines[i]))))
        })?;
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    out[at.0] = at.1;
    let result = out.concat();
    let parsed = super::skills::parse_skill_markdown("custom-copy", &result, false)?;
    (parsed.name == name).then_some(result)
}

/// The bundle entry of custom skill `id`, `None` for any other skill or a
/// custom skill that no longer exists.
pub fn bundle_skill(id: &str) -> Option<BundledSkill> {
    // A pinned version travels as its own file, named apart from the current
    // one so an import holding both never mistakes one for the other.
    let pinned = super::skill_versions::split_pin(id)
        .ok()
        .and_then(|(_, n)| n);
    let raw = match pinned {
        Some(_) => super::skill_versions::pinned_file(id)?,
        None => super::skills::custom_skill_file(id)?,
    };
    let mut content = portable_content(&raw);
    if let Some(n) = pinned {
        let name = super::skills::parse_skill_markdown(id, &content, false)?.name;
        content = renamed(&content, &format!("{name} (v{n})")).unwrap_or(content);
    }
    let skill = super::skills::parse_skill_markdown(id, &content, false)?;
    Some(BundledSkill {
        id: id.to_string(),
        name: skill.name,
        version: pinned.or_else(|| super::skill_versions::newest_index(id)),
        sha256: sha256_hex(&content),
        content,
    })
}

/// The bundle entries of every custom skill among `ids`, once each.
pub fn bundle_skills<'a>(ids: impl IntoIterator<Item = &'a String>) -> Vec<BundledSkill> {
    let wanted: BTreeSet<&String> = ids
        .into_iter()
        .filter(|id| id.starts_with("custom-"))
        .collect();
    wanted
        .into_iter()
        .filter_map(|id| bundle_skill(id))
        .collect()
}

/// A bundled skill whose file is intact, with the name its file gives.
fn checked(skill: &BundledSkill) -> Result<String, String> {
    if skill.id.trim().is_empty() || skill.id.len() > 128 {
        return Err("A bundled skill has an invalid id".into());
    }
    if skill.content.len() > MAX_SKILL_BYTES {
        return Err(format!(
            "Bundled skill `{}` exceeds {} KiB",
            skill.id,
            MAX_SKILL_BYTES / 1024
        ));
    }
    if sha256_hex(&skill.content) != skill.sha256 {
        return Err(format!(
            "Bundled skill `{}`: its content does not match its sha256",
            skill.id
        ));
    }
    super::skills::parse_skill_markdown(&skill.id, &skill.content, false)
        .map(|parsed| parsed.name)
        .ok_or_else(|| format!("Bundled skill `{}` is not a valid skill file", skill.id))
}

/// What an import into `project_id` does with each bundled skill. A local
/// skill counts only when it is global or belongs to that project.
pub fn plan(
    bundled: &[BundledSkill],
    choices: &[SkillImportChoice],
    project_id: Option<&str>,
) -> Result<Vec<SkillImportDecision>, String> {
    let mut sources = BTreeSet::new();
    let mut names = Vec::with_capacity(bundled.len());
    for skill in bundled {
        names.push(checked(skill)?);
        if !sources.insert(skill.id.as_str()) {
            return Err(format!("Bundled skill `{}` appears twice", skill.id));
        }
    }
    let mut resolutions = HashMap::new();
    for choice in choices {
        if !sources.contains(choice.source_id.as_str())
            || resolutions
                .insert(choice.source_id.as_str(), choice.resolution)
                .is_some()
        {
            return Err(format!(
                "Unknown or duplicate skill choice `{}`",
                choice.source_id
            ));
        }
    }
    let visible = |owner: Option<&str>| owner.is_none_or(|owner| Some(owner) == project_id);
    let local: Vec<(String, String)> = super::skills::custom_skill_files()
        .into_iter()
        .filter_map(|(id, raw)| {
            let skill = super::skills::parse_skill_markdown(&id, &raw, false)?;
            visible(skill.project_id.as_deref()).then(|| (id, sha256_hex(&portable_content(&raw))))
        })
        .collect();
    let catalog = super::skills::list_all_skills();
    Ok(bundled
        .iter()
        .zip(names)
        .map(|(skill, name)| {
            let identical = local
                .iter()
                .filter(|(_, hash)| *hash == skill.sha256)
                .min_by_key(|(id, _)| (id != &skill.id, id.clone()))
                .map(|(id, _)| id.clone());
            if let Some(existing) = identical {
                return SkillImportDecision {
                    source_id: skill.id.clone(),
                    name,
                    action: SkillImportAction::Reuse,
                    existing_id: Some(existing),
                    resolution: None,
                };
            }
            let same_name = catalog
                .iter()
                .filter(|local| visible(local.project_id.as_deref()))
                .find(|local| local.name.trim().eq_ignore_ascii_case(name.trim()))
                .map(|local| local.id.clone());
            match same_name {
                Some(existing) => SkillImportDecision {
                    source_id: skill.id.clone(),
                    resolution: Some(
                        resolutions
                            .get(skill.id.as_str())
                            .copied()
                            .unwrap_or_default(),
                    ),
                    name,
                    action: SkillImportAction::Conflict,
                    existing_id: Some(existing),
                },
                None => SkillImportDecision {
                    source_id: skill.id.clone(),
                    name,
                    action: SkillImportAction::Create,
                    existing_id: None,
                    resolution: None,
                },
            }
        })
        .collect())
}

/// The id each source skill keeps after the import, as far as a preview can
/// tell: a skill still to create gets a placeholder that names no local skill.
pub fn predicted_ids(decisions: &[SkillImportDecision]) -> HashMap<String, String> {
    decisions
        .iter()
        .map(|decision| {
            let target = match (decision.action, decision.resolution) {
                (SkillImportAction::Reuse, _)
                | (SkillImportAction::Conflict, Some(SkillConflictResolution::UseExisting)) => {
                    decision.existing_id.clone().unwrap_or_default()
                }
                _ => format!("custom-to-create:{}", decision.source_id),
            };
            (decision.source_id.clone(), target)
        })
        .collect()
}

/// A display name no visible skill uses yet: `name (2)`, `name (3)`…
fn free_copy_name(name: &str) -> String {
    let taken: BTreeSet<String> = super::skills::list_all_skills()
        .into_iter()
        .map(|skill| skill.name.trim().to_lowercase())
        .collect();
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .expect("an unbounded range always yields a free name")
}

/// Carry out `decisions` (from [`plan`] on the same bundle): the source id →
/// local id of every bundled skill. Skills are files, outside any database
/// transaction: one created before a later failure stays, and the next
/// import of the same bundle reuses it.
pub fn apply(
    bundled: &[BundledSkill],
    decisions: &[SkillImportDecision],
    project_id: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    let mut ids = HashMap::new();
    for decision in decisions {
        let skill = bundled
            .iter()
            .find(|skill| skill.id == decision.source_id)
            .ok_or_else(|| format!("Bundled skill `{}` is missing", decision.source_id))?;
        let existing = || {
            decision
                .existing_id
                .clone()
                .ok_or_else(|| format!("Skill `{}` has no local match", decision.source_id))
        };
        let portable = without_allowed_tools(&portable_content(&skill.content))?;
        let target = match (decision.action, decision.resolution) {
            (SkillImportAction::Reuse, _)
            | (SkillImportAction::Conflict, Some(SkillConflictResolution::UseExisting)) => {
                existing()?
            }
            (SkillImportAction::Conflict, _) => {
                let copy = renamed(&portable, &free_copy_name(&decision.name))
                    .unwrap_or_else(|| portable.clone());
                super::skills::create_custom_skill_from_file(&with_project(&copy, project_id))?
            }
            (SkillImportAction::Create, _) => {
                super::skills::create_custom_skill_from_file(&with_project(&portable, project_id))?
            }
        };
        // The step loaded a fixed version: it keeps loading this content even
        // after later edits of the imported skill.
        let target = match (
            skill.id.contains('@'),
            super::skill_versions::newest_index(&target),
        ) {
            (true, Some(n)) => format!("{target}@{n}"),
            _ => target,
        };
        ids.insert(skill.id.clone(), target);
    }
    Ok(ids)
}

/// Rewrites every id of `ids` found in `remap`; others stay as they are.
pub fn remap_ids(ids: &mut [String], remap: &HashMap<String, String>) {
    for id in ids.iter_mut() {
        if let Some(target) = remap.get(id.as_str()) {
            *id = target.clone();
        }
    }
}

/// Every value of a JsonData `payload` spelled like a custom skill id
/// (`custom-x`, `custom-x@N`): a script that loads skills by the ids its
/// configuration lists needs them as much as a step's `skill_ids`. Export
/// bundles those that exist; any other string is left out.
pub fn ids_in_payload(payload: &serde_json::Value, out: &mut BTreeSet<String>) {
    match payload {
        serde_json::Value::String(text) => {
            if text.starts_with("custom-")
                && !text.contains(char::is_whitespace)
                && super::skill_versions::split_pin(text).is_ok()
            {
                out.insert(text.clone());
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| ids_in_payload(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| ids_in_payload(item, out)),
        _ => {}
    }
}

/// [`remap_ids`] for the values of a JsonData `payload`: a value equal to a
/// bundled skill id becomes the local id; keys and other values stay.
pub fn remap_payload(payload: &mut serde_json::Value, remap: &HashMap<String, String>) {
    match payload {
        serde_json::Value::String(text) => {
            if let Some(target) = remap.get(text.as_str()) {
                *text = target.clone();
            }
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|item| remap_payload(item, remap))
        }
        serde_json::Value::Object(map) => {
            map.values_mut().for_each(|item| remap_payload(item, remap))
        }
        _ => {}
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::SkillCategory;
    use serial_test::serial;

    struct DataDir {
        dir: std::path::PathBuf,
        previous: Option<std::ffi::OsString>,
    }

    impl DataDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kronn-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
            crate::core::child_env::set_var("KRONN_DATA_DIR", &dir);
            Self { dir, previous }
        }
    }

    impl Drop for DataDir {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => crate::core::child_env::set_var("KRONN_DATA_DIR", value),
                None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn create(name: &str, body: &str, project: Option<&str>) -> String {
        crate::core::skills::save_custom_skill(
            name,
            "d",
            "🔧",
            &SkillCategory::Domain,
            body,
            None,
            None,
            project,
        )
        .unwrap()
    }

    #[test]
    fn an_imported_skill_never_pre_approves_tools_whatever_the_spelling() {
        for raw in [
            "---\nname: Review\ndescription: d\nallowed-tools: Bash(git *) Write\n---\nBody\n",
            "---\nname: Review\ndescription: d\nallowed-tools:\n  - Bash\n  - Write\n---\nBody\n",
            "---\nname: Review\ndescription: d\nallowed-tools: >-\n  Bash Write\n---\nBody\n",
        ] {
            let stripped = without_allowed_tools(raw).unwrap();
            assert!(!stripped.contains("allowed-tools"), "{stripped}");
            assert!(!stripped.contains("Write"), "{stripped}");
            assert!(stripped.contains("description: d") && stripped.ends_with("Body\n"));
        }
        assert_eq!(
            without_allowed_tools("No frontmatter\n").unwrap(),
            "No frontmatter\n"
        );
    }

    #[test]
    #[serial]
    fn a_pinned_version_travels_apart_and_comes_back_pinned() {
        let _data = DataDir::new("skill-bundle-pin");
        let id = create("Review", "rule one", None);
        crate::core::skills::update_custom_skill(
            &id,
            "Review",
            "d",
            "🔧",
            &SkillCategory::Domain,
            "rule two",
            None,
            None,
            None,
        )
        .unwrap();
        let pinned = format!("{id}@1");
        let bundled = bundle_skills([&pinned, &id]);
        let old = bundled.iter().find(|s| s.id == pinned).unwrap();
        assert_eq!(old.version, Some(1));
        assert!(old.content.contains("rule one") && old.content.contains("Review (v1)"));

        crate::core::skills::delete_custom_skill(&id).unwrap();
        let decisions = plan(&bundled, &[], None).unwrap();
        let ids = apply(&bundled, &decisions, None).unwrap();
        let target = &ids[&pinned];
        assert!(target.ends_with("@1"), "{target}");
        assert_eq!(
            crate::core::skills::get_skill(target).unwrap().content,
            "rule one"
        );
        assert!(!ids[&id].contains('@'));
    }

    #[test]
    fn the_project_scope_is_left_out_and_put_back() {
        let raw =
            "---\nname: \"A\"\ncategory: domain\nmetadata:\n  kronn-project: \"p1\"\n---\nBody\n";
        let portable = portable_content(raw);
        assert_eq!(portable, "---\nname: \"A\"\ncategory: domain\n---\nBody\n");
        let scoped = with_project(&portable, Some("p2"));
        let skill = crate::core::skills::parse_skill_markdown("custom-a", &scoped, false).unwrap();
        assert_eq!(skill.project_id.as_deref(), Some("p2"));
        assert_eq!(skill.content, "Body");
        assert_eq!(with_project(&portable, None), portable);
        // Other metadata entries stay.
        let raw = "---\r\nname: x\r\nmetadata:\r\n  kronn-icon: \"🦀\"\r\n  kronn-project: \"p\"\r\n---\r\nB";
        assert_eq!(
            portable_content(raw),
            "---\r\nname: x\r\nmetadata:\r\n  kronn-icon: \"🦀\"\r\n---\r\nB"
        );
        let scoped = with_project(&portable_content(raw), Some("q"));
        let skill = crate::core::skills::parse_skill_markdown("custom-x", &scoped, false).unwrap();
        assert_eq!(skill.project_id.as_deref(), Some("q"));
        assert_eq!(skill.icon, "🦀");
    }

    #[test]
    fn a_copy_gets_the_name_it_asks_for() {
        let raw = "---\nname: \"Review\"\ncategory: domain\n---\nBody";
        let copy = renamed(raw, "Review (2)").unwrap();
        let skill = crate::core::skills::parse_skill_markdown("custom-r", &copy, false).unwrap();
        assert_eq!(skill.name, "Review (2)");
        assert!(renamed("no frontmatter", "x").is_none());
    }

    #[test]
    #[serial]
    fn plan_reuses_identical_creates_missing_and_flags_same_name() {
        let _dir = DataDir::new("skill-bundle-plan");
        let id = create("Review", "v1", None);
        let bundled = bundle_skill(&id).unwrap();
        assert_eq!(bundled.version, Some(1));
        assert_eq!(bundled.sha256, sha256_hex(&bundled.content));
        assert!(bundle_skill("rust").is_none(), "builtins stay references");

        let decisions = plan(std::slice::from_ref(&bundled), &[], None).unwrap();
        assert_eq!(decisions[0].action, SkillImportAction::Reuse);
        assert_eq!(decisions[0].existing_id.as_deref(), Some(id.as_str()));

        let mut changed = bundled.clone();
        changed.content = changed.content.replace("v1", "v2");
        changed.sha256 = sha256_hex(&changed.content);
        let decisions = plan(std::slice::from_ref(&changed), &[], None).unwrap();
        assert_eq!(decisions[0].action, SkillImportAction::Conflict);
        assert_eq!(
            decisions[0].resolution,
            Some(SkillConflictResolution::CreateCopy)
        );
        let ids = apply(std::slice::from_ref(&changed), &decisions, None).unwrap();
        let copy = crate::core::skills::get_skill(&ids[&changed.id]).unwrap();
        assert_eq!(copy.name, "Review (2)");
        assert_eq!(copy.content, "v2");

        let choice = SkillImportChoice {
            source_id: changed.id.clone(),
            resolution: SkillConflictResolution::UseExisting,
        };
        let mut other = changed.clone();
        other.content = other.content.replace("v2", "v3");
        other.sha256 = sha256_hex(&other.content);
        let decisions = plan(std::slice::from_ref(&other), &[choice], None).unwrap();
        let ids = apply(std::slice::from_ref(&other), &decisions, None).unwrap();
        assert_eq!(ids[&other.id], id, "use_existing points at the local skill");

        let mut tampered = bundled.clone();
        tampered.content.push('!');
        assert!(plan(&[tampered], &[], None).is_err());
    }

    #[test]
    #[serial]
    fn a_skill_of_another_project_is_neither_reused_nor_a_conflict() {
        let _dir = DataDir::new("skill-bundle-scope");
        let id = create("Scoped", "body", Some("p-other"));
        let bundled = bundle_skill(&id).unwrap();
        assert!(!bundled.content.contains("p-other"));
        let decisions = plan(std::slice::from_ref(&bundled), &[], Some("p-mine")).unwrap();
        assert_eq!(decisions[0].action, SkillImportAction::Create);
        let ids = apply(std::slice::from_ref(&bundled), &decisions, Some("p-mine")).unwrap();
        let created = crate::core::skills::get_skill(&ids[&id]).unwrap();
        assert_eq!(created.project_id.as_deref(), Some("p-mine"));
        // Same project as the existing one: identical content is reused.
        let decisions = plan(&[bundled], &[], Some("p-other")).unwrap();
        assert_eq!(decisions[0].action, SkillImportAction::Reuse);
    }
}
