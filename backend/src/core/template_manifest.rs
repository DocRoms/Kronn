use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::{docs_location::DocsRoot, fs_guard, kronn_state, scanner};
use crate::models::PlaceholderDoc;

const SPEC_PATH: &str = "conventions/agents-md-format-v1.md";

// Exact bundled spec revisions, verified from git history before manifest tracking.
const LEGACY_SPEC_SHA256: &[&str] = &[
    "131ccfc12d0c0f28cbad421759d263905c52ed40afa6ee2a5f571f0b4ea1c6b8", // af450c5f3
    "1e3edb0d935c2404ef4bbf3da00d10a1f84c2a8294f10f40188cde6ee452f4b5", // 22ad2b77d
    "4bacb8eed77ffcef34a86770daa0f1f7e7178fbc77f3ca404eed20985ed741df", // 6e298b0f3
    "6213475e9909a40f164b1ffae9596b4054ef76bcf2219f0cceee95ab444adfbb", // a65724fa0
    "ec409461bd1900dab3d89e412ad177f38e65f25151d33671aa382e308cdd0dc8", // 6ff6f93fc
    "9e5e3efced05bb6ac681b46eb761a8b48fe5e78a18e98810bba294a1cbc9f7dd", // 5cba9b994
    "c51a1c6a9cfbd6732c0b3fe87c7a29c1f546abdb87fbffa8e1f5e2cb6669aa3a", // 66ce8fb48
    "392b2001c9fe86a543dc35717b44098bef68b0cb511943ac4092b77b50489a02", // 1fa3fe163
    "d0d39859a2cf31968de74b1737bd2c47d0bb6e587c3b6f5e3371682ce7a448de", // 913658a85
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemplateManifest {
    pub version: Option<String>,
    /// Paths relative to the docs root, hashes of upstream template bytes.
    pub files: BTreeMap<String, String>,
    /// The last spec bytes Kronn installed; never inferred from a filename.
    #[serde(default)]
    pub installed_spec_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TemplateRename {
    pub from: String,
    pub to: String,
    /// An occupied destination is preserved and needs human reconciliation.
    pub conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TemplateUpdate {
    pub available: bool,
    pub installed_version: Option<String>,
    pub current_version: Option<String>,
    pub missing_files: Vec<PlaceholderDoc>,
    pub renames: Vec<TemplateRename>,
    /// Removed upstream paths whose successor cannot be proven uniquely.
    pub unresolved_files: Vec<String>,
    pub spec_modified: bool,
}

fn version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("<!-- kronn:doc-version=\"")?
            .strip_suffix("\" -->")
            .map(str::to_owned)
    })
}

pub fn snapshot(template_docs: &Path) -> Result<TemplateManifest, String> {
    let files = crate::core::docs_merge::load_template(template_docs)?;
    Ok(TemplateManifest {
        version: files
            .iter()
            .find(|f| f.rel == "AGENTS.md")
            .and_then(|f| std::str::from_utf8(&f.bytes).ok())
            .and_then(version),
        files: files
            .into_iter()
            .map(|f| (f.rel, crate::core::docs_merge::sha256_hex(&f.bytes)))
            .collect(),
        installed_spec_sha256: None,
    })
}

pub fn check(project: &Path, template_docs: &Path) -> Result<TemplateUpdate, String> {
    let current = snapshot(template_docs)?;
    let root = DocsRoot::for_project(project)?;
    let docs = scanner::detect_docs_dir(project);
    let previous = kronn_state::read(project).and_then(|s| s.template);
    let chain = crate::api::audit::steps_for(crate::models::AuditKind::Full, &root);
    let path_of = |rel: &str| {
        docs.join(rel)
            .strip_prefix(project)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let missing_files = current
        .files
        .keys()
        .filter(|rel| std::fs::symlink_metadata(docs.join(rel)).is_err())
        .map(|rel| {
            let path = path_of(rel);
            let step = chain
                .iter()
                .position(|s| {
                    s.target_file == path && crate::api::audit::partial_selectable(s, &root)
                })
                .map(|i| i + 1);
            PlaceholderDoc { path, step }
        })
        .collect::<Vec<_>>();
    let mut renames = Vec::new();
    let mut unresolved_files = Vec::new();
    if let Some(old) = &previous {
        for (from, hash) in old
            .files
            .iter()
            .filter(|(p, _)| !current.files.contains_key(*p))
        {
            // State is project input: it may not smuggle a path outside docs/.
            let src = docs.join(from);
            fs_guard::assert_contained_no_symlink(&docs, &src)?;
            if !src.is_file() {
                continue;
            }
            let candidates: Vec<_> = current
                .files
                .iter()
                .filter(|(to, h)| !old.files.contains_key(*to) && *h == hash)
                .collect();
            let unique_source = old
                .files
                .iter()
                .filter(|(p, h)| !current.files.contains_key(*p) && *h == hash)
                .count()
                == 1;
            if candidates.len() == 1 && unique_source {
                let to = candidates[0].0;
                renames.push(TemplateRename {
                    from: path_of(from),
                    to: path_of(to),
                    conflict: std::fs::symlink_metadata(docs.join(to)).is_ok(),
                });
            } else {
                unresolved_files.push(path_of(from));
            }
        }
    }
    let spec = docs.join(SPEC_PATH);
    fs_guard::assert_contained_no_symlink(project, &spec)?;
    let spec_hash = std::fs::read(&spec)
        .ok()
        .map(|bytes| crate::core::docs_merge::sha256_hex(&bytes));
    let current_spec = current.files.get(SPEC_PATH);
    let spec_modified = spec_hash.as_ref().is_some_and(|hash| {
        Some(hash) != current_spec
            && previous
                .as_ref()
                .and_then(|p| p.installed_spec_sha256.as_ref())
                != Some(hash)
            && !LEGACY_SPEC_SHA256.contains(&hash.as_str())
    });
    let installed_version = previous
        .as_ref()
        .and_then(|p| p.version.clone())
        .or_else(|| {
            std::fs::read_to_string(scanner::detect_docs_entry(project))
                .ok()
                .and_then(|s| version(&s))
        });
    Ok(TemplateUpdate {
        available: previous
            .as_ref()
            .is_none_or(|p| p.version != current.version || p.files != current.files)
            || !missing_files.is_empty()
            || !renames.is_empty(),
        installed_version,
        current_version: current.version,
        missing_files,
        renames,
        unresolved_files,
        spec_modified,
    })
}

/// Move only a uniquely proven upstream rename, keeping the project's bytes.
/// Hard-link publication refuses an occupied destination rather than overwriting it.
pub fn apply_renames(project: &Path, template_docs: &Path) -> Result<(), String> {
    apply_renames_in(project, template_docs, None)
}

pub fn apply_renames_in(
    project: &Path,
    template_docs: &Path,
    allowed: Option<&[String]>,
) -> Result<(), String> {
    for rename in check(project, template_docs)?
        .renames
        .into_iter()
        .filter(|r| !r.conflict && allowed.is_none_or(|paths| paths.contains(&r.to)))
    {
        let from = project.join(&rename.from);
        let to = project.join(&rename.to);
        fs_guard::assert_contained_no_symlink(project, &from)?;
        fs_guard::guarded_create_dir_all(project, to.parent().ok_or("missing rename parent")?)?;
        fs_guard::assert_contained_no_symlink(project, &to)?;
        std::fs::hard_link(&from, &to)
            .map_err(|e| format!("rename {} to {}: {e}", rename.from, rename.to))?;
        std::fs::remove_file(&from).map_err(|e| format!("remove renamed {}: {e}", rename.from))?;
    }
    Ok(())
}

/// Refresh the bundled spec only with proof that its current bytes are ours.
pub fn record_install(project: &Path, template_docs: &Path) -> Result<(), String> {
    let mut state = kronn_state::load_for_mutation(project)?;
    let mut manifest = snapshot(template_docs)?;
    let spec = scanner::detect_docs_dir(project).join(SPEC_PATH);
    fs_guard::assert_contained_no_symlink(project, &spec)?;
    let expected = state
        .template
        .as_ref()
        .and_then(|p| p.installed_spec_sha256.as_ref());
    let bytes = crate::core::anti_halluc::SPEC_AGENTS_MD_V1.as_bytes();
    let hash = crate::core::docs_merge::sha256_hex(bytes);
    let mtime = crate::core::mcp_scanner::read_target_mtime(&spec);
    let actual = match std::fs::read(&spec) {
        Ok(bytes) => Some(crate::core::docs_merge::sha256_hex(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", spec.display())),
    };
    if actual.is_none() {
        if fs_guard::guarded_write_new(project, &spec, bytes)? {
            manifest.installed_spec_sha256 = Some(hash);
        }
    } else if actual.as_ref() == Some(&hash) {
        manifest.installed_spec_sha256 = Some(hash);
    } else if actual.as_ref() == expected
        || actual
            .as_ref()
            .is_some_and(|h| LEGACY_SPEC_SHA256.contains(&h.as_str()))
    {
        crate::core::mcp_scanner::atomic_write_checked(
            &spec,
            crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
            mtime,
        )
        .map_err(|e| format!("spec update refused: {e:?}"))?;
        manifest.installed_spec_sha256 = Some(hash);
    } else {
        manifest.installed_spec_sha256 = expected.cloned();
    }
    retain_unresolved(project, state.template.as_ref(), &mut manifest)?;
    state.template = Some(manifest);
    kronn_state::write(project, &mut state)
}

fn retain_unresolved(
    project: &Path,
    previous: Option<&TemplateManifest>,
    manifest: &mut TemplateManifest,
) -> Result<(), String> {
    // Keep unresolved removed entries: a later inspection must still explain them.
    if let Some(old) = previous {
        for (path, hash) in &old.files {
            let old_path = scanner::detect_docs_dir(project).join(path);
            fs_guard::assert_contained_no_symlink(&scanner::detect_docs_dir(project), &old_path)?;
            if !manifest.files.contains_key(path) && old_path.exists() {
                manifest.files.insert(path.clone(), hash.clone());
            }
        }
    }
    Ok(())
}

/// Recording audit metadata must not install files or adopt modified spec bytes.
pub fn for_audit(
    project: &Path,
    template_docs: &Path,
    previous: Option<&TemplateManifest>,
) -> Result<TemplateManifest, String> {
    let mut manifest = snapshot(template_docs)?;
    manifest.installed_spec_sha256 = previous.and_then(|p| p.installed_spec_sha256.clone());
    retain_unresolved(project, previous, &mut manifest)?;
    Ok(manifest)
}

#[cfg(test)]
#[path = "template_manifest_test.rs"]
mod tests;
