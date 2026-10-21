//! Cross-agent audit (KT-1183, experimental): several agents detect the same
//! finding step independently, then challenge each other's findings.
//!
//! Founding rule: never a vote. How many agents found a finding is displayed,
//! never weighed — `arbitrate` does not even receive it. Only the deterministic
//! citation check and the proof attached to a refutation or a reply decide.
//!
//! Everything here is deterministic: parsing the agents' machine-readable
//! blocks, the citation check, matching, challenger assignment, arbitration and
//! rendering. The model calls (detection, refutation, reply) live in
//! `api::audit::cross_audit`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::anti_halluc::{verify_source_marker, SourceStatus};
use crate::models::AgentType;

/// Briefing line that turns the option on, followed by the chosen agents.
pub const BRIEFING_OPTION_PREFIX: &str = "- Cross-agent audit (experimental):";
/// The finding steps run cross-agent. The documentation steps never are.
pub const CROSS_CHECKED_TARGETS: &[&str] = &["docs/inconsistencies-security.md"];
pub const MIN_AGENTS: usize = 2;
pub const MAX_AGENTS: usize = 3;
/// Findings challenged per step; the rest stay in the audit, listed as not cross-checked.
pub const MAX_CROSS_CHECKED: usize = 30;
/// Findings read from one agent's detection block.
pub const MAX_FINDINGS_PER_AGENT: usize = 60;
/// Lines around the cited range where the quote may sit (agents miscount by one or two).
pub const LINE_TOLERANCE: usize = 2;
/// A quote shorter than this proves nothing.
pub const MIN_QUOTE_CHARS: usize = 4;
/// Two findings of one file and category this close are about the same thing.
pub const MATCH_LINE_WINDOW: u32 = 10;
pub const FINDINGS_FENCE: &str = "kronn-findings";
pub const VERDICTS_FENCE: &str = "kronn-verdicts";
pub const REPLIES_FENCE: &str = "kronn-replies";
const BLOCK_START: &str = "<!-- kronn:cross-audit:start -->";
const BLOCK_END: &str = "<!-- kronn:cross-audit:end -->";
const TEXT_MAX: usize = 600;

// ── Option ──────────────────────────────────────────────────────────────────

/// Agents a cross-agent audit may use: audit-capable and launchable without a
/// named connection.
pub fn eligible_agent(agent: &AgentType) -> bool {
    crate::api::audit::agent_can_audit(agent) && *agent != AgentType::Custom
}

/// The briefing line for `agents`.
pub fn briefing_option_line(agents: &[AgentType]) -> String {
    let names: Vec<String> = agents.iter().map(agent_label).collect();
    format!("{BRIEFING_OPTION_PREFIX} {}", names.join(", "))
}

/// The chosen agents when the briefing turns the option on with 2 or 3
/// distinct eligible agents; `None` otherwise, which keeps the audit single-agent.
pub fn briefing_cross_agents(notes: &str) -> Option<Vec<AgentType>> {
    let rest = notes
        .lines()
        .find_map(|line| line.trim().strip_prefix(BRIEFING_OPTION_PREFIX))?;
    parse_agent_list(rest)
}

/// Comma-separated agent names, validated like the briefing line.
pub fn parse_agent_list(list: &str) -> Option<Vec<AgentType>> {
    let mut agents: Vec<AgentType> = Vec::new();
    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let agent: AgentType =
            serde_json::from_value(serde_json::Value::String(name.into())).ok()?;
        if !eligible_agent(&agent) {
            return None;
        }
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    (MIN_AGENTS..=MAX_AGENTS)
        .contains(&agents.len())
        .then_some(agents)
}

pub fn is_cross_checked_target(target_file: &str) -> bool {
    CROSS_CHECKED_TARGETS.contains(&target_file)
}

pub fn agent_label(agent: &AgentType) -> String {
    format!("{agent:?}")
}

// ── Machine-readable blocks ─────────────────────────────────────────────────

/// A place in the repository plus the text found there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub file: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawFinding {
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub severity: String,
    #[serde(flatten)]
    pub citation: Citation,
    #[serde(default)]
    pub impact: String,
    #[serde(default)]
    pub remediation: String,
    /// An existing TD sheet this finding is, reused instead of a new slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub td_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistLine {
    pub item: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionOutput {
    #[serde(default)]
    pub findings: Vec<RawFinding>,
    #[serde(default)]
    pub checklist: Vec<ChecklistLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Confirm,
    Refute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub id: String,
    pub verdict: VerdictKind,
    #[serde(default)]
    pub argument: String,
    #[serde(default)]
    pub proof: Vec<Citation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictsOutput {
    #[serde(default)]
    pub verdicts: Vec<Verdict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Maintain,
    Withdraw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub stance: Stance,
    #[serde(default)]
    pub argument: String,
    #[serde(default)]
    pub proof: Vec<Citation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepliesOutput {
    #[serde(default)]
    pub replies: Vec<Reply>,
}

/// The JSON of the LAST ```<fence> block in `text`: an agent may restate the
/// format before answering.
pub fn parse_fenced<T: serde::de::DeserializeOwned>(text: &str, fence: &str) -> Result<T, String> {
    let mut last: Option<String> = None;
    let mut open: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        match open.as_mut() {
            Some(body) => {
                if trimmed == "```" {
                    last = open.take();
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            None => {
                if trimmed.strip_prefix("```").map(str::trim) == Some(fence) {
                    open = Some(String::new());
                }
            }
        }
    }
    let body = last.ok_or_else(|| format!("no closed ```{fence} block in the answer"))?;
    serde_json::from_str(&body).map_err(|e| format!("```{fence} block is not valid JSON: {e}"))
}

// ── Deterministic citation check ────────────────────────────────────────────

fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

/// The file exists inside the project, the lines exist, and the quote is
/// there. Same path and bounds verifier as the `[src:]` markers.
pub fn verify_citation(root: &Path, citation: &Citation) -> Result<(), String> {
    let file = normalized_path(&citation.file);
    if file.is_empty() {
        return Err("citation names no file".into());
    }
    if citation.line == 0 {
        return Err(format!("citation line of `{file}` must start at 1"));
    }
    let end = citation
        .line_end
        .unwrap_or(citation.line)
        .max(citation.line);
    let marker = if end > citation.line {
        format!("file: {file}:{}-{end}", citation.line)
    } else {
        format!("file: {file}:{}", citation.line)
    };
    let check = verify_source_marker(&marker, Some(root));
    if check.status != SourceStatus::Verified {
        return Err(format!("`{file}:{}`: {}", citation.line, check.detail));
    }
    let quote = normalize_ws(&citation.quote);
    if quote.chars().filter(|c| !c.is_whitespace()).count() < MIN_QUOTE_CHARS {
        return Err(format!(
            "`{file}:{}`: the quote is shorter than {MIN_QUOTE_CHARS} characters",
            citation.line
        ));
    }
    let text = std::fs::read_to_string(root.join(&file))
        .map_err(|e| format!("`{file}` is not readable as text: {e}"))?;
    let lines: Vec<&str> = text.lines().collect();
    let from = (citation.line as usize).saturating_sub(1 + LINE_TOLERANCE);
    let to = (end as usize + LINE_TOLERANCE).min(lines.len());
    let window = normalize_ws(&lines[from.min(to)..to].join("\n"));
    if window.contains(&quote) {
        Ok(())
    } else {
        Err(format!(
            "`{file}:{}`: the quoted text is not at the cited lines",
            citation.line
        ))
    }
}

/// A proof stands only when it has at least one citation and every one verifies.
pub fn verify_proof(root: &Path, proof: &[Citation]) -> Result<(), String> {
    if proof.is_empty() {
        return Err("no proof cited".into());
    }
    let failures: Vec<String> = proof
        .iter()
        .filter_map(|c| verify_citation(root, c).err())
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

// ── Detection → filter → matching ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFinding {
    pub author: AgentType,
    pub finding: RawFinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DroppedFinding {
    pub author: AgentType,
    pub finding: RawFinding,
    pub reason: String,
}

/// Keeps the findings whose citation verifies. A false citation drops the
/// finding here, before any model sees it.
pub fn filter_findings(
    root: &Path,
    detections: &[(AgentType, DetectionOutput)],
) -> (Vec<AgentFinding>, Vec<DroppedFinding>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (author, output) in detections {
        for finding in output.findings.iter().take(MAX_FINDINGS_PER_AGENT) {
            let finding = finding.clone();
            let reason = if finding.title.trim().is_empty() {
                Err("the finding has no title".to_string())
            } else {
                verify_citation(root, &finding.citation)
            };
            match reason {
                Ok(()) => kept.push(AgentFinding {
                    author: author.clone(),
                    finding,
                }),
                Err(reason) => dropped.push(DroppedFinding {
                    author: author.clone(),
                    finding,
                    reason,
                }),
            }
        }
    }
    (kept, dropped)
}

/// Findings of several agents about the same thing. Information only.
#[derive(Debug, Clone, PartialEq)]
pub struct FindingGroup {
    pub id: String,
    /// The first member: the finding submitted to the cross-check.
    pub members: Vec<AgentFinding>,
}

impl FindingGroup {
    pub fn representative(&self) -> &AgentFinding {
        &self.members[0]
    }

    pub fn detected_by(&self) -> Vec<AgentType> {
        let mut out: Vec<AgentType> = Vec::new();
        for m in &self.members {
            if !out.contains(&m.author) {
                out.push(m.author.clone());
            }
        }
        out
    }
}

fn category_key(finding: &RawFinding) -> String {
    finding.category.trim().to_lowercase()
}

/// Groups findings of one file and category whose lines are within
/// `MATCH_LINE_WINDOW`. One agent's two findings never merge.
pub fn match_findings(kept: Vec<AgentFinding>) -> Vec<FindingGroup> {
    let mut groups: Vec<FindingGroup> = Vec::new();
    for finding in kept {
        let path = normalized_path(&finding.finding.citation.file);
        let category = category_key(&finding.finding);
        let line = finding.finding.citation.line;
        let target = groups.iter_mut().find(|g| {
            let rep = &g.representative().finding;
            normalized_path(&rep.citation.file) == path
                && category_key(rep) == category
                && rep.citation.line.abs_diff(line) <= MATCH_LINE_WINDOW
                && !g.members.iter().any(|m| m.author == finding.author)
        });
        match target {
            Some(group) => group.members.push(finding),
            None => {
                let id = format!("F{}", groups.len() + 1);
                groups.push(FindingGroup {
                    id,
                    members: vec![finding],
                })
            }
        }
    }
    groups
}

fn severity_rank(severity: &str) -> u8 {
    match severity.trim().to_lowercase().as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

/// Which group each agent challenges, for the first `MAX_CROSS_CHECKED`
/// groups by severity. The challenger never authored the finding and, when
/// possible, did not detect it either. The rest is not cross-checked.
pub fn assign_challengers(
    groups: &[FindingGroup],
    agents: &[AgentType],
) -> (BTreeMap<String, AgentType>, Vec<String>) {
    let mut order: Vec<(usize, &FindingGroup)> = groups.iter().enumerate().collect();
    order.sort_by_key(|(i, g)| (severity_rank(&g.representative().finding.severity), *i));
    let mut assigned = BTreeMap::new();
    let mut unchecked = Vec::new();
    for (rank, (_, group)) in order.into_iter().enumerate() {
        if rank >= MAX_CROSS_CHECKED {
            unchecked.push(group.id.clone());
            continue;
        }
        let author = &group.representative().author;
        let detectors = group.detected_by();
        let start = agents.iter().position(|a| a == author).unwrap_or(0);
        let candidates: Vec<&AgentType> = (1..agents.len())
            .map(|k| &agents[(start + k) % agents.len()])
            .filter(|a| *a != author)
            .collect();
        let challenger = candidates
            .iter()
            .find(|a| !detectors.contains(a))
            .or_else(|| candidates.first());
        match challenger {
            Some(agent) => {
                assigned.insert(group.id.clone(), (*agent).clone());
            }
            None => unchecked.push(group.id.clone()),
        }
    }
    (assigned, unchecked)
}

// ── Cross-check, reply, arbitration ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChallengeResult {
    Confirmed {
        argument: String,
    },
    /// Refuted, and every cited proof verified.
    RefutedProven {
        argument: String,
        proof: Vec<Citation>,
    },
    /// Refuted without a proof that verifies.
    RefutedUnproven {
        argument: String,
        proof: Vec<Citation>,
        why: String,
    },
    /// The challenger gave no usable verdict for this finding.
    NoAnswer {
        why: String,
    },
}

pub fn evaluate_verdict(
    root: &Path,
    verdict: Option<&Verdict>,
    failure: Option<&str>,
) -> ChallengeResult {
    let Some(verdict) = verdict else {
        return ChallengeResult::NoAnswer {
            why: failure
                .unwrap_or("the challenger returned no verdict for this finding")
                .to_string(),
        };
    };
    match verdict.verdict {
        VerdictKind::Confirm => ChallengeResult::Confirmed {
            argument: verdict.argument.clone(),
        },
        VerdictKind::Refute => match verify_proof(root, &verdict.proof) {
            Ok(()) => ChallengeResult::RefutedProven {
                argument: verdict.argument.clone(),
                proof: verdict.proof.clone(),
            },
            Err(why) => ChallengeResult::RefutedUnproven {
                argument: verdict.argument.clone(),
                proof: verdict.proof.clone(),
                why,
            },
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyResult {
    MaintainedProven {
        argument: String,
        proof: Vec<Citation>,
    },
    MaintainedUnproven {
        argument: String,
        why: String,
    },
    Withdrawn {
        argument: String,
    },
    NoReply {
        why: String,
    },
}

pub fn evaluate_reply(root: &Path, reply: Option<&Reply>, failure: Option<&str>) -> ReplyResult {
    let Some(reply) = reply else {
        return ReplyResult::NoReply {
            why: failure
                .unwrap_or("the author returned no reply for this finding")
                .to_string(),
        };
    };
    match reply.stance {
        Stance::Withdraw => ReplyResult::Withdrawn {
            argument: reply.argument.clone(),
        },
        Stance::Maintain => match verify_proof(root, &reply.proof) {
            Ok(()) => ReplyResult::MaintainedProven {
                argument: reply.argument.clone(),
                proof: reply.proof.clone(),
            },
            Err(why) => ReplyResult::MaintainedUnproven {
                argument: reply.argument.clone(),
                why,
            },
        },
    }
}

/// Whether the author gets a right of reply: only a proven refutation can
/// dismiss a finding, so only it is answered.
pub fn needs_reply(challenge: &ChallengeResult) -> bool {
    matches!(challenge, ChallengeResult::RefutedProven { .. })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Confirmed,
    NotRefuted,
    /// Refuted without verifiable proof: stays in the audit.
    Contested,
    /// Refuted with verified proof, not answered with verified proof.
    Dismissed,
    /// Proven refutation AND proven reply: a human decides.
    Disputed,
    NotCrossChecked,
}

impl Outcome {
    pub fn in_audit(self) -> bool {
        !matches!(self, Outcome::Dismissed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Outcome::Confirmed => "confirmed",
            Outcome::NotRefuted => "not refuted",
            Outcome::Contested => "contested (refutation without verified proof)",
            Outcome::Dismissed => "dismissed (refuted with verified proof)",
            Outcome::Disputed => "disputed (awaiting a human decision)",
            Outcome::NotCrossChecked => "not cross-checked",
        }
    }
}

/// The verdict. It receives the cross-check and the reply only: the number
/// of agents that found the finding cannot weigh in.
pub fn arbitrate(challenge: Option<&ChallengeResult>, reply: Option<&ReplyResult>) -> Outcome {
    match challenge {
        None => Outcome::NotCrossChecked,
        Some(ChallengeResult::Confirmed { .. }) => Outcome::Confirmed,
        Some(ChallengeResult::NoAnswer { .. }) => Outcome::NotRefuted,
        Some(ChallengeResult::RefutedUnproven { .. }) => Outcome::Contested,
        Some(ChallengeResult::RefutedProven { .. }) => match reply {
            Some(ReplyResult::MaintainedProven { .. }) => Outcome::Disputed,
            _ => Outcome::Dismissed,
        },
    }
}

// ── Provenance ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProvenance {
    pub agent: String,
    pub citation: Citation,
}

/// One finding after the cross-check, as persisted and rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossFindingRecord {
    pub group_id: String,
    pub td_id: Option<String>,
    pub title: String,
    pub category: String,
    pub severity: String,
    pub citation: Citation,
    pub impact: String,
    pub remediation: String,
    pub author: String,
    pub detected_by: Vec<String>,
    /// Information only, never an input of the verdict.
    pub detection_count: usize,
    pub agents_total: usize,
    pub members: Vec<MemberProvenance>,
    pub challenger: Option<String>,
    pub challenge: Option<ChallengeResult>,
    pub reply: Option<ReplyResult>,
    pub outcome: Outcome,
    pub question_key: Option<String>,
    #[serde(skip)]
    pub proposed_td_id: Option<String>,
}

pub fn build_record(
    group: &FindingGroup,
    agents_total: usize,
    challenger: Option<&AgentType>,
    challenge: Option<ChallengeResult>,
    reply: Option<ReplyResult>,
) -> CrossFindingRecord {
    let rep = group.representative();
    let outcome = arbitrate(challenge.as_ref(), reply.as_ref());
    let detected_by: Vec<String> = group.detected_by().iter().map(agent_label).collect();
    CrossFindingRecord {
        group_id: group.id.clone(),
        td_id: None,
        title: rep.finding.title.trim().to_string(),
        category: match rep.finding.category.trim() {
            "" => "security".to_string(),
            c => c.to_lowercase(),
        },
        severity: normalized_severity(&rep.finding.severity),
        citation: Citation {
            file: normalized_path(&rep.finding.citation.file),
            ..rep.finding.citation.clone()
        },
        impact: rep.finding.impact.trim().to_string(),
        remediation: rep.finding.remediation.trim().to_string(),
        author: agent_label(&rep.author),
        detection_count: detected_by.len(),
        detected_by,
        agents_total,
        members: group
            .members
            .iter()
            .map(|m| MemberProvenance {
                agent: agent_label(&m.author),
                citation: m.finding.citation.clone(),
            })
            .collect(),
        challenger: challenger.map(agent_label),
        challenge,
        reply,
        outcome,
        question_key: None,
        proposed_td_id: group.members.iter().find_map(|m| m.finding.td_id.clone()),
    }
}

fn normalized_severity(severity: &str) -> String {
    match severity_rank(severity) {
        0 => "Critical",
        1 => "High",
        2 => "Medium",
        3 => "Low",
        _ => "Medium",
    }
    .to_string()
}

// ── TD ids and rendering ────────────────────────────────────────────────────

fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "finding".into()
    } else {
        out
    }
}

/// The sheet id: the agent's proposed id when a sheet already holds it (no
/// slug churn), else `TD-<date>-<slug>`, unique within the run.
pub fn assign_td_ids(records: &mut [CrossFindingRecord], tech_debt_dir: &Path, yyyymmdd: &str) {
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for record in records.iter_mut().filter(|r| r.outcome.in_audit()) {
        let reused = record
            .proposed_td_id
            .as_ref()
            .filter(|id| crate::core::td_index::is_td_id(id))
            .filter(|id| tech_debt_dir.join(format!("{id}.md")).is_file())
            .filter(|id| !taken.contains(*id))
            .cloned();
        let id = reused.unwrap_or_else(|| {
            let base = format!("TD-{yyyymmdd}-{}", slug(&record.title));
            let mut id = base.clone();
            let mut n = 2;
            while taken.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        });
        taken.insert(id.clone());
        record.td_id = Some(id);
    }
}

/// Agent text made safe for a Kronn-written document: one line, bounded, no
/// citation marker, placeholder or comment it could smuggle in.
pub fn sanitize_inline(text: &str) -> String {
    let flat = normalize_ws(text)
        .replace("[src:", "[src :")
        .replace("{{", "{ {")
        .replace("}}", "} }")
        .replace("<!--", "&lt;!--")
        .replace('|', "\\|");
    let mut out: String = flat.chars().take(TEXT_MAX).collect();
    if flat.chars().count() > TEXT_MAX {
        out.push('…');
    }
    out
}

/// Backticks would break out of an inline code span.
fn code_path(path: &str) -> String {
    sanitize_inline(path).replace('`', "'")
}

fn src(citation: &Citation) -> String {
    format!(
        "[src: file: {}:{}]",
        code_path(&citation.file),
        citation.line
    )
}

fn proof_line(proof: &[Citation]) -> String {
    proof.iter().map(src).collect::<Vec<_>>().join(" ")
}

fn found_by_line(record: &CrossFindingRecord) -> String {
    record
        .detected_by
        .iter()
        .map(|a| {
            if *a == record.author {
                format!("{a} (author)")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Kronn-owned provenance block of a TD sheet.
pub fn provenance_block(record: &CrossFindingRecord) -> String {
    let mut lines = vec![
        BLOCK_START.to_string(),
        "## Cross-agent audit (experimental)".to_string(),
        String::new(),
        format!("- **Verdict**: {}", record.outcome.label()),
        format!("- **Found by**: {}", found_by_line(record)),
        format!(
            "- **Detection count**: {} of {} agents (information only, never a verdict)",
            record.detection_count, record.agents_total
        ),
    ];
    match (&record.challenger, &record.challenge) {
        (Some(by), Some(challenge)) => {
            let line = match challenge {
                ChallengeResult::Confirmed { argument } => {
                    format!("confirmed: {}", sanitize_inline(argument))
                }
                ChallengeResult::RefutedProven { argument, proof } => format!(
                    "refuted with verified proof {}: {}",
                    proof_line(proof),
                    sanitize_inline(argument)
                ),
                ChallengeResult::RefutedUnproven { argument, why, .. } => format!(
                    "refuted without verified proof ({}): {}",
                    sanitize_inline(why),
                    sanitize_inline(argument)
                ),
                ChallengeResult::NoAnswer { why } => {
                    format!("no verdict ({})", sanitize_inline(why))
                }
            };
            lines.push(format!("- **Checked by**: {by} — {line}"));
        }
        _ => lines.push(format!(
            "- **Checked by**: nobody — beyond the cap of {MAX_CROSS_CHECKED} cross-checked findings"
        )),
    }
    if let Some(reply) = &record.reply {
        let line = match reply {
            ReplyResult::MaintainedProven { argument, proof } => format!(
                "maintained with verified proof {}: {}",
                proof_line(proof),
                sanitize_inline(argument)
            ),
            ReplyResult::MaintainedUnproven { argument, why } => format!(
                "maintained without verified proof ({}): {}",
                sanitize_inline(why),
                sanitize_inline(argument)
            ),
            ReplyResult::Withdrawn { argument } => {
                format!("withdrawn: {}", sanitize_inline(argument))
            }
            ReplyResult::NoReply { why } => format!("none ({})", sanitize_inline(why)),
        };
        lines.push(format!("- **Author's reply**: {line}"));
    }
    if let Some(key) = &record.question_key {
        lines.push(format!("- **Human card**: `{key}`"));
    }
    lines.push(BLOCK_END.to_string());
    lines.join("\n")
}

/// A new sheet for an audited finding.
pub fn render_td_sheet(record: &CrossFindingRecord) -> String {
    let id = record.td_id.clone().unwrap_or_default();
    let title = sanitize_inline(&record.title);
    format!(
        "---\nid: {id}\ndescription: {desc}\ncategory: {category}\n---\n# {id}: {title}\n\n\
         - **ID**: {id}\n\
         - **Area**: `{file}`\n\
         - **Severity**: {severity}\n\
         - **Category**: {category}\n\
         - **Status**: Verified in source\n\
         - **Problem**: {title}\n\
         - **Impact**: {impact}\n\
         - **Where**: `{file}:{line}` {src}\n\
         - **Suggested fix**: {fix}\n\n{block}\n",
        desc = title.replace(':', " -"),
        category = sanitize_inline(&record.category),
        file = code_path(&record.citation.file),
        severity = record.severity,
        impact = or_dash(&record.impact),
        line = record.citation.line,
        src = src(&record.citation),
        fix = or_dash(&record.remediation),
        block = provenance_block(record),
    )
}

fn or_dash(text: &str) -> String {
    if text.trim().is_empty() {
        "—".into()
    } else {
        sanitize_inline(text)
    }
}

/// An existing sheet keeps its body; only the Kronn block is replaced or appended.
pub fn merge_provenance(existing: &str, record: &CrossFindingRecord) -> String {
    let block = provenance_block(record);
    if let (Some(start), Some(end)) = (existing.find(BLOCK_START), existing.find(BLOCK_END)) {
        if start < end {
            let mut out = existing[..start].to_string();
            out.push_str(&block);
            out.push_str(&existing[end + BLOCK_END.len()..]);
            return out;
        }
    }
    let mut out = existing.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(&block);
    out.push('\n');
    out
}

/// What the step's index file shows.
pub struct IndexInput<'a> {
    pub heading: &'a str,
    pub agents: &'a [AgentType],
    pub records: &'a [CrossFindingRecord],
    pub dropped: &'a [DroppedFinding],
    pub checklists: &'a [(AgentType, Vec<ChecklistLine>)],
    pub failed_agents: &'a [(AgentType, String)],
}

pub fn render_index(input: &IndexInput<'_>) -> String {
    let agents: Vec<String> = input.agents.iter().map(agent_label).collect();
    let mut out = format!(
        "# {}\n\n> Cross-agent audit (experimental) — agents: {}. Kronn checked every citation, \
         then had each finding challenged by another agent. The number of agents that found a \
         finding is information only: it never decided a verdict. Search first: open a finding's \
         TD sheet before acting on it.\n\n| ID | Problem | Area | Severity |\n|----|---------|------|----------|\n",
        input.heading,
        agents.join(", ")
    );
    let audited: Vec<&CrossFindingRecord> = input
        .records
        .iter()
        .filter(|r| r.outcome.in_audit())
        .collect();
    for r in &audited {
        let mark = match r.outcome {
            Outcome::Contested => " (contested)",
            Outcome::Disputed => " (disputed — human decision pending)",
            _ => "",
        };
        out.push_str(&format!(
            "| {} | {}{mark} | `{}` | {} |\n",
            r.td_id.clone().unwrap_or_default(),
            sanitize_inline(&r.title),
            code_path(&r.citation.file),
            r.severity
        ));
    }
    if audited.is_empty() {
        out.push_str("| — | No finding survived the cross-check | — | — |\n");
    }

    out.push_str("\n## Cross-check verdicts\n\n| ID | Verdict | Found by | Detections | Checked by |\n|----|---------|----------|------------|------------|\n");
    for r in input.records {
        out.push_str(&format!(
            "| {} | {} | {} | {}/{} | {} |\n",
            r.td_id.clone().unwrap_or_else(|| r.group_id.clone()),
            r.outcome.label(),
            found_by_line(r),
            r.detection_count,
            r.agents_total,
            r.challenger.clone().unwrap_or_else(|| "—".into())
        ));
    }

    out.push_str("\n## Dismissed with verified proof (logged)\n\n");
    let dismissed: Vec<&CrossFindingRecord> = input
        .records
        .iter()
        .filter(|r| r.outcome == Outcome::Dismissed)
        .collect();
    if dismissed.is_empty() {
        out.push_str("None.\n");
    }
    for r in dismissed {
        let refutation = match &r.challenge {
            Some(ChallengeResult::RefutedProven { argument, proof }) => {
                format!("{} {}", proof_line(proof), sanitize_inline(argument))
            }
            _ => String::new(),
        };
        let reply = match &r.reply {
            Some(ReplyResult::Withdrawn { .. }) => "withdrawn by the author",
            Some(ReplyResult::MaintainedUnproven { .. }) => "maintained without verified proof",
            _ => "no reply",
        };
        out.push_str(&format!(
            "- **{}** {} — found by {}; refuted by {}: {refutation}. Reply: {reply}.\n",
            sanitize_inline(&r.title),
            src(&r.citation),
            found_by_line(r),
            r.challenger.clone().unwrap_or_default()
        ));
    }

    out.push_str("\n## Dropped by the citation check\n\n");
    if input.dropped.is_empty() {
        out.push_str("None.\n");
    }
    for d in input.dropped {
        out.push_str(&format!(
            "- **{}** by {} — {} (no model was asked about it)\n",
            sanitize_inline(&d.finding.title),
            agent_label(&d.author),
            sanitize_inline(&d.reason).replace('`', "'")
        ));
    }

    let unchecked: Vec<&CrossFindingRecord> = input
        .records
        .iter()
        .filter(|r| r.outcome == Outcome::NotCrossChecked)
        .collect();
    if !unchecked.is_empty() {
        out.push_str(&format!(
            "\n## Not cross-checked\n\nBeyond the cap of {MAX_CROSS_CHECKED} cross-checked findings per step; they stay in the audit unchallenged.\n\n"
        ));
        for r in unchecked {
            out.push_str(&format!(
                "- {} — {}\n",
                r.td_id.clone().unwrap_or_default(),
                sanitize_inline(&r.title)
            ));
        }
    }

    if !input.failed_agents.is_empty() {
        out.push_str("\n## Agents without a usable detection\n\n");
        for (agent, why) in input.failed_agents {
            out.push_str(&format!(
                "- {}: {}\n",
                agent_label(agent),
                sanitize_inline(why).replace('`', "'")
            ));
        }
    }

    out.push_str("\n## Checklist (per agent)\n");
    for (agent, lines) in input.checklists {
        out.push_str(&format!("\n### {}\n\n", agent_label(agent)));
        if lines.is_empty() {
            out.push_str("No checklist returned.\n");
        }
        for line in lines.iter().take(80) {
            out.push_str(&format!(
                "- {} — {}\n",
                sanitize_inline(&line.item),
                or_dash(&line.status)
            ));
        }
    }
    out
}

// ── Prompts ─────────────────────────────────────────────────────────────────

/// Appended to a finding step's prompt in detection mode. Overrides the
/// step's output section: no file is written, the findings come back as text.
pub fn detection_appendix(agent: &AgentType) -> String {
    format!(
        "\n\n## CROSS-AGENT AUDIT — DETECTION MODE (overrides the OUTPUT FORMAT above)\n\
         You are agent `{agent}` in an independent detection pass. Other agents run the same step \
         separately; you never see their work and they never see yours.\n\
         - Do NOT create, edit or delete ANY file. Kronn writes the step's documents itself.\n\
         - Run the checklist exactly as instructed, then answer with ONE fenced block, last in \
         your answer:\n\
         ```{FINDINGS_FENCE}\n\
         {{\"findings\":[{{\"title\":\"one sentence\",\"category\":\"security\",\"severity\":\"Critical|High|Medium|Low\",\
         \"file\":\"path/relative/to/repo\",\"line\":12,\"line_end\":14,\"quote\":\"exact text found at those lines\",\
         \"impact\":\"what an attacker gains\",\"remediation\":\"one sentence\",\"td_id\":\"TD-... when an existing sheet is this finding\"}}],\
         \"checklist\":[{{\"item\":\"checklist item\",\"status\":\"verified present|verified absent|Not evaluated safely ...\"}}]}}\n\
         ```\n\
         - Every finding cites ONE place: `file`, `line` (and optional `line_end`) and a `quote` copied \
         verbatim from those lines. Kronn checks the file, the lines and the quote mechanically; a \
         finding whose citation does not check is dropped without discussion.\n\
         - The quote must never contain a secret value: quote a non-secret fragment of the line \
         (the key name, the call), never the value.\n\
         - At most {MAX_FINDINGS_PER_AGENT} findings.",
        agent = agent_label(agent)
    )
}

#[derive(Serialize)]
struct PromptFinding<'a> {
    id: &'a str,
    title: &'a str,
    category: &'a str,
    severity: &'a str,
    file: &'a str,
    line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_end: Option<u32>,
    impact: &'a str,
}

fn prompt_finding<'a>(id: &'a str, finding: &'a RawFinding) -> PromptFinding<'a> {
    PromptFinding {
        id,
        title: &finding.title,
        category: &finding.category,
        severity: &finding.severity,
        file: &finding.citation.file,
        line: finding.citation.line,
        line_end: finding.citation.line_end,
        impact: &finding.impact,
    }
}

const PROOF_RULE: &str = "Each proof item is {\"file\":\"path\",\"line\":N,\"line_end\":M (optional),\"quote\":\"exact text at those lines\"}. \
Kronn verifies every proof citation mechanically (file, lines, quote). A refutation whose proof does not verify cannot \
dismiss anything. Never quote a secret value.";

/// One challenger, all the findings assigned to it. It is not told who found
/// a finding nor how many agents did.
pub fn challenge_prompt(agent: &AgentType, findings: &[(&str, &RawFinding)]) -> String {
    let list: Vec<PromptFinding<'_>> = findings
        .iter()
        .map(|(id, f)| prompt_finding(id, f))
        .collect();
    format!(
        "CROSS-AGENT AUDIT — CROSS-CHECK. You are agent `{agent}`. Another agent reported the findings \
         below about this repository. Your job: try to REFUTE each one with proof from the code, or \
         confirm it. Read the code; do NOT create, edit or delete any file.\n\n\
         A refutation needs proof: the code that shows the finding is wrong (a sanitizer, a guard, a \
         config that already fixes it, a dead path). {PROOF_RULE}\n\n\
         Findings:\n```json\n{findings}\n```\n\n\
         Answer with ONE fenced block, last in your answer:\n\
         ```{VERDICTS_FENCE}\n\
         {{\"verdicts\":[{{\"id\":\"F1\",\"verdict\":\"confirm|refute\",\"argument\":\"why\",\"proof\":[]}}]}}\n\
         ```",
        agent = agent_label(agent),
        findings = serde_json::to_string_pretty(&list).unwrap_or_default(),
    )
}

#[derive(Serialize)]
struct PromptRefutation<'a> {
    #[serde(flatten)]
    finding: PromptFinding<'a>,
    refutation: &'a str,
    refutation_proof: &'a [Citation],
}

/// The author's single right of reply, before arbitration.
pub fn reply_prompt(agent: &AgentType, items: &[(&str, &RawFinding, &str, &[Citation])]) -> String {
    let list: Vec<PromptRefutation<'_>> = items
        .iter()
        .map(|(id, f, argument, proof)| PromptRefutation {
            finding: prompt_finding(id, f),
            refutation: argument,
            refutation_proof: proof,
        })
        .collect();
    format!(
        "CROSS-AGENT AUDIT — RIGHT OF REPLY. You are agent `{agent}`. You reported the findings below; \
         another agent refuted each one with verified proof. You may answer ONCE: maintain the finding \
         with your own proof, or withdraw it. There is no further round. Read the code; do NOT create, \
         edit or delete any file. {PROOF_RULE}\n\n\
         Refuted findings:\n```json\n{items}\n```\n\n\
         Answer with ONE fenced block, last in your answer:\n\
         ```{REPLIES_FENCE}\n\
         {{\"replies\":[{{\"id\":\"F1\",\"stance\":\"maintain|withdraw\",\"argument\":\"why\",\"proof\":[]}}]}}\n\
         ```",
        agent = agent_label(agent),
        items = serde_json::to_string_pretty(&list).unwrap_or_default(),
    )
}

// ── Human card ──────────────────────────────────────────────────────────────

pub fn question_key(run_id: &str, record: &CrossFindingRecord) -> String {
    let run: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let id = record
        .td_id
        .clone()
        .unwrap_or_else(|| record.group_id.clone());
    let mut key = format!("cross-audit.{run}.{id}");
    key.truncate(100);
    key
}

/// A `kronn-question` card with both arguments side by side. No recommended
/// option: the human judges on the evidence.
pub fn dispute_card(record: &CrossFindingRecord) -> Option<String> {
    let key = record.question_key.clone()?;
    let (refutation, refutation_proof) = match &record.challenge {
        Some(ChallengeResult::RefutedProven { argument, proof }) => {
            (argument.as_str(), proof.as_slice())
        }
        _ => return None,
    };
    let (reply, reply_proof) = match &record.reply {
        Some(ReplyResult::MaintainedProven { argument, proof }) => {
            (argument.as_str(), proof.as_slice())
        }
        _ => return None,
    };
    let cite = |proof: &[Citation]| -> String {
        proof
            .iter()
            .map(|c| format!("{}:{}", c.file, c.line))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let clip = |text: &str| -> String { text.chars().take(900).collect() };
    let context = format!(
        "Finding ({}, {}): {} — {}:{}\n\nFor the finding — {} (author): {}\nProof: {}\n\nAgainst — {} (challenger): {}\nProof: {}\n\nBoth proofs passed Kronn's citation check. Detection count: {} of {} (information only).",
        record.severity,
        record.category,
        clip(&record.title),
        record.citation.file,
        record.citation.line,
        record.author,
        clip(reply),
        cite(reply_proof),
        record.challenger.clone().unwrap_or_default(),
        clip(refutation),
        cite(refutation_proof),
        record.detection_count,
        record.agents_total,
    );
    let context: String = context.chars().take(3900).collect();
    let question: String = format!("Cross-agent dispute: keep this finding? {}", record.title)
        .chars()
        .take(900)
        .collect();
    let spec = serde_json::json!({
        "version": 1,
        "key": key,
        "question": question,
        "context": context,
        "options": [
            {"id": "keep", "label": "Keep the finding", "description": "The author's proof holds; the finding stays in the audit."},
            {"id": "dismiss", "label": "Dismiss the finding", "description": "The challenger's proof holds; the finding leaves the audit."}
        ],
        "multiple": false,
        "recommended_option_ids": []
    });
    Some(format!(
        "```kronn-question\n{}\n```",
        serde_json::to_string_pretty(&spec).ok()?
    ))
}

#[cfg(test)]
#[path = "cross_audit_test.rs"]
mod tests;
