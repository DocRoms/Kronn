//! Tolerant reading of `config.toml` (KT-1513).
//!
//! A value this build does not know (a variant a newer build or an unmerged
//! branch wrote, a key it never had) must cost that one value, never the whole
//! configuration. [`read`] drops what fails to deserialize one value at a time,
//! reports it, and remembers the original so the next save writes it back.
//! [`salvage_sections`] does the same for a file whose TOML is broken: every
//! table that still parses is kept.

use super::crypto;
use crate::models::AppConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use toml::Value;

/// Top-level keys of `AppConfig` that carry settings. A file holding none of
/// them is not a Kronn configuration, whatever else it holds.
const SECTIONS: &[&str] = &[
    "server",
    "tokens",
    "scan",
    "agents",
    "comfyui",
    "language",
    "ui_language",
    "stt_model",
    "tts_voices",
    "disabled_agents",
    "secret_themes",
    "unlocked_profiles",
    "disabled_auto_skills",
    "embed_allowed_origins",
    "audit_reverify_cron",
    "audit_briefing_docs",
];

/// Fields `AppConfig` reads but never writes back: removing one changes the
/// config in memory but not what is serialized, so they are not "unknown".
const READ_ONLY: &[&[&str]] = &[
    &["encryption_secret"],
    &["tokens", "anthropic"],
    &["tokens", "openai"],
    &["tokens", "google"],
];

/// What a save writes back unchanged because this build could not hold it.
#[derive(Debug, Clone, PartialEq)]
pub enum Preserved {
    /// A value at a place: the key path and what was there.
    Value(Vec<String>, Value),
    /// A list element, whole and verbatim: the key path of the list and the
    /// element, appended back to the list.
    Element(Vec<String>, Value),
}

impl Preserved {
    fn path(&self) -> &[String] {
        match self {
            Preserved::Value(path, _) | Preserved::Element(path, _) => path,
        }
    }
}

/// Everything a save must write back, with the config as it was loaded: the
/// reference that tells "the user changed this place" from "nobody touched it".
#[derive(Debug, Clone, Default)]
pub struct Kept {
    /// Read from `config.toml` at the load: written back there on a save.
    pub entries: Vec<Preserved>,
    /// Read from the encrypted shelter at the boot: they live nowhere else, and
    /// are never written to `config.toml`.
    pub sheltered: Vec<Preserved>,
    pub baseline: toml::Table,
}

impl Kept {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.sheltered.is_empty()
    }
}

#[derive(Debug)]
pub struct Reading {
    pub config: AppConfig,
    /// Values dropped because this build cannot read them: `path (why)`.
    pub ignored: Vec<String>,
    /// Keys this build has no field for: `path`.
    pub unknown: Vec<String>,
    /// What a save must write back when the config no longer holds it.
    pub kept: Kept,
    /// `encryption_secret` was present but unreadable. The caller must treat
    /// the file as damaged: the key is what everything else depends on. The
    /// value itself is never kept: it is not a setting, and the damaged file
    /// is set aside once.
    pub key_unreadable: bool,
    /// Something ignored or unknown sits on a credential path.
    credential_left: bool,
}

impl Reading {
    /// One sentence for the user, `None` when the file was read as is.
    pub fn notice(&self) -> Option<String> {
        if self.ignored.is_empty() && self.unknown.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.ignored.is_empty() {
            parts.push(format!(
                "could not understand {} (ignored, the value stays in the file)",
                self.ignored.join("; ")
            ));
        }
        if !self.unknown.is_empty() {
            parts.push(format!(
                "does not know {} (kept in the file)",
                self.unknown.join(", ")
            ));
        }
        let credentials = if self.credential_left {
            " A credential among them stays in config.toml until the encrypted store holds the \
             credentials; from then on it is kept encrypted with this instance's key in \
             config.toml.preserved.enc and no longer written to config.toml."
        } else {
            ""
        };
        Some(format!(
            "This Kronn {}. Every other setting was loaded.{credentials}",
            parts.join(" and ")
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Seg {
    Key(String),
    Idx(usize),
}

fn show(path: &[Seg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Key(k) if out.is_empty() => out.push_str(k),
            Seg::Key(k) => {
                out.push('.');
                out.push_str(k);
            }
            Seg::Idx(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

fn get_at<'a>(root: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    path.iter().try_fold(root, |value, seg| match seg {
        Seg::Key(k) => value.get(k.as_str()),
        Seg::Idx(i) => value.get(*i),
    })
}

fn get_mut_at<'a>(root: &'a mut Value, path: &[Seg]) -> Option<&'a mut Value> {
    path.iter().try_fold(root, |value, seg| match seg {
        Seg::Key(k) => value.get_mut(k.as_str()),
        Seg::Idx(i) => value.get_mut(*i),
    })
}

fn remove_at(root: &mut Value, path: &[Seg]) -> Option<Value> {
    let (last, parent) = path.split_last()?;
    let parent = get_mut_at(root, parent)?;
    match (last, parent) {
        (Seg::Key(k), Value::Table(t)) => t.remove(k.as_str()),
        (Seg::Idx(i), Value::Array(a)) if *i < a.len() => Some(a.remove(*i)),
        _ => None,
    }
}

fn set_at(root: &mut Value, path: &[Seg], value: Value) -> bool {
    let Some((last, parent)) = path.split_last() else {
        return false;
    };
    match (last, get_mut_at(root, parent)) {
        (Seg::Key(k), Some(Value::Table(t))) => {
            t.insert(k.clone(), value);
            true
        }
        (Seg::Idx(i), Some(Value::Array(a))) if *i < a.len() => {
            a[*i] = value;
            true
        }
        _ => false,
    }
}

fn segments(path: &serde_path_to_error::Path) -> Option<Vec<Seg>> {
    use serde_path_to_error::Segment;
    path.iter()
        .map(|seg| match seg {
            Segment::Map { key } => Some(Seg::Key(key.clone())),
            Segment::Enum { variant } => Some(Seg::Key(variant.clone())),
            Segment::Seq { index } => Some(Seg::Idx(*index)),
            Segment::Unknown => None,
        })
        .collect()
}

fn parse(root: &Value) -> Result<AppConfig, serde_path_to_error::Error<toml::de::Error>> {
    serde_path_to_error::deserialize(root.clone())
}

/// `name` out of `missing field `name``.
fn missing_field(message: &str) -> Option<&str> {
    let rest = message.split("missing field `").nth(1)?;
    rest.split('`').next()
}

fn key_names(path: &[Seg]) -> Option<Vec<String>> {
    path.iter()
        .map(|seg| match seg {
            Seg::Key(k) => Some(k.clone()),
            Seg::Idx(_) => None,
        })
        .collect()
}

/// Read `table` as an `AppConfig`, dropping every value this build cannot
/// read. `None` when nothing in it is a Kronn setting, or when a value cannot
/// be dropped without guessing.
pub fn read(table: &toml::Table) -> Option<Reading> {
    if !SECTIONS.iter().any(|s| table.contains_key(*s)) {
        return None;
    }
    let defaults = Value::try_from(super::config::default_config()).ok()?;
    let mut root = Value::Table(table.clone());
    let mut ignored = Vec::new();
    let mut preserved: Vec<Preserved> = Vec::new();
    let mut key_unreadable = false;
    let mut credential_left = false;
    let mut seen = std::collections::HashSet::new();

    let config = loop {
        let err = match parse(&root) {
            Ok(config) => break config,
            Err(err) => err,
        };
        let path = segments(err.path())?;
        let message = err.inner().message().to_string();
        // Each step must change something, or the loop would never end.
        if !seen.insert((path.clone(), message.clone())) {
            return None;
        }
        if let Some(name) = missing_field(&message) {
            // A required field is absent (or was just dropped): its default,
            // so the siblings around it survive. A list element has none: it
            // goes whole, below.
            let mut full = path.clone();
            full.push(Seg::Key(name.to_string()));
            match get_at(&defaults, &full) {
                Some(default) => {
                    if !set_at(&mut root, &full, default.clone()) {
                        return None;
                    }
                    continue;
                }
                None if path.iter().any(|s| matches!(s, Seg::Idx(_))) => {}
                None => return None,
            }
        }
        if path.is_empty() {
            return None;
        }
        // A bad element of a list goes with the whole element: its siblings
        // do not depend on it, and a half-element cannot be rebuilt.
        let cut = path
            .iter()
            .position(|s| matches!(s, Seg::Idx(_)))
            .map_or(path.len(), |i| i + 1);
        let path = &path[..cut];
        let original = get_at(&root, path)?.clone();
        if path == [Seg::Key("encryption_secret".into())] {
            key_unreadable = true;
        }
        // The message quotes the offending value: never when it may be a secret.
        let owner = match path.last()? {
            Seg::Idx(_) => &path[..path.len() - 1],
            Seg::Key(_) => path,
        };
        let why = if holds_credential(&key_names(owner).unwrap_or_default(), &original) {
            match message.split(", expected ").nth(1) {
                Some(expected) => format!("expected {}", expected.lines().next()?),
                None => "unreadable value".to_string(),
            }
        } else {
            message.lines().next()?.to_string()
        };
        ignored.push(format!("{} ({why})", show(path)));
        match (path.last()?, get_at(&defaults, path)) {
            (Seg::Key(_), Some(default)) => {
                set_at(&mut root, path, default.clone());
            }
            _ => {
                remove_at(&mut root, path)?;
            }
        }
        // A list element is kept whole; anything else where it sat.
        let entry = match path.last()? {
            Seg::Idx(_) => {
                key_names(&path[..path.len() - 1]).map(|k| Preserved::Element(k, original))
            }
            Seg::Key(_) => key_names(path).map(|k| Preserved::Value(k, original)),
        };
        if let Some(entry) = entry {
            if READ_ONLY.iter().any(|p| p.iter().eq(entry.path().iter())) {
                continue;
            }
            credential_left |= match &entry {
                Preserved::Value(p, v) | Preserved::Element(p, v) => holds_credential(p, v),
            };
            preserved.push(entry);
        }
    };

    // Keys this build has no field for. One it serializes is known; one it
    // does not is unknown unless it is a field kept off the file at its
    // default (`skip_serializing_if`), which any value of another type would
    // be refused by.
    let baseline = Value::try_from(&config).ok()?;
    let mut unknown = Vec::new();
    for keys in leaf_paths(&root, &mut Vec::new()) {
        let path: Vec<Seg> = keys.iter().cloned().map(Seg::Key).collect();
        if READ_ONLY.iter().any(|p| p.iter().eq(keys.iter()))
            || get_at(&baseline, &path).is_some()
            || preserved.iter().any(|p| *p.path() == keys)
        {
            continue;
        }
        if ignores_every_value(&root, &path, &baseline) {
            let value = get_at(&root, &path)?.clone();
            unknown.push(keys.join("."));
            credential_left |= holds_credential(&keys, &value);
            preserved.push(Preserved::Value(keys, value));
        }
    }

    Some(Reading {
        config,
        ignored,
        unknown,
        kept: Kept {
            entries: preserved,
            sheltered: Vec::new(),
            baseline: baseline.as_table().cloned().unwrap_or_default(),
        },
        key_unreadable,
        credential_left,
    })
}

/// The config is the same without the value at `path` and with it replaced by
/// a value of each TOML type: no field of this build reads it.
fn ignores_every_value(root: &Value, path: &[Seg], baseline: &Value) -> bool {
    let probes = [
        Value::String("\u{1}probe".into()),
        Value::Integer(1),
        Value::Boolean(true),
        Value::Float(0.5),
        Value::Array(Vec::new()),
        Value::Table(toml::Table::new()),
    ];
    let unchanged = |probe: &Value| {
        parse(probe)
            .ok()
            .and_then(|c| Value::try_from(&c).ok())
            .is_some_and(|v| v == *baseline)
    };
    let mut without = root.clone();
    if remove_at(&mut without, path).is_none() || !unchanged(&without) {
        return false;
    }
    probes.into_iter().all(|value| {
        let mut with = root.clone();
        set_at(&mut with, path, value) && unchanged(&with)
    })
}

/// The path of every leaf of the nested tables of `value`. Arrays are leaves.
fn leaf_paths(value: &Value, prefix: &mut Vec<String>) -> Vec<Vec<String>> {
    let Value::Table(table) = value else {
        return vec![prefix.clone()];
    };
    let mut out = Vec::new();
    for (key, child) in table {
        prefix.push(key.clone());
        match child {
            Value::Table(_) => out.extend(leaf_paths(child, prefix)),
            _ => out.push(prefix.clone()),
        }
        prefix.pop();
    }
    out
}

/// A key name as snake_case words: `apiKey`, `x-api-key` and `API_KEY` alike.
fn normalised(name: &str) -> String {
    let mut out = String::new();
    let mut after_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if after_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            after_lower = false;
        } else if c.is_alphanumeric() {
            out.push(c);
            after_lower = true;
        } else {
            out.push('_');
            after_lower = false;
        }
    }
    out
}

/// A key name that holds a credential, whatever table it sits in and however
/// it is cased.
fn credential_name(name: &str) -> bool {
    let mut name = normalised(name);
    // A version or rotation suffix does not make a credential a setting:
    // `token_v2`, `api_key_2`.
    while let Some((head, last)) = name.rsplit_once('_') {
        let numbered = last.strip_prefix('v').unwrap_or(last);
        if numbered.is_empty() || !numbered.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        name = head.to_string();
    }
    // `tokens` alone is a credential table; `max_tokens` is a count.
    if name == "tokens" {
        return true;
    }
    [
        "token",
        "secret",
        "secrets",
        "password",
        "passwords",
        "passwd",
        "pwd",
        "passphrase",
        "credential",
        "credentials",
        "key",
        "keys",
        "apikey",
        "auth",
        "authorization",
        "bearer",
        "dsn",
        "connection_string",
    ]
    .iter()
    .any(|word| name == *word || name.ends_with(&format!("_{word}")))
}

/// A string that is an authorization header value, whatever key holds it.
fn secret_string(text: &str) -> bool {
    let text = text.trim_start().to_ascii_lowercase();
    text.starts_with("bearer ") || text.starts_with("basic ")
}

fn holds_string(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Array(items) => items.iter().any(holds_string),
        Value::Table(table) => table.values().any(holds_string),
        _ => false,
    }
}

/// The whole value at `path` is a credential: the token store (any text under
/// `tokens` is a provider key), the encryption secret, a place named like one,
/// an authorization header. It is never written to `config.toml` once the
/// encrypted store holds the credentials.
fn whole_credential(path: &[String], value: &Value) -> bool {
    let token_store = match path {
        [first] => first == "tokens",
        [first, second, ..] => first == "tokens" && second != "disabled_overrides",
        [] => false,
    } && (path.len() == 1 || holds_string(value));
    token_store
        || path.first().is_some_and(|k| k == "encryption_secret")
        // The root `tokens` is the store, judged above: its settings of a newer
        // build are not credentials. Any other place named like one is.
        || path
            .iter()
            .enumerate()
            .any(|(at, name)| !(at == 0 && name == "tokens") && credential_name(name))
        || matches!(value, Value::String(text) if secret_string(text))
}

/// `value` without any credential-named key or authorization header, at any
/// depth.
fn scrub(value: &Value) -> Value {
    match value {
        Value::Table(table) => Value::Table(
            table
                .iter()
                .filter(|(k, v)| {
                    !credential_name(k) && !matches!(v, Value::String(t) if secret_string(t))
                })
                .map(|(k, v)| (k.clone(), scrub(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|v| !matches!(v, Value::String(t) if secret_string(t)))
                .map(scrub)
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The value at `path` is, or holds, a credential.
fn holds_credential(path: &[String], value: &Value) -> bool {
    whole_credential(path, value) || scrub(value) != *value
}

fn filled(value: Option<&Value>) -> bool {
    match value {
        None => false,
        Some(Value::Table(t)) => !t.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

fn key_path(path: &[String]) -> Vec<Seg> {
    path.iter().cloned().map(Seg::Key).collect()
}

/// The user set or cleared this place since the config was loaded: what was
/// preserved there is theirs to replace, it must not come back.
fn superseded(entry: &Preserved, baseline: &Value, now: &Value) -> bool {
    let path = key_path(entry.path());
    // A value is replaced when the config holds something other than it held at
    // load. A list is the config's: what it holds now is what the user left.
    let (ancestors, replaced) = match entry {
        Preserved::Value(..) => (1..path.len(), get_at(now, &path) != get_at(baseline, &path)),
        Preserved::Element(..) => (1..path.len() + 1, false),
    };
    replaced
        || ancestors
            .into_iter()
            .any(|n| filled(get_at(baseline, &path[..n])) && !filled(get_at(now, &path[..n])))
}

/// What of `kept` a save still has to write, given the config it saves
/// (`current`, credentials included): a place the user set or cleared since the
/// load is theirs and its preserved value goes for good.
pub fn prune(kept: &Kept, current: &toml::Table) -> Kept {
    let baseline = Value::Table(kept.baseline.clone());
    let now = Value::Table(current.clone());
    let live = |entries: &[Preserved]| -> Vec<Preserved> {
        entries
            .iter()
            .filter(|e| !superseded(e, &baseline, &now))
            .cloned()
            .collect()
    };
    Kept {
        entries: live(&kept.entries),
        sheltered: live(&kept.sheltered),
        baseline: kept.baseline.clone(),
    }
}

fn value_of(entry: &Preserved) -> &Value {
    let (Preserved::Value(_, value) | Preserved::Element(_, value)) = entry;
    value
}

/// What a save writes to `config.toml` for `entry`: nothing for a credential
/// once the encrypted store holds them, the value as is otherwise (without its
/// credential keys under `strip_credentials`).
fn written(entry: &Preserved, strip_credentials: bool) -> Option<Value> {
    let value = value_of(entry);
    if !strip_credentials {
        return Some(value.clone());
    }
    (!whole_credential(entry.path(), value)).then(|| scrub(value))
}

/// Whether the file holds `entry` as a save writes it.
fn in_file(entry: &Preserved, file: &toml::Table, strip_credentials: bool) -> bool {
    let Some(expected) = written(entry, strip_credentials) else {
        // Not written to the file at all: nothing there to compare.
        return true;
    };
    let path = key_path(entry.path());
    let file = Value::Table(file.clone());
    // As the last save wrote it, or as the file had it before that save
    // stripped its credentials.
    let is_it = |found: &Value| *found == expected || found == value_of(entry);
    match (entry, get_at(&file, &path)) {
        (Preserved::Value(..), found) => found.is_some_and(is_it),
        (Preserved::Element(..), Some(Value::Array(items))) => items.iter().any(is_it),
        (Preserved::Element(..), _) => false,
    }
}

/// `kept` minus what `file` (the `config.toml` on disk now) no longer holds:
/// the user removed it by hand, a revoked credential must not be written back
/// by the next save. Entries read from the encrypted shelter live nowhere else
/// and stay.
pub fn drop_gone(kept: &Kept, file: &toml::Table, strip_credentials: bool) -> Kept {
    Kept {
        entries: kept
            .entries
            .iter()
            .filter(|e| in_file(e, file, strip_credentials))
            .cloned()
            .collect(),
        sheltered: kept.sheltered.clone(),
        baseline: kept.baseline.clone(),
    }
}

/// Write back what a save would otherwise lose. With `strip_credentials`
/// nothing credential comes back and the rest loses its credential keys: the
/// encrypted store and the shelter hold those, the file must not.
pub fn restore(table: &mut toml::Table, entries: &[Preserved], strip_credentials: bool) {
    for entry in entries {
        if entry
            .path()
            .first()
            .is_some_and(|k| k == "encryption_secret")
        {
            continue;
        }
        let Some(value) = written(entry, strip_credentials) else {
            continue;
        };
        match entry {
            Preserved::Value(path, _) => put(table, path, value),
            Preserved::Element(path, _) => append(table, path, value),
        }
    }
}

/// Whether `a` and `b` are the same place: a value of the same path (the one
/// the file holds now replaces the one an earlier save sheltered), or the same
/// element of a list.
fn same_place(a: &Preserved, b: &Preserved) -> bool {
    match (a, b) {
        (Preserved::Value(pa, _), Preserved::Value(pb, _)) => pa == pb,
        _ => a == b,
    }
}

/// What the encrypted shelter must hold: every preserved value that is, or
/// holds, a credential (the file does not carry it, or not whole), once per
/// place. What the file holds (`entries`) is newer than what an earlier save
/// sheltered: the user changed that place by hand, the old value is revoked.
pub fn to_shelter(kept: &Kept) -> Vec<Preserved> {
    let mut out: Vec<Preserved> = Vec::new();
    for entry in kept.entries.iter().chain(&kept.sheltered) {
        if entry
            .path()
            .first()
            .is_some_and(|k| k == "encryption_secret")
        {
            continue;
        }
        if holds_credential(entry.path(), value_of(entry))
            && !out.iter().any(|held| same_place(held, entry))
        {
            out.push(entry.clone());
        }
    }
    out
}

/// The file holds `sheltered`'s place with the value a stripped save leaves
/// there (the credentials taken out): nobody touched it, the shelter's value is
/// the whole one.
fn left_stripped(held: &Preserved, sheltered: &Preserved) -> bool {
    same_place(held, sheltered)
        && value_of(held) != value_of(sheltered)
        && *value_of(held) == scrub(value_of(sheltered))
}

/// `kept` plus the shelter's entries it does not hold yet, against `baseline`
/// (the config as the boot leaves it, credentials filled in). A place the file
/// holds again with another value (the user changed it by hand) keeps the
/// file's value: the shelter's is revoked. A place the file holds as a save
/// strips it is the shelter's own, not a change. A later read of the shelter
/// replaces what an earlier one brought for the same place.
pub fn adopt(kept: &Kept, shelter: Vec<Preserved>, baseline: toml::Table) -> Kept {
    let mut sheltered = kept.sheltered.clone();
    for entry in shelter {
        if kept
            .entries
            .iter()
            .any(|held| same_place(held, &entry) && !left_stripped(held, &entry))
        {
            continue;
        }
        sheltered.retain(|held| !same_place(held, &entry));
        sheltered.push(entry);
    }
    Kept {
        entries: kept.entries.clone(),
        sheltered,
        baseline,
    }
}

/// The table at `path` in `table`, created on the way; `None` when something
/// else than a table is in the way.
fn table_at<'a>(table: &'a mut toml::Table, path: &[String]) -> Option<&'a mut toml::Table> {
    match path {
        [] => Some(table),
        [key, rest @ ..] => {
            let entry = table
                .entry(key.clone())
                .or_insert_with(|| Value::Table(toml::Table::new()));
            match entry {
                Value::Table(inner) => table_at(inner, rest),
                _ => None,
            }
        }
    }
}

fn put(table: &mut toml::Table, path: &[String], value: Value) {
    if let Some((last, parents)) = path.split_last() {
        if let Some(parent) = table_at(table, parents) {
            parent.insert(last.clone(), value);
        }
    }
}

fn append(table: &mut toml::Table, path: &[String], element: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let Some(parent) = table_at(table, parents) else {
        return;
    };
    let list = parent
        .entry(last.clone())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(items) = list {
        if !items.contains(&element) {
            items.push(element);
        }
    }
}

/// What a save by this build never writes, whatever was restored: the
/// credentials the encrypted store holds (`strip_credentials`), and the auth
/// token of a session that must not persist it (`session_token`).
pub fn strip(table: &mut toml::Table, strip_credentials: bool, session_token: bool) {
    if strip_credentials || session_token {
        if let Some(Value::Table(server)) = table.get_mut("server") {
            server.remove("auth_token");
        }
    }
    if strip_credentials {
        table.remove("encryption_secret");
        if let Some(Value::Table(tokens)) = table.get_mut("tokens") {
            // The credentials only: a setting of a newer build that sits in
            // `tokens` without being one stays.
            tokens.retain(|k, v| !whole_credential(&["tokens".to_string(), k.to_string()], v));
            tokens.insert("keys".into(), Value::Array(Vec::new()));
        }
    }
}

/// The header line as TOML: as written, or with the bracket a truncated line
/// lost (`[tokens`). `None` when it is neither.
fn header_text(line: &str) -> Option<String> {
    let line = line.trim();
    let candidates = [
        line.to_string(),
        format!("{line}{}", if line.starts_with("[[") { "]]" } else { "]" }),
    ];
    candidates
        .into_iter()
        .map(|c| format!("{c}\n"))
        .find(|c| c.parse::<toml::Table>().is_ok())
}

/// The parseable part of a file whose TOML is broken: every table that parses,
/// and in a table that does not, every line that still parses on its own. A
/// header missing its closing bracket is repaired, its lines kept; any other
/// broken header is dropped with its lines: they cannot be attributed to a
/// section. The result is valid TOML text, `None` when nothing
/// is left.
pub fn salvage_sections(content: &str) -> Option<String> {
    let mut chunks: Vec<(Option<&str>, Vec<&str>)> = vec![(None, Vec::new())];
    for line in content.lines() {
        if line.trim_start().starts_with('[') {
            chunks.push((Some(line), Vec::new()));
        } else if let Some((_, lines)) = chunks.last_mut() {
            lines.push(line);
        }
    }
    let mut out = String::new();
    for (header, lines) in chunks {
        let head = match header {
            None => String::new(),
            Some(h) => match header_text(h) {
                Some(head) => head,
                None => continue,
            },
        };
        let whole = format!("{head}{}\n", lines.join("\n"));
        if whole.parse::<toml::Table>().is_ok() {
            out.push_str(&whole);
            continue;
        }
        let mut good = String::new();
        for line in lines {
            if !line.trim().is_empty() && format!("{head}{line}\n").parse::<toml::Table>().is_ok() {
                good.push_str(line);
                good.push('\n');
            }
        }
        if header.is_none() || !good.is_empty() {
            out.push_str(&head);
            out.push_str(&good);
        }
    }
    (!out.trim().is_empty()).then_some(out)
}

/// Where credentials this build could not hold are kept once the encrypted
/// store holds the credentials: encrypted with the instance key, next to
/// `config.toml`, which must not carry them in clear.
pub const SHELTER_FILENAME: &str = "config.toml.preserved.enc";

/// One writer of the shelter at a time in this process: the read-compare-write
/// of `write_shelter` must not interleave with another save's.
static SHELTER_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static SHELTER_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Sheltered {
    element: bool,
    path: Vec<String>,
    value: Value,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ShelterFile {
    #[serde(default)]
    entries: Vec<Sheltered>,
}

fn sheltered_entries(entries: &[Preserved]) -> Vec<Sheltered> {
    let mut out: Vec<Sheltered> = entries
        .iter()
        .map(|e| Sheltered {
            element: matches!(e, Preserved::Element(..)),
            path: e.path().to_vec(),
            value: value_of(e).clone(),
        })
        .collect();
    out.sort_by(|a, b| (&a.path, a.element).cmp(&(&b.path, b.element)));
    out.dedup();
    out
}

fn decode_shelter(encrypted: &str, key_hex: &str) -> Result<Vec<Sheltered>> {
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    let text = crypto::decrypt(encrypted.trim(), &key).map_err(anyhow::Error::msg)?;
    let file: ShelterFile = toml::from_str(&text).context("parse the preserved values")?;
    Ok(file.entries)
}

/// The values the shelter of `dir` holds, decrypted with `key_hex`. Empty when
/// there is no shelter.
pub fn read_shelter(dir: &Path, key_hex: &str) -> Result<Vec<Preserved>> {
    let encrypted = match std::fs::read_to_string(dir.join(SHELTER_FILENAME)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("read the preserved values"),
    };
    Ok(decode_shelter(&encrypted, key_hex)?
        .into_iter()
        .map(|s| {
            if s.element {
                Preserved::Element(s.path, s.value)
            } else {
                Preserved::Value(s.path, s.value)
            }
        })
        .collect())
}

/// Make the shelter of `dir` hold exactly `entries`, encrypted with `key_hex`
/// (0600). A shelter this key cannot open is set aside, never overwritten.
pub fn write_shelter(dir: &Path, key_hex: &str, entries: &[Preserved]) -> Result<()> {
    let _writing = SHELTER_WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let path = dir.join(SHELTER_FILENAME);
    let wanted = sheltered_entries(entries);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(decode_shelter(&text, key_hex)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("read the preserved values"),
    };
    match existing {
        None if wanted.is_empty() => return Ok(()),
        Some(Ok(have)) if have == wanted => return Ok(()),
        Some(Ok(_)) if wanted.is_empty() => {
            return std::fs::remove_file(&path).context("remove the preserved values");
        }
        // Not ours to read, and nothing to write: left as it is.
        Some(Err(_)) if wanted.is_empty() => return Ok(()),
        Some(Err(_)) => {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
            let aside = dir.join(format!("{SHELTER_FILENAME}.unreadable.{stamp}"));
            match std::fs::rename(&path, &aside) {
                Ok(()) => tracing::warn!(
                    "{SHELTER_FILENAME} could not be read with the current key; kept as {}",
                    aside.display()
                ),
                // Another process already moved it.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).context("move the preserved values this key cannot read aside")
                }
            }
        }
        _ => {}
    }
    let text = toml::to_string(&ShelterFile { entries: wanted })
        .context("serialize the preserved values")?;
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    let encrypted = crypto::encrypt(&text, &key).map_err(anyhow::Error::msg)?;
    // A name of its own per write, like config.toml's temp: concurrent saves
    // never meet on one temp file.
    let seq = SHELTER_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{SHELTER_FILENAME}.{}.{seq}.tmp",
        std::process::id()
    ));
    super::keyvault::write_private_atomic(&tmp, &path, encrypted.as_bytes())
        .context("write the preserved values")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header that lost its closing bracket keeps its lines: they belong to
    /// the table it names.
    #[test]
    fn a_truncated_header_keeps_its_lines() {
        let text = salvage_sections(
            "language = \"en\"\n[tokens\ndisabled_overrides = [\"x\"]\n[scan]\npaths = [\"/a\"]\n",
        )
        .unwrap();
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(table["language"].as_str(), Some("en"));
        assert_eq!(table["tokens"]["disabled_overrides"][0].as_str(), Some("x"));
        assert_eq!(table["scan"]["paths"][0].as_str(), Some("/a"));
    }

    #[test]
    fn a_header_that_cannot_be_repaired_is_dropped_with_its_lines() {
        let text =
            salvage_sections("language = \"en\"\n[a b c\nx = 1\n[scan]\npaths = []\n").unwrap();
        let table: toml::Table = text.parse().unwrap();
        assert!(!table.contains_key("x"));
        assert!(table.contains_key("scan"));
    }

    fn path(p: &str) -> Vec<String> {
        p.split('.').map(str::to_string).collect()
    }

    fn table(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    #[test]
    fn credential_places_are_recognised() {
        let text = |t: &str| Value::String(t.into());
        let whole = |p: &str, v: &Value| whole_credential(&path(p), v);
        assert!(whole("tokens.keys", &Value::Array(vec![])));
        assert!(whole("tokens.mistral", &text("sk-future")));
        assert!(whole("server.auth_token", &Value::Integer(7)));
        assert!(whole("encryption_secret", &text("k")));
        assert!(whole("future.api_key", &text("sk-x")));
        assert!(!whole("tokens.disabled_overrides", &Value::Array(vec![])));
        assert!(!whole("comfyui.image_family", &text("flux2_klein4b")));
        assert!(!whole("future.max_tokens", &Value::Integer(5)));
        // A setting of a newer build under `tokens` is not a credential.
        assert!(!whole("tokens.future_rotation_days", &Value::Integer(30)));
        assert!(!whole("tokens.future_flag", &Value::Boolean(true)));
        // An authorization header is, whatever holds it.
        assert!(whole("future.header", &text("Bearer abc")));
    }

    /// One policy for every spelling of a credential name.
    #[test]
    fn credential_names_are_normalised() {
        for name in [
            "api_key",
            "apiKey",
            "API_KEY",
            "APIKey",
            "x-api-key",
            "accessToken",
            "access_token",
            "clientSecret",
            "privateKey",
            "Authorization",
            "proxy-authorization",
            "Auth",
            "dbPassword",
            "secrets",
            "passwords",
            "credentials",
            "tokens",
            "Tokens",
            "pwd",
            "db_pwd",
            "dsn",
            "DATABASE_DSN",
            "token_v2",
            "api_key_2",
            "secretV3",
        ] {
            assert!(credential_name(name), "{name} must be a credential name");
        }
        for name in [
            "v2",
            "tokens_per_second",
            "maxTokens",
            "max_tokens",
            "image_family",
            "authority",
            "monkey_patch",
            "port",
        ] {
            assert!(!credential_name(name), "{name} must not be one");
        }
    }

    /// A table named like a credential store holds credentials, wherever it
    /// sits: its whole content is kept out of the clear file.
    #[test]
    fn a_table_named_like_a_credential_store_is_one_wherever_it_sits() {
        let github: Value = toml::from_str("github = \"ghp_x\"\n").unwrap();
        for place in [
            "future.secrets",
            "future.passwords",
            "future.credentials",
            "future.tokens",
            "future.token_v2",
            "future.dsn",
            "future.pwd",
        ] {
            assert!(
                whole_credential(&path(place), &github),
                "{place} must be kept out of the clear file"
            );
        }
        let mut entries = vec![Preserved::Value(path("future.secrets"), github.clone())];
        entries.push(Preserved::Value(
            path("future.pwd"),
            Value::String("p".into()),
        ));
        let mut stripped = toml::Table::new();
        restore(&mut stripped, &entries, true);
        let text = toml::to_string(&stripped).unwrap();
        assert!(!text.contains("ghp_x") && !text.contains("\"p\""), "{text}");
        // They are sheltered instead, so nothing is lost.
        let kept = Kept {
            entries,
            ..Kept::default()
        };
        assert_eq!(to_shelter(&kept).len(), 2);
    }

    /// A place the user changed by hand: the file's value replaces what the
    /// shelter held, and the revoked value is in neither.
    #[test]
    fn a_value_changed_by_hand_replaces_the_sheltered_one() {
        let old = Preserved::Value(path("tokens.mistral"), Value::String("sk-old".into()));
        let new = Preserved::Value(path("tokens.mistral"), Value::String("sk-new".into()));
        let other = Preserved::Value(path("tokens.cohere"), Value::String("sk-c".into()));
        let from_file = Kept {
            entries: vec![new.clone()],
            ..Kept::default()
        };
        let adopted = adopt(
            &from_file,
            vec![old.clone(), other.clone()],
            toml::Table::new(),
        );
        assert_eq!(adopted.sheltered, vec![other.clone()]);
        let held = to_shelter(&adopted);
        assert!(held.contains(&new) && held.contains(&other));
        assert!(!held.contains(&old), "{held:?}");

        // A later read of the shelter replaces the earlier one's value.
        let again = adopt(&adopted, vec![old.clone()], toml::Table::new());
        assert_eq!(again.sheltered, vec![other]);
        let twice = adopt(&Kept::default(), vec![old.clone()], toml::Table::new());
        let newer = adopt(&twice, vec![new.clone()], toml::Table::new());
        assert_eq!(newer.sheltered, vec![new]);
        // Two elements of one list are two places.
        let a = Preserved::Element(path("tokens.keys"), Value::String("a".into()));
        let b = Preserved::Element(path("tokens.keys"), Value::String("b".into()));
        let both = adopt(
            &Kept::default(),
            vec![a.clone(), b.clone()],
            toml::Table::new(),
        );
        assert_eq!(both.sheltered, vec![a, b]);
    }

    /// An unknown array holding a credential: the file keeps it stripped, which
    /// is the shelter's own value and not a change by hand; another array the
    /// user wrote there is.
    #[test]
    fn an_array_left_stripped_in_the_file_is_not_a_change_by_hand() {
        let whole: Value = toml::from_str::<toml::Table>(
            "headers = [\"Bearer sk-hdr\", \"plain\"]\n\
             [[servers]]\nname = \"a\"\npassword = \"pw\"\n",
        )
        .unwrap()
        .into();
        for (place, full) in [
            ("future.headers", &whole["headers"]),
            ("future.servers", &whole["servers"]),
        ] {
            let sheltered = Preserved::Value(path(place), full.clone());
            let stripped = Preserved::Value(path(place), scrub(full));
            assert_ne!(stripped, sheltered, "{place}: the scrub changes it");
            let from_file = Kept {
                entries: vec![stripped],
                ..Kept::default()
            };
            let adopted = adopt(&from_file, vec![sheltered.clone()], toml::Table::new());
            assert_eq!(adopted.sheltered, vec![sheltered.clone()], "{place}");
            assert!(to_shelter(&adopted).contains(&sheltered), "{place}");

            let by_hand = Preserved::Value(path(place), Value::Array(vec![Value::from("mine")]));
            let from_file = Kept {
                entries: vec![by_hand],
                ..Kept::default()
            };
            let adopted = adopt(&from_file, vec![sheltered.clone()], toml::Table::new());
            assert!(
                adopted.sheltered.is_empty(),
                "{place}: the old one is revoked"
            );
            assert!(!to_shelter(&adopted).contains(&sheltered), "{place}");
        }
    }

    /// Saves racing on one directory never meet on a temp file or lose the
    /// shelter.
    #[test]
    fn parallel_shelter_writes_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let key = crypto::generate_secret();
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let (dir, key) = (dir.path().to_path_buf(), key.clone());
                std::thread::spawn(move || {
                    for round in 0..30 {
                        let entries = [Preserved::Value(
                            path("tokens.mistral"),
                            Value::String(format!("sk-{worker}-{round}")),
                        )];
                        write_shelter(&dir, &key, &entries).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(read_shelter(dir.path(), &key).unwrap().len(), 1);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_value_is_scrubbed_of_each_spelling_of_a_credential() {
        let value: Value = toml::from_str(
            "name = \"a\"\napiKey = \"sk-1\"\naccessToken = \"sk-2\"\n\
             Authorization = \"Bearer sk-3\"\nnote = \"Bearer sk-4\"\n\
             list = [\"Bearer sk-5\", \"ok\"]\n[nested]\nprivate_key = \"sk-6\"\nkept = 1\n",
        )
        .unwrap();
        let clean = toml::to_string(&scrub(&value)).unwrap();
        for secret in ["sk-1", "sk-2", "sk-3", "sk-4", "sk-5", "sk-6"] {
            assert!(!clean.contains(secret), "{secret} survived: {clean}");
        }
        assert!(clean.contains("name = \"a\"") && clean.contains("kept = 1"));
        assert!(clean.contains("\"ok\""), "{clean}");
    }

    /// Under credential stripping a preserved value loses its credential keys
    /// but keeps the rest; without it, it is written as it was.
    #[test]
    fn a_stripped_save_writes_a_preserved_value_without_its_credentials() {
        let element: Value = toml::from_str("name = \"h\"\napiKey = \"sk-in-list\"\n").unwrap();
        let entries = [
            Preserved::Value(path("future_hosts"), Value::Array(vec![element])),
            Preserved::Value(path("tokens.mistral"), Value::String("sk-future".into())),
            Preserved::Value(path("tokens.future_rotation_days"), Value::Integer(30)),
            Preserved::Value(path("server.auth_token"), Value::Integer(7)),
        ];
        let mut stripped = toml::Table::new();
        restore(&mut stripped, &entries, true);
        let text = toml::to_string(&stripped).unwrap();
        assert!(text.contains("name = \"h\""), "{text}");
        assert!(
            !text.contains("sk-in-list") && !text.contains("apiKey"),
            "{text}"
        );
        assert!(
            !text.contains("sk-future") && !text.contains("auth_token"),
            "{text}"
        );
        assert!(text.contains("future_rotation_days = 30"), "{text}");

        let mut plain = toml::Table::new();
        restore(&mut plain, &entries, false);
        let text = toml::to_string(&plain).unwrap();
        assert!(
            text.contains("sk-in-list") && text.contains("sk-future"),
            "{text}"
        );
    }

    #[test]
    fn the_strip_keeps_a_setting_of_a_newer_build_under_tokens() {
        let mut config = table(
            "encryption_secret = \"k\"\n[server]\nauth_token = \"t\"\n\
             [tokens]\nkeys = []\nmistral = \"sk-x\"\nfuture_rotation_days = 30\n\
             disabled_overrides = [\"a\"]\n",
        );
        strip(&mut config, true, false);
        let text = toml::to_string(&config).unwrap();
        assert!(
            !text.contains("sk-x") && !text.contains("auth_token"),
            "{text}"
        );
        assert!(!text.contains("encryption_secret"), "{text}");
        assert!(text.contains("future_rotation_days = 30"), "{text}");
        assert!(text.contains("disabled_overrides"), "{text}");
    }

    /// Clearing a whole list: what was preserved for it does not come back.
    #[test]
    fn a_cleared_list_drops_its_preserved_elements() {
        let kept = Kept {
            entries: vec![Preserved::Element(
                path("disabled_agents"),
                Value::String("agent_from_the_future".into()),
            )],
            sheltered: Vec::new(),
            baseline: table("disabled_agents = [\"ClaudeCode\"]\n"),
        };
        let untouched = prune(&kept, &table("disabled_agents = [\"ClaudeCode\"]\n"));
        assert_eq!(untouched.entries.len(), 1);
        let cleared = prune(&kept, &table("disabled_agents = []\n"));
        assert!(cleared.entries.is_empty(), "{:?}", cleared.entries);
    }

    #[test]
    fn a_value_removed_from_the_file_by_hand_is_not_written_back() {
        let kept = Kept {
            entries: vec![
                Preserved::Value(path("tokens.mistral"), Value::String("sk-old".into())),
                Preserved::Value(path("future.flag"), Value::Boolean(true)),
                Preserved::Element(path("disabled_agents"), Value::String("x".into())),
            ],
            sheltered: vec![Preserved::Value(
                path("tokens.cohere"),
                Value::String("sk-c".into()),
            )],
            baseline: toml::Table::new(),
        };
        // The user revoked the key and left the rest.
        let file = table("[future]\nflag = true\n");
        let seen = drop_gone(&kept, &file, false);
        assert_eq!(seen.entries.len(), 1);
        assert_eq!(seen.entries[0].path(), path("future.flag").as_slice());
        // What only the shelter holds is not the file's to judge.
        assert_eq!(seen.sheltered.len(), 1);
        // Under the strip a credential is not in the file by design.
        let stripped = drop_gone(&kept, &file, true);
        assert!(stripped
            .entries
            .iter()
            .any(|e| e.path() == path("tokens.mistral").as_slice()));
        assert!(!stripped
            .entries
            .iter()
            .any(|e| e.path() == path("disabled_agents").as_slice()));
    }

    /// The file holds the value either as the load read it or, after a stripped
    /// save, without its credential keys: both are "still there".
    #[test]
    fn a_value_holding_a_credential_is_still_in_the_file_before_and_after_a_strip() {
        let original = table("future_hosts = [{ name = \"h\", apiKey = \"sk-x\" }]\n");
        let entry = Preserved::Value(path("future_hosts"), original["future_hosts"].clone());
        let kept = Kept {
            entries: vec![entry],
            ..Kept::default()
        };
        let scrubbed = table("future_hosts = [{ name = \"h\" }]\n");
        for file in [&original, &scrubbed] {
            assert_eq!(drop_gone(&kept, file, true).entries.len(), 1);
        }
        assert_eq!(drop_gone(&kept, &scrubbed, false).entries.len(), 0);
        assert_eq!(drop_gone(&kept, &toml::Table::new(), true).entries.len(), 0);
    }

    #[test]
    fn the_notice_says_where_a_credential_is_kept_and_never_quotes_it() {
        let reading = read(&table(
            "language = \"en\"\n[server]\nauth_token = 12345678\n\
             [tokens]\nmistral = \"sk-future\"\n",
        ))
        .unwrap();
        let notice = reading.notice().unwrap();
        assert!(notice.contains(SHELTER_FILENAME), "{notice}");
        assert!(notice.contains("encrypted"), "{notice}");
        assert!(
            !notice.contains("12345678") && !notice.contains("sk-future"),
            "{notice}"
        );
    }

    #[test]
    fn the_shelter_is_encrypted_exact_and_never_overwritten_by_another_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = crypto::generate_secret();
        let other = crypto::generate_secret();
        let entries = vec![
            Preserved::Value(path("tokens.mistral"), Value::String("sk-future".into())),
            Preserved::Element(
                path("tokens.keys"),
                toml::from_str::<Value>("id = \"2\"\nvalue = \"sk-other\"\nactive = \"yes\"\n")
                    .unwrap(),
            ),
        ];
        write_shelter(dir.path(), &key, &entries).unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join(SHELTER_FILENAME)).unwrap();
        assert!(!on_disk.contains("sk-future") && !on_disk.contains("sk-other"));
        let read_back = read_shelter(dir.path(), &key).unwrap();
        assert_eq!(read_back.len(), 2);
        assert!(read_back.contains(&entries[0]) && read_back.contains(&entries[1]));
        assert!(read_shelter(dir.path(), &other).is_err());

        // Nothing to add and nothing to remove: the file is left alone.
        write_shelter(dir.path(), &key, &entries).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(SHELTER_FILENAME)).unwrap(),
            on_disk
        );

        // Another key never overwrites it: the old file is set aside.
        write_shelter(dir.path(), &other, &entries[..1]).unwrap();
        assert_eq!(read_shelter(dir.path(), &other).unwrap().len(), 1);
        let aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".unreadable."))
            .count();
        assert_eq!(aside, 1);

        // Nothing left to keep: the shelter goes.
        write_shelter(dir.path(), &other, &[]).unwrap();
        assert!(!dir.path().join(SHELTER_FILENAME).exists());
    }
}
