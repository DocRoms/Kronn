//! Old-to-new line mapping from a zero-context Git diff.

use std::collections::HashMap;
use std::ffi::OsStr;
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

    /// [`Self::line`], but a line a hunk changed or deleted lands where the
    /// hunk starts on the new side instead of having no mapping: the closest
    /// position that still says where the old line was.
    fn line_nearest(&self, old_line: usize) -> Option<usize> {
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
                return hunk.old_start.checked_add_signed(delta);
            }
            delta += hunk.new_len as isize - hunk.old_len as isize;
        }
        old_line.checked_add_signed(delta)
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

/// Lazily loads one `git diff -U0 <base> HEAD -- <file>` per cited file, or
/// `git diff -U0 <base> -- <file>` (the working tree) for a merge in progress.
pub(super) struct DiffMap<'a> {
    root: &'a Path,
    base: &'a str,
    to_worktree: bool,
    files: HashMap<PathBuf, FileMap>,
}

impl<'a> DiffMap<'a> {
    pub(super) fn new(root: &'a Path, base: &'a str) -> Self {
        Self {
            root,
            base,
            to_worktree: false,
            files: HashMap::new(),
        }
    }

    /// Maps lines of `base` onto the working tree, which is where an
    /// uncommitted merge result lives.
    pub(super) fn to_worktree(root: &'a Path, base: &'a str) -> Self {
        Self {
            to_worktree: true,
            ..Self::new(root, base)
        }
    }

    fn file(&mut self, file: &Path) -> Result<&FileMap, String> {
        if !self.files.contains_key(file) {
            // KT-1595: an agent may have configured the repository; no textconv,
            // external diff or other program its configuration names runs.
            let mut args: Vec<&OsStr> = [
                "--literal-pathspecs",
                "diff",
                "--unified=0",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=all",
            ]
            .map(OsStr::new)
            .to_vec();
            args.push(OsStr::new(self.base));
            if !self.to_worktree {
                args.push(OsStr::new("HEAD"));
            }
            args.extend([OsStr::new("--"), file.as_os_str()]);
            let stdout =
                crate::core::cmd::agent_git_output(self.root, &args, &[]).ok_or_else(|| {
                    let target = if self.to_worktree { "" } else { " HEAD" };
                    format!("git diff {}{target} failed", self.base)
                })?;
            let parsed = FileMap::parse(&String::from_utf8_lossy(&stdout))?;
            self.files.insert(file.to_path_buf(), parsed);
        }
        Ok(&self.files[file])
    }

    pub(super) fn span(
        &mut self,
        file: &Path,
        old: (usize, usize),
    ) -> Result<Option<(usize, usize)>, String> {
        Ok(self.file(file)?.span(old))
    }

    /// Where the old span starts on the new side, even when a hunk changed
    /// that very line; `None` for a file the diff did not touch at all.
    pub(super) fn nearest_start(
        &mut self,
        file: &Path,
        old: (usize, usize),
    ) -> Result<Option<usize>, String> {
        let map = self.file(file)?;
        Ok(if map.hunks.is_empty() {
            None
        } else {
            map.line_nearest(old.0)
        })
    }
}

/// Merge-base with the repository's default branch. A non-Git repository or
/// one without a resolvable default branch has no diff fallback.
pub(super) fn default_merge_base(root: &Path) -> Result<Option<String>, String> {
    let Some(target) = crate::core::lane_git::default_branch_ref(root) else {
        return Ok(None);
    };
    let output = crate::core::lane_git::git(root)?
        .args(["merge-base"])
        .arg(&target)
        .arg("HEAD")
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
    fn a_changed_line_lands_where_its_hunk_starts_on_the_new_side() {
        let map = FileMap::parse("@@ -2,0 +3,2 @@\n+a\n+a\n@@ -6,2 +8,3 @@\n-o\n-o\n+n\n+n\n+n\n")
            .unwrap();
        assert_eq!(map.line_nearest(1), Some(1));
        assert_eq!(map.line_nearest(5), Some(7));
        assert_eq!(map.line_nearest(6), Some(8));
        assert_eq!(map.line_nearest(7), Some(8));
        assert_eq!(map.line_nearest(8), Some(11));
        assert_eq!(map.line_nearest(0), None);
    }

    #[test]
    fn a_range_maps_only_when_both_endpoints_survive() {
        let map = FileMap::parse("@@ -4,2 +4,3 @@\n-old\n-old\n+new\n+new\n+new\n").unwrap();
        assert_eq!(map.span((3, 6)), Some((3, 7)));
        assert_eq!(map.span((4, 6)), None);
        assert_eq!(map.span((3, 5)), None);
    }

    /// KT-1595 round 12: mapping lines never runs a textconv driver the
    /// repository's configuration names.
    #[cfg(unix)] // runs a `#!/bin/sh` fixture as a program
    #[test]
    fn mapping_lines_runs_no_program_the_repository_names() {
        use crate::core::cmd::hostile_git::{append_config, git, marker_program, repo};
        let dir = repo();
        let base = git(dir.path(), &["rev-parse", "HEAD"]);
        std::fs::write(dir.path().join("a.txt"), "inserted\na\n").unwrap();
        git(dir.path(), &["commit", "-q", "-am", "insert"]);
        let (program, marker) = marker_program(dir.path());
        append_config(
            dir.path(),
            format!("[diff \"evil\"]\n\ttextconv = {program}\n").as_bytes(),
        );
        std::fs::create_dir_all(dir.path().join(".git/info")).unwrap();
        std::fs::write(dir.path().join(".git/info/attributes"), "a.txt diff=evil\n").unwrap();
        let mut map = DiffMap::new(dir.path(), &base);
        assert_eq!(map.span(Path::new("a.txt"), (1, 1)).unwrap(), Some((2, 2)));
        assert!(!marker.exists(), "the textconv driver ran");
        // Against the working tree git reads the index: no monitor runs either.
        git(dir.path(), &["config", "core.fsmonitor", &program]);
        let mut map = DiffMap::to_worktree(dir.path(), &base);
        assert_eq!(map.span(Path::new("a.txt"), (1, 1)).unwrap(), Some((2, 2)));
        assert!(!marker.exists(), "the fsmonitor hook ran");
    }

    /// KT-1595 round 14: a cited path that is a submodule never runs a driver of
    /// the submodule's own config, even when `.gitmodules` asks git to look in.
    #[cfg(unix)] // runs a `#!/bin/sh` fixture as a program
    #[test]
    fn a_submodule_path_never_runs_its_driver_whatever_gitmodules_says() {
        use crate::core::cmd::hostile_git::{git, marker_program, repo};
        let sub = repo();
        let dir = repo();
        git(
            dir.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub.path().to_str().unwrap(),
                "sub",
            ],
        );
        git(
            dir.path(),
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.sub.ignore",
                "none",
            ],
        );
        git(dir.path(), &["commit", "-q", "-am", "sub"]);
        let base = git(dir.path(), &["rev-parse", "HEAD"]);
        let (program, marker) = marker_program(dir.path());
        git(
            &dir.path().join(".git/modules/sub"),
            &["config", "filter.evil.clean", &program],
        );
        std::fs::write(dir.path().join("sub/.gitattributes"), "* filter=evil\n").unwrap();
        std::fs::write(dir.path().join("sub/a.txt"), "A\n").unwrap();
        let mut map = DiffMap::to_worktree(dir.path(), &base);
        let _ = map.span(Path::new("sub"), (1, 1));
        assert!(!marker.exists(), "the submodule's filter ran");
    }
}
