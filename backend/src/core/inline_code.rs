//! Inline code handed to an interpreter (`bash -c`, `python3 -c`, `node -e`…)
//! in a workflow Exec step: where it sits in argv, whether it interpolates a
//! template value, and how to move such values into separate argv entries.
//!
//! One classifier serves the save-time validator, the run-time refusal and
//! the per-workflow report, so the three can never disagree.

use crate::models::{UnsafeExecStep, WorkflowStep};

/// Shells whose `-c` argument is a script.
const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "fish", "ksh", "ash"];

/// Inline-code options of an interpreter: the short option letters (also
/// valid inside a cluster such as `-ec`, or with the code attached, as in
/// `-cprint(1)`) and the long options (`--eval`, `--eval=<code>`).
struct InlineCodeOptions {
    letters: &'static [char],
    long: &'static [&'static str],
    /// PowerShell takes single-dash long names, abbreviated, any case.
    case_insensitive: bool,
}

fn base_name(cmd: &str) -> String {
    cmd.trim().to_ascii_lowercase()
}

fn is_shell(cmd: &str) -> bool {
    SHELLS.contains(&base_name(cmd).as_str())
}

fn is_python(cmd: &str) -> bool {
    let lower = base_name(cmd);
    lower.starts_with("python") || lower.starts_with("pypy")
}

fn is_node(cmd: &str) -> bool {
    matches!(base_name(cmd).as_str(), "node" | "nodejs")
}

fn inline_code_options(cmd: &str) -> Option<InlineCodeOptions> {
    let lower = base_name(cmd);
    let (letters, long, case_insensitive): (&[char], &[&str], bool) =
        if is_shell(cmd) || is_python(cmd) {
            (&['c'], &["--command"], false)
        } else if matches!(lower.as_str(), "node" | "nodejs" | "bun" | "deno") {
            (&['e', 'p'], &["--eval", "--print"], false)
        } else if matches!(lower.as_str(), "ruby" | "perl") {
            (&['e', 'E'], &[], false)
        } else if lower == "php" {
            (&['r'], &[], false)
        } else if matches!(lower.as_str(), "pwsh" | "powershell") {
            (&['c', 'e'], &[], true)
        } else {
            return None;
        };
    Some(InlineCodeOptions {
        letters,
        long,
        case_insensitive,
    })
}

/// One argument holding inline code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineCode {
    /// Index of the argument that holds (or may hold) the code.
    pub index: usize,
    /// The code is attached to its option (`-cCODE`, `--eval=CODE`).
    pub attached: bool,
}

/// The arguments that may hold inline code for `cmd`, scanning every option
/// (Perl and Ruby repeat `-e`). In doubt an argument counts as code: a false
/// refusal costs a rewrite, a miss runs a payload. A separate option also
/// counts itself, so a placeholder in an option cluster is never missed.
pub fn inline_code_args(cmd: &str, args: &[String]) -> Vec<InlineCode> {
    let Some(options) = inline_code_options(cmd) else {
        return Vec::new();
    };
    let mut code = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        let (is_code_option, attached) = if let Some(rest) = arg.strip_prefix("--") {
            let (name, value) = rest
                .split_once('=')
                .map_or((rest, None), |(name, value)| (name, Some(value)));
            let flag = format!("--{}", name.to_ascii_lowercase());
            (options.long.contains(&flag.as_str()), value.is_some())
        } else if let Some(cluster) = arg.strip_prefix('-') {
            let hit = cluster.char_indices().find(|(_, c)| {
                options.letters.contains(c)
                    || (options.case_insensitive
                        && options.letters.contains(&c.to_ascii_lowercase()))
            });
            match hit {
                // PowerShell names are words (`-Command`), never attached code.
                Some((at, c)) => (
                    true,
                    !options.case_insensitive && at + c.len_utf8() < cluster.len(),
                ),
                None => (false, false),
            }
        } else {
            (false, false)
        };
        if !is_code_option {
            continue;
        }
        code.push(InlineCode { index, attached });
        if !attached && index + 1 < args.len() {
            code.push(InlineCode {
                index: index + 1,
                attached: false,
            });
        }
    }
    code
}

/// Values Kronn produces itself; every other placeholder may carry text an
/// outsider controls (tracker fields, step outputs, launch inputs).
pub fn is_trusted_template_path(path: &str) -> bool {
    path == "run.id" || path.starts_with("time.now")
}

/// Why an inline script is unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineFinding {
    /// A placeholder outside the trusted set, by path (filters stripped).
    Untrusted(String),
    /// An unclosed or malformed placeholder: refused, it cannot be checked.
    Malformed,
}

/// The first unsafe placeholder in the inline code of `cmd args`, if any.
/// Inside code a value can always find a context where it runs (heredoc,
/// eval, nested quotes), whatever filter it went through, so only
/// `{{run.id}}` and `{{time.now…}}` are accepted.
pub fn first_unsafe_placeholder(cmd: &str, args: &[String]) -> Option<InlineFinding> {
    for code in inline_code_args(cmd, args) {
        let Ok(paths) = crate::workflows::template::placeholder_paths(&args[code.index]) else {
            return Some(InlineFinding::Malformed);
        };
        if let Some(path) = paths
            .into_iter()
            .find(|path| !is_trusted_template_path(path))
        {
            return Some(InlineFinding::Untrusted(path));
        }
    }
    None
}

/// The recipe shown with every refusal.
pub fn safe_recipe(path: &str) -> String {
    format!(
        "passe la valeur en argument séparé après le script, que l'interpréteur ne lit jamais \
         comme du code : `exec_args=[\"-c\", \"echo \\\"$1\\\"\", \"_\", \"{{{{{path}}}}}\"]` pour \
         un shell, `sys.argv[1]` / `process.argv[1]` sinon, ou via `exec_stdin`"
    )
}

/// Save-time refusal for one command line (main or setup).
pub fn validation_error(step: &str, cmd: &str, args: &[String]) -> Option<String> {
    match first_unsafe_placeholder(cmd, args)? {
        InlineFinding::Malformed => Some(format!(
            "Step Exec « {step} » : le code inline de `{cmd}` contient un placeholder mal formé."
        )),
        InlineFinding::Untrusted(path) => Some(format!(
            "Step Exec « {step} » : le script inline de `{cmd}` interpole `{{{{{path}}}}}` — \
             dans du code, une valeur peut toujours être exécutée (heredoc, eval, guillemets), \
             même filtrée par `|sh`. {}.",
            safe_recipe(&path)
        )),
    }
}

/// Every unsafe command line of a saved step (main, then setup), with a
/// suggested rewrite when one is provably equivalent.
pub fn classify_step(step: &WorkflowStep, on_failure: bool) -> Vec<UnsafeExecStep> {
    if !matches!(step.step_type, crate::models::StepType::Exec) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut check = |phase: &str, cmd: Option<&str>, args: &[String]| {
        let Some(cmd) = cmd.map(str::trim).filter(|cmd| !cmd.is_empty()) else {
            return;
        };
        let Some(finding) = first_unsafe_placeholder(cmd, args) else {
            return;
        };
        let placeholder = match &finding {
            InlineFinding::Untrusted(path) => format!("{{{{{path}}}}}"),
            InlineFinding::Malformed => String::new(),
        };
        let (suggested_args, manual_fix) = match suggest_args(cmd, args) {
            Ok(rewritten) => (Some(rewritten), None),
            Err(reason) => (None, Some(reason)),
        };
        found.push(UnsafeExecStep {
            step_name: step.name.clone(),
            on_failure,
            phase: phase.to_string(),
            command: cmd.to_string(),
            args: args.to_vec(),
            placeholder,
            reason: match finding {
                InlineFinding::Untrusted(_) => "inline_code_interpolation".into(),
                InlineFinding::Malformed => "malformed_placeholder".into(),
            },
            suggested_args,
            manual_fix,
        });
    };
    check("main", step.exec_command.as_deref(), &step.exec_args);
    check(
        "setup",
        step.exec_setup_command.as_deref(),
        &step.exec_setup_args,
    );
    found
}

/// Every unsafe command line of a workflow, main chain then rollback chain.
pub fn classify_workflow(
    steps: &[WorkflowStep],
    on_failure: &[WorkflowStep],
) -> Vec<UnsafeExecStep> {
    steps
        .iter()
        .flat_map(|step| classify_step(step, false))
        .chain(on_failure.iter().flat_map(|step| classify_step(step, true)))
        .collect()
}

/// Run-time refusal: the explicit error a dangerous step fails with.
pub fn runtime_refusal(step: &WorkflowStep) -> Option<String> {
    let finding = classify_step(step, false).into_iter().next()?;
    let what = if finding.placeholder.is_empty() {
        "un placeholder mal formé".to_string()
    } else {
        format!("`{}`", finding.placeholder)
    };
    let phase = if finding.phase == "setup" {
        " (setup)"
    } else {
        ""
    };
    Some(format!(
        "Exec step `{}`{phase} refusé avant exécution : le code inline de `{}` interpole {what}, \
         qu'une valeur hostile (titre de ticket, sortie d'étape) pourrait faire exécuter. \
         Ouvre le workflow et applique la correction proposée (« Proposer une correction »), \
         ou {}.",
        step.name,
        finding.command,
        safe_recipe(
            finding
                .placeholder
                .trim_start_matches("{{")
                .trim_end_matches("}}")
        )
    ))
}

// ─── Assisted migration ──────────────────────────────────────────────────────

/// A placeholder occurrence in a script: byte span, path (filters and
/// fallback kept in `inner`), and whether it carried `|sh`.
struct Placeholder {
    start: usize,
    end: usize,
    path: String,
    /// The placeholder text to pass as an argument (`|sh` removed).
    as_argument: String,
    shell_filter: bool,
}

fn placeholders(script: &str) -> Result<Vec<Placeholder>, String> {
    let mut found = Vec::new();
    let mut offset = 0;
    while let Some(open) = script[offset..].find("{{") {
        let start = offset + open;
        let Some(close) = script[start + 2..].find("}}") else {
            return Err("un placeholder n'est pas fermé".into());
        };
        let end = start + 2 + close + 2;
        let inner = script[start + 2..end - 2].trim();
        let (path_part, fallback) = match inner.split_once("??") {
            Some((path, fallback)) => (path.trim(), Some(fallback.trim())),
            None => (inner, None),
        };
        let (path, shell_filter) = crate::workflows::template::split_shell_filter(path_part);
        let as_argument = match fallback {
            Some(fallback) => format!("{{{{{path} ?? {fallback}}}}}"),
            None => format!("{{{{{path}}}}}"),
        };
        found.push(Placeholder {
            start,
            end,
            path: path.to_string(),
            as_argument,
            shell_filter,
        });
        offset = end;
    }
    Ok(found)
}

/// A rewritten argv where every untrusted value travels as its own argument,
/// or the reason no rewrite is provably equivalent ("manual fix required").
/// Benign values keep the same output; hostile ones stay data.
pub fn suggest_args(cmd: &str, args: &[String]) -> Result<Vec<String>, String> {
    let codes = inline_code_args(cmd, args);
    let unsafe_codes: Vec<InlineCode> = codes
        .iter()
        .copied()
        .filter(|code| {
            crate::workflows::template::placeholder_paths(&args[code.index])
                .map(|paths| paths.iter().any(|path| !is_trusted_template_path(path)))
                .unwrap_or(true)
        })
        .collect();
    let manual = |why: &str| Err(format!("correction manuelle requise : {why}"));
    let [code] = unsafe_codes.as_slice() else {
        return manual("plusieurs arguments de code interpolent une valeur");
    };
    if code.attached
        || code.index == 0
        || !codes.contains(&InlineCode {
            index: code.index - 1,
            attached: false,
        })
    {
        return manual("le code est collé à son option (`-cCODE`, `--eval=CODE`)");
    }
    let script = &args[code.index];
    let rest = &args[code.index + 1..];
    let rewritten = if is_shell(cmd) {
        rewrite_shell(&args[..code.index], script, rest)
    } else if is_python(cmd) {
        rewrite_literals(&args[..code.index], script, rest, Language::Python)
    } else if is_node(cmd) {
        rewrite_literals(&args[..code.index], script, rest, Language::Node)
    } else {
        manual("aucune réécriture automatique pour cet interpréteur")
    }?;
    // Never suggest a line the validator would still refuse.
    if first_unsafe_placeholder(cmd, &rewritten).is_some() {
        return manual("la réécriture laisserait une valeur dans le code");
    }
    Ok(rewritten)
}

/// Shell: each untrusted placeholder becomes `"$N"` (or `$N` inside double
/// quotes; `${N}` from 10) and its value an argument after `$0`. Refused when the value sits
/// in single quotes, a heredoc, a command substitution or an `eval`, or when
/// the script reads the whole argument list.
fn rewrite_shell(head: &[String], script: &str, rest: &[String]) -> Result<Vec<String>, String> {
    let manual = |why: &str| Err(format!("correction manuelle requise : {why}"));
    if script.contains("<<") {
        return manual("le script contient un heredoc");
    }
    let found =
        placeholders(script).map_err(|why| format!("correction manuelle requise : {why}"))?;
    let mut bare = script.to_string();
    for placeholder in found.iter().rev() {
        bare.replace_range(placeholder.start..placeholder.end, " ");
    }
    let words: Vec<&str> = bare
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .collect();
    if words.contains(&"eval") || words.contains(&"shift") {
        return manual("le script utilise `eval` ou `shift`");
    }
    if ["$@", "$*", "$#", "${@", "${*", "${#"]
        .iter()
        .any(|token| script.contains(token))
    {
        return manual("le script lit la liste de ses arguments");
    }
    // A program that reads its own code (Python, awk, jq…) may receive the
    // value inside that code; only a value that is plainly a shell word stays.
    let nested = words.iter().any(|word| {
        let word = word.to_ascii_lowercase();
        NESTED_INTERPRETERS.contains(&word.as_str())
            || word.starts_with("python")
            || word.starts_with("pypy")
    });
    let contexts = shell_contexts(script, &found)?;
    // `$0` is `rest[0]` when present; new values follow the existing ones.
    let mut tail: Vec<String> = if rest.is_empty() {
        vec!["_".into()]
    } else {
        rest.to_vec()
    };
    let mut numbered: Vec<(String, usize)> = Vec::new();
    let mut rewritten = String::with_capacity(script.len());
    let mut cursor = 0;
    for (placeholder, context) in found.iter().zip(contexts) {
        rewritten.push_str(&script[cursor..placeholder.start]);
        cursor = placeholder.end;
        if is_trusted_template_path(&placeholder.path) {
            rewritten.push_str(&script[placeholder.start..placeholder.end]);
            continue;
        }
        let quoted = match context {
            ShellContext::Plain | ShellContext::Double { whole: true }
                if nested && !value_word_is_data(script, placeholder.start) =>
            {
                return manual("la valeur est passée à une option ou à un interpréteur imbriqué")
            }
            ShellContext::Double { .. } if placeholder.shell_filter => {
                return manual("`|sh` entre guillemets doubles")
            }
            ShellContext::Double { whole: false } if nested => return manual(
                "la valeur est dans une chaîne qu'un interpréteur imbriqué peut lire comme du code",
            ),
            ShellContext::Plain => false,
            ShellContext::Double { .. } => true,
            ShellContext::Single => return manual("une valeur entre guillemets simples"),
            ShellContext::Substitution => {
                return manual("une valeur dans une substitution de commande")
            }
        };
        let n = match numbered
            .iter()
            .find(|(arg, _)| *arg == placeholder.as_argument)
        {
            Some((_, n)) => *n,
            None => {
                tail.push(placeholder.as_argument.clone());
                let n = tail.len() - 1;
                numbered.push((placeholder.as_argument.clone(), n));
                n
            }
        };
        let parameter = if n < 10 {
            format!("${n}")
        } else {
            format!("${{{n}}}")
        };
        if quoted {
            rewritten.push_str(&parameter);
        } else {
            rewritten.push_str(&format!("\"{parameter}\""));
        }
    }
    rewritten.push_str(&script[cursor..]);
    let mut out = head.to_vec();
    out.push(rewritten);
    out.extend(tail);
    Ok(out)
}

/// Programs a shell script can hand code to.
const NESTED_INTERPRETERS: &[&str] = &[
    "node",
    "nodejs",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "awk",
    "gawk",
    "mawk",
    "sed",
    "jq",
    "yq",
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "osascript",
    "ssh",
    "su",
    "pwsh",
    "powershell",
    "lua",
    "tclsh",
    "expect",
    "rscript",
];

/// Whether the word holding the placeholder at `at` is plain data: neither
/// attached to an option (`--x={{v}}`) nor directly after an option or an
/// interpreter name.
fn value_word_is_data(script: &str, at: usize) -> bool {
    let line_start = script[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = script[line_start..at].trim_end_matches('"');
    let attached = before
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    if !attached.is_empty() {
        return !attached.starts_with('-');
    }
    let previous = before.split_whitespace().next_back().unwrap_or_default();
    let lower = previous.to_ascii_lowercase();
    !(previous.starts_with('-')
        || NESTED_INTERPRETERS.contains(&lower.as_str())
        || lower.starts_with("python"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellContext {
    Plain,
    Single,
    /// `whole`: the double-quoted word holds nothing but the placeholder.
    Double {
        whole: bool,
    },
    Substitution,
}

/// The quoting context of each placeholder: single quotes, and a stack of
/// double quotes, `$(…)` and backticks (each substitution opens a fresh
/// quoting context, as in the shell).
fn shell_contexts(script: &str, found: &[Placeholder]) -> Result<Vec<ShellContext>, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Frame {
        Double(usize),
        Substitution,
        Backtick,
    }
    let mut contexts = Vec::with_capacity(found.len());
    let mut stack: Vec<Frame> = Vec::new();
    let mut single = false;
    let mut next = found.iter().peekable();
    let mut chars = script.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        // A placeholder an escape jumped into cannot be classified.
        while next
            .peek()
            .is_some_and(|placeholder| placeholder.start < at)
        {
            contexts.push(ShellContext::Substitution);
            next.next();
        }
        if let Some(placeholder) = next.peek() {
            if at == placeholder.start {
                let substituted = stack
                    .iter()
                    .any(|frame| matches!(frame, Frame::Substitution | Frame::Backtick));
                contexts.push(if single {
                    ShellContext::Single
                } else if substituted {
                    ShellContext::Substitution
                } else if let Some(Frame::Double(start)) = stack.last() {
                    ShellContext::Double {
                        whole: start + 1 == placeholder.start
                            && script[placeholder.end..].starts_with('"'),
                    }
                } else {
                    ShellContext::Plain
                });
                let end = placeholder.end;
                next.next();
                while chars.peek().is_some_and(|(i, _)| *i < end) {
                    chars.next();
                }
                continue;
            }
        }
        if single {
            if c == '\'' {
                single = false;
            }
            continue;
        }
        let opens_substitution = c == '$' && chars.peek().is_some_and(|(_, n)| *n == '(');
        match (stack.last().copied(), c) {
            (_, '\\') => {
                chars.next();
            }
            (_, '$') if opens_substitution => {
                chars.next();
                stack.push(Frame::Substitution);
            }
            (Some(Frame::Double(_)), '"') => {
                stack.pop();
            }
            (Some(Frame::Backtick), '`') => {
                stack.pop();
            }
            (_, '`') => stack.push(Frame::Backtick),
            (Some(Frame::Double(_)), _) => {}
            (Some(Frame::Substitution), ')') => {
                stack.pop();
            }
            (_, '\'') => single = true,
            (_, '"') => stack.push(Frame::Double(at)),
            _ => {}
        }
    }
    contexts.extend(next.map(|_| ShellContext::Substitution));
    if single || !stack.is_empty() {
        return Err(
            "correction manuelle requise : les guillemets du script ne sont pas équilibrés".into(),
        );
    }
    Ok(contexts)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Python,
    Node,
}

/// Python / Node: a placeholder that is the whole content of a plain string
/// literal (`'{{x}}'`, `"{{x}}"`) becomes `sys.argv[N]` / `process.argv[N]`.
/// Anything else (bare code, part of a longer string, f-strings, template
/// literals, triple quotes) is not provably equivalent.
fn rewrite_literals(
    head: &[String],
    script: &str,
    rest: &[String],
    language: Language,
) -> Result<Vec<String>, String> {
    let manual = |why: &str| Err(format!("correction manuelle requise : {why}"));
    let found =
        placeholders(script).map_err(|why| format!("correction manuelle requise : {why}"))?;
    // Python: argv[0] is `-c`, extra arguments start at 1. Node `-e`/`-p`:
    // argv[0] is the node binary, extra arguments start at 1 too.
    let first_index = rest.len() + 1;
    let mut tail = rest.to_vec();
    let mut numbered: Vec<(String, usize)> = Vec::new();
    let mut rewritten = String::with_capacity(script.len());
    let mut cursor = 0;
    let openings = literal_openings(script, &found, language)?;
    for (index, placeholder) in found.iter().enumerate() {
        if is_trusted_template_path(&placeholder.path) {
            continue;
        }
        if placeholder.shell_filter {
            return manual("`|sh` dans du code non shell");
        }
        let before = script[..placeholder.start].chars().next_back();
        let after = script[placeholder.end..].chars().next();
        let quote = match (before, after) {
            (Some(q @ ('\'' | '"')), Some(a)) if a == q => q,
            _ => return manual("la valeur n'est pas une chaîne littérale entière"),
        };
        let literal_start = placeholder.start - 1;
        if openings[index] != Some(literal_start) {
            return manual("la valeur n'est pas une chaîne littérale entière");
        }
        let prefix = script[..literal_start].chars().next_back();
        if prefix.is_some_and(|p| p.is_ascii_alphanumeric() || p == '_' || p == quote || p == '\\')
        {
            return manual("chaîne préfixée, triple ou échappée");
        }
        if script[placeholder.end + 1..].starts_with(quote) {
            return manual("chaîne triple");
        }
        if literal_start < cursor {
            return manual("deux valeurs dans la même chaîne");
        }
        let n = match numbered
            .iter()
            .find(|(arg, _)| *arg == placeholder.as_argument)
        {
            Some((_, n)) => *n,
            None => {
                tail.push(placeholder.as_argument.clone());
                let n = first_index + numbered.len();
                numbered.push((placeholder.as_argument.clone(), n));
                n
            }
        };
        rewritten.push_str(&script[cursor..literal_start]);
        rewritten.push_str(&match language {
            Language::Python => format!("sys.argv[{n}]"),
            Language::Node => format!("process.argv[{n}]"),
        });
        cursor = placeholder.end + 1;
    }
    rewritten.push_str(&script[cursor..]);
    if language == Language::Python && !rewritten.contains("import sys") {
        if rewritten.trim_start().starts_with("from __future__") {
            return manual("le script commence par `from __future__`");
        }
        rewritten = format!("import sys\n{rewritten}");
    }
    let mut out = head.to_vec();
    out.push(rewritten);
    out.extend(tail);
    Ok(out)
}

/// For each placeholder, the byte index of the plain (single-line quote)
/// string literal it sits in, when that literal was opened in code; `None`
/// inside a comment, a triple-quoted string or a JS template literal.
fn literal_openings(
    script: &str,
    found: &[Placeholder],
    language: Language,
) -> Result<Vec<Option<usize>>, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Plain { quote: char, start: usize },
        Triple { quote: char },
        Template,
        Comment { block: bool },
    }
    let bytes = script.as_bytes();
    let at = |i: usize, text: &str| bytes.get(i..i + text.len()) == Some(text.as_bytes());
    let mut openings = Vec::with_capacity(found.len());
    let mut state = State::Code;
    let mut next = found.iter().peekable();
    let mut i = 0;
    while i < bytes.len() {
        // A placeholder an escape jumped into is not a whole literal.
        while next.peek().is_some_and(|placeholder| placeholder.start < i) {
            openings.push(None);
            next.next();
        }
        if let Some(placeholder) = next.peek() {
            if i == placeholder.start {
                openings.push(match state {
                    State::Plain { start, .. } => Some(start),
                    _ => None,
                });
                i = placeholder.end;
                next.next();
                continue;
            }
        }
        let c = bytes[i];
        let step = match state {
            State::Code => {
                if language == Language::Python && c == b'#'
                    || language == Language::Node && at(i, "//")
                {
                    state = State::Comment { block: false };
                    1
                } else if language == Language::Node && at(i, "/*") {
                    state = State::Comment { block: true };
                    2
                } else if c == b'\'' || c == b'"' {
                    let quote = c as char;
                    if language == Language::Python && at(i, &format!("{quote}{quote}{quote}")) {
                        state = State::Triple { quote };
                        3
                    } else {
                        state = State::Plain { quote, start: i };
                        1
                    }
                } else {
                    if language == Language::Node && c == b'`' {
                        state = State::Template;
                    }
                    1
                }
            }
            State::Plain { quote, .. } => {
                if c == b'\\' {
                    2
                } else {
                    if c == quote as u8 || c == b'\n' {
                        state = State::Code;
                    }
                    1
                }
            }
            State::Triple { quote } => {
                if c == b'\\' {
                    2
                } else if at(i, &format!("{quote}{quote}{quote}")) {
                    state = State::Code;
                    3
                } else {
                    1
                }
            }
            State::Template => {
                if c == b'\\' {
                    2
                } else {
                    if c == b'`' {
                        state = State::Code;
                    }
                    1
                }
            }
            State::Comment { block } => {
                if !block && c == b'\n' || block && at(i, "*/") {
                    state = State::Code;
                    if block {
                        2
                    } else {
                        1
                    }
                } else {
                    1
                }
            }
        };
        i += step;
    }
    openings.extend(next.map(|_| None));
    if !matches!(state, State::Code | State::Comment { block: false }) {
        return Err(
            "correction manuelle requise : les chaînes du script ne sont pas fermées".into(),
        );
    }
    Ok(openings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::template::TemplateContext;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    const HOSTILE: &str = "a'b\"c $(touch pwned) ; touch pwned2 `touch pwned3` \\ é🦀";

    #[test]
    fn shell_values_move_to_positional_arguments_in_order() {
        assert_eq!(
            suggest_args("bash", &args(&["-c", "echo {{x}}"])).unwrap(),
            args(&["-c", "echo \"$1\"", "_", "{{x}}"])
        );
        assert_eq!(
            suggest_args(
                "bash",
                &args(&["-ec", "printf '%s' \"a={{a}} b={{b|sh}}\" {{a}} {{run.id}}"])
            )
            .unwrap_err(),
            "correction manuelle requise : `|sh` entre guillemets doubles"
        );
        assert_eq!(
            suggest_args(
                "bash",
                &args(&[
                    "-ec",
                    "printf '%s %s' \"a={{a}}\" {{b|sh}} {{a}} {{run.id}}"
                ])
            )
            .unwrap(),
            args(&[
                "-ec",
                "printf '%s %s' \"a=$1\" \"$2\" \"$1\" {{run.id}}",
                "_",
                "{{a}}",
                "{{b}}"
            ])
        );
        // A whole quoted word next to a nested interpreter stays a shell word.
        assert_eq!(
            suggest_args(
                "bash",
                &args(&[
                    "-c",
                    "a=\"$(python3 -c 'print(1)')\"\n[ \"$a\" = \"{{x}}\" ] && echo y"
                ])
            )
            .unwrap(),
            args(&[
                "-c",
                "a=\"$(python3 -c 'print(1)')\"\n[ \"$a\" = \"$1\" ] && echo y",
                "_",
                "{{x}}"
            ])
        );
        // Existing positional arguments keep their numbers.
        assert_eq!(
            suggest_args(
                "sh",
                &args(&["-c", "echo \"$1\" {{x ?? \"none\"}}", "me", "one"])
            )
            .unwrap(),
            args(&["-c", "echo \"$1\" \"$2\"", "me", "one", "{{x ?? \"none\"}}"])
        );
    }

    #[test]
    fn unprovable_rewrites_require_a_manual_fix() {
        for (cmd, line) in [
            ("bash", vec!["-c", "cat <<EOF\n{{x}}\nEOF"]),
            ("bash", vec!["-c", "echo '{{x}}'"]),
            ("bash", vec!["-c", "echo \"$(printf {{x}})\""]),
            ("bash", vec!["-c", "echo `echo {{x}}`"]),
            ("bash", vec!["-c", "eval echo {{x}}"]),
            ("bash", vec!["-c", "echo {{x}} \"$@\""]),
            ("bash", vec!["-cecho {{x}}"]),
            ("python3", vec!["-c", "print('Hi {{x}}')"]),
            ("python3", vec!["-c", "print(f'{{x}}')"]),
            ("python3", vec!["-c", "n = {{x}}"]),
            ("python3", vec!["-c", "print('''{{x}}''')"]),
            ("node", vec!["-e", "console.log(`a '{{x}}' b`)"]),
            ("python3", vec!["-c", "print(\"it's '{{x}}'\")"]),
            ("python3", vec!["-c", "# '{{x}}'\nprint(1)"]),
            ("python3", vec!["-c", "print('\\{{x}}')"]),
            ("node", vec!["-e", "console.log('a' + '{{x}}b')"]),
            ("perl", vec!["-e", "print '{{x}}'"]),
            ("bash", vec!["-c", "python3 -c \"me='{{x}}'\""]),
            ("bash", vec!["-c", "python3 -c \"{{x}}\""]),
            ("bash", vec!["-c", "jq -r {{x}} f.json"]),
            ("bash", vec!["-c", "awk \"/{{x}}/\" f"]),
        ] {
            let line = args(&line);
            assert!(
                first_unsafe_placeholder(cmd, &line).is_some(),
                "{cmd} {line:?}"
            );
            let reason = suggest_args(cmd, &line).unwrap_err();
            assert!(
                reason.starts_with("correction manuelle requise"),
                "{cmd} {line:?}"
            );
        }
    }

    #[test]
    fn python_and_node_literals_become_argv_reads() {
        assert_eq!(
            suggest_args(
                "python3",
                &args(&["-c", "print('{{x}}', \"{{y}}\", '{{x}}')"])
            )
            .unwrap(),
            args(&[
                "-c",
                "import sys\nprint(sys.argv[1], sys.argv[2], sys.argv[1])",
                "{{x}}",
                "{{y}}"
            ])
        );
        assert_eq!(
            suggest_args("node", &args(&["-e", "console.log(\"{{x}}\")"])).unwrap(),
            args(&["-e", "console.log(process.argv[1])", "{{x}}"])
        );
    }

    #[test]
    fn classification_covers_main_and_setup_and_ignores_safe_lines() {
        let mut step = crate::models::WorkflowStep {
            name: "build".into(),
            step_type: crate::models::StepType::Exec,
            exec_command: Some("bash".into()),
            exec_args: args(&["-c", "echo \"$1\"", "_", "{{issue.title}}"]),
            exec_setup_command: Some("python3".into()),
            exec_setup_args: args(&["-c", "print('{{issue.body}}')"]),
            ..Default::default()
        };
        let found = classify_step(&step, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].phase, "setup");
        assert_eq!(found[0].placeholder, "{{issue.body}}");
        assert!(found[0].suggested_args.is_some());
        assert!(runtime_refusal(&step).unwrap().contains("{{issue.body}}"));
        step.exec_setup_args = args(&["-c", "print(1)"]);
        assert!(classify_step(&step, false).is_empty());
        assert!(runtime_refusal(&step).is_none());
    }

    fn run(cmd: &str, line: &[String], value: &str, dir: &std::path::Path) -> Option<String> {
        let mut ctx = TemplateContext::new();
        ctx.set("x", value);
        let rendered: Vec<String> = line
            .iter()
            .map(|arg| ctx.render_strict(arg).unwrap())
            .collect();
        let output = crate::core::cmd::sync_cmd(cmd)
            .args(&rendered)
            .current_dir(dir)
            .output()
            .ok()?;
        assert!(output.status.success(), "{cmd} {rendered:?}: {output:?}");
        Some(String::from_utf8(output.stdout).unwrap())
    }

    /// Real execution: the migrated line prints a benign value exactly like
    /// the original, and a hostile one verbatim, without running any of it.
    #[cfg(unix)]
    #[test]
    fn migrated_lines_keep_benign_output_and_never_run_a_hostile_value() {
        for (cmd, original) in [
            ("bash", vec!["-c", "echo start {{x}} end"]),
            ("bash", vec!["-ec", "printf '%s\\n' \"value: {{x}}\""]),
            ("python3", vec!["-c", "print('{{x}}')"]),
            ("node", vec!["-e", "console.log(\"{{x}}\")"]),
        ] {
            let original = args(&original);
            let migrated = suggest_args(cmd, &original).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let Some(before) = run(cmd, &original, "benign value", dir.path()) else {
                eprintln!("{cmd} is not installed here; skipped");
                continue;
            };
            let after = run(cmd, &migrated, "benign value", dir.path()).unwrap();
            assert_eq!(before, after, "{cmd}: {migrated:?}");

            let printed = run(cmd, &migrated, HOSTILE, dir.path()).unwrap();
            assert!(printed.contains(HOSTILE), "{cmd}: {printed}");
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "{cmd}: the hostile value ran"
            );
            assert!(
                first_unsafe_placeholder(cmd, &migrated).is_none(),
                "{migrated:?}"
            );
        }
    }
}
