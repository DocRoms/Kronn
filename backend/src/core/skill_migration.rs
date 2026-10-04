//! "Migrate everything to `.agents/skills`": gather the skills scattered across
//! the agents' own folders (`.claude/skills`, `.gemini/skills`…) and the
//! `kronn/skills` Kronn used to write into, in the one folder every agent reads.
//!
//! Two steps, so nothing is written before the user has seen it: [`plan`] only
//! reads the repository and lists what would move and what needs a choice, and
//! [`apply`] performs exactly that, recomputed on the spot. A slug found in
//! several places with different contents is a conflict; it is written only when
//! the caller names the version to keep, and a folder is removed only when the
//! target now holds its exact content, so no copy is lost or overwritten
//! silently.
//!
//! Every path goes through `validate_relative` and the no-symlink walk of
//! `fs_guard`, and a skill folder holding a symbolic link is never touched.
//! Nothing is committed: the files stay in the working tree.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::core::repository_resources::{self as resources, LEGACY_SKILLS_ROOT, SKILLS_ROOT};
use crate::models::{
    ProjectRepositoryResourceKind, SkillMigrationAction, SkillMigrationBlockReason,
    SkillMigrationBlocked, SkillMigrationConflict, SkillMigrationMove, SkillMigrationPlan,
    SkillMigrationResolution, SkillMigrationResult, SkillMigrationVersion,
};

/// The folders skills are gathered from, in the order their copies are
/// preferred.
pub const SOURCE_ROOTS: &[&str] = &[
    ".claude/skills",
    ".gemini/skills",
    ".codex/skills",
    ".github/skills",
    ".opencode/skill",
    ".opencode/skills",
    ".cursor/skills",
    LEGACY_SKILLS_ROOT,
];

/// `.agents/skills/kronn` is Kronn's own router skill.
const RESERVED_SLUG: &str = "kronn";
const MAX_FILES: usize = 500;
const MAX_BYTES: usize = 20 * 1024 * 1024;
const SKILL_FILE: &str = "SKILL.md";

struct FileData {
    /// What the target receives (a converted `SKILL.md` differs from the file).
    bytes: Vec<u8>,
    /// What the file holds where it was read, checked before it is deleted.
    original: Vec<u8>,
    #[cfg(unix)]
    mode: u32,
}

/// One folder holding a skill, read whole.
struct Candidate {
    /// Repository-relative, `<root>/<slug>`.
    dir: String,
    files: BTreeMap<String, FileData>,
    hash: String,
    converted: bool,
    kronn_managed: bool,
}

#[derive(Default)]
struct Group {
    target: Option<Candidate>,
    sources: Vec<Candidate>,
}

struct Scan {
    groups: BTreeMap<String, Group>,
    blocked: Vec<SkillMigrationBlocked>,
}

fn block(path: &str, reason: SkillMigrationBlockReason) -> SkillMigrationBlocked {
    SkillMigrationBlocked {
        path: path.to_string(),
        reason,
    }
}

fn tree_hash(files: &BTreeMap<String, FileData>) -> String {
    let mut digest = Sha256::new();
    for (path, file) in files {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(&file.bytes);
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Reads a skill folder's regular files, refusing anything that is not one.
fn read_tree(
    root: &Path,
    dir: &str,
) -> Result<BTreeMap<String, FileData>, SkillMigrationBlockReason> {
    fn walk(
        base: &Path,
        directory: &Path,
        files: &mut BTreeMap<String, FileData>,
        total: &mut usize,
    ) -> Result<(), SkillMigrationBlockReason> {
        let entries =
            std::fs::read_dir(directory).map_err(|_| SkillMigrationBlockReason::Unreadable)?;
        for entry in entries {
            let path = entry
                .map_err(|_| SkillMigrationBlockReason::Unreadable)?
                .path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| SkillMigrationBlockReason::Unreadable)?;
            if metadata.file_type().is_symlink() {
                return Err(SkillMigrationBlockReason::Symlink);
            }
            if metadata.is_dir() {
                walk(base, &path, files, total)?;
                continue;
            }
            if !metadata.is_file() {
                return Err(SkillMigrationBlockReason::Unreadable);
            }
            *total += usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if files.len() >= MAX_FILES || *total > MAX_BYTES {
                return Err(SkillMigrationBlockReason::TooLarge);
            }
            let bytes = std::fs::read(&path).map_err(|_| SkillMigrationBlockReason::Unreadable)?;
            let relative = path
                .strip_prefix(base)
                .map_err(|_| SkillMigrationBlockReason::Unreadable)?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            resources::validate_relative(&relative)
                .map_err(|_| SkillMigrationBlockReason::Unreadable)?;
            files.insert(
                relative,
                FileData {
                    original: bytes.clone(),
                    bytes,
                    #[cfg(unix)]
                    mode: {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode()
                    },
                },
            );
        }
        Ok(())
    }

    resources::validate_relative(dir).map_err(|_| SkillMigrationBlockReason::Unreadable)?;
    let base = root.join(dir);
    crate::core::fs_guard::assert_contained_no_symlink(root, &base)
        .map_err(|_| SkillMigrationBlockReason::Symlink)?;
    let mut files = BTreeMap::new();
    walk(&base, &base, &mut files, &mut 0)?;
    Ok(files)
}

/// Whether `dir` holds a `SKILL.md` that is not a symbolic link, as
/// `Some(true)`; `Some(false)` for a link; `None` when there is none.
fn skill_file_state(root: &Path, dir: &str) -> Option<bool> {
    match std::fs::symlink_metadata(root.join(dir).join(SKILL_FILE)) {
        Ok(metadata) => Some(!metadata.file_type().is_symlink()),
        Err(_) => None,
    }
}

fn candidate(
    root: &Path,
    dir: String,
    convert_legacy: bool,
    managed_slugs: &[String],
    slug: &str,
) -> Result<Candidate, SkillMigrationBlockReason> {
    let mut files = read_tree(root, &dir)?;
    let mut converted = false;
    if convert_legacy {
        let legacy_file = files
            .get(SKILL_FILE)
            .and_then(|file| std::str::from_utf8(&file.bytes).ok())
            .is_some_and(|text| text.contains("<!-- kronn:resource\n"));
        if legacy_file {
            let standard =
                resources::legacy_skill_as_standard(root, slug, &format!("{dir}/{SKILL_FILE}"))
                    .map_err(|_| SkillMigrationBlockReason::Unreadable)?;
            if let Some(file) = files.get_mut(SKILL_FILE) {
                file.bytes = standard;
            }
            converted = true;
        }
    }
    let kronn_managed =
        convert_legacy && managed_slugs.iter().any(|managed| managed.as_str() == slug);
    let hash = tree_hash(&files);
    Ok(Candidate {
        dir,
        files,
        hash,
        converted,
        kronn_managed,
    })
}

/// Skills `kronn.lock` lists at their former `kronn/skills` location.
fn managed_legacy_slugs(root: &Path) -> Vec<String> {
    resources::load_lock(root)
        .ok()
        .flatten()
        .map(|lock| {
            lock.resources
                .iter()
                .filter(|entry| {
                    entry.kind == ProjectRepositoryResourceKind::Skill
                        && entry
                            .paths
                            .iter()
                            .any(|path| resources::is_legacy_skill_path(path))
                })
                .map(|entry| entry.slug.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn scan(root: &Path) -> Scan {
    let managed = managed_legacy_slugs(root);
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut blocked = Vec::new();
    for source_root in SOURCE_ROOTS {
        let directory = root.join(source_root);
        if crate::core::fs_guard::assert_contained_no_symlink(root, &directory).is_err() {
            blocked.push(block(source_root, SkillMigrationBlockReason::Symlink));
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for slug in names {
            let dir = format!("{source_root}/{slug}");
            let Ok(metadata) = std::fs::symlink_metadata(root.join(&dir)) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                if skill_file_state(root, &dir).is_some() {
                    blocked.push(block(&dir, SkillMigrationBlockReason::Symlink));
                }
                continue;
            }
            if !metadata.is_dir() || slug.starts_with('.') {
                continue;
            }
            match skill_file_state(root, &dir) {
                None => continue,
                Some(false) => {
                    blocked.push(block(&dir, SkillMigrationBlockReason::Symlink));
                    continue;
                }
                Some(true) => {}
            }
            // A catalogue skill Kronn synced here is not the repository's own.
            if crate::core::native_files::is_kronn_owned_file(root, &format!("{dir}/{SKILL_FILE}"))
            {
                continue;
            }
            if slug == RESERVED_SLUG {
                blocked.push(block(&dir, SkillMigrationBlockReason::ReservedSlug));
                continue;
            }
            match candidate(
                root,
                dir.clone(),
                *source_root == LEGACY_SKILLS_ROOT,
                &managed,
                &slug,
            ) {
                Ok(found) => groups.entry(slug).or_default().sources.push(found),
                Err(reason) => blocked.push(block(&dir, reason)),
            }
        }
    }
    let slugs: Vec<String> = groups.keys().cloned().collect();
    for slug in slugs {
        let dir = format!("{SKILLS_ROOT}/{slug}");
        let Ok(metadata) = std::fs::symlink_metadata(root.join(&dir)) else {
            continue;
        };
        let refusal = if metadata.file_type().is_symlink() {
            Some(SkillMigrationBlockReason::Symlink)
        } else if !metadata.is_dir() || skill_file_state(root, &dir) != Some(true) {
            Some(SkillMigrationBlockReason::TargetOccupied)
        } else {
            None
        };
        if let Some(reason) = refusal {
            blocked.push(block(&dir, reason));
            groups.remove(&slug);
            continue;
        }
        match candidate(root, dir.clone(), false, &managed, &slug) {
            Ok(found) => {
                if let Some(group) = groups.get_mut(&slug) {
                    group.target = Some(found);
                }
            }
            Err(reason) => {
                blocked.push(block(&dir, reason));
                groups.remove(&slug);
            }
        }
    }
    Scan { groups, blocked }
}

fn target_dir(slug: &str) -> String {
    format!("{SKILLS_ROOT}/{slug}")
}

fn move_of(slug: &str, source: &Candidate, action: SkillMigrationAction) -> SkillMigrationMove {
    SkillMigrationMove {
        slug: slug.to_string(),
        source: source.dir.clone(),
        target: target_dir(slug),
        action,
        converted: source.converted,
        kronn_managed: source.kronn_managed,
    }
}

/// The distinct contents of a slug's folders, the target's first, each with
/// the folders holding it.
fn versions(group: &Group) -> Vec<(String, Vec<&Candidate>)> {
    let mut versions: Vec<(String, Vec<&Candidate>)> = Vec::new();
    for found in group.target.iter().chain(group.sources.iter()) {
        match versions.iter_mut().find(|(hash, _)| *hash == found.hash) {
            Some((_, holders)) => holders.push(found),
            None => versions.push((found.hash.clone(), vec![found])),
        }
    }
    versions
}

/// What a migration would do. Reads the repository only.
pub fn plan(root: &Path) -> SkillMigrationPlan {
    let scanned = scan(root);
    let mut plan = SkillMigrationPlan {
        target_root: SKILLS_ROOT.to_string(),
        blocked: scanned.blocked,
        ..SkillMigrationPlan::default()
    };
    for (slug, group) in &scanned.groups {
        let versions = versions(group);
        if versions.len() > 1 {
            plan.conflicts.push(SkillMigrationConflict {
                slug: slug.clone(),
                target: target_dir(slug),
                versions: versions
                    .iter()
                    .map(|(hash, holders)| SkillMigrationVersion {
                        fingerprint: resources::fingerprint(hash),
                        paths: holders.iter().map(|found| found.dir.clone()).collect(),
                        at_target: group
                            .target
                            .as_ref()
                            .is_some_and(|target| target.hash == *hash),
                    })
                    .collect(),
            });
            continue;
        }
        for (index, source) in group.sources.iter().enumerate() {
            let action = if group.target.is_none() && index == 0 {
                SkillMigrationAction::Move
            } else {
                SkillMigrationAction::Duplicate
            };
            plan.moves.push(move_of(slug, source, action));
        }
    }
    plan
}

fn write_tree(root: &Path, dir: &str, files: &BTreeMap<String, FileData>) -> Result<(), String> {
    for (relative, file) in files {
        let path = root.join(dir).join(relative);
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
        if let Some(parent) = path.parent() {
            crate::core::fs_guard::guarded_create_dir_all(root, parent)?;
        }
        crate::core::mcp_scanner::atomic_write_bytes(&path, &file.bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(file.mode));
        }
    }
    Ok(())
}

/// Puts `files` at `dir`, replacing what is there. The former folder is only
/// deleted once the new one is in place, and put back if writing fails.
fn replace_tree(root: &Path, dir: &str, files: &BTreeMap<String, FileData>) -> Result<(), String> {
    let path = root.join(dir);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    let backup = format!("{dir}.kronn-old-{}", uuid::Uuid::new_v4());
    let had_previous = std::fs::symlink_metadata(&path).is_ok();
    if had_previous {
        std::fs::rename(&path, root.join(&backup))
            .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    }
    match write_tree(root, dir, files) {
        Ok(()) => {
            if had_previous {
                let _ = std::fs::remove_dir_all(root.join(&backup));
            }
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&path);
            if had_previous {
                let _ = std::fs::rename(root.join(&backup), &path);
            }
            Err(error)
        }
    }
}

/// Removes a skill folder file by file, keeping any file that is not exactly
/// what was read, then the folders that emptied.
fn remove_tree(root: &Path, source: &Candidate) {
    let base = root.join(&source.dir);
    let mut directories: Vec<std::path::PathBuf> = Vec::new();
    for (relative, file) in &source.files {
        let path = base.join(relative);
        let unchanged = std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
            && std::fs::read(&path).is_ok_and(|bytes| bytes == file.original);
        if unchanged {
            let _ = std::fs::remove_file(&path);
        }
        let mut parent = path.parent();
        while let Some(directory) = parent.filter(|directory| *directory != base) {
            if !directories.iter().any(|known| known == directory) {
                directories.push(directory.to_path_buf());
            }
            parent = directory.parent();
        }
    }
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
    for directory in directories {
        let _ = std::fs::remove_dir(directory);
    }
    if std::fs::remove_dir(&base).is_ok() {
        resources::remove_empty_parents(root, &base);
    }
}

/// Migrates every slug that is unambiguous, and every conflict `resolutions`
/// names a version for; the rest is left as it is and reported.
pub fn apply(
    root: &Path,
    resolutions: &[SkillMigrationResolution],
) -> Result<SkillMigrationResult, String> {
    let scanned = scan(root);
    let mut result = SkillMigrationResult {
        blocked: scanned.blocked,
        ..SkillMigrationResult::default()
    };
    for (slug, group) in &scanned.groups {
        let versions = versions(group);
        let chosen_hash = if versions.len() == 1 {
            Some(versions[0].0.clone())
        } else {
            resolutions
                .iter()
                .find(|resolution| resolution.slug == *slug)
                .and_then(|resolution| {
                    group
                        .target
                        .iter()
                        .chain(group.sources.iter())
                        .find(|found| found.dir == resolution.keep)
                })
                .map(|found| found.hash.clone())
        };
        let Some(chosen_hash) = chosen_hash else {
            result.unresolved.push(slug.clone());
            continue;
        };
        let chosen = group
            .target
            .iter()
            .chain(group.sources.iter())
            .find(|found| found.hash == chosen_hash)
            .ok_or("the chosen version disappeared")?;
        let target_holds_it = group
            .target
            .as_ref()
            .is_some_and(|target| target.hash == chosen_hash);
        if !target_holds_it {
            replace_tree(root, &target_dir(slug), &chosen.files)?;
        }
        let mut written_from_source = target_holds_it;
        for source in &group.sources {
            if source.hash != chosen_hash {
                result.kept.push(source.dir.clone());
                continue;
            }
            let action = if written_from_source {
                SkillMigrationAction::Duplicate
            } else {
                written_from_source = true;
                SkillMigrationAction::Move
            };
            remove_tree(root, source);
            result.moved.push(move_of(slug, source, action));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill(root: &Path, skills_root: &str, slug: &str, body: &str) {
        write(
            root,
            &format!("{skills_root}/{slug}/SKILL.md"),
            &format!("---\nname: {slug}\ndescription: A skill.\n---\n\n{body}\n"),
        );
    }

    fn resolution(slug: &str, keep: &str) -> SkillMigrationResolution {
        SkillMigrationResolution {
            slug: slug.into(),
            keep: keep.into(),
        }
    }

    #[test]
    fn every_native_root_is_proposed_and_the_plan_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        for (index, source) in SOURCE_ROOTS.iter().enumerate() {
            skill(root.path(), source, &format!("skill-{index}"), "Body.");
        }
        skill(root.path(), ".agents/skills", "already-there", "Body.");
        skill(root.path(), ".vibe/skills", "vibe-only", "Body.");

        let plan = plan(root.path());
        assert_eq!(plan.target_root, ".agents/skills");
        assert!(plan.conflicts.is_empty() && plan.blocked.is_empty());
        assert_eq!(plan.moves.len(), SOURCE_ROOTS.len());
        for (index, source) in SOURCE_ROOTS.iter().enumerate() {
            let entry = &plan.moves[index];
            assert_eq!(entry.source, format!("{source}/skill-{index}"));
            assert_eq!(entry.target, format!(".agents/skills/skill-{index}"));
            assert_eq!(entry.action, SkillMigrationAction::Move);
        }
        assert!(
            plan.moves.iter().all(|entry| entry.slug != "vibe-only"),
            "only the folders the task names are gathered"
        );
        for source in SOURCE_ROOTS {
            assert!(
                root.path().join(source).is_dir(),
                "planning must not move {source}"
            );
        }
        assert!(!root.path().join(".agents/skills/skill-0").exists());
    }

    #[test]
    fn kronn_synced_catalogue_copies_are_not_offered_for_migration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().to_string();
        crate::core::native_files::sync_project_native_files(
            &path,
            &["accessibility".to_string(), "api-design".to_string()],
            &[],
        )
        .unwrap();
        assert!(root
            .path()
            .join(".claude/skills/accessibility/SKILL.md")
            .exists());
        skill(root.path(), ".gemini/skills", "repo-own", "Body.");

        let plan = plan(root.path());
        assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts.len());
        assert_eq!(
            plan.moves
                .iter()
                .map(|entry| entry.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["repo-own"]
        );

        // Once the reader edits Kronn's copy it is theirs, and it is offered.
        write(
            root.path(),
            ".claude/skills/api-design/SKILL.md",
            "---\nname: api-design\ndescription: Mine now.\n---\n\nEdited.\n",
        );
        let plan = super::plan(root.path());
        assert!(
            plan.moves.iter().any(|entry| entry.slug == "api-design")
                || plan
                    .conflicts
                    .iter()
                    .any(|entry| entry.slug == "api-design")
        );
    }

    #[test]
    fn identical_copies_merge_into_one_without_loss_or_duplicate() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Same.");
        skill(root.path(), ".gemini/skills", "review", "Same.");
        skill(root.path(), ".cursor/skills", "review", "Same.");
        write(
            root.path(),
            ".claude/skills/review/scripts/run.sh",
            "echo hi\n",
        );
        write(
            root.path(),
            ".gemini/skills/review/scripts/run.sh",
            "echo hi\n",
        );
        write(
            root.path(),
            ".cursor/skills/review/scripts/run.sh",
            "echo hi\n",
        );

        let plan = plan(root.path());
        assert!(plan.conflicts.is_empty());
        let actions: Vec<_> = plan.moves.iter().map(|entry| entry.action).collect();
        assert_eq!(
            actions,
            vec![
                SkillMigrationAction::Move,
                SkillMigrationAction::Duplicate,
                SkillMigrationAction::Duplicate
            ]
        );

        let result = apply(root.path(), &[]).unwrap();
        assert_eq!(result.moved.len(), 3);
        assert!(result.kept.is_empty() && result.unresolved.is_empty());
        assert!(root.path().join(".agents/skills/review/SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/scripts/run.sh"))
                .unwrap(),
            "echo hi\n",
            "the whole folder moves, not only SKILL.md"
        );
        for source in [".claude/skills", ".gemini/skills", ".cursor/skills"] {
            assert!(
                !root.path().join(source).exists(),
                "{source} is empty once its skill moved"
            );
        }
    }

    #[test]
    fn a_target_that_already_has_the_same_content_only_loses_the_source() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Same.");
        skill(root.path(), ".agents/skills", "review", "Same.");
        let plan = plan(root.path());
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].action, SkillMigrationAction::Duplicate);
        apply(root.path(), &[]).unwrap();
        assert!(!root.path().join(".claude/skills/review").exists());
        assert!(root.path().join(".agents/skills/review/SKILL.md").is_file());
    }

    #[test]
    fn different_contents_are_a_conflict_and_nothing_is_written_without_a_choice() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Claude version.");
        skill(root.path(), ".agents/skills", "review", "Agents version.");
        skill(root.path(), ".claude/skills", "other", "Fine.");

        let plan = plan(root.path());
        assert_eq!(plan.conflicts.len(), 1);
        let conflict = &plan.conflicts[0];
        assert_eq!(conflict.slug, "review");
        assert_eq!(conflict.target, ".agents/skills/review");
        assert_eq!(conflict.versions.len(), 2);
        assert!(conflict.versions[0].at_target);
        assert_eq!(conflict.versions[0].paths, vec![".agents/skills/review"]);
        assert_eq!(conflict.versions[1].paths, vec![".claude/skills/review"]);
        assert_ne!(
            conflict.versions[0].fingerprint,
            conflict.versions[1].fingerprint
        );
        assert_eq!(plan.moves.len(), 1, "the unambiguous skill still moves");

        let result = apply(root.path(), &[]).unwrap();
        assert_eq!(result.unresolved, vec!["review"]);
        assert!(root.path().join(".claude/skills/review/SKILL.md").is_file());
        assert!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md"))
                .unwrap()
                .contains("Agents version.")
        );
        assert!(root.path().join(".agents/skills/other/SKILL.md").is_file());
    }

    #[test]
    fn a_chosen_version_replaces_the_target_and_other_copies_stay() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Claude version.");
        skill(root.path(), ".gemini/skills", "review", "Gemini version.");
        skill(root.path(), ".agents/skills", "review", "Agents version.");
        write(root.path(), ".agents/skills/review/extra.txt", "old extra");

        let result = apply(
            root.path(),
            &[resolution("review", ".claude/skills/review")],
        )
        .unwrap();
        assert_eq!(result.moved.len(), 1);
        assert_eq!(result.moved[0].source, ".claude/skills/review");
        assert_eq!(result.moved[0].action, SkillMigrationAction::Move);
        assert_eq!(result.kept, vec![".gemini/skills/review"]);
        let target = root.path().join(".agents/skills/review");
        assert!(std::fs::read_to_string(target.join("SKILL.md"))
            .unwrap()
            .contains("Claude version."));
        assert!(
            !target.join("extra.txt").exists(),
            "the target becomes exactly the chosen version"
        );
        assert!(root.path().join(".gemini/skills/review/SKILL.md").is_file());
        assert!(!root.path().join(".claude/skills/review").exists());
        let leftovers: Vec<_> = std::fs::read_dir(root.path().join(".agents/skills"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["review"], "no backup is left behind");
    }

    #[test]
    fn keeping_the_targets_own_version_changes_nothing_there() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Claude version.");
        skill(root.path(), ".agents/skills", "review", "Agents version.");
        let result = apply(
            root.path(),
            &[resolution("review", ".agents/skills/review")],
        )
        .unwrap();
        assert!(result.moved.is_empty());
        assert_eq!(result.kept, vec![".claude/skills/review"]);
        assert!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md"))
                .unwrap()
                .contains("Agents version.")
        );
    }

    #[test]
    fn a_choice_that_names_no_listed_folder_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Claude.");
        skill(root.path(), ".agents/skills", "review", "Agents.");
        let result = apply(root.path(), &[resolution("review", "../elsewhere")]).unwrap();
        assert_eq!(result.unresolved, vec!["review"]);
        assert!(root.path().join(".claude/skills/review").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_blocks_its_skill_and_nothing_follows_it() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", "outside the repository");
        skill(root.path(), ".claude/skills", "linked-inside", "Body.");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join(".claude/skills/linked-inside/notes.txt"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path(),
            root.path().join(".claude/skills/linked-folder"),
        )
        .unwrap();
        skill(root.path(), ".claude/skills", "fine", "Body.");
        std::fs::create_dir_all(root.path().join(".gemini")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".gemini/skills")).unwrap();
        write(
            outside.path(),
            "leaked/SKILL.md",
            "---\nname: leaked\n---\n",
        );

        let plan = plan(root.path());
        let blocked: Vec<_> = plan
            .blocked
            .iter()
            .map(|entry| (entry.path.as_str(), entry.reason))
            .collect();
        assert!(blocked.contains(&(
            ".claude/skills/linked-inside",
            SkillMigrationBlockReason::Symlink
        )));
        assert!(blocked.contains(&(".gemini/skills", SkillMigrationBlockReason::Symlink)));
        assert_eq!(
            plan.moves
                .iter()
                .map(|entry| entry.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["fine"],
            "only the clean skill is offered"
        );

        apply(root.path(), &[]).unwrap();
        assert!(root.path().join(".agents/skills/fine/SKILL.md").is_file());
        assert!(!root.path().join(".agents/skills/linked-inside").exists());
        assert!(root
            .path()
            .join(".claude/skills/linked-inside/SKILL.md")
            .is_file());
        assert!(outside.path().join("secret.txt").is_file());
        assert!(!root.path().join(".agents/skills/leaked").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_target_is_never_written_through() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Body.");
        std::fs::create_dir_all(root.path().join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".agents/skills/review"))
            .unwrap();
        let plan = plan(root.path());
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(plan.blocked[0].path, ".agents/skills/review");
        apply(root.path(), &[]).unwrap();
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        assert!(root.path().join(".claude/skills/review/SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_moved_script_keeps_its_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Body.");
        let script = root.path().join(".claude/skills/review/run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        apply(root.path(), &[]).unwrap();
        let moved = root.path().join(".agents/skills/review/run.sh");
        assert_eq!(
            std::fs::metadata(moved).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn the_router_skill_and_an_occupied_target_are_left_alone() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "kronn", "Not the router.");
        skill(root.path(), ".claude/skills", "review", "Body.");
        write(
            root.path(),
            ".agents/skills/review/README.md",
            "not a skill",
        );
        let plan = plan(root.path());
        assert!(plan.moves.is_empty());
        let reasons: Vec<_> = plan.blocked.iter().map(|entry| entry.reason).collect();
        assert!(reasons.contains(&SkillMigrationBlockReason::ReservedSlug));
        assert!(reasons.contains(&SkillMigrationBlockReason::TargetOccupied));
        apply(root.path(), &[]).unwrap();
        assert!(root.path().join(".claude/skills/kronn/SKILL.md").is_file());
        assert!(root.path().join(".claude/skills/review/SKILL.md").is_file());
    }

    #[test]
    fn a_folder_that_changed_since_it_was_read_keeps_its_new_file() {
        let root = tempfile::tempdir().unwrap();
        skill(root.path(), ".claude/skills", "review", "Body.");
        let scanned = scan(root.path());
        let source = &scanned.groups["review"].sources[0];
        write(
            root.path(),
            ".claude/skills/review/late.txt",
            "added after the scan",
        );
        std::fs::write(
            root.path().join(".claude/skills/review/SKILL.md"),
            "edited after the scan",
        )
        .unwrap();
        remove_tree(root.path(), source);
        assert!(root.path().join(".claude/skills/review/late.txt").is_file());
        assert!(root.path().join(".claude/skills/review/SKILL.md").is_file());
    }
}
