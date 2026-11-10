//! KT-1656 — the false-positive rate of the redaction every Agent message goes
//! through on insert (`discussions::mask_agent_output`), which is irreversible.
//!
//! The measurement is ignored by default. It runs the exact insert path over
//! every Agent message of a **copy** of a Kronn database, and/or over text files
//! cut into message-sized chunks (a corpus of prose, code and hashes), and lists
//! every change so each can be classified:
//!
//! ```text
//! KRONN_REDACT_DB=<copy of kronn.db> \
//! KRONN_REDACT_DIRS=<dir>:<dir> \
//! KRONN_REDACT_OUT=<report file> \
//!   cargo nextest run --lib --run-ignored only --no-capture \
//!     -E 'test(/insert_redaction_measure/)'
//! ```
//!
//! The report names the audit redaction too (what the insert path used before it
//! was narrowed), so the false positives that were removed stay visible. Never
//! point it at the live database: copy it first. The report holds the lines the
//! redaction changed, secrets included: keep it where the database copy is.

use std::path::{Path, PathBuf};

use crate::core::redact::{redact_agent_output, redact_for_audit_artifact};

const CHUNK_CHARS: usize = 3000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "rs", "ts", "tsx", "js", "toml", "json", "sql", "yml", "yaml", "txt", "sh", "py", "html",
    "css", "conf",
];
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".kronn",
    ".pnpm-store",
    "dist",
];

/// Message-sized pieces of a text: blank-line separated blocks, grouped up to
/// [`CHUNK_CHARS`].
fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for block in text.split("\n\n") {
        if !current.is_empty() && current.len() + block.len() > CHUNK_CHARS {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(block);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn text_files(root: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                text_files(&path, into);
            }
        } else if kind.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext))
            && entry
                .metadata()
                .is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES)
        {
            into.push(path);
        }
    }
}

/// The lines that differ: what was cut from the common head and tail, each
/// clipped to a window around the first character that changed.
fn changed_lines(before: &str, after: &str) -> Vec<String> {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    let head = before
        .iter()
        .zip(&after)
        .take_while(|(left, right)| left == right)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let removed = &before[head..before.len() - tail];
    let added = &after[head..after.len() - tail];
    let window = |line: &str, other: Option<&&str>| -> String {
        let pivot = other.map_or(0, |other| {
            line.chars()
                .zip(other.chars())
                .take_while(|(left, right)| left == right)
                .count()
        });
        line.chars()
            .skip(pivot.saturating_sub(60))
            .take(200)
            .collect()
    };
    let mut lines: Vec<String> = Vec::new();
    for (index, line) in removed.iter().enumerate() {
        lines.push(format!("  - {}", window(line, added.get(index))));
    }
    for (index, line) in added.iter().enumerate() {
        lines.push(format!("  + {}", window(line, removed.get(index))));
    }
    lines
}

#[derive(Default)]
struct Tally {
    scanned: usize,
    /// Changed by the insert path.
    changed: usize,
    /// Changed by the audit redaction (the insert path's previous behaviour).
    audit_changed: usize,
    report: Vec<String>,
}

impl Tally {
    fn scan(&mut self, label: &str, text: &str) {
        self.scanned += 1;
        let audit = redact_for_audit_artifact(text).0;
        if audit != text {
            self.audit_changed += 1;
        }
        let stored = redact_agent_output(text);
        if stored != text {
            self.changed += 1;
            self.report.push(format!("[{label}]"));
            self.report.extend(changed_lines(text, &stored));
        }
    }
}

/// Needs a copy of a database or a corpus: see the module doc.
#[test]
#[ignore = "measurement: needs KRONN_REDACT_DB and/or KRONN_REDACT_DIRS"]
fn insert_redaction_measure() {
    let mut tally = Tally::default();
    let mut sources = 0;

    if let Ok(path) = crate::core::child_env::var("KRONN_REDACT_DB") {
        sources += 1;
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open the database copy read-only");
        let mut statement = conn
            .prepare("SELECT id, content FROM messages WHERE role = 'Agent'")
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        for row in rows {
            let (id, content) = row.unwrap();
            tally.scan(&format!("message {id}"), &content);
        }
    }

    if let Ok(dirs) = crate::core::child_env::var("KRONN_REDACT_DIRS") {
        sources += 1;
        for root in std::env::split_paths(&dirs) {
            let mut files = Vec::new();
            text_files(&root, &mut files);
            files.sort();
            for file in files {
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                for (index, chunk) in chunks(&text).iter().enumerate() {
                    tally.scan(&format!("{}#{index}", file.display()), chunk);
                }
            }
        }
    }
    assert!(sources > 0, "set KRONN_REDACT_DB and/or KRONN_REDACT_DIRS");

    let summary = format!(
        "insert path: {} of {} message(s) changed; audit redaction (before): {}",
        tally.changed, tally.scanned, tally.audit_changed
    );
    eprintln!("{summary}");
    if let Ok(out) = crate::core::child_env::var("KRONN_REDACT_OUT") {
        let mut report = vec![summary];
        report.extend(tally.report);
        std::fs::write(out, report.join("\n")).expect("write the report");
    }
}

/// What a reply is made of — prose, code, hashes, placeholders, the names of
/// secrets without a secret — and must be stored exactly as written. Every entry
/// came from the corpus measurement (`insert_redaction_measure`) or from the
/// shapes it showed; each was rewritten by the audit redaction.
const AS_WRITTEN: &[&str] = &[
    // Code that names a token or a password without holding one.
    "let token = lexer.next_token();",
    "const password = await bcrypt.hash(request.password, 12);",
    "self.api_key = config.api_key.clone();",
    "token: Option<String>,",
    "if (token === undefined) return;",
    "fn verify_pin(pin: u32) -> bool { pin == self.pin }",
    "password_hash = hash_password(raw_password)",
    "export const secret = process.env.APP_SECRET;",
    "headers.insert(\"authorization\", format!(\"Bearer {token}\"));",
    // Configuration and docs that show where a secret goes.
    "API_KEY=<your-api-key>",
    "GITHUB_TOKEN=${{ secrets.GITHUB_TOKEN }}",
    "export OPENAI_API_KEY=$OPENAI_API_KEY",
    "password: changeme",
    "\"token\": \"your-token-here\"",
    "\"password\": \"string\"",
    "api_key: \"xxxxxxxxxxxx\"",
    "Authorization: Bearer $TOKEN",
    "token: ***REDACTED***",
    // Found by the corpus measurement: placeholders and ellipses, the name of an
    // environment variable, an i18n key, a pagination cursor, a script with no
    // spaces, the word "pin", a reference to a secret, a header cut by a line.
    "disc_join({token: \"kr-join-...\"}) first",
    "agents proposed `apikey: 'VOTRE_API_KEY'` placeholders",
    "SLACK_BOT_TOKEN: 'xoxb-xxxxxxxxxxxx',",
    "// last DEDUP_WINDOW_MS, drop the new fire. Pin: TD-20260510-multi-",
    "\"github.state.connected_stored_token\": \"已通过保存的令牌连接\",",
    "token: 'disc.inviteTabToken',",
    "`Authorization: Bearer\n//! <access_token>` header on every request",
    "The credential of an `Authorization: Bearer …` header",
    "The machine's token: `GH_TOKEN` / `GITHUB_TOKEN` of the backend",
    "'Bearer header (Authorization: Bearer ...)'",
    "{\"token\": \"secret://DEPLOY_PASSWORD\"}",
    "\"nextPageToken\": \"abc123\",",
    "masked (`\"GITHUB_PERSONAL_ACCESS_TOKEN\": \"t0ken…\"`, `GEMINI_API_KEY=…`)",
    "export const FOCUS_RING_TOKEN = '--kr-accent-ink';",
    "token_help: Some(\"Connection string: postgresql://user:pass@host:5432/db\")",
    r#"format!("Authorization: Bearer \"{TOKEN}\"")"#,
    "The SAFETY-CRITICAL pin: `test_does_not_auto_fill_source_session_id` guards the fix",
    "the secret: `refresh_token_ttl_seconds` is read once",
    // Found by the measurement over the 16,004 Agent messages of a real database
    // (KT-1656): a room invitation posted on purpose, and the names of things.
    "disc_join({token: \"kr-join-0123456789abcdef0123456789abcdef\"}) to enter the room",
    "Run `disc_join({token: 'kr-join-fedcba9876543210fedcba9876543210'})` within 10 minutes",
    // Prose.
    "The token budget is 5000 and the password policy asks for 12 characters.",
    "password: required, 12 characters minimum",
    "pin: the number of the pin on the board",
    "secret: see the vault note for how a secret reaches an agent",
    // Hashes and identifiers, which carry no secret name.
    "commit 3f786850e387550fdab836ed7e6dc881de23001b fixes the token refresh",
    "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    "id 123e4567-e89b-12d3-a456-426614174000 for the discussion",
    "![logo](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==)",
    "\"integrity\": \"sha512-Xr3GkEXQYHF2qZ3Hq0O1m3m4eJmZrYw6qk7f0Zr5r0ZyQ3nJt2kTQ6o1Qj8m1gVPWk4N0a3t2w9c1r8h5y7z6A==\"",
    "checksum: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
];

/// Round 4 stored these as written (they were the false positives of the real
/// database measurement); round 5 flips them. An identifier, a route, the name
/// repeated and a short number are also what a real password, token or PIN can
/// be, so masking them is the accepted cost of zero defect on secrets.
const EXPECTED_MASKED: &[&str] = &[
    "{\"token\":\"initGeneration\",\"sessionId\":\"s-1\"}",
    "the cancel token: `session/cancel`",
    "ce n'est **pas** un secret : `TooLarge`",
    "const CHANNEL_TOKEN = 'channeltoken';",
    "Example: PIN=1234",
];

#[test]
fn an_identifier_under_a_secret_name_is_masked_the_accepted_cost() {
    for sample in EXPECTED_MASKED {
        let stored = redact_agent_output(sample);
        assert_ne!(stored, *sample, "stored as written: {sample}");
        assert!(stored.contains("REDACTED"), "{sample}\n   -> {stored}");
    }
}

#[test]
fn what_a_reply_is_made_of_is_stored_as_written() {
    let rewritten: Vec<String> = AS_WRITTEN
        .iter()
        .filter_map(|sample| {
            let stored = redact_agent_output(sample);
            (stored != *sample).then(|| format!("{sample}\n   -> {stored}"))
        })
        .collect();
    assert!(
        rewritten.is_empty(),
        "stored differently from what the agent wrote:\n{}",
        rewritten.join("\n")
    );
}

/// The point of the redaction: a credential file printed whole is masked, in
/// the shapes a worker prints it.
#[test]
fn a_printed_credential_file_is_masked_on_insert() {
    const SECRET: &str = "t0kenNotShapedLikeAVendor01";
    let printed = [
        // `cat ~/.gemini/settings.json`, as a reply quotes it.
        format!(
            "```json\n{{\"mcpServers\": {{\"github\": {{\"env\": {{\"GITHUB_PERSONAL_ACCESS_TOKEN\": \"{SECRET}\", \"LOG_LEVEL\": \"debug\"}}}}}}}}\n```"
        ),
        // The same file as the whole message.
        format!(
            "{{\"mcpServers\": {{\"github\": {{\"env\": {{\"GITHUB_PERSONAL_ACCESS_TOKEN\": \"{SECRET}\"}}}}}}}}"
        ),
        format!("GEMINI_API_KEY={SECRET}"),
        format!("export GITHUB_TOKEN={SECRET}"),
        format!("password: \"{SECRET}\""),
        format!("{{\\\"apiKey\\\": \\\"{SECRET}\\\"}}"),
    ];
    for text in printed {
        let stored = redact_agent_output(&text);
        assert!(!stored.contains(SECRET), "{text}\n   -> {stored}");
        assert!(stored.contains("REDACTED"), "{text}\n   -> {stored}");
    }
    // The settings that surround it stay readable.
    let stored = redact_agent_output(&format!(
        "{{\"env\": {{\"GITHUB_PERSONAL_ACCESS_TOKEN\": \"{SECRET}\", \"LOG_LEVEL\": \"debug\"}}}}"
    ));
    assert!(stored.contains("\"LOG_LEVEL\": \"debug\""), "{stored}");
    serde_json::from_str::<serde_json::Value>(&stored).unwrap();
}
