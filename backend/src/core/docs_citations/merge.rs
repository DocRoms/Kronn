//! Doc citations across a merge (KT-1321).
//!
//! Two branches that shift the same cited file each re-anchor the same
//! `[src: file: path:N-M#symbol]` markers, and the merge then conflicts on text
//! that only differs by its line numbers. [`settle_merge_docs`] runs inside the
//! merge in progress, before the merge commit exists: it resolves the conflicts
//! that are nothing but anchors (the target's text wins) and re-anchors every
//! citation the merged result left behind, so the commit carries both.
//!
//! Generic on purpose: the docs are every tracked `.md` file, not Kronn's own
//! doc trees, and nothing here reads a Kronn setting.

use super::diff_map::DiffMap;
use super::{
    file_citation_anchored, markers_at, reanchor_doc, suggested_span, DocCitationFailure,
    ReanchorOutcome, EXCLUDED,
};
use crate::core::anti_halluc::{anchor_miss, anchor_miss_in};
use crate::core::cmd::git_cmd;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What settling the docs of a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeDocs {
    /// Docs whose conflicts were only citation anchors, resolved to the target's text.
    pub resolved: Vec<PathBuf>,
    pub outcome: ReanchorOutcome,
}

impl MergeDocs {
    /// Every doc the settling changed or staged.
    pub fn touched(&self) -> BTreeSet<PathBuf> {
        self.resolved
            .iter()
            .cloned()
            .chain(self.outcome.rewritten.iter().map(|(doc, ..)| doc.clone()))
            .collect()
    }

    /// One line for the merge commit body; `None` when nothing was done.
    pub fn summary(&self) -> Option<String> {
        if self.resolved.is_empty() && self.outcome.rewritten.is_empty() {
            return None;
        }
        Some(format!(
            "Doc citations: {} anchor-only conflict(s) resolved to the target's text, {} citation(s) re-anchored.",
            self.resolved.len(),
            self.outcome.rewritten.len()
        ))
    }
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = git_cmd()
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    Ok(String::from_utf8_lossy(&git_stdout(root, args)?)
        .split(['\0', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Whether `root` is in the middle of a merge.
pub fn merge_in_progress(root: &Path) -> bool {
    git_stdout(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
}

/// A markdown file that may carry citations: the historical release notes
/// describe the code of their time and are never touched.
fn is_citation_doc(path: &str) -> bool {
    path.ends_with(".md")
        && !EXCLUDED
            .iter()
            .any(|dir| path.strip_prefix(dir).is_some_and(|r| r.starts_with('/')))
}

/// The tracked docs that hold at least one `[src:` marker, sorted.
fn citation_docs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut docs: Vec<PathBuf> = git_lines(root, &["ls-files", "-z", "--", "*.md"])?
        .into_iter()
        .filter(|path| is_citation_doc(path))
        .filter(|path| {
            std::fs::read_to_string(root.join(path))
                .is_ok_and(|text| text.to_ascii_lowercase().contains("[src:"))
        })
        .map(PathBuf::from)
        .collect();
    docs.sort();
    Ok(docs)
}

/// The text with every file citation reduced to its path, whether it has a
/// line spec, and its `#symbol`: two texts equal under it differ only by line
/// numbers.
fn anchor_free(text: &str) -> String {
    let mut out = text.to_string();
    let mut markers = markers_at(text);
    markers.reverse();
    for (chars, inner) in markers {
        let Some((open, _)) = text.char_indices().nth(chars) else {
            continue;
        };
        let start = open + "[src:".len();
        let Some(found) = text[start..].find(&inner) else {
            continue;
        };
        let canonical = match file_citation_anchored(&inner) {
            Some((path, lines, anchor)) => {
                format!("{path}|{}|{}", lines.is_some(), anchor.unwrap_or_default())
            }
            None => inner.clone(),
        };
        out.replace_range(start + found..start + found + inner.len(), &canonical);
    }
    out
}

/// One conflict hunk, or the text between two.
enum Piece<'a> {
    Plain(&'a str),
    Conflict { ours: String, theirs: String },
}

/// The conflict markers of a file git left mid-merge. `None` when they do not
/// pair up (a stray marker, a conflict cut by the end of the file).
fn parse_conflicts(text: &str) -> Option<Vec<Piece<'_>>> {
    enum State {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let mut pieces = Vec::new();
    let mut state = State::Outside;
    let (mut ours, mut theirs) = (String::new(), String::new());
    let mut plain_from = 0usize;
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        let at = offset;
        offset += raw.len();
        let is = |marker: &str| {
            line == marker
                || line
                    .strip_prefix(marker)
                    .is_some_and(|rest| rest.starts_with(' '))
        };
        match state {
            State::Outside if is("<<<<<<<") => {
                if at > plain_from {
                    pieces.push(Piece::Plain(&text[plain_from..at]));
                }
                state = State::Ours;
            }
            State::Outside => {}
            State::Ours if is("|||||||") => state = State::Base,
            State::Ours if line == "=======" => state = State::Theirs,
            State::Base if line == "=======" => state = State::Theirs,
            State::Ours => ours.push_str(raw),
            State::Base => {}
            State::Theirs if is(">>>>>>>") => {
                pieces.push(Piece::Conflict {
                    ours: std::mem::take(&mut ours),
                    theirs: std::mem::take(&mut theirs),
                });
                plain_from = offset;
                state = State::Outside;
            }
            State::Theirs => theirs.push_str(raw),
        }
    }
    if !matches!(state, State::Outside) {
        return None;
    }
    if plain_from < text.len() {
        pieces.push(Piece::Plain(&text[plain_from..]));
    }
    Some(pieces)
}

/// The file with each conflict taken from the target's side, when every one of
/// them differs from ours only by citation line numbers.
fn resolve_anchor_only(text: &str) -> Option<String> {
    let pieces = parse_conflicts(text)?;
    let mut out = String::with_capacity(text.len());
    let mut conflicts = 0usize;
    for piece in &pieces {
        match piece {
            Piece::Plain(plain) => out.push_str(plain),
            Piece::Conflict { ours, theirs } => {
                if anchor_free(ours) != anchor_free(theirs) {
                    return None;
                }
                conflicts += 1;
                out.push_str(theirs);
            }
        }
    }
    (conflicts > 0).then_some(out)
}

/// Resolves, in the merge in progress, every doc whose conflicts are only
/// citation anchors: the target's side (`MERGE_HEAD`) wins and the file is
/// staged. Any other conflict is left exactly as git wrote it.
pub fn resolve_anchor_only_conflicts(root: &Path) -> Result<Vec<PathBuf>, String> {
    let unmerged = git_lines(root, &["diff", "--name-only", "-z", "--diff-filter=U"])?;
    let mut resolved = Vec::new();
    for path in unmerged.into_iter().filter(|path| is_citation_doc(path)) {
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let Some(merged) = resolve_anchor_only(&text) else {
            continue;
        };
        std::fs::write(root.join(&path), merged)
            .map_err(|error| format!("cannot write {path}: {error}"))?;
        git_stdout(root, &["add", "--", &path])?;
        resolved.push(PathBuf::from(path));
    }
    Ok(resolved)
}

/// The text of `path` at `rev`, when the file exists there.
fn blob(root: &Path, rev: &str, path: &str) -> Option<String> {
    let spec = format!("{rev}:{path}");
    let bytes = git_stdout(root, &["show", &spec]).ok()?;
    String::from_utf8(bytes).ok()
}

/// The cited range exists in `rev`'s version of the file and names the symbol
/// there: the citation was written against that version.
fn holds_at(root: &Path, rev: &str, path: &str, span: (usize, usize), symbol: &str) -> bool {
    blob(root, rev, path).is_some_and(|text| {
        span.0 >= 1
            && span.0 <= span.1
            && span.1 <= text.lines().count()
            && anchor_miss_in(&text, span, symbol).is_none()
    })
}

/// Maps a citation of one side onto the merged working tree.
struct MergeMapper<'a> {
    root: &'a Path,
    revs: &'a [String],
    maps: Vec<DiffMap<'a>>,
}

impl<'a> MergeMapper<'a> {
    fn new(root: &'a Path, revs: &'a [String]) -> Self {
        Self {
            root,
            revs,
            maps: revs
                .iter()
                .map(|rev| DiffMap::to_worktree(root, rev))
                .collect(),
        }
    }

    /// Where the moved citation belongs in the merged tree. A unique symbol
    /// needs no history. Otherwise the revisions the citation was written
    /// against map it line by line, and the symbol must lie in the mapped
    /// range. When a hunk changed the cited lines themselves, the occurrence of
    /// the symbol nearest the mapped position wins, again only if it holds.
    fn span(&mut self, failure: &DocCitationFailure) -> Result<Option<(usize, usize)>, String> {
        let Some(moved) = &failure.moved else {
            return Ok(None);
        };
        if moved.occurrences.len() == 1 {
            return Ok(suggested_span(moved.span, &moved.occurrences, moved.total));
        }
        let Some((path, Some(old), Some(anchor))) = file_citation_anchored(&failure.citation)
        else {
            return Ok(None);
        };
        let origins: Vec<usize> = (0..self.revs.len())
            .filter(|&index| holds_at(self.root, &self.revs[index], &path, old, &anchor))
            .collect();
        let merged = self.root.join(&path);
        for &index in &origins {
            if let Some(mapped) = self.maps[index].span(Path::new(&path), old)? {
                if anchor_miss(&merged, mapped, &anchor).is_none() {
                    return Ok(Some(mapped));
                }
            }
        }
        for &index in &origins {
            let Some(start) = self.maps[index].nearest_start(Path::new(&path), old)? else {
                continue;
            };
            let Some(&line) = moved.occurrences.iter().min_by_key(|l| l.abs_diff(start)) else {
                continue;
            };
            let span = (line, (line + (old.1 - old.0)).min(moved.total.max(line)));
            if anchor_miss(&merged, span, &anchor).is_none() {
                return Ok(Some(span));
            }
        }
        Ok(None)
    }
}

/// Re-anchors, in the merge in progress, the citations the merged working tree
/// leaves broken. Each is mapped from the side it was written against (ours,
/// the target's, or a merge base) to the merged file.
pub fn reanchor_merge_in_progress(root: &Path) -> Result<ReanchorOutcome, String> {
    let theirs = git_lines(root, &["rev-parse", "MERGE_HEAD"])?
        .into_iter()
        .next()
        .ok_or("MERGE_HEAD does not resolve")?;
    let ours = git_lines(root, &["rev-parse", "HEAD"])?
        .into_iter()
        .next()
        .ok_or("HEAD does not resolve")?;
    let mut revs = vec![theirs, ours];
    for base in git_lines(root, &["merge-base", "--all", "HEAD", "MERGE_HEAD"]).unwrap_or_default()
    {
        if !revs.contains(&base) {
            revs.push(base);
        }
    }
    let unmerged: BTreeSet<String> =
        git_lines(root, &["diff", "--name-only", "-z", "--diff-filter=U"])?
            .into_iter()
            .collect();
    let mut mapper = MergeMapper::new(root, &revs);
    let mut outcome = ReanchorOutcome::default();
    for doc in citation_docs(root)? {
        if unmerged.contains(doc.to_string_lossy().as_ref()) {
            continue;
        }
        let path = root.join(&doc);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", doc.display()))?;
        let new = reanchor_doc(&doc, &text, root, &mut |f| mapper.span(f), &mut outcome)?;
        if new != text {
            std::fs::write(&path, new)
                .map_err(|error| format!("cannot write {}: {error}", doc.display()))?;
        }
    }
    Ok(outcome)
}

/// Re-anchors what the merge shifted and stages every doc that changed,
/// `resolved` ones included. Nothing is committed here.
pub fn reanchor_and_stage(root: &Path, resolved: Vec<PathBuf>) -> Result<MergeDocs, String> {
    let outcome = reanchor_merge_in_progress(root)?;
    let docs = MergeDocs { resolved, outcome };
    let touched: Vec<String> = docs
        .touched()
        .iter()
        .map(|doc| doc.to_string_lossy().into_owned())
        .collect();
    if !touched.is_empty() {
        let mut args = vec!["add", "--"];
        args.extend(touched.iter().map(String::as_str));
        git_stdout(root, &args)?;
    }
    Ok(docs)
}

/// The whole step on a merge whose other conflicts are already settled:
/// resolve the anchor-only conflicts, then re-anchor and stage.
pub fn settle_merge_docs(root: &Path) -> Result<MergeDocs, String> {
    let resolved = resolve_anchor_only_conflicts(root)?;
    reanchor_and_stage(root, resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_that_differ_only_by_line_numbers_are_anchor_free_equal() {
        let a = "See [src: file: src/a.rs:10-20#run] and `[src: file: x.rs:3]`.\n";
        let b = "See [src: file: src/a.rs:13-23#run] and `[src: file: x.rs:9]`.\n";
        assert_eq!(anchor_free(a), anchor_free(b));
    }

    #[test]
    fn a_different_symbol_path_prose_or_missing_line_is_not_an_anchor_only_difference() {
        let base = "See [src: file: src/a.rs:10-20#run].\n";
        for other in [
            "See [src: file: src/a.rs:10-20#walk].\n",
            "See [src: file: src/b.rs:10-20#run].\n",
            "See [src: file: src/a.rs#run].\n",
            "Read [src: file: src/a.rs:10-20#run].\n",
        ] {
            assert_ne!(anchor_free(base), anchor_free(other), "{other}");
        }
    }

    #[test]
    fn only_all_anchor_conflicts_resolve_and_the_targets_side_wins() {
        let conflict = |ours: &str, theirs: &str| {
            format!("head\n<<<<<<< HEAD\n{ours}\n=======\n{theirs}\n>>>>>>> target\ntail\n")
        };
        let merged = resolve_anchor_only(&conflict(
            "A [src: file: a.rs:1-2#f]",
            "A [src: file: a.rs:4-5#f]",
        ));
        assert_eq!(
            merged.as_deref(),
            Some("head\nA [src: file: a.rs:4-5#f]\ntail\n")
        );
        assert_eq!(
            resolve_anchor_only(&conflict("A one", "A two")),
            None,
            "a text conflict stays a conflict"
        );
        let mixed = format!(
            "{}{}",
            conflict("[src: file: a.rs:1#f]", "[src: file: a.rs:2#f]"),
            conflict("prose one", "prose two")
        );
        assert_eq!(
            resolve_anchor_only(&mixed),
            None,
            "one real conflict keeps them all"
        );
        assert_eq!(resolve_anchor_only("no conflict here\n"), None);
        assert_eq!(resolve_anchor_only("<<<<<<< HEAD\nx\n=======\ny\n"), None);
    }

    #[test]
    fn a_diff3_conflict_resolves_the_same_way() {
        let text = "<<<<<<< HEAD\n[src: file: a.rs:1-2#f]\n||||||| base\n[src: file: a.rs:1-1#f]\n=======\n[src: file: a.rs:7-8#f]\n>>>>>>> target\n";
        assert_eq!(
            resolve_anchor_only(text).as_deref(),
            Some("[src: file: a.rs:7-8#f]\n")
        );
    }

    #[test]
    fn release_notes_are_not_citation_docs() {
        assert!(is_citation_doc("docs/a.md"));
        assert!(is_citation_doc("ai/a.md"));
        assert!(!is_citation_doc("docs/releases/0.1.md"));
        assert!(!is_citation_doc("src/a.rs"));
    }
}
