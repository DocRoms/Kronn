//! The latest tool calls of a running agent, published for readers other than
//! the client streaming its output.
//!
//! What is shown is built from a call's structured input under fixed rules,
//! never copied from free text: a shell command shows its program names, a URL
//! its scheme and host, a file tool its path. Model prose and runtime titles
//! are never shown.

use std::collections::VecDeque;

use serde_json::Value;

use crate::models::{AgentActivity, AuditActivityEntry, AuditRecentActivity};

/// Holds the latest activity of one launch; `None` until a tool call starts.
pub type AgentActivitySink = tokio::sync::watch::Sender<Option<AgentActivity>>;

/// Longest target kept, in characters.
pub const TARGET_MAX_CHARS: usize = 120;

pub fn tool_started(sink: Option<&AgentActivitySink>, tool: &str) {
    if let Some(sink) = sink {
        // Counted here, where every call passes: a reader polling the sink
        // would miss the calls that land between two reads.
        sink.send_modify(|current| {
            let calls = current
                .as_ref()
                .map_or(0, |previous| previous.calls)
                .saturating_add(1);
            *current = Some(AgentActivity {
                tool: tool.to_owned(),
                target: None,
                at: chrono::Utc::now(),
                calls,
            });
        });
    }
}

/// Attach the completed input's target to the call started last.
pub fn tool_target(sink: Option<&AgentActivitySink>, target: String) {
    if let Some(sink) = sink {
        sink.send_if_modified(|current| match current {
            Some(activity) if activity.target.as_deref() != Some(target.as_str()) => {
                activity.target = Some(target);
                true
            }
            _ => false,
        });
    }
}

/// Entries kept by [`RecentActivity`].
pub const RECENT_MAX_ENTRIES: usize = 15;
const TOOL_NAME_MAX_CHARS: usize = 60;
/// Longest input scanned for a target: longer text is not parsed or redacted,
/// so the work stays bounded wherever it runs.
const INPUT_SCAN_MAX_BYTES: usize = 4096;

/// A target built by this module's rules; nothing else can make one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeTarget(String);

impl SafeTarget {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// Redaction as defence in depth over what the rules kept, then the bound.
    fn finish(text: String) -> Option<Self> {
        let (redacted, _) = crate::core::redact::redact_for_audit_artifact(&text);
        let trimmed = redacted.trim();
        (!trimmed.is_empty()).then(|| Self(truncate_chars(trimmed, TARGET_MAX_CHARS)))
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

/// The target of a tool's raw JSON input, for the live views.
pub fn tool_input_target(raw_input: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(raw_input).ok()?;
    input_target(&value).map(SafeTarget::into_string)
}

/// The target of a tool's JSON input:
/// - a shell command shows its program names (see [`shell_target`]);
/// - a search shows `in <path>`, never its pattern;
/// - a file tool shows its path;
/// - a fetch shows the URL's scheme and host.
///
/// Every other argument — contents, edits, queries — is left out.
pub fn input_target(input: &Value) -> Option<SafeTarget> {
    if let Some(command) = input.get("command").or_else(|| input.get("cmd")) {
        return match command {
            Value::String(script) => shell_target(script),
            Value::Array(argv) => argv_target(argv),
            _ => None,
        };
    }
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let path = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|key| field(key));
    if input.get("pattern").is_some() {
        return path
            .and_then(path_text)
            .and_then(|path| SafeTarget::finish(format!("in {path}")));
    }
    if let Some(path) = path {
        return path_text(path).and_then(SafeTarget::finish);
    }
    field("url").and_then(url_text).and_then(SafeTarget::finish)
}

/// A generic ACP `tool_call` / `tool_call_update`, from its structured fields:
/// its kind, and the target of its raw input or of its first location. Its
/// title is free text — often the command line itself — and is never read.
pub fn acp_tool_update(update: &Value) -> Option<ToolActivityUpdate> {
    let call = update.get("toolCall").unwrap_or(update);
    let id = call
        .get("toolCallId")
        .or_else(|| update.get("toolCallId"))
        .and_then(Value::as_str)
        .map(|id| id.chars().take(128).collect::<String>());
    let tool = call.get("kind").and_then(Value::as_str).map(|kind| {
        match kind {
            "read" => "Read",
            "edit" => "Edit",
            "delete" => "Delete",
            "move" => "Move",
            "search" => "Search",
            "execute" => "Execute",
            "think" => "Think",
            "fetch" => "Fetch",
            _ => "Tool",
        }
        .to_owned()
    });
    let target = call.get("rawInput").and_then(input_target).or_else(|| {
        let path = call
            .get("locations")?
            .as_array()?
            .first()?
            .get("path")?
            .as_str()?;
        path_text(path).and_then(SafeTarget::finish)
    });
    (id.is_some() || tool.is_some() || target.is_some()).then_some(ToolActivityUpdate {
        id,
        tool,
        target,
    })
}

/// A path as given, when it is one line and complete.
fn path_text(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.len() > INPUT_SCAN_MAX_BYTES || path.contains(['\n', '\r']) {
        return None;
    }
    // Shortened by the runtime: its end may be the start of a secret.
    if path.ends_with("...") || path.ends_with('…') {
        return None;
    }
    if path.contains("://") {
        return url_text(path);
    }
    Some(path.to_owned())
}

/// `scheme://host` and nothing else: no user info, port, path, query or fragment.
fn url_text(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    {
        return None;
    }
    let before_query = rest
        .split(|c: char| matches!(c, '?' | '#') || c.is_whitespace())
        .next()?;
    // User info may hold a `/`: the host starts after the last `@`.
    let authority_and_path = before_query.rsplit('@').next()?;
    let authority = authority_and_path.split('/').next()?;
    // The port is dropped too: `user:1234/…` cannot be told from `host:port`.
    let host = authority.split(':').next()?;
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    host_ok.then(|| format!("{}://{host}", scheme.to_ascii_lowercase()))
}

/// `["bash", "-lc", "<script>"]` is a script; any other argv shows its program.
fn argv_target(argv: &[Value]) -> Option<SafeTarget> {
    let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
    let program = program_name(words.first()?)?;
    if matches!(program.as_str(), "bash" | "sh" | "zsh" | "dash" | "fish") {
        if let Some(at) = words.iter().position(|word| {
            word.starts_with('-') && word.ends_with('c') && !word.starts_with("--")
        }) {
            if let Some(script) = words.get(at + 1) {
                return shell_target(script);
            }
        }
    }
    SafeTarget::finish(program)
}

/// A shell command reduced to its structure, each segment of a pipeline or
/// sequence by its program name: `curl | jq`, `PGPASSWORD=… psql`,
/// `cat > ~/.pgpass`. Arguments, assigned values, heredoc delimiters and
/// bodies, here-strings and every line after the first are never shown. On
/// any doubt about the parse, only the first program name is.
pub fn shell_target(script: &str) -> Option<SafeTarget> {
    let line = script.trim_start().lines().next()?;
    let summary = if line.len() > INPUT_SCAN_MAX_BYTES {
        first_program(&line[..floor_char_boundary(line, INPUT_SCAN_MAX_BYTES)])
    } else {
        match tokenize(line) {
            Some(tokens) => summarize(&tokens),
            None => first_program(line),
        }
    };
    summary.and_then(SafeTarget::finish)
}

fn floor_char_boundary(text: &str, at: usize) -> usize {
    (0..=at.min(text.len()))
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0)
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    /// A word, unquoted; `assign` is the name of a leading `NAME=`.
    Word {
        text: String,
        assign: Option<String>,
    },
    Control(&'static str),
    Redirect(&'static str),
}

/// Shell words, quotes and escapes resolved. `None` on anything this small
/// parser does not follow: unterminated quotes, substitutions, subshells.
fn tokenize(line: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = line.chars().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = at(i) {
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let (token, width) = match (c, at(i + 1), at(i + 2)) {
            ('|', Some('|'), _) => (Some(Token::Control("||")), 2),
            ('|', Some('&'), _) => (Some(Token::Control("|")), 2),
            ('|', _, _) => (Some(Token::Control("|")), 1),
            ('&', Some('&'), _) => (Some(Token::Control("&&")), 2),
            ('&', Some('>'), Some('>')) => (Some(Token::Redirect(">>")), 3),
            ('&', Some('>'), _) => (Some(Token::Redirect(">")), 2),
            ('&', _, _) => (Some(Token::Control("&")), 1),
            (';', _, _) => (Some(Token::Control(";")), 1),
            ('>', Some('>'), _) => (Some(Token::Redirect(">>")), 2),
            ('>', Some('&'), _) => (Some(Token::Redirect(">&")), 2),
            ('>', Some('|'), _) => (Some(Token::Redirect(">")), 2),
            ('>', _, _) => (Some(Token::Redirect(">")), 1),
            ('<', Some('<'), Some('<')) => (Some(Token::Redirect("<<<")), 3),
            ('<', Some('<'), Some('-')) => (Some(Token::Redirect("<<")), 3),
            ('<', Some('<'), _) => (Some(Token::Redirect("<<")), 2),
            ('<', Some('&'), _) => (Some(Token::Redirect("<&")), 2),
            ('<', Some('('), _) => return None,
            ('<', _, _) => (Some(Token::Redirect("<")), 1),
            ('(' | ')' | '`' | '{' | '}', _, _) => return None,
            ('#', _, _) => break,
            _ => (None, 0),
        };
        if let Some(token) = token {
            tokens.push(token);
            i += width;
            continue;
        }
        let mut text = String::new();
        let mut assign = None;
        let mut quoted = false;
        while let Some(c) = at(i) {
            if c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '`') {
                break;
            }
            match c {
                '\'' => {
                    quoted = true;
                    i += 1;
                    loop {
                        match at(i)? {
                            '\'' => break,
                            inner => text.push(inner),
                        }
                        i += 1;
                    }
                    i += 1;
                }
                '"' => {
                    quoted = true;
                    i += 1;
                    loop {
                        match at(i)? {
                            '"' => break,
                            '`' => return None,
                            '$' if at(i + 1) == Some('(') => return None,
                            '\\' => {
                                i += 1;
                                text.push(at(i)?);
                            }
                            inner => text.push(inner),
                        }
                        i += 1;
                    }
                    i += 1;
                }
                '\\' => {
                    text.push(at(i + 1)?);
                    i += 2;
                }
                '$' if at(i + 1) == Some('(') => return None,
                '=' if assign.is_none() && !quoted && is_env_name(&text) => {
                    assign = Some(text.clone());
                    text.push('=');
                    i += 1;
                }
                other => {
                    text.push(other);
                    i += 1;
                }
            }
        }
        // `2>`: a file descriptor, part of the redirection that follows.
        let fd = !quoted
            && !text.is_empty()
            && text.chars().all(|c| c.is_ascii_digit())
            && matches!(at(i), Some('<' | '>'));
        if !fd {
            tokens.push(Token::Word { text, assign });
        }
    }
    Some(tokens)
}

fn summarize(tokens: &[Token]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut segment: Vec<String> = Vec::new();
    let mut has_program = false;
    let mut iter = tokens.iter().peekable();
    let flush = |segment: &mut Vec<String>, parts: &mut Vec<String>| {
        if !segment.is_empty() {
            parts.push(segment.join(" "));
            segment.clear();
        }
    };
    while let Some(token) = iter.next() {
        match token {
            Token::Control(op) => {
                flush(&mut segment, &mut parts);
                has_program = false;
                if parts.last().is_some_and(|last| !is_control(last)) {
                    parts.push((*op).to_owned());
                }
            }
            Token::Redirect(op) => {
                let target = match iter.peek() {
                    Some(Token::Word { text, .. }) => {
                        iter.next();
                        Some(text)
                    }
                    _ => None,
                };
                // Only a file path is shown: never a heredoc delimiter, a
                // here-string or a duplicated descriptor.
                if let (">" | ">>" | "<", Some(path)) = (*op, target) {
                    if is_plain_path(path) {
                        segment.push(format!("{op} {path}"));
                    }
                }
            }
            Token::Word { text, assign } => {
                if has_program {
                    continue;
                }
                match assign {
                    Some(name) => segment.push(format!("{}=…", shown_word(name))),
                    None => {
                        segment.push(program_name(text).unwrap_or_else(|| "…".to_owned()));
                        has_program = true;
                    }
                }
            }
        }
    }
    flush(&mut segment, &mut parts);
    while parts.last().is_some_and(|last| is_control(last)) {
        parts.pop();
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = String::new();
    for part in parts {
        match part.as_str() {
            ";" => out.push(';'),
            op if is_control(op) => {
                out.push(' ');
                out.push_str(op);
            }
            text => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text);
            }
        }
    }
    Some(out)
}

fn is_control(part: &str) -> bool {
    matches!(part, "|" | "||" | "&&" | "&" | ";")
}

/// The first program of a line this parser did not follow, alone.
fn first_program(line: &str) -> Option<String> {
    let word = line.split_whitespace().find(|word| !word.contains('='))?;
    if word.contains(['\'', '"', '\\', '$', '`', '(', ')']) {
        return None;
    }
    program_name(word)
}

/// A program's base name, when it reads like one.
fn program_name(word: &str) -> Option<String> {
    let name = word.rsplit('/').next()?;
    let ok = !name.is_empty()
        && name.chars().count() <= 40
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    ok.then(|| shown_word(name))
}

fn is_env_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text.len() <= 64
}

/// A name or path made only of characters a path uses, with no value in it.
fn is_plain_path(text: &str) -> bool {
    !text.is_empty()
        && text.chars().count() <= TARGET_MAX_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '~' | '+' | '-'))
        && !text.split('/').any(looks_like_token)
}

/// A word shown as is unless it looks like a token: a long run of letters and
/// digits is no program or variable name anyone types.
fn shown_word(word: &str) -> String {
    if looks_like_token(word) {
        "…".to_owned()
    } else {
        word.to_owned()
    }
}

fn looks_like_token(word: &str) -> bool {
    word.len() >= 16
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

/// A tool's name as the runtime gave it when it is an identifier; anything
/// else (a sentence, a command line) becomes `tool`.
pub fn safe_tool_name(raw: &str) -> String {
    let name = raw.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= TOOL_NAME_MAX_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
    if ok {
        name.to_owned()
    } else {
        "tool".to_owned()
    }
}

/// One step in a tool call's life: its start, or its target once known. A
/// call with an id is updated in place by later updates carrying that id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivityUpdate {
    id: Option<String>,
    tool: Option<String>,
    target: Option<SafeTarget>,
}

impl ToolActivityUpdate {
    /// A call started, with its target when already known.
    pub fn call(id: Option<String>, raw_name: &str, target: Option<SafeTarget>) -> Self {
        Self {
            id,
            tool: Some(safe_tool_name(raw_name)),
            target,
        }
    }

    /// The target of the call with this id, or of the call started last.
    pub fn target(id: Option<String>, target: SafeTarget) -> Self {
        Self {
            id,
            tool: None,
            target: Some(target),
        }
    }

    /// Whether it says more than the call's id.
    pub fn carries_detail(&self) -> bool {
        self.tool.is_some() || self.target.is_some()
    }

    pub fn tool(&self) -> Option<&str> {
        self.tool.as_deref()
    }

    pub fn target_text(&self) -> Option<&str> {
        self.target.as_ref().map(SafeTarget::as_str)
    }
}

/// The latest tool calls of a running agent, shown to users. In memory only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentActivity {
    entries: VecDeque<(Option<String>, AuditActivityEntry)>,
}

impl RecentActivity {
    /// Apply one update; `true` when it announced a call not seen before.
    pub fn apply(&mut self, update: &ToolActivityUpdate) -> bool {
        let target = update.target.as_ref().map(|target| target.0.clone());
        if let Some(id) = &update.id {
            if let Some((_, entry)) = self
                .entries
                .iter_mut()
                .find(|(known, _)| known.as_deref() == Some(id.as_str()))
            {
                if let Some(tool) = &update.tool {
                    entry.tool = tool.clone();
                }
                if target.is_some() {
                    entry.target = target;
                }
                return false;
            }
        } else if update.tool.is_none() {
            if let Some((_, last)) = self.entries.back_mut() {
                if last.target.is_none() {
                    last.target = target;
                }
            }
            return false;
        }
        if self.entries.len() == RECENT_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((
            update.id.clone(),
            AuditActivityEntry {
                tool: update.tool.clone().unwrap_or_else(|| "tool".to_owned()),
                target,
                at: chrono::Utc::now(),
            },
        ));
        true
    }

    /// Newest first.
    pub fn snapshot(&self) -> AuditRecentActivity {
        AuditRecentActivity {
            entries: self
                .entries
                .iter()
                .rev()
                .map(|(_, entry)| entry.clone())
                .collect(),
        }
    }
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
