//! KT-1387 — the commit conventions of a repository, as observed.
//!
//! An agent asked to document "the commit convention" writes the one it knows
//! (Conventional Commits `type(scope): subject`) or the one a tool name hints
//! at ("Commitizen enforces every commit"), not the one the history shows.
//! [`CommitFacts::detect`] computes the facts instead, from
//! `git log -n 200` and the repository's own tooling: the share of each
//! subject shape (colon after the type or not), ticket prefixes, types,
//! scopes, language, trailers, and which commit tooling exists and whether a
//! CI step actually enforces it (a local git hook never does: it runs only where
//! it was installed). [`CommitFacts::render_block`] hands
//! them to the step as ground truth.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::core::rooted_io;

/// Subjects read from the history.
const SAMPLE: &str = "200";
/// Below this many subjects there is no pattern to observe.
const MIN_HISTORY: usize = 10;
const MAX_READ: u64 = 256 * 1024;
const SHOWN: usize = 12;
/// Workflow files read at most.
const MAX_CI_FILES: usize = 64;

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
    /// A CI step that runs it, for everyone (where).
    Enforced(String),
    /// A command people run by hand; nothing forces it.
    OptIn,
    /// Configured, or run by a local hook, but no CI step is proven to run it.
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
                Enforcement::Enforced(by) => format!("ENFORCED — checked in CI by {by}"),
                Enforcement::OptIn => "opt-in, run by hand — nothing enforces it".to_string(),
                Enforcement::ConfiguredOnly => {
                    "configured, but no CI step is proven to run it — not enforced".to_string()
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
             2. Say a convention is enforced ONLY for a tool above marked ENFORCED, and say it is checked in CI. A local git hook is never enforcement: write `local hook (not enforced)`, active only where a developer installed it. An opt-in or configured-only tool is described as exactly that.\n\
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

/// What a local git hook is worth as a fact: the repository holds it, but git
/// runs it only in a clone where someone installed it (`core.hooksPath`, husky's
/// `prepare`, lefthook install). It is never enforcement.
const LOCAL_HOOK_NOTE: &str =
    "local hook (not enforced): runs only where each developer installed it";

/// A local hook or CI step that checks commit messages. Only a CI one
/// (`ci: true`) can make a tool ENFORCED.
struct Enforcer {
    at: String,
    ci: bool,
    /// The named tool it runs, when it names one.
    tool: Option<&'static str>,
    /// A live command line of it RUNS a message checker. A hook that runs
    /// something else is only listed: when execution is not proven the fact is
    /// "configured", never "enforced".
    runs_checker: bool,
    /// A CI action that checks the pull-request TITLE, not the commit messages
    /// (`amannn/action-semantic-pull-request`): it needs a separate squash or
    /// merge setting to become the commit message, which is not read here.
    pr_title: bool,
}

/// THE way this module reads a file of the project: through
/// [`rooted_io::read`], which opens nothing below the root through a link and
/// refuses anything but a regular file with a single link, so neither a
/// symlink nor a hard link can bring outside text into the facts. Bounded.
fn read_small(project: &Path, rel: &str) -> Option<String> {
    let rel = Path::new(rel);
    if fs::symlink_metadata(project.join(rel)).ok()?.len() > MAX_READ {
        return None;
    }
    let bytes = rooted_io::read(project, rel).ok()?;
    if bytes.len() as u64 > MAX_READ {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Whether `rel` is a regular, single-link file of the project, reached
/// without a link (the same rule as [`read_small`]).
fn is_regular(project: &Path, rel: &str) -> bool {
    read_small(project, rel).is_some()
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

/// A command of a shell text, as the shell would split it: quoted strings stay
/// one word (an `echo 'a; commitlint'` is an echo), comments and here-document
/// bodies are gone.
struct Cmd {
    /// Line of the first word, counted from the text's first line.
    line: usize,
    words: Vec<String>,
    /// Holds a `$( … )` or backtick substitution: what runs is not readable.
    doubtful: bool,
    /// How it is joined to the command before it and to the one after it.
    before: Join,
    after: Join,
}

/// The operator between two commands. Only a newline or `;` runs the next one
/// unconditionally: the others make a command depend on, or hide the status of,
/// its neighbour.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Join {
    #[default]
    Seq,
    And,
    Or,
    Pipe,
    Background,
}

#[derive(Default)]
struct Lexer {
    out: Vec<Cmd>,
    words: Vec<String>,
    word: String,
    in_word: bool,
    line: usize,
    cmd_line: usize,
    doubtful: bool,
    /// Here-document delimiters waiting for the end of their line, with `<<-`.
    heredocs: Vec<(String, bool)>,
    heredoc_next: Option<bool>,
    /// The operator that opened the command being read.
    pending: Join,
}

impl Lexer {
    fn begin_word(&mut self) {
        if !self.in_word {
            if self.words.is_empty() {
                self.cmd_line = self.line;
            }
            self.in_word = true;
        }
    }

    fn end_word(&mut self) {
        if !self.in_word {
            return;
        }
        self.in_word = false;
        let word = std::mem::take(&mut self.word);
        if let Some(dash) = self.heredoc_next.take() {
            self.heredocs.push((word, dash));
            return;
        }
        if let Some(rest) = word.strip_prefix("<<").filter(|r| !r.starts_with('<')) {
            let (dash, delimiter) = match rest.strip_prefix('-') {
                Some(delimiter) => (true, delimiter),
                None => (false, rest),
            };
            if delimiter.is_empty() {
                self.heredoc_next = Some(dash);
            } else {
                self.heredocs.push((delimiter.to_string(), dash));
            }
            return;
        }
        self.words.push(word);
    }

    fn end_command(&mut self) -> bool {
        self.end_word();
        let pushed = !self.words.is_empty();
        if pushed {
            self.out.push(Cmd {
                line: self.cmd_line,
                words: std::mem::take(&mut self.words),
                doubtful: self.doubtful,
                before: std::mem::take(&mut self.pending),
                after: Join::Seq,
            });
        }
        self.doubtful = false;
        pushed
    }

    /// An operator ends the command before it and opens the next one.
    fn operator(&mut self, join: Join) {
        if self.end_command() {
            if let Some(last) = self.out.last_mut() {
                last.after = join;
            }
        }
        self.pending = join;
    }

    /// Take `chars[from..to]` into the current word as one raw piece.
    fn take_raw(&mut self, chars: &[char], from: usize, to: usize) {
        self.line += chars[from..to].iter().filter(|c| **c == '\n').count();
        self.word.extend(&chars[from..to]);
    }
}

/// The index after the `$( … )` that opens at `at` (the `$`): up to its
/// matching parenthesis, quotes and nesting honoured.
fn skip_substitution(chars: &[char], at: usize) -> usize {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut i = at + 1;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(open) => {
                if c == '\\' && open == '"' {
                    i += 1;
                } else if c == open {
                    quote = None;
                }
            }
            None => match c {
                '\\' => i += 1,
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    chars.len()
}

/// Whether the `&` or `|` at `at` belongs to a redirection (`2>&1`, `<&3`,
/// `&>file`, `>|file`) and not to an operator.
fn is_redirection(chars: &[char], at: usize) -> bool {
    let prev = at.checked_sub(1).map(|p| chars[p]);
    match chars[at] {
        '&' => matches!(prev, Some('>' | '<')) || chars.get(at + 1) == Some(&'>'),
        _ => prev == Some('>'),
    }
}

/// The simple commands of `text`, whose first line is `first_line`.
fn shell_commands(text: &str, first_line: usize) -> Vec<Cmd> {
    let chars: Vec<char> = text.chars().collect();
    let mut lex = Lexer {
        line: first_line,
        cmd_line: first_line,
        ..Lexer::default()
    };
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if let Some(open) = quote {
            match c {
                c if c == open => quote = None,
                '\\' if open == '"' && next.is_some() => {
                    i += 1;
                    if chars[i] == '\n' {
                        // A continuation: the line is joined to the next.
                        lex.line += 1;
                    } else {
                        lex.word.push(chars[i]);
                    }
                }
                '$' if open == '"' && next == Some('(') => {
                    let end = skip_substitution(&chars, i);
                    lex.doubtful = true;
                    lex.take_raw(&chars, i, end);
                    i = end;
                    continue;
                }
                '`' if open == '"' => {
                    lex.doubtful = true;
                    lex.word.push(c);
                }
                '\n' => {
                    lex.line += 1;
                    lex.word.push(c);
                }
                _ => lex.word.push(c),
            }
            i += 1;
            continue;
        }
        match c {
            '\\' => match next {
                Some('\n') => {
                    lex.line += 1;
                    i += 1;
                }
                Some(escaped) => {
                    lex.begin_word();
                    lex.word.push(escaped);
                    i += 1;
                }
                None => {}
            },
            '\'' | '"' => {
                lex.begin_word();
                quote = Some(c);
            }
            '`' => {
                lex.begin_word();
                lex.doubtful = true;
                let end = chars[i + 1..]
                    .iter()
                    .position(|c| *c == '`')
                    .map_or(chars.len(), |p| i + 2 + p);
                lex.take_raw(&chars, i, end);
                i = end;
                continue;
            }
            '$' if next == Some('(') => {
                lex.begin_word();
                lex.doubtful = true;
                let end = skip_substitution(&chars, i);
                lex.take_raw(&chars, i, end);
                i = end;
                continue;
            }
            '#' if !lex.in_word => {
                while i + 1 < chars.len() && chars[i + 1] != '\n' {
                    i += 1;
                }
            }
            // `2>&1`, `&>file` and `>|` are redirections, not operators.
            '&' | '|' if is_redirection(&chars, i) => {
                lex.begin_word();
                lex.word.push(c);
            }
            ';' => lex.operator(Join::Seq),
            '&' if next == Some('&') => {
                lex.operator(Join::And);
                i += 1;
            }
            '|' if next == Some('|') => {
                lex.operator(Join::Or);
                i += 1;
            }
            '|' => {
                lex.operator(Join::Pipe);
                if next == Some('&') {
                    i += 1;
                }
            }
            '&' => lex.operator(Join::Background),
            '\n' => {
                lex.end_command();
                lex.line += 1;
                i += 1;
                // The bodies of the here-documents opened on this line.
                for (delimiter, dash) in std::mem::take(&mut lex.heredocs) {
                    while i < chars.len() {
                        let end = chars[i..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map_or(chars.len(), |p| i + p);
                        let body: String = chars[i..end].iter().collect();
                        i = (end + 1).min(chars.len());
                        lex.line += 1;
                        let body = if dash {
                            body.trim_start_matches('\t')
                        } else {
                            body.as_str()
                        };
                        if body.trim_end() == delimiter {
                            break;
                        }
                    }
                }
                continue;
            }
            c if c.is_whitespace() => lex.end_word(),
            _ => {
                lex.begin_word();
                lex.word.push(c);
            }
        }
        i += 1;
    }
    lex.end_command();
    lex.out
}

fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// A command that only fetches or installs software: naming a package there
/// (`npm install --save-dev @commitlint/cli`) is not running it.
fn installs(words: &[String]) -> bool {
    const MANAGERS: &[&str] = &[
        "npm", "yarn", "pnpm", "bun", "pip", "pip3", "pipx", "poetry", "uv", "apt", "apt-get",
        "apk", "brew", "gem", "cargo", "go", "composer", "conda",
    ];
    const VERBS: &[&str] = &[
        "install",
        "i",
        "add",
        "ci",
        "update",
        "upgrade",
        "remove",
        "uninstall",
        "global",
        "get",
        "fetch",
        "download",
    ];
    let mut rest = words
        .iter()
        .skip_while(|w| is_env_assignment(w) || *w == "sudo")
        .map(String::as_str);
    let Some(manager) = rest.next() else {
        return false;
    };
    MANAGERS.contains(&manager)
        && rest
            .find(|w| !w.starts_with('-'))
            .is_some_and(|verb| VERBS.contains(&verb))
}

/// A command that checks nothing: it prints, succeeds, loads the hook runner or
/// only installs packages.
fn is_inert(cmd: &Cmd) -> bool {
    let mut words = cmd
        .words
        .iter()
        .skip_while(|w| is_env_assignment(w) || *w == "sudo");
    match words.next().map(String::as_str) {
        None => true,
        Some("echo" | "printf" | "true" | ":" | "exit" | "set" | "source") => true,
        Some(".") => cmd.words.iter().any(|w| w.contains("husky.sh")),
        Some(_) => installs(&cmd.words),
    }
}

/// Words that only launch another command: they never name the checker.
const LAUNCHERS: &[&str] = &[
    "npx",
    "bunx",
    "pnpx",
    "npm",
    "yarn",
    "pnpm",
    "bun",
    "exec",
    "run",
    "run-script",
    "dlx",
    "sudo",
    "sh",
    "bash",
    "zsh",
    "env",
    "time",
    "nohup",
    "poetry",
    "pipx",
    "pipenv",
    "uv",
    "uvx",
    "python",
    "python3",
];

/// The `scripts` of the project's `package.json`, by name.
type Scripts = BTreeMap<String, String>;

fn package_scripts(project: &Path) -> Scripts {
    let Some(body) = read_small(project, "package.json") else {
        return Scripts::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Scripts::new();
    };
    json.get("scripts")
        .and_then(|scripts| scripts.as_object())
        .map(|scripts| {
            scripts
                .iter()
                .filter_map(|(name, body)| Some((name.clone(), body.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// A flag that asks the tool about itself instead of checking a message.
const SELF_FLAGS: &[&str] = &["--help", "-h", "--version", "-V", "--print-config"];

/// Whether `cmd` RUNS a commit-message checker: `Some(Some(tool))` for a named
/// one, `Some(None)` for a checker with no name here, `None` otherwise. Only the
/// program a command executes counts: not a word in a string, an install, an
/// environment key, a `--help`. `npm run <script>` (and `yarn <script>`) is
/// resolved one level to the script's own command. When unsure: `None`.
fn command_runs_checker(
    cmd: &Cmd,
    scripts: &Scripts,
    depth: usize,
) -> Option<Option<&'static str>> {
    if cmd.doubtful || installs(&cmd.words) {
        return None;
    }
    let words = &cmd.words;
    let mut manager: Option<&str> = None;
    let mut via_run = false;
    let mut binary = false;
    let mut i = 0;
    while i < words.len() {
        let word = words[i].trim_start_matches(['(', '{']);
        // `sh -c "<command>"` runs that command.
        if word == "-c" && i > 0 && matches!(words[i - 1].as_str(), "sh" | "bash" | "zsh") {
            let inner = words.get(i + 1)?;
            return script_checker(&shell_commands(inner, cmd.line), scripts, depth, false);
        }
        if word.is_empty() || is_env_assignment(word) || word.starts_with(['-', '@']) {
            i += 1;
            continue;
        }
        if LAUNCHERS.contains(&word) {
            match word {
                "npm" | "yarn" | "pnpm" | "bun" => manager = Some(word),
                "run" | "run-script" if manager.is_some() => via_run = true,
                "npx" | "bunx" | "pnpx" | "exec" | "dlx" | "uvx" => binary = true,
                _ => {}
            }
            i += 1;
            continue;
        }
        let name = word;
        if manager == Some("npm") && !via_run && !binary {
            return None;
        }
        let by_script = via_run || (matches!(manager, Some("yarn" | "pnpm" | "bun")) && !binary);
        if by_script {
            if let Some(body) = scripts.get(name) {
                if depth > 0 {
                    return None;
                }
                return script_checker(&shell_commands(body, cmd.line), scripts, depth + 1, false);
            }
            if manager == Some("npm") {
                return None;
            }
        }
        let program = name.rsplit('/').next().unwrap_or("").to_lowercase();
        let rest = &words[i + 1..];
        if rest.iter().any(|w| SELF_FLAGS.contains(&w.as_str())) {
            return None;
        }
        return match program.as_str() {
            "commitlint" => Some(Some("commitlint")),
            "gitlint" if !rest.iter().any(|w| w == "install-hook") => Some(Some("gitlint")),
            "cz" | "commitizen" if rest.iter().any(|w| w == "check") => Some(Some("commitizen")),
            "conventional-pre-commit" => Some(None),
            _ => None,
        };
    }
    None
}

/// Words that decide whether, or how far, the rest of a script runs. A script
/// holding one is not read any further: Kronn does not interpret shell, so it
/// cannot tell that the checker after `exit 0` never runs, or that one inside
/// `if` is only sometimes run.
const CONTROL_WORDS: &[&str] = &[
    "exit", "return", "trap", "if", "then", "else", "elif", "fi", "case", "esac", "while", "until",
    "for", "select", "do", "done", "function", "{", "}", "(", ")", "!", "break", "continue",
    "eval", "exec", "true", "false", ":", "wait", "[[", "[", "test",
];

/// The first word a command runs, past environment assignments.
fn program(cmd: &Cmd) -> Option<&str> {
    cmd.words
        .iter()
        .map(String::as_str)
        .find(|w| !is_env_assignment(w))
}

/// Whether `cmd` loads the hook runner or a version manager: it runs nothing
/// that can end the script.
fn loads_environment(cmd: &Cmd) -> bool {
    matches!(program(cmd), Some("." | "source"))
        && cmd
            .words
            .iter()
            .any(|w| w.contains("husky.sh") || w.contains("nvm.sh"))
}

/// A command that cannot decide whether the following ones run: it prints,
/// installs, moves, sets a variable or loads the environment.
fn is_trivial(cmd: &Cmd) -> bool {
    if loads_environment(cmd) {
        return true;
    }
    match program(cmd) {
        None | Some("echo" | "printf" | "cd" | "export" | "unset") => true,
        Some("set") => !cmd.words.iter().any(|w| w.starts_with('+')),
        Some(_) => installs(&cmd.words),
    }
}

/// Whether `cmd` is, or opens, a control-flow construct: a keyword, a group or
/// subshell, a function definition, an `exit`, `set +e`, a `source` of anything
/// but the environment loaders.
fn is_control_flow(cmd: &Cmd) -> bool {
    if loads_environment(cmd) {
        return false;
    }
    let Some(first) = program(cmd) else {
        return false;
    };
    CONTROL_WORDS.contains(&first)
        || matches!(first, "." | "source")
        || first.starts_with(['(', '{'])
        || first.ends_with("()")
        || (first == "set" && cmd.words.iter().any(|w| w.starts_with('+')))
}

/// What a script (a CI `run:` text, a package script, a `sh -c` string, a hook)
/// answers to "does it run a commit-message checker?". `Some` ONLY for a
/// straight-line script whose first non-trivial command is the checker: no
/// `exit`/`return`, no `||`, pipe or `&`, no `&&` behind a command that can fail
/// on its own, no `if`/`case`/loop, no function or group, no `set +e` anywhere
/// in it. Anything else abstains (`None`, so "configured"): a false "enforced"
/// is the harmful error.
///
/// `errexit` says the shell stops at the first failing command, as a CI `run:`
/// step does by default (GitHub Actions starts `bash -e {0}`): the checker's
/// failure then ends the script whatever follows it (`checker; echo done`
/// fails). Without it (`sh -c`, an `npm run` script) the script's status is its
/// LAST command's, so the checker must be the last command, or the head of an
/// `&&` chain that runs to the end of the script.
fn script_checker(
    cmds: &[Cmd],
    scripts: &Scripts,
    depth: usize,
    errexit: bool,
) -> Option<Option<&'static str>> {
    let first = cmds.iter().find(|cmd| !is_trivial(cmd))?;
    let at = cmds.iter().position(|cmd| std::ptr::eq(cmd, first))?;
    let tail = &cmds[at + 1..];
    // The script's status is the checker's when nothing runs after it, or when
    // everything after it hangs off it with `&&` (a failing checker skips the
    // rest and the script ends on its status).
    let status_reaches_the_end = tail.is_empty()
        || (first.after == Join::And && tail.iter().all(|cmd| cmd.before == Join::And))
        || (errexit && first.after == Join::Seq);
    if !status_reaches_the_end {
        return None;
    }
    if cmds.iter().any(|cmd| {
        is_control_flow(cmd) && !std::ptr::eq(cmd, first)
            || matches!(cmd.before, Join::Or | Join::Pipe | Join::Background)
            || matches!(cmd.after, Join::Or | Join::Pipe | Join::Background)
            // `a && checker`: the checker is conditional unless `a` cannot fail.
            || (cmd.after == Join::And && !is_trivial(cmd) && !std::ptr::eq(cmd, first))
    }) {
        return None;
    }
    // `exec` is control flow unless it launches the checker itself.
    command_runs_checker(first, scripts, depth)
        .filter(|_| !is_control_flow(first) || program(first) == Some("exec"))
}

/// What a `uses:` line is worth for commit conventions.
enum Action {
    /// A dedicated commit-message action that runs this checker on the commits
    /// (`wagoid/commitlint-github-action`).
    Checker(&'static str),
    /// A check of the pull-request title, not of the commit messages.
    PrTitle,
    /// Related to commit messages, but not proven to check them.
    Unproven,
}

/// A CI step that `uses:` a commit-message action. Only the action's own name
/// counts, never its inputs.
fn action_used(value: &str) -> Option<Action> {
    let action = value
        .trim()
        .trim_matches(['"', '\''])
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase();
    // A title check never counts as a commit check, whatever tool it names.
    if action.contains("pull-request")
        || action.contains("pull_request")
        || action.contains("pr-title")
        || action.contains("pr_title")
    {
        Some(Action::PrTitle)
    } else if action.contains("commitlint") {
        Some(Action::Checker("commitlint"))
    } else if action.contains("gitlint") {
        Some(Action::Checker("gitlint"))
    } else if action.contains("conventional-pre-commit") {
        Some(Action::Unproven)
    } else {
        None
    }
}

/// What a CI file asks to run.
enum Step {
    /// Shell text starting at a line.
    Shell(usize, String),
    /// A `uses:` action.
    Uses(usize, String),
}

/// Keys whose value is a command, in the CI systems that matter here.
const COMMAND_KEYS: &[&str] = &[
    "run",
    "script",
    "command",
    "commands",
    "before_script",
    "after_script",
    "bash",
    "sh",
    "pwsh",
    "powershell",
];

fn is_block_indicator(value: &str) -> bool {
    value.starts_with(['|', '>'])
        && value[1..]
            .chars()
            .all(|c| c == '-' || c == '+' || c.is_ascii_digit())
}

/// `key: rest` when `item` opens with a bare YAML key.
fn bare_key(item: &str) -> Option<(&str, &str)> {
    let (key, rest) = item.split_once(':')?;
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    (bare && (rest.is_empty() || rest.starts_with(' '))).then_some((key, rest))
}

fn unquote_yaml(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Keys whose children are data, not commands: an entry named `run` or
/// `script` under one of them is a value of the CI file, never a step.
const DATA_KEYS: &[&str] = &[
    "env",
    "environment",
    "with",
    "vars",
    "variables",
    "inputs",
    "args",
    "secrets",
    "matrix",
    "parameters",
    "params",
    "labels",
    "outputs",
    "defaults",
];

/// Whether the CI file `rel` is one whose semantics are read well enough to
/// call a checker ENFORCED: a GitHub Actions workflow, whose Linux/macOS
/// default shell is `bash -e {0}` (the step stops at the first failing
/// command; a Windows runner defaults to `pwsh`, which does not, see
/// [`yaml_steps_shell`]). Any other CI
/// system (GitLab, CircleCI, Azure, Bitbucket, Drone, Jenkins) has its own
/// `allow_failure` / `when` / `rules` / `only` / `condition` / Groovy
/// semantics that are not read here: a checker found there is listed as
/// configured, never enforced (KT-1387).
fn ci_proves_enforcement(rel: &str) -> bool {
    rel.starts_with(".github/workflows/")
}

/// A `shell:` that is the default one of its CI system (`bash`, `sh`): the step
/// runs with errexit. Any other value is a custom command line (`bash {0}`,
/// `pwsh`, `python`) whose exit handling is its own.
fn default_shell(value: &str) -> bool {
    matches!(
        unquote_yaml(without_comment(value).trim()).trim(),
        "bash" | "sh"
    )
}

/// Whether `key: value` makes a failing step NOT fail the run: the step's
/// failure is masked, so a checker in it enforces nothing.
fn masks_failure(key: &str, value: &str) -> bool {
    let value = unquote_yaml(without_comment(value).trim())
        .trim()
        .to_lowercase();
    let unmasked = matches!(value.as_str(), "false" | "0" | "no" | "off");
    match key {
        // Anything but a literal false (`true`, an expression, a mapping with
        // `exit_codes`, an empty value opening one) is doubt.
        "continue-on-error" | "continueOnError" | "allow_failure" => !unmasked,
        "failure" => value == "ignore",
        "shell" => !default_shell(&value),
        _ => false,
    }
}

/// Whether `key: value` switches its step (or job, or hook) off: a step with
/// `if: false` never runs, whatever its command says.
fn switches_off(key: &str, value: &str) -> bool {
    if masks_failure(key, value) {
        return true;
    }
    let value = unquote_yaml(without_comment(value).trim()).trim();
    if value.is_empty() {
        return false;
    }
    let inner = value
        .strip_prefix("${{")
        .and_then(|v| v.strip_suffix("}}"))
        .map_or(value, str::trim);
    let inner = unquote_yaml(inner).trim().to_lowercase();
    match key {
        "if" => {
            matches!(inner.as_str(), "false" | "0" | "null" | "none")
                || inner.starts_with("false &&")
        }
        "enabled" => inner == "false",
        "skip" | "disabled" => inner == "true",
        "when" => matches!(inner.as_str(), "never" | "manual"),
        _ => false,
    }
}

/// The commands and actions of a YAML CI file: the value of a command key
/// (inline, block scalar or list) and `uses:` lines. Any other value (an `env:`
/// entry, an input, a block scalar holding an example) is not run by the file,
/// and neither is a step, job or hook switched off (`if: false`).
///
/// A command key counts only when none of its ancestors is a data section
/// ([`DATA_KEYS`]): `env: { run: … }` is an environment variable named `run`.
fn yaml_steps(body: &str) -> Vec<Step> {
    yaml_steps_shell(body)
        .into_iter()
        .map(|(step, _)| step)
        .collect()
}

/// Whether a `runs-on:` value is a literal label of a runner whose default
/// shell is `bash -e` (Linux or macOS): `ubuntu-*`, `macos-*`, or `self-hosted`
/// with a `linux` label. A Windows label (default shell `pwsh`, which does not
/// fail a step on a failing native command), an expression
/// (`${{ matrix.os }}`), a mapping or nothing at all is not established.
fn unix_runner(value: &str) -> bool {
    let value = unquote_yaml(without_comment(value).trim()).trim();
    if value.is_empty() || value.contains('$') {
        return false;
    }
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    let labels: Vec<String> = inner
        .split(',')
        .map(|label| unquote_yaml(label.trim()).trim().to_lowercase())
        .filter(|label| !label.is_empty())
        .collect();
    if labels.is_empty() || labels.iter().any(|l| l.contains("windows")) {
        return false;
    }
    let hosted = labels
        .iter()
        .any(|l| l.starts_with("ubuntu") || l.starts_with("macos"));
    let linux_self_hosted =
        labels.iter().any(|l| l == "self-hosted") && labels.iter().any(|l| l == "linux");
    hosted || linux_self_hosted
}

/// [`yaml_steps`] with, for each step, whether its effective shell is
/// established as `bash`/`sh` (so a failing command stops the step): the most
/// specific of `steps[*].shell`, `jobs.<id>.defaults.run.shell` and the
/// workflow's `defaults.run.shell`, else the implicit shell of a literal
/// Linux/macOS `runs-on:`. A Windows, matrix or unknown runner without an
/// explicit `bash`/`sh` is not established (KT-1387).
fn yaml_steps_shell(body: &str) -> Vec<(Step, bool)> {
    struct Open {
        col: usize,
        commands: bool,
        first: usize,
        text: String,
        mappings: Vec<usize>,
    }
    /// A YAML mapping: its key column and an identity, so that an `if:` finds
    /// the `run:` next to it (and a job's `if:` the steps under it).
    struct Mapping {
        col: usize,
        id: usize,
    }
    let mut out: Vec<(Step, Vec<usize>)> = Vec::new();
    let mut off: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut open: Option<Open> = None;
    let mut list_col: Option<usize> = None;
    let mut mappings: Vec<Mapping> = Vec::new();
    let mut keys: Vec<(usize, String)> = Vec::new();
    let mut next_id = 0usize;
    let mut everything_masked = false;
    // The mapping that carries a `shell:` (a step, or the owner of a
    // `defaults: run:`) and the one that carries a `runs-on:` (a job).
    let mut shells: BTreeMap<usize, bool> = BTreeMap::new();
    let mut runners: BTreeMap<usize, bool> = BTreeMap::new();
    for (index, raw) in body.lines().enumerate() {
        let number = index + 1;
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();
        if let Some(block) = open.as_mut() {
            if trimmed.is_empty() {
                block.text.push('\n');
                continue;
            }
            if indent > block.col {
                if block.commands {
                    block.text.push_str(trimmed);
                    block.text.push('\n');
                }
                continue;
            }
            if let Some(block) = open.take().filter(|block| block.commands) {
                out.push((Step::Shell(block.first, block.text), block.mappings));
            }
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let dashed = trimmed == "-" || trimmed.starts_with("- ");
        if let Some(col) = list_col {
            if indent < col || (indent == col && !dashed) {
                list_col = None;
            }
        }
        let mut item = trimmed;
        while let Some(rest) = item.strip_prefix("- ") {
            item = rest.trim_start();
        }
        let col = raw.len() - item.len();
        let key = bare_key(item);
        let mut data = false;
        if let Some((name, rest)) = key {
            while mappings.last().is_some_and(|m| m.col > col) {
                mappings.pop();
            }
            match mappings.last_mut() {
                Some(m) if m.col == col && dashed => {
                    next_id += 1;
                    m.id = next_id;
                }
                Some(m) if m.col == col => {}
                _ => {
                    next_id += 1;
                    mappings.push(Mapping { col, id: next_id });
                }
            }
            while keys.last().is_some_and(|(c, _)| *c >= col) {
                keys.pop();
            }
            data = keys.iter().any(|(_, k)| DATA_KEYS.contains(&k.as_str()));
            if name == "shell" {
                let owner = if !data {
                    mappings.last().map(|m| m.id)
                } else if let [.., (defaults_col, defaults), (_, run)] = keys.as_slice() {
                    // `defaults: run: shell:` belongs to the mapping that holds
                    // the `defaults:` key (the workflow or a job).
                    (defaults == "defaults" && run == "run")
                        .then(|| mappings.iter().rev().find(|m| m.col == *defaults_col))
                        .flatten()
                        .map(|m| m.id)
                } else {
                    None
                };
                if let Some(owner) = owner {
                    shells.insert(owner, default_shell(rest));
                }
            }
            if name == "runs-on" && !data {
                if let Some(m) = mappings.last() {
                    runners.insert(m.id, unix_runner(rest));
                }
            }
            // `defaults: run: shell: pwsh` changes the shell of every step.
            if data && masks_failure(name, rest) {
                everything_masked = true;
            }
            keys.push((col, name.to_string()));
            if switches_off(name, rest) {
                if let Some(m) = mappings.last() {
                    off.insert(m.id);
                }
            }
        }
        let ids: Vec<usize> = mappings.iter().map(|m| m.id).collect();
        if data {
            // Inside an `env:` / `with:` section nothing is a command, not
            // even the block scalars it may hold.
            if let Some((_, rest)) = key {
                if is_block_indicator(without_comment(rest).trim()) {
                    open = Some(Open {
                        col,
                        commands: false,
                        first: number + 1,
                        text: String::new(),
                        mappings: ids,
                    });
                }
            }
            continue;
        }
        if let Some(value) = item.strip_prefix("uses:") {
            out.push((Step::Uses(number, value.trim().to_string()), ids));
            continue;
        }
        match key {
            Some((name, rest)) => {
                let rest = without_comment(rest).trim();
                let commands = COMMAND_KEYS.contains(&name);
                if is_block_indicator(rest) {
                    open = Some(Open {
                        col,
                        commands,
                        first: number + 1,
                        text: String::new(),
                        mappings: ids,
                    });
                } else if commands && rest.is_empty() {
                    list_col = Some(col);
                } else if commands {
                    out.push((Step::Shell(number, unquote_yaml(rest).to_string()), ids));
                }
            }
            None if dashed && list_col.is_some() => {
                let value = without_comment(item).trim();
                if is_block_indicator(value) {
                    open = Some(Open {
                        col: indent,
                        commands: true,
                        first: number + 1,
                        text: String::new(),
                        mappings: ids,
                    });
                } else {
                    out.push((Step::Shell(number, unquote_yaml(item).to_string()), ids));
                }
            }
            None => {}
        }
    }
    if let Some(block) = open.filter(|block| block.commands) {
        out.push((Step::Shell(block.first, block.text), block.mappings));
    }
    out.into_iter()
        .filter(|(_, ids)| !everything_masked && !ids.iter().any(|id| off.contains(id)))
        .map(|(step, ids)| {
            let established = match ids.iter().rev().find_map(|id| shells.get(id)) {
                Some(shell) => *shell,
                None => ids
                    .iter()
                    .rev()
                    .find_map(|id| runners.get(id))
                    .copied()
                    .unwrap_or(false),
            };
            (step, established)
        })
        .collect()
}

/// The `sh '…'` / `sh """…"""` steps of a Jenkinsfile.
fn jenkins_steps(body: &str) -> Vec<Step> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        index += 1;
        let Some(rest) = ["sh ", "sh(", "bat ", "bat(", "powershell ", "pwsh "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        else {
            continue;
        };
        let rest = rest.trim_start().trim_start_matches("script:").trim_start();
        if let Some((triple, after)) = ["'''", "\"\"\""]
            .iter()
            .find_map(|triple| rest.strip_prefix(triple).map(|after| (*triple, after)))
        {
            let number = index;
            let mut text = String::new();
            let mut current = after.to_string();
            loop {
                if let Some(end) = current.find(triple) {
                    text.push_str(&current[..end]);
                    break;
                }
                text.push_str(&current);
                text.push('\n');
                let Some(next) = lines.get(index) else { break };
                index += 1;
                current = next.to_string();
            }
            out.push(Step::Shell(number, text));
        } else if let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) {
            if let Some(end) = rest[1..].find(quote) {
                out.push(Step::Shell(index, rest[1..1 + end].to_string()));
            }
        }
    }
    out
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
    let scripts = package_scripts(project);
    for rel in [
        ".husky/commit-msg",
        ".githooks/commit-msg",
        ".git-hooks/commit-msg",
    ] {
        let Some(body) = read_small(project, rel) else {
            continue;
        };
        // A hook that only prints or loads the runner checks nothing, and a
        // tool named in a comment or a string is not run by it.
        let cmds = shell_commands(&body, 1);
        if let Some(first) = cmds.iter().find(|cmd| !is_inert(cmd)) {
            let checker = script_checker(&cmds, &scripts, 0, true);
            out.push(Enforcer {
                at: format!("{rel}:{}", first.line),
                ci: false,
                tool: checker.flatten(),
                runs_checker: checker.is_some(),
                pr_title: false,
            });
        }
    }
    for rel in [
        "lefthook.yml",
        "lefthook.yaml",
        ".lefthook.yml",
        ".lefthook.yaml",
    ] {
        let Some(body) = read_small(project, rel) else {
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
            // A `commit-msg:` header with no command under it (nothing but
            // comments, keys, an echo or an install) runs nothing.
            let cmds: Vec<Cmd> = yaml_steps(&block[1..].join("\n"))
                .into_iter()
                .filter_map(|step| match step {
                    Step::Shell(line, text) => Some(shell_commands(&text, line)),
                    Step::Uses(..) => None,
                })
                .flatten()
                .collect();
            if cmds.iter().all(is_inert) {
                continue;
            }
            let checker = script_checker(&cmds, &scripts, 0, true);
            out.push(Enforcer {
                at: format!("{rel}:{}", start + 1),
                ci: false,
                tool: checker.flatten(),
                runs_checker: checker.is_some(),
                pr_title: false,
            });
        }
    }
    if let Some(body) = read_small(project, ".pre-commit-config.yaml") {
        let needles = ["commit-msg", "conventional-pre-commit", "gitlint"];
        if let Some(line) = first_line_with(&body, &needles) {
            let text = live_text(&body);
            out.push(Enforcer {
                at: format!(".pre-commit-config.yaml:{line}"),
                ci: false,
                tool: tool_named_in(&text),
                runs_checker: true,
                pr_title: false,
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
        ci.extend(
            names
                .into_iter()
                .take(MAX_CI_FILES)
                .map(|n| format!(".github/workflows/{n}")),
        );
    }
    for rel in ci {
        let Some(body) = read_small(project, &rel) else {
            continue;
        };
        // A CI file runs what its command keys and `uses:` lines say: a
        // comment, a `name:` label, an `env:` entry or an `echo` that mentions
        // a tool does not run it (KT-1387).
        let proves = ci_proves_enforcement(&rel);
        let steps: Vec<(Step, bool)> = if rel == "Jenkinsfile" {
            jenkins_steps(&body)
                .into_iter()
                .map(|step| (step, false))
                .collect()
        } else {
            yaml_steps_shell(&body)
        };
        for (step, established) in steps {
            match step {
                Step::Uses(number, value) => {
                    if let Some(action) = action_used(&value) {
                        // `uses:` is a GitHub Actions construct; elsewhere the
                        // line proves nothing and names no tool.
                        let (tool, runs_checker, pr_title) = match action {
                            Action::Checker(tool) if proves => (Some(tool), true, false),
                            Action::PrTitle => (None, false, true),
                            _ => (None, false, false),
                        };
                        out.push(Enforcer {
                            at: format!("{rel}:{number}"),
                            ci: true,
                            tool,
                            runs_checker,
                            pr_title,
                        });
                    }
                }
                Step::Shell(number, text) => {
                    // The step is read as one script: a checker after
                    // `exit 0`, behind `||` or inside an `if` is not proven to
                    // run, so the step is listed as configured, not enforced.
                    let cmds = shell_commands(&text, number);
                    // Only a GitHub Actions step is proof. Another CI system
                    // (and a Jenkinsfile, Groovy around its `sh` steps) decides
                    // what a failing step does in ways not read here. And only
                    // when its shell is established as bash/sh: the implicit
                    // shell of a Windows, matrix or unknown runner is pwsh or
                    // unknown, which can mask a failing checker.
                    let proven = if proves && established {
                        script_checker(&cmds, &scripts, 0, true)
                    } else {
                        None
                    };
                    if let Some(tool) = proven {
                        if let Some(first) = cmds.iter().find(|cmd| !is_trivial(cmd)) {
                            out.push(Enforcer {
                                at: format!("{rel}:{}", first.line),
                                ci: true,
                                tool,
                                runs_checker: true,
                                pr_title: false,
                            });
                        }
                    } else if let Some(cmd) = cmds.iter().find(|cmd| {
                        !is_inert(cmd) && command_runs_checker(cmd, &scripts, 0).is_some()
                    }) {
                        out.push(Enforcer {
                            at: format!("{rel}:{}", cmd.line),
                            ci: true,
                            tool: None,
                            runs_checker: false,
                            pr_title: false,
                        });
                    }
                }
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
    let package = read_small(project, "package.json");
    let pyproject = read_small(project, "pyproject.toml");
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
        if is_regular(project, rel) {
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
        // Only a CI step enforces: it runs for everyone. A local hook runs
        // where each developer installed it, whatever its script says.
        let enforcer = enforcers
            .iter()
            .find(|e| e.ci && e.runs_checker && e.tool == Some(name));
        let local_hook = enforcers
            .iter()
            .find(|e| !e.ci && e.runs_checker && e.tool == Some(name));
        let (enforcement, note) = match (enforcer, local_hook) {
            (Some(e), _) => (Enforcement::Enforced(e.at.clone()), None),
            (None, Some(_)) => (
                Enforcement::ConfiguredOnly,
                Some(LOCAL_HOOK_NOTE.to_string()),
            ),
            (None, None) if name == "commitizen" => (
                Enforcement::OptIn,
                Some("used through its CLI; no CI step runs `cz check`".to_string()),
            ),
            (None, None) => (Enforcement::ConfiguredOnly, None),
        };
        let mut evidence = evidence;
        if let Some(e) = enforcer.or(local_hook) {
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
                    && is_regular(project, t)
            });
        if let Some(script) = script {
            evidence.push(format!("{script}:1"));
        }
        tools.push(Tool {
            name: format!("Makefile target `make {target}`"),
            evidence,
            enforcement: Enforcement::OptIn,
            note: Some("a command people choose to run; a plain `git commit` bypasses it".into()),
        });
    }

    // Hooks and CI steps that name no tool. A local hook is never enforcement;
    // a CI step whose commands are not proven to run a message checker is
    // listed as configured too.
    for e in enforcers.iter().filter(|e| e.tool.is_none()) {
        let name = if e.pr_title {
            "PR title check (CI)"
        } else if e.ci {
            "CI commit-message check"
        } else {
            "commit-msg hook"
        };
        let (enforcement, note) = if e.pr_title {
            // The title becomes the commit message only through a separate
            // squash/merge setting that is not read here: never a commit fact.
            (
                Enforcement::ConfiguredOnly,
                Some(
                    "checks the pull-request title, not each commit message; it reaches the history only if the repository squashes or merges with the title as the commit message (a separate setting, not read here)"
                        .to_string(),
                ),
            )
        } else if !e.ci {
            let read = if e.runs_checker {
                ""
            } else {
                "; it is not proven to run a commit-message checker: read the file before saying what it checks"
            };
            (
                Enforcement::ConfiguredOnly,
                Some(format!("{LOCAL_HOOK_NOTE}{read}")),
            )
        } else if e.runs_checker {
            (Enforcement::Enforced(e.at.clone()), None)
        } else {
            (
                Enforcement::ConfiguredOnly,
                Some(
                    "not proven to run a commit-message checker (other commands, control flow or an unknown tool around it): read the file before saying what it checks"
                        .to_string(),
                ),
            )
        };
        tools.push(Tool {
            name: name.to_string(),
            evidence: vec![e.at.clone()],
            enforcement,
            note,
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
        let Some(body) = read_small(project, &rel) else {
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
                if !target.to_lowercase().contains("commit")
                    || !target
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
                {
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
