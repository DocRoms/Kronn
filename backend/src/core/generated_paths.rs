//! Paths that are generated, vendored or framework scaffolding: a hygiene
//! detector signal there is not actionable (the fix is upstream), so those
//! walks skip them instead of forcing a TD in Step 8 (KT-1323).
//!
//! Security-dimension detectors and the Docker/CI inventory never use this: a
//! `.gitattributes` line must not be able to hide a secret `ARG` or a risky CI
//! trigger.

use std::path::Path;

/// Directory names that are generated output wherever they sit.
const ANY_DEPTH_DIRS: &[&str] = &[
    "generated",
    "__generated__",
    ".generated",
    "bower_components",
    ".nuxt",
    ".svelte-kit",
    ".angular",
];

/// Build-output directories that only count at the project root: `src/coverage/`
/// or `lib/Pods/` is ordinary code.
const ROOT_DIRS: &[&str] = &["coverage", "storybook-static", "Pods"];

/// Mobile projects keep CocoaPods one level down.
const POD_PARENTS: &[&str] = &["ios", "macos"];

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
/// `web/config.php` requirements check).
const SCAFFOLDING_FILES: &[&str] = &["web/config.php"];

/// Bounds on a hostile `.gitattributes`.
const MAX_RULES: usize = 1_000;
const MAX_PATTERN_LEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attr {
    Generated,
    Vendored,
}

/// One `.gitattributes` line's verdict on one attribute.
#[derive(Debug, Clone)]
struct Rule {
    pattern: String,
    attr: Attr,
    set: bool,
}

/// The skip rules of one project: the built-in lists plus the
/// `linguist-generated` / `linguist-vendored` attributes of its root
/// `.gitattributes`.
#[derive(Debug, Default, Clone)]
pub struct GeneratedPaths {
    rules: Vec<Rule>,
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
        let mut rules = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next() else {
                continue;
            };
            if pattern.len() > MAX_PATTERN_LEN {
                continue;
            }
            for token in parts {
                let (name, set) = match token.split_once('=') {
                    Some((name, value)) => (name, !value.eq_ignore_ascii_case("false")),
                    None => match token.strip_prefix('-') {
                        Some(name) => (name, false),
                        None => (token, true),
                    },
                };
                let attr = match name {
                    "linguist-generated" => Attr::Generated,
                    "linguist-vendored" => Attr::Vendored,
                    _ => continue,
                };
                if rules.len() < MAX_RULES {
                    rules.push(Rule {
                        pattern: pattern.to_string(),
                        attr,
                        set,
                    });
                }
            }
        }
        Self {
            rules,
            disabled: false,
        }
    }

    /// Whether the project-relative path `rel` (`/`-separated) is generated,
    /// vendored or scaffolding. A directory is only pruned by the built-in
    /// directory rules: `.gitattributes` decides per file, so a later
    /// `-linguist-vendored` line can still re-include a file under it.
    pub fn is_generated(&self, rel: &str, is_dir: bool) -> bool {
        let rel = rel.trim_matches('/');
        if self.disabled || rel.is_empty() {
            return false;
        }
        let segments: Vec<&str> = rel.split('/').collect();
        if in_generated_dir(&segments, is_dir) {
            return true;
        }
        if is_dir {
            return false;
        }
        let name = segments.last().copied().unwrap_or(rel);
        let lower = name.to_ascii_lowercase();
        if GENERATED_NAME_PARTS.iter().any(|part| lower.contains(part)) {
            return true;
        }
        if SCAFFOLDING_FILES.contains(&rel) {
            return true;
        }
        [Attr::Generated, Attr::Vendored]
            .into_iter()
            .any(|attr| self.attribute_set(attr, rel, &segments))
    }

    /// Last matching rule wins, as in git; `-attr` / `attr=false` unset it.
    fn attribute_set(&self, attr: Attr, rel: &str, segments: &[&str]) -> bool {
        self.rules
            .iter()
            .rev()
            .find(|r| r.attr == attr && attribute_matches(&r.pattern, rel, segments))
            .is_some_and(|r| r.set)
    }
}

/// Whether the path sits in (or, for a directory, is) a generated directory.
fn in_generated_dir(segments: &[&str], is_dir: bool) -> bool {
    let dirs = if is_dir {
        segments
    } else {
        &segments[..segments.len().saturating_sub(1)]
    };
    if dirs.iter().any(|seg| ANY_DEPTH_DIRS.contains(seg)) {
        return true;
    }
    match dirs {
        [first, ..] if ROOT_DIRS.contains(first) => true,
        [parent, "Pods", ..] => POD_PARENTS.contains(parent),
        _ => false,
    }
}

/// `.gitattributes` pattern against a relative path: no inner `/` matches a
/// base name at any depth, otherwise the path is anchored at the root; a
/// pattern naming a directory also covers everything under it.
fn attribute_matches(pattern: &str, rel: &str, segments: &[&str]) -> bool {
    let pat = pattern.trim_start_matches('/').trim_end_matches('/');
    if pat.is_empty() {
        return false;
    }
    if !pattern.trim_end_matches('/').contains('/') {
        // Any single segment (file or enclosing directory) may match.
        return segments.iter().any(|seg| glob(pat, seg));
    }
    // Anchored: the pattern may match the path or any of its directory prefixes.
    let mut end = 0usize;
    for (i, seg) in segments.iter().enumerate() {
        end += seg.len() + usize::from(i > 0);
        if glob(pat, &rel[..end]) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Lit(u8),
    /// `?`: one character other than `/`.
    Any,
    /// `*`: any run of characters other than `/`.
    Star,
    /// `**`: any run of characters, `/` included.
    DoubleStar,
    /// `**/`: zero or more whole directories.
    DoubleStarSlash,
}

fn tokenize(pattern: &str) -> Vec<Token> {
    let bytes = pattern.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'*' => {
                let start = i;
                while i < bytes.len() && bytes[i] == b'*' {
                    i += 1;
                }
                if i - start >= 2 {
                    if bytes.get(i) == Some(&b'/') {
                        i += 1;
                        out.push(Token::DoubleStarSlash);
                    } else {
                        out.push(Token::DoubleStar);
                    }
                } else {
                    out.push(Token::Star);
                }
            }
            b'?' => {
                out.push(Token::Any);
                i += 1;
            }
            c => {
                out.push(Token::Lit(c));
                i += 1;
            }
        }
    }
    out
}

/// `*` (not across `/`), `**` (across `/`) and `?` glob. Dynamic programming
/// over pattern tokens: O(pattern × text), so a hostile pattern such as
/// `*a*a*a*a*b` cannot backtrack exponentially.
fn glob(pattern: &str, text: &str) -> bool {
    let t = text.as_bytes();
    // reach[i]: the tokens consumed so far can match exactly t[..i].
    let mut reach = vec![false; t.len() + 1];
    reach[0] = true;
    for token in tokenize(pattern) {
        let mut next = vec![false; t.len() + 1];
        match token {
            Token::Lit(c) => {
                for i in 0..t.len() {
                    next[i + 1] = reach[i] && t[i] == c;
                }
            }
            Token::Any => {
                for i in 0..t.len() {
                    next[i + 1] = reach[i] && t[i] != b'/';
                }
            }
            Token::Star => {
                for i in 0..=t.len() {
                    next[i] = reach[i] || (i > 0 && next[i - 1] && t[i - 1] != b'/');
                }
            }
            Token::DoubleStar => {
                for i in 0..=t.len() {
                    next[i] = reach[i] || (i > 0 && next[i - 1]);
                }
            }
            Token::DoubleStarSlash => {
                // Zero directories, or anything up to and including a `/`.
                let mut any_before = false;
                for i in 0..=t.len() {
                    next[i] = reach[i] || (i > 0 && t[i - 1] == b'/' && any_before);
                    any_before |= reach[i];
                }
            }
        }
        if !next.iter().any(|r| *r) {
            return false;
        }
        reach = next;
    }
    reach[t.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(g: &GeneratedPaths, rel: &str) -> bool {
        g.is_generated(rel, false)
    }

    #[test]
    fn built_in_generated_dirs_files_and_scaffolding_are_skipped() {
        let g = GeneratedPaths::default();
        assert!(file(&g, "sass/generated/icons.html"));
        assert!(file(&g, "web/config.php"));
        assert!(file(&g, "public/app.min.js"));
        assert!(!file(&g, "src/Controller/HomeController.php"));
        assert!(!file(&g, "web/index.php"));
        assert!(!file(&g, "templates/poc.html.twig"));
        // The exposed debug front controller stays visible.
        assert!(!file(&g, "web/app_dev.php"));
    }

    #[test]
    fn build_output_dirs_only_count_at_their_root() {
        let g = GeneratedPaths::default();
        assert!(file(&g, "coverage/lcov-report/index.html"));
        assert!(g.is_generated("coverage", true));
        assert!(file(&g, "Pods/Foo/foo.h"));
        assert!(file(&g, "ios/Pods/Foo/foo.h"));
        assert!(!file(&g, "src/coverage/report.rs"));
        assert!(!g.is_generated("src/coverage", true));
        assert!(!file(&g, "lib/Pods/handler.rb"));
        assert!(!file(&g, "app/storybook-static/page.html"));
    }

    #[test]
    fn gitattributes_generated_and_vendored_patterns_are_honoured() {
        let g = GeneratedPaths::from_gitattributes(
            "# comment\n*.snap linguist-generated\n/lib/ext/** linguist-vendored=true\n\
             docs/api/ linguist-generated\nkeep.php -linguist-generated\nother.php text\n",
        );
        assert!(file(&g, "a/b/x.snap"));
        assert!(file(&g, "lib/ext/deep/y.js"));
        assert!(file(&g, "docs/api/index.html"));
        assert!(!file(&g, "keep.php"));
        assert!(!file(&g, "other.php"));
        assert!(!file(&g, "src/lib/ext/y.js"));
    }

    #[test]
    fn gitattributes_last_match_wins_and_negation_re_includes() {
        let g = GeneratedPaths::from_gitattributes(
            "vendor/** linguist-vendored\nvendor/keep.js -linguist-vendored\n\
             *.gen.txt linguist-generated\n*.gen.txt linguist-generated=false\n\
             late.txt text\nlate.txt linguist-generated\n",
        );
        assert!(file(&g, "vendor/lib.js"));
        assert!(!file(&g, "vendor/keep.js"));
        assert!(!file(&g, "a.gen.txt"));
        assert!(file(&g, "late.txt"));
        // A negation is per attribute: it does not undo the other one.
        let both = GeneratedPaths::from_gitattributes(
            "x/** linguist-generated\nx/** linguist-vendored\nx/a -linguist-vendored\n",
        );
        assert!(file(&both, "x/a"));
        // A directory is not pruned by an attribute, so a re-included file is reached.
        assert!(!g.is_generated("vendor", true));
    }

    #[test]
    fn glob_is_linear_on_hostile_patterns() {
        let hostile = "*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b";
        let text = "a".repeat(200);
        assert!(!glob(hostile, &text));
        assert!(glob("*a*a*b", "xaxab"));
        assert!(glob("a/**/b", "a/b"));
        assert!(glob("a/**/b", "a/x/y/b"));
        assert!(glob("**/x.js", "x.js"));
        assert!(glob("**/x.js", "a/b/x.js"));
        assert!(!glob("a/*/b", "a/x/y/b"));
        assert!(glob("?.js", "a.js"));
        assert!(!glob("?.js", "/.js"));
        // A hostile .gitattributes line is also bounded by the pattern cap.
        let long = format!("{} linguist-generated\n", "*a".repeat(MAX_PATTERN_LEN));
        assert!(!file(&GeneratedPaths::from_gitattributes(&long), "a"));
    }
}
