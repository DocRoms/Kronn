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
    /// What the run holds so far: its dependency watch and the pages its agent
    /// fetched up to now.
    pub fn of(deps: &Option<DependencySnapshot>, tracker: &FetchTracker) -> Self {
        Self {
            deps: deps.clone(),
            fetched: tracker.fetched(),
        }
    }

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

/// Scheme, userinfo, host case, a leading `www.`, fragment and trailing `/`
/// removed, and the value of a credential-style query parameter masked: two
/// spellings of one page compare equal, and what is stored holds no secret
/// (the scheme is gone, so the connection-string redaction can no longer see
/// `user:pass@`; a signed URL's `sig=` is not an assignment it knows).
pub fn normalize_url(url: &str) -> String {
    let url = url.trim().split('#').next().unwrap_or("");
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &url[scheme.len()..]))
        .unwrap_or(url);
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(mask_credential_params(query))),
        None => (tail, None),
    };
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let mut out = host.to_owned();
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        out.push('?');
        out.push_str(&query);
    }
    out
}

/// A query string whose credential-style values (signatures, tokens, keys,
/// passwords) are replaced by `***`: `sig`, `X-Amz-Signature`, `access_token`…
fn mask_credential_params(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_credential_param(key) => format!("{key}=***"),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_credential_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['_', '-'], "");
    matches!(key.as_str(), "sig" | "key" | "code" | "sas" | "auth")
        || [
            "signature",
            "token",
            "secret",
            "password",
            "passwd",
            "apikey",
            "credential",
            "authorization",
        ]
        .iter()
        .any(|word| key.contains(word))
}

/// Whether a tool, by the name its runtime reports, opens a page by the `url`
/// it is given: Claude's `WebFetch`, Kronn's `web_fetch`, an MCP fetch tool, a
/// browser's `browser_navigate` / `navigate_page` / `new_page`.
pub(crate) fn opens_url(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "fetch", "navigate", "new_page", "browse", "goto", "scrape", "crawl",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// Whether a tool is a documentation lookup whose RESULT names the pages it
/// read (Context7): the agent then cites a URL it was handed, not one it sent.
pub(crate) fn returns_urls(name: &str) -> bool {
    name.to_ascii_lowercase().contains("context7")
}

/// A sub-agent runs its own tools; whether their calls surface in the parent's
/// stream is not something Kronn can rely on.
fn is_subagent(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "task" | "agent")
}

/// Whether a shell command downloads a page.
fn downloads(command: &str) -> bool {
    command.contains("curl") || command.contains("wget")
}

fn url_pattern() -> &'static regex_lite::Regex {
    static PATTERN: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex_lite::Regex::new(r#"https?://[^\s"'<>\\`)\]}|;]+"#).expect("url pattern compiles")
    })
}

/// Every normalized `http(s)` URL written in `text`.
pub(crate) fn urls_in(text: &str) -> Vec<String> {
    url_pattern()
        .find_iter(text)
        .map(|m| normalize_url(m.as_str().trim_end_matches(['.', ',', ':'])))
        .filter(|u| !u.is_empty())
        .collect()
}

/// The text of a tool result: a string, or the `text` blocks of an array.
pub(crate) fn result_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The pages one tool call reads, from its structured input: the `url` of a
/// page opener (`opens`: a fetch or a browser navigation), the URLs of a
/// `curl` / `wget` command. Read from the whole input, before any excerpt.
pub(crate) fn input_urls(opens: bool, input: &serde_json::Value) -> Vec<String> {
    let mut urls = Vec::new();
    if opens {
        let single = input.get("url").and_then(|u| u.as_str());
        let many = input
            .get("urls")
            .and_then(|u| u.as_array())
            .into_iter()
            .flatten()
            .filter_map(|u| u.as_str());
        urls.extend(
            single
                .into_iter()
                .chain(many)
                .map(normalize_url)
                .filter(|u| !u.is_empty()),
        );
    }
    let command = input.get("command").map(|command| match command {
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        other => other.as_str().unwrap_or_default().to_owned(),
    });
    if let Some(command) = command.filter(|c| downloads(c)) {
        urls.extend(urls_in(&command));
    }
    urls
}

/// The pages one tool call reads, from the input text it reported. A bounded
/// excerpt may be cut: the `url` or the command it still holds is salvaged.
fn call_urls(name: &str, input: &str) -> Vec<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(input) {
        return input_urls(opens_url(name), &json);
    }
    let mut urls = Vec::new();
    if opens_url(name) {
        let url = input
            .split("\"url\":\"")
            .nth(1)
            .and_then(|tail| tail.split('"').next());
        urls.extend(url.map(normalize_url).filter(|u| !u.is_empty()));
    }
    if input.contains("\"command\"") && downloads(input) {
        urls.extend(urls_in(input));
    }
    urls
}

/// The URLs fetched by a CLI agent that streams its tool calls
/// (Claude stream-json): a call starts, its input arrives in fragments, it ends.
#[derive(Debug, Clone)]
pub struct FetchTracker {
    open: Option<(String, String)>,
    /// Ids of the documentation calls whose result is read for URLs.
    docs_calls: BTreeSet<String>,
    urls: BTreeSet<String>,
    observable: bool,
}

impl Default for FetchTracker {
    fn default() -> Self {
        Self {
            open: None,
            docs_calls: BTreeSet::new(),
            urls: BTreeSet::new(),
            observable: true,
        }
    }
}

impl FetchTracker {
    pub fn tool_started(&mut self, name: &str) {
        self.open = Some((name.to_owned(), String::new()));
    }

    pub fn tool_input(&mut self, fragment: &str) {
        if let Some((_, input)) = self.open.as_mut() {
            if input.len() + fragment.len() <= MAX_TOOL_INPUT_BYTES {
                input.push_str(fragment);
            }
        }
    }

    pub fn tool_ended(&mut self) {
        if let Some((name, input)) = self.open.take() {
            self.record(&name, &input);
        }
    }

    fn record(&mut self, name: &str, input: &str) {
        if is_subagent(name) {
            self.observable = false;
        }
        self.urls.extend(call_urls(name, input));
    }

    /// One raw stream-json line: the complete tool calls of an `assistant`
    /// message, and the result a documentation tool (Context7) sent back to the
    /// agent, whose pages are the ones the agent read.
    pub fn ingest_line(&mut self, line: &str) {
        if !line.contains("\"tool_use\"") && !line.contains("\"tool_result\"") {
            return;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            return;
        };
        let Some(blocks) = json.pointer("/message/content").and_then(|c| c.as_array()) else {
            return;
        };
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block
                        .get("input")
                        .map(|i| i.to_string())
                        .unwrap_or_default();
                    self.record(name, &input);
                    if let (true, Some(id)) = (
                        returns_urls(name),
                        block.get("id").and_then(|id| id.as_str()),
                    ) {
                        self.docs_calls.insert(id.to_owned());
                    }
                }
                Some("tool_result") => {
                    let held = block
                        .get("tool_use_id")
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| self.docs_calls.contains(id));
                    if held {
                        let text = block.get("content").map(result_text).unwrap_or_default();
                        self.urls.extend(urls_in(&text));
                    }
                }
                _ => {}
            }
        }
    }

    /// The fetches an ACP-hosted agent (Claude and Codex through their adapters,
    /// the native ACP agents) reported in the tool traces its process captured.
    /// A trace carries the pages its call opened, and the pages a documentation
    /// tool returned (read where the result is seen, never the result itself).
    /// A documentation call whose result was not seen leaves the pages the
    /// agent read unknown: the run's `url:` citations are then left unchecked.
    pub fn ingest_traces(&mut self, captured: &[String]) {
        for trace in crate::agents::tool_trace::collect(captured) {
            let Some(name) = trace.name.as_deref() else {
                self.urls.extend(trace.urls);
                continue;
            };
            // A lookup whose result was seen and named no page (Context7's
            // `resolve-library-id`) is no blind spot; one never seen is.
            if returns_urls(name) && trace.urls.is_empty() && !trace.result_seen {
                self.observable = false;
            }
            self.urls.extend(trace.urls.iter().cloned());
            self.record(name, &trace.input.unwrap_or_default());
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
    use serde_json::json;

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
                    lockfile: None,
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
                urls: vec![],
                result_seen: false,
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

    fn cites(url: &str) -> String {
        format!("Node 22 [src: url: {url}] (read 2026-10-16)")
    }

    fn run_of(tracker: &FetchTracker) -> RunProvenance {
        RunProvenance::of(&Some(watch()), tracker)
    }

    fn assistant_call(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn user_result(id: &str, content: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":{content}}}]}}}}"#
        )
    }

    #[test]
    fn a_page_a_context7_result_names_is_a_page_the_run_read() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_line(&assistant_call(
            "t1",
            "mcp__context7__query-docs",
            r#"{"libraryId":"/vercel/next.js","query":"app router"}"#,
        ));
        tracker.ingest_line(&user_result(
            "t1",
            r#"[{"type":"text","text":"Routing\nSource: https://nextjs.org/docs/app/building-your-application/routing\nSee [x](https://react.dev/reference/react)."}]"#,
        ));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites(
                "https://nextjs.org/docs/app/building-your-application/routing"
            ))
            .is_empty());
        assert!(run
            .problems(&cites("https://react.dev/reference/react"))
            .is_empty());
        // A page the result did not name is still invented.
        assert_eq!(run.problems(&cites("https://nodejs.org/en")).len(), 1);
    }

    #[test]
    fn only_a_documentation_tool_result_is_read_for_urls() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_line(&assistant_call("r1", "Read", r#"{"file_path":"/a.md"}"#));
        tracker.ingest_line(&user_result("r1", r#""see https://seen-in-a-file.dev/a""#));
        assert_eq!(
            run_of(&tracker)
                .problems(&cites("https://seen-in-a-file.dev/a"))
                .len(),
            1
        );
    }

    #[test]
    fn a_browser_navigation_is_a_fetch() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("mcp__Playwright__browser_navigate");
        tracker.tool_input(r#"{"url":"https://nodejs.org/en/about/previous-releases"}"#);
        tracker.tool_ended();
        tracker.ingest_line(&assistant_call(
            "d1",
            "mcp__Chrome DevTools__navigate_page",
            r#"{"type":"url","url":"https://vitest.dev/guide/"}"#,
        ));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://nodejs.org/en/about/previous-releases"))
            .is_empty());
        assert!(run.problems(&cites("https://vitest.dev/guide")).is_empty());
    }

    #[test]
    fn a_curl_or_wget_command_is_a_fetch() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("Bash");
        tracker.tool_input(r#"{"command":"curl -sL 'https://nodejs.org/dist/index.json' | head"}"#);
        tracker.tool_ended();
        tracker.ingest_line(&assistant_call(
            "b1",
            "Bash",
            r#"{"command":"wget -qO- https://registry.npmjs.org/jest/latest"}"#,
        ));
        // Codex reports the command as an array.
        tracker.record(
            "command_execution",
            r#"{"command":["curl","https://endoflife.date/api/node.json"]}"#,
        );
        // A command that downloads nothing names no page it read.
        tracker.ingest_line(&assistant_call(
            "b2",
            "Bash",
            r#"{"command":"echo https://example.com/not-fetched"}"#,
        ));
        let run = run_of(&tracker);
        for url in [
            "https://nodejs.org/dist/index.json",
            "https://registry.npmjs.org/jest/latest",
            "https://endoflife.date/api/node.json",
        ] {
            assert!(run.problems(&cites(url)).is_empty(), "{url}");
        }
        assert_eq!(
            run.problems(&cites("https://example.com/not-fetched"))
                .len(),
            1
        );
    }

    #[test]
    fn a_leading_www_does_not_make_another_page() {
        assert_eq!(normalize_url("https://www.Nodejs.org/en/"), "nodejs.org/en");
        let mut tracker = FetchTracker::default();
        tracker.tool_started("WebFetch");
        tracker.tool_input(r#"{"url":"https://www.typescriptlang.org/docs"}"#);
        tracker.tool_ended();
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://typescriptlang.org/docs"))
            .is_empty());
        assert!(run
            .problems(&cites("https://www.typescriptlang.org/docs/"))
            .is_empty());
    }

    #[test]
    fn a_sub_agent_leaves_the_url_citations_unchecked() {
        for name in ["Task", "Agent"] {
            let mut tracker = FetchTracker::default();
            tracker.ingest_line(&assistant_call(
                "s1",
                name,
                r#"{"description":"look up the LTS","prompt":"fetch it"}"#,
            ));
            assert!(
                run_of(&tracker)
                    .problems(&cites("https://nodejs.org/en"))
                    .is_empty(),
                "{name}"
            );
            // The dependency watch is still the run's own.
            assert_eq!(
                run_of(&tracker)
                    .problems("Jest [src: deps: JS / TS/jest@2026-10-16]")
                    .len(),
                1
            );
        }
    }

    #[test]
    fn an_acp_run_reads_browsers_and_commands_and_cannot_see_a_docs_result() {
        let trace = |id: &str, name: &str, input: &str| {
            crate::agents::tool_trace::ToolTraceUpdate {
                id: id.into(),
                name: Some(name.into()),
                input: Some(input.into()),
                status: Some("in_progress".into()),
                urls: vec![],
                result_seen: false,
            }
            .marker()
        };
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[
            trace(
                "a",
                "mcp__playwright__browser_navigate",
                r#"{"url":"https://vitest.dev/guide"}"#,
            ),
            trace(
                "b",
                "command_execution",
                r#"{"command":"curl -s https://nodejs.org/dist/index.json"}"#,
            ),
        ]);
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://vitest.dev/guide")).is_empty());
        assert!(run
            .problems(&cites("https://nodejs.org/dist/index.json"))
            .is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io")).len(), 1);

        tracker.ingest_traces(&[trace(
            "c",
            "mcp__context7__query-docs",
            r#"{"query":"jest"}"#,
        )]);
        assert!(run_of(&tracker)
            .problems(&cites("https://jestjs.io"))
            .is_empty());
    }

    /// The traces the ACP adapter would capture for these Claude lines.
    fn claude_traces(lines: &[String]) -> Vec<String> {
        let mut reader = crate::agents::tool_trace::ClaudeTraceReader::default();
        lines
            .iter()
            .flat_map(|line| reader.read(line))
            .map(|trace| trace.marker())
            .collect()
    }

    #[test]
    fn a_context7_result_url_survives_the_acp_trace() {
        let page = "https://nextjs.org/docs/app/routing";
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "t1",
                "mcp__context7__query-docs",
                r#"{"libraryId":"/vercel/next.js","query":"routing"}"#,
            ),
            user_result(
                "t1",
                &format!(r#"[{{"type":"text","text":"Source: {page}\nmore"}}]"#),
            ),
            // A result of another tool is not read for URLs.
            assistant_call("t2", "Read", r#"{"file_path":"/a.md"}"#),
            user_result("t2", r#""see https://seen-in-a-file.dev/a""#),
        ]));
        let run = run_of(&tracker);
        assert!(run.fetched.is_some(), "the result was seen");
        assert!(run.problems(&cites(page)).is_empty());
        assert_eq!(
            run.problems(&cites("https://seen-in-a-file.dev/a")).len(),
            1
        );
        assert_eq!(run.problems(&cites("https://react.dev")).len(), 1);
    }

    #[test]
    fn a_trace_keeps_the_url_of_an_input_the_excerpt_cut() {
        let long = "x".repeat(5000);
        let fetch = format!(r#"{{"url":"https://a.dev/page","prompt":"{long}"}}"#);
        let shell = format!(r#"{{"command":"echo {long} && curl -s https://b.dev/api"}}"#);
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call("f1", "WebFetch", &fetch),
            assistant_call("f2", "Bash", &shell),
        ]));
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://a.dev/page")).is_empty());
        assert!(run.problems(&cites("https://b.dev/api")).is_empty());
        assert_eq!(run.problems(&cites("https://c.dev")).len(), 1);
    }

    #[test]
    fn a_native_acp_fetch_is_known_by_its_kind_even_retitled_to_its_url() {
        use crate::agents::tool_trace::from_acp;
        let frames = [
            json!({"toolCallId":"n1","kind":"fetch","title":"Fetch","status":"pending",
                   "rawInput":{"url":"https://www.x.dev/docs"}}),
            // The runtime retitles the call to the page it opened.
            json!({"toolCallId":"n1","title":"https://x.dev/docs","status":"completed"}),
            json!({"toolCallId":"n2","kind":"fetch","title":"https://y.dev/a?b=1",
                   "status":"in_progress"}),
            json!({"toolCallId":"n3","kind":"read","title":"Read",
                   "rawInput":{"url":"https://not-a-fetch.dev"}}),
        ];
        let captured: Vec<String> = frames
            .iter()
            .map(|frame| from_acp(frame).unwrap().marker())
            .collect();
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&captured);
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://x.dev/docs")).is_empty());
        assert!(run.problems(&cites("https://y.dev/a?b=1")).is_empty());
        assert_eq!(run.problems(&cites("https://not-a-fetch.dev")).len(), 1);
    }

    #[test]
    fn a_codex_context7_result_url_survives_the_trace() {
        let item = json!({"id":"c1","type":"mcp_tool_call","server":"context7",
            "tool":"query-docs","arguments":{"query":"jest"},"status":"completed",
            "result":{"content":[{"type":"text","text":"Docs: https://jestjs.io/docs/api\nend"}]}});
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[crate::agents::tool_trace::from_codex(&item)
            .unwrap()
            .marker()]);
        let run = run_of(&tracker);
        assert!(run.fetched.is_some());
        assert!(run
            .problems(&cites("https://jestjs.io/docs/api"))
            .is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io/blog")).len(), 1);
    }

    #[test]
    fn a_resolved_library_id_names_no_page_and_is_no_blind_spot() {
        let page = "https://jestjs.io/docs/getting-started";
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "r1",
                "mcp__context7__resolve-library-id",
                r#"{"libraryName":"jest","query":"latest"}"#,
            ),
            user_result(
                "r1",
                r#"[{"type":"text","text":"- Title: Jest\n- Context7-compatible library ID: /facebook/jest\n- Code Snippets: 120\n- Benchmark Score: 91"}]"#,
            ),
            assistant_call(
                "q1",
                "mcp__context7__query-docs",
                r#"{"libraryId":"/facebook/jest","query":"latest"}"#,
            ),
            user_result(
                "q1",
                &format!(r#"[{{"type":"text","text":"Source: {page}"}}]"#),
            ),
        ]));
        let run = run_of(&tracker);
        assert!(run.fetched.is_some(), "the resolve result was seen");
        assert!(run.problems(&cites(page)).is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io/blog")).len(), 1);

        // A resolve alone: still checked, the invented page is refused.
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "r1",
                "mcp__context7__resolve-library-id",
                r#"{"libraryName":"jest"}"#,
            ),
            user_result(
                "r1",
                r#""- Context7-compatible library ID: /facebook/jest""#,
            ),
        ]));
        assert_eq!(
            run_of(&tracker)
                .problems(&cites("https://jestjs.io/blog"))
                .len(),
            1
        );
    }

    #[test]
    fn a_documentation_call_whose_result_never_arrived_leaves_the_citations_unchecked() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[assistant_call(
            "q1",
            "mcp__context7__query-docs",
            r#"{"query":"jest"}"#,
        )]));
        assert!(run_of(&tracker).fetched.is_none());
    }

    #[test]
    fn a_native_acp_resolve_with_a_seen_result_is_no_blind_spot() {
        use crate::agents::tool_trace::from_acp;
        let frame = json!({"toolCallId":"n1","title":"mcp__context7__resolve-library-id",
            "status":"completed","rawOutput":{"id":"/facebook/jest","snippets":120}});
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[from_acp(&frame).unwrap().marker()]);
        assert!(run_of(&tracker).fetched.is_some());
        let unseen = json!({"toolCallId":"n2","title":"mcp__context7__resolve-library-id",
            "status":"in_progress"});
        tracker.ingest_traces(&[from_acp(&unseen).unwrap().marker()]);
        assert!(run_of(&tracker).fetched.is_none());
    }

    #[test]
    fn a_url_keeps_no_userinfo_and_no_signature() {
        assert_eq!(
            normalize_url("https://deploy:hunter2@Files.example.com/a.json"),
            "files.example.com/a.json"
        );
        assert_eq!(
            normalize_url("https://u@x.dev?z=1"),
            "x.dev?z=1",
            "a query with no path"
        );
        let signed = "https://bucket.s3.amazonaws.com/a.json?X-Amz-Algorithm=AWS4-HMAC-SHA256\
            &X-Amz-Credential=AKIAFAKE%2F20261018&X-Amz-Signature=deadbeef01&version=3";
        let n = normalize_url(signed);
        assert!(!n.contains("deadbeef01") && !n.contains("AKIAFAKE"), "{n}");
        assert!(n.contains("version=3"), "{n}");
        for (url, secret) in [
            (
                "https://a.blob.core.windows.net/c/f?sv=1&sig=s3cr3tsig",
                "s3cr3tsig",
            ),
            ("https://a.dev/f?access_token=tok123abc", "tok123abc"),
            ("https://a.dev/f?api-key=key123abc&page=2", "key123abc"),
        ] {
            assert!(!normalize_url(url).contains(secret), "{url}");
        }
        // Two spellings of one signed page still compare equal.
        assert_eq!(
            normalize_url("https://a.dev/f?sig=one&v=1"),
            normalize_url("https://www.a.dev/f?sig=two&v=1")
        );
    }

    #[test]
    fn the_stored_trace_urls_hold_no_secret() {
        let mut tracker = FetchTracker::default();
        let lines = claude_traces(&[
            assistant_call(
                "f1",
                "WebFetch",
                r#"{"url":"https://ci:hunter2@dl.example.com/a?sig=abc123secret&v=2","prompt":"x"}"#,
            ),
            assistant_call("t1", "mcp__context7__query-docs", r#"{"query":"q"}"#),
            user_result(
                "t1",
                r#""Docs https://docs.example.com/x?X-Amz-Signature=feedface99&p=1""#,
            ),
        ]);
        // The URL list the trace records (the input excerpt is redacted by its
        // own, older rules).
        let stored: String = crate::agents::tool_trace::collect(&lines)
            .iter()
            .flat_map(|trace| trace.urls.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!stored.is_empty());
        for secret in ["hunter2", "abc123secret", "feedface99"] {
            assert!(!stored.contains(secret), "{secret} in {stored}");
        }
        tracker.ingest_traces(&lines);
        let run = run_of(&tracker);
        // The citation, written with or without the secret, matches the page.
        assert!(run
            .problems(&cites("https://dl.example.com/a?sig=other&v=2"))
            .is_empty());
        assert!(run
            .problems(&cites("https://docs.example.com/x?X-Amz-Signature=zz&p=1"))
            .is_empty());
    }

    #[test]
    fn a_large_documentation_result_still_backs_a_late_citation() {
        let text: String = (0..200)
            .map(|i| format!("Page https://docs.example.com/p/{i}\n"))
            .collect();
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call("q1", "mcp__context7__query-docs", r#"{"query":"all"}"#),
            user_result("q1", &json!([{"type":"text","text":text}]).to_string()),
        ]));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://docs.example.com/p/199"))
            .is_empty());
        assert_eq!(
            run.problems(&cites("https://docs.example.com/p/200")).len(),
            1
        );
    }
}
