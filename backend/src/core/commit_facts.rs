//! KT-1387 — the commit conventions of a repository, as observed.
//!
//! An agent asked to document "the commit convention" writes the one it knows
//! (Conventional Commits `type(scope): subject`) or the one a tool name hints
//! at ("Commitizen enforces every commit"), not the one the history shows.
//! [`CommitFacts::detect`] computes the facts instead, from
//! `git log -n 200` and the repository's own tooling: the share of each
//! subject shape (colon after the type or not), ticket prefixes, types,
//! scopes, language, trailers, and which commit tooling exists and whether a
//! hook or CI step actually enforces it. [`CommitFacts::render_block`] hands
//! them to the step as ground truth.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Subjects read from the history.
const SAMPLE: &str = "200";
/// Below this many subjects there is no pattern to observe.
const MIN_HISTORY: usize = 10;
const MAX_READ: u64 = 256 * 1024;
const SHOWN: usize = 12;

/// The shape of one subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `type(scope): subject`
    ScopedColon,
    /// `type(scope) subject`
    ScopedPlain,
    /// `type: subject`
    BareColon,
    Free,
}

#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    form: Form,
    kind: Option<String>,
    scope: Option<String>,
    /// Ticket key and whether it leads the subject (else it follows the type).
    ticket: Option<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub prefix: String,
    pub count: usize,
    pub leading: usize,
    pub after_type: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// A hook or CI step that runs it (where).
    Enforced(String),
    /// A command people run by hand; nothing forces it.
    OptIn,
    /// Configured, but no hook or CI step runs it.
    ConfiguredOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    /// `path:line` of what shows it.
    pub evidence: Vec<String>,
    pub enforcement: Enforcement,
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommitFacts {
    /// Merge commits, left out of the shape statistics.
    pub merges: usize,
    pub scoped_colon: usize,
    pub scoped_plain: usize,
    pub bare_colon: usize,
    pub free_form: usize,
    pub types: Vec<(String, usize)>,
    pub scopes: Vec<(String, usize)>,
    pub tickets: Vec<Ticket>,
    pub french: usize,
    pub english: usize,
    pub signed_off: usize,
    pub co_authored: usize,
    /// Short sha and subject of commits in the dominant shape.
    pub examples: Vec<(String, String)>,
    pub tooling: Vec<Tool>,
}

impl CommitFacts {
    /// Subjects analysed (merges excluded).
    pub fn analysed(&self) -> usize {
        self.scoped_colon + self.scoped_plain + self.bare_colon + self.free_form
    }

    pub fn detect(project: &Path) -> Self {
        let mut facts = Self::from_log(&git_log(project));
        facts.tooling = detect_tooling(project);
        facts
    }

    /// `records`: one `sha \x1f subject \x1f signed-off \x1f co-authored` per commit.
    fn from_log(log: &str) -> Self {
        let mut facts = Self::default();
        let mut types: BTreeMap<String, usize> = BTreeMap::new();
        let mut scopes: BTreeMap<String, usize> = BTreeMap::new();
        let mut tickets: BTreeMap<String, Ticket> = BTreeMap::new();
        let mut parsed: Vec<(String, String, Form)> = Vec::new();
        for record in log.split('\u{1e}').filter(|r| !r.trim().is_empty()) {
            let mut fields = record.trim_start_matches('\n').split('\u{1f}');
            let sha = fields.next().unwrap_or("").trim().to_string();
            let subject = fields.next().unwrap_or("").trim().to_string();
            let trailer = |value: Option<&str>| {
                value.is_some_and(|v| !v.trim().is_empty() && !v.trim_start().starts_with("%("))
            };
            let signed = trailer(fields.next());
            let co_authored = trailer(fields.next());
            if subject.is_empty() {
                continue;
            }
            if subject.starts_with("Merge ") {
                facts.merges += 1;
                continue;
            }
            facts.signed_off += usize::from(signed);
            facts.co_authored += usize::from(co_authored);
            let p = parse_subject(&subject);
            match p.form {
                Form::ScopedColon => facts.scoped_colon += 1,
                Form::ScopedPlain => facts.scoped_plain += 1,
                Form::BareColon => facts.bare_colon += 1,
                Form::Free => facts.free_form += 1,
            }
            if let Some(kind) = &p.kind {
                *types.entry(kind.clone()).or_default() += 1;
            }
            if let Some(scope) = &p.scope {
                *scopes.entry(scope.clone()).or_default() += 1;
            }
            if let Some((prefix, leading)) = &p.ticket {
                let t = tickets.entry(prefix.clone()).or_insert_with(|| Ticket {
                    prefix: prefix.clone(),
                    count: 0,
                    leading: 0,
                    after_type: 0,
                });
                t.count += 1;
                if *leading {
                    t.leading += 1;
                } else {
                    t.after_type += 1;
                }
            }
            match language_of(&subject) {
                Some(true) => facts.french += 1,
                Some(false) => facts.english += 1,
                None => {}
            }
            parsed.push((sha, subject, p.form));
        }
        facts.types = ranked(types);
        facts.scopes = ranked(scopes);
        facts.tickets = {
            let mut all: Vec<Ticket> = tickets.into_values().collect();
            all.sort_by(|a, b| b.count.cmp(&a.count).then(a.prefix.cmp(&b.prefix)));
            all
        };
        if let Some(form) = facts.dominant_form() {
            facts.examples = parsed
                .into_iter()
                .filter(|(_, _, f)| *f == form)
                .take(3)
                .map(|(sha, subject, _)| (sha, subject))
                .collect();
        }
        facts
    }

    fn dominant_form(&self) -> Option<Form> {
        [
            (Form::ScopedColon, self.scoped_colon),
            (Form::ScopedPlain, self.scoped_plain),
            (Form::BareColon, self.bare_colon),
            (Form::Free, self.free_form),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map(|(form, _)| form)
    }

    /// The prompt block; `commits_doc` is the path of the commit-conventions
    /// document the step writes.
    pub fn render_block(&self, commits_doc: &str) -> String {
        let total = self.analysed();
        let mut out = String::from(
            "## Commit facts — computed by Kronn (ground truth: describe these, never contradict them)\n\n",
        );
        if total < MIN_HISTORY {
            out.push_str(&format!(
                "Source: `git log -n {SAMPLE}`. Only {total} usable commit subject(s) in the history: no pattern can be observed. \
                 In `{commits_doc}` write `Not formalized — ask before assuming.` for the message convention; never write Conventional Commits from habit.\n"
            ));
        } else {
            let pct = |n: usize| n * 100 / total;
            out.push_str(&format!(
                "Source: `git log -n {SAMPLE} --format=%s` — {total} subjects analysed, {} merge commit(s) left out.\n\n",
                self.merges
            ));
            out.push_str("Subject shape observed:\n");
            for (label, n) in [
                ("`type(scope) subject` — NO colon", self.scoped_plain),
                ("`type(scope): subject` — with colon", self.scoped_colon),
                ("`type: subject` — with colon, no scope", self.bare_colon),
                ("free-form (no type prefix)", self.free_form),
            ] {
                out.push_str(&format!("- {label}: {n} of {total} ({}%)\n", pct(n)));
            }
            let with = self.scoped_colon + self.bare_colon;
            let without = self.scoped_plain;
            if with + without > 0 {
                out.push_str(&format!(
                    "Colon after the type or scope: written in {with} subject(s), absent in {without}. Write the form the history uses, colon or not.\n"
                ));
            }
            if !self.examples.is_empty() {
                out.push_str("Examples of the dominant shape:");
                for (sha, subject) in &self.examples {
                    out.push_str(&format!(" `{subject}` [src: commit: {sha}];"));
                }
                out.push('\n');
            }
            out.push_str(&format!("Ticket keys: {}\n", self.tickets_line()));
            out.push_str(&format!("Types seen: {}\n", list_counts(&self.types)));
            out.push_str(&format!("Scopes seen: {}\n", list_counts(&self.scopes)));
            out.push_str(&format!(
                "Language of the subjects: {}\n",
                self.language_line(total)
            ));
            out.push_str(&format!(
                "Trailers in the history: `Signed-off-by` on {} of {total}; `Co-authored-by` on {} of {total}.\n",
                self.signed_off, self.co_authored
            ));
        }
        out.push_str("\nCommit tooling found in the repository:\n");
        if self.tooling.is_empty() {
            out.push_str("- none (no commitizen, commitlint, gitlint, commit-msg hook, CI message check or Makefile commit target)\n");
        }
        for tool in &self.tooling {
            let status = match &tool.enforcement {
                Enforcement::Enforced(by) => format!("ENFORCED by {by}"),
                Enforcement::OptIn => "opt-in, run by hand — nothing enforces it".to_string(),
                Enforcement::ConfiguredOnly => {
                    "configured, but no commit-msg hook or CI step runs it — not enforced"
                        .to_string()
                }
            };
            let evidence = tool
                .evidence
                .iter()
                .map(|e| format!("[src: file: {e}]"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("- {} {evidence}: {status}", tool.name));
            if let Some(note) = &tool.note {
                out.push_str(&format!(" ({note})"));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "\nRules for `{commits_doc}`:\n\
             1. Describe the OBSERVED subject shape above, with its colon or absence of colon, its language and its ticket keys. Do not write the standard Conventional Commits form unless it is what the history shows.\n\
             2. Say a convention is enforced ONLY for a tool above marked ENFORCED (a hook or CI step). An opt-in or configured-only tool is described as exactly that.\n\
             3. Read every tool file listed above. If one defines a closed list of types or scopes, write the full lists with a citation.\n\
             4. State the attribution and sign-off policy only from the trailer counts and from what the repository's own docs say; when neither establishes one, write `Not formalized — ask before assuming.`\n\
             5. Replace every `{{{{...}}}}` placeholder and remove the template banner and `<!-- Fill: -->` hints of `{commits_doc}`: a file that still holds the template fails this step.\n"
        ));
        out
    }

    fn tickets_line(&self) -> String {
        if self.tickets.is_empty() {
            return "none seen".into();
        }
        self.tickets
            .iter()
            .take(SHOWN)
            .map(|t| {
                let place = match (t.leading, t.after_type) {
                    (l, 0) if l > 0 => "leading the subject".to_string(),
                    (0, a) if a > 0 => "after the type".to_string(),
                    (l, a) => format!("leading {l}, after the type {a}"),
                };
                format!("`{}-<number>` on {} ({place})", t.prefix, t.count)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn language_line(&self, total: usize) -> String {
        let decided = self.french + self.english;
        if decided == 0 {
            return "undetermined".into();
        }
        let fr = self.french * 100 / total;
        let en = self.english * 100 / total;
        let verdict = if self.french * 10 >= decided * 7 {
            "mostly French"
        } else if self.english * 10 >= decided * 7 {
            "mostly English"
        } else {
            "mixed French and English"
        };
        format!("{verdict} (French {fr}%, English {en}%, the rest undetermined)")
    }
}

fn ranked(counts: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut all: Vec<(String, usize)> = counts.into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    all
}

fn list_counts(items: &[(String, usize)]) -> String {
    if items.is_empty() {
        return "none".into();
    }
    let shown = items
        .iter()
        .take(SHOWN)
        .map(|(name, n)| format!("`{name}` ({n})"))
        .collect::<Vec<_>>()
        .join(", ");
    match items.len().saturating_sub(SHOWN) {
        0 => shown,
        more => format!("{shown}, … {more} more"),
    }
}

fn git_log(project: &Path) -> String {
    let format = "--format=%x1e%h%x1f%s%x1f%(trailers:key=Signed-off-by,valueonly,separator=%x2C)%x1f%(trailers:key=Co-authored-by,valueonly,separator=%x2C)";
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(project)
        .args(["log", "-n", SAMPLE, format])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// `ABC-123` at the start of `s`, optionally in brackets: the key and the
/// bytes it takes.
fn ticket_at(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let bracket = bytes.first() == Some(&b'[');
    let start = usize::from(bracket);
    if !bytes.get(start)?.is_ascii_uppercase() {
        return None;
    }
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_uppercase() || bytes[i].is_ascii_digit()) {
        i += 1;
    }
    if !(2..=10).contains(&(i - start)) || bytes.get(i) != Some(&b'-') {
        return None;
    }
    let prefix = s[start..i].to_string();
    // `UTF-8`, `SHA-256`, `ISO-8601`: standards, not tracker projects.
    if [
        "UTF", "ISO", "SHA", "RFC", "PSR", "CVE", "TLS", "AES", "RSA", "HTTP", "IPV",
    ]
    .contains(&prefix.as_str())
    {
        return None;
    }
    i += 1;
    let digits = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits {
        return None;
    }
    if bracket {
        if bytes.get(i) != Some(&b']') {
            return None;
        }
        i += 1;
    }
    Some((prefix, i))
}

fn parse_subject(subject: &str) -> Parsed {
    let mut rest = subject.trim();
    let mut ticket = None;
    if let Some((prefix, used)) = ticket_at(rest) {
        ticket = Some((prefix, true));
        rest = rest[used..].trim_start_matches(|c: char| c == ':' || c == '-' || c.is_whitespace());
    }
    let free = |ticket| Parsed {
        form: Form::Free,
        kind: None,
        scope: None,
        ticket,
    };
    let kind_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_lowercase() || *b == b'-')
        .count();
    if kind_len == 0 || kind_len > 14 {
        return free(ticket);
    }
    let kind = &rest[..kind_len];
    let mut after = &rest[kind_len..];
    let mut scope = None;
    if let Some(inner) = after.strip_prefix('(') {
        match inner.find(')') {
            Some(close) if (1..=40).contains(&close) => {
                scope = Some(inner[..close].to_string());
                after = &inner[close + 1..];
            }
            _ => return free(ticket),
        }
    }
    after = after.strip_prefix('!').unwrap_or(after);
    let (colon, tail) = match after.strip_prefix(':') {
        Some(tail) => (true, tail),
        None => (false, after),
    };
    if !colon && !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
        return free(ticket);
    }
    let form = match (scope.is_some(), colon) {
        (true, true) => Form::ScopedColon,
        (true, false) => Form::ScopedPlain,
        (false, true) => Form::BareColon,
        // A lowercase word then a space is prose, not a type.
        (false, false) => return free(ticket),
    };
    if ticket.is_none() {
        if let Some((prefix, _)) = ticket_at(tail.trim_start()) {
            ticket = Some((prefix, false));
        }
    }
    Parsed {
        form,
        kind: Some(kind.to_string()),
        scope,
        ticket,
    }
}

/// `Some(true)` French, `Some(false)` English, `None` when the words do not say.
fn language_of(subject: &str) -> Option<bool> {
    const FRENCH: &[&str] = &[
        "le",
        "la",
        "les",
        "des",
        "du",
        "de",
        "un",
        "une",
        "et",
        "pour",
        "avec",
        "sans",
        "sur",
        "dans",
        "ajout",
        "ajoute",
        "ajouter",
        "correction",
        "corrige",
        "corriger",
        "mise",
        "jour",
        "suppression",
        "supprime",
        "retire",
        "est",
        "au",
        "aux",
        "ce",
        "cette",
        "par",
        "pas",
        "nouvelle",
        "nouveau",
        "modification",
        "modifie",
        "ajustement",
        "gestion",
        "affichage",
    ];
    const ENGLISH: &[&str] = &[
        "the", "and", "for", "with", "add", "adds", "added", "update", "updates", "updated",
        "remove", "removes", "removed", "to", "of", "in", "on", "from", "use", "make", "when",
        "now", "instead", "handle", "support", "improve", "bump", "is", "not", "new", "after",
        "before", "fixes", "fixed",
    ];
    let lowered = subject.to_lowercase();
    let (mut fr, mut en) = (0usize, 0usize);
    for word in lowered.split(|c: char| !c.is_alphabetic()) {
        fr += usize::from(FRENCH.contains(&word));
        en += usize::from(ENGLISH.contains(&word));
    }
    match fr.cmp(&en) {
        std::cmp::Ordering::Greater => Some(true),
        std::cmp::Ordering::Less => Some(false),
        std::cmp::Ordering::Equal => None,
    }
}

// ─── tooling ────────────────────────────────────────────────────────────────

/// A hook or CI step that checks commit messages.
struct Enforcer {
    at: String,
    ci: bool,
    /// The named tool it runs, when it names one.
    tool: Option<&'static str>,
}

fn read_small(path: &Path) -> Option<String> {
    if fs::metadata(path).ok()?.len() > MAX_READ {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn first_line_with(body: &str, needles: &[&str]) -> Option<usize> {
    live_lines(body)
        .into_iter()
        .find(|(_, text)| {
            let lowered = text.to_lowercase();
            needles.iter().any(|n| lowered.contains(n))
        })
        .map(|(number, _)| number)
}

/// A line without its trailing `# comment` (shell and YAML share the rule: the
/// `#` opens a comment at the start of a word, outside quotes).
fn without_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (at, c) in line.char_indices() {
        match quote {
            Some(open) => {
                if c == open {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '#' if previous.is_whitespace() => return &line[..at],
                _ => {}
            },
        }
        previous = c;
    }
    line
}

/// `(line number, text)` of the lines that say something once comments and
/// blanks are gone: a tool named only in a comment is not run by the file.
fn live_lines(body: &str) -> Vec<(usize, String)> {
    body.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let text = without_comment(line).trim();
            (!text.is_empty()).then(|| (index + 1, text.to_string()))
        })
        .collect()
}

/// The live text of a body, lowercased, one line each.
fn live_text(body: &str) -> String {
    live_lines(body)
        .into_iter()
        .map(|(_, text)| text.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A command that checks nothing: it prints, succeeds, or loads the hook runner.
fn is_inert(command: &str) -> bool {
    let command = command.trim();
    match command.split_whitespace().next() {
        None => true,
        Some("echo" | "printf" | "true" | ":" | "exit" | "set" | "source") => true,
        Some(".") => command.contains("husky.sh"),
        Some(_) => false,
    }
}

/// The command a CI line carries: none for a label (`name:`) or an inert step.
fn ci_command(text: &str) -> Option<&str> {
    let text = text.trim_start_matches('-').trim();
    for label in ["name:", "description:", "label:", "title:", "displayName:"] {
        if text.starts_with(label) {
            return None;
        }
    }
    let command = ["run:", "script:", "command:"]
        .iter()
        .find_map(|key| text.strip_prefix(key))
        .unwrap_or(text)
        .trim()
        .trim_matches(['"', '\'']);
    (!is_inert(command)).then_some(command)
}

const TOOL_KEYWORDS: &[(&str, &[&str])] = &[
    ("commitizen", &["commitizen", "cz check"]),
    ("commitlint", &["commitlint"]),
    ("gitlint", &["gitlint"]),
];

fn tool_named_in(text: &str) -> Option<&'static str> {
    TOOL_KEYWORDS
        .iter()
        .find(|(_, words)| words.iter().any(|w| text.contains(w)))
        .map(|(name, _)| *name)
}

fn detect_enforcers(project: &Path) -> Vec<Enforcer> {
    let mut out = Vec::new();
    for rel in [
        ".husky/commit-msg",
        ".githooks/commit-msg",
        ".git-hooks/commit-msg",
    ] {
        let Some(body) = read_small(&project.join(rel)) else {
            continue;
        };
        // A hook that only prints or loads the runner checks nothing, and a
        // tool named in its comments is not run by it.
        let live: Vec<(usize, String)> = live_lines(&body)
            .into_iter()
            .filter(|(_, text)| !is_inert(text))
            .collect();
        if let Some((first, _)) = live.first() {
            let text = live
                .iter()
                .map(|(_, text)| text.to_lowercase())
                .collect::<Vec<_>>()
                .join("\n");
            out.push(Enforcer {
                at: format!("{rel}:{first}"),
                ci: false,
                tool: tool_named_in(&text),
            });
        }
    }
    for rel in [
        "lefthook.yml",
        "lefthook.yaml",
        ".lefthook.yml",
        ".lefthook.yaml",
    ] {
        let Some(body) = read_small(&project.join(rel)) else {
            continue;
        };
        let lines: Vec<&str> = body.lines().collect();
        if let Some(start) = lines.iter().position(|l| l.starts_with("commit-msg:")) {
            let block: Vec<&str> = lines[start..]
                .iter()
                .enumerate()
                .take_while(|(i, l)| *i == 0 || l.is_empty() || l.starts_with([' ', '\t', '#']))
                .map(|(_, l)| *l)
                .collect();
            // A `commit-msg:` header with nothing but comments under it runs nothing.
            let text = live_text(&block[1..].join("\n"));
            if text.is_empty() {
                continue;
            }
            out.push(Enforcer {
                at: format!("{rel}:{}", start + 1),
                ci: false,
                tool: tool_named_in(&text),
            });
        }
    }
    if let Some(body) = read_small(&project.join(".pre-commit-config.yaml")) {
        let needles = ["commit-msg", "conventional-pre-commit", "gitlint"];
        if let Some(line) = first_line_with(&body, &needles) {
            let text = live_text(&body);
            out.push(Enforcer {
                at: format!(".pre-commit-config.yaml:{line}"),
                ci: false,
                tool: tool_named_in(&text),
            });
        }
    }
    let mut ci: Vec<String> = [
        ".gitlab-ci.yml",
        ".circleci/config.yml",
        "azure-pipelines.yml",
        "bitbucket-pipelines.yml",
        "Jenkinsfile",
        ".drone.yml",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Ok(entries) = fs::read_dir(project.join(".github/workflows")) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
            .collect();
        names.sort();
        ci.extend(names.into_iter().map(|n| format!(".github/workflows/{n}")));
    }
    for rel in ci {
        let Some(body) = read_small(&project.join(&rel)) else {
            continue;
        };
        // A CI step runs the tool: a comment, a `name:` label or an `echo`
        // that mentions it does not (KT-1387).
        for (number, line) in live_lines(&body) {
            let Some(command) = ci_command(&line) else {
                continue;
            };
            let text = command.to_lowercase();
            let named = tool_named_in(&text);
            let semantic =
                text.contains("semantic-pull-request") || text.contains("conventional-pre-commit");
            if named.is_some() || semantic {
                out.push(Enforcer {
                    at: format!("{rel}:{number}"),
                    ci: true,
                    tool: named,
                });
            }
        }
    }
    out
}

fn detect_tooling(project: &Path) -> Vec<Tool> {
    let enforcers = detect_enforcers(project);
    let mut tools: Vec<Tool> = Vec::new();

    // Named tools: configured where a config file shows them.
    let mut configured: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let package = read_small(&project.join("package.json"));
    let pyproject = read_small(&project.join("pyproject.toml"));
    for (rel, tool) in [
        (".czrc", "commitizen"),
        (".cz.json", "commitizen"),
        (".cz.toml", "commitizen"),
        (".cz.yaml", "commitizen"),
        ("cz.toml", "commitizen"),
        ("cz.json", "commitizen"),
        ("cz.yaml", "commitizen"),
        (".cz-config.js", "commitizen"),
        (".cz-config.cjs", "commitizen"),
        ("commitlint.config.js", "commitlint"),
        ("commitlint.config.cjs", "commitlint"),
        ("commitlint.config.mjs", "commitlint"),
        ("commitlint.config.ts", "commitlint"),
        ("commitlint.config.json", "commitlint"),
        (".commitlintrc", "commitlint"),
        (".commitlintrc.json", "commitlint"),
        (".commitlintrc.yml", "commitlint"),
        (".commitlintrc.yaml", "commitlint"),
        (".commitlintrc.js", "commitlint"),
        (".commitlintrc.cjs", "commitlint"),
        (".commitlintrc.mjs", "commitlint"),
        (".commitlintrc.ts", "commitlint"),
        (".gitlint", "gitlint"),
    ] {
        if project.join(rel).is_file() {
            configured.entry(tool).or_default().push(format!("{rel}:1"));
        }
    }
    if let Some(body) = &package {
        for (tool, needles) in [
            (
                "commitizen",
                &[
                    "\"commitizen\"",
                    "cz-conventional-changelog",
                    "cz-customizable",
                ][..],
            ),
            ("commitlint", &["\"commitlint\"", "@commitlint/"][..]),
        ] {
            if let Some(line) = first_line_with(body, needles) {
                configured
                    .entry(tool)
                    .or_default()
                    .push(format!("package.json:{line}"));
            }
        }
    }
    if let Some(body) = &pyproject {
        if let Some(line) = first_line_with(body, &["[tool.commitizen]"]) {
            configured
                .entry("commitizen")
                .or_default()
                .push(format!("pyproject.toml:{line}"));
        }
    }
    let mut names: Vec<&'static str> = configured.keys().copied().collect();
    for e in &enforcers {
        if let Some(tool) = e.tool {
            if !names.contains(&tool) {
                names.push(tool);
            }
        }
    }
    names.sort();
    for name in names {
        let evidence = configured.get(name).cloned().unwrap_or_default();
        let enforcer = enforcers.iter().find(|e| e.tool == Some(name));
        let (enforcement, note) = match enforcer {
            Some(e) => (Enforcement::Enforced(e.at.clone()), None),
            None if name == "commitizen" => (
                Enforcement::OptIn,
                Some(
                    "used through its CLI; no commit-msg hook or CI step runs `cz check`"
                        .to_string(),
                ),
            ),
            None => (Enforcement::ConfiguredOnly, None),
        };
        let mut evidence = evidence;
        if let Some(e) = enforcer {
            evidence.push(e.at.clone());
        }
        tools.push(Tool {
            name: name.to_string(),
            evidence,
            enforcement,
            note,
        });
    }

    // Makefile targets that open a commit tool.
    for (rel, target, line, recipe) in make_commit_targets(project) {
        let mut evidence = vec![format!("{rel}:{line}")];
        let script = recipe
            .split_whitespace()
            .map(|t| t.trim_start_matches("./"))
            .find(|t| {
                (t.ends_with(".sh") || t.ends_with(".js") || t.ends_with(".py"))
                    && project.join(t).is_file()
            });
        if let Some(script) = script {
            evidence.push(format!("{script}:1"));
        }
        tools.push(Tool {
            name: format!("Makefile target `make {target}` (runs `{recipe}`)"),
            evidence,
            enforcement: Enforcement::OptIn,
            note: Some("a command people choose to run; a plain `git commit` bypasses it".into()),
        });
    }

    // Hooks and CI steps that name no tool.
    for e in enforcers.iter().filter(|e| e.tool.is_none()) {
        let name = if e.ci {
            "CI commit-message check"
        } else {
            "commit-msg hook"
        };
        tools.push(Tool {
            name: name.to_string(),
            evidence: vec![e.at.clone()],
            enforcement: Enforcement::Enforced(e.at.clone()),
            note: (name == "commit-msg hook").then(|| {
                "a hook in the repository is active only where it is installed".to_string()
            }),
        });
    }
    tools
}

/// `(file, target, line, first recipe line)` of every Makefile target whose
/// name holds `commit`, at the root and one level down.
fn make_commit_targets(project: &Path) -> Vec<(String, String, usize, String)> {
    let mut files = Vec::new();
    // One root Makefile: a case-insensitive filesystem answers to all three.
    if let Some(name) = ["Makefile", "makefile", "GNUmakefile"]
        .into_iter()
        .find(|name| {
            fs::read_dir(project)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| entry.file_name() == *name)
        })
    {
        files.push(name.to_string());
    }
    if let Ok(entries) = fs::read_dir(project) {
        let mut dirs: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                !n.starts_with('.') && n != "node_modules" && n != "vendor" && n != "target"
            })
            .collect();
        dirs.sort();
        for dir in dirs {
            files.push(format!("{dir}/Makefile"));
        }
    }
    let mut out = Vec::new();
    for rel in files {
        let Some(body) = read_small(&project.join(&rel)) else {
            continue;
        };
        let lines: Vec<&str> = body.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with(['\t', ' ', '#', '.']) {
                continue;
            }
            let Some(colon) = line.find(':') else {
                continue;
            };
            if line[colon..].starts_with(":=") || line[..colon].contains('=') {
                continue;
            }
            for target in line[..colon].split_whitespace() {
                if !target.to_lowercase().contains("commit") || target.contains(['$', '%']) {
                    continue;
                }
                let recipe = lines[index + 1..]
                    .iter()
                    .take_while(|l| l.starts_with('\t') || l.trim().is_empty())
                    .map(|l| l.trim().trim_start_matches(['@', '-']).trim())
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_string();
                out.push((rel.clone(), target.to_string(), index + 1, recipe));
            }
        }
    }
    out
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "commit_facts_test.rs"]
mod tests;
