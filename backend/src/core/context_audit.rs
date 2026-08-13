//! Context Architecture Audit — KT-194.
//!
//! Every agent session pays for its instruction files before doing any useful
//! work, so those files are a product cost. KT-191 measured Kronn's own; this
//! turns that dogfood into something any monitored project can run.
//!
//! What it answers: which files each agent actually loads, in what order, how big
//! the result is, what is said twice, what points at nothing, and which sections
//! could move out of the always-loaded tier.
//!
//! Two rules it never breaks.
//!
//! IT DOES NOT WRITE. The tier split is a PROPOSAL — a description of moves, for a
//! human to accept or reject. There is no apply path in this module, deliberately:
//! an audit that rewrites instruction files can silently delete the one rule that
//! was holding something together.
//!
//! AND AN ABSENCE IS NOT A ZERO. A file that cannot be read is `unreadable`, not
//! 0 bytes; a repository with no instruction file at all is reported as having no
//! convention, not as having an optimal context.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use ts_rs::TS;

/// Cap on the rendered report.
pub const AUDIT_REPORT_MAX_BYTES: usize = 32_768;

const _: () = assert!(
    AUDIT_REPORT_MAX_BYTES <= 65_536,
    "an audit report above 64 KiB is itself a context problem"
);

/// Files scanned per repository. A tree with more instruction files than this has
/// a bigger problem than the cap.
pub const MAX_FILES_SCANNED: usize = 200;
/// Per-file read cap. Beyond it the file is reported as oversized rather than
/// loaded, since loading it is the cost being measured.
pub const MAX_FILE_BYTES: usize = 262_144;
/// A paragraph shorter than this repeats for ordinary reasons — a heading, a
/// one-line note — and flagging it as duplication would bury the real cases.
pub const DUPLICATE_MIN_BYTES: usize = 240;

/// Which files an agent reads, in the order it reads them.
pub struct Convention {
    pub agent: &'static str,
    /// Repo-relative paths, highest precedence first. A `*` suffix matches every
    /// file directly inside that directory.
    pub paths: &'static [&'static str],
}

/// The conventions Kronn knows about.
///
/// Precedence within an agent is the order below, taken from each vendor's own
/// documented loading order. An agent absent from a repository contributes
/// nothing — which is itself reported, because "this agent loads nothing here" is
/// a finding when a teammate is using it.
pub const CONVENTIONS: &[Convention] = &[
    Convention {
        agent: "ClaudeCode",
        paths: &[
            "CLAUDE.local.md",
            "CLAUDE.md",
            ".claude/CLAUDE.md",
            "AGENTS.md",
        ],
    },
    Convention {
        agent: "Codex",
        paths: &["AGENTS.md", ".codex/AGENTS.md"],
    },
    Convention {
        agent: "GeminiCli",
        paths: &["GEMINI.md", ".gemini/GEMINI.md", "AGENTS.md"],
    },
    Convention {
        agent: "CopilotCli",
        paths: &[".github/copilot-instructions.md", "AGENTS.md"],
    },
    Convention {
        agent: "Cursor",
        paths: &[".cursorrules", ".cursor/rules/*"],
    },
    Convention {
        agent: "Kiro",
        paths: &[".kiro/steering/*", "AGENTS.md"],
    },
    Convention {
        agent: "Vibe",
        paths: &["AGENTS.md", ".vibe/AGENTS.md"],
    },
    Convention {
        agent: "Windsurf",
        paths: &[".windsurfrules"],
    },
    Convention {
        agent: "Cline",
        paths: &[".clinerules"],
    },
];

/// What a section is, for the purpose of deciding whether it must be loaded every
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SectionClass {
    /// Carries a MUST / NEVER imperative. Cannot be moved out of the always-loaded
    /// tier without a human deciding to.
    Critical,
    /// Applies to every task but states no hard rule.
    Universal,
    /// A pointer to somewhere else. Cheap to keep, and the thing that makes
    /// tiering work.
    Routed,
    /// A narrative about something that already happened — dates, an incident, a
    /// version history. Real information, wrong place: nobody needs it to start a
    /// task.
    Historical,
    /// Said in full somewhere else in the same agent's context.
    Duplicated,
    /// References a path that no longer exists.
    PossiblyObsolete,
}

impl SectionClass {
    /// Whether a section can be proposed for a lower tier.
    ///
    /// `Critical` cannot: it is the class whose whole point is that removing it
    /// changes behaviour. `Routed` cannot either — it is already the cheap form.
    pub fn is_movable(self) -> bool {
        matches!(
            self,
            Self::Historical | Self::Duplicated | Self::PossiblyObsolete
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Section {
    pub heading: String,
    pub bytes: usize,
    pub class: SectionClass,
    /// Why it was classed that way. Kept because a classification a human cannot
    /// check is a classification they will either trust blindly or ignore.
    pub because: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InstructionFile {
    pub path: String,
    /// `None` when the file could not be read. NOT 0 — an unreadable file is not
    /// an empty one, and its cost is unknown rather than absent.
    pub bytes: Option<usize>,
    pub agents: Vec<String>,
    pub sections: Vec<Section>,
    /// Markdown link targets that do not exist on disk.
    pub broken_links: Vec<String>,
    /// Files this one tells the reader to load.
    pub redirects_to: Vec<String>,
}

/// One agent's effective context.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentContext {
    pub agent: String,
    /// Present files, in the agent's own precedence order.
    pub files: Vec<String>,
    /// Sum over readable files. `None` when at least one is unreadable, because a
    /// partial sum presented as a total understates the cost.
    pub total_bytes: Option<usize>,
    pub unreadable_files: Vec<String>,
    pub duplicated_bytes: usize,
    pub broken_links: usize,
    /// A redirect chain that comes back to a file already in the chain.
    pub redirect_cycles: Vec<Vec<String>>,
}

/// A move the audit suggests. Never applied.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TierMove {
    pub from_path: String,
    pub heading: String,
    pub bytes: usize,
    /// 0 = always loaded, 1 = per-session bootstrap, 2 = on demand, 3 = archive.
    pub to_tier: u8,
    pub because: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditFinding {
    pub severity: String,
    pub what: String,
    pub where_: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextAudit {
    pub files: Vec<InstructionFile>,
    pub per_agent: Vec<AgentContext>,
    pub proposal: Vec<TierMove>,
    pub findings: Vec<AuditFinding>,
    /// True when no known instruction file was found. Reported explicitly: an
    /// empty audit must not read as a clean one.
    pub no_convention_found: bool,
}

/// One file as handed to the pure analysis.
pub struct ScannedFile {
    pub path: String,
    /// `None` when it could not be read or exceeded the cap.
    pub content: Option<String>,
}

/// Analyse an already-read set of files.
///
/// Pure on purpose: the classification rules are the part worth testing, and a
/// test that has to build a directory tree to check a heuristic tests the tree.
/// `existing_paths` is what the repository contains, used to resolve links.
pub fn analyse(files: Vec<ScannedFile>, existing_paths: &HashSet<String>) -> ContextAudit {
    let mut analysed: Vec<InstructionFile> = Vec::new();
    let mut findings = Vec::new();

    // Paragraph -> the files containing it, for duplicate detection across files.
    let mut paragraph_owners: HashMap<String, Vec<String>> = HashMap::new();
    for file in &files {
        if let Some(content) = &file.content {
            for paragraph in paragraphs(content) {
                paragraph_owners
                    .entry(paragraph)
                    .or_default()
                    .push(file.path.clone());
            }
        }
    }

    for file in &files {
        let Some(content) = &file.content else {
            findings.push(AuditFinding {
                severity: "warn".into(),
                what: "instruction file could not be read, so its cost is unknown \
                       rather than zero"
                    .into(),
                where_: file.path.clone(),
            });
            analysed.push(InstructionFile {
                path: file.path.clone(),
                bytes: None,
                agents: agents_reading(&file.path),
                sections: Vec::new(),
                broken_links: Vec::new(),
                redirects_to: Vec::new(),
            });
            continue;
        };

        let links = markdown_links(content);
        let broken_links: Vec<String> = links
            .iter()
            .filter(|target| is_broken(target, &file.path, existing_paths))
            .cloned()
            .collect();
        for target in &broken_links {
            findings.push(AuditFinding {
                severity: "error".into(),
                what: format!("points at `{target}`, which does not exist"),
                where_: file.path.clone(),
            });
        }

        // Both forms, because the dominant one in practice is not a link.
        // `CLAUDE.md` in Kronn and in front_euronews says "Read `docs/AGENTS.md`
        // first" — a backticked path in prose. Detecting only `](target)` found
        // the redirect in docroms-web and missed it in the two repositories that
        // matter most, which is how a 15 152 B index stayed unattributed.
        let mut redirects_to: Vec<String> = links
            .iter()
            .filter(|target| is_instruction_path(target))
            .map(|target| resolve_link(target, &file.path))
            .collect();
        for mention in instruction_mentions(content) {
            let resolved = resolve_link(&mention, &file.path);
            // Only a target that EXISTS counts. A mention of an absent file is
            // already a finding; charging an agent for bytes nobody can read
            // would turn one problem into a wrong number.
            if existing_paths.contains(&resolved) && !redirects_to.contains(&resolved) {
                redirects_to.push(resolved);
            }
        }

        let sections = classify_sections(content, &file.path, &paragraph_owners, existing_paths);

        analysed.push(InstructionFile {
            path: file.path.clone(),
            bytes: Some(content.len()),
            agents: agents_reading(&file.path),
            sections,
            broken_links,
            redirects_to,
        });
    }

    let per_agent = per_agent_contexts(&analysed);

    // Backfill each file with the agents that actually LOAD it, redirects
    // included. `agents_reading` only knows the convention patterns, so before
    // this a tiered index reached from nine root files reported `agents = 0` — and
    // the drift report then called the most-loaded file in the repository unused.
    for file in &mut analysed {
        let loaders: Vec<String> = per_agent
            .iter()
            .filter(|context| context.files.contains(&file.path))
            .map(|context| context.agent.clone())
            .collect();
        if !loaders.is_empty() {
            file.agents = loaders;
        }
    }

    for context in &per_agent {
        for cycle in &context.redirect_cycles {
            findings.push(AuditFinding {
                severity: "error".into(),
                what: format!("redirect cycle: {}", cycle.join(" → ")),
                where_: context.agent.clone(),
            });
        }
        if context.files.is_empty() {
            findings.push(AuditFinding {
                severity: "info".into(),
                what: "this agent loads no instruction file in this repository".into(),
                where_: context.agent.clone(),
            });
        }
    }

    let proposal = propose_tiers(&analysed);
    let no_convention_found = analysed.is_empty();
    if no_convention_found {
        findings.push(AuditFinding {
            severity: "warn".into(),
            what: "no known instruction file found — agents here start with no \
                   project context at all"
                .into(),
            where_: ".".into(),
        });
    }

    ContextAudit {
        files: analysed,
        per_agent,
        proposal,
        findings,
        no_convention_found,
    }
}

/// Which agents read a given path, by convention.
fn agents_reading(path: &str) -> Vec<String> {
    CONVENTIONS
        .iter()
        .filter(|convention| {
            convention
                .paths
                .iter()
                .any(|pattern| path_matches(pattern, path))
        })
        .map(|convention| convention.agent.to_string())
        .collect()
}

/// `.cursor/rules/*` matches `.cursor/rules/anything.mdc`, but not deeper.
fn path_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('*') {
        None => pattern == path,
        Some(prefix) => path.starts_with(prefix) && !path[prefix.len()..].contains('/'),
    }
}

fn per_agent_contexts(files: &[InstructionFile]) -> Vec<AgentContext> {
    let by_path: HashMap<&str, &InstructionFile> = files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();

    CONVENTIONS
        .iter()
        .map(|convention| {
            // Precedence is the convention's own order, filtered to what exists.
            let mut present: Vec<&InstructionFile> = Vec::new();
            for pattern in convention.paths {
                let mut matched: Vec<&InstructionFile> = files
                    .iter()
                    .filter(|file| path_matches(pattern, &file.path))
                    .collect();
                // Stable within a wildcard so the report does not shuffle between
                // runs on the same tree.
                matched.sort_by(|a, b| a.path.cmp(&b.path));
                present.extend(matched);
            }

            // A file reached only through a redirect is still LOADED, so it is
            // still paid for. Measured on front_euronews — the pattern Kronn
            // itself installs — nine 200-byte root files each say "read
            // docs/AGENTS.md first", and that target is 15 152 B. Counting only
            // the entry points reported 421 B for Claude Code against a real
            // 15 573 B: understated 36x, on the repository in heaviest use.
            let reached = follow_redirects(&present, &by_path);

            let unreadable: Vec<String> = reached
                .iter()
                .filter(|file| file.bytes.is_none())
                .map(|file| file.path.clone())
                .collect();
            // One unreadable file makes the total unknown. A sum over the rest
            // would be presented as the cost and be wrong by an unknown amount.
            let total_bytes = if unreadable.is_empty() {
                Some(reached.iter().filter_map(|file| file.bytes).sum())
            } else {
                None
            };

            let duplicated_bytes = reached
                .iter()
                .flat_map(|file| &file.sections)
                .filter(|section| section.class == SectionClass::Duplicated)
                .map(|section| section.bytes)
                .sum();

            AgentContext {
                agent: convention.agent.to_string(),
                // Entry points first, then what they lead to, so the list reads in
                // load order rather than as one undifferentiated set.
                files: reached.iter().map(|file| file.path.clone()).collect(),
                total_bytes,
                unreadable_files: unreadable,
                duplicated_bytes,
                broken_links: reached.iter().map(|f| f.broken_links.len()).sum(),
                redirect_cycles: find_cycles(&present, &by_path),
            }
        })
        .collect()
}

/// Everything an agent ends up loading: its entry points, plus what those tell it
/// to read, transitively.
///
/// Breadth-first from the entry points, so the result reads in load order. Visited
/// paths are skipped, which both bounds the walk against a cycle and stops one
/// agent paying twice for a file two of its entry points both point at.
fn follow_redirects<'a>(
    entries: &[&'a InstructionFile],
    by_path: &HashMap<&str, &'a InstructionFile>,
) -> Vec<&'a InstructionFile> {
    let mut ordered: Vec<&InstructionFile> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: std::collections::VecDeque<&InstructionFile> = entries
        .iter()
        .copied()
        .filter(|file| seen.insert(file.path.as_str()))
        .inspect(|file| ordered.push(file))
        .collect();

    while let Some(current) = queue.pop_front() {
        for target in &current.redirects_to {
            // Only files that exist and were scanned. A redirect to something
            // absent is already reported as a broken link; adding it here would
            // charge an agent for bytes nobody can read.
            let Some(next) = by_path.get(target.as_str()) else {
                continue;
            };
            if seen.insert(next.path.as_str()) {
                ordered.push(next);
                queue.push_back(next);
            }
        }
    }
    ordered
}

/// Follow `redirects_to` from each entry point and report any chain that revisits
/// a file.
///
/// A cycle costs an agent the whole loop before it can start, and it is invisible
/// in any single file — which is why this is computed per agent rather than
/// per file.
fn find_cycles(
    entries: &[&InstructionFile],
    by_path: &HashMap<&str, &InstructionFile>,
) -> Vec<Vec<String>> {
    let mut cycles = Vec::new();
    for entry in entries {
        let mut chain = vec![entry.path.clone()];
        let mut current = *entry;
        // Bounded: a chain longer than the file count has already repeated.
        for _ in 0..by_path.len() + 1 {
            // The first target the file names. Following every edge would report
            // the same loop once per path into it; stated as a limitation rather
            // than presented as exhaustive cycle detection.
            let Some(next) = current
                .redirects_to
                .iter()
                // A file naming its own path is a self-reference, not a cycle: the
                // file is already loaded, so following it costs nothing. Reported
                // on a real repository it produced 11 findings and no cost.
                .find(|target| **target != current.path)
            else {
                break;
            };
            if chain.contains(next) {
                let mut cycle = chain.clone();
                cycle.push(next.clone());
                if !cycles.contains(&cycle) {
                    cycles.push(cycle);
                }
                break;
            }
            chain.push(next.clone());
            match by_path.get(next.as_str()) {
                Some(file) => current = file,
                None => break,
            }
        }
    }
    cycles
}

/// Split on `#` headings and classify each section.
fn classify_sections(
    content: &str,
    path: &str,
    paragraph_owners: &HashMap<String, Vec<String>>,
    existing_paths: &HashSet<String>,
) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut heading = String::from("(preamble)");
    let mut body = String::new();

    let flush = |heading: &str, body: &str, sections: &mut Vec<Section>| {
        if body.trim().is_empty() {
            return;
        }
        let (class, because) = classify_one(body, path, paragraph_owners, existing_paths);
        sections.push(Section {
            heading: heading.to_string(),
            bytes: body.len(),
            class,
            because,
        });
    };

    for line in content.lines() {
        if line.starts_with('#') {
            flush(&heading, &body, &mut sections);
            heading = line.trim_start_matches('#').trim().to_string();
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&heading, &body, &mut sections);
    sections
}

/// The classification rules, in the order that matters.
///
/// Duplication and obsolescence are checked BEFORE the imperative test: a MUST
/// that is stated identically in two files is still a duplicate, and calling it
/// critical in both places is how the duplication survives every audit.
fn classify_one(
    body: &str,
    path: &str,
    paragraph_owners: &HashMap<String, Vec<String>>,
    existing_paths: &HashSet<String>,
) -> (SectionClass, String) {
    let duplicated: Vec<&String> = paragraphs(body)
        .iter()
        .filter(|paragraph| paragraph.len() >= DUPLICATE_MIN_BYTES)
        .filter_map(|paragraph| paragraph_owners.get(paragraph))
        .flatten()
        .filter(|owner| owner.as_str() != path)
        .collect();
    if let Some(other) = duplicated.first() {
        return (
            SectionClass::Duplicated,
            format!("said in full in `{other}` too"),
        );
    }

    // Same resolution as the file-level check: relative to the containing file.
    let dead: Vec<String> = markdown_links(body)
        .into_iter()
        .filter(|target| is_broken(target, path, existing_paths))
        .collect();
    if let Some(target) = dead.first() {
        return (
            SectionClass::PossiblyObsolete,
            format!("references `{target}`, which no longer exists"),
        );
    }

    if is_historical(body) {
        return (
            SectionClass::Historical,
            "recounts something that already happened; nobody needs it to start a task".to_string(),
        );
    }

    // Before Critical: a pointer is cheap and is the mechanism tiering relies on,
    // even when the thing it points at is mandatory.
    if is_routed(body) {
        return (
            SectionClass::Routed,
            "a pointer rather than content".to_string(),
        );
    }

    if has_imperative(body) {
        return (
            SectionClass::Critical,
            "states a hard rule; moving it changes behaviour".to_string(),
        );
    }

    (
        SectionClass::Universal,
        "applies broadly but states no hard rule".to_string(),
    )
}

fn has_imperative(body: &str) -> bool {
    let upper = body.to_uppercase();
    [
        "MUST", "NEVER", "ALWAYS", "DO NOT", "REQUIRED", "JAMAIS", "TOUJOURS",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn is_routed(body: &str) -> bool {
    let trimmed = body.trim();
    // Short AND carrying a link: a long section with a link is content that
    // happens to cite something.
    trimmed.len() < 600 && (trimmed.contains("](") || trimmed.to_lowercase().contains("see "))
}

/// Dated narrative: a version list, an incident, a changelog fragment.
fn is_historical(body: &str) -> bool {
    let lower = body.to_lowercase();
    let dated = regex_lite::Regex::new(r"20\d\d-\d\d-\d\d")
        .map(|re| re.is_match(body))
        .unwrap_or(false);
    let narrative = [
        "incident",
        "release history",
        "changelog",
        "was fixed",
        "used to be",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    dated && narrative
}

/// Non-trivial paragraphs, normalised so whitespace differences do not hide a
/// duplicate.
fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|block| block.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|block| block.len() >= 40)
        .collect()
}

fn markdown_links(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        if let Some(close) = rest.find(')') {
            let target = rest[..close].trim();
            if !target.is_empty() {
                targets.push(target.split('#').next().unwrap_or(target).to_string());
            }
            rest = &rest[close..];
        } else {
            break;
        }
    }
    targets
}

/// Resolve a markdown link against the directory of the file containing it.
///
/// The first version treated every target as repo-root-relative, and the audit
/// then reported 8 of 8 findings falsely on Kronn: `docs/AGENTS.md` links to
/// `conventions/agents-md-format-v1.md`, which is `docs/conventions/…` and exists.
/// A markdown link is relative to its own file — the same rule every renderer
/// uses — and an audit that is confidently wrong is worse than one that says less.
fn resolve_link(target: &str, from_file: &str) -> String {
    // A leading `/` is the one root-relative form.
    if let Some(rooted) = target.strip_prefix('/') {
        return rooted.to_string();
    }
    let dir = match from_file.rfind('/') {
        Some(index) => &from_file[..index],
        None => "",
    };
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Whether a link points at nothing.
///
/// Checked BOTH resolved and as written: a repository whose docs use root-relative
/// links without a leading slash is a real convention, and reporting every one of
/// them as broken would make the audit unusable there. Only a target that resolves
/// nowhere either way is a finding.
fn is_broken(target: &str, from_file: &str, existing_paths: &HashSet<String>) -> bool {
    if !is_repo_path(target) {
        return false;
    }
    let resolved = resolve_link(target, from_file);
    !existing_paths.contains(&resolved) && !existing_paths.contains(target)
}

/// A link into the repository, as opposed to a URL or an anchor.
fn is_repo_path(target: &str) -> bool {
    !target.starts_with("http")
        && !target.starts_with('#')
        && !target.starts_with("mailto:")
        && !target.is_empty()
}

/// Backticked paths naming an instruction file: `` `docs/AGENTS.md` ``.
///
/// Deliberately narrow. Only a path whose FILENAME is one of the known instruction
/// files counts, so an ordinary mention of a source file is not read as a
/// redirect — and the caller additionally requires the target to exist. The
/// alternative, matching any wording like "read X", would classify prose.
fn instruction_mentions(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for candidate in text.split('`').skip(1).step_by(2) {
        let candidate = candidate.trim();
        if candidate.is_empty() || candidate.contains(char::is_whitespace) {
            continue;
        }
        if is_instruction_path(candidate) && !found.contains(&candidate.to_string()) {
            found.push(candidate.to_string());
        }
    }
    found
}

fn is_instruction_path(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    name.eq_ignore_ascii_case("AGENTS.md")
        || name.eq_ignore_ascii_case("CLAUDE.md")
        || name.eq_ignore_ascii_case("GEMINI.md")
        || name == "copilot-instructions.md"
}

/// Propose where movable sections could go.
///
/// Only classes that are movable by definition. A `Critical` section is never
/// proposed, however large: deciding that a hard rule is not needed up front is a
/// human's call, not a heuristic's.
fn propose_tiers(files: &[InstructionFile]) -> Vec<TierMove> {
    let mut moves = Vec::new();
    for file in files {
        for section in &file.sections {
            if !section.class.is_movable() {
                continue;
            }
            let to_tier = match section.class {
                // Already written down elsewhere: replace with a pointer.
                SectionClass::Duplicated => 2,
                SectionClass::Historical => 3,
                SectionClass::PossiblyObsolete => 3,
                _ => continue,
            };
            moves.push(TierMove {
                from_path: file.path.clone(),
                heading: section.heading.clone(),
                bytes: section.bytes,
                to_tier,
                because: section.because.clone(),
            });
        }
    }
    // Biggest saving first, so a human reading only the top of the list sees the
    // moves worth making.
    moves.sort_by_key(|item| std::cmp::Reverse(item.bytes));
    moves
}

/// Read a repository's instruction files from disk.
///
/// Only paths the conventions name. A recursive sweep would pick up every markdown
/// file in the tree and report a documentation site as an agent's context.
pub fn scan_repo(root: &Path) -> (Vec<ScannedFile>, HashSet<String>) {
    let mut wanted: Vec<String> = Vec::new();
    for convention in CONVENTIONS {
        for pattern in convention.paths {
            match pattern.strip_suffix('*') {
                None => wanted.push((*pattern).to_string()),
                Some(prefix) => {
                    if let Ok(entries) = std::fs::read_dir(root.join(prefix)) {
                        for entry in entries.flatten() {
                            if entry.path().is_file() {
                                wanted.push(format!(
                                    "{prefix}{}",
                                    entry.file_name().to_string_lossy()
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    // Hierarchical AGENTS.md: the ones a nested task would also load.
    for nested in nested_agents_files(root) {
        wanted.push(nested);
    }
    wanted.sort();
    wanted.dedup();
    wanted.truncate(MAX_FILES_SCANNED);

    let mut existing: HashSet<String> = HashSet::new();
    let mut scanned = Vec::new();
    for relative in wanted {
        let full = root.join(&relative);
        if !full.is_file() {
            continue;
        }
        existing.insert(relative.clone());
        let content = match std::fs::metadata(&full) {
            Ok(meta) if meta.len() as usize > MAX_FILE_BYTES => None,
            _ => std::fs::read_to_string(&full).ok(),
        };
        scanned.push(ScannedFile {
            path: relative,
            content,
        });
    }

    // Link targets are checked against everything present, not only instruction
    // files, or every reference to a source file would be reported as broken.
    collect_existing(root, root, &mut existing, 0);
    (scanned, existing)
}

/// Repo-relative paths, two directories deep. Bounded because the only use is
/// resolving links, and a full sweep of a monorepo costs more than the audit.
fn collect_existing(root: &Path, dir: &Path, out: &mut HashSet<String>, depth: usize) {
    if depth > 3 || out.len() > 20_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "node_modules" || name == "target" || name == ".git" {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            out.insert(relative.to_string_lossy().to_string());
        }
        if path.is_dir() {
            collect_existing(root, &path, out, depth + 1);
        }
    }
}

fn nested_agents_files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for candidate in ["docs", "backend", "frontend", "src", "apps", "packages"] {
        let path = PathBuf::from(candidate).join("AGENTS.md");
        if root.join(&path).is_file() {
            found.push(path.to_string_lossy().to_string());
        }
    }
    found
}

/// Audit a repository on disk.
pub fn audit_repo(root: &Path) -> ContextAudit {
    let (files, existing) = scan_repo(root);
    analyse(files, &existing)
}

/// What changed since a previous audit — KT-194 drift.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Drift {
    /// Files that grew, with the delta.
    pub grown: Vec<(String, i64)>,
    /// Instruction files that did not exist before. Reported because a new one
    /// changes every session's cost without anyone deciding to.
    pub new_files: Vec<String>,
    pub newly_broken_routes: Vec<String>,
    /// Files no agent reads. Real cost, zero effect — the pack nobody loads.
    pub unused_files: Vec<String>,
    /// Growth of the effective instruction payload per agent. This is the
    /// paid, repeated cost users need to act on; total documentation size is
    /// intentionally absent because most docs are loaded only on demand.
    pub paid_agent_growth: Vec<AgentContextGrowth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentContextGrowth {
    pub agent: String,
    pub previous_bytes: usize,
    pub current_bytes: usize,
    pub delta_bytes: usize,
}

/// Compare two audits.
///
/// A file that disappeared is NOT reported as a shrink: losing an instruction file
/// is a different event from trimming one, and folding them together would show a
/// deletion as an improvement.
pub fn drift(previous: &ContextAudit, current: &ContextAudit) -> Drift {
    let before: HashMap<&str, Option<usize>> = previous
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.bytes))
        .collect();

    let mut grown = Vec::new();
    let mut new_files = Vec::new();
    for file in &current.files {
        match before.get(file.path.as_str()) {
            None => new_files.push(file.path.clone()),
            Some(previous_bytes) => {
                if let (Some(now), Some(was)) = (file.bytes, previous_bytes) {
                    if now > *was {
                        grown.push((file.path.clone(), now as i64 - *was as i64));
                    }
                }
            }
        }
    }
    grown.sort_by_key(|(_, delta)| std::cmp::Reverse(*delta));

    let broken_before: HashSet<&String> = previous
        .files
        .iter()
        .flat_map(|file| &file.broken_links)
        .collect();
    let newly_broken_routes = current
        .files
        .iter()
        .flat_map(|file| &file.broken_links)
        .filter(|target| !broken_before.contains(target))
        .cloned()
        .collect();

    let unused_files = current
        .files
        .iter()
        .filter(|file| file.agents.is_empty())
        .map(|file| file.path.clone())
        .collect();

    let previous_agents: HashMap<&str, Option<usize>> = previous
        .per_agent
        .iter()
        .map(|context| (context.agent.as_str(), context.total_bytes))
        .collect();
    let mut paid_agent_growth: Vec<AgentContextGrowth> = current
        .per_agent
        .iter()
        .filter_map(|context| {
            let previous_bytes = previous_agents.get(context.agent.as_str())?.as_ref()?;
            let current_bytes = context.total_bytes?;
            (current_bytes > *previous_bytes).then(|| AgentContextGrowth {
                agent: context.agent.clone(),
                previous_bytes: *previous_bytes,
                current_bytes,
                delta_bytes: current_bytes - *previous_bytes,
            })
        })
        .collect();
    paid_agent_growth.sort_by_key(|growth| std::cmp::Reverse(growth.delta_bytes));

    Drift {
        grown,
        new_files,
        newly_broken_routes,
        unused_files,
        paid_agent_growth,
    }
}

/// Render the audit for a reader.
///
/// Findings first, then the proposal, then the per-agent totals. Same ordering
/// rule as the review payload: what someone must act on cannot be the part a
/// truncation drops.
pub fn render(audit: &ContextAudit) -> String {
    let mut out = String::from("CONTEXT ARCHITECTURE AUDIT\n");

    if audit.no_convention_found {
        out.push_str(
            "\nNo known instruction file found. Agents start here with no project \
             context at all — this is not a clean result.\n",
        );
    }

    if !audit.findings.is_empty() {
        out.push_str(&format!("\nFINDINGS ({}):\n", audit.findings.len()));
        for finding in &audit.findings {
            out.push_str(&format!(
                "- [{}] {}: {}\n",
                finding.severity, finding.where_, finding.what
            ));
        }
    }

    if !audit.proposal.is_empty() {
        let movable: usize = audit.proposal.iter().map(|item| item.bytes).sum();
        out.push_str(&format!(
            "\nPROPOSED MOVES ({}, {movable} B) — a proposal, nothing is rewritten:\n",
            audit.proposal.len()
        ));
        for item in &audit.proposal {
            out.push_str(&format!(
                "- tier {} ← {} § {} ({} B): {}\n",
                item.to_tier, item.from_path, item.heading, item.bytes, item.because
            ));
        }
    }

    out.push_str("\nPER AGENT:\n");
    for context in &audit.per_agent {
        let total = match context.total_bytes {
            Some(bytes) => format!("{bytes} B"),
            // Never "0 B": one unreadable file makes the cost unknown.
            None => format!("unknown ({} unreadable)", context.unreadable_files.len()),
        };
        out.push_str(&format!(
            "- {}: {} file(s), {}{}\n",
            context.agent,
            context.files.len(),
            total,
            if context.duplicated_bytes > 0 {
                format!(", {} B duplicated", context.duplicated_bytes)
            } else {
                String::new()
            }
        ));
    }

    if out.len() > AUDIT_REPORT_MAX_BYTES {
        let mut cut = AUDIT_REPORT_MAX_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n… truncated\n");
    }
    out
}

#[cfg(test)]
#[path = "context_audit_test.rs"]
mod context_audit_test;
