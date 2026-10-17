//! Paths that are generated, vendored or framework scaffolding: a detector
//! signal there is not actionable (the fix is upstream), so the detector walk
//! skips them instead of forcing a TD in Step 8 (KT-1323).

use std::path::Path;

/// Directory names that hold generated or third-party output.
const GENERATED_DIRS: &[&str] = &[
    "generated",
    "__generated__",
    ".generated",
    "bower_components",
    "coverage",
    ".nuxt",
    ".svelte-kit",
    ".angular",
    "storybook-static",
    "Pods",
];

/// Substrings of a file name that mark a generated or minified artifact.
const GENERATED_NAME_PARTS: &[&str] = &[
    ".min.js",
    ".min.css",
    ".bundle.js",
    ".generated.",
    ".g.dart",
    ".pb.go",
    "_pb2.py",
    ".designer.cs",
];

/// Framework installer scaffolding, by project-relative path (Symfony 2/3
/// `web/config.php` requirements check and dev front controller).
const SCAFFOLDING_FILES: &[&str] = &["web/config.php", "web/app_dev.php"];

/// The skip rules of one project: the built-in lists plus the
/// `linguist-generated` / `linguist-vendored` patterns of its root
/// `.gitattributes`.
#[derive(Debug, Default, Clone)]
pub struct GeneratedPaths {
    patterns: Vec<String>,
    disabled: bool,
}

impl GeneratedPaths {
    /// Skips nothing.
    pub fn none() -> Self {
        Self {
            disabled: true,
            ..Self::default()
        }
    }

    pub fn load(root: &Path) -> Self {
        let content = std::fs::read_to_string(root.join(".gitattributes")).unwrap_or_default();
        Self::from_gitattributes(&content)
    }

    pub fn from_gitattributes(content: &str) -> Self {
        let patterns = content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let mut parts = line.split_whitespace();
                let pattern = parts.next()?;
                parts
                    .any(|attr| {
                        matches!(
                            attr,
                            "linguist-generated"
                                | "linguist-generated=true"
                                | "linguist-vendored"
                                | "linguist-vendored=true"
                        )
                    })
                    .then(|| pattern.to_string())
            })
            .collect();
        Self {
            patterns,
            disabled: false,
        }
    }

    /// Whether the project-relative path `rel` (`/`-separated) is generated,
    /// vendored or scaffolding. Also true for a directory whose path matches.
    pub fn is_generated(&self, rel: &str) -> bool {
        let rel = rel.trim_matches('/');
        if self.disabled || rel.is_empty() {
            return false;
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if rel.split('/').any(|seg| GENERATED_DIRS.contains(&seg)) {
            return true;
        }
        let lower = name.to_ascii_lowercase();
        if GENERATED_NAME_PARTS.iter().any(|part| lower.contains(part)) {
            return true;
        }
        if SCAFFOLDING_FILES.contains(&rel) {
            return true;
        }
        self.patterns.iter().any(|p| attribute_matches(p, rel))
    }
}

/// `.gitattributes` pattern against a relative path: no inner `/` matches a
/// base name at any depth, otherwise the path is anchored at the root; a
/// pattern naming a directory also covers everything under it.
fn attribute_matches(pattern: &str, rel: &str) -> bool {
    let pat = pattern.trim_start_matches('/').trim_end_matches('/');
    if pat.is_empty() {
        return false;
    }
    let segments: Vec<&str> = rel.split('/').collect();
    if !pattern.trim_end_matches('/').contains('/') {
        // Any single segment (file or enclosing directory) may match.
        return segments.iter().any(|seg| glob(pat, seg));
    }
    // Anchored: the pattern may match the path or any of its directory prefixes.
    (1..=segments.len()).any(|n| glob(pat, &segments[..n].join("/")))
}

/// `*` (not across `/`), `**` (across `/`) and `?` glob.
fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                let double = p.get(1) == Some(&b'*');
                let rest = if double { &p[2..] } else { &p[1..] };
                // `**/` also matches zero directories.
                if double && rest.first() == Some(&b'/') && go(&rest[1..], t) {
                    return true;
                }
                (0..=t.len()).any(|i| (double || !t[..i].contains(&b'/')) && go(rest, &t[i..]))
            }
            Some(b'?') => t.first().is_some_and(|c| *c != b'/') && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_generated_dirs_files_and_scaffolding_are_skipped() {
        let g = GeneratedPaths::default();
        assert!(g.is_generated("sass/generated/icons.html"));
        assert!(g.is_generated("web/config.php"));
        assert!(g.is_generated("public/app.min.js"));
        assert!(!g.is_generated("src/Controller/HomeController.php"));
        assert!(!g.is_generated("web/index.php"));
        assert!(!g.is_generated("templates/poc.html.twig"));
    }

    #[test]
    fn gitattributes_generated_and_vendored_patterns_are_honoured() {
        let g = GeneratedPaths::from_gitattributes(
            "# comment\n*.snap linguist-generated\n/lib/ext/** linguist-vendored=true\n\
             docs/api/ linguist-generated\nkeep.php -linguist-generated\nother.php text\n",
        );
        assert!(g.is_generated("a/b/x.snap"));
        assert!(g.is_generated("lib/ext/deep/y.js"));
        assert!(g.is_generated("docs/api/index.html"));
        assert!(!g.is_generated("keep.php"));
        assert!(!g.is_generated("other.php"));
        assert!(!g.is_generated("src/lib/ext/y.js"));
    }
}
