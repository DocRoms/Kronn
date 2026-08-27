//! Native tools that give an HTTP agent a web and a workspace (KT-338).
//!
//! Until now the native catalogue was Kronn-internal only: plan, tasks, QA and
//! `api_call` (which can reach *configured* REST plugins, never an arbitrary
//! URL). So an HTTP agent — Ollama, LiteLLM, NVIDIA — could talk about work but
//! not do any: no page to read, no file to open, no file to produce. The gap was
//! never per-provider, it was per *execution mode*: the tool loop, the codec and
//! the executor plumbing are shared, only the catalogue was empty.
//!
//! These tools run SERVER-SIDE, which is what makes them safe to hand to a model
//! that has no shell. Three boundaries carry that safety, and each is enforced
//! here rather than documented as a convention:
//!
//! * **Reach** — `web_fetch` refuses anything that resolves to a private or
//!   loopback address. Without it, a model could ask Kronn to read the host's
//!   own metadata service or an internal admin panel: the classic SSRF, made
//!   trivial by the fact that the *server* issues the request, not the model.
//! * **Scope** — file tools resolve every path inside the discussion's declared
//!   workspace and refuse anything that escapes it, after canonicalisation (so
//!   `..`, a symlink, or an absolute path all land on the same refusal).
//! * **Mutation** — the only Git mutation is a commit of explicitly named files,
//!   and the caller separately proves that this is the current managed task
//!   worktree. There is no shell, checkout, amend or push surface.
//! * **Size** — responses and files are read up to a cap and the truncation is
//!   *announced in the payload*. A silently clipped file is worse than a refused
//!   one: the model would reason confidently about content it never saw.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Cap on a fetched body and on a file read. Large enough for a page or a source
/// file, small enough that one tool call cannot blow the model's window.
const MAX_BYTES: usize = 256 * 1024;

/// Deadline for one fetch. Shorter than the tool loop's patience so a hung host
/// costs one turn, not the whole run.
const FETCH_TIMEOUT_SECS: u64 = 20;

/// Ceilings for a recursive walk. A non-recursive listing forced the model to
/// spend one turn per directory, which made exploring a real project impossible
/// (observed in production: an agent gave up on "find the largest file"). Walking
/// is therefore allowed, but bounded — and the truncation is reported, never
/// silent, so the model knows its view is partial.
const MAX_WALK_DEPTH: usize = 8;
/// Raised from 800 after a real repository proved it useless: walking Kronn from
/// the root spent the whole budget before ever reaching `backend/src`, so
/// `find_files("**/*.rs")` answered "there are no .rs files" — confidently wrong,
/// which is worse than refusing. Entries are small (path, is_dir, size), so the
/// ceiling can be generous; it exists to bound pathological trees, not real ones.
const MAX_WALK_ENTRIES: usize = 20_000;

/// Ceilings for a content search. The point of the tool is to replace paging a
/// large file with one targeted question, so the answer must stay small enough
/// to read in one turn: a hundred hits across a repository is already enough to
/// know where to look, and more would just be the paging problem again.
const MAX_SEARCH_MATCHES: usize = 100;
/// Per file, so one generated file cannot crowd out every other hit.
const MAX_SEARCH_MATCHES_PER_FILE: usize = 20;
/// A single matched line, exact (indentation included): enough to recognise the
/// hit and to use as an `edit_file` anchor, capped so a minified bundle cannot
/// spend the window on one line.
const MAX_SEARCH_LINE_CHARS: usize = 240;
/// Files larger than this are not searched. Beyond it a text file is generated,
/// and a hit inside it tells the model nothing it can act on.
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Ceiling for a file `edit_file` will load. Editing needs the whole file in
/// memory to anchor the replacement; a text file past this is generated, and
/// generated files are rewritten by their generator, not patched.
const MAX_EDIT_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Stable structural-refusal marker consumed by the bounded worker loop. It is
/// not merely prose: seeing it moves a local worker directly into its single
/// strict correction attempt instead of granting more repository exploration.
pub(crate) const RUST_SYNTAX_REFUSAL_PREFIX: &str = "Rust syntax validation refused";

/// Byte-exact optimistic-concurrency receipt shared by read/search/edit tools.
/// Hashing the bytes already read matters: hashing the path in a second I/O
/// would let a concurrent writer make the receipt describe different content
/// from the text returned to the model.
fn content_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Refuse a Rust mutation before its bytes reach disk when the proposed file
/// is not a complete syntax tree. This is intentionally a per-file structural
/// gate, not a compiler invocation: unresolved names and cross-file refactors
/// remain possible, while a missing delimiter can never poison the managed
/// worktree that a stronger fallback must resume.
fn validate_proposed_source(path: &Path, requested: &str, content: &str) -> Result<(), String> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
        return Ok(());
    }
    syn::parse_file(content).map(|_| ()).map_err(|error| {
        let start = error.span().start();
        format!(
            "{RUST_SYNTAX_REFUSAL_PREFIX} `{requested}` at line {line}, column {column}: \
             {error}. Nothing was written; the previous `content_sha256` remains authoritative. \
             Make one bounded repair from this exact parser error, then hand off to a stronger \
             worker instead of exploring again.",
            line = start.line,
            column = start.column + 1,
        )
    })
}

/// A local model occasionally copies a long hexadecimal receipt without its
/// final token(s). Requiring the whole 256-bit spelling turned a correct,
/// bounded edit into a retry loop, even though a 128-bit prefix is already a
/// cryptographically strong optimistic-concurrency capability. Never accept a
/// shorter prefix, and always compare it against the current whole-file hash.
const MIN_SHA256_RECEIPT_CHARS: usize = 32;

fn sha256_receipt_is_well_formed(receipt: &str) -> bool {
    (MIN_SHA256_RECEIPT_CHARS..=64).contains(&receipt.len())
        && receipt.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_receipt_matches(actual: &str, receipt: &str) -> bool {
    sha256_receipt_is_well_formed(receipt)
        && actual
            .get(..receipt.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(receipt))
}

fn invalid_sha256_receipt_message() -> String {
    format!(
        "`expected_sha256` must be a {MIN_SHA256_RECEIPT_CHARS}-to-64-character hexadecimal prefix of the `content_sha256` returned by `read_file` or `search_text`."
    )
}

/// Directories that are never worth walking into: they are enormous, generated,
/// and contain nothing the model reasons about. Skipping them is what makes a
/// bounded walk useful rather than exhausted on build artefacts.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "__pycache__",
    ".pnpm-store",
];

/// Why a request or a path was refused. Every variant is a *readable* refusal the
/// model can act on, never an opaque error string.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// Not http(s): `file://`, `gopher://`, a data URI…
    UnsupportedScheme(String),
    /// Resolves to a loopback, private, link-local or unspecified address.
    PrivateAddress(String),
    /// The path leaves the workspace once canonicalised.
    OutsideWorkspace(String),
    /// The discussion has no declared workspace, so there is nothing to scope to.
    NoWorkspace,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::UnsupportedScheme(url) => format!(
                "refused: only http and https URLs can be fetched (got `{url}`). \
                 Local files are read with read_file, inside the workspace."
            ),
            Refusal::PrivateAddress(host) => format!(
                "refused: `{host}` resolves to a private or loopback address. \
                 Kronn issues this request from the server, so an internal host \
                 would be reachable that you could not reach yourself."
            ),
            Refusal::OutsideWorkspace(path) => format!(
                "refused: `{path}` is outside this discussion's workspace. \
                 Paths are relative to the workspace root; `..` and absolute \
                 paths cannot leave it."
            ),
            Refusal::NoWorkspace => "refused: this discussion is not attached to any \
                 directory — it has no project and no workspace — so there is nothing to \
                 read from or write to. Attach it to a project to give it one."
                .to_string(),
        }
    }
}

/// Is this host safe to fetch from? Rejects loopback, private ranges, link-local
/// and unspecified addresses — including a hostname that *resolves* to one, which
/// is why resolution happens before the request and not inside `reqwest`.
///
/// Pure over an already-resolved address list so it is testable without DNS.
pub fn is_public_addr(addr: &std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // 100.64.0.0/10 (carrier NAT) and 169.254/16 are already covered
                // above; 0.0.0.0/8 and 240/4 are caught here.
                || v4.octets()[0] == 0
                || v4.octets()[0] >= 240)
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 unique-local and fe80::/10 link-local.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Validate a URL for fetching: scheme first (cheap, no I/O), then every address
/// the host resolves to. A host is refused if *any* of its addresses is private —
/// a name that resolves to both a public and a private address is exactly the
/// DNS-rebinding shape we must not follow.
pub async fn check_fetch_url(raw: &str) -> Result<reqwest::Url, Refusal> {
    let url = reqwest::Url::parse(raw).map_err(|_| Refusal::UnsupportedScheme(raw.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Refusal::UnsupportedScheme(raw.to_string()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Refusal::UnsupportedScheme(raw.to_string()))?
        .to_string();
    // A literal address needs no DNS round-trip.
    if let Ok(literal) = host.parse::<std::net::IpAddr>() {
        return if is_public_addr(&literal) {
            Ok(url)
        } else {
            Err(Refusal::PrivateAddress(host))
        };
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let lookup_host = host.clone();
    let addrs = tokio::net::lookup_host((lookup_host.as_str(), port))
        .await
        .map_err(|_| Refusal::PrivateAddress(host.clone()))?
        .map(|socket| socket.ip())
        .collect::<Vec<_>>();
    if addrs.is_empty() || !addrs.iter().all(is_public_addr) {
        return Err(Refusal::PrivateAddress(host));
    }
    Ok(url)
}

/// Fetch a validated URL and return the body plus whether it was truncated.
/// Truncation is a *field*, not a silent cut: the model must know it is reasoning
/// about a partial document.
pub async fn fetch_text(url: reqwest::Url) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        // A redirect could land on a private address that passed the pre-flight
        // check, so redirects are followed only within the same guarantee: we
        // re-validate each hop by refusing them outright and reporting the
        // Location, which keeps the guard honest instead of approximate.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("could not build client: {error}"))?;
    fetch_text_with_client(&client, url).await
}

/// Transport half of [`fetch_text`], split out so timeout and oversized-body
/// behavior can be exercised against a local mock after the public SSRF gate.
async fn fetch_text_with_client(
    client: &reqwest::Client,
    url: reqwest::Url,
) -> Result<Value, String> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| format!("fetch failed: {error}"))?;
    let status = response.status().as_u16();
    if let Some(location) = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
    {
        return Ok(json!({
            "url": url.to_string(),
            "status": status,
            "redirect_to": location,
            "note": "redirect not followed automatically — re-issue web_fetch with this URL \
                     so its address is validated too",
        }));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    // Never materialise an unbounded response merely to truncate it later.
    // MAX_BYTES + 1 is enough to prove truncation; the remaining stream is
    // dropped immediately once that byte arrives.
    let mut body = Vec::with_capacity(MAX_BYTES.saturating_add(1));
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("could not read body: {error}"))?;
        let remaining = MAX_BYTES.saturating_add(1).saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() > MAX_BYTES {
            break;
        }
    }
    let truncated = body.len() > MAX_BYTES;
    let prefix = &body[..body.len().min(MAX_BYTES)];
    // HTTP bodies are not guaranteed to be valid UTF-8. Lossy decoding keeps
    // the tool readable, then this second cap ensures replacement characters
    // cannot make the returned UTF-8 payload exceed the advertised byte bound.
    let decoded = String::from_utf8_lossy(prefix);
    let mut text = String::with_capacity(prefix.len());
    for ch in decoded.chars() {
        if text.len().saturating_add(ch.len_utf8()) > MAX_BYTES {
            break;
        }
        text.push(ch);
    }
    Ok(json!({
        "url": url.to_string(),
        "status": status,
        "content_type": content_type,
        "truncated": truncated,
        "bytes_returned": text.len(),
        "text": text,
    }))
}

/// Resolve a model-supplied path inside `root`, refusing anything that escapes.
///
/// The check is on the *canonical* form of the parent chain, so `..`, a symlink
/// pointing outside, and an absolute path are all caught by the same rule. The
/// path need not exist yet (a write creates it), which is why the deepest
/// existing ancestor is canonicalised rather than the target itself.
pub fn resolve_in_workspace(root: &Path, requested: &str) -> Result<PathBuf, Refusal> {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        return Err(Refusal::OutsideWorkspace(requested.to_string()));
    }
    // Reject the traversal syntactically first: clearer message, and it avoids
    // depending on canonicalisation for the common case.
    if requested_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Refusal::OutsideWorkspace(requested.to_string()));
    }
    let root_canonical = root.canonicalize().map_err(|_| Refusal::NoWorkspace)?;
    let target = root_canonical.join(requested_path);
    // Canonicalise the deepest existing ancestor: this is what catches a symlink
    // that leaves the workspace even though the textual path looks innocent.
    let mut probe = target.clone();
    loop {
        match probe.canonicalize() {
            Ok(resolved) => {
                if !resolved.starts_with(&root_canonical) {
                    return Err(Refusal::OutsideWorkspace(requested.to_string()));
                }
                break;
            }
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent.to_path_buf(),
                _ => return Err(Refusal::OutsideWorkspace(requested.to_string())),
            },
        }
    }
    Ok(target)
}

/// Cap on a diff or a log payload. A full diff of a large branch would blow the
/// model's window and be useless; truncation is reported so a partial diff is
/// never mistaken for the whole change.
const MAX_GIT_BYTES: usize = 192 * 1024;

/// Run ONE of a closed set of read-only git subcommands in the workspace.
///
/// This is deliberately NOT a shell: the model never supplies a command, only a
/// path or a revision, and every argument list below is written here. A shell tool
/// would be arbitrary code execution on the host — for a model running on someone
/// else's servers, that is not a boundary worth crossing. Read-only git is what an
/// agent actually needs to review work. The separate commit primitive below is
/// narrower still: named paths only, after durable execution authorisation.
fn git_read(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::core::cmd::sync_cmd("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("git could not run: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "git returned an error".to_string()
        } else {
            // Trim: a git error can be long, and the model only needs the reason.
            stderr.chars().take(600).collect()
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn truncated_payload(text: String) -> (String, bool) {
    if text.len() <= MAX_GIT_BYTES {
        return (text, false);
    }
    let mut end = MAX_GIT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Working-tree state: current branch plus the porcelain status. This is the call
/// that answers "what are you working on right now", which a file-reading agent
/// could not otherwise know (observed in production: an agent found the branch by
/// reading .git/HEAD by hand, then had to ask the human to paste a diff).
pub fn git_status_payload(root: &Path) -> Result<Value, String> {
    let branch = git_read(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let status = git_read(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    let entries: Vec<Value> = status
        .lines()
        .filter(|line| line.len() > 3)
        .map(|line| {
            json!({
                // Porcelain v1: two status chars, then the path.
                "state": line[..2].trim().to_string(),
                "path": line[3..].to_string(),
            })
        })
        .collect();
    Ok(json!({
        "branch": branch,
        "changed_count": entries.len(),
        "changes": entries,
    }))
}

/// The diff. `revision_range` is optional (`main...HEAD`, a single sha…); without
/// it, the uncommitted work — which is what "review my current work" means.
pub fn git_diff_payload(
    root: &Path,
    revision_range: Option<&str>,
    path: Option<&str>,
) -> Result<Value, String> {
    // A revision is model-supplied, so refuse anything that could turn into an
    // option: `--output=…` would let a read tool write a file.
    if let Some(rev) = revision_range {
        if rev.starts_with('-') {
            return Err("refused: a revision cannot start with `-`".into());
        }
    }
    if let Some(rel) = path {
        // Reuse the workspace guard so a diff cannot reach outside either.
        resolve_in_workspace(root, rel).map_err(|refusal| refusal.message())?;
    }
    let mut args: Vec<&str> = vec!["diff"];
    if let Some(rev) = revision_range {
        args.push(rev);
    }
    if let Some(rel) = path {
        args.push("--");
        args.push(rel);
    }
    let raw = git_read(root, &args)?;
    let (text, truncated) = truncated_payload(raw);
    Ok(json!({
        "revision_range": revision_range.unwrap_or(""),
        "path": path.unwrap_or(""),
        "truncated": truncated,
        "diff": text,
    }))
}

/// Recent commits, one line each: enough to situate work without reading the diff.
/// When `path` is given, only the commits touching that path are listed — the
/// path is resolved inside the workspace first, so a path that escapes is refused.
pub fn git_log_payload(
    root: &Path,
    limit: Option<u32>,
    path: Option<&str>,
) -> Result<Value, String> {
    let count = limit.unwrap_or(20).clamp(1, 100).to_string();
    let mut args: Vec<&str> = vec![
        "log",
        "--max-count",
        &count,
        "--date=short",
        "--format=%h|%ad|%an|%s",
    ];
    if let Some(rel) = path {
        // Reuse the workspace guard so a log cannot reach outside either.
        resolve_in_workspace(root, rel).map_err(|refusal| refusal.message())?;
        args.push("--");
        args.push(rel);
    }
    let raw = git_read(root, &args)?;
    let commits: Vec<Value> = raw
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            Some(json!({
                "sha": parts.next()?,
                "date": parts.next()?,
                "author": parts.next()?,
                "subject": parts.next()?,
            }))
        })
        .collect();
    Ok(json!({ "count": commits.len(), "commits": commits }))
}

/// Commit only explicitly named paths in an already-authorised managed task
/// worktree. Authorisation is intentionally not inferred from a path here: the
/// executor proves ownership from SQLite before calling this filesystem helper.
/// Every path is validated before the first `git add`, so a refused final entry
/// cannot leave earlier entries staged.
pub fn git_commit_payload(root: &Path, files: &[String], message: &str) -> Result<Value, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("refused: commit message cannot be empty".into());
    }
    if message.chars().count() > 500 {
        return Err("refused: commit message cannot exceed 500 characters".into());
    }
    if files.is_empty() {
        return Err("refused: `files` must name at least one relative path".into());
    }
    if files.len() > 100 {
        return Err("refused: at most 100 paths can be committed at once".into());
    }

    let canonical_root = root
        .canonicalize()
        .map_err(|_| Refusal::NoWorkspace.message())?;
    let mut normalized = BTreeSet::new();
    for requested in files {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err("refused: commit paths cannot be empty".into());
        }
        let resolved = resolve_in_workspace(&canonical_root, requested)
            .map_err(|refusal| refusal.message())?;
        let relative = resolved
            .strip_prefix(&canonical_root)
            .map_err(|_| Refusal::OutsideWorkspace(requested.to_string()).message())?;
        if relative.as_os_str().is_empty() {
            return Err("refused: commit paths must name files, not the workspace root".into());
        }
        normalized.insert(relative.to_string_lossy().to_string());
    }
    let normalized: Vec<String> = normalized.into_iter().collect();
    let committed =
        crate::api::git_ops::run_git_commit(&canonical_root, &normalized, message, false, false)?;
    Ok(json!({
        "hash": committed.hash,
        "message": committed.message,
        "files": normalized,
    }))
}

/// The tool definitions this module contributes to the native catalogue.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch an http(s) URL and return its text. Use it to read \
                                documentation, an API response or a page you were given. \
                                Kronn issues the request server-side: private and loopback \
                                addresses are refused, the body is capped, and `truncated` \
                                tells you when you are seeing only part of it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Absolute http(s) URL." },
                    },
                    "required": ["url"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file from this discussion's workspace. The path is \
                                relative to the workspace root; it cannot escape it. \
                                `found` is false when the file does not exist; `text` is then \
                                empty because there was nothing to read, not because the file is \
                                empty. `truncated` tells you when the file was longer than the \
                                cap. A large source file can exceed your context in one call: \
                                pass `offset` and `limit` to read it in slices. The reply carries \
                                `total_lines`, `next_offset` and a byte-exact `content_sha256` \
                                revision for the whole file. Pass that revision to `edit_lines`, \
                                `edit_file`, or a whole-file `write_file`; a stale mutation is \
                                refused. Slices preserve the file's own line \
                                endings, so copied anchors stay byte-exact. `next_offset` is where you COULD \
                                continue, not where you should: stop as soon as you have what you \
                                came for. To find something rather than survey a file, use \
                                `search_text` — reading a large file end to end costs the turn.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the workspace root, e.g. `src/main.rs`.",
                        },
                        "offset": {
                            "type": "integer",
                            "description": "First line to return, 1-based. Omit to start at the top.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "How many lines to return from `offset`. Omit for the \
                                            whole file, subject to the byte cap.",
                        },
                    },
                    "required": ["path"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create a NEW text file, or replace a small existing file WHOLE, \
                                in this discussion's workspace. Creation never overwrites an \
                                existing path, even if another worker wins the race. Replacing an \
                                existing file requires at least the first 32 hexadecimal \
                                characters of its `content_sha256` from \
                                `read_file` or `search_text`; a stale or missing receipt is refused. \
                                To change only part of an existing file, prefer `edit_lines`: it \
                                needs only the replacement range, never the rest. Parent \
                                directories are created and the path cannot escape the workspace. \
                                A proposed `.rs` file must parse as a complete Rust syntax tree \
                                before Kronn writes any byte.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the workspace root.",
                        },
                        "content": { "type": "string", "description": "Full file content." },
                        "expected_sha256": {
                            "type": "string",
                            "minLength": 32,
                            "maxLength": 64,
                            "description": "Required only when the path already exists: 32 to 64 \
                                            leading hexadecimal characters of the `content_sha256` \
                                            returned by the read/search used to construct this \
                                            whole-file replacement."
                        },
                    },
                    "required": ["path", "content"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace one exact region of an existing file — the way to make a \
                                change without rewriting the file. Workflow: `search_text` to find \
                                the place, `read_file` with `offset`/`limit` to read that region, \
                                then pass the exact text, the file's `content_sha256` receipt, and \
                                its replacement. \
                                `old_string` must match byte for byte, indentation included — copy \
                                it from `read_file` output (the line `search_text` \
                                returns is exact when `text_truncated` is false, indentation \
                                included, and can serve \
                                as an `edit_file` anchor) — and must appear exactly once, \
                                unless `replace_all`. \
                                If the file changed after the read/search receipt, the edit is \
                                refused: re-read and decide against the new bytes. Kronn never \
                                guesses an edit from approximately matching whitespace. A `.rs` \
                                proposal is syntax-parsed before any write; on refusal, use the \
                                unchanged receipt for one bounded repair from the exact parser \
                                error, then hand off to a stronger worker. \
                                The reply names `first_changed_line` so you can re-read just that \
                                region to verify.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the workspace root, e.g. `src/main.rs`. \
                                            The file must already exist.",
                        },
                        "old_string": {
                            "type": "string",
                            "description": "Exact text to replace, copied byte for byte from \
                                            `read_file` — include enough surrounding lines to be \
                                            unique in the file.",
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement text. May be empty to delete `old_string`.",
                        },
                        "expected_sha256": {
                            "type": "string",
                            "minLength": 32,
                            "maxLength": 64,
                            "description": "The first 32 to 64 hexadecimal characters of the \
                                            `content_sha256` returned by the `read_file` slice or \
                                            `search_text.file_revisions[path]` used to construct \
                                            this edit. Refuses stale reads.",
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Default false. Set true to replace every occurrence, \
                                            e.g. renaming a symbol.",
                        },
                    },
                    "required": ["path", "old_string", "new_string", "expected_sha256"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_lines",
                "description": "Replace an inclusive line range in an existing file, guarded by \
                                a strong 32-to-64-character prefix of the byte-exact \
                                `content_sha256` from `read_file` or `search_text`. \
                                This is the preferred editing tool after `search_text`: copy its \
                                1-based line number into `start_line`/`end_line`, supply only the \
                                replacement text, and Kronn refuses if any byte changed since the \
                                read. You do not copy an `old_string`, so indentation cannot select \
                                the wrong nesting level. Newlines in `new_string` are converted to \
                                the file's existing LF or CRLF style. An empty replacement deletes \
                                the selected lines. A `.rs` proposal is syntax-parsed before any \
                                write; on refusal, use the unchanged receipt for one bounded repair \
                                from the exact parser error, then hand off to a stronger worker.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the workspace root."
                        },
                        "start_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "First line to replace, 1-based and inclusive."
                        },
                        "end_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Last line to replace, 1-based and inclusive."
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement text. May be empty to delete the range."
                        },
                        "expected_sha256": {
                            "type": "string",
                            "minLength": 32,
                            "maxLength": 64,
                            "description": "The first 32 to 64 hexadecimal characters of the \
                                            `content_sha256` returned by the read/search used to \
                                            choose this line range."
                        }
                    },
                    "required": ["path", "start_line", "end_line", "new_string", "expected_sha256"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "insert_after_line",
                "description": "Insert text immediately after one existing 1-based anchor line, guarded by a strong 32-to-64-character prefix of the byte-exact `content_sha256` from `read_file`. The anchor bytes are preserved mechanically: this tool has no replacement or deletion argument. Newlines in `new_string` are converted to the file's existing LF or CRLF style. Empty insertions, stale receipts and out-of-range anchors are refused without writing. A `.rs` proposal is syntax-parsed before any durable write.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to the workspace root."
                        },
                        "anchor_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Existing line after which the new text is inserted, 1-based."
                        },
                        "new_string": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Text to insert. It cannot replace or delete the anchor."
                        },
                        "expected_sha256": {
                            "type": "string",
                            "minLength": 32,
                            "maxLength": 64,
                            "description": "The first 32 to 64 hexadecimal characters of the `content_sha256` returned by the authoritative read."
                        }
                    },
                    "required": ["path", "anchor_line", "new_string", "expected_sha256"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List entries of a directory in this discussion's workspace \
                                (non-recursive). Call it to discover paths before read_file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory relative to the workspace root; omit for the root.",
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Walk sub-directories too (bounded; build and vendor \
                                            directories are skipped). Each entry carries its size, \
                                            and `truncated` tells you when the walk was cut short.",
                        },
                    },
                    "required": [],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "find_files",
                "description": "Find files in this discussion's workspace by glob pattern, e.g. \
                                `**/*.rs` or `src/**/*.ts`. One call instead of one listing per \
                                directory. Returns each match with its size; `truncated` tells you \
                                when the search hit its bound. Build and vendor directories are \
                                skipped.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob relative to the workspace root. `*` stays within a \
                                            path segment, `**` crosses segments.",
                        },
                    },
                    "required": ["pattern"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "search_text",
                "description": "Search the CONTENT of files in this discussion's workspace for a \
                                literal string — the way to locate a function, a constant or a \
                                call site without reading whole files. Prefer it to paging a large \
                                file with `read_file`: one call tells you the path and line, and \
                                you then read only that region. Returns each hit as `path`, `line` \
                                and the matched line itself, exact (indentation included) unless \
                                `text_truncated` is true. Long lines are visibly capped with `…`. \
                                `file_revisions[path]` is the byte-exact receipt required by \
                                `edit_lines`, `edit_file`, or a whole-file `write_file`, so an \
                                untruncated returned line can serve as an \
                                anchor without allowing a stale write. `truncated` \
                                means there were more hits than reported, so narrow the query; \
                                `walk_truncated` means part of the tree was not reached. Binary \
                                and very large files, build and vendor directories are skipped.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Literal text to find. Not a regular expression: \
                                            `fn reclaim_preserved` matches exactly those characters.",
                        },
                        "path_glob": {
                            "type": "string",
                            "description": "Optional glob restricting where to search, e.g. \
                                            `backend/src/**/*.rs`. Omit to search everywhere.",
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Default false, which is what you want for prose. \
                                            Set true when the case carries meaning, e.g. a symbol.",
                        },
                    },
                    "required": ["query"],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_status",
                "description": "Current branch and the list of changed files in this workspace. \
                                Call it before reviewing work so you know WHAT changed instead of \
                                asking for it. Read-only.",
                "parameters": { "type": "object", "properties": {}, "required": [] },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_diff",
                "description": "The diff of this workspace. Without arguments: the uncommitted \
                                work, which is what 'review my current changes' means. Pass \
                                `revision_range` (e.g. `main...HEAD`) to compare branches, and/or \
                                `path` to narrow to one file. `truncated` tells you when the diff \
                                was too large to return whole — say so rather than concluding from \
                                a partial diff. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "revision_range": {
                            "type": "string",
                            "description": "A git revision or range, e.g. `main...HEAD` or a sha.",
                        },
                        "path": {
                            "type": "string",
                            "description": "Limit the diff to this path, relative to the workspace root.",
                        },
                    },
                    "required": [],
                },
            },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_log",
                "description": "Recent commits (sha, date, author, subject) to situate the work. \
                                 `limit` defaults to 20, capped at 100. `path` optionally restricts \
                                 the history to the commits touching that path (resolved inside the \
                                 workspace; a path that escapes is refused). Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "How many commits (1-100)." },
                         "path": { "type": "string", "description": "Optional path to restrict the history to the commits touching it." },
                    },
                    "required": [],
                    },
                    },
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_commit",
                "description": "Commit explicitly named changed files in the managed task \
                                worktree. Use this after edits and before task_exec_deliver. \
                                Kronn refuses this tool outside the exact active task execution; \
                                it cannot amend, checkout or push.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "maxItems": 100,
                            "description": "Changed paths relative to the task worktree root.",
                        },
                        "message": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 500,
                            "description": "Commit message.",
                        },
                    },
                    "required": ["files", "message"],
                },
            },
        }),
    ]
}

/// Names contributed here, so the catalogue gate can recognise them.
pub const TOOL_NAMES: &[&str] = &[
    "web_fetch",
    "read_file",
    "write_file",
    "edit_file",
    "edit_lines",
    "insert_after_line",
    "list_files",
    "find_files",
    "search_text",
    "git_status",
    "git_diff",
    "git_log",
    "git_commit",
];

/// Read a workspace file, optionally one slice of it.
///
/// KT-399 — the byte cap alone is blind to who is reading. Kronn's own
/// `orchestration.rs` is 449 KB; served whole to a local model with a 32k
/// window, a single call buries the conversation and everything after it is the
/// model flailing in a full context. `offset`/`limit` let a reader take the file
/// in slices, and `total_lines`/`next_offset` tell it whether more remains
/// rather than leaving it to guess.
///
/// Both are optional and absent by default: a large-context model that reads the
/// whole file in one call behaves exactly as before.
pub fn read_file_payload(
    root: &Path,
    requested: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, String> {
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        // Repository discovery legitimately probes several conventional paths.
        // An absent candidate is evidence the model can use, not a broken tool;
        // keeping it successful also prevents those probes from opening the
        // runner's per-tool failure circuit. Every other I/O failure remains an
        // error so permissions and unexpected filesystem state stay visible.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({
                "path": requested,
                "found": false,
                "truncated": false,
                "bytes_returned": 0,
                "text": "",
                "note": "file does not exist",
            }));
        }
        Err(error) => return Err(format!("could not read `{requested}`: {error}")),
    };
    // The receipt covers the exact bytes whose decoded view follows. It is a
    // file revision, not a hash of the returned slice: any concurrent change
    // anywhere in the file must make a later edit refuse.
    let revision = content_sha256(&bytes);
    // Lossy on purpose: a binary file must produce a readable refusal-ish payload
    // rather than an error the model cannot interpret.
    let whole = String::from_utf8_lossy(&bytes).to_string();
    let total_lines = whole.lines().count();

    // A slice is requested when either bound is given. `offset` is 1-based
    // because that is what the reader sees in an editor and in every diff.
    let sliced = offset.is_some() || limit.is_some();
    let start = offset.unwrap_or(1).max(1) - 1;
    let text = if sliced {
        // `lines().join("\n")` silently normalised CRLF and made a supposedly
        // byte-exact copy from read_file fail as an edit anchor. Inclusive
        // splitting preserves the file's own line endings verbatim.
        whole
            .split_inclusive('\n')
            .skip(start)
            .take(limit.unwrap_or(usize::MAX))
            .collect::<String>()
    } else {
        whole
    };

    // The byte cap still applies, to the slice as well: a caller can ask for
    // 10 000 lines and hit it just the same.
    let truncated = text.len() > MAX_BYTES;
    let text = if truncated {
        // Cut on a char boundary — a mid-codepoint split would corrupt the tail.
        let mut cut = MAX_BYTES;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text[..cut].to_string()
    } else {
        text
    };

    let lines_returned = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    // Where to continue, or null when there is nothing left. Saying it costs one
    // field and saves the reader from probing for the end.
    let next_offset = match start + lines_returned {
        consumed if consumed < total_lines => Some(consumed + 1),
        _ => None,
    };

    Ok(json!({
        "path": requested,
        "found": true,
        "content_sha256": revision,
        "truncated": truncated,
        "bytes_returned": text.len(),
        "total_lines": total_lines,
        "lines_returned": lines_returned,
        "next_offset": next_offset,
        "text": text,
    }))
}

/// Create a file without overwriting an existing path.
///
/// Most internal callers use this helper to prepare fixtures. The tool executor
/// calls [`write_file_payload_with_receipt`] directly so an agent may replace a
/// file only under an exact revision receipt.
pub fn write_file_payload(root: &Path, requested: &str, content: &str) -> Result<Value, String> {
    write_file_payload_inner(root, requested, content, None)
}

/// Create a new file atomically, or replace an existing one under a byte-exact
/// optimistic-concurrency receipt. This closes the escape hatch where a model
/// could bypass `edit_file`/`edit_lines` CAS simply by choosing `write_file`.
pub fn write_file_payload_with_receipt(
    root: &Path,
    requested: &str,
    content: &str,
    expected_sha256: Option<&str>,
) -> Result<Value, String> {
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    validate_proposed_source(&path, requested, content)?;
    write_file_payload_inner(root, requested, content, expected_sha256)
}

/// Fixture/internal creation primitive. Runtime agents always enter through
/// [`write_file_payload_with_receipt`], which applies the source guard first.
fn write_file_payload_inner(
    root: &Path,
    requested: &str,
    content: &str,
    expected_sha256: Option<&str>,
) -> Result<Value, String> {
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create `{}`: {error}", parent.display()))?;
    }

    let (created, previous_sha256) = if let Some(expected_sha256) = expected_sha256 {
        if !sha256_receipt_is_well_formed(expected_sha256) {
            return Err(invalid_sha256_receipt_message());
        }
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|_| {
                format!(
                    "`{requested}` does not exist, so an overwrite receipt cannot apply. \
                     Retry without `expected_sha256` to create it."
                )
            })?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
            format!(
                "`{requested}` is already being edited by another worker. Re-read it after that edit finishes."
            )
        })?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("could not inspect `{requested}`: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("`{requested}` is not a regular file."));
        }
        if metadata.len() > MAX_EDIT_FILE_BYTES || content.len() as u64 > MAX_EDIT_FILE_BYTES {
            return Err(format!(
                "`{requested}` exceeds the {MAX_EDIT_FILE_BYTES}-byte whole-file write ceiling. \
                 Change the generator or use a bounded edit instead."
            ));
        }
        let mut before = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut before)
            .map_err(|error| format!("could not read `{requested}`: {error}"))?;
        if before.contains(&0) {
            return Err(format!(
                "`{requested}` is binary; refusing a whole-file text replacement."
            ));
        }
        let actual_sha256 = content_sha256(&before);
        if !sha256_receipt_matches(&actual_sha256, expected_sha256) {
            return Err(format!(
                "`{requested}` changed after the read used for this replacement (expected \
                 {expected_sha256}, current {actual_sha256}). Nothing was written. Re-read it \
                 and decide against the current bytes."
            ));
        }
        file.seek(std::io::SeekFrom::Start(0))
            .map_err(|error| format!("could not seek `{requested}` for writing: {error}"))?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("could not write `{requested}`: {error}"))?;
        file.set_len(content.len() as u64)
            .map_err(|error| format!("could not truncate `{requested}` after writing: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("could not flush `{requested}` after writing: {error}"))?;
        (false, Some(actual_sha256))
    } else {
        if content.len() as u64 > MAX_EDIT_FILE_BYTES {
            return Err(format!(
                "`{requested}` exceeds the {MAX_EDIT_FILE_BYTES}-byte whole-file write ceiling."
            ));
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if path.exists() {
                    format!(
                        "`{requested}` already exists. Nothing was written. Read it first, then \
                         pass its `content_sha256` to replace it, or use `edit_lines` for a \
                         bounded change."
                    )
                } else {
                    format!("could not create `{requested}`: {error}")
                }
            })?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("could not write `{requested}`: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("could not flush `{requested}` after writing: {error}"))?;
        (true, None)
    };
    let next_sha256 = content_sha256(content.as_bytes());
    Ok(json!({
        "path": requested,
        "bytes_written": content.len(),
        "created": created,
        "overwritten": !created,
        "previous_sha256": previous_sha256,
        "content_sha256": next_sha256,
    }))
}

/// Replace one anchored region of an existing file.
///
/// This tool is the missing link that ten measured delegations failed on. The
/// only mutation primitive was `write_file`, which takes the ENTIRE file — so
/// to change one function of an 11 426-line file, a worker reading 120-line
/// slices under a 24-read budget had to reproduce ~8 500 lines it could never
/// read. Every generation analysed the code correctly, announced the edit, and
/// then stalled or paged: the last step was architecturally impossible.
///
/// `old_string` must match byte for byte — indentation included — and appear
/// exactly once, so the model proves it read the region before changing it.
/// Zero matches and ambiguity are distinct errors, because they are fixed
/// differently: one by re-reading, the other by widening the anchor.
pub fn edit_file_payload(
    root: &Path,
    requested: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    expected_sha256: &str,
) -> Result<Value, String> {
    if old_string.is_empty() {
        return Err(
            "`old_string` is empty. Copy the exact text to replace from `read_file`.".into(),
        );
    }
    if old_string == new_string {
        return Err("`old_string` and `new_string` are identical — nothing would change.".into());
    }
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    if !sha256_receipt_is_well_formed(expected_sha256) {
        return Err(invalid_sha256_receipt_message());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|_| format!("`{requested}` does not exist. `edit_file` never creates a file; use `write_file` for a new one."))?;
    // Avoid two Kronn HTTP workers editing the same inode concurrently. The
    // hash below is still the authoritative CAS guard; the lock only closes
    // the check/write gap between cooperating tool calls.
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        format!(
            "`{requested}` is already being edited by another worker. Re-read it after that edit finishes."
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect `{requested}`: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("`{requested}` is not a regular file."));
    }
    if metadata.len() > MAX_EDIT_FILE_BYTES {
        return Err(format!(
            "`{requested}` is {} bytes, past the {MAX_EDIT_FILE_BYTES}-byte edit ceiling. \
             A text file that size is generated; change its generator instead.",
            metadata.len()
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut body)
        .map_err(|error| format!("could not read `{requested}`: {error}"))?;
    if body.contains(&0) {
        return Err(format!(
            "`{requested}` is binary; there is no text in it to anchor an edit."
        ));
    }
    let actual_sha256 = content_sha256(&body);
    if !sha256_receipt_matches(&actual_sha256, expected_sha256) {
        return Err(format!(
            "`{requested}` changed after the read used for this edit (expected {expected_sha256}, current {actual_sha256}). Nothing was written. Re-read the region and decide against the current bytes."
        ));
    }
    let text = String::from_utf8(body)
        .map_err(|_| format!("`{requested}` is not valid UTF-8; refusing a lossy rewrite."))?;

    let must_start_line = old_string
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'));
    let matches = text
        .match_indices(old_string)
        .map(|(offset, _)| offset)
        .filter(|offset| {
            !must_start_line || *offset == 0 || text.as_bytes().get(offset - 1) == Some(&b'\n')
        })
        .collect::<Vec<_>>();
    let occurrences = matches.len();
    if occurrences > 1 && !replace_all {
        return Err(format!(
            "`old_string` appears {occurrences} times; an edit must be unambiguous. \
             Include more surrounding lines so it matches exactly once, or pass \
             `replace_all: true` to change every occurrence."
        ));
    }
    if occurrences == 0 {
        return Err(not_found_error());
    }

    // 1-based line of the first change, so the model can re-read just that
    // region to verify instead of paging the file again.
    let first_changed_line = text[..matches[0]]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let selected = if replace_all {
        &matches[..]
    } else {
        &matches[..1]
    };
    let mut replaced = String::with_capacity(text.len());
    let mut cursor = 0;
    for offset in selected {
        replaced.push_str(&text[cursor..*offset]);
        replaced.push_str(new_string);
        cursor = *offset + old_string.len();
    }
    replaced.push_str(&text[cursor..]);
    let total_lines = replaced.lines().count();
    validate_proposed_source(&path, requested, &replaced)?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| format!("could not seek `{requested}` for editing: {error}"))?;
    file.write_all(replaced.as_bytes())
        .map_err(|error| format!("could not write `{requested}`: {error}"))?;
    file.set_len(replaced.len() as u64)
        .map_err(|error| format!("could not truncate `{requested}` after editing: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("could not flush `{requested}` after editing: {error}"))?;
    let next_sha256 = content_sha256(replaced.as_bytes());
    Ok(json!({
        "path": requested,
        "match": "exact",
        "previous_sha256": actual_sha256,
        "content_sha256": next_sha256,
        "replacements": if replace_all { occurrences } else { 1 },
        "first_changed_line": first_changed_line,
        "total_lines": total_lines,
    }))
}

/// Replace a 1-based inclusive line range under a whole-file revision receipt.
/// The range is the target identity; no whitespace or substring heuristic is
/// involved, and the hash refuses line-number drift between read and write.
pub fn edit_lines_payload(
    root: &Path,
    requested: &str,
    start_line: usize,
    end_line: usize,
    new_string: &str,
    expected_sha256: &str,
) -> Result<Value, String> {
    if start_line == 0 || end_line < start_line {
        return Err(
            "`start_line` and `end_line` must form a non-empty 1-based inclusive range.".into(),
        );
    }
    if !sha256_receipt_is_well_formed(expected_sha256) {
        return Err(invalid_sha256_receipt_message());
    }
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|_| format!("`{requested}` does not exist. `edit_lines` never creates a file; use `write_file` for a new one."))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        format!(
            "`{requested}` is already being edited by another worker. Re-read it after that edit finishes."
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect `{requested}`: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("`{requested}` is not a regular file."));
    }
    if metadata.len() > MAX_EDIT_FILE_BYTES {
        return Err(format!(
            "`{requested}` is {} bytes, past the {MAX_EDIT_FILE_BYTES}-byte edit ceiling. \
             A text file that size is generated; change its generator instead.",
            metadata.len()
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut body)
        .map_err(|error| format!("could not read `{requested}`: {error}"))?;
    if body.contains(&0) {
        return Err(format!(
            "`{requested}` is binary; there are no text lines to edit."
        ));
    }
    let actual_sha256 = content_sha256(&body);
    if !sha256_receipt_matches(&actual_sha256, expected_sha256) {
        return Err(format!(
            "`{requested}` changed after the read used for this edit (expected {expected_sha256}, current {actual_sha256}). Nothing was written. Re-read and choose the range against the current bytes."
        ));
    }
    let text = String::from_utf8(body)
        .map_err(|_| format!("`{requested}` is not valid UTF-8; refusing a lossy rewrite."))?;
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if end_line > lines.len() {
        return Err(format!(
            "line range {start_line}..={end_line} exceeds `{requested}` ({total} lines). Nothing was written.",
            total = lines.len()
        ));
    }
    let region_start = lines[..start_line - 1]
        .iter()
        .map(|line| line.len())
        .sum::<usize>();
    let region_end = region_start
        + lines[start_line - 1..end_line]
            .iter()
            .map(|line| line.len())
            .sum::<usize>();
    let selected_had_newline = lines[end_line - 1].ends_with('\n');
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut replacement = new_string.replace("\r\n", "\n");
    if line_ending == "\r\n" {
        replacement = replacement.replace('\n', "\r\n");
    }
    if selected_had_newline && !replacement.is_empty() && !replacement.ends_with(line_ending) {
        replacement.push_str(line_ending);
    }
    if text[region_start..region_end] == replacement {
        return Err("the selected lines already equal `new_string` — nothing would change.".into());
    }

    let mut replaced =
        String::with_capacity(text.len() - (region_end - region_start) + replacement.len());
    replaced.push_str(&text[..region_start]);
    replaced.push_str(&replacement);
    replaced.push_str(&text[region_end..]);
    validate_proposed_source(&path, requested, &replaced)?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| format!("could not seek `{requested}` for editing: {error}"))?;
    file.write_all(replaced.as_bytes())
        .map_err(|error| format!("could not write `{requested}`: {error}"))?;
    file.set_len(replaced.len() as u64)
        .map_err(|error| format!("could not truncate `{requested}` after editing: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("could not flush `{requested}` after editing: {error}"))?;
    Ok(json!({
        "path": requested,
        "match": "line_range_cas",
        "start_line": start_line,
        "end_line": end_line,
        "previous_sha256": actual_sha256,
        "content_sha256": content_sha256(replaced.as_bytes()),
        "line_ending": if line_ending == "\r\n" { "crlf" } else { "lf" },
        "total_lines": replaced.lines().count(),
    }))
}

/// Insert text after one existing line while preserving that line byte for
/// byte. Unlike `edit_lines`, the caller never supplies replacement bytes for
/// the anchor, so a small/local worker cannot accidentally delete it.
pub fn insert_after_line_payload(
    root: &Path,
    requested: &str,
    anchor_line: usize,
    new_string: &str,
    expected_sha256: &str,
) -> Result<Value, String> {
    if anchor_line == 0 {
        return Err("`anchor_line` must be a positive 1-based line number.".into());
    }
    if new_string.is_empty() {
        return Err("`new_string` must be non-empty for an insertion.".into());
    }
    if !sha256_receipt_is_well_formed(expected_sha256) {
        return Err(invalid_sha256_receipt_message());
    }
    let path = resolve_in_workspace(root, requested).map_err(|refusal| refusal.message())?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|_| {
            format!("`{requested}` does not exist. `insert_after_line` never creates a file.")
        })?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        format!(
            "`{requested}` is already being edited by another worker. Re-read it after that edit finishes."
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect `{requested}`: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("`{requested}` is not a regular file."));
    }
    if metadata.len() > MAX_EDIT_FILE_BYTES {
        return Err(format!(
            "`{requested}` is {} bytes, past the {MAX_EDIT_FILE_BYTES}-byte edit ceiling. A text file that size is generated; change its generator instead.",
            metadata.len()
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut body)
        .map_err(|error| format!("could not read `{requested}`: {error}"))?;
    if body.contains(&0) {
        return Err(format!(
            "`{requested}` is binary; there are no text lines to edit."
        ));
    }
    let actual_sha256 = content_sha256(&body);
    if !sha256_receipt_matches(&actual_sha256, expected_sha256) {
        return Err(format!(
            "`{requested}` changed after the read used for this insertion (expected {expected_sha256}, current {actual_sha256}). Nothing was written. Re-read the frozen anchor against the current bytes."
        ));
    }
    let text = String::from_utf8(body)
        .map_err(|_| format!("`{requested}` is not valid UTF-8; refusing a lossy rewrite."))?;
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if anchor_line > lines.len() {
        return Err(format!(
            "anchor line {anchor_line} exceeds `{requested}` ({total} lines). Nothing was written.",
            total = lines.len()
        ));
    }
    let insertion_at = lines[..anchor_line]
        .iter()
        .map(|line| line.len())
        .sum::<usize>();
    let anchor_has_newline = lines[anchor_line - 1].ends_with('\n');
    let has_following_bytes = insertion_at < text.len();
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut inserted = new_string.replace("\r\n", "\n");
    if line_ending == "\r\n" {
        inserted = inserted.replace('\n', "\r\n");
    }
    if !anchor_has_newline {
        inserted.insert_str(0, line_ending);
    }
    if (anchor_has_newline || has_following_bytes) && !inserted.ends_with(line_ending) {
        inserted.push_str(line_ending);
    }

    let mut replaced = String::with_capacity(text.len() + inserted.len());
    replaced.push_str(&text[..insertion_at]);
    replaced.push_str(&inserted);
    replaced.push_str(&text[insertion_at..]);
    validate_proposed_source(&path, requested, &replaced)?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| format!("could not seek `{requested}` for editing: {error}"))?;
    file.write_all(replaced.as_bytes())
        .map_err(|error| format!("could not write `{requested}`: {error}"))?;
    file.set_len(replaced.len() as u64)
        .map_err(|error| format!("could not truncate `{requested}` after editing: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("could not flush `{requested}` after editing: {error}"))?;
    Ok(json!({
        "path": requested,
        "match": "insert_after_line_cas",
        "anchor_line": anchor_line,
        "anchor_preserved": true,
        "previous_sha256": actual_sha256,
        "content_sha256": content_sha256(replaced.as_bytes()),
        "line_ending": if line_ending == "\r\n" { "crlf" } else { "lf" },
        "total_lines": replaced.lines().count(),
    }))
}

fn not_found_error() -> String {
    "`old_string` was not found byte for byte. Nothing was written: Kronn never \
     guesses an edit by ignoring indentation. Copy it from a `read_file` slice \
     (or an untruncated `search_text` line) and re-read the region if needed."
        .to_string()
}

/// Walk `dir` breadth-first, bounded by depth, entry count and the skip list.
/// Returns the relative paths found and whether the walk was cut short — the
/// caller MUST surface that, otherwise the model concludes from a partial tree.
fn walk_bounded(root: &Path, start: &Path, max_depth: usize) -> (Vec<Value>, bool) {
    // `start` comes back canonicalised from `resolve_in_workspace` while `root`
    // is whatever the caller holds. On macOS those differ whenever the path
    // crosses a symlink (`/var` → `/private/var`), and then `strip_prefix(root)`
    // fails and every entry reports an ABSOLUTE path — which no relative glob
    // can match, so a targeted search answers "nothing found". Strip against
    // both spellings so the result is root-relative either way.
    let canonical_root = std::fs::canonicalize(root).ok();
    let relative_to_root = |path: &Path| -> String {
        path.strip_prefix(root)
            .ok()
            .or_else(|| {
                canonical_root
                    .as_deref()
                    .and_then(|base| path.strip_prefix(base).ok())
            })
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    };
    let mut out: Vec<Value> = Vec::new();
    let mut queue: Vec<(PathBuf, usize)> = vec![(start.to_path_buf(), 0)];
    let mut truncated = false;
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_WALK_ENTRIES {
                truncated = true;
                return (out, truncated);
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let path = entry.path();
            let rel = relative_to_root(&path);
            // `size` is what makes "which file is big" answerable in ONE call
            // instead of one read per file.
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(json!({ "path": rel, "is_dir": is_dir, "size": size }));
            if is_dir {
                if depth + 1 < max_depth {
                    queue.push((path, depth + 1));
                } else {
                    truncated = true;
                }
            }
        }
    }
    (out, truncated)
}

/// Match a path against a simple glob: `*` matches within a segment, `**` spans
/// segments. Deliberately not a full glob crate — this covers `**/*.rs` and
/// `src/**` , which is what a model asks for, with no new dependency.
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn inner(pat: &[u8], txt: &[u8]) -> bool {
        if pat.is_empty() {
            return txt.is_empty();
        }
        if pat[0] == b'*' {
            // `**` crosses `/`; a single `*` stops at a separator.
            let double = pat.len() > 1 && pat[1] == b'*';
            let rest = if double { &pat[2..] } else { &pat[1..] };
            let rest = if double && !rest.is_empty() && rest[0] == b'/' {
                &rest[1..]
            } else {
                rest
            };
            for skip in 0..=txt.len() {
                if !double && txt[..skip].contains(&b'/') {
                    break;
                }
                if inner(rest, &txt[skip..]) {
                    return true;
                }
            }
            return false;
        }
        if txt.is_empty() || (pat[0] != b'?' && pat[0] != txt[0]) {
            return false;
        }
        inner(&pat[1..], &txt[1..])
    }
    inner(pattern.as_bytes(), path.as_bytes())
}

/// Find files by glob inside the workspace. This is the tool that makes the file
/// catalogue usable: without it, a model had to spend one turn per directory and
/// gave up before finding anything (observed in production).
/// The literal directory prefix of a glob, i.e. everything before the first
/// wildcard segment. `backend/src/**/*.rs` starts at `backend/src`, which is the
/// difference between walking one subtree and walking the whole repository.
fn glob_literal_prefix(pattern: &str) -> String {
    let mut prefix = Vec::new();
    for segment in pattern.split('/') {
        if segment.contains('*') || segment.contains('?') {
            break;
        }
        prefix.push(segment);
    }
    // The last literal segment may be the file itself (`src/main.rs`); only keep
    // segments we are sure are directories by dropping a trailing one that carries
    // an extension.
    if let Some(last) = prefix.last() {
        if last.contains('.') {
            prefix.pop();
        }
    }
    prefix.join("/")
}

/// Search file CONTENT across the workspace.
///
/// Its absence was measured, not guessed: asked to change one function in a
/// 10 000-line file, a worker had `find_files` (names only) and `read_file`
/// (one slice at a time) and nothing else, so it paged the file 24 times until
/// the read budget refused it — never reaching the function. Searching by name
/// is not searching.
///
/// Literal substring, never a regex: a wrong regex from a weaker model either
/// matches nothing or backtracks, and both failures look like "the code is not
/// there".
pub fn search_text_payload(
    root: &Path,
    query: &str,
    path_glob: Option<&str>,
    case_sensitive: bool,
) -> Result<Value, String> {
    if query.is_empty() {
        return Err("search_text needs a non-empty `query`.".into());
    }
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };

    // Start where the glob says to start, exactly as find_files does: walking
    // from the root and filtering afterwards is what made a targeted search
    // answer "nothing found" on this very repository.
    let start = match path_glob.map(glob_literal_prefix).filter(|p| !p.is_empty()) {
        Some(prefix) => match resolve_in_workspace(root, &prefix) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => root.to_path_buf(),
            Err(refusal) => return Err(refusal.message()),
        },
        None => root.to_path_buf(),
    };
    let (entries, walk_truncated) = walk_bounded(root, &start, MAX_WALK_DEPTH);

    let mut matches: Vec<Value> = Vec::new();
    let mut file_revisions: BTreeMap<String, String> = BTreeMap::new();
    let mut files_with_matches = 0usize;
    let mut files_searched = 0usize;
    let mut capped = false;

    for entry in entries {
        if matches.len() >= MAX_SEARCH_MATCHES {
            capped = true;
            break;
        }
        if entry["is_dir"] != json!(false) {
            continue;
        }
        let Some(relative) = entry["path"].as_str() else {
            continue;
        };
        if let Some(pattern) = path_glob {
            if !glob_matches(pattern, relative) {
                continue;
            }
        }
        if entry["size"].as_u64().unwrap_or_default() > MAX_SEARCH_FILE_BYTES {
            continue;
        }
        let Ok(body) = std::fs::read(root.join(relative)) else {
            // Unreadable is not a failure of the search: it is one file the
            // answer does not cover, and the walk already told us it exists.
            continue;
        };
        if body.contains(&0) {
            continue; // Binary. A byte match in it is not a line the model can read.
        }
        let revision = content_sha256(&body);
        let Ok(text) = String::from_utf8(body) else {
            continue;
        };

        // A file that passed every filter above is genuinely opened and scanned,
        // so it counts toward files_searched. Files skipped for being binary,
        // too large, or unreadable do not.
        files_searched += 1;

        let mut in_this_file = 0usize;
        for (index, line) in text.lines().enumerate() {
            let hay = if case_sensitive {
                line.to_string()
            } else {
                line.to_lowercase()
            };
            if !hay.contains(&needle) {
                continue;
            }
            if in_this_file >= MAX_SEARCH_MATCHES_PER_FILE || matches.len() >= MAX_SEARCH_MATCHES {
                capped = true;
                break;
            }
            let text_truncated = line.chars().count() > MAX_SEARCH_LINE_CHARS;
            let shown: String = if text_truncated {
                line.chars().take(MAX_SEARCH_LINE_CHARS).collect::<String>() + "…"
            } else {
                line.to_string()
            };
            matches.push(json!({
                "path": relative,
                "line": index + 1,
                "text": shown,
                "text_truncated": text_truncated,
            }));
            in_this_file += 1;
        }
        if in_this_file > 0 {
            files_with_matches += 1;
            file_revisions.insert(relative.to_string(), revision);
        }
    }

    Ok(json!({
        "query": query,
        "case_sensitive": case_sensitive,
        "path_glob": path_glob,
        "count": matches.len(),
        "files_with_matches": files_with_matches,
         "files_searched": files_searched,
        // Two different partial answers, and the model must be able to tell them
        // apart: too many hits means refine the query, an exhausted walk means
        // the search never reached part of the tree.
        "truncated": capped,
        "walk_truncated": walk_truncated,
        // One revision per matching file, rather than repeating 64 hex chars on
        // every hit. Pass this receipt to edit_file.expected_sha256.
        "file_revisions": file_revisions,
        "matches": matches,
    }))
}

pub fn find_files_payload(root: &Path, pattern: &str) -> Result<Value, String> {
    // Start where the pattern says to start. Walking from the root and filtering
    // afterwards is what made this tool answer "no matches" on a large repo.
    let prefix = glob_literal_prefix(pattern);
    let start = if prefix.is_empty() {
        root.to_path_buf()
    } else {
        match resolve_in_workspace(root, &prefix) {
            Ok(path) if path.is_dir() => path,
            // A prefix that does not resolve is not an error: the glob simply
            // matches nothing, and saying so beats a refusal about paths.
            Ok(_) => root.to_path_buf(),
            Err(refusal) => return Err(refusal.message()),
        }
    };
    let (entries, truncated) = walk_bounded(root, &start, MAX_WALK_DEPTH);
    let mut matches: Vec<Value> = entries
        .into_iter()
        .filter(|e| {
            e["is_dir"] == json!(false)
                && e["path"].as_str().is_some_and(|p| glob_matches(pattern, p))
        })
        .collect();
    matches.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(json!({
        "pattern": pattern,
        "truncated": truncated,
        "count": matches.len(),
        "files": matches,
    }))
}

pub fn list_files_payload(
    root: &Path,
    requested: Option<&str>,
    recursive: bool,
) -> Result<Value, String> {
    let dir = match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(rel) => resolve_in_workspace(root, rel).map_err(|refusal| refusal.message())?,
        None => root.to_path_buf(),
    };
    if recursive {
        let (mut entries, truncated) = walk_bounded(root, &dir, MAX_WALK_DEPTH);
        entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        return Ok(json!({
            "path": requested.unwrap_or(""),
            "recursive": true,
            "truncated": truncated,
            "count": entries.len(),
            "entries": entries,
        }));
    }
    let mut entries: Vec<Value> = Vec::new();
    let read = std::fs::read_dir(&dir)
        .map_err(|error| format!("could not list `{}`: {error}", dir.display()))?;
    for entry in read.flatten() {
        let file_type = entry.file_type().ok();
        entries.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "is_dir": file_type.map(|t| t.is_dir()).unwrap_or(false),
            // Size here too: "which file is biggest" is a common ask and it should
            // not require one read per file.
            "size": entry.metadata().map(|m| m.len()).unwrap_or(0),
        }));
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({
        "path": requested.unwrap_or(""),
        "recursive": false,
        "entries": entries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn addr(raw: &str) -> std::net::IpAddr {
        raw.parse().unwrap()
    }

    fn revision(root: &Path, requested: &str) -> String {
        content_sha256(&std::fs::read(root.join(requested)).unwrap())
    }

    #[test]
    fn private_and_loopback_addresses_are_refused() {
        // The server issues the request, so these would otherwise be reachable
        // through Kronn by a model that cannot reach them itself.
        for private in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.10",
            "172.16.0.1",
            "169.254.169.254", // cloud metadata — the canonical SSRF target
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!is_public_addr(&addr(private)), "{private} must be refused");
        }
        for public in ["1.1.1.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(is_public_addr(&addr(public)), "{public} must be allowed");
        }
    }

    #[tokio::test]
    async fn only_http_schemes_are_fetchable() {
        for bad in [
            "file:///etc/passwd",
            "data:text/plain,hello",
            "ftp://example.com/x",
            "not a url",
        ] {
            let refusal = check_fetch_url(bad).await.unwrap_err();
            assert!(
                matches!(refusal, Refusal::UnsupportedScheme(_)),
                "{bad} → {refusal:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_literal_private_host_is_refused_before_any_request() {
        let refusal = check_fetch_url("http://169.254.169.254/latest/meta-data")
            .await
            .unwrap_err();
        assert!(matches!(refusal, Refusal::PrivateAddress(_)));
        let refusal = check_fetch_url("http://localhost:8080/admin")
            .await
            .unwrap_err();
        assert!(matches!(refusal, Refusal::PrivateAddress(_)));
    }

    #[tokio::test]
    async fn web_fetch_reads_a_mocked_success_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain; charset=utf-8")
                    .set_body_string("hello from the web"),
            )
            .mount(&server)
            .await;
        let url = reqwest::Url::parse(&format!("{}/page", server.uri())).unwrap();

        let payload = fetch_text_with_client(&reqwest::Client::new(), url)
            .await
            .unwrap();

        assert_eq!(payload["status"], json!(200));
        assert_eq!(payload["text"], json!("hello from the web"));
        assert_eq!(payload["truncated"], json!(false));
    }

    #[tokio::test]
    async fn web_fetch_timeout_is_bounded_and_readable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(250))
                    .set_body_string("too late"),
            )
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(25))
            .build()
            .unwrap();
        let url = reqwest::Url::parse(&format!("{}/slow", server.uri())).unwrap();

        let error = fetch_text_with_client(&client, url).await.unwrap_err();

        assert!(error.contains("fetch failed"), "{error}");
    }

    #[tokio::test]
    async fn web_fetch_stops_after_the_announced_body_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(MAX_BYTES + 8_192)))
            .mount(&server)
            .await;
        let url = reqwest::Url::parse(&format!("{}/large", server.uri())).unwrap();

        let payload = fetch_text_with_client(&reqwest::Client::new(), url)
            .await
            .unwrap();

        assert_eq!(payload["truncated"], json!(true));
        assert_eq!(payload["bytes_returned"], json!(MAX_BYTES));
        assert_eq!(payload["text"].as_str().unwrap().len(), MAX_BYTES);
    }

    #[test]
    fn paths_cannot_escape_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        // Inside is fine, including a path whose file does not exist yet (write).
        assert!(resolve_in_workspace(root.path(), "src/main.rs").is_ok());
        assert!(resolve_in_workspace(root.path(), "docs/new/report.md").is_ok());
        // Traversal, absolute paths and a symlink out are all the same refusal.
        for escape in ["../secret", "src/../../secret", "/etc/passwd"] {
            assert_eq!(
                resolve_in_workspace(root.path(), escape),
                Err(Refusal::OutsideWorkspace(escape.to_string())),
                "{escape} must be refused"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_leaving_the_workspace_is_refused() {
        // The textual path looks innocent; only canonicalisation catches it.
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s3cret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(matches!(
            resolve_in_workspace(root.path(), "escape/secret.txt"),
            Err(Refusal::OutsideWorkspace(_))
        ));
    }

    /// The failure ten real delegations shared, stated as a test: changing one
    /// function must never require reproducing the rest of the file.
    #[test]
    fn a_region_is_changed_without_touching_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let before = format!(
            "{}fn reclaim() {{\n    old_body();\n}}\n{}",
            "// head\n".repeat(500),
            "// tail\n".repeat(500)
        );
        write_file_payload(root.path(), "src/big.rs", &before).unwrap();

        let edited = edit_file_payload(
            root.path(),
            "src/big.rs",
            "fn reclaim() {\n    old_body();\n}",
            "fn reclaim() {\n    new_body();\n}",
            false,
            &revision(root.path(), "src/big.rs"),
        )
        .unwrap();
        assert_eq!(edited["replacements"], json!(1));
        assert_eq!(edited["first_changed_line"], json!(501), "{edited}");

        let after = read_file_payload(root.path(), "src/big.rs", None, None).unwrap();
        let text = after["text"].as_str().unwrap();
        assert!(text.contains("new_body();"), "the change landed");
        assert!(!text.contains("old_body();"), "and replaced the old text");
        assert_eq!(
            text.matches("// head").count(),
            500,
            "everything the edit did not name is untouched"
        );
        assert_eq!(text.matches("// tail").count(), 500);
    }

    #[test]
    fn invalid_rust_edits_leave_bytes_and_receipts_untouched_for_one_bounded_repair() {
        let root = tempfile::tempdir().unwrap();
        let original = "fn answer() -> u32 {\n    41\n}\n";
        write_file_payload(root.path(), "src/answer.rs", original).unwrap();
        let receipt = revision(root.path(), "src/answer.rs");

        let refused = edit_file_payload(
            root.path(),
            "src/answer.rs",
            "    41",
            "    if true {",
            false,
            &receipt,
        )
        .unwrap_err();
        assert!(
            refused.contains("Rust syntax validation refused"),
            "{refused}"
        );
        assert!(refused.contains("line 1, column 20"), "{refused}");
        assert!(refused.contains("Nothing was written"), "{refused}");
        assert!(refused.contains("one bounded repair"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("src/answer.rs")).unwrap(),
            original
        );
        assert_eq!(revision(root.path(), "src/answer.rs"), receipt);

        edit_file_payload(
            root.path(),
            "src/answer.rs",
            "    41",
            "    42",
            false,
            &receipt,
        )
        .expect("the unchanged receipt must authorize the bounded repair");
        assert_eq!(
            std::fs::read_to_string(root.path().join("src/answer.rs")).unwrap(),
            "fn answer() -> u32 {\n    42\n}\n"
        );
    }

    #[test]
    fn unresolved_rust_names_remain_a_valid_structural_edit() {
        let result = validate_proposed_source(
            Path::new("answer.rs"),
            "answer.rs",
            "fn answer() -> u32 { UNRESOLVED }",
        );
        assert!(result.is_ok());
    }
    #[test]
    fn rust_comments_with_braces_remain_a_valid_structural_edit() {
        let result = validate_proposed_source(
            Path::new("answer.rs"),
            "answer.rs",
            "fn answer() -> u32 { /* } */ 42 }",
        );
        assert!(result.is_ok());
    }
    #[test]
    fn invalid_rust_line_edits_and_whole_file_writes_never_reach_disk() {
        let root = tempfile::tempdir().unwrap();
        let original = "fn answer() -> u32 {\n    41\n}\n";
        write_file_payload(root.path(), "answer.rs", original).unwrap();
        let receipt = revision(root.path(), "answer.rs");

        let line_error =
            edit_lines_payload(root.path(), "answer.rs", 2, 2, "    if true {", &receipt)
                .unwrap_err();
        assert!(line_error.contains("Rust syntax validation refused"));
        assert_eq!(revision(root.path(), "answer.rs"), receipt);
        assert_eq!(
            std::fs::read_to_string(root.path().join("answer.rs")).unwrap(),
            original
        );

        let create_error =
            write_file_payload_with_receipt(root.path(), "new.rs", "fn unfinished(", None)
                .unwrap_err();
        assert!(create_error.contains("line 1"), "{create_error}");
        assert!(!root.path().join("new.rs").exists());

        write_file_payload(root.path(), "notes.txt", "fn unfinished(")
            .expect("non-Rust text keeps its existing behavior");
        assert_eq!(
            std::fs::read_to_string(root.path().join("notes.txt")).unwrap(),
            "fn unfinished("
        );
    }

    #[test]
    fn insert_after_line_preserves_the_anchor_and_refuses_stale_or_empty_input() {
        let root = tempfile::tempdir().unwrap();
        let original = "before\nANCHOR\nafter\n";
        write_file_payload(root.path(), "guide.md", original).unwrap();
        let receipt = revision(root.path(), "guide.md");

        let inserted =
            insert_after_line_payload(root.path(), "guide.md", 2, "new one\nnew two", &receipt)
                .unwrap();
        assert_eq!(inserted["anchor_preserved"], json!(true));
        assert_eq!(
            std::fs::read_to_string(root.path().join("guide.md")).unwrap(),
            "before\nANCHOR\nnew one\nnew two\nafter\n"
        );

        let after_first_edit = std::fs::read_to_string(root.path().join("guide.md")).unwrap();
        let stale =
            insert_after_line_payload(root.path(), "guide.md", 2, "must not land", &receipt)
                .unwrap_err();
        assert!(stale.contains("changed after the read"), "{stale}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("guide.md")).unwrap(),
            after_first_edit
        );

        let current = revision(root.path(), "guide.md");
        let empty =
            insert_after_line_payload(root.path(), "guide.md", 2, "", &current).unwrap_err();
        assert!(empty.contains("non-empty"), "{empty}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("guide.md")).unwrap(),
            after_first_edit
        );
    }

    #[test]
    fn insert_after_line_keeps_line_endings_and_final_newline_shape() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "windows.txt", "one\r\ntwo\r\n").unwrap();
        let windows_receipt = revision(root.path(), "windows.txt");
        insert_after_line_payload(
            root.path(),
            "windows.txt",
            1,
            "added\nagain",
            &windows_receipt,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("windows.txt")).unwrap(),
            "one\r\nadded\r\nagain\r\ntwo\r\n"
        );

        write_file_payload(root.path(), "no-final-newline.txt", "one\ntwo").unwrap();
        let eof_receipt = revision(root.path(), "no-final-newline.txt");
        insert_after_line_payload(
            root.path(),
            "no-final-newline.txt",
            2,
            "three",
            &eof_receipt,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("no-final-newline.txt")).unwrap(),
            "one\ntwo\nthree"
        );
    }

    #[test]
    fn insert_after_line_refuses_missing_anchors_without_touching_the_file() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "short.txt", "only\n").unwrap();
        let short_receipt = revision(root.path(), "short.txt");
        let past_eof =
            insert_after_line_payload(root.path(), "short.txt", 2, "must not land", &short_receipt)
                .unwrap_err();
        assert!(past_eof.contains("exceeds"), "{past_eof}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("short.txt")).unwrap(),
            "only\n"
        );

        write_file_payload(root.path(), "empty.txt", "").unwrap();
        let empty_receipt = revision(root.path(), "empty.txt");
        let no_anchor = insert_after_line_payload(
            root.path(),
            "empty.txt",
            1,
            "must not create an anchor",
            &empty_receipt,
        )
        .unwrap_err();
        assert!(no_anchor.contains("exceeds"), "{no_anchor}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("empty.txt")).unwrap(),
            ""
        );
    }

    #[test]
    fn invalid_rust_insertions_preserve_the_original_bytes_and_receipt() {
        let root = tempfile::tempdir().unwrap();
        let original = "fn answer() -> u32 {\n    42\n}\n";
        write_file_payload(root.path(), "answer.rs", original).unwrap();
        let receipt = revision(root.path(), "answer.rs");

        let error = insert_after_line_payload(root.path(), "answer.rs", 1, "if true {", &receipt)
            .unwrap_err();
        assert!(error.contains("Rust syntax validation refused"), "{error}");
        assert_eq!(revision(root.path(), "answer.rs"), receipt);
        assert_eq!(
            std::fs::read_to_string(root.path().join("answer.rs")).unwrap(),
            original
        );
    }

    #[test]
    fn a_stale_read_receipt_refuses_even_when_the_anchor_still_exists() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "race.txt", "keep\nold();\n").unwrap();
        let read = read_file_payload(root.path(), "race.txt", Some(2), Some(1)).unwrap();
        let stale = read["content_sha256"].as_str().unwrap().to_string();

        // A concurrent worker changes another line. An anchor-only edit would
        // still land and silently combine work based on a stale view.
        std::fs::write(root.path().join("race.txt"), "changed elsewhere\nold();\n").unwrap();
        let refused = edit_file_payload(root.path(), "race.txt", "old();", "new();", false, &stale)
            .unwrap_err();
        assert!(refused.contains("changed after the read"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("race.txt")).unwrap(),
            "changed elsewhere\nold();\n",
            "a stale CAS refusal writes nothing"
        );
    }

    #[test]
    fn a_strong_receipt_prefix_survives_local_model_copying_without_weakening_cas() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "prefix.txt", "one\ntwo\nthree\n").unwrap();
        let full = revision(root.path(), "prefix.txt");
        let strong_prefix = full[..MIN_SHA256_RECEIPT_CHARS].to_ascii_uppercase();

        edit_lines_payload(root.path(), "prefix.txt", 2, 2, "changed", &strong_prefix)
            .expect("a 128-bit prefix of the current receipt remains a strong CAS guard");
        assert_eq!(
            std::fs::read_to_string(root.path().join("prefix.txt")).unwrap(),
            "one\nchanged\nthree\n"
        );

        let current = revision(root.path(), "prefix.txt");
        let too_short = &current[..MIN_SHA256_RECEIPT_CHARS - 1];
        let refused =
            edit_lines_payload(root.path(), "prefix.txt", 2, 2, "short", too_short).unwrap_err();
        assert!(refused.contains("32-to-64-character"), "{refused}");

        let wrong_first_nibble = if current.starts_with('0') { "1" } else { "0" };
        let wrong_prefix = format!(
            "{wrong_first_nibble}{}",
            &current[1..MIN_SHA256_RECEIPT_CHARS]
        );
        let mismatch = edit_lines_payload(root.path(), "prefix.txt", 2, 2, "wrong", &wrong_prefix)
            .unwrap_err();
        assert!(mismatch.contains("changed after the read"), "{mismatch}");

        let stale = edit_lines_payload(root.path(), "prefix.txt", 2, 2, "stale", &strong_prefix)
            .unwrap_err();
        assert!(stale.contains("changed after the read"), "{stale}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("prefix.txt")).unwrap(),
            "one\nchanged\nthree\n",
            "neither a short nor a stale prefix may mutate the file"
        );
    }

    #[test]
    fn paged_reads_and_exact_edits_preserve_crlf() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("windows.txt"),
            b"head\r\n    old();\r\ntail\r\n",
        )
        .unwrap();
        let read = read_file_payload(root.path(), "windows.txt", Some(2), Some(1)).unwrap();
        assert_eq!(read["text"], json!("    old();\r\n"));

        edit_file_payload(
            root.path(),
            "windows.txt",
            read["text"].as_str().unwrap(),
            "    new();\r\n",
            false,
            read["content_sha256"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("windows.txt")).unwrap(),
            b"head\r\n    new();\r\ntail\r\n"
        );
    }

    #[test]
    fn line_range_cas_targets_the_named_nesting_level_without_an_anchor() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(
            root.path(),
            "nested.txt",
            "if outer {\n    run();\n    if inner {\n        run();\n    }\n}\n",
        )
        .unwrap();
        let read = read_file_payload(root.path(), "nested.txt", Some(4), Some(1)).unwrap();
        let edited = edit_lines_payload(
            root.path(),
            "nested.txt",
            4,
            4,
            "        stop();",
            read["content_sha256"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(edited["match"], json!("line_range_cas"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested.txt")).unwrap(),
            "if outer {\n    run();\n    if inner {\n        stop();\n    }\n}\n"
        );
    }

    #[test]
    fn line_range_cas_preserves_crlf_without_making_the_model_spell_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("windows.txt"),
            b"head\r\n    old();\r\ntail\r\n",
        )
        .unwrap();
        let read = read_file_payload(root.path(), "windows.txt", Some(2), Some(1)).unwrap();
        let edited = edit_lines_payload(
            root.path(),
            "windows.txt",
            2,
            2,
            "    first();\n    second();",
            read["content_sha256"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(edited["line_ending"], json!("crlf"));
        assert_eq!(
            std::fs::read(root.path().join("windows.txt")).unwrap(),
            b"head\r\n    first();\r\n    second();\r\ntail\r\n"
        );
    }

    #[test]
    fn line_range_cas_refuses_stale_or_out_of_bounds_coordinates_without_writing() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "range.txt", "one\ntwo\nthree\n").unwrap();
        let stale = revision(root.path(), "range.txt");
        std::fs::write(root.path().join("range.txt"), "changed\ntwo\nthree\n").unwrap();
        assert!(
            edit_lines_payload(root.path(), "range.txt", 2, 2, "new", &stale)
                .unwrap_err()
                .contains("changed after the read")
        );
        let current = revision(root.path(), "range.txt");
        assert!(
            edit_lines_payload(root.path(), "range.txt", 2, 99, "new", &current)
                .unwrap_err()
                .contains("exceeds")
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("range.txt")).unwrap(),
            "changed\ntwo\nthree\n"
        );
    }

    /// Zero matches and ambiguity are different mistakes with different
    /// remedies, and the error must say which one the model made.
    #[test]
    fn a_missing_anchor_and_an_ambiguous_anchor_are_told_apart() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "a.txt", "let x = 1;\nlet x = 1;\n").unwrap();

        let missing = edit_file_payload(
            root.path(),
            "a.txt",
            "let y = 2;",
            "let y = 3;",
            false,
            &revision(root.path(), "a.txt"),
        )
        .unwrap_err();
        assert!(missing.contains("not found"), "{missing}");
        assert!(
            missing.contains("read_file"),
            "the remedy for a miss is re-reading: {missing}"
        );

        let ambiguous = edit_file_payload(
            root.path(),
            "a.txt",
            "let x = 1;",
            "let x = 2;",
            false,
            &revision(root.path(), "a.txt"),
        )
        .unwrap_err();
        assert!(ambiguous.contains("2 times"), "{ambiguous}");
        assert!(
            ambiguous.contains("replace_all"),
            "the remedy for ambiguity is widening or replace_all: {ambiguous}"
        );

        let all = edit_file_payload(
            root.path(),
            "a.txt",
            "let x = 1;",
            "let x = 2;",
            true,
            &revision(root.path(), "a.txt"),
        )
        .unwrap();
        assert_eq!(all["replacements"], json!(2));
    }

    /// A small model often reconstructs indentation instead of copying it. That
    /// mistake must cost a readable refusal, never a guessed write.
    #[test]
    fn a_drifted_anchor_never_writes() {
        let root = tempfile::tempdir().unwrap();
        let original = "let Ok(text) = String::from_utf8(body) else {\n    continue;\n};\n";
        write_file_payload(root.path(), "d.txt", original).unwrap();

        let refused = edit_file_payload(
            root.path(),
            "d.txt",
            "let Ok(text) = String::from_utf8(body) else {\n      continue;\n  };",
            "let Ok(text) = String::from_utf8(body) else {\n      continue;\n  };\nfiles_searched += 1;",
            false,
            &revision(root.path(), "d.txt"),
        )
        .unwrap_err();
        assert!(refused.contains("never guesses"), "{refused}");

        let after = read_file_payload(root.path(), "d.txt", None, None).unwrap();
        assert_eq!(after["text"], json!(original), "a refused edit is inert");
    }

    /// Identical trimmed content at different nesting levels was the dangerous
    /// case for the removed fuzzy matcher: it could borrow bytes from the wrong
    /// region. Exact matching makes the intended nesting part of the proof.
    #[test]
    fn trim_identical_lines_at_different_nesting_levels_are_not_interpreted() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(
            root.path(),
            "e.txt",
            "if a {\n    go();\n    if b {\n        go();\n    }\n}\n",
        )
        .unwrap();
        let refused = edit_file_payload(
            root.path(),
            "e.txt",
            "      go();",
            "      stop();",
            false,
            &revision(root.path(), "e.txt"),
        )
        .unwrap_err();
        assert!(refused.contains("not found byte for byte"), "{refused}");
        assert!(!std::fs::read_to_string(root.path().join("e.txt"))
            .unwrap()
            .contains("stop"));

        let exact = edit_file_payload(
            root.path(),
            "e.txt",
            "        go();",
            "        stop();",
            false,
            &revision(root.path(), "e.txt"),
        )
        .unwrap();
        assert_eq!(exact["match"], json!("exact"), "{exact}");
    }

    #[test]
    fn an_empty_replacement_deletes_and_a_pointless_edit_is_refused() {
        let root = tempfile::tempdir().unwrap();
        write_file_payload(root.path(), "c.txt", "keep\ndrop me\nkeep\n").unwrap();

        edit_file_payload(
            root.path(),
            "c.txt",
            "drop me\n",
            "",
            false,
            &revision(root.path(), "c.txt"),
        )
        .unwrap();
        let after = read_file_payload(root.path(), "c.txt", None, None).unwrap();
        assert_eq!(after["text"], json!("keep\nkeep\n"));

        assert!(
            edit_file_payload(
                root.path(),
                "c.txt",
                "keep",
                "keep",
                false,
                &revision(root.path(), "c.txt"),
            )
            .unwrap_err()
            .contains("identical"),
            "old == new changes nothing and must say so"
        );
        assert!(edit_file_payload(
            root.path(),
            "c.txt",
            "",
            "x",
            false,
            &revision(root.path(), "c.txt"),
        )
        .is_err());
    }

    /// `edit_file` mutates nothing it cannot honestly edit: absent files are
    /// `write_file`'s job, binaries have no text to anchor.
    #[test]
    fn edit_file_refuses_what_it_cannot_honestly_edit() {
        let root = tempfile::tempdir().unwrap();
        let absent =
            edit_file_payload(root.path(), "no.rs", "a", "b", false, &"0".repeat(64)).unwrap_err();
        assert!(
            absent.contains("write_file"),
            "point at the right tool: {absent}"
        );

        std::fs::write(root.path().join("img.bin"), b"a\0b").unwrap();
        assert!(edit_file_payload(
            root.path(),
            "img.bin",
            "a",
            "b",
            false,
            &revision(root.path(), "img.bin"),
        )
        .unwrap_err()
        .contains("binary"));
    }

    /// The gap this tool closes, stated as a test: locating a symbol in a large
    /// file must cost one call, not one call per 120 lines.
    #[test]
    fn a_symbol_is_found_by_content_not_by_paging() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("backend/src/api")).unwrap();
        let mut haystack = "// filler\n".repeat(4000);
        haystack.push_str("pub fn reclaim_preserved_worktree_artifacts() {}\n");
        haystack.push_str(&"// filler\n".repeat(4000));
        std::fs::write(
            root.path().join("backend/src/api/orchestration.rs"),
            haystack,
        )
        .unwrap();

        let found = search_text_payload(
            root.path(),
            "reclaim_preserved_worktree_artifacts",
            None,
            false,
        )
        .unwrap();
        assert_eq!(found["count"], json!(1), "{found}");
        assert_eq!(
            found["matches"][0]["path"],
            json!("backend/src/api/orchestration.rs")
        );
        assert_eq!(
            found["matches"][0]["line"],
            json!(4001),
            "the line must be exact: {found}"
        );
        assert_eq!(found["truncated"], json!(false));
        let path = "backend/src/api/orchestration.rs";
        assert_eq!(
            found["file_revisions"][path],
            json!(revision(root.path(), path)),
            "search and read must expose the same byte revision"
        );
    }

    #[test]
    fn a_glob_restricts_where_the_search_looks() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("backend/src")).unwrap();
        std::fs::create_dir_all(root.path().join("frontend/src")).unwrap();
        std::fs::write(root.path().join("backend/src/lib.rs"), "let marker = 1;\n").unwrap();
        std::fs::write(
            root.path().join("frontend/src/app.ts"),
            "const marker = 1;\n",
        )
        .unwrap();

        let everywhere = search_text_payload(root.path(), "marker", None, false).unwrap();
        assert_eq!(everywhere["count"], json!(2), "{everywhere}");

        let scoped =
            search_text_payload(root.path(), "marker", Some("backend/**/*.rs"), false).unwrap();
        assert_eq!(scoped["count"], json!(1), "{scoped}");
        assert_eq!(scoped["matches"][0]["path"], json!("backend/src/lib.rs"));
    }

    #[test]
    fn case_is_ignored_unless_the_caller_says_otherwise() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("notes.md"),
            "TaskExecution and taskexecution\n",
        )
        .unwrap();

        let loose = search_text_payload(root.path(), "taskexecution", None, false).unwrap();
        assert_eq!(
            loose["count"],
            json!(1),
            "one LINE matches, twice over: {loose}"
        );

        let exact = search_text_payload(root.path(), "TaskExecution", None, true).unwrap();
        assert_eq!(exact["count"], json!(1), "{exact}");
        let absent = search_text_payload(root.path(), "taskexecution", None, true).unwrap();
        assert_eq!(
            absent["count"],
            json!(1),
            "the lowercase spelling is on that same line"
        );
        let missing = search_text_payload(root.path(), "TASKEXECUTION", None, true).unwrap();
        assert_eq!(
            missing["count"],
            json!(0),
            "case-sensitive means case-sensitive: {missing}"
        );
    }

    /// A generated file must not be able to spend the whole answer, and the
    /// model has to be told the answer is partial rather than complete.
    #[test]
    fn one_file_cannot_crowd_out_every_other_hit() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("generated.rs"), "marker\n".repeat(200)).unwrap();
        std::fs::write(root.path().join("real.rs"), "marker here\n").unwrap();

        let found = search_text_payload(root.path(), "marker", None, false).unwrap();
        assert!(
            found["count"].as_u64().unwrap() <= MAX_SEARCH_MATCHES as u64,
            "the total is bounded: {found}"
        );
        assert_eq!(
            found["truncated"],
            json!(true),
            "a bounded answer must say so: {found}"
        );
        let from_generated = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|hit| hit["path"] == json!("generated.rs"))
            .count();
        assert_eq!(
            from_generated, MAX_SEARCH_MATCHES_PER_FILE,
            "one file is capped so the others still get through: {found}"
        );
        assert_eq!(found["files_with_matches"], json!(2), "{found}");
    }

    /// A byte match inside a binary is not a line anyone can act on, and the
    /// bytes themselves would be shown to the model as text.
    #[test]
    fn binary_files_are_not_searched() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("image.bin"), b"before\0marker\0after").unwrap();
        let found = search_text_payload(root.path(), "marker", None, false).unwrap();
        assert_eq!(found["count"], json!(0), "{found}");
    }

    /// `files_searched` must distinguish "the text is not there" from "the
    /// search never looked at anything": a glob that matches no file leaves
    /// `files_searched` at 0, while a fruitless search over real files leaves
    /// it above 0.
    #[test]
    fn files_searched_distinguishes_no_files_from_no_matches() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "hello\n").unwrap();
        std::fs::write(root.path().join("b.txt"), "world\n").unwrap();

        // A glob that matches nothing: the search opened no file at all.
        let no_files = search_text_payload(root.path(), "hello", Some("**/*.rs"), false).unwrap();
        assert_eq!(no_files["count"], json!(0), "{no_files}");
        assert_eq!(
            no_files["files_searched"],
            json!(0),
            "a glob that matches nothing must report zero files searched: {no_files}"
        );

        // A fruitless search over real files: the files were opened and scanned,
        // but the needle was not in them.
        let no_matches = search_text_payload(root.path(), "zzz_not_here", None, false).unwrap();
        assert_eq!(no_matches["count"], json!(0), "{no_matches}");
        assert_eq!(
            no_matches["files_searched"],
            json!(2),
            "a fruitless search over two real files must report two files searched: {no_matches}"
        );
    }

    #[test]
    fn a_very_long_line_is_shortened_rather_than_returned_whole() {
        let root = tempfile::tempdir().unwrap();
        let long = format!("marker{}", "x".repeat(MAX_SEARCH_LINE_CHARS * 3));
        std::fs::write(root.path().join("bundle.js"), long).unwrap();
        let found = search_text_payload(root.path(), "marker", None, false).unwrap();
        let text = found["matches"][0]["text"].as_str().unwrap();
        assert!(
            text.chars().count() <= MAX_SEARCH_LINE_CHARS + 1,
            "a minified line must not spend the window: {} chars",
            text.chars().count()
        );
        assert!(text.ends_with('…'), "and the trim must be visible: {text}");
        assert_eq!(found["matches"][0]["text_truncated"], json!(true));
    }

    /// The returned line is exact, indentation included, so it can serve as an
    /// `edit_file` anchor: a copy-paste from `search_text` must match the file
    /// byte for byte.
    #[test]
    fn an_indented_line_is_returned_with_its_indentation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.rs"), "    let marker = 42;\n").unwrap();
        let found = search_text_payload(root.path(), "marker", None, false).unwrap();
        let text = found["matches"][0]["text"].as_str().unwrap();
        assert_eq!(
            text, "    let marker = 42;",
            "indentation must be preserved: {text:?}"
        );
        assert_eq!(found["matches"][0]["text_truncated"], json!(false));
    }

    #[test]
    fn an_empty_query_is_refused_rather_than_matching_everything() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "anything\n").unwrap();
        assert!(search_text_payload(root.path(), "", None, false).is_err());
    }

    #[test]
    fn write_then_read_round_trips_and_reports_creation() {
        let root = tempfile::tempdir().unwrap();
        let first = write_file_payload(root.path(), "notes/report.md", "# Rapport\n").unwrap();
        assert_eq!(first["created"], json!(true));
        assert_eq!(first["overwritten"], json!(false));
        let read = read_file_payload(root.path(), "notes/report.md", None, None).unwrap();
        assert_eq!(read["found"], json!(true));
        assert_eq!(read["text"], json!("# Rapport\n"));
        assert_eq!(read["truncated"], json!(false));
        // A blind second write is refused: the model cannot bypass edit CAS by
        // selecting the whole-file primitive.
        let blind = write_file_payload(root.path(), "notes/report.md", "# Perdu\n").unwrap_err();
        assert!(
            blind.contains("content_sha256"),
            "actionable refusal: {blind}"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("notes/report.md")).unwrap(),
            "# Rapport\n"
        );

        // The exact receipt from the read authorises one whole-file replacement.
        let second = write_file_payload_with_receipt(
            root.path(),
            "notes/report.md",
            "# Autre\n",
            read["content_sha256"].as_str(),
        )
        .unwrap();
        assert_eq!(second["created"], json!(false));
        assert_eq!(second["overwritten"], json!(true));
        assert_eq!(
            second["previous_sha256"], read["content_sha256"],
            "the mutation transcript remains auditable"
        );
        assert_ne!(second["content_sha256"], second["previous_sha256"]);

        // The same receipt is single-revision evidence, not a reusable bypass.
        let stale = write_file_payload_with_receipt(
            root.path(),
            "notes/report.md",
            "# Encore\n",
            read["content_sha256"].as_str(),
        )
        .unwrap_err();
        assert!(
            stale.contains("changed after the read"),
            "stale CAS: {stale}"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("notes/report.md")).unwrap(),
            "# Autre\n"
        );
    }

    #[test]
    fn an_absent_file_is_a_non_fatal_discovery_result() {
        let root = tempfile::tempdir().unwrap();

        let read = read_file_payload(root.path(), "config/missing.toml", None, None).unwrap();

        assert_eq!(read["path"], json!("config/missing.toml"));
        assert_eq!(read["found"], json!(false));
        assert_eq!(read["bytes_returned"], json!(0));
        assert_eq!(read["note"], json!("file does not exist"));
    }

    #[test]
    fn a_long_file_is_truncated_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_BYTES + 1_000);
        write_file_payload(root.path(), "big.txt", &big).unwrap();
        let read = read_file_payload(root.path(), "big.txt", None, None).unwrap();
        assert_eq!(read["truncated"], json!(true));
        assert_eq!(read["bytes_returned"], json!(MAX_BYTES));
    }

    #[test]
    fn a_slice_returns_the_asked_lines_and_says_where_to_continue() {
        // KT-399 — Kronn's own `orchestration.rs` is 449 KB. Served whole to a
        // local model with a 32k window it buries the conversation, and every
        // call after that is the model flailing in a full context. Slices make a
        // large source file readable; `next_offset` means the reader never has
        // to probe for the end.
        let root = tempfile::tempdir().unwrap();
        let body: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        write_file_payload(root.path(), "big.rs", &body).unwrap();

        let read = read_file_payload(root.path(), "big.rs", Some(10), Some(5)).unwrap();

        assert_eq!(
            read["text"],
            json!("line 10\nline 11\nline 12\nline 13\nline 14\n")
        );
        assert_eq!(read["total_lines"], json!(100));
        assert_eq!(read["lines_returned"], json!(5));
        assert_eq!(
            read["next_offset"],
            json!(15),
            "the reader is told where to resume"
        );
        assert_eq!(read["truncated"], json!(false));
    }

    #[test]
    fn the_last_slice_says_there_is_nothing_after_it() {
        // The difference between "ask again" and "you have it all" has to be in
        // the payload; a reader that cannot tell will either stop early or loop.
        let root = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        write_file_payload(root.path(), "small.rs", &body).unwrap();

        let tail = read_file_payload(root.path(), "small.rs", Some(8), Some(50)).unwrap();
        assert_eq!(tail["lines_returned"], json!(3));
        assert_eq!(
            tail["next_offset"],
            json!(null),
            "nothing follows the last line"
        );

        // An offset past the end is not an error: it is an empty, final slice.
        let beyond = read_file_payload(root.path(), "small.rs", Some(999), None).unwrap();
        assert_eq!(beyond["lines_returned"], json!(0));
        assert_eq!(beyond["next_offset"], json!(null));
        assert_eq!(beyond["total_lines"], json!(10));
    }

    #[test]
    fn omitting_both_bounds_reads_exactly_as_it_always_did() {
        // The guarantee that keeps this from degrading a large-context model:
        // a caller that passes neither bound gets the previous behaviour, and
        // the new fields describe the whole file rather than a slice of it.
        let root = tempfile::tempdir().unwrap();
        let body: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        write_file_payload(root.path(), "whole.rs", &body).unwrap();

        let read = read_file_payload(root.path(), "whole.rs", None, None).unwrap();

        assert_eq!(read["text"], json!(body));
        assert_eq!(read["total_lines"], json!(40));
        assert_eq!(read["lines_returned"], json!(40));
        assert_eq!(read["next_offset"], json!(null));
    }

    #[test]
    fn a_slice_is_still_bounded_by_the_byte_cap() {
        // A reader can ask for more lines than the cap can carry. The cap wins,
        // and `truncated` says so — a silent cut would make the slice a lie.
        let root = tempfile::tempdir().unwrap();
        let fat_line = "x".repeat(4_096);
        let body: String = (0..100).map(|_| format!("{fat_line}\n")).collect();
        write_file_payload(root.path(), "fat.txt", &body).unwrap();

        let read = read_file_payload(root.path(), "fat.txt", Some(1), Some(100)).unwrap();

        assert_eq!(read["truncated"], json!(true));
        assert!(
            read["bytes_returned"].as_u64().unwrap() <= MAX_BYTES as u64,
            "the byte cap still bounds a slice",
        );
    }

    /// Build a throwaway repo so the git tools are exercised for real rather than
    /// mocked: the point of these tools is that they talk to an actual repository.
    #[cfg(unix)]
    fn tiny_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            crate::core::cmd::sync_cmd("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "first"]);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn git_status_reports_branch_and_uncommitted_work() {
        // The production gap: an agent read .git/HEAD by hand to find the branch,
        // then had to ask the human to paste a diff. One call answers both now.
        let repo = tiny_repo();
        std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.path().join("new.txt"), "fresh\n").unwrap();

        let status = git_status_payload(repo.path()).unwrap();
        assert_eq!(status["branch"], json!("main"));
        let paths: Vec<&str> = status["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"a.txt"), "modified file missing: {paths:?}");
        assert!(
            paths.contains(&"new.txt"),
            "untracked file missing: {paths:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_diff_returns_uncommitted_work_and_can_narrow_to_one_file() {
        let repo = tiny_repo();
        std::fs::write(repo.path().join("a.txt"), "one\nchanged\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "other\n").unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["add", "b.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let all = git_diff_payload(repo.path(), None, None).unwrap();
        assert!(all["diff"].as_str().unwrap().contains("changed"));
        assert_eq!(all["truncated"], json!(false));

        let narrowed = git_diff_payload(repo.path(), None, Some("a.txt")).unwrap();
        assert!(narrowed["diff"].as_str().unwrap().contains("a.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn a_revision_cannot_smuggle_an_option_and_a_path_cannot_escape() {
        // The model supplies these two strings, so they are the attack surface: an
        // argument starting with `-` could turn a read tool into a writing one
        // (`--output=…`), and a path must obey the same workspace guard as read_file.
        let repo = tiny_repo();
        assert!(git_diff_payload(repo.path(), Some("--output=/tmp/pwned"), None).is_err());
        assert!(git_diff_payload(repo.path(), None, Some("../escape")).is_err());
        assert!(!std::path::Path::new("/tmp/pwned").exists());
    }

    #[cfg(unix)]
    #[test]
    fn git_log_is_bounded_and_parsed() {
        let repo = tiny_repo();
        let log = git_log_payload(repo.path(), Some(5), None).unwrap();
        assert_eq!(log["count"], json!(1));
        assert_eq!(log["commits"][0]["subject"], json!("first"));
        // A model-supplied limit is clamped, never trusted.
        let huge = git_log_payload(repo.path(), Some(100_000), None).unwrap();
        assert!(huge["count"].as_u64().unwrap() <= 100);
    }

    #[cfg(unix)]
    #[test]
    fn git_log_path_restricts_history_to_the_touched_path() {
        let repo = tiny_repo();
        // A second file with its own history, distinct from a.txt's.
        std::fs::write(repo.path().join("b.txt"), "one\n").unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["add", "b.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .args(["commit", "-m", "second"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        // Without a path, the whole history is returned.
        let all = git_log_payload(repo.path(), Some(10), None).unwrap();
        assert_eq!(all["count"], json!(2));

        // With a path, only the commits touching that path are returned.
        let only_b = git_log_payload(repo.path(), Some(10), Some("b.txt")).unwrap();
        assert_eq!(only_b["count"], json!(1));
        assert_eq!(only_b["commits"][0]["subject"], json!("second"));

        // A path that escapes the workspace is refused.
        let refused = git_log_payload(repo.path(), Some(10), Some("../outside")).unwrap_err();
        assert!(refused.contains("outside"));
    }

    #[cfg(unix)]
    #[test]
    fn git_commit_requires_a_message_and_named_paths_before_staging_anything() {
        let repo = tiny_repo();
        std::fs::write(repo.path().join("a.txt"), "changed\n").unwrap();

        assert!(git_commit_payload(repo.path(), &[], "message").is_err());
        assert!(git_commit_payload(repo.path(), &["a.txt".into()], "   ").is_err());
        let refused = git_commit_payload(
            repo.path(),
            &["a.txt".into(), "../outside".into()],
            "must be atomic",
        )
        .unwrap_err();
        assert!(refused.contains("outside"));
        let staged = git_read(repo.path(), &["diff", "--cached", "--name-only"]).unwrap();
        assert!(
            staged.trim().is_empty(),
            "validation must finish before the first path is staged: {staged}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_commit_advances_head_for_only_the_explicitly_named_files() {
        let repo = tiny_repo();
        std::fs::write(repo.path().join("a.txt"), "changed a\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "changed b\n").unwrap();
        // A host CLI still has shell access inside its worktree and may have
        // staged an unrelated file before asking Kronn to commit. The explicit
        // mediated inventory must remain authoritative even then.
        git_read(repo.path(), &["add", "b.txt"]).unwrap();
        let before = git_read(repo.path(), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let committed = git_commit_payload(
            repo.path(),
            &["a.txt".into(), "a.txt".into()],
            "commit named file",
        )
        .unwrap();

        let after = git_read(repo.path(), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(before, after);
        assert_eq!(committed["message"], json!("commit named file"));
        assert_eq!(committed["files"], json!(["a.txt"]));
        let names = git_read(repo.path(), &["diff", "HEAD^", "HEAD", "--name-only"]).unwrap();
        assert_eq!(names.trim(), "a.txt");
        let status = git_status_payload(repo.path()).unwrap();
        assert_eq!(status["changed_count"], json!(1));
        assert_eq!(status["changes"][0]["path"], json!("b.txt"));
        let still_staged = git_read(repo.path(), &["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(still_staged.trim(), "b.txt");
    }

    #[test]
    fn find_files_answers_in_one_call_where_listing_needed_many() {
        // The production failure this fixes: a model asked to find the largest
        // file gave up, because a non-recursive listing costs one turn per
        // directory and it had no shell. One glob call now answers.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/deep/deeper")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("src/deep/lib.rs"), "pub fn a() {}").unwrap();
        std::fs::write(root.path().join("src/deep/deeper/x.rs"), "x").unwrap();
        std::fs::write(root.path().join("README.md"), "# hi").unwrap();

        let found = find_files_payload(root.path(), "**/*.rs").unwrap();
        let paths: Vec<&str> = found["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            vec!["src/deep/deeper/x.rs", "src/deep/lib.rs", "src/main.rs"]
        );
        // Size travels with each match, so "which is biggest" needs no extra reads.
        assert!(found["files"][2]["size"].as_u64().unwrap() > 0);
        // A narrower pattern must not cross segments.
        let shallow = find_files_payload(root.path(), "src/*.rs").unwrap();
        assert_eq!(shallow["count"], json!(1));
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_reached_through_a_symlink_still_reports_relative_paths() {
        // The defect this pins: `resolve_in_workspace` canonicalises, the caller's
        // root does not, and on macOS a temp path crosses `/var` → `/private/var`.
        // `strip_prefix` then failed and every entry reported an ABSOLUTE path, so
        // no relative glob matched and a targeted search answered "nothing found"
        // — the exact confidently-wrong reply this tool was built to stop.
        let real = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(real.path().join("backend/src")).unwrap();
        std::fs::write(real.path().join("backend/src/api.rs"), "x").unwrap();

        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("workspace-link");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        // Addressed through the symlink, exactly as a project path may be.
        let found = find_files_payload(&link, "backend/**/*.rs").unwrap();
        assert_eq!(
            found["count"],
            json!(1),
            "a prefixed glob must still match when the root is a symlink"
        );
        assert_eq!(
            found["files"][0]["path"].as_str().unwrap(),
            "backend/src/api.rs",
            "paths stay relative to the workspace root, never absolute"
        );

        // The recursive listing shares the same walk, so it shares the defect.
        let listed = list_files_payload(&link, Some("backend"), true).unwrap();
        let paths: Vec<&str> = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["path"].as_str())
            .collect();
        assert!(
            paths.iter().all(|p| !p.starts_with('/')),
            "no entry may leak an absolute path: {paths:?}"
        );
    }

    #[test]
    fn every_declared_tool_has_a_handler_and_vice_versa() {
        // The gap Romu asked about is exactly this class: a tool declared but not
        // dispatched (the model calls it and gets "unknown tool"), or dispatched but
        // not declared (dead code the model can never reach). Both are silent, so the
        // two lists are compared here rather than trusted to stay in sync by review.
        let declared: Vec<String> = tool_definitions()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        let mut declared_sorted = declared.clone();
        declared_sorted.sort();
        let mut names_sorted: Vec<String> = TOOL_NAMES.iter().map(|n| n.to_string()).collect();
        names_sorted.sort();
        assert_eq!(
            declared_sorted, names_sorted,
            "TOOL_NAMES (used by the dispatcher) and the declared catalogue must match exactly"
        );
        // Names are what the model types: a rename that misses one side is the same
        // silent break, so pin the whole set.
        assert_eq!(
            names_sorted,
            vec![
                "edit_file".to_string(),
                "edit_lines".to_string(),
                "find_files".to_string(),
                "git_commit".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "git_status".to_string(),
                "insert_after_line".to_string(),
                "list_files".to_string(),
                "read_file".to_string(),
                "search_text".to_string(),
                "web_fetch".to_string(),
                "write_file".to_string(),
            ]
        );
    }

    #[test]
    fn a_glob_prefix_starts_the_walk_where_the_pattern_points() {
        // The production failure: find_files("**/*.rs") on a real repository walked
        // from the root, spent its entry budget on the frontend tree, and answered
        // "there are no .rs files in backend/src" — confidently wrong. Starting at
        // the pattern's literal prefix is what makes a targeted search targeted.
        assert_eq!(glob_literal_prefix("backend/src/**/*.rs"), "backend/src");
        assert_eq!(glob_literal_prefix("**/*.rs"), "");
        assert_eq!(glob_literal_prefix("src/*.ts"), "src");
        // A trailing literal that is a file, not a directory, must not become the
        // starting directory.
        assert_eq!(glob_literal_prefix("src/main.rs"), "src");

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("backend/src/api")).unwrap();
        std::fs::create_dir_all(root.path().join("frontend/src")).unwrap();
        std::fs::write(root.path().join("backend/src/api/mod.rs"), "x").unwrap();
        std::fs::write(root.path().join("frontend/src/app.ts"), "x").unwrap();

        let scoped = find_files_payload(root.path(), "backend/**/*.rs").unwrap();
        assert_eq!(scoped["count"], json!(1));
        assert_eq!(
            scoped["files"][0]["path"].as_str().unwrap(),
            "backend/src/api/mod.rs",
            "paths stay relative to the workspace root, not to the walk start"
        );
    }

    #[test]
    fn a_recursive_walk_skips_build_directories() {
        // Without the skip list a bounded walk is spent entirely on artefacts:
        // node_modules alone can hold more entries than the whole ceiling.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(root.path().join("target/debug/bin"), "x").unwrap();
        std::fs::write(root.path().join("src/main.rs"), "x").unwrap();

        let listing = list_files_payload(root.path(), None, true).unwrap();
        let paths: Vec<String> = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap().to_string())
            .collect();
        assert!(paths.iter().any(|p| p == "src/main.rs"));
        assert!(
            !paths
                .iter()
                .any(|p| p.starts_with("node_modules") || p.starts_with("target")),
            "build and vendor directories must not be walked: {paths:?}"
        );
    }

    #[test]
    fn glob_star_stays_in_one_segment_and_doublestar_crosses() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(
            !glob_matches("*.rs", "src/main.rs"),
            "a single * must not cross /"
        );
        assert!(glob_matches("**/*.rs", "src/deep/main.rs"));
        assert!(glob_matches("src/**", "src/a/b.rs"));
        assert!(!glob_matches("src/**", "other/a.rs"));
    }

    #[test]
    fn listing_is_sorted_and_marks_directories() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("a.txt"), "a").unwrap();
        let listing = list_files_payload(root.path(), None, false).unwrap();
        let entries = listing["entries"].as_array().unwrap();
        assert_eq!(entries[0]["name"], json!("a.txt"));
        assert_eq!(entries[0]["is_dir"], json!(false));
        assert_eq!(entries[1]["name"], json!("src"));
        assert_eq!(entries[1]["is_dir"], json!(true));
    }

    #[test]
    fn write_outside_the_workspace_never_touches_the_filesystem() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        let relative = format!(
            "../{}/victim.txt",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        assert!(write_file_payload(root.path(), &relative, "pwned").is_err());
        assert!(!victim.exists(), "the refusal must happen before any write");
    }
}

#[cfg(test)]
mod real_repo_probe {
    use super::*;

    /// Exercises the glob against THIS repository, not a fixture. An agent reported
    /// `backend/src/workflows/runner.rs` as the largest `.rs` file when two larger
    /// ones exist under `backend/src/api/`, and a fixture cannot tell whether the
    /// tool or the model was wrong. Ignored by default: it depends on the checkout.
    #[test]
    #[ignore]
    fn find_files_sees_every_rs_file_of_this_repo() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let found = find_files_payload(&root, "backend/src/**/*.rs").unwrap();
        let count = found["count"].as_u64().unwrap();
        let truncated = found["truncated"].as_bool().unwrap();
        let mut biggest: Vec<(u64, String)> = found["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["size"].as_u64().unwrap_or(0),
                    f["path"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        biggest.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
        eprintln!("count={count} truncated={truncated}");
        for (size, path) in biggest.iter().take(3) {
            eprintln!("  {size:>8}  {path}");
        }
        assert!(!truncated, "the walk must not truncate on this repo");
        assert!(count > 200, "expected the full tree, got {count}");
    }
}
