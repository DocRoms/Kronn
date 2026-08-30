//! Deterministic post-audit documentation budgets and routing diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const CONFIG_FILE: &str = "docs/.kronn-document-budgets.json";
pub const REPORT_FILE: &str = "docs/.kronn-document-optimization.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentBudgets {
    pub adapter_max_words: usize,
    pub agents_md_max_words: usize,
    pub mandatory_path_max_words: usize,
    pub initially_routed_max_documents: usize,
    pub large_inventory_min_words: usize,
    pub exception: Option<BudgetException>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BudgetException {
    pub justification: String,
    pub provenance: String,
    pub reviewed_by: String,
    pub reviewed_at: String,
}

impl Default for DocumentBudgets {
    fn default() -> Self {
        Self {
            adapter_max_words: 220,
            agents_md_max_words: 800,
            mandatory_path_max_words: 1_200,
            initially_routed_max_documents: 2,
            large_inventory_min_words: 1_200,
            exception: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentMeasure {
    pub path: String,
    pub bytes: usize,
    pub words: usize,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentLoadPath {
    pub adapter: String,
    pub documents: Vec<DocumentMeasure>,
    pub bytes: usize,
    pub words: usize,
    pub estimated_tokens: usize,
    pub initially_routed_documents: usize,
    pub top_contributors: Vec<DocumentMeasure>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentaryOptimizationReport {
    pub phase: String,
    pub budgets: DocumentBudgets,
    pub agents: Vec<AgentLoadPath>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DocumentaryOptimizationReport {
    pub fn blocking_diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.blocking)
    }
}

pub fn analyze(project: &Path) -> Result<DocumentaryOptimizationReport, String> {
    let budgets = load_budgets(project)?;
    let docs = markdown_files(&project.join("docs"))?;
    let adapters = root_adapters(project)?;
    let mut diagnostics = Vec::new();
    let mut agents = Vec::new();

    for adapter in &adapters {
        let adapter_rel = relative(project, adapter);
        let content = read(adapter)?;
        let adapter_measure = measure(&adapter_rel, &content);
        if contains_placeholder(&content) {
            diagnostics.push(diag(
                "placeholder",
                &adapter_rel,
                "unresolved template or TODO placeholder".into(),
            ));
        }
        inspect_citations(project, &adapter_rel, &content, &mut diagnostics);
        let visible = visible_markdown(&content);
        for line in visible.lines().filter(|line| mutable_line_reference(line)) {
            diagnostics.push(diag(
                "mutable_line_reference",
                &adapter_rel,
                line.trim().to_string(),
            ));
        }
        if adapter_measure.words > budgets.adapter_max_words {
            diagnostics.push(diag(
                "adapter_budget",
                &adapter_rel,
                format!(
                    "{} words exceeds adapter budget {}",
                    adapter_measure.words, budgets.adapter_max_words
                ),
            ));
        }
        let mut routed = routed_markdown(project, adapter, &content, &mut diagnostics);
        let entry = project.join("docs/AGENTS.md");
        if entry.is_file() && adapter != &entry && !routed.contains(&entry) {
            routed.insert(0, entry);
        }
        routed.dedup();
        let mut measures = vec![adapter_measure];
        measures.extend(
            routed
                .iter()
                .filter_map(|p| read(p).ok().map(|s| measure(&relative(project, p), &s))),
        );
        let words = measures.iter().map(|m| m.words).sum();
        if words > budgets.mandatory_path_max_words {
            diagnostics.push(diag(
                "mandatory_path_budget",
                &adapter_rel,
                format!(
                    "mandatory path is {words} words; budget is {}",
                    budgets.mandatory_path_max_words
                ),
            ));
        }
        if routed.len() > budgets.initially_routed_max_documents {
            diagnostics.push(diag(
                "initial_routing_budget",
                &adapter_rel,
                format!(
                    "{} documents routed initially; budget is {}",
                    routed.len(),
                    budgets.initially_routed_max_documents
                ),
            ));
        }
        let mut top = measures.clone();
        top.sort_by_key(|m| std::cmp::Reverse(m.words));
        top.truncate(5);
        agents.push(AgentLoadPath {
            adapter: adapter_rel,
            documents: measures.clone(),
            bytes: measures.iter().map(|m| m.bytes).sum(),
            words,
            estimated_tokens: measures.iter().map(|m| m.estimated_tokens).sum(),
            initially_routed_documents: routed.len(),
            top_contributors: top,
        });
    }

    inspect_docs(project, &docs, &adapters, &budgets, &mut diagnostics);
    diagnostics.sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
    Ok(DocumentaryOptimizationReport {
        phase: "documentary_optimization".into(),
        budgets,
        agents,
        diagnostics,
    })
}

pub fn analyze_and_write(project: &Path) -> Result<DocumentaryOptimizationReport, String> {
    let report = analyze(project)?;
    let body = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let project_real = project
        .canonicalize()
        .map_err(|e| format!("resolve project root: {e}"))?;
    let docs = project.join("docs");
    let docs_real = docs
        .canonicalize()
        .map_err(|e| format!("resolve docs directory: {e}"))?;
    if !docs_real.starts_with(&project_real) {
        return Err("docs directory resolves outside the project".into());
    }
    let target = project.join(REPORT_FILE);
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{REPORT_FILE} is a symlink; refusing report write"));
    }
    crate::core::mcp_scanner::atomic_write(&target, &format!("{body}\n"))
        .map_err(|e| format!("write {REPORT_FILE}: {e}"))?;
    Ok(report)
}

fn load_budgets(project: &Path) -> Result<DocumentBudgets, String> {
    let path = project.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(DocumentBudgets::default());
    }
    let budgets: DocumentBudgets =
        serde_json::from_str(&read(&path)?).map_err(|e| format!("invalid {CONFIG_FILE}: {e}"))?;
    validate_budgets(&budgets)?;
    Ok(budgets)
}

fn validate_budgets(budgets: &DocumentBudgets) -> Result<(), String> {
    let defaults = DocumentBudgets::default();
    let relaxed = budgets.adapter_max_words > defaults.adapter_max_words
        || budgets.agents_md_max_words > defaults.agents_md_max_words
        || budgets.mandatory_path_max_words > defaults.mandatory_path_max_words
        || budgets.initially_routed_max_documents > defaults.initially_routed_max_documents
        || budgets.large_inventory_min_words > defaults.large_inventory_min_words;
    if budgets.adapter_max_words > 800
        || budgets.agents_md_max_words > 1_200
        || budgets.mandatory_path_max_words > 2_000
        || budgets.initially_routed_max_documents > 4
        || budgets.large_inventory_min_words > 2_000
    {
        return Err(format!(
            "invalid {CONFIG_FILE}: budget exception exceeds hard safety limits"
        ));
    }
    if relaxed {
        let Some(exception) = &budgets.exception else {
            return Err(format!(
                "invalid {CONFIG_FILE}: relaxed budgets require an explicitly reviewed exception"
            ));
        };
        if [
            &exception.justification,
            &exception.provenance,
            &exception.reviewed_by,
            &exception.reviewed_at,
        ]
        .iter()
        .any(|value| value.trim().is_empty())
        {
            return Err(format!(
                "invalid {CONFIG_FILE}: exception justification, provenance, reviewer and review date are required"
            ));
        }
    }
    Ok(())
}

fn inspect_docs(
    project: &Path,
    docs: &[PathBuf],
    adapters: &[PathBuf],
    budgets: &DocumentBudgets,
    out: &mut Vec<Diagnostic>,
) {
    let mut titles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut linked = BTreeSet::new();
    for adapter in adapters {
        if let Ok(content) = read(adapter) {
            collect_valid_links(project, adapter, &content, &mut linked, Some(out));
        }
    }
    for path in docs {
        let Ok(content) = read(path) else { continue };
        let rel = relative(project, path);
        if rel == "docs/AGENTS.md" && word_count(&content) > budgets.agents_md_max_words {
            out.push(diag(
                "agents_md_budget",
                &rel,
                format!(
                    "{} words exceeds budget {}",
                    word_count(&content),
                    budgets.agents_md_max_words
                ),
            ));
        }
        if contains_placeholder(&content) {
            out.push(diag(
                "placeholder",
                &rel,
                "unresolved template or TODO placeholder".into(),
            ));
        }
        let visible = visible_markdown(&content);
        for line in visible.lines() {
            if let Some(title) = line.strip_prefix("# ") {
                titles
                    .entry(title.trim().to_lowercase())
                    .or_default()
                    .push(rel.clone());
            }
            if mutable_line_reference(line) {
                out.push(diag(
                    "mutable_line_reference",
                    &rel,
                    line.trim().to_string(),
                ));
            }
        }
        collect_valid_links(project, path, &content, &mut linked, Some(out));
        inspect_citations(project, &rel, &content, out);
        if word_count(&content) >= budgets.large_inventory_min_words && !search_first(&content) {
            out.push(diag(
                "large_inventory_not_search_first",
                &rel,
                "large document is not marked search-first".into(),
            ));
        }
        if content.to_lowercase().contains("obsolete")
            || content.to_lowercase().contains("deprecated guide")
        {
            out.push(diag(
                "obsolete_guide",
                &rel,
                "document declares obsolete/deprecated guidance".into(),
            ));
        }
    }
    out.sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
    out.dedup_by(|a, b| a.path == b.path && a.code == b.code && a.message == b.message);
    for paths in titles.values().filter(|p| p.len() > 1) {
        for path in paths {
            out.push(diag(
                "duplicate_guide",
                path,
                format!(
                    "duplicate top-level title shared by {} documents",
                    paths.len()
                ),
            ));
        }
    }
    for path in docs {
        let rel = relative(project, path);
        if rel != "docs/AGENTS.md"
            && !rel.starts_with("docs/tech-debt/")
            && !linked.contains(&normalize(path))
        {
            out.push(diag(
                "orphan_document",
                &rel,
                "document is not linked from another documentation file".into(),
            ));
        }
    }
}

fn inspect_citations(project: &Path, rel: &str, content: &str, out: &mut Vec<Diagnostic>) {
    let visible = visible_markdown(content);
    for tail in visible.split("[src: file:").skip(1) {
        let Some(raw) = tail.split(']').next() else {
            continue;
        };
        let raw = raw.trim();
        if raw.contains('<') || raw.contains('>') {
            continue;
        }
        let Some((path_part, line_part)) = raw.rsplit_once(':') else {
            out.push(diag(
                "broken_citation",
                rel,
                format!("malformed file citation `{raw}`"),
            ));
            continue;
        };
        let path = project.join(path_part.trim());
        if outside_project(project, &path) || !path.is_file() {
            out.push(diag(
                "broken_citation",
                rel,
                format!("citation path does not exist `{}`", path_part.trim()),
            ));
            continue;
        }
        let max_line = read(&path).map(|s| s.lines().count()).unwrap_or(0);
        let parsed = line_part
            .trim()
            .split('-')
            .filter_map(|n| n.parse::<usize>().ok())
            .max();
        if parsed.is_none() || parsed.is_some_and(|n| n == 0 || n > max_line) {
            out.push(diag(
                "broken_citation",
                rel,
                format!(
                    "citation line `{line_part}` is outside `{}`",
                    path_part.trim()
                ),
            ));
        }
    }
}

fn root_adapters(project: &Path) -> Result<Vec<PathBuf>, String> {
    let names = [
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".github/copilot-instructions.md",
        ".cursor/rules/repo-instructions.mdc",
        ".kiro/steering/instructions.md",
        ".vibe/instructions.md",
    ];
    let mut found: Vec<_> = names
        .iter()
        .map(|n| project.join(n))
        .filter(|p| {
            fs::symlink_metadata(p)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        })
        .collect();
    if found.is_empty() && project.join("docs/AGENTS.md").is_file() {
        found.push(project.join("docs/AGENTS.md"));
    }
    Ok(found)
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let project_real = root
        .parent()
        .ok_or_else(|| format!("{} has no project parent", root.display()))?
        .canonicalize()
        .map_err(|e| format!("resolve project root: {e}"))?;
    let root_real = root
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", root.display()))?;
    if !root_real.starts_with(&project_real) {
        return Err("docs directory resolves outside the project".into());
    }
    fn walk(
        dir: &Path,
        root_real: &Path,
        visited: &mut BTreeSet<PathBuf>,
        out: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let real = dir
            .canonicalize()
            .map_err(|e| format!("resolve {}: {e}", dir.display()))?;
        if !real.starts_with(root_real) || !visited.insert(real) {
            return Ok(());
        }
        for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                walk(&path, root_real, visited, out)?;
            } else if metadata.is_file()
                && path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &root_real, &mut BTreeSet::new(), &mut out)?;
    out.sort();
    Ok(out)
}

fn routed_markdown(
    project: &Path,
    adapter: &Path,
    content: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PathBuf> {
    markdown_targets(content)
        .into_iter()
        .filter_map(|target| {
            if target.starts_with("http") || target.starts_with('#') {
                return None;
            }
            let clean = target.split('#').next().unwrap_or("");
            let path = adapter.parent().unwrap_or(project).join(clean);
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                return None;
            }
            if !contained_file(project, &path) {
                diagnostics.push(diag(
                    "broken_link",
                    &relative(project, adapter),
                    format!("broken routed document `{target}`"),
                ));
                None
            } else {
                Some(normalize(&path))
            }
        })
        .collect()
}

fn markdown_targets(content: &str) -> Vec<String> {
    let visible = visible_markdown(content);
    let mut definitions = BTreeMap::new();
    for line in visible.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(split) = rest.find("]:") {
                let id = rest[..split].trim().to_ascii_lowercase();
                let target = markdown_destination(rest[split + 2..].trim());
                if !id.is_empty() && !target.is_empty() {
                    definitions.insert(id, target);
                }
            }
        }
    }
    let bytes = visible.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            let mut depth = 1usize;
            let mut escaped = false;
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'(' {
                    depth += 1;
                } else if b == b')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }
            if depth == 0 {
                let target = markdown_destination(&visible[start..end]);
                if !target.is_empty() {
                    out.push(target);
                }
                i = end + 1;
                continue;
            }
        } else if bytes[i] == b']' && bytes[i + 1] == b'[' {
            if let Some(close) = visible[i + 2..].find(']') {
                let explicit_id = visible[i + 2..i + 2 + close].trim();
                let id = if explicit_id.is_empty() {
                    reference_label_before(&visible, i).unwrap_or_default()
                } else {
                    explicit_id.to_ascii_lowercase()
                };
                if let Some(target) = definitions.get(&id) {
                    out.push(target.clone());
                }
                i += close + 3;
                continue;
            }
        } else if bytes[i] == b']' && bytes[i + 1] != b':' {
            if let Some(id) = reference_label_before(&visible, i) {
                if let Some(target) = definitions.get(&id) {
                    out.push(target.clone());
                }
            }
        }
        i += 1;
    }
    out
}

fn reference_label_before(content: &str, close: usize) -> Option<String> {
    let open = content[..close].rfind('[')?;
    let label = content[open + 1..close].trim().to_ascii_lowercase();
    (!label.is_empty()).then_some(label)
}

fn markdown_destination(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('<') {
        return rest.split('>').next().unwrap_or("").to_string();
    }
    raw.split_whitespace().next().unwrap_or("").to_string()
}

fn collect_valid_links(
    project: &Path,
    source: &Path,
    content: &str,
    linked: &mut BTreeSet<PathBuf>,
    mut diagnostics: Option<&mut Vec<Diagnostic>>,
) {
    for target in markdown_targets(content) {
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with('#')
        {
            continue;
        }
        let clean = target.split('#').next().unwrap_or("");
        let resolved = source.parent().unwrap_or(project).join(clean);
        if outside_project(project, &resolved) || !contained_path(project, &resolved) {
            if let Some(out) = diagnostics.as_deref_mut() {
                out.push(diag(
                    "broken_link",
                    &relative(project, source),
                    format!("broken link target `{target}`"),
                ));
            }
        } else {
            linked.insert(normalize(&resolved));
        }
    }
}

fn measure(path: &str, content: &str) -> DocumentMeasure {
    let words = word_count(content);
    DocumentMeasure {
        path: path.into(),
        bytes: content.len(),
        words,
        estimated_tokens: content.chars().count().div_ceil(4),
    }
}
fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}
fn contains_placeholder(s: &str) -> bool {
    crate::api::audit::validation::count_raw_placeholders(s) > 0
        || visible_markdown(s).lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("<!-- TODO:") || trimmed == "PLACEHOLDER"
        })
}
fn visible_markdown(s: &str) -> String {
    crate::core::anti_halluc::strip_inline_code(&crate::core::anti_halluc::strip_fenced_code(s))
}
fn mutable_line_reference(s: &str) -> bool {
    s.contains("[src: file:")
        && s.contains('-')
        && s.split("[src: file:").skip(1).any(|p| {
            p.split(']')
                .next()
                .is_some_and(|c| c.rsplit(':').next().is_some_and(|n| n.contains('-')))
        })
}
fn search_first(s: &str) -> bool {
    let l = s.to_lowercase();
    l.contains("search-first") || l.contains("search first") || l.contains("use `rg`")
}
fn read(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))
}
fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(c.as_os_str()),
        }
    }
    out
}
fn outside_project(root: &Path, p: &Path) -> bool {
    !normalize(p).starts_with(normalize(root).as_path())
}
fn contained_path(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    path.canonicalize().is_ok_and(|path| path.starts_with(root))
}
fn contained_file(root: &Path, path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && contained_path(root, path)
    })
}
fn diag(code: &str, path: &str, message: String) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        path: path.into(),
        message,
        blocking: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn small_context_onboarding_stays_within_default_budgets() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "Read [docs/AGENTS.md](docs/AGENTS.md).\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\n\nSearch-first.\n",
        );
        let report = analyze(tmp.path()).unwrap();
        assert_eq!(report.agents[0].initially_routed_documents, 1);
        assert!(
            report.blocking_diagnostics().next().is_none(),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn africanews_baseline_accepts_613_word_router_and_755_word_path() {
        let tmp = tempfile::tempdir().unwrap();
        let router = std::iter::repeat_n("router", 612)
            .collect::<Vec<_>>()
            .join(" ")
            + " [entry](docs/AGENTS.md)";
        let entry = std::iter::repeat_n("entry", 139)
            .collect::<Vec<_>>()
            .join(" ");
        write(&tmp.path().join("AGENTS.md"), &router);
        write(
            &tmp.path().join("docs/AGENTS.md"),
            &format!("# Entry\nSearch-first {entry}"),
        );
        write(
            &tmp.path().join("docs/.kronn-document-budgets.json"),
            r#"{
                "adapter_max_words": 620,
                "exception": {
                    "justification": "Reviewed Africanews baseline",
                    "provenance": "KT-524 fixture",
                    "reviewed_by": "test reviewer",
                    "reviewed_at": "2026-08-30"
                }
            }"#,
        );
        let report = analyze(tmp.path()).unwrap();
        assert_eq!(report.agents[0].documents[0].words, 613);
        assert_eq!(report.agents[0].words, 755);
        assert!(
            report.blocking_diagnostics().next().is_none(),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn blocks_budgets_placeholders_broken_links_orphans_and_mutable_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md) [missing](docs/no.md)",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\n{{TODO}} [bad](missing.md) [src: file: src/a.rs:1-2]",
        );
        write(
            &tmp.path().join("docs/orphan.md"),
            "# Orphan\nobsolete guide",
        );
        write(
            &tmp.path().join("docs/duplicate.md"),
            "# Orphan\nNot search-first. ",
        );
        let inventory = std::iter::repeat_n("inventory", 1_201)
            .collect::<Vec<_>>()
            .join(" ");
        write(
            &tmp.path().join("docs/inventory.md"),
            &format!("# Inventory\n{inventory}"),
        );
        write(
            &tmp.path().join("docs/.kronn-document-budgets.json"),
            r#"{"adapter_max_words":1,"agents_md_max_words":2,"mandatory_path_max_words":3,"initially_routed_max_documents":0}"#,
        );
        let codes: BTreeSet<_> = analyze(tmp.path())
            .unwrap()
            .diagnostics
            .into_iter()
            .map(|d| d.code)
            .collect();
        for code in [
            "adapter_budget",
            "agents_md_budget",
            "mandatory_path_budget",
            "initial_routing_budget",
            "placeholder",
            "broken_link",
            "orphan_document",
            "obsolete_guide",
            "mutable_line_reference",
            "duplicate_guide",
            "large_inventory_not_search_first",
        ] {
            assert!(codes.contains(code), "missing {code}: {codes:?}");
        }
    }

    #[test]
    fn grammar_examples_and_kronn_docs_do_not_create_false_diagnostics() {
        let template = include_str!("../../../templates/docs/AGENTS.md");
        assert!(contains_placeholder(template));
        assert!(!contains_placeholder(
            "Twig {{ asset('app.css') }} and prose `{{EXAMPLE_SLOT}}`"
        ));
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut template_citations = Vec::new();
        inspect_citations(
            project,
            "templates/docs/AGENTS.md",
            template,
            &mut template_citations,
        );
        assert!(template_citations.is_empty(), "{template_citations:?}");
        assert!(!visible_markdown(template)
            .lines()
            .any(mutable_line_reference));

        let actual = fs::read_to_string(project.join("docs/AGENTS.md")).unwrap();
        assert!(!contains_placeholder(&actual));
        let mut actual_citations = Vec::new();
        inspect_citations(project, "docs/AGENTS.md", &actual, &mut actual_citations);
        assert!(actual_citations.is_empty(), "{actual_citations:?}");
    }

    #[test]
    fn relaxed_budgets_require_review_metadata_and_have_hard_caps() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        write(
            &tmp.path().join(CONFIG_FILE),
            r#"{"adapter_max_words": 620}"#,
        );
        assert!(analyze(tmp.path())
            .unwrap_err()
            .contains("explicitly reviewed"));
        write(
            &tmp.path().join(CONFIG_FILE),
            r#"{"adapter_max_words":18446744073709551615,"exception":{"justification":"x","provenance":"KT-524","reviewed_by":"reviewer","reviewed_at":"2026-08-30"}}"#,
        );
        assert!(analyze(tmp.path())
            .unwrap_err()
            .contains("hard safety limits"));
    }

    #[test]
    fn root_reference_links_with_titles_and_parentheses_prevent_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md \"entry title\") [guide][guide-ref]\n[guide-ref]: <docs/guide_(v1).md> \"Guide title\"\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first. [nested](guide_(v1).md#usage \"Nested title\")\n",
        );
        write(&tmp.path().join("docs/guide_(v1).md"), "# Guide\nSmall.\n");
        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "broken_link" || d.code == "orphan_document"),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn collapsed_and_shortcut_reference_links_prevent_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md) [collapsed][] [shortcut]\n[collapsed]: docs/collapsed.md\n[shortcut]: docs/shortcut.md\n[ordinary text]\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first.\n",
        );
        write(
            &tmp.path().join("docs/collapsed.md"),
            "# Collapsed\nSmall.\n",
        );
        write(&tmp.path().join("docs/shortcut.md"), "# Shortcut\nSmall.\n");

        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "broken_link" || d.code == "orphan_document"),
            "{:#?}",
            report.diagnostics
        );
        assert_eq!(
            markdown_targets("[ordinary text]\n[guide]: docs/guide.md\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn root_adapter_reference_links_report_broken_targets() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md) [missing][missing-ref]\n[missing-ref]: docs/missing_(v1).md \"Missing title\"\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first.\n```text\n[src: file: src/example.rs:10-20]\n```\n",
        );
        let report = analyze(tmp.path()).unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "broken_link" && diagnostic.path == "AGENTS.md"
        }));
        assert!(!report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "mutable_line_reference"));
    }

    #[cfg(unix)]
    #[test]
    fn report_write_refuses_an_external_symlink_target() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let external = tempfile::NamedTempFile::new().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        symlink(external.path(), tmp.path().join(REPORT_FILE)).unwrap();
        let before = fs::read_to_string(external.path()).unwrap();
        let error = analyze_and_write(tmp.path()).unwrap_err();
        assert!(error.contains("is a symlink"), "{error}");
        assert_eq!(fs::read_to_string(external.path()).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn docs_traversal_does_not_follow_external_links_or_loops() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        write(
            &external.path().join("outside.md"),
            "# Outside\nPLACEHOLDER\n",
        );
        symlink(external.path(), tmp.path().join("docs/external")).unwrap();
        symlink(tmp.path().join("docs"), tmp.path().join("docs/loop")).unwrap();
        let files = markdown_files(&tmp.path().join("docs")).unwrap();
        assert_eq!(files, vec![tmp.path().join("docs/AGENTS.md")]);
        let report = analyze(tmp.path()).unwrap();
        assert!(!report
            .diagnostics
            .iter()
            .any(|d| d.path.contains("outside")));
    }
}
