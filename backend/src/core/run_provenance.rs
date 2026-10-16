//! What an audit run can prove it read (KT-1308): the dependency watch it was
//! handed and the URLs its agent fetched. The documentary check holds the
//! `[src: deps: …]` and `[src: url: …]` citations of a written doc against
//! them whatever the anti-hallucination mode: a version "read" from a source
//! the run never opened is invented, and is refused in favour of
//! `not verified: no source`.

use std::collections::BTreeSet;

use crate::core::anti_halluc::{extract_source_markers, url_reference};
use crate::core::version_sources::{verify_dep_marker, DependencySnapshot};

/// A tool input is a few hundred bytes; a bound keeps a runaway stream cheap.
const MAX_TOOL_INPUT_BYTES: usize = 16 * 1024;

/// The citation check of one audit run.
#[derive(Debug, Clone, Default)]
pub struct RunProvenance {
    /// The dependency watch injected into the run; `None` when it had none, in
    /// which case no `deps:` citation can be backed.
    pub deps: Option<DependencySnapshot>,
    /// The URLs the agent fetched, normalized; `None` when the run's agent does
    /// not report its tool inputs (the fetches cannot be told apart, so a
    /// `url:` citation is left unchecked rather than refused).
    pub fetched: Option<BTreeSet<String>>,
}

impl RunProvenance {
    /// One message per `deps:` / `url:` citation of `content` the run cannot
    /// back. Quoted examples (fences, inline code) are not citations.
    pub fn problems(&self, content: &str) -> Vec<String> {
        let mut out = Vec::new();
        for raw in extract_source_markers(content) {
            if let Some((status, detail)) = verify_dep_marker(&raw, self.deps.as_ref()) {
                if status.is_fabricated() {
                    out.push(format!(
                        "`[src: {raw}]` is not backed by this run: {detail}. Write \
                         `not verified: no source` instead of a version the dependency watch \
                         does not hold"
                    ));
                }
                continue;
            }
            let (Some(fetched), Some(url)) = (&self.fetched, url_reference(&raw)) else {
                continue;
            };
            if !fetched.contains(&normalize_url(&url)) {
                out.push(format!(
                    "`[src: {raw}]` was never fetched in this run. Fetch that page and read it, \
                     or write `not verified: no source` instead of a version or date taken from \
                     memory"
                ));
            }
        }
        out
    }
}

/// Scheme, host case, fragment and trailing `/` removed: two spellings of one
/// page compare equal.
pub fn normalize_url(url: &str) -> String {
    let url = url.trim().split('#').next().unwrap_or("");
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &url[scheme.len()..]))
        .unwrap_or(url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.trim_end_matches('/');
    let host = host.to_ascii_lowercase();
    if path.is_empty() {
        host
    } else {
        format!("{host}/{path}")
    }
}

/// Whether a tool, by the name its runtime reports, fetches a page by URL:
/// Claude's `WebFetch`, Kronn's `web_fetch`, an MCP `…fetch…` tool.
fn is_fetch_tool(name: &str) -> bool {
    name.to_ascii_lowercase().contains("fetch")
}

/// The URLs fetched by a CLI agent that streams its tool calls
/// (Claude stream-json): a call starts, its input arrives in fragments, it ends.
#[derive(Debug, Clone)]
pub struct FetchTracker {
    input: Option<String>,
    urls: BTreeSet<String>,
    observable: bool,
}

impl Default for FetchTracker {
    fn default() -> Self {
        Self {
            input: None,
            urls: BTreeSet::new(),
            observable: true,
        }
    }
}

impl FetchTracker {
    pub fn tool_started(&mut self, name: &str) {
        self.input = is_fetch_tool(name).then(String::new);
    }

    pub fn tool_input(&mut self, fragment: &str) {
        if let Some(input) = self.input.as_mut() {
            if input.len() + fragment.len() <= MAX_TOOL_INPUT_BYTES {
                input.push_str(fragment);
            }
        }
    }

    pub fn tool_ended(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let url = serde_json::from_str::<serde_json::Value>(&input)
            .ok()
            .and_then(|json| json.get("url").and_then(|u| u.as_str()).map(normalize_url));
        self.urls.extend(url.filter(|u| !u.is_empty()));
    }

    /// The fetches an ACP-hosted agent (Claude and Codex through their adapters,
    /// the native ACP agents) reported in the tool traces its process captured.
    pub fn ingest_traces(&mut self, captured: &[String]) {
        for trace in crate::agents::tool_trace::collect(captured) {
            if !trace.name.as_deref().is_some_and(is_fetch_tool) {
                continue;
            }
            let input = trace.input.unwrap_or_default();
            let url = serde_json::from_str::<serde_json::Value>(&input)
                .ok()
                .and_then(|json| json.get("url").and_then(|u| u.as_str()).map(str::to_owned))
                // The excerpt is bounded: a cut input may still hold its url.
                .or_else(|| {
                    let tail = input.split("\"url\":\"").nth(1)?;
                    Some(tail.split('"').next()?.to_owned())
                });
            self.urls
                .extend(url.map(|u| normalize_url(&u)).filter(|u| !u.is_empty()));
        }
    }

    /// An agent whose tool inputs are not reported was run: from now on the
    /// fetches of the run are not known.
    pub fn mark_unobservable(&mut self) {
        self.observable = false;
    }

    /// The fetches to check against, `None` when they are not all known.
    pub fn fetched(&self) -> Option<BTreeSet<String>> {
        self.observable.then(|| self.urls.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::version_sources::WatchedPackage;

    fn watch() -> DependencySnapshot {
        DependencySnapshot {
            checked_on: "2026-10-16".into(),
            packages: ["argparse", "joi", "qs"]
                .iter()
                .map(|name| WatchedPackage {
                    manager: "JS / TS".into(),
                    manifest: "package.json".into(),
                    name: (*name).into(),
                    current: "1.0.0".into(),
                    latest: "2.0.0".into(),
                    major: true,
                    declared_only: false,
                })
                .collect(),
            unchecked: vec![],
            up_to_date: vec![],
        }
    }

    fn provenance(fetched: &[&str]) -> RunProvenance {
        let mut tracker = FetchTracker::default();
        for url in fetched {
            tracker.tool_started("WebFetch");
            tracker.tool_input(&format!("{{\"url\":\"{url}\",\"prompt\":\"v\"}}"));
            tracker.tool_ended();
        }
        RunProvenance {
            deps: Some(watch()),
            fetched: tracker.fetched(),
        }
    }

    #[test]
    fn an_invented_latest_stable_with_fake_provenance_is_refused() {
        let table = "| Node | 23.0.0 [src: url: https://nodejs.org/en/about/previous-releases] (read 2026-10-16) |\n\
                     | Jest | 30.0.0 [src: deps: JS / TS/jest@2026-10-16] |\n";
        let problems = provenance(&[]).problems(table);
        assert_eq!(problems.len(), 2, "{problems:#?}");
        assert!(problems[0].contains("never fetched"), "{}", problems[0]);
        assert!(problems[0].contains("not verified"), "{}", problems[0]);
        assert!(
            problems[1].contains("not in the dependency watch"),
            "{}",
            problems[1]
        );
    }

    #[test]
    fn held_and_fetched_citations_pass() {
        let table = "joi lags [src: deps: JS / TS/joi@2026-10-16]; \
                     Node 22 [src: url: https://NodeJS.org/en/about/previous-releases/#lts] (read 2026-10-16)";
        let provenance = provenance(&["https://nodejs.org/en/about/previous-releases"]);
        assert!(provenance.problems(table).is_empty());
    }

    #[test]
    fn not_verified_and_quoted_examples_pass() {
        let text = "| Prettier | not verified: no source |\n\
                    Grammar: `[src: url: https://example.com]` and\n\
                    ```\n[src: deps: npm/ghost@2026-10-16]\n```\n";
        assert!(provenance(&[]).problems(text).is_empty());
    }

    #[test]
    fn a_run_with_no_dependency_watch_backs_no_deps_citation() {
        let mut run = provenance(&[]);
        run.deps = None;
        assert_eq!(
            run.problems("Jest [src: deps: JS / TS/jest@2026-10-16]")
                .len(),
            1
        );
    }

    #[test]
    fn url_citations_are_left_alone_when_the_fetches_are_not_observable() {
        let mut tracker = FetchTracker::default();
        tracker.mark_unobservable();
        let run = RunProvenance {
            deps: Some(watch()),
            fetched: tracker.fetched(),
        };
        assert!(run
            .problems("Node [src: url: https://nodejs.org/en/about] (read 2026-10-16)")
            .is_empty());
    }

    #[test]
    fn the_tracker_reads_only_fetch_tools_and_their_url() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("Read");
        tracker.tool_input("{\"url\":\"https://not-a-fetch.dev\"}");
        tracker.tool_ended();
        tracker.tool_started("mcp__fetch__fetch");
        tracker.tool_input("{\"ur");
        tracker.tool_input("l\":\"http://Example.com/a/\"}");
        tracker.tool_ended();
        tracker.tool_ended();
        assert_eq!(
            tracker.fetched().unwrap().into_iter().collect::<Vec<_>>(),
            vec!["example.com/a".to_string()]
        );
    }

    #[test]
    fn the_tracker_reads_the_fetches_of_an_acp_hosted_agent_from_its_traces() {
        let trace = |id: &str, name: &str, input: &str| {
            crate::agents::tool_trace::ToolTraceUpdate {
                id: id.into(),
                name: Some(name.into()),
                input: Some(input.into()),
                status: Some("in_progress".into()),
            }
            .marker()
        };
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[
            trace(
                "a",
                "WebFetch",
                r#"{"prompt":"p","url":"https://Nodejs.org/en/"}"#,
            ),
            trace("b", "Read", r#"{"url":"https://not-a-fetch.dev"}"#),
            trace(
                "c",
                "WebFetch",
                r#"{"prompt":"cut","url":"https://x.dev/a/b"#,
            ),
            "plain stderr line".into(),
        ]);
        assert_eq!(
            tracker.fetched().unwrap().into_iter().collect::<Vec<_>>(),
            vec!["nodejs.org/en".to_string(), "x.dev/a/b".to_string()]
        );
    }

    #[test]
    fn url_citation_forms_resolve_to_one_address() {
        assert_eq!(
            url_reference("url: https://a.dev/x (read 2026-10-16)").as_deref(),
            Some("https://a.dev/x")
        );
        assert_eq!(
            url_reference("https://a.dev/x").as_deref(),
            Some("https://a.dev/x")
        );
        assert_eq!(url_reference("file: src/a.rs:1"), None);
        assert_eq!(normalize_url("HTTPS://A.dev/x/#top"), "a.dev/x");
    }
}
