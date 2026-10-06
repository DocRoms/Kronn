//! The latest tool call of a running agent, published for readers other than
//! the client streaming its output.

use crate::models::{AgentActivity, AuditActivityEntry, AuditRecentActivity};

/// Holds the latest activity of one launch; `None` until a tool call starts.
pub type AgentActivitySink = tokio::sync::watch::Sender<Option<AgentActivity>>;

/// Longest target kept, in characters.
const TARGET_MAX_CHARS: usize = 120;

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

/// The most informative field of a tool's JSON input — file path, command,
/// pattern or URL — on one line and truncated.
pub fn tool_input_target(raw_input: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw_input).ok()?;
    let detail = ["file_path", "path", "command", "pattern", "url"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|field| field.as_str()))?
        .replace('\n', " ");
    if detail.chars().count() > TARGET_MAX_CHARS {
        let mut truncated: String = detail.chars().take(TARGET_MAX_CHARS).collect();
        truncated.push('…');
        Some(truncated)
    } else {
        Some(detail)
    }
}

/// Entries kept by [`RecentActivity`].
pub const RECENT_MAX_ENTRIES: usize = 15;
/// Longest tool name, target or thought kept in [`RecentActivity`], in characters.
pub const RECENT_MAX_CHARS: usize = 120;
const TOOL_NAME_MAX_CHARS: usize = 60;
/// Prose kept to find the last line in; older text is dropped.
const PROSE_TAIL_MAX_CHARS: usize = 600;

/// The latest tool calls of a running agent and its last line of prose, shown
/// to users. Every string is sanitized on the way in: one line, secrets
/// redacted, bounded, and a target is a path, pattern or command only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentActivity {
    entries: std::collections::VecDeque<AuditActivityEntry>,
    prose_tail: String,
}

impl RecentActivity {
    pub fn tool_started(&mut self, tool: &str) {
        let Some(tool) = sanitize_line(tool, TOOL_NAME_MAX_CHARS) else {
            return;
        };
        if self.entries.len() == RECENT_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(AuditActivityEntry {
            tool,
            target: None,
            at: chrono::Utc::now(),
        });
        // A tool call ends the sentence before it.
        self.push_prose("\n", true);
    }

    /// Attach a target to the call started last, once.
    pub fn tool_target(&mut self, raw: &str) {
        if let Some(last) = self.entries.back_mut().filter(|last| last.target.is_none()) {
            last.target = sanitize_line(raw, RECENT_MAX_CHARS);
        }
    }

    /// Attach the target found in the call's JSON input.
    pub fn tool_input(&mut self, input: &serde_json::Value) {
        if let Some(target) = input_target(input) {
            self.tool_target(&target);
        }
    }

    /// Append the agent's prose. `fragment` text is concatenated as is; a whole
    /// line gets its own line.
    pub fn push_prose(&mut self, text: &str, fragment: bool) {
        self.prose_tail.push_str(text);
        if !fragment {
            self.prose_tail.push('\n');
        }
        let len = self.prose_tail.chars().count();
        if len > PROSE_TAIL_MAX_CHARS {
            let kept: String = self
                .prose_tail
                .chars()
                .skip(len - PROSE_TAIL_MAX_CHARS)
                .collect();
            // The cut may land inside a word, a secret's tail included.
            self.prose_tail = match kept.find(char::is_whitespace) {
                Some(at) => kept[at..].to_owned(),
                None => String::new(),
            };
        }
    }

    /// Newest first, with the last non-empty line of prose.
    pub fn snapshot(&self) -> AuditRecentActivity {
        // A word still streaming may be a secret's first characters, too short
        // yet for the redaction to recognize.
        let tail = self.prose_tail.as_str();
        let settled = if tail.ends_with(char::is_whitespace) {
            tail
        } else {
            tail.rfind(char::is_whitespace).map_or("", |at| &tail[..at])
        };
        let thought = settled
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .and_then(|line| sanitize_line(line, RECENT_MAX_CHARS));
        AuditRecentActivity {
            entries: self.entries.iter().rev().cloned().collect(),
            thought,
        }
    }
}

/// The short target of a tool's JSON input: `"pattern" path` for a search,
/// else its file path, command, URL or query. Every other argument — file
/// contents, edits, environment — is left out.
pub fn input_target(input: &serde_json::Value) -> Option<String> {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| input.get(*key).and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let path = field(&["file_path", "notebook_path", "path"]);
    if let Some(pattern) = field(&["pattern"]) {
        return Some(match path {
            Some(path) => format!("\"{pattern}\" {path}"),
            None => format!("\"{pattern}\""),
        });
    }
    path.or_else(|| field(&["command", "cmd", "url", "query"]))
        .map(str::to_owned)
}

/// One line of untrusted text made safe to show: first line only (a heredoc's
/// body never shows), URL queries cut, environment assignments and secret-like
/// flag values masked, then [`crate::core::redact::redact_for_audit_artifact`],
/// then truncated on a word boundary. `None` when nothing is left.
pub fn sanitize_line(raw: &str, max_chars: usize) -> Option<String> {
    let mut line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_owned();
    // Truncated upstream: its last word may be the start of a secret.
    if let Some(cut) = line.strip_suffix('…') {
        line = match cut.trim_end().rsplit_once(char::is_whitespace) {
            Some((head, _)) => head.to_owned(),
            None => cut.to_owned(),
        };
    }
    let mut mask_next = false;
    let words: Vec<String> = line
        .split_whitespace()
        .map(|word| {
            if std::mem::take(&mut mask_next) {
                return MASK.to_owned();
            }
            mask_word(word, &mut mask_next)
        })
        .collect();
    let (redacted, _) = crate::core::redact::redact_for_audit_artifact(&words.join(" "));
    let redacted = redacted.trim();
    if redacted.is_empty() {
        return None;
    }
    if redacted.chars().count() <= max_chars {
        return Some(redacted.to_owned());
    }
    let head: String = redacted.chars().take(max_chars).collect();
    let head = match head.rsplit_once(' ') {
        Some((before, _)) if before.chars().count() >= max_chars / 2 => before.to_owned(),
        _ => head,
    };
    Some(format!("{}…", head.trim_end()))
}

/// The redaction helper's own marker, so a second pass leaves it alone.
const MASK: &str = "***REDACTED***";
const SECRET_FLAG_WORDS: [&str; 7] = [
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "auth",
    "credential",
];

fn mask_word(word: &str, mask_next: &mut bool) -> String {
    // `NAME=value` sets an environment variable: its value never shows.
    if let Some((name, _)) = word.split_once('=') {
        let bare = name.trim_start_matches('-');
        let is_env = !name.starts_with('-')
            && bare
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if is_env || (name.starts_with('-') && secret_flag(bare)) {
            return format!("{name}={MASK}");
        }
    }
    if word.starts_with('-') && secret_flag(word.trim_start_matches('-')) {
        *mask_next = true;
        return word.to_owned();
    }
    // A URL's query and fragment carry tokens; its path is enough.
    if word.contains("://") {
        if let Some(cut) = word.find(['?', '#']) {
            return word[..cut].to_owned();
        }
    }
    word.to_owned()
}

fn secret_flag(flag: &str) -> bool {
    let flag = flag.to_ascii_lowercase();
    !flag.is_empty() && SECRET_FLAG_WORDS.iter().any(|word| flag.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_completes_the_call_started_last_and_never_invents_one() {
        let (sink, rx) = tokio::sync::watch::channel(None);
        tool_target(Some(&sink), "orphan".into());
        assert_eq!(*rx.borrow(), None, "no call started, nothing to complete");

        tool_started(Some(&sink), "Read");
        tool_target(Some(&sink), "src/lib.rs".into());
        let first = rx.borrow().clone().unwrap();
        assert_eq!(
            (first.tool.as_str(), first.target.as_deref()),
            ("Read", Some("src/lib.rs"))
        );

        tool_started(Some(&sink), "Bash");
        let second = rx.borrow().clone().unwrap();
        assert_eq!((second.tool.as_str(), second.target), ("Bash", None));
        assert!(second.at >= first.at);
        assert_eq!((first.calls, second.calls), (1, 2));
    }

    #[test]
    fn every_call_is_counted_even_when_nobody_reads_in_between() {
        let (sink, rx) = tokio::sync::watch::channel(None);
        for _ in 0..50 {
            tool_started(Some(&sink), "Read");
            tool_target(Some(&sink), "src/é.rs".into());
        }
        assert_eq!(rx.borrow().as_ref().unwrap().calls, 50);
    }

    #[test]
    fn the_target_is_the_informative_field_on_one_line_and_bounded() {
        assert_eq!(
            tool_input_target(r#"{"file_path":"docs/é.md","content":"x"}"#).as_deref(),
            Some("docs/é.md")
        );
        assert_eq!(
            tool_input_target(r#"{"command":"cargo test\n--lib"}"#).as_deref(),
            Some("cargo test --lib")
        );
        let long = format!(r#"{{"pattern":"{}"}}"#, "é".repeat(200));
        let target = tool_input_target(&long).unwrap();
        assert_eq!(target.chars().count(), TARGET_MAX_CHARS + 1);
        assert!(target.ends_with('…'));
        assert_eq!(tool_input_target(r#"{"todos":[]}"#), None);
        assert_eq!(tool_input_target("{not json"), None);
    }

    #[test]
    fn the_recent_actions_are_bounded_newest_first_and_short() {
        let mut recent = RecentActivity::default();
        assert_eq!(recent.snapshot(), AuditRecentActivity::default());
        for call in 0..40 {
            recent.tool_started(&format!("Read{call}"));
            recent.tool_target(&format!("src/{}.rs", "é".repeat(300)));
        }
        let shown = recent.snapshot();
        assert_eq!(shown.entries.len(), RECENT_MAX_ENTRIES);
        assert_eq!(shown.entries[0].tool, "Read39");
        assert_eq!(shown.entries[RECENT_MAX_ENTRIES - 1].tool, "Read25");
        let target = shown.entries[0].target.as_deref().unwrap();
        assert_eq!(target.chars().count(), RECENT_MAX_CHARS + 1);
        assert!(target.ends_with('…'));

        recent.tool_started(&"T".repeat(500));
        assert!(recent.snapshot().entries[0].tool.chars().count() <= TOOL_NAME_MAX_CHARS + 1);
        recent.push_prose(&"word ".repeat(400), true);
        let thought = recent.snapshot().thought.unwrap();
        assert!(thought.chars().count() <= RECENT_MAX_CHARS + 1, "{thought}");

        // A token still streaming never shows, whole or in part.
        let mut recent = RecentActivity::default();
        recent.push_prose("The key is sk-ant-api03-abc", true);
        assert_eq!(recent.snapshot().thought.as_deref(), Some("The key is"));
        recent.push_prose("defghijklmnopqrstuvwxyz0123 ok", true);
        let thought = recent.snapshot().thought.unwrap();
        assert!(!thought.contains("abcdef"), "{thought}");
        // Nor the end of one cut off by the bound.
        let mut recent = RecentActivity::default();
        recent.push_prose(
            &format!("ghp_{} tail end\n", "a".repeat(PROSE_TAIL_MAX_CHARS)),
            true,
        );
        assert_eq!(recent.snapshot().thought.as_deref(), Some("tail end"));
    }

    #[test]
    fn a_target_never_carries_a_secret_an_env_value_or_a_heredoc_body() {
        let shown = |raw: &str| sanitize_line(raw, RECENT_MAX_CHARS).unwrap();
        let bash = shown("OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz npm test");
        assert_eq!(bash, "OPENAI_API_KEY=***REDACTED*** npm test");
        let bash =
            shown("git push https://x:ghp_abcdefghijklmnopqrstuvwxyz0123456789@github.com/o/r");
        assert!(!bash.contains("ghp_abcdefghijklmnopqrstuvwxyz"), "{bash}");
        let bash = shown("echo sk-abcdefghijklmnopqrstuvwxyz0123 | gh auth --with-token");
        assert!(
            !bash.contains("sk-abcdefghijklmnopqrstuvwxyz0123"),
            "{bash}"
        );
        assert_eq!(
            shown("mytool --api-key hunter2 --verbose"),
            "mytool --api-key ***REDACTED*** --verbose"
        );
        assert_eq!(
            shown("mytool --password=hunter2"),
            "mytool --password=***REDACTED***"
        );
        assert_eq!(
            shown("curl https://h.io/p?access_token=abc#f"),
            "curl https://h.io/p"
        );
        assert_eq!(
            shown("cat > .env <<EOF\nDB_PASSWORD=hunter2\nEOF"),
            "cat > .env <<EOF"
        );
        // A target truncated upstream loses its last, possibly partial, word.
        assert_eq!(
            shown("deploy --to prod sk-ant-api03-abc…"),
            "deploy --to prod"
        );
        // A long command is cut on a word, after redaction.
        let long = format!(
            "npm test {} sk-abcdefghijklmnopqrstuvwxyz0123",
            "a ".repeat(70)
        );
        let cut = shown(&long);
        assert!(
            cut.chars().count() <= RECENT_MAX_CHARS + 1 && cut.ends_with('…'),
            "{cut}"
        );
        assert!(!cut.contains("sk-abc"), "{cut}");
        assert_eq!(sanitize_line("   \n  ", RECENT_MAX_CHARS), None);
    }

    #[test]
    fn the_input_target_is_a_path_pattern_or_command_only() {
        let target = |v: serde_json::Value| input_target(&v);
        assert_eq!(
            target(serde_json::json!({"file_path": "docs/a.md", "content": "secret"})).as_deref(),
            Some("docs/a.md")
        );
        assert_eq!(
            target(serde_json::json!({"pattern": "useAuth", "path": "src/"})).as_deref(),
            Some("\"useAuth\" src/")
        );
        assert_eq!(
            target(serde_json::json!({"old_string": "a", "new_string": "b"})),
            None
        );
    }
}
