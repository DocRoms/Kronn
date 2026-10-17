//! Doc citations across a merge (KT-1321).
//!
//! Two branches that shift the same cited file each re-anchor the same
//! `[src: file: path:N-M#symbol]` markers, and the merge then conflicts on text
//! that only differs by its line numbers. [`settle_merge_docs`] runs inside the
//! merge in progress, before the merge commit exists: it resolves the conflicts
//! that are nothing but anchors (the target's text wins) and re-anchors every
//! citation the merged result left behind, so the commit carries both.
//!
//! Generic on purpose: the docs are the citation corpus the gate reads
//! ([`is_checked_doc`]: `docs/` and `backend/docs/`, release notes apart), and
//! nothing here reads a Kronn setting. Re-anchoring is computed from the merged
//! index, never from the working tree, so work in progress nobody named stays out
//! of the merge commit.

use super::diff_map::DiffMap;
use super::{
    file_citation_anchored, is_checked_doc, markers_at, reanchor_doc, suggested_span,
    DocCitationFailure, ReanchorOutcome,
};
use crate::core::anti_halluc::{anchor_miss, anchor_miss_in};
use crate::core::cmd::git_cmd;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

/// How far, in lines, the nearest occurrence of a symbol may lie from where a
/// changed hunk maps the cited range. Beyond it the symbol is another place
/// that merely shares its name, and a human picks.
pub const NEAREST_OCCURRENCE_LIMIT: usize = 20;

/// What settling the docs of a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeDocs {
    /// Docs whose conflicts were only citation anchors, resolved to the target's text.
    pub resolved: Vec<PathBuf>,
    pub outcome: ReanchorOutcome,
}

/// Why the docs of a merge were not settled. Nothing was written, bar the
/// anchor-only conflicts already resolved and staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleError {
    /// Settling would rewrite a file that carries changes the merge did not
    /// bring.
    Refused { paths: Vec<String>, message: String },
    /// The merge broke citations the evidence cannot place: the symbol moved
    /// too far, or two occurrences are equally near. A human points them.
    Unplaced { docs: Vec<String>, message: String },
    /// git or the filesystem failed.
    Failed(String),
}

impl fmt::Display for SettleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(text) => f.write_str(text),
            Self::Refused { message, .. } | Self::Unplaced { message, .. } => f.write_str(message),
        }
    }
}

impl From<String> for SettleError {
    fn from(text: String) -> Self {
        Self::Failed(text)
    }
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

/// The merged (staged) text of every doc of the citation corpus that holds at
/// least one `[src:` marker, sorted by path. The working tree is never read.
fn citation_docs(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let output = git_cmd()
        .args([
            "grep", "--cached", "-l", "-z", "-i", "-F", "[src:", "--", "*.md",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git grep: {error}"))?;
    // Exit 1 is "no match", not a failure.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "git grep failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| is_checked_doc(path))
        .map(str::to_string)
        .collect();
    paths.sort();
    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let text = blob(root, "", &path)?;
            Some((PathBuf::from(path), text))
        })
        .collect())
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
    for path in unmerged.into_iter().filter(|path| is_checked_doc(path)) {
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

/// Maps a citation of one side onto the merged tree.
struct MergeMapper<'a> {
    root: &'a Path,
    revs: &'a [String],
    maps: Vec<DiffMap<'a>>,
    /// Files whose merged content differs from at least one side.
    changed: BTreeSet<String>,
    /// Files whose working tree differs from the index.
    dirty: BTreeSet<String>,
    /// Dirty files a citation to be moved points into.
    blocked: BTreeSet<String>,
    /// `(doc line, citation)` of the current doc the evidence could not place.
    unplaced: Vec<(usize, String)>,
}

impl<'a> MergeMapper<'a> {
    fn new(
        root: &'a Path,
        revs: &'a [String],
        changed: BTreeSet<String>,
        dirty: BTreeSet<String>,
    ) -> Self {
        Self {
            root,
            revs,
            maps: revs
                .iter()
                .map(|rev| DiffMap::to_worktree(root, rev))
                .collect(),
            changed,
            dirty,
            blocked: BTreeSet::new(),
            unplaced: Vec::new(),
        }
    }

    /// Where the moved citation belongs in the merged tree. Only a file the
    /// merge changed is considered. A unique symbol needs no history. Otherwise
    /// the revisions the citation was written against map it line by line, and
    /// the symbol must lie in the mapped range. When a hunk changed the cited
    /// lines themselves, the occurrence of the symbol nearest the mapped
    /// position wins, again only if it holds, lies within
    /// [`NEAREST_OCCURRENCE_LIMIT`] and no other is as near.
    fn span(&mut self, failure: &DocCitationFailure) -> Result<Option<(usize, usize)>, String> {
        let Some(moved) = &failure.moved else {
            return Ok(None);
        };
        let Some((path, lines, anchor)) = file_citation_anchored(&failure.citation) else {
            return Ok(None);
        };
        if !self.changed.contains(&path) {
            return Ok(None);
        }
        if self.dirty.contains(&path) {
            // The cited lines are those of a file with changes nobody committed:
            // pointing a doc at them would anchor it to work in progress.
            self.blocked.insert(path);
            return Ok(None);
        }
        if moved.occurrences.len() == 1 {
            return Ok(suggested_span(moved.span, &moved.occurrences, moved.total));
        }
        let (Some(old), Some(anchor)) = (lines, anchor) else {
            return Ok(None);
        };
        let origins: Vec<usize> = (0..self.revs.len())
            .filter(|&index| holds_at(self.root, &self.revs[index], &path, old, &anchor))
            .collect();
        if origins.is_empty() {
            // Written against no version of the file: not the merge's doing.
            return Ok(None);
        }
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
            let nearest = moved
                .occurrences
                .iter()
                .map(|&line| (line.abs_diff(start), line))
                .filter(|&(distance, _)| distance <= NEAREST_OCCURRENCE_LIMIT)
                .min();
            let Some((distance, line)) = nearest else {
                continue;
            };
            let tied = moved
                .occurrences
                .iter()
                .filter(|&&other| other.abs_diff(start) == distance)
                .count();
            if tied > 1 {
                continue;
            }
            let span = (line, (line + (old.1 - old.0)).min(moved.total.max(line)));
            if anchor_miss(&merged, span, &anchor).is_none() {
                return Ok(Some(span));
            }
        }
        // It held on a side and the merge broke it, and nothing safe remains.
        self.unplaced.push((failure.line, failure.citation.clone()));
        Ok(None)
    }
}

/// What re-anchoring the merged docs would write, nothing written yet.
struct Plan {
    /// `(doc, merged text re-anchored)` for each doc that changes.
    writes: Vec<(PathBuf, String)>,
    outcome: ReanchorOutcome,
    unplaced: Vec<(PathBuf, usize, String)>,
    blocked: BTreeSet<String>,
    /// Docs among `writes` whose working tree is not what the index holds.
    dirty_docs: Vec<String>,
}

fn path_set(root: &Path, args: &[&str]) -> Result<BTreeSet<String>, String> {
    Ok(git_lines(root, args)?.into_iter().collect())
}

/// Computes, from the merged index, the re-anchoring of the citations the merge
/// shifted. Each is mapped from the side it was written against (ours, the
/// target's, or a merge base) to the merged file, and only a citation whose
/// cited file the merge changed is considered.
fn plan_reanchor(root: &Path) -> Result<Plan, String> {
    let theirs = git_lines(root, &["rev-parse", "MERGE_HEAD"])?
        .into_iter()
        .next()
        .ok_or("MERGE_HEAD does not resolve")?;
    let ours = git_lines(root, &["rev-parse", "HEAD"])?
        .into_iter()
        .next()
        .ok_or("HEAD does not resolve")?;
    let mut revs = vec![theirs.clone(), ours.clone()];
    for base in git_lines(root, &["merge-base", "--all", "HEAD", "MERGE_HEAD"]).unwrap_or_default()
    {
        if !revs.contains(&base) {
            revs.push(base);
        }
    }
    let unmerged = path_set(root, &["diff", "--name-only", "-z", "--diff-filter=U"])?;
    let dirty = path_set(root, &["diff", "--name-only", "-z", "--no-renames"])?;
    let mut changed = BTreeSet::new();
    for side in [&ours, &theirs] {
        changed.extend(path_set(
            root,
            &[
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--no-renames",
                side,
            ],
        )?);
    }
    let mut mapper = MergeMapper::new(root, &revs, changed, dirty.clone());
    let mut plan = Plan {
        writes: Vec::new(),
        outcome: ReanchorOutcome::default(),
        unplaced: Vec::new(),
        blocked: BTreeSet::new(),
        dirty_docs: Vec::new(),
    };
    for (doc, text) in citation_docs(root)? {
        let name = doc.to_string_lossy().into_owned();
        if unmerged.contains(&name) {
            continue;
        }
        let new = reanchor_doc(
            &doc,
            &text,
            root,
            &mut |f| mapper.span(f),
            &mut plan.outcome,
        )?;
        plan.unplaced.extend(
            mapper
                .unplaced
                .drain(..)
                .map(|(line, citation)| (doc.clone(), line, citation)),
        );
        if new != text {
            if dirty.contains(&name) {
                plan.dirty_docs.push(name);
            }
            plan.writes.push((doc, new));
        }
    }
    plan.blocked = mapper.blocked;
    Ok(plan)
}

/// Refuses, writing nothing, when settling would have to rewrite a file that
/// carries changes the merge did not bring.
fn refuse_unclean(plan: &Plan) -> Result<(), SettleError> {
    if plan.dirty_docs.is_empty() && plan.blocked.is_empty() {
        return Ok(());
    }
    let list = |paths: Vec<&String>| paths.into_iter().cloned().collect::<Vec<_>>().join(", ");
    let mut reasons = Vec::new();
    if !plan.dirty_docs.is_empty() {
        reasons.push(format!(
            "the doc(s) {} need their citations re-anchored but have unstaged changes",
            list(plan.dirty_docs.iter().collect())
        ));
    }
    if !plan.blocked.is_empty() {
        reasons.push(format!(
            "the cited file(s) {} have unstaged changes",
            list(plan.blocked.iter().collect())
        ));
    }
    let paths: BTreeSet<&String> = plan.dirty_docs.iter().chain(&plan.blocked).collect();
    Err(SettleError::Refused {
        paths: paths.into_iter().cloned().collect(),
        message: format!(
            "refused: {}. Commit or stash that work, or name the files in `files` if they belong to \
             the merge; the merge state was left as it was.",
            reasons.join(" and ")
        ),
    })
}

/// Writes the planned docs into the working tree.
fn write_plan(root: &Path, plan: &Plan) -> Result<(), String> {
    for (doc, text) in &plan.writes {
        std::fs::write(root.join(doc), text)
            .map_err(|error| format!("cannot write {}: {error}", doc.display()))?;
    }
    Ok(())
}

/// Re-anchors, in the merge in progress, the citations the merged index leaves
/// broken, in the working tree and nothing else (`make docs-reanchor` mid-merge).
pub fn reanchor_merge_in_progress(root: &Path) -> Result<ReanchorOutcome, SettleError> {
    let plan = plan_reanchor(root)?;
    refuse_unclean(&plan)?;
    write_plan(root, &plan)?;
    Ok(plan.outcome)
}

/// Re-anchors what the merge shifted and stages the docs that changed,
/// `resolved` ones included. Nothing is committed here. Refuses, writing
/// nothing, when a doc to rewrite or a file it cites has unstaged changes: the
/// merge commit takes only what the merge brought and the worker named. A
/// citation the merge broke and the evidence cannot place is not guessed: it
/// fails the step, again writing nothing.
pub fn reanchor_and_stage(root: &Path, resolved: Vec<PathBuf>) -> Result<MergeDocs, SettleError> {
    let plan = plan_reanchor(root)?;
    refuse_unclean(&plan)?;
    if !plan.unplaced.is_empty() {
        let docs: BTreeSet<String> = plan
            .unplaced
            .iter()
            .map(|(doc, ..)| doc.to_string_lossy().into_owned())
            .collect();
        let cites: Vec<String> = plan
            .unplaced
            .iter()
            .map(|(doc, line, citation)| format!("{}:{line} [src: {citation}]", doc.display()))
            .collect();
        return Err(SettleError::Unplaced {
            docs: docs.into_iter().collect(),
            message: format!(
                "the merge broke doc citations that cannot be re-anchored safely (the symbol moved \
                 more than {NEAREST_OCCURRENCE_LIMIT} lines, or two occurrences are equally near): \
                 {}. Point each at its new lines by hand.",
                cites.join("; ")
            ),
        });
    }
    write_plan(root, &plan)?;
    let docs = MergeDocs {
        resolved,
        outcome: plan.outcome,
    };
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
pub fn settle_merge_docs(root: &Path) -> Result<MergeDocs, SettleError> {
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
    fn only_the_gates_corpus_is_settled() {
        assert!(is_checked_doc("docs/a.md"));
        assert!(is_checked_doc("backend/docs/a.md"));
        for other in [
            "docs/releases/0.1.md",
            "src/a.rs",
            "CHANGELOG.md",
            "README.md",
            "templates/docs/a.md",
            "backend/tests/fixtures/a.md",
            "prompts/a.md",
        ] {
            assert!(!is_checked_doc(other), "{other}");
        }
    }
}
