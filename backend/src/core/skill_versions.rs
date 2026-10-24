//! Version history of custom skills (KT-1434), the counterpart of the Quick
//! Prompt history. A custom skill is a file, so each version is a full copy
//! of that file under `skills/.versions/<slug>/<n>.md`: the catalog scan
//! only reads `skills/*.md` and never sees them.

use std::path::{Path, PathBuf};

use crate::models::SkillVersion;

const VERSIONS_DIR: &str = ".versions";

fn slug_of(id: &str) -> Result<&str, String> {
    let slug = id
        .strip_prefix("custom-")
        .ok_or("Builtin skills have no version history")?;
    if !super::native_files::is_valid_slug(slug) {
        return Err(format!("Invalid skill id '{id}'"));
    }
    Ok(slug)
}

fn history_dir(skills_dir: &Path, slug: &str) -> PathBuf {
    skills_dir.join(VERSIONS_DIR).join(slug)
}

/// Version numbers saved for `slug`, oldest first.
fn indexes(dir: &Path) -> Vec<u32> {
    let mut found: Vec<u32> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        return None;
                    }
                    path.file_stem()?.to_str()?.parse::<u32>().ok()
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_unstable();
    found
}

fn version_path(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("{index}.md"))
}

/// Adds `raw` as the next version of `slug`, unless it is the latest one
/// already: a save that changes nothing adds nothing. Returns the new index.
pub(crate) fn record(skills_dir: &Path, slug: &str, raw: &str) -> Result<Option<u32>, String> {
    let dir = history_dir(skills_dir, slug);
    let existing = indexes(&dir);
    if let Some(last) = existing.last() {
        if std::fs::read_to_string(version_path(&dir, *last))
            .ok()
            .as_deref()
            == Some(raw)
        {
            return Ok(None);
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create skill history: {e}"))?;
    let next = existing.last().map_or(1, |last| last + 1);
    crate::core::mcp_scanner::atomic_write(&version_path(&dir, next), raw)
        .map_err(|e| format!("Cannot write skill version: {e}"))?;
    Ok(Some(next))
}

/// A skill written before its history existed gets its current file as v1,
/// so the first change made afterwards can still be undone.
pub(crate) fn record_baseline(skills_dir: &Path, slug: &str, raw: &str) -> Result<(), String> {
    if indexes(&history_dir(skills_dir, slug)).is_empty() {
        record(skills_dir, slug, raw)?;
    }
    Ok(())
}

pub(crate) fn forget(skills_dir: &Path, slug: &str) -> Result<(), String> {
    let dir = history_dir(skills_dir, slug);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot delete skill history: {e}")),
    }
}

fn to_version(id: &str, index: u32, path: &Path, current: bool) -> Option<SkillVersion> {
    let raw = std::fs::read_to_string(path).ok()?;
    let skill = super::skills::parse_skill_markdown(id, &raw, false)?;
    let created_at = std::fs::metadata(path).ok()?.modified().ok()?.into();
    Some(SkillVersion {
        skill_id: id.to_string(),
        version_index: index,
        name: skill.name,
        description: skill.description,
        icon: skill.icon,
        category: skill.category,
        content: skill.content,
        created_at,
        current,
    })
}

/// Every version of skill `id`, newest first. A custom skill saved before
/// histories existed shows its current file as v1. Builtin skills: none.
pub fn history(id: &str) -> Result<Vec<SkillVersion>, String> {
    if !id.starts_with("custom-") {
        return Ok(Vec::new());
    }
    let slug = slug_of(id)?;
    let skills_dir = super::skills::skills_dir()?;
    let current = skills_dir.join(format!("{slug}.md"));
    if !current.exists() {
        return Err(format!("Skill '{id}' not found"));
    }
    let dir = history_dir(&skills_dir, slug);
    if indexes(&dir).is_empty() {
        let raw = std::fs::read_to_string(&current)
            .map_err(|e| format!("Cannot read skill '{id}': {e}"))?;
        crate::core::config::refuse_real_data_dir_in_tests()?;
        record(&skills_dir, slug, &raw)?;
    }
    let all = indexes(&dir);
    let newest = all.last().copied();
    Ok(all
        .iter()
        .rev()
        .filter_map(|index| {
            to_version(
                id,
                *index,
                &version_path(&dir, *index),
                Some(*index) == newest,
            )
        })
        .collect())
}

/// `custom-x@N` names version N of `custom-x` (KT-1447); any other id is
/// not pinned. A malformed pin is an error, never silently the latest.
pub fn split_pin(id: &str) -> Result<(&str, Option<u32>), String> {
    let Some((base, version)) = id.split_once('@') else {
        return Ok((id, None));
    };
    let index = version
        .parse::<u32>()
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| {
            format!("Skill '{id}': a pinned version is `@` followed by a number from 1")
        })?;
    if !base.starts_with("custom-") {
        return Err(format!(
            "Skill '{id}': only custom skills have versions to pin"
        ));
    }
    Ok((base, Some(index)))
}

/// The skill as version N saved it, under its pinned id `custom-x@N`.
pub fn pinned_skill(id: &str) -> Option<crate::models::Skill> {
    let (base, Some(index)) = split_pin(id).ok()? else {
        return None;
    };
    let slug = slug_of(base).ok()?;
    let path = version_path(
        &history_dir(&super::skills::skills_dir().ok()?, slug),
        index,
    );
    let raw = std::fs::read_to_string(path).ok()?;
    super::skills::parse_skill_markdown(id, &raw, false)
}

/// Refuses a pinned skill id whose version does not exist; unpinned ids are
/// left to the launch, which already skips an unknown one.
pub fn validate_pins(ids: &[String]) -> Result<(), String> {
    for id in ids {
        if split_pin(id)?.1.is_some() && pinned_skill(id).is_none() {
            return Err(format!("Skill '{id}': this version does not exist"));
        }
    }
    Ok(())
}

/// Version `index` of skill `id`.
pub fn version(id: &str, index: u32) -> Result<SkillVersion, String> {
    history(id)?
        .into_iter()
        .find(|v| v.version_index == index)
        .ok_or_else(|| format!("Skill '{id}' has no version {index}"))
}

/// Makes version `index` the skill's content again, as a new version. The
/// skill keeps its current project, arguments and variables.
pub fn restore(id: &str, index: u32) -> Result<SkillVersion, String> {
    let old = version(id, index)?;
    let slug = slug_of(id)?;
    let path = version_path(&history_dir(&super::skills::skills_dir()?, slug), index);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("Cannot read version: {e}"))?;
    let file = super::skills::parse_skill_markdown(id, &raw, false)
        .ok_or_else(|| format!("Version {index} of '{id}' cannot be read"))?;
    super::skills::update_custom_skill(
        id,
        &old.name,
        &old.description,
        &old.icon,
        &old.category,
        &old.content,
        file.license.as_deref(),
        file.allowed_tools.as_deref(),
        None,
    )?;
    history(id)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("Skill '{id}' has no history"))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::core::skills::{get_skill, list_all_skills, save_custom_skill, update_custom_skill};
    use crate::models::SkillCategory;
    use serial_test::serial;

    struct DataDir {
        dir: PathBuf,
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

    fn create(content: &str) -> String {
        save_custom_skill(
            "Review",
            "desc",
            "🔎",
            &SkillCategory::Domain,
            content,
            None,
            None,
            None,
        )
        .unwrap()
    }

    fn update(id: &str, content: &str) {
        update_custom_skill(
            id,
            "Review",
            "desc",
            "🔎",
            &SkillCategory::Domain,
            content,
            None,
            None,
            None,
        )
        .unwrap();
    }

    fn contents(id: &str) -> Vec<(u32, String, bool)> {
        history(id)
            .unwrap()
            .into_iter()
            .map(|v| (v.version_index, v.content, v.current))
            .collect()
    }

    #[test]
    #[serial]
    fn each_change_adds_a_version_and_an_unchanged_save_adds_none() {
        let _data = DataDir::new("skill-versions-record");
        let id = create("rule one");
        update(&id, "rule two");
        update(&id, "rule two");
        assert_eq!(
            contents(&id),
            vec![(2, "rule two".into(), true), (1, "rule one".into(), false)]
        );
        assert_eq!(version(&id, 1).unwrap().content, "rule one");
        assert!(version(&id, 3).is_err());
    }

    #[test]
    #[serial]
    fn restore_brings_an_old_version_back_as_a_new_one() {
        let _data = DataDir::new("skill-versions-restore");
        let id = create("rule one");
        update(&id, "rule two");
        let restored = restore(&id, 1).unwrap();
        assert_eq!((restored.version_index, restored.current), (3, true));
        assert_eq!(get_skill(&id).unwrap().content, "rule one");
        assert_eq!(contents(&id).len(), 3);
    }

    #[test]
    #[serial]
    fn a_skill_saved_before_histories_existed_keeps_its_first_state() {
        let data = DataDir::new("skill-versions-baseline");
        let id = create("rule one");
        std::fs::remove_dir_all(data.dir.join("skills").join(VERSIONS_DIR)).unwrap();
        update(&id, "rule two");
        assert_eq!(
            contents(&id),
            vec![(2, "rule two".into(), true), (1, "rule one".into(), false)]
        );
        std::fs::remove_dir_all(data.dir.join("skills").join(VERSIONS_DIR)).unwrap();
        assert_eq!(contents(&id), vec![(1, "rule two".into(), true)]);
    }

    #[test]
    #[serial]
    fn the_history_is_not_a_skill_and_goes_with_its_skill() {
        let data = DataDir::new("skill-versions-catalog");
        let id = create("rule one");
        update(&id, "rule two");
        let custom: Vec<_> = list_all_skills()
            .into_iter()
            .filter(|s| !s.is_builtin)
            .map(|s| s.id)
            .collect();
        assert_eq!(custom, vec![id.clone()]);
        assert!(crate::core::skills::delete_custom_skill(&id).unwrap());
        let slug = id.strip_prefix("custom-").unwrap();
        assert!(!data
            .dir
            .join("skills")
            .join(VERSIONS_DIR)
            .join(slug)
            .exists());
        assert!(history(&id).is_err());
    }

    #[test]
    fn a_pin_names_a_version_and_a_bad_pin_is_an_error() {
        assert_eq!(split_pin("custom-review").unwrap(), ("custom-review", None));
        assert_eq!(
            split_pin("custom-review@3").unwrap(),
            ("custom-review", Some(3))
        );
        assert!(split_pin("custom-review@0").is_err());
        assert!(split_pin("custom-review@latest").is_err());
        assert!(split_pin("rust@1").is_err());
    }

    #[test]
    #[serial]
    fn a_pinned_id_loads_that_version_even_after_later_changes() {
        let _data = DataDir::new("skill-versions-pin");
        let id = create("rule one");
        update(&id, "rule two");
        let pinned = format!("{id}@1");
        let skill = crate::core::skills::get_skill(&pinned).unwrap();
        assert_eq!(
            (skill.id.as_str(), skill.content.as_str()),
            (pinned.as_str(), "rule one")
        );
        let both = crate::core::skills::get_skills_by_ids(&[pinned.clone(), id.clone()]);
        let contents: Vec<_> = both.iter().map(|s| s.content.as_str()).collect();
        assert_eq!(contents, vec!["rule one", "rule two"]);
        assert!(validate_pins(&[pinned, id.clone()]).is_ok());
        assert!(validate_pins(&[format!("{id}@9")]).is_err());
        assert!(crate::core::skills::get_skill(&format!("{id}@9")).is_none());
    }

    #[test]
    fn builtin_skills_have_no_history() {
        assert!(history("rust").unwrap().is_empty());
        assert!(restore("rust", 1).is_err());
    }
}
