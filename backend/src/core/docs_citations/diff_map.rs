//! Old-to-new line mapping from a zero-context Git diff.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    old_start: usize,
    old_len: usize,
    new_len: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FileMap {
    hunks: Vec<Hunk>,
}

impl FileMap {
    fn parse(diff: &str) -> Result<Self, String> {
        let mut hunks = Vec::new();
        for line in diff.lines().filter(|line| line.starts_with("@@ ")) {
            let mut fields = line.split_whitespace();
            if fields.next() != Some("@@") {
                continue;
            }
            let old = fields
                .next()
                .and_then(|field| field.strip_prefix('-'))
                .ok_or_else(|| format!("invalid Git hunk header: {line}"))?;
            let new = fields
                .next()
                .and_then(|field| field.strip_prefix('+'))
                .ok_or_else(|| format!("invalid Git hunk header: {line}"))?;
            let (old_start, old_len) = parse_range(old, line)?;
            let (_, new_len) = parse_range(new, line)?;
            if hunks
                .last()
                .is_some_and(|previous: &Hunk| old_start < previous.old_start)
            {
                return Err(format!("Git hunks are not ordered: {line}"));
            }
            hunks.push(Hunk {
                old_start,
                old_len,
                new_len,
            });
        }
        Ok(Self { hunks })
    }

    /// Maps a 1-based line from the old side to the new side. A line changed
    /// or deleted by a hunk deliberately has no mapping.
    fn line(&self, old_line: usize) -> Option<usize> {
        if old_line == 0 {
            return None;
        }
        let mut delta = 0isize;
        for hunk in &self.hunks {
            if hunk.old_len == 0 {
                if old_line > hunk.old_start {
                    delta += hunk.new_len as isize;
                }
                continue;
            }
            if old_line < hunk.old_start {
                break;
            }
            let old_end = hunk.old_start.checked_add(hunk.old_len - 1)?;
            if old_line <= old_end {
                return None;
            }
            delta += hunk.new_len as isize - hunk.old_len as isize;
        }
        old_line.checked_add_signed(delta)
    }

    fn span(&self, old: (usize, usize)) -> Option<(usize, usize)> {
        let mapped = (self.line(old.0)?, self.line(old.1)?);
        (mapped.0 <= mapped.1).then_some(mapped)
    }
}

fn parse_range(field: &str, header: &str) -> Result<(usize, usize), String> {
    let (start, len) = field.split_once(',').unwrap_or((field, "1"));
    let start = start
        .parse()
        .map_err(|_| format!("invalid Git hunk range in: {header}"))?;
    let len = len
        .parse()
        .map_err(|_| format!("invalid Git hunk range in: {header}"))?;
    Ok((start, len))
}

/// Lazily loads one `git diff -U0 <base> HEAD -- <file>` per cited file.
pub(super) struct DiffMap<'a> {
    root: &'a Path,
    base: &'a str,
    files: HashMap<PathBuf, FileMap>,
}

impl<'a> DiffMap<'a> {
    pub(super) fn new(root: &'a Path, base: &'a str) -> Self {
        Self {
            root,
            base,
            files: HashMap::new(),
        }
    }

    pub(super) fn span(
        &mut self,
        file: &Path,
        old: (usize, usize),
    ) -> Result<Option<(usize, usize)>, String> {
        if !self.files.contains_key(file) {
            let output = crate::core::cmd::git_cmd()
                .args(["diff", "--unified=0", "--no-ext-diff", "--no-color"])
                .arg(self.base)
                .arg("HEAD")
                .arg("--")
                .arg(file)
                .current_dir(self.root)
                .output()
                .map_err(|error| format!("cannot run git diff: {error}"))?;
            if !output.status.success() {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(if detail.is_empty() {
                    format!("git diff {} HEAD failed", self.base)
                } else {
                    format!("git diff {} HEAD failed: {detail}", self.base)
                });
            }
            let parsed = FileMap::parse(&String::from_utf8_lossy(&output.stdout))?;
            self.files.insert(file.to_path_buf(), parsed);
        }
        Ok(self.files.get(file).and_then(|map| map.span(old)))
    }
}

/// Merge-base with the repository's default branch. A non-Git repository or
/// one without a resolvable default branch has no diff fallback.
pub(super) fn default_merge_base(root: &Path) -> Result<Option<String>, String> {
    let Some(target) = crate::core::project_profile::default_branch_ref(root) else {
        return Ok(None);
    };
    let output = crate::core::cmd::git_cmd()
        .args(["merge-base"])
        .arg(&target)
        .arg("HEAD")
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git merge-base: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("git merge-base {target} HEAD failed")
        } else {
            format!("git merge-base {target} HEAD failed: {detail}")
        });
    }
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if base.is_empty() {
        Err(format!("git merge-base {target} HEAD returned no commit"))
    } else {
        Ok(Some(base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_surviving_lines_across_insert_delete_and_replace_hunks() {
        let map = FileMap::parse(
            "@@ -2,0 +3,2 @@\n+new\n+new\n\
             @@ -5,2 +7,0 @@\n-old\n-old\n\
             @@ -9 +9,3 @@\n-old\n+new\n+new\n+new\n",
        )
        .unwrap();
        assert_eq!(map.line(1), Some(1));
        assert_eq!(map.line(2), Some(2));
        assert_eq!(map.line(3), Some(5));
        assert_eq!(map.line(5), None);
        assert_eq!(map.line(6), None);
        assert_eq!(map.line(7), Some(7));
        assert_eq!(map.line(9), None);
        assert_eq!(map.line(10), Some(12));
    }

    #[test]
    fn a_range_maps_only_when_both_endpoints_survive() {
        let map = FileMap::parse("@@ -4,2 +4,3 @@\n-old\n-old\n+new\n+new\n+new\n").unwrap();
        assert_eq!(map.span((3, 6)), Some((3, 7)));
        assert_eq!(map.span((4, 6)), None);
        assert_eq!(map.span((3, 5)), None);
    }
}
