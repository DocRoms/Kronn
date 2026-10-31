//! KT-1496 — the routes that return a secret, or are an instance operation, ask
//! for the strong confirmation of KT-1492 (`core::trust_proof`) when one is
//! enrolled.
//!
//! This module holds the pure parts: which routes are protected, the object a
//! proof is bound to, and the short-lived single-use grants the screen obtains
//! once the code is typed. The gate that applies them is `api::strong_confirm`.
//!
//! Without an enrolment nothing here refuses anything: the owner's rule is to
//! never bridle more than before. With one, the proof is a code of the
//! authenticator (the code is the proof on every channel) or a grant minted
//! from such a code, and it is spent on one route and one object.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use aes_gcm::aead::{rand_core::RngCore, OsRng};

/// The header that carries a grant minted by `POST /api/trust/grants`.
pub const GRANT_HEADER: &str = "x-kronn-grant";
/// The JSON body field that may carry the code, for a caller that cannot set a header.
pub const BODY_FIELD: &str = "kronn_confirm";
/// Seconds a grant lives.
pub const GRANT_TTL_SECS: u64 = 120;
/// Grants kept at once: older ones are dropped first.
const MAX_GRANTS: usize = 64;
/// Prefix of a grant, so a leaked one is recognisable.
const GRANT_PREFIX: &str = "kgr_";

/// Whether a route asks for the proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Nothing is asked (this module never refuses a route it does not list).
    Open,
    /// Every call needs the proof.
    Always,
    /// Only some calls need it: the handler decides on the body and the stored
    /// state, and asks through `api::strong_confirm::Strong::require`.
    Conditional,
}

/// Routes whose every call returns a secret, yields one, changes what protects
/// one (the inventory of KT-1496), or is an operation on the instance itself.
/// This list is the whole of what asks for the proof, besides the sensitive
/// deletions and the conditional routes: `is_destructive` (the local-or-token
/// gate) does not decide it. A non-secret maintenance route stays open here.
const ALWAYS: &[(&str, &str)] = &[
    // The passphrase that wraps the encryption key, and the code that unwraps it.
    ("POST", "/api/config/recovery/set"),
    // Every encrypted row plus the recovery blob.
    ("GET", "/api/config/export"),
    // A decrypted value.
    ("POST", "/api/mcps/configs/{id}/reveal"),
    ("POST", "/api/external-api/connections/{id}/reveal"),
    ("POST", "/api/execution-context/{run_kind}/{run_id}/reveal"),
    // A new operator token, and who may reach it.
    ("POST", "/api/config/auth-token/regenerate"),
    ("POST", "/api/config/network-exposure"),
    // Provider keys written into other tools' files.
    ("POST", "/api/config/sync-agent-tokens"),
    // What an agent may do, where Kronn listens, which GitHub identity a project uses.
    ("POST", "/api/config/agent-access"),
    ("POST", "/api/config/server"),
    ("PUT", "/api/projects/{id}/github"),
    ("POST", "/api/agents/install"),
    // A human credential is revoked for good (the admin secret is its own gate
    // for who may; the code is the owner's, for whether now).
    ("POST", "/api/human-credentials/revoke"),
    // Instance operations (owner decision, 28/10): wipe it, replace it with an
    // import, re-encrypt an import, open the host to a project's validations,
    // remove an agent CLI, or bring in a page that can carry code.
    ("POST", "/api/setup/reset"),
    ("POST", "/api/config/import"),
    ("POST", "/api/config/recovery/reencrypt"),
    ("PUT", "/api/projects/{id}/validations-access"),
    ("POST", "/api/agents/uninstall"),
    ("POST", "/api/pages/import"),
];

/// The deletions that ask for the proof (owner decision, 28/10): an
/// irreversible loss of value. A project; a plugin or a connection together with
/// its credentials; a stored provider API key. Every other `DELETE` is an
/// everyday deletion (a message, a discussion, a task, a workflow, a page, a
/// Quick Prompt, a Quick Exec, a skill, …) and goes through: it stays behind the
/// destructive gate of `auth_allows` (local or token), which this does not touch.
const SENSITIVE_DELETES: &[&str] = &[
    "/api/projects/{id}",
    "/api/mcps/configs/{id}",
    "/api/external-api/connections/{id}",
    "/api/config/api-keys/{id}",
];

/// Routes where the handler decides, because only part of the calls matter:
/// a bundle export with values, a base URL or endpoint that changes, a Quick
/// Exec saved with the human approval.
const CONDITIONAL: &[(&str, &str)] = &[
    ("POST", "/api/mcps/bundles/export"),
    ("PUT", "/api/mcps/custom/{server_id}"),
    ("PUT", "/api/external-api/connections/{id}"),
    ("POST", "/api/quick-execs"),
    ("PUT", "/api/quick-execs/{id}"),
];

/// Routes that stay open on purpose, though `is_destructive` lists them:
/// - the trust routes ask for the proof themselves (`api::trust`), and take the
///   same code, so asking twice would spend two codes for one action;
/// - a restore carries the recovery passphrase, its own proof;
/// - starting a new key is for a key that is lost, so the seed that would be
///   checked is lost with it.
const OWN_PROOF: &[(&str, &str)] = &[
    ("POST", "/api/config/recovery/restore"),
    ("POST", "/api/config/recovery/start-new-key"),
];

/// The protection of a route, by its method and its matched pattern.
pub fn protection(method: &str, pattern: &str) -> Protection {
    // A HEAD runs the GET handler (the body is dropped after the work is done).
    let method = if method.eq_ignore_ascii_case("HEAD") {
        "GET"
    } else {
        method
    };
    let is = |list: &[(&str, &str)]| {
        list.iter()
            .any(|(m, p)| m.eq_ignore_ascii_case(method) && *p == pattern)
    };
    if pattern.starts_with("/api/trust") || is(OWN_PROOF) {
        return Protection::Open;
    }
    if is(CONDITIONAL) {
        return Protection::Conditional;
    }
    if method.eq_ignore_ascii_case("DELETE") {
        // An explicit list, not "every DELETE": see `SENSITIVE_DELETES`.
        return if SENSITIVE_DELETES.contains(&pattern) {
            Protection::Always
        } else {
            Protection::Open
        };
    }
    // An explicit list too: a route that `is_destructive` gates, and that is not
    // in `ALWAYS`, is everyday maintenance (a log purge, a quota rearm, …) and
    // keeps only its local-or-token gate. The tests classify each one on purpose.
    if is(ALWAYS) {
        Protection::Always
    } else {
        Protection::Open
    }
}

/// `METHOD /pattern`, as a proof is bound to it and the journal names it.
pub fn route_label(method: &str, pattern: &str) -> String {
    format!("{} {}", method.to_ascii_uppercase(), pattern)
}

/// The object of a call: the values of the pattern's `{…}` segments, in order
/// (`-` when it has none), read from the concrete path.
pub fn object_of(pattern: &str, path: &str) -> String {
    let values: Vec<&str> = pattern
        .split('/')
        .zip(path.split('/'))
        .filter(|(pattern, _)| pattern.starts_with('{') && pattern.ends_with('}'))
        .map(|(_, value)| value)
        .collect();
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join("/")
    }
}

/// The code a JSON body carries in [`BODY_FIELD`], if any.
pub fn body_code(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()?
        .get(BODY_FIELD)?
        .as_str()
        .map(|code| code.trim().to_owned())
        .filter(|code| !code.is_empty())
}

#[derive(Debug)]
struct Grant {
    target: String,
    expires: u64,
}

/// What a grant is bound to: the method and the concrete path of the one call
/// (`DELETE /api/projects/p1`), so it names the route and the object at once.
pub fn target_of(method: &str, path: &str) -> String {
    format!("{} {}", method.to_ascii_uppercase(), path)
}

fn grants() -> &'static Mutex<HashMap<String, Grant>> {
    static GRANTS: std::sync::OnceLock<Mutex<HashMap<String, Grant>>> = std::sync::OnceLock::new();
    GRANTS.get_or_init(Default::default)
}

/// A grant for one call ([`target_of`]), valid for [`GRANT_TTL_SECS`] from
/// `now`. It lives in memory only: a restart ends every grant.
pub fn mint(target: &str, now: u64) -> String {
    let mut bytes = [0u8; 24];
    OsRng.fill_bytes(&mut bytes);
    let token = format!(
        "{GRANT_PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut all = grants().lock().unwrap_or_else(|p| p.into_inner());
    all.retain(|_, grant| grant.expires > now);
    while all.len() >= MAX_GRANTS {
        let Some(oldest) = all
            .iter()
            .min_by_key(|(_, grant)| grant.expires)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        all.remove(&oldest);
    }
    all.insert(
        token.clone(),
        Grant {
            target: target.to_owned(),
            expires: now + GRANT_TTL_SECS,
        },
    );
    token
}

/// Spend a grant on the call `target`. A grant is removed by the first attempt
/// that names it, whatever its binding: one presented on another call is burnt,
/// not kept for a second try.
pub fn consume(token: &str, target: &str, now: u64) -> bool {
    let mut all = grants().lock().unwrap_or_else(|p| p.into_inner());
    match all.remove(token.trim()) {
        Some(grant) => grant.expires > now && grant.target == target,
        None => false,
    }
}

/// A path as the file system would resolve it, without needing it to exist:
/// symlinks of the deepest existing ancestor are resolved, `.` and `..` folded.
fn resolved(path: &Path) -> PathBuf {
    let mut absolute = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::CurDir => {}
            other => absolute.push(other.as_os_str()),
        }
    }
    let mut tail = Vec::new();
    let mut head = absolute.clone();
    loop {
        if let Ok(real) = head.canonicalize() {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (head.file_name().map(|n| n.to_owned()), head.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                head = parent.to_path_buf();
            }
            _ => return absolute,
        }
    }
}

/// Whether `path` is the data directory, inside it, or contains it. A project
/// there would let project commands read the vault (KT-1496), so it is refused
/// whatever the enrolment.
pub fn overlaps_data_dir(path: &Path, data_dir: &Path) -> bool {
    let (path, data) = (resolved(path), resolved(data_dir));
    path.starts_with(&data) || data.starts_with(&path)
}

#[cfg(test)]
#[path = "strong_confirm_tests.rs"]
mod strong_confirm_tests;
