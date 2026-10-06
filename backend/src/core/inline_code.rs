//! Inline code handed to an interpreter (`bash -c`, `python3 -c`, `node -e`…)
//! in a workflow Exec step: where it sits in argv, whether it interpolates a
//! template value, and how to move such values into separate argv entries.
//!
//! One classifier serves the save-time validator, the run-time refusal and
//! the per-workflow report, so the three can never disagree.

use crate::models::{UnsafeExecStep, WorkflowStep};

/// Shells whose `-c` argument is a script.
const SHELLS: &[&str] = &[
    "bash", "sh", "zsh", "dash", "fish", "ksh", "ash", "mksh", "yash", "csh", "tcsh", "xonsh",
    "nu", "elvish", "rbash", "oksh", "loksh", "posh", "hush", "osh", "ysh", "rc", "es",
];

/// Inline-code options of an interpreter: the short option letters (also
/// valid inside a cluster such as `-ec`, or with the code attached, as in
/// `-cprint(1)`) and the long options (`--eval`, `--eval=<code>`).
struct InlineCodeOptions {
    letters: &'static [char],
    long: &'static [&'static str],
    /// PowerShell takes single-dash long names, abbreviated, any case.
    case_insensitive: bool,
    /// The code may be glued to its option (`python -cCODE`). Shells never
    /// glue `-c`: in `bash -cx`, `x` is another flag and the script is the
    /// next operand.
    glued: bool,
}

use crate::core::argv_roles::Trust;

/// The normalised program name (path, case, `.exe` and version suffix
/// removed), see [`crate::core::argv_roles::normalize_command`].
fn base_name(cmd: &str) -> String {
    crate::core::argv_roles::normalize_command(cmd)
}

fn is_shell(cmd: &str) -> bool {
    SHELLS.contains(&base_name(cmd).as_str())
}

/// Any Python: `python`, `python3.12`, `pypy3`, and explicitly every name
/// that starts with `python` or `pypy` (`python3-dbg`, `pythonw`).
fn is_python(cmd: &str) -> bool {
    let lower = base_name(cmd);
    lower.starts_with("python") || lower.starts_with("pypy")
}

fn is_node(cmd: &str) -> bool {
    matches!(base_name(cmd).as_str(), "node" | "nodejs")
}

fn inline_code_options(cmd: &str) -> Option<InlineCodeOptions> {
    let lower = base_name(cmd);
    let (letters, long, case_insensitive, glued): (&[char], &[&str], bool, bool) = if is_shell(cmd)
    {
        (&['c'], &["--command"], false, false)
    } else if is_python(cmd) {
        (&['c'], &["--command"], false, true)
    } else if matches!(lower.as_str(), "node" | "nodejs" | "bun" | "deno") {
        (&['e', 'p'], &["--eval", "--print"], false, true)
    } else if matches!(lower.as_str(), "ruby" | "perl") {
        (&['e', 'E'], &[], false, true)
    } else if lower == "php" {
        (&['r'], &[], false, true)
    } else if matches!(lower.as_str(), "pwsh" | "powershell") {
        (&['c', 'e'], &[], true, false)
    } else {
        return None;
    };
    Some(InlineCodeOptions {
        letters,
        long,
        case_insensitive,
        glued,
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
                Some((at, c)) => (true, options.glued && at + c.len_utf8() < cluster.len()),
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
    /// A value sits where the interpreter still reads options, so it could
    /// turn into an option or into code once rendered (`{{mode}}` → `-c`).
    OptionPosition(String),
    /// A value names the program to run (`{{cmd}}`, `env {{cmd}}`).
    TemplatedExecutable(String),
    /// A value reaches a program the classifier does not model, which may
    /// read its arguments as anything: `(program, path)`. Accepted only when
    /// a human approved the step.
    UnmodelledProgram(String, String),
}

/// The first unsafe placeholder of `cmd args`, if any: in the program name,
/// in program text (only `{{run.id}}` and `{{time.now…}}` are accepted
/// there, whatever filter a value went through), where the program still
/// reads options, or where it names a program to run.
pub fn first_unsafe_placeholder(cmd: &str, args: &[String]) -> Option<InlineFinding> {
    first_unsafe_placeholder_with(cmd, args, false)
}

/// [`first_unsafe_placeholder`] for a line with this [`Trust`]: who wrote it,
/// whether a human approved it, which scripts its step pins.
pub fn first_unsafe_placeholder_with(
    cmd: &str,
    args: &[String],
    trust: impl Into<Trust>,
) -> Option<InlineFinding> {
    let trust = trust.into();
    if let Some(finding) = untrusted_in(cmd) {
        return Some(match finding {
            Some(path) => InlineFinding::TemplatedExecutable(path),
            None => InlineFinding::Malformed,
        });
    }
    let tainted = tainted_templates(args);
    let roles = crate::core::argv_roles::roles_with(cmd, args, &tainted, &trust);
    let finding_at = |index: usize, make: fn(String) -> InlineFinding| {
        Some(match untrusted_in(&args[index]).flatten() {
            Some(path) => make(path),
            None => InlineFinding::Malformed,
        })
    };
    use crate::core::argv_roles::Role;
    // Program text first: its message is the most specific.
    if let Some(index) = (0..args.len()).find(|&i| tainted[i] && roles[i] == Role::Code) {
        return finding_at(index, InlineFinding::Untrusted);
    }
    for index in (0..args.len()).filter(|&i| tainted[i]) {
        match roles[index] {
            Role::Executable => return finding_at(index, InlineFinding::TemplatedExecutable),
            Role::Option => return finding_at(index, InlineFinding::OptionPosition),
            _ => {}
        }
    }
    if !trust.approved {
        if let Some(index) = (0..args.len()).find(|&i| tainted[i] && roles[i] == Role::Unmodelled) {
            let program = crate::core::argv_roles::owning_program(cmd, args, &roles, index);
            return Some(match untrusted_in(&args[index]).flatten() {
                Some(path) => InlineFinding::UnmodelledProgram(program, path),
                None => InlineFinding::Malformed,
            });
        }
    }
    None
}

/// `Some(Some(path))` for the first untrusted placeholder in `text`,
/// `Some(None)` for a malformed one, `None` when `text` is safe.
fn untrusted_in(text: &str) -> Option<Option<String>> {
    match crate::workflows::template::placeholder_paths(text) {
        Ok(paths) => paths
            .into_iter()
            .find(|path| !is_trusted_template_path(path))
            .map(Some),
        Err(_) => Some(None),
    }
}

/// The recipe shown with every refusal.
pub fn safe_recipe(path: &str) -> String {
    format!(
        "passe la valeur en argument séparé après le script, que l'interpréteur ne lit jamais \
         comme du code ni comme une option : `exec_args=[\"-c\", \"echo \\\"$1\\\"\", \"_\", \"{{{{{path}}}}}\"]` \
         pour un shell, `[\"-c\", \"import sys; print(sys.argv[1])\", \"{{{{{path}}}}}\"]` pour Python, \
         `[\"-e\", \"console.log(process.argv[1])\", \"--\", \"{{{{{path}}}}}\"]` pour Node (le `--` est \
         obligatoire), sans autre option, ou via `exec_stdin` à un programme de données ou à \
         l'une de ces formes (code inline ou script donné). Écrite par un agent, la ligne attend \
         ensuite qu'un humain approuve l'étape dans l'éditeur"
    )
}

/// Save-time refusal for one command line (main or setup).
pub fn validation_error(
    step: &str,
    cmd: &str,
    args: &[String],
    trust: impl Into<Trust>,
) -> Option<String> {
    refusal(&format!("Step Exec « {step} »"), cmd, args, &trust.into())
}

/// Save-time refusal for a Quick Exec, with the same rule, message and
/// recipe as a workflow step. When the rewrite is provably equivalent it is
/// spelled out, so the fix is one copy away.
pub fn quick_exec_validation_error(
    name: &str,
    cmd: &str,
    args: &[String],
    trust: impl Into<Trust>,
) -> Option<String> {
    let message = refusal(&format!("Quick Exec « {name} »"), cmd, args, &trust.into())?;
    Some(match suggest_args(cmd, args) {
        Ok(fixed) => format!(
            "{message} Arguments proposés : {}",
            serde_json::to_string(&fixed).unwrap_or_default()
        ),
        Err(_) => message,
    })
}

fn refusal(subject: &str, cmd: &str, args: &[String], trust: &Trust) -> Option<String> {
    match first_unsafe_placeholder_with(cmd, args, trust.clone())? {
        InlineFinding::Malformed => Some(format!(
            "{subject} : le code inline de `{cmd}` contient un placeholder mal formé."
        )),
        InlineFinding::Untrusted(path) => Some(format!(
            "{subject} : le script inline de `{cmd}` interpole `{{{{{path}}}}}` — \
             dans du code, une valeur peut toujours être exécutée (heredoc, eval, guillemets), \
             même filtrée par `|sh`. {}.",
            safe_recipe(&path)
        )),
        InlineFinding::OptionPosition(path) => Some(format!(
            "{subject} : `{{{{{path}}}}}` est à une position où `{cmd}` lit encore ses options — \
             une fois rendue, la valeur pourrait devenir une option ou du code (`-c`, `--eval=…`). \
             {}.",
            safe_recipe(&path)
        )),
        InlineFinding::TemplatedExecutable(path) => Some(format!(
            "{subject} : `{{{{{path}}}}}` choisirait le programme exécuté — le programme d'une \
             étape Exec (et celui qu'un `env`, `sudo`, `xargs`, `find -exec` ou `docker run` \
             lance) doit être écrit en clair ; seules ses données peuvent venir d'une valeur."
        )),
        InlineFinding::UnmodelledProgram(program, path) => {
            Some(unmodelled_message(subject, &program, &path))
        }
    }
}

/// The refusal for a value reaching an unmodelled program, with what to do.
pub fn unmodelled_message(subject: &str, program: &str, path: &str) -> String {
    format!(
        "{subject} : `{{{{{path}}}}}` est passé à `{program}` dans une ligne que Kronn ne vérifie \
         pas seul. Un humain doit approuver cette étape dans l'éditeur (« {program} reçoit des \
         valeurs du run ; je confirme qu'il traite ses arguments comme de simples données »). \
         Une ligne écrite par un agent attend toujours cette approbation."
    )
}

/// The script of a line in a trusted script shape (`python3 tool.py …`).
pub fn trusted_script(cmd: &str, args: &[String]) -> Option<String> {
    let tainted = tainted_templates(args);
    matches!(
        crate::core::argv_roles::trusted_shape(cmd, args, &tainted),
        Ok(crate::core::argv_roles::Shape::Script)
    )
    .then(|| args[0].clone())
}

/// The [`Trust`] of a saved step's lines.
pub fn step_trust(step: &WorkflowStep) -> Trust {
    Trust {
        approved: step.exec_unmodelled_args_approved == Some(true),
        agent_written: step.exec_agent_written == Some(true),
        declared_scripts: step
            .exec_script_files
            .iter()
            .map(|file| file.path.clone())
            .collect(),
    }
}

/// Every unsafe command line of a saved step (main, then setup, or each
/// inline Quick Exec source of a CollectApiData step), with a suggested
/// rewrite when one is provably equivalent.
pub fn classify_step(step: &WorkflowStep, on_failure: bool) -> Vec<UnsafeExecStep> {
    let trust = step_trust(step);
    let mut found = Vec::new();
    let mut check = |phase: &str, alias: Option<&str>, cmd: Option<&str>, args: &[String]| {
        let Some(cmd) = cmd.map(str::trim).filter(|cmd| !cmd.is_empty()) else {
            return;
        };
        let Some(finding) = first_unsafe_placeholder_with(cmd, args, trust.clone()) else {
            return;
        };
        let placeholder = match &finding {
            InlineFinding::Untrusted(path)
            | InlineFinding::OptionPosition(path)
            | InlineFinding::TemplatedExecutable(path)
            | InlineFinding::UnmodelledProgram(_, path) => format!("{{{{{path}}}}}"),
            InlineFinding::Malformed => String::new(),
        };
        let (suggested_args, manual_fix) = match (&finding, suggest_args(cmd, args)) {
            (InlineFinding::UnmodelledProgram(program, path), _) => (
                None,
                Some(unmodelled_message(
                    &format!("Step Exec « {} »", step.name),
                    program,
                    path,
                )),
            ),
            (_, Ok(rewritten)) => (Some(rewritten), None),
            (_, Err(reason)) => (None, Some(reason)),
        };
        found.push(UnsafeExecStep {
            step_name: step.name.clone(),
            on_failure,
            phase: phase.to_string(),
            source_alias: alias.map(str::to_string),
            command: cmd.to_string(),
            args: args.to_vec(),
            placeholder,
            reason: match finding {
                InlineFinding::Untrusted(_) => "inline_code_interpolation".into(),
                InlineFinding::OptionPosition(_) => "option_position_interpolation".into(),
                InlineFinding::TemplatedExecutable(_) => "templated_executable".into(),
                InlineFinding::UnmodelledProgram(..) => "unmodelled_program".into(),
                InlineFinding::Malformed => "malformed_placeholder".into(),
            },
            suggested_args,
            manual_fix,
        });
    };
    match step.step_type {
        crate::models::StepType::Exec => {
            check("main", None, step.exec_command.as_deref(), &step.exec_args);
            check(
                "setup",
                None,
                step.exec_setup_command.as_deref(),
                &step.exec_setup_args,
            );
            let command = step
                .exec_command
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if let Some(stdin) = step.exec_stdin.as_deref() {
                if let Some(placeholder) = stdin_finding(command, &step.exec_args, stdin, &trust) {
                    let unmodelled = stdin_unmodelled_program(command, &step.exec_args, &trust);
                    let manual_fix = match &unmodelled {
                        Some(program) => unmodelled_message(
                            &format!("Step Exec « {} » (stdin)", step.name),
                            program,
                            placeholder.trim_start_matches("{{").trim_end_matches("}}"),
                        ),
                        None => "correction manuelle requise : le programme lit son code sur \
                                 stdin ; donne-lui un script ou du code inline sans valeur, et \
                                 passe la valeur en argument séparé"
                            .into(),
                    };
                    found.push(UnsafeExecStep {
                        step_name: step.name.clone(),
                        on_failure,
                        phase: "stdin".to_string(),
                        source_alias: None,
                        command: command.to_string(),
                        args: stdin_line(&step.exec_args, stdin),
                        placeholder,
                        reason: if unmodelled.is_some() {
                            "unmodelled_program".into()
                        } else {
                            "stdin_program".into()
                        },
                        suggested_args: None,
                        manual_fix: Some(manual_fix),
                    });
                }
            }
        }
        crate::models::StepType::CollectApiData => {
            for source in step
                .collect_api_data
                .iter()
                .flat_map(|config| &config.sources)
            {
                if let Some(exec) = &source.quick_exec {
                    check(
                        "source",
                        Some(&source.alias),
                        Some(&exec.command),
                        &exec.args,
                    );
                }
            }
        }
        _ => {}
    }
    found
}

/// Save-time refusal of an `exec_stdin` that the program would run as code.
pub fn stdin_validation_error(
    step: &str,
    cmd: &str,
    args: &[String],
    stdin: &str,
    trust: impl Into<Trust>,
) -> Option<String> {
    let trust = trust.into();
    let path = stdin_finding(cmd, args, stdin, &trust)?;
    if let Some(program) = stdin_unmodelled_program(cmd, args, &trust) {
        return Some(unmodelled_message(
            &format!("Step Exec « {step} » (stdin)"),
            &program,
            path.trim_start_matches("{{").trim_end_matches("}}"),
        ));
    }
    Some(format!(
        "Step Exec « {step} » : `{cmd}` lit son programme sur stdin, et `exec_stdin` y place \
         `{path}` — la valeur serait exécutée. Donne le code au programme (script ou code inline \
         sans valeur) et passe la valeur en argument séparé, ou fais lire stdin par un code qui \
         la traite comme des données (`[\"-c\", \"import sys; print(sys.stdin.read())\"]`)."
    ))
}

/// What a templated `exec_stdin` reaches.
enum StdinReach {
    /// A data-only program, or a trusted shape: bash/sh/python3/node given
    /// their code inline or as a script ([`crate::core::argv_roles::trusted_shape`]).
    Data,
    /// A program that may read it as code: refused, approval or not.
    Code,
    /// Anything else: accepted only when a human approved the step.
    Unchecked(String),
}

fn stdin_reach(cmd: &str, args: &[String], trust: &Trust) -> StdinReach {
    // Code readers first: no approval or shape lifts them. Then the shape
    // alone decides; a value inside the code is the argument check's refusal.
    if reads_program_from_stdin(cmd, args) {
        return StdinReach::Code;
    }
    let untainted = vec![false; args.len()];
    if !trust.agent_written
        && (crate::core::argv_roles::is_data_only(cmd)
            || crate::core::argv_roles::trusted_by_shape(cmd, args, &untainted, trust))
    {
        return StdinReach::Data;
    }
    StdinReach::Unchecked(
        crate::core::argv_roles::launched_program(cmd, args)
            .map_or_else(|| cmd.trim().to_string(), |(inner, _)| inner),
    )
}

/// The first untrusted placeholder of `stdin` when it reaches code, or a
/// program outside the trusted set that no human approved.
fn stdin_finding(cmd: &str, args: &[String], stdin: &str, trust: &Trust) -> Option<String> {
    let finding = untrusted_in(stdin)?;
    match stdin_reach(cmd, args, trust) {
        StdinReach::Data => return None,
        StdinReach::Unchecked(_) if trust.approved => return None,
        _ => {}
    }
    Some(match finding {
        Some(path) => format!("{{{{{path}}}}}"),
        None => "un placeholder mal formé".to_string(),
    })
}

/// The program a templated stdin reaches when only a human approval can let
/// it through.
pub fn stdin_unmodelled_program(cmd: &str, args: &[String], trust: &Trust) -> Option<String> {
    match stdin_reach(cmd, args, trust) {
        StdinReach::Unchecked(program) => Some(program),
        _ => None,
    }
}

/// The identity of a stdin line for the unchanged-line exception: the
/// program's arguments followed by the stdin template.
pub fn stdin_line(args: &[String], stdin: &str) -> Vec<String> {
    let mut line = args.to_vec();
    line.push(stdin.to_string());
    line
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
    if finding.reason == "unmodelled_program" {
        return Some(format!(
            "Exec step `{}` refusé avant exécution — {}",
            step.name,
            finding.manual_fix.unwrap_or_default()
        ));
    }
    let what = if finding.placeholder.is_empty() {
        "un placeholder mal formé".to_string()
    } else {
        format!("`{}`", finding.placeholder)
    };
    let phase = match finding.phase.as_str() {
        "setup" => " (setup)",
        "stdin" => " (stdin)",
        _ => "",
    };
    Some(format!(
        "Exec step `{}`{phase} refusé avant exécution : `{}` reçoit {what} dans son code (inline ou \
         stdin) ou là où il lit encore ses options, qu'une valeur hostile (titre de ticket, sortie d'étape) \
         pourrait faire exécuter. \
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

/// How an interpreter stops reading options, which decides where a value can
/// sit without ever becoming an option or code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionParsing {
    /// Shells and Python: options end with the inline code, `--`, or the
    /// first plain argument (the script).
    StopsAtCodeOrScript,
    /// Node, Bun, Perl, Ruby: options continue after the inline code; they end
    /// with `--` or a script file (by extension).
    StopsAtDashDashOrScript(&'static [&'static str]),
    /// PHP, Deno: only `--` is a provable end of options.
    StopsAtDashDash,
    /// PowerShell: `-Command` takes the rest of the line, no data position.
    Never,
}

fn option_parsing(cmd: &str) -> Option<OptionParsing> {
    let lower = base_name(cmd);
    Some(if is_shell(cmd) || is_python(cmd) {
        OptionParsing::StopsAtCodeOrScript
    } else if matches!(lower.as_str(), "node" | "nodejs" | "bun") {
        OptionParsing::StopsAtDashDashOrScript(&[".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"])
    } else if lower == "perl" {
        OptionParsing::StopsAtDashDashOrScript(&[".pl", ".pm"])
    } else if lower == "ruby" {
        OptionParsing::StopsAtDashDashOrScript(&[".rb"])
    } else if matches!(lower.as_str(), "php" | "deno") {
        OptionParsing::StopsAtDashDash
    } else if matches!(lower.as_str(), "pwsh" | "powershell") {
        OptionParsing::Never
    } else {
        return None;
    })
}

/// Options that take the next argument as their value.
fn value_options(cmd: &str) -> &'static [&'static str] {
    let lower = base_name(cmd);
    if is_shell(cmd) {
        &["-o", "+o", "-O", "+O", "--rcfile", "--init-file"]
    } else if is_python(cmd) {
        &["-W", "-X", "-m"]
    } else if matches!(lower.as_str(), "node" | "nodejs" | "bun") {
        &[
            "-r",
            "--require",
            "--import",
            "--loader",
            "--experimental-loader",
            "-C",
            "--conditions",
            "--input-type",
            "--env-file",
            "--title",
            "--inspect-port",
        ]
    } else if lower == "ruby" {
        &["-I", "-r", "-C", "-E"]
    } else if lower == "perl" {
        &["-I", "-M", "-m"]
    } else {
        &[]
    }
}

/// Whether `arg` is an inline-code option of `cmd`, and whether its code is
/// attached to it.
fn code_option(options: &InlineCodeOptions, arg: &str) -> Option<bool> {
    if let Some(rest) = arg.strip_prefix("--") {
        let (name, value) = rest
            .split_once('=')
            .map_or((rest, None), |(name, value)| (name, Some(value)));
        let flag = format!("--{}", name.to_ascii_lowercase());
        return options
            .long
            .contains(&flag.as_str())
            .then_some(value.is_some());
    }
    let cluster = arg.strip_prefix('-')?;
    let (at, c) = cluster.char_indices().find(|(_, c)| {
        options.letters.contains(c)
            || (options.case_insensitive && options.letters.contains(&c.to_ascii_lowercase()))
    })?;
    Some(options.glued && at + c.len_utf8() < cluster.len())
}

/// For a shell given `-c`: `Some(script index)`, or `Some(None)` when no
/// script is found before the arguments run out. `-c` is a flag: the script
/// is the first operand after every option (`bash -c -- X` and `bash -c -x X`
/// run `X`), and a value in the option zone never ends it. `None` without
/// `-c`.
fn shell_script(args: &[String], tainted: &[bool]) -> Option<Option<usize>> {
    let mut command = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if tainted.get(i).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        if arg == "--" {
            return command.then_some((i + 1 < args.len()).then_some(i + 1));
        }
        if arg.len() > 1 && (arg.starts_with('-') || arg.starts_with('+')) {
            if !arg.starts_with("--") && arg[1..].contains('c') {
                command = true;
            }
            i += if matches!(arg, "-o" | "+o" | "-O" | "+O") {
                2
            } else {
                1
            };
            continue;
        }
        return command.then_some(Some(i));
    }
    command.then_some(None)
}

/// Where an interpreter's arguments become plain data (`args.len()` when
/// never). A tainted argument never ends option parsing: at save time it is
/// a placeholder, at run time its rendered text is not trusted.
fn interpreter_data_start(cmd: &str, args: &[String], tainted: &[bool]) -> Option<usize> {
    let parsing = option_parsing(cmd)?;
    let options = inline_code_options(cmd)?;
    let is_tainted = |i: usize| tainted.get(i).copied().unwrap_or(false);
    if parsing == OptionParsing::Never {
        return Some(args.len());
    }
    if is_shell(cmd) {
        if let Some(script) = shell_script(args, tainted) {
            return Some(script.map_or(args.len(), |script| script + 1));
        }
    }
    let values = value_options(cmd);
    let mut i = 0;
    while i < args.len() {
        if is_tainted(i) {
            i += 1;
            continue;
        }
        let arg = &args[i];
        if arg == "--" {
            // Shells and Python read the script name right after `--`.
            return Some(if parsing == OptionParsing::StopsAtCodeOrScript {
                (i + 2).min(args.len())
            } else {
                i + 1
            });
        }
        if arg.len() > 1 && arg.starts_with('-') {
            if let Some(attached) = code_option(&options, arg) {
                let after = if attached { i + 1 } else { i + 2 };
                if parsing == OptionParsing::StopsAtCodeOrScript {
                    return Some(after.min(args.len()));
                }
                i = after;
                continue;
            }
            if values.contains(&arg.as_str()) {
                // `python -m module`: what follows is the module's argv.
                if is_python(cmd) && arg == "-m" {
                    return Some((i + 2).min(args.len()));
                }
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        match parsing {
            OptionParsing::StopsAtCodeOrScript => return Some(i + 1),
            OptionParsing::StopsAtDashDashOrScript(extensions)
                if extensions
                    .iter()
                    .any(|ext| arg.to_ascii_lowercase().ends_with(ext)) =>
            {
                return Some(i + 1)
            }
            _ => i += 1,
        }
    }
    Some(args.len())
}

/// Whether `cmd args` reads its program text from stdin, so `exec_stdin`
/// would be code: a shell or interpreter with neither inline code nor a
/// script operand (or with the script `-`), a SQL client without a query
/// option, `make -f -`, a remote shell without a command, and so on.
/// Wrappers are followed to the program they launch.
pub fn reads_program_from_stdin(cmd: &str, args: &[String]) -> bool {
    let name = base_name(cmd);
    // xargs and parallel turn stdin into argv of the program they launch.
    if matches!(name.as_str(), "xargs" | "parallel") {
        return stdin_becomes_code_argv(&name, args);
    }
    if let Some((inner, inner_args)) = crate::core::argv_roles::launched_program(cmd, args) {
        return reads_program_from_stdin(&inner, &inner_args);
    }
    if let Some((inner, inner_args)) = stdin_forwarded_to(&name, cmd, args) {
        return reads_program_from_stdin(&inner, &inner_args);
    }
    let has = |options: &[&str]| {
        args.iter().any(|arg| {
            options
                .iter()
                .any(|option| arg == option || arg.starts_with(&format!("{option}=")))
        })
    };
    let operands: Vec<&String> = args
        .iter()
        .filter(|arg| !arg.starts_with('-') || *arg == "-")
        .collect();
    // The file named by one of `options` (`-f FILE`, `-fFILE`, `--file=FILE`).
    let file_option = |options: &[&str]| -> Option<&str> {
        args.iter().enumerate().find_map(|(i, arg)| {
            options.iter().find_map(|option| {
                if arg == option {
                    args.get(i + 1).map(String::as_str)
                } else if option.starts_with("--") {
                    arg.strip_prefix(&format!("{option}="))
                } else {
                    arg.strip_prefix(option).filter(|rest| !rest.is_empty())
                }
            })
        })
    };
    let stdin = |path: &str| crate::core::argv_roles::is_stdin_path(path);
    if let Some(parsing) = option_parsing(cmd) {
        if parsing == OptionParsing::Never {
            // PowerShell reads stdin unless `-File <script>` names a file.
            let names_code = args.iter().any(|arg| {
                let lower = arg.to_ascii_lowercase();
                matches!(lower.as_str(), "-c" | "-e" | "-ec")
                    || lower.starts_with("-com")
                    || lower.starts_with("-enc")
            });
            return !names_code
                && !args.windows(2).any(|pair| {
                    matches!(pair[0].to_ascii_lowercase().as_str(), "-file" | "-f")
                        && pair[1] != "-"
                });
        }
        let start = interpreter_data_start(cmd, args, &[]).unwrap_or(args.len());
        if inline_code_args(cmd, args)
            .iter()
            .any(|code| code.index < start)
        {
            return false;
        }
        // `sh -s`: commands come from stdin whatever operands follow.
        let shell_reads_stdin = is_shell(cmd)
            && args
                .iter()
                .take_while(|arg| *arg != "--")
                .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('s'));
        // `python -i` reads stdin as code after the script or the inline code.
        let interactive = is_python(cmd)
            && args
                .iter()
                .take_while(|arg| !arg.is_empty() && arg.starts_with('-') && *arg != "--")
                .any(|arg| {
                    !arg.starts_with("--")
                        && arg[1..]
                            .chars()
                            .take_while(|c| !matches!(c, 'c' | 'm' | 'W' | 'X'))
                            .any(|c| c == 'i')
                });
        return shell_reads_stdin
            || interactive
            || interpreter_script(cmd, args).is_none_or(crate::core::argv_roles::is_stdin_path);
    }
    match name.as_str() {
        "lua" | "luajit" | "tclsh" | "wish" | "osascript" | "rscript" => {
            !has(&["-e"]) && operands.first().is_none_or(|first| stdin(first))
        }
        "r" => !has(&["-e", "-f", "--file"]),
        "sqlite" => args.iter().filter(|arg| !arg.starts_with('-')).count() <= 1 && !has(&["-cmd"]),
        "mysql" | "mariadb" => !args.iter().any(|arg| {
            arg == "--execute"
                || arg.starts_with("--execute=")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('e'))
        }),
        "psql" => !has(&["-c", "--command"]) && file_option(&["-f", "--file"]).is_none_or(stdin),
        "make" | "gmake" | "bmake" => {
            file_option(&["-f", "--file", "--makefile"]).is_some_and(stdin)
        }
        "awk" | "gawk" | "mawk" | "nawk" | "sed" | "gsed" => {
            file_option(&["-f", "--file"]).is_some_and(stdin)
        }
        "gdb" | "ssh" => true,
        "docker" | "podman" => args.iter().any(|arg| {
            arg == "-i"
                || arg.starts_with("--interactive")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'))
        }),
        _ => crate::core::argv_roles::is_unmodelled_evaluator(&name),
    }
}

/// The program a launcher hands its stdin to: `npx`/`bunx`/`uvx`,
/// `npm|pnpm|yarn exec|dlx`, `runuser`, `kubectl|oc exec|run -i`,
/// `docker|podman compose run|exec` (stdin attached by default).
fn stdin_forwarded_to(name: &str, cmd: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let interactive = || {
        args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-i" | "--stdin" | "--stdin=true" | "--interactive"
            ) || arg.starts_with("--interactive=t")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('i'))
        })
    };
    let forwards = match name {
        "npx" | "bunx" | "uvx" | "npm" | "pnpm" | "yarn" | "runuser" => true,
        "kubectl" | "oc" => interactive(),
        "docker" | "podman" => args.iter().any(|arg| arg == "compose"),
        _ => false,
    };
    if !forwards {
        return None;
    }
    let roles = crate::core::argv_roles::model_roles(cmd, args, &vec![false; args.len()]);
    let at = roles
        .iter()
        .position(|role| *role == crate::core::argv_roles::Role::Executable)?;
    Some((args[at].clone(), args[at + 1..].to_vec()))
}

/// Whether the stdin words xargs or parallel append to the program they
/// launch land on its code, an option or its program name (`xargs sh -c`,
/// `xargs -I{} sh -c {}`, `parallel` with no command).
fn stdin_becomes_code_argv(name: &str, args: &[String]) -> bool {
    use crate::core::argv_roles::Role;
    let roles = crate::core::argv_roles::model_roles(name, args, &vec![false; args.len()]);
    let Some(at) = roles.iter().position(|role| *role == Role::Executable) else {
        // No program: parallel runs each line as a command, xargs runs echo.
        return name == "parallel";
    };
    let replace = args[..at]
        .iter()
        .enumerate()
        .find_map(|(i, arg)| match arg.as_str() {
            "-I" | "--replace" => args.get(i + 1).cloned(),
            "-i" => Some("{}".to_string()),
            other => other
                .strip_prefix("-I")
                .or_else(|| other.strip_prefix("--replace="))
                .filter(|rest| !rest.is_empty())
                .map(str::to_string),
        });
    let inner = &args[at];
    let mut inner_args: Vec<String> = args[at + 1..].to_vec();
    let mut tainted: Vec<bool> = inner_args
        .iter()
        .map(|arg| replace.as_deref().is_some_and(|r| arg.contains(r)))
        .collect();
    if replace.is_none() {
        inner_args.push("{{stdin}}".to_string());
        tainted.push(true);
    }
    let inner_roles = crate::core::argv_roles::model_roles(inner, &inner_args, &tainted);
    let lands_on_code = (0..inner_args.len()).any(|i| {
        tainted[i] && matches!(inner_roles[i], Role::Code | Role::Option | Role::Executable)
    });
    lands_on_code || reads_program_from_stdin(inner, &args[at + 1..])
}

/// The script operand of an interpreter (file, `-` for stdin, or the
/// module of `python -m`), if any.
fn interpreter_script<'a>(cmd: &str, args: &'a [String]) -> Option<&'a str> {
    let parsing = option_parsing(cmd)?;
    let options = inline_code_options(cmd)?;
    let values = value_options(cmd);
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return args.get(i + 1).map(String::as_str);
        }
        if arg.len() > 1 && arg.starts_with('-') {
            if code_option(&options, arg).is_some() {
                return None;
            }
            if values.contains(&arg) {
                if is_python(cmd) && arg == "-m" {
                    return args.get(i + 1).map(String::as_str);
                }
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        match parsing {
            OptionParsing::StopsAtDashDashOrScript(extensions)
                if arg != "-"
                    && !extensions
                        .iter()
                        .any(|ext| arg.to_ascii_lowercase().ends_with(ext)) =>
            {
                i += 1;
            }
            _ => return Some(arg),
        }
    }
    None
}

/// Roles of an interpreter's arguments: program text (the inline code and
/// its option), option positions until the data start, data after.
pub(crate) fn interpreter_roles(
    cmd: &str,
    args: &[String],
    tainted: &[bool],
) -> Option<Vec<crate::core::argv_roles::Role>> {
    use crate::core::argv_roles::Role;
    let start = interpreter_data_start(cmd, args, tainted)?;
    let mut roles = vec![Role::Option; args.len()];
    roles[start..].fill(Role::Data);
    for code in inline_code_args(cmd, args) {
        if code.index < start {
            roles[code.index] = Role::Code;
        }
    }
    if is_shell(cmd) {
        if let Some(Some(script)) = shell_script(args, tainted) {
            roles[script] = Role::Code;
        }
    }
    Some(roles)
}

/// The first argument that carries an outside value (`tainted`) while `cmd`
/// may still read it as an option, as code or as a program to run.
pub fn first_tainted_option_position(
    cmd: &str,
    args: &[String],
    tainted: &[bool],
) -> Option<usize> {
    first_tainted_position_with(cmd, args, tainted, &Trust::default())
}

/// [`first_tainted_option_position`]; with `approved`, a value given to an
/// unmodelled program is accepted.
fn first_tainted_position_with(
    cmd: &str,
    args: &[String],
    tainted: &[bool],
    trust: &Trust,
) -> Option<usize> {
    use crate::core::argv_roles::Role;
    let roles = crate::core::argv_roles::roles_with(cmd, args, tainted, trust);
    (0..args.len()).find(|&i| {
        tainted.get(i).copied().unwrap_or(false)
            && !matches!(roles[i], Role::Data | Role::RuntimeOption)
            && !(trust.approved && roles[i] == Role::Unmodelled)
    })
}

/// Which template arguments carry a value from outside Kronn.
fn tainted_templates(args: &[String]) -> Vec<bool> {
    args.iter()
        .map(
            |arg| match crate::workflows::template::placeholder_paths(arg) {
                Ok(paths) => paths.iter().any(|path| !is_trusted_template_path(path)),
                Err(_) => true,
            },
        )
        .collect()
}

/// Run-time check on the rendered argv, with the provenance of each field:
/// a field rendered from an outside value must sit in a data position, both
/// in the saved structure and in the structure the program will really
/// parse, and an operand the program reads as an option when it starts with
/// `-` (git, find) must not start with `-`.
pub fn rendered_refusal(
    step: &str,
    cmd: &str,
    templates: &[String],
    rendered: &[String],
    trust: impl Into<Trust>,
) -> Option<String> {
    use crate::core::argv_roles::Role;
    let trust = trust.into();
    let tainted = tainted_templates(templates);
    let saved = crate::core::argv_roles::model_roles(cmd, templates, &tainted);
    let option_like = (0..rendered.len()).find(|&i| {
        tainted.get(i).copied().unwrap_or(false)
            && saved.get(i) == Some(&Role::RuntimeOption)
            && !templates[i].starts_with('-')
            && (rendered[i].starts_with('-')
                // make reads `NAME=value` operands as assignments, which
                // can expand `$(shell …)`.
                || (matches!(
                    crate::core::argv_roles::normalize_command(cmd).as_str(),
                    "make" | "gmake" | "bmake"
                ) && rendered[i].contains('=')))
    });
    // `-u{{user}}` rendered with an empty value leaves `-u` alone, which then
    // takes the next argument as its value.
    let emptied = (0..rendered.len()).find(|&i| {
        tainted.get(i).copied().unwrap_or(false)
            && saved.get(i) == Some(&Role::Data)
            && !templates[i].starts_with("--")
            && templates[i].split("{{").next().is_some_and(|prefix| {
                prefix.len() == 2 && prefix.starts_with('-') && rendered[i] == prefix
            })
    });
    // The structure the program parses: rendered text, except that an
    // option the author wrote (`-u{{user}}`) keeps its template, so the
    // value cannot be read as more option letters.
    let parsed: Vec<String> = rendered
        .iter()
        .zip(templates)
        .zip(&tainted)
        .map(|((rendered, template), tainted)| {
            if *tainted && template.starts_with('-') {
                template.clone()
            } else {
                rendered.clone()
            }
        })
        .collect();
    let index = first_tainted_position_with(cmd, &parsed, &tainted, &trust)
        .or_else(|| first_tainted_position_with(cmd, templates, &tainted, &trust))
        .or(option_like)
        .or(emptied)?;
    Some(format!(
        "Exec step `{step}` refusé avant exécution : l'argument #{index} de `{cmd}` vient d'une \
         valeur extérieure et tombe là où `{cmd}` lit encore des options ou du code. Ouvre le \
         workflow et applique la correction proposée, ou fais approuver l'étape par un humain \
         dans l'éditeur."
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
    let start = interpreter_data_start(cmd, args, &tainted_templates(args)).unwrap_or(0);
    let codes: Vec<InlineCode> = inline_code_args(cmd, args)
        .into_iter()
        .filter(|code| code.index < start)
        .collect();
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
        // Only `-c SCRIPT` is trusted: shell flags move into the script.
        let flags = shell_set_flags(&args[..code.index]).ok_or_else(|| {
            "correction manuelle requise : une option du shell n'a pas d'équivalent `set`"
                .to_string()
        })?;
        let script = if flags.is_empty() {
            script.clone()
        } else {
            format!("set {flags}\n{script}")
        };
        rewrite_shell(&["-c".to_string()], &script, rest)
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

/// The shell options before the script (`-ec`, `-o pipefail -c`) as the
/// `set` arguments that give the same behaviour from inside the script, or
/// `None` when one has no such equivalent.
fn shell_set_flags(head: &[String]) -> Option<String> {
    const SET_LETTERS: &str = "aefnuvxBC";
    let mut letters = String::new();
    let mut names = Vec::new();
    let mut i = 0;
    while i < head.len() {
        let arg = head[i].as_str();
        if arg == "-o" {
            let name = head.get(i + 1)?;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            names.push(format!("-o {name}"));
            i += 2;
            continue;
        }
        let cluster = arg
            .strip_prefix('-')
            .filter(|rest| !rest.starts_with('-'))?;
        for c in cluster.chars() {
            if c == 'c' {
                continue;
            }
            if !SET_LETTERS.contains(c) {
                return None;
            }
            letters.push(c);
        }
        i += 1;
    }
    let mut parts = Vec::new();
    if !letters.is_empty() {
        parts.push(format!("-{letters}"));
    }
    parts.extend(names);
    Some(parts.join(" "))
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
            // Unquoted, the original word-splits and globs the value (an
            // empty value vanishes, spaces collapse): `"$1"` would not be
            // equivalent. A `|sh` value was already one quoted word.
            ShellContext::Plain if placeholder.shell_filter => false,
            ShellContext::Plain => {
                return manual("une valeur hors guillemets (le shell la découpe en mots)")
            }
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
    // Python: argv[0] is `-c` and everything after the code is data, so new
    // values follow the existing ones. Node keeps parsing options after `-e`
    // until `--` (which it drops from process.argv): the values go after a
    // `--`, the first one at process.argv[1].
    let (first_index, mut tail) = match language {
        Language::Python => (rest.len() + 1, rest.to_vec()),
        Language::Node if rest.is_empty() => (1, vec!["--".to_string()]),
        Language::Node if rest[0] == "--" => (rest.len(), rest.to_vec()),
        Language::Node => return manual("des arguments existants suivent le code sans `--`"),
    };
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

    /// Values that an interpreter still parsing options would obey.
    const OPTION_LIKE: [&str; 5] = [
        "--eval=require('fs').writeFileSync('pwned','x')",
        "-e",
        "--",
        "-cimport os; os.system('touch pwned')",
        "--rcfile=/dev/null",
    ];

    const HOSTILE: &str = "a'b\"c $(touch pwned) ; touch pwned2 `touch pwned3` \\ é🦀";

    #[test]
    fn shell_values_move_to_positional_arguments_in_order() {
        assert_eq!(
            suggest_args("bash", &args(&["-c", "echo \"{{x}}\""])).unwrap(),
            args(&["-c", "echo \"$1\"", "_", "{{x}}"])
        );
        // Unquoted, the original word-splits the value: not equivalent.
        assert!(suggest_args("bash", &args(&["-c", "echo {{x}}"]))
            .unwrap_err()
            .contains("hors guillemets"));
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
                    "printf '%s %s' \"a={{a}}\" {{b|sh}} \"{{a}}\" {{run.id}}"
                ])
            )
            .unwrap(),
            args(&[
                "-c",
                "set -e\nprintf '%s %s' \"a=$1\" \"$2\" \"$1\" {{run.id}}",
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
                &args(&["-c", "echo \"$1\" \"{{x ?? 'none'}}\"", "me", "one"])
            )
            .unwrap(),
            args(&["-c", "echo \"$1\" \"$2\"", "me", "one", "{{x ?? 'none'}}"])
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
            args(&["-e", "console.log(process.argv[1])", "--", "{{x}}"])
        );
    }

    #[test]
    fn inline_quick_exec_sources_of_a_collect_step_are_classified() {
        let source = |alias: &str, args: &[&str]| crate::models::CollectApiDataSource {
            alias: alias.into(),
            quick_api_id: String::new(),
            quick_exec_id: String::new(),
            quick_exec: Some(crate::models::CollectQuickExecSource {
                command: "python3".into(),
                args: args.iter().map(|arg| arg.to_string()).collect(),
                timeout_secs: None,
                output_format: Default::default(),
            }),
            required: true,
            variables: Default::default(),
        };
        let step = crate::models::WorkflowStep {
            name: "collect".into(),
            step_type: crate::models::StepType::CollectApiData,
            collect_api_data: Some(crate::models::CollectApiDataConfig {
                sources: vec![
                    source("safe", &["-c", "import sys; print(sys.argv[1])", "{{x}}"]),
                    source("ticket", &["-c", "print('{{issue.title}}')"]),
                ],
                concurrent_limit: None,
            }),
            ..Default::default()
        };
        let found = classify_step(&step, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].phase, "source");
        assert_eq!(found[0].source_alias.as_deref(), Some("ticket"));
        assert_eq!(
            found[0].suggested_args,
            Some(args(&[
                "-c",
                "import sys\nprint(sys.argv[1])",
                "{{issue.title}}"
            ]))
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
        let output = std::process::Command::new(cmd)
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
            ("bash", vec!["-c", "echo start \"{{x}}\" end"]),
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

            for hostile in OPTION_LIKE.iter().copied().chain([HOSTILE]) {
                let printed = run(cmd, &migrated, hostile, dir.path()).unwrap();
                assert!(printed.contains(hostile), "{cmd} {hostile}: {printed}");
                assert_eq!(
                    std::fs::read_dir(dir.path()).unwrap().count(),
                    0,
                    "{cmd}: the hostile value {hostile} ran"
                );
            }
            assert!(
                first_unsafe_placeholder(cmd, &migrated).is_none(),
                "{migrated:?}"
            );
        }
    }

    fn tainted_position(cmd: &str, line: &[&str]) -> Option<usize> {
        let line = args(line);
        first_tainted_option_position(cmd, &line, &tainted_templates(&line))
    }

    #[test]
    fn a_templated_argument_where_options_are_still_read_is_refused() {
        for (cmd, line) in [
            ("python3", vec!["{{mode}}", "{{issue.title}}"]),
            ("bash", vec!["-cx", "{{x}}"]),
            ("sh", vec!["-vc", "{{x}}"]),
            ("bash", vec!["-ec", "{{x}}", "_"]),
            ("bash", vec!["{{mode}}", "echo hi"]),
            ("bash", vec!["-o", "{{opt}}", "-c", "echo hi"]),
            ("python3", vec!["-W", "{{x}}", "script.py"]),
            ("python3", vec!["-m", "{{module}}"]),
            ("python3", vec!["--", "{{script}}"]),
            ("node", vec!["-e", "console.log(1)", "{{x}}"]),
            ("node", vec!["-r", "{{x}}", "app.js"]),
            ("node", vec!["run", "{{x}}"]),
            ("perl", vec!["-e", "print 1", "{{x}}"]),
            ("ruby", vec!["-e", "puts 1", "x", "{{x}}"]),
            ("php", vec!["script.php", "{{x}}"]),
            ("pwsh", vec!["-File", "s.ps1", "{{x}}"]),
        ] {
            assert!(tainted_position(cmd, &line).is_some(), "{cmd} {line:?}");
            let finding = first_unsafe_placeholder(cmd, &args(&line));
            assert!(finding.is_some(), "{cmd} {line:?}");
        }
    }

    /// Only five exact shapes, and the data-only programs, take a value
    /// without a human; any other option, a templated script or code, or a
    /// stdin script makes the line need an approval (or refuses it).
    #[test]
    fn only_the_trusted_shapes_take_a_value_without_a_human() {
        for (cmd, line) in [
            ("bash", vec!["-c", "echo \"$1\"", "_", "{{x}}"]),
            (
                "sh",
                vec!["-c", "printf %s \"$1\"", "name", "{{x}}", "{{y}}"],
            ),
            (
                "python3",
                vec!["-c", "import sys; print(sys.argv[1])", "{{x}}"],
            ),
            (
                "node",
                vec!["-e", "console.log(process.argv[1])", "--", "{{x}}"],
            ),
            ("echo", vec!["{{x}}"]),
            ("seq", vec!["--", "{{x}}"]),
        ] {
            assert_eq!(
                first_unsafe_placeholder(cmd, &args(&line)),
                None,
                "{cmd} {line:?}"
            );
        }
        for (cmd, line) in [
            (
                "bash",
                vec!["-o", "pipefail", "-c", "echo \"$1\"", "_", "{{x}}"],
            ),
            ("bash", vec!["-ec", "echo \"$1\"", "_", "{{x}}"]),
            ("bash", vec!["./run.sh", "{{x}}"]),
            ("python3", vec!["-X", "utf8", "tool.py", "{{x}}"]),
            ("python3", vec!["-m", "http.server", "{{port}}"]),
            ("python", vec!["tool.py", "{{x}}"]),
            ("python3", vec!["tool.py", "--flag", "{{x}}"]),
            ("node", vec!["app.js", "{{x}}"]),
            ("grep", vec!["--", "{{x}}", "file"]),
            ("python3", vec!["/dev/stdin", "{{x}}"]),
            ("perl", vec!["-e", "print @ARGV", "--", "{{x}}"]),
            ("git", vec!["commit", "-m", "{{message}}"]),
            ("env", vec!["A=1", "python3", "tool.py", "{{x}}"]),
            ("sudo", vec!["rm", "--", "{{x}}"]),
            ("make", vec!["{{target}}"]),
        ] {
            let line = args(&line);
            assert!(
                matches!(
                    first_unsafe_placeholder(cmd, &line),
                    Some(InlineFinding::UnmodelledProgram(..))
                ),
                "{cmd} {line:?}"
            );
            assert_eq!(
                first_unsafe_placeholder_with(cmd, &line, true),
                None,
                "{cmd} {line:?}"
            );
        }
        // Code, an option position or a program position stays refused,
        // approval or not.
        for (cmd, line) in [
            ("bash", vec!["-c", "echo \"{{x}}\""]),
            ("python3", vec!["{{script}}", "a"]),
            ("node", vec!["-e", "console.log('{{x}}')", "--"]),
            ("node", vec!["-e", "console.log(1)", "x", "{{x}}"]),
            ("env", vec!["{{cmd}}"]),
            ("git", vec!["-c", "core.sshCommand={{x}}", "fetch"]),
            ("date", vec!["-{{x}}"]),
        ] {
            let line = args(&line);
            assert!(
                first_unsafe_placeholder_with(cmd, &line, true).is_some(),
                "{cmd} {line:?}"
            );
        }
    }

    /// Review 4: obscure options of programs the classifier models are no
    /// longer trusted; each line needs a human or is refused outright.
    #[test]
    fn review_four_lines_need_a_human_or_are_refused() {
        for (cmd, line) in [
            (
                "python3",
                vec!["--check-hash-based-pycs", "always", "-c", "print('{{x}}')"],
            ),
            ("python3", vec!["-i", "tool.py", "{{x}}"]),
            ("fish", vec!["-C", "{{x}}", "-c", "true"]),
            ("nu", vec!["--config", "{{x}}", "-c", "ls"]),
            ("psql", vec!["-o", "{{v}}", "-c", "select 1"]),
            ("psql", vec!["--output={{v}}", "-c", "select 1"]),
            ("sftp", vec!["-s", "{{v}}", "host"]),
            ("rsync", vec!["-a", "src", "backup:/srv/{{name}}"]),
            ("tar", vec!["-c", "-f", "o.tar", "-T{{v}}"]),
            ("make", vec!["-f", "{{v}}"]),
            ("make", vec!["-C", "{{v}}"]),
            ("make", vec!["-I", "{{v}}"]),
            ("git", vec!["-C", "{{v}}", "status"]),
            ("git", vec!["--git-dir={{v}}", "status"]),
            ("mysql", vec!["--defaults-file={{v}}", "db"]),
            ("gdb", vec!["-D", "{{v}}", "./prog"]),
            ("deno", vec!["run", "--seed", "1", "{{x}}", "main.ts"]),
            ("stat", vec!["-f{{v}}", "x"]),
            ("ls", vec!["-w{{v}}"]),
            ("npx", vec!["tool", "{{x}}"]),
            ("xargs", vec!["git", "{{x}}"]),
        ] {
            assert!(
                first_unsafe_placeholder(cmd, &args(&line)).is_some(),
                "{cmd} {line:?}"
            );
        }
        // stdin: device paths, `-f -`, `python -i` read it as code.
        for (cmd, line) in [
            ("bash", vec!["/dev/stdin"]),
            ("bash", vec!["/dev/fd/0"]),
            ("python3", vec!["/proc/self/fd/0"]),
            ("python3", vec!["-i", "tool.py"]),
            ("psql", vec!["-f", "-"]),
            ("awk", vec!["-f", "/dev/stdin"]),
            ("sed", vec!["-f-"]),
            ("make", vec!["-f", "/dev/stdin"]),
        ] {
            assert!(
                stdin_validation_error("s", cmd, &args(&line), "{{x}}", true).is_some(),
                "{cmd} {line:?}"
            );
        }
        // Launchers and other programs pass stdin on: a human must approve.
        for (cmd, line) in [
            ("npx", vec!["node"]),
            ("xargs", vec!["git"]),
            ("tar", vec!["-c", "-f", "o.tar", "-T", "-"]),
            ("docker", vec!["compose", "run", "svc"]),
            ("kubectl", vec!["exec", "-i", "pod", "--", "sh"]),
            ("bash", vec!["-ec", "cat > /tmp/in.json"]),
        ] {
            let line = args(&line);
            assert!(
                stdin_validation_error("s", cmd, &line, "{{x}}", false).is_some(),
                "{cmd} {line:?}"
            );
        }
        assert!(stdin_validation_error("s", "npx", &args(&["prettier"]), "{{x}}", true).is_none());
    }

    /// A templated stdin is data only for a data-only program or a trusted
    /// shape (bash/sh `-c`, python3, node given their code or a script).
    #[test]
    fn a_templated_stdin_is_trusted_only_by_data_programs_and_trusted_shapes() {
        let pinned = |script: &str| Trust {
            declared_scripts: vec![script.to_string()],
            ..Trust::default()
        };
        for (cmd, line, trust) in [
            ("echo", vec![], Trust::default()),
            (
                "python3",
                vec!["-c", "import sys; print(sys.stdin.read())"],
                Trust::default(),
            ),
            ("python3", vec!["tool.py"], pinned("tool.py")),
            (
                "node",
                vec!["-e", "process.stdin.pipe(process.stdout)"],
                Trust::default(),
            ),
            ("node", vec!["tool.js"], pinned("tool.js")),
        ] {
            assert!(
                stdin_validation_error("s", cmd, &args(&line), "{{x}}", trust).is_none(),
                "{cmd} {line:?}"
            );
        }
        // An unpinned script shape, or a former data reader, needs a human.
        for (cmd, line) in [
            ("python3", vec!["tool.py"]),
            ("jq", vec!["."]),
            ("cat", vec![]),
        ] {
            assert!(
                stdin_validation_error("s", cmd, &args(&line), "{{x}}", false).is_some(),
                "{cmd} {line:?}"
            );
        }
        // A shell given its script with `-c` reads stdin as data, like python3.
        for cmd in ["bash", "sh"] {
            assert!(
                stdin_validation_error("s", cmd, &args(&["-c", "cat > in.json"]), "{{x}}", false)
                    .is_none(),
                "{cmd}"
            );
        }
        // A shell reading its program from stdin stays refused when approved.
        assert!(stdin_validation_error("s", "bash", &[], "{{x}}", true).is_some());
        assert!(stdin_validation_error("s", "bash", &args(&["-s"]), "{{x}}", true).is_some());
        assert!(stdin_validation_error("s", "sh", &args(&["-s", "a"]), "{{x}}", true).is_some());
    }

    /// R5-01: a shell's `-c` is a flag; the script is the first operand
    /// after every option, so `bash -c -- {{x}}` puts the value in code.
    #[test]
    fn a_shell_script_is_the_first_operand_after_its_options() {
        for cmd in ["bash", "sh", "zsh", "dash"] {
            for line in [
                vec!["-c", "--", "{{x}}"],
                vec!["-c", "-x", "{{x}}"],
                vec!["-c", "+e", "{{x}}"],
            ] {
                let line = args(&line);
                for approved in [false, true] {
                    assert!(
                        matches!(
                            first_unsafe_placeholder_with(cmd, &line, approved),
                            Some(InlineFinding::Untrusted(_) | InlineFinding::OptionPosition(_))
                        ),
                        "{cmd} {line:?} approved={approved}"
                    );
                }
            }
            // A literal option first: not the exact shape, so a human approves.
            let line = args(&["-c", "--", "echo \"$1\"", "_", "{{x}}"]);
            assert!(first_unsafe_placeholder(cmd, &line).is_some(), "{cmd}");
        }
    }

    /// B: a line an agent wrote has no trusted shape; any value needs a
    /// human's approval, and code positions stay refused even then.
    #[test]
    fn an_agent_written_line_needs_a_human_for_any_value() {
        let agent = Trust {
            agent_written: true,
            ..Trust::default()
        };
        for (cmd, line) in [
            ("bash", vec!["-c", "terraform plan \"$1\"", "_", "{{x}}"]),
            (
                "python3",
                vec!["-c", "import sys; print(sys.argv[1])", "{{x}}"],
            ),
            ("echo", vec!["{{x}}"]),
        ] {
            let line = args(&line);
            assert_eq!(
                first_unsafe_placeholder(cmd, &line),
                None,
                "{cmd} by a human"
            );
            assert!(
                matches!(
                    first_unsafe_placeholder_with(cmd, &line, agent.clone()),
                    Some(InlineFinding::UnmodelledProgram(..))
                ),
                "{cmd} by an agent"
            );
            let approved = Trust {
                approved: true,
                ..agent.clone()
            };
            assert_eq!(
                first_unsafe_placeholder_with(cmd, &line, approved),
                None,
                "{cmd}"
            );
        }
        assert!(stdin_validation_error(
            "s",
            "bash",
            &args(&["-c", "psql"]),
            "{{x}}",
            agent.clone()
        )
        .is_some());
        assert!(
            stdin_validation_error("s", "bash", &args(&["-c", "psql"]), "{{x}}", false).is_none()
        );
    }

    /// R5-04: a stdin or device path in any spelling is not a script.
    #[test]
    fn stdin_path_aliases_are_never_a_trusted_script() {
        for path in [
            "//dev/stdin",
            "/dev/./stdin",
            "/dev/../dev/stdin",
            "//proc/self/fd/0",
            "/dev/fd/0",
        ] {
            assert!(crate::core::argv_roles::is_stdin_path(path), "{path}");
            for cmd in ["python3", "node", "python3.12"] {
                assert!(
                    stdin_validation_error("s", cmd, &args(&[path]), "{{x}}", true).is_some(),
                    "{cmd} {path}"
                );
            }
        }
        assert!(!crate::core::argv_roles::is_stdin_path(
            "tools/dev/stdin.py"
        ));
        assert_eq!(
            crate::core::argv_roles::normalize_path("a/./b/../c//d"),
            "a/c/d"
        );
    }

    /// R5-06/R5-07: programs reading or writing paths, or the environment,
    /// are no longer data-only: a value reaching them needs a human.
    #[test]
    fn path_and_environment_programs_need_a_human_for_a_value() {
        for (cmd, line) in [
            ("jq", vec!["-r", "{{x}}", "data.json"]),
            ("jq", vec!["-r", ".a", "{{file}}"]),
            ("printenv", vec!["{{name}}"]),
            ("cat", vec!["{{path}}"]),
            ("tee", vec!["{{path}}"]),
            ("cp", vec!["{{src}}", "{{dst}}"]),
            ("rm", vec!["--", "{{x}}"]),
            ("chmod", vec!["+x", "{{path}}"]),
            ("printf", vec!["{{format}}", "a"]),
            ("printf", vec!["--", "{{format}}"]),
            ("date", vec!["-r", "{{file}}"]),
            ("date", vec!["-f", "{{file}}"]),
        ] {
            let line = args(&line);
            assert!(
                matches!(
                    first_unsafe_placeholder(cmd, &line),
                    Some(InlineFinding::UnmodelledProgram(..))
                ),
                "{cmd} {line:?}"
            );
        }
        for (cmd, line) in [
            ("echo", vec!["{{x}}"]),
            ("printf", vec!["%s\\n", "{{x}}"]),
            ("test", vec!["{{a}}", "=", "{{b}}"]),
            ("basename", vec!["{{path}}"]),
            ("dirname", vec!["{{path}}"]),
            ("seq", vec!["1", "{{n}}"]),
            ("date", vec!["-d{{when}}"]),
            ("date", vec!["--date={{when}}"]),
        ] {
            assert_eq!(
                first_unsafe_placeholder(cmd, &args(&line)),
                None,
                "{cmd} {line:?}"
            );
        }
        // stdin to a former data reader needs a human too.
        assert!(stdin_validation_error("s", "jq", &args(&["."]), "{{x}}", false).is_some());
        assert!(stdin_validation_error("s", "jq", &args(&["."]), "{{x}}", true).is_none());
    }

    /// R5-08: "refused even when approved" holds through every launcher.
    #[test]
    fn a_code_reader_behind_a_launcher_stays_refused_when_approved() {
        for (cmd, line) in [
            ("npx", vec!["node"]),
            ("kubectl", vec!["exec", "-i", "pod", "--", "sh"]),
            ("docker", vec!["compose", "run", "svc", "sh"]),
            ("runuser", vec!["-u", "u", "--", "sh"]),
            ("xargs", vec!["sh", "-c"]),
            ("xargs", vec!["-I{}", "sh", "-c", "{}"]),
            ("parallel", vec![]),
        ] {
            assert!(
                stdin_validation_error("s", cmd, &args(&line), "{{x}}", true).is_some(),
                "{cmd} {line:?}"
            );
        }
        assert!(stdin_validation_error("s", "xargs", &args(&["echo"]), "{{x}}", true).is_none());
    }

    /// R5-10: a script shape is trusted only for a script the step pins.
    #[test]
    fn a_script_shape_needs_its_script_pinned() {
        let line = args(&["tools/report.py", "{{x}}"]);
        assert!(matches!(
            first_unsafe_placeholder("python3", &line),
            Some(InlineFinding::UnmodelledProgram(..))
        ));
        let pinned = Trust {
            declared_scripts: vec!["tools/report.py".into()],
            ..Trust::default()
        };
        assert_eq!(
            first_unsafe_placeholder_with("python3", &line, pinned.clone()),
            None
        );
        assert_eq!(
            first_unsafe_placeholder_with(
                "node",
                &args(&["./tools/report.js", "{{x}}"]),
                Trust {
                    declared_scripts: vec!["tools/report.js".into()],
                    ..Trust::default()
                }
            ),
            None
        );
        let other = Trust {
            declared_scripts: vec!["tools/other.py".into()],
            ..Trust::default()
        };
        assert!(first_unsafe_placeholder_with("python3", &line, other).is_some());
    }

    /// R5-12: the shape rules, independent of the program model.
    #[test]
    fn the_trusted_shape_rules_hold_on_their_own() {
        use crate::core::argv_roles::{trusted_shape, Shape, Untrusted};
        let (f, t) = (false, true);
        let shape =
            |cmd: &str, items: &[&str], tainted: &[bool]| trusted_shape(cmd, &args(items), tainted);
        assert_eq!(
            shape("node", &["-e", "c", "app.js", "{{x}}"], &[f, f, f, t]),
            Err(Untrusted::Shape)
        );
        assert_eq!(
            shape("node", &["-e", "c", "--", "{{x}}"], &[f, f, f, t]),
            Ok(Shape::InlineCode)
        );
        assert_eq!(
            shape("bash", &["-c", "{{x}}"], &[f, t]),
            Err(Untrusted::TemplatedCode)
        );
        assert_eq!(
            shape("python3", &["{{s}}", "a"], &[t, f]),
            Err(Untrusted::TemplatedCode)
        );
        assert_eq!(
            shape("python3", &["tool.py", "{{x}}"], &[f, t]),
            Ok(Shape::Script)
        );
        assert_eq!(
            shape("python3", &["//dev/stdin"], &[f]),
            Err(Untrusted::StdinScript)
        );
        assert_eq!(
            shape("python", &["-c", "c"], &[f, f]),
            Err(Untrusted::Program)
        );
        if cfg!(windows) {
            assert_eq!(
                shape("bash", &["-c", "c", "_", "{{x}}"], &[f, f, f, t]),
                Err(Untrusted::Program)
            );
        } else {
            assert_eq!(
                shape("bash", &["-c", "c", "_", "{{x}}"], &[f, f, f, t]),
                Ok(Shape::InlineCode)
            );
            assert_eq!(
                shape("bash", &["-c", "--", "c"], &[f, f, f]),
                Err(Untrusted::Shape)
            );
            assert_eq!(
                shape("sh", &["-c", "+x", "c"], &[f, f, f]),
                Err(Untrusted::Shape)
            );
        }
    }

    #[test]
    fn values_in_data_positions_are_accepted() {
        for (cmd, line) in [
            ("bash", vec!["-c", "echo \"$1\"", "_", "{{x}}"]),
            (
                "bash",
                vec!["-o", "pipefail", "-c", "echo \"$1\"", "_", "{{x}}"],
            ),
            ("bash", vec!["./run.sh", "{{x}}"]),
            (
                "python3",
                vec!["-c", "import sys; print(sys.argv[1])", "{{x}}"],
            ),
            ("python3", vec!["-X", "utf8", "tool.py", "--flag", "{{x}}"]),
            ("python3", vec!["-m", "http.server", "{{port}}"]),
            (
                "node",
                vec!["-e", "console.log(process.argv[1])", "--", "{{x}}"],
            ),
            ("node", vec!["app.js", "{{x}}"]),
            ("perl", vec!["-e", "print @ARGV", "--", "{{x}}"]),
            ("ruby", vec!["script.rb", "{{x}}"]),
            ("php", vec!["-r", "echo $argv[1];", "--", "{{x}}"]),
            ("make", vec!["{{x}}"]),
            ("node", vec!["-e", "console.log({{run.id}})"]),
        ] {
            // Data to the program's model; outside the trusted shapes, an
            // approval is still needed (see `only_the_trusted_shapes_…`).
            let line = args(&line);
            let tainted = tainted_templates(&line);
            assert_eq!(
                first_tainted_position_with(cmd, &line, &tainted, &Trust::from(true)),
                None,
                "{cmd} {line:?}"
            );
        }
    }

    #[test]
    fn rendered_fields_from_outside_values_must_stay_data() {
        // `{{mode}}` rendered to `-c`: the field is tainted, it must not
        // become an option even though its rendered text looks like one.
        let templates = args(&["{{mode}}", "{{issue.title}}"]);
        let rendered = args(&["-c", "print(1)"]);
        assert!(rendered_refusal("s", "python3", &templates, &rendered, false).is_some());
        // The positional recipe stays data whatever the value renders to.
        let templates = args(&["-e", "console.log(process.argv[1])", "--", "{{x}}"]);
        let rendered = args(&["-e", "console.log(process.argv[1])", "--", "--eval=1"]);
        assert!(rendered_refusal("s", "node", &templates, &rendered, false).is_none());
        // A tainted field rendered to `--` does not end option parsing.
        let templates = args(&["-e", "console.log(1)", "{{a}}", "{{b}}"]);
        let rendered = args(&["-e", "console.log(1)", "--", "--eval=1"]);
        assert!(rendered_refusal("s", "node", &templates, &rendered, false).is_some());
    }

    #[test]
    fn node_rewrites_end_options_with_a_double_dash() {
        assert_eq!(
            suggest_args("node", &args(&["-e", "console.log('{{x}}')", "--", "a"])).unwrap(),
            args(&["-e", "console.log(process.argv[2])", "--", "a", "{{x}}"])
        );
        let reason = suggest_args("node", &args(&["-e", "console.log('{{x}}')", "a"])).unwrap_err();
        assert!(
            reason.starts_with("correction manuelle requise"),
            "{reason}"
        );
    }

    /// Runs `cmd line` with the template values set; `None` when the
    /// interpreter is not installed.
    fn run_case(
        cmd: &str,
        line: &[String],
        values: &[(&str, &str)],
        dir: &std::path::Path,
    ) -> Option<(Option<i32>, String)> {
        let mut ctx = TemplateContext::new();
        for (name, value) in values {
            ctx.set(*name, *value);
        }
        let rendered: Vec<String> = line
            .iter()
            .map(|arg| ctx.render_strict(arg).unwrap())
            .collect();
        let output = std::process::Command::new(cmd)
            .args(&rendered)
            .current_dir(dir)
            .output()
            .ok()?;
        Some((
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }

    /// Differential proof of each auto-fix family: on a corpus of benign
    /// values (those the original line itself treats as plain text), the
    /// rewritten line prints the same thing and exits the same way. The
    /// shapes include the 13 fixes found on real workflows.
    #[cfg(unix)]
    #[test]
    fn suggested_fixes_are_equivalent_on_a_value_corpus() {
        // Shell double quotes: everything but `"`, `$`, backtick, backslash.
        let shell_double = [
            "simple",
            "two  words",
            "it's",
            "é🦀 unicode",
            "",
            "-n",
            "--version",
            "line1\nline2",
            " leading",
            "a*b",
            "bob",
        ];
        // A `|sh` value is one quoted word: quotes and `$` are benign too.
        let shell_filtered = [
            "simple",
            "say \"hi\"",
            "it's",
            "$HOME",
            "",
            "-n",
            "a  b",
            "é🦀",
        ];
        // Python single-quoted literal: everything but `'`, backslash, newline.
        let python_single = [
            "simple",
            "two  words",
            "say \"hi\"",
            "é🦀",
            "",
            "-n",
            "--version",
            " x",
        ];
        // JavaScript double-quoted literal: everything but `"`, backslash, newline.
        let node_double = [
            "simple",
            "two  words",
            "it's",
            "é🦀",
            "",
            "-x",
            "--eval=1",
            " x",
        ];
        let cases: Vec<(&str, Vec<&str>, &[&str])> = vec![
            // Real shapes: skip_note, isown, setup, prnum, skip_check.
            ("bash", vec!["-c", "n=\"{{x}}\"\nif [ -f \".kronn/skip-$n\" ]; then echo \"A $n\"; else echo \"B $n\"; fi"], &shell_double),
            ("bash", vec!["-c", "a=\"$(python3 -c 'print(\"bob\")')\"\nif [ \"$a\" = \"{{x}}\" ]; then printf true; else printf false; fi"], &shell_double),
            ("bash", vec!["-c", "set -e\nn=\"{{x}}\"\nwt=\".kronn/shadow-pr-$n\"\necho \"setup OK PR=$n wt=$wt\""], &shell_double),
            ("bash", vec!["-c", "printf %s \"{{x}}\""], &shell_double),
            ("bash", vec!["-c", "set -euo pipefail\nn=\"{{x}}\"\nh=\"{{y}}\"\nsb=.kronn/r\nif [ -f \"$sb/done-$n-$h\" ]; then echo ALREADY; exit 5; fi\necho \"NEW $n $h\""], &shell_double),
            ("bash", vec!["-ec", "printf '%s\\n' \"value: {{x}}\""], &shell_double),
            ("sh", vec!["-c", "echo start {{x|sh}} end"], &shell_filtered),
            ("python3", vec!["-c", "print('{{x}}')"], &python_single),
            ("python3", vec!["-c", "import json; print(json.dumps({'v': '{{x}}'}))"], &python_single),
            ("python3", vec!["-c", "x = '{{x}}'\nprint(len(x), x.upper())"], &python_single),
            ("node", vec!["-e", "console.log(\"{{x}}\")"], &node_double),
            ("node", vec!["-e", "const v = \"{{x}}\"; console.log(v.length, JSON.stringify(v))"], &node_double),
        ];
        for (cmd, original, corpus) in cases {
            let original = args(&original);
            let rewritten = suggest_args(cmd, &original)
                .unwrap_or_else(|why| panic!("{cmd} {original:?}: {why}"));
            for value in corpus {
                let dir = tempfile::tempdir().unwrap();
                let values = [("x", *value), ("y", "second")];
                let Some(before) = run_case(cmd, &original, &values, dir.path()) else {
                    eprintln!("{cmd} is not installed here; skipped");
                    break;
                };
                let after = run_case(cmd, &rewritten, &values, dir.path()).unwrap();
                assert_eq!(
                    before, after,
                    "{cmd} {value:?}\n{original:?}\n{rewritten:?}"
                );
            }
        }
    }

    /// Why an unquoted value gets no automatic fix: the original word-splits
    /// it, so `"$1"` prints something else for an empty or spaced value.
    #[cfg(unix)]
    #[test]
    fn an_unquoted_shell_value_diverges_and_stays_manual() {
        let original = args(&["-c", "echo start {{x}} end"]);
        let naive = args(&["-c", "echo start \"$1\" end", "_", "{{x}}"]);
        let dir = tempfile::tempdir().unwrap();
        for value in ["", "a  b"] {
            let values = [("x", value)];
            assert_ne!(
                run_case("bash", &original, &values, dir.path()),
                run_case("bash", &naive, &values, dir.path()),
                "{value:?}"
            );
        }
        assert!(suggest_args("bash", &original).is_err());
    }

    #[test]
    fn a_shell_never_glues_its_script_to_c() {
        // `bash -cx SCRIPT`: `x` is a flag, the script is the next operand.
        for line in [["-cx", "{{x}}"], ["-vc", "{{x}}"], ["-ce", "{{x}}"]] {
            let line = args(&line);
            assert!(
                matches!(
                    first_unsafe_placeholder("bash", &line),
                    Some(InlineFinding::Untrusted(_))
                ),
                "{line:?}"
            );
        }
        for line in [
            vec!["-cx", "echo \"$1\"", "_", "{{x}}"],
            vec!["-xc", "echo \"$1\"", "_", "{{x}}"],
        ] {
            // Data to bash, but not the exact `-c` shape: approval needed.
            assert!(
                matches!(
                    first_unsafe_placeholder("bash", &args(&line)),
                    Some(InlineFinding::UnmodelledProgram(..))
                ),
                "{line:?}"
            );
        }
        // Python does glue: `-cCODE` holds the code itself, so the value is
        // data, though not in the exact `-c CODE` shape.
        assert_eq!(
            first_unsafe_placeholder_with(
                "python3",
                &args(&["-cimport sys; print(sys.argv[1])", "{{x}}"]),
                true
            ),
            None
        );
    }

    #[test]
    fn programs_that_read_their_code_on_stdin_are_detected() {
        for (cmd, line) in [
            ("bash", vec![]),
            ("sh", vec!["-s", "--", "a"]),
            ("bash", vec!["-o", "pipefail"]),
            ("python3", vec!["-"]),
            ("python3", vec![]),
            ("node", vec![]),
            ("perl", vec!["-w"]),
            ("env", vec!["A=1", "bash"]),
            ("timeout", vec!["5", "python3"]),
            ("mysql", vec!["db"]),
            ("psql", vec!["db"]),
            ("sqlite3", vec!["db.sqlite"]),
            ("make", vec!["-f", "-"]),
            ("lua", vec![]),
            ("bc", vec![]),
            ("crontab", vec!["-"]),
            ("pwsh", vec!["-NoProfile"]),
        ] {
            assert!(
                reads_program_from_stdin(cmd, &args(&line)),
                "{cmd} {line:?}"
            );
        }
        for (cmd, line) in [
            ("bash", vec!["./run.sh"]),
            ("bash", vec!["-c", "cat"]),
            ("python3", vec!["-c", "import sys; print(sys.stdin.read())"]),
            ("python3", vec!["tool.py"]),
            ("python3", vec!["-m", "json.tool"]),
            ("node", vec!["app.js"]),
            ("cat", vec![]),
            ("jq", vec!["."]),
            ("mysql", vec!["-e", "select 1", "db"]),
            ("psql", vec!["-c", "select 1"]),
            ("sqlite3", vec!["db.sqlite", ".tables"]),
            ("make", vec!["build"]),
            ("pwsh", vec!["-File", "s.ps1"]),
        ] {
            assert!(
                !reads_program_from_stdin(cmd, &args(&line)),
                "{cmd} {line:?}"
            );
        }
    }

    /// A templated stdin reaching a program Kronn does not model is refused
    /// unless a human approved the step; data-only programs and modelled data
    /// readers keep it, wrappers are followed.
    #[test]
    fn a_templated_stdin_to_an_unmodelled_program_needs_a_human_approval() {
        let duckdb = args(&["db.duckdb"]);
        let error = stdin_validation_error("s", "duckdb", &duckdb, "{{issue.title}}", false);
        assert!(
            error.as_deref().is_some_and(|e| e.contains("duckdb")),
            "{error:?}"
        );
        assert!(stdin_validation_error("s", "duckdb", &duckdb, "{{issue.title}}", true).is_none());
        assert!(stdin_validation_error(
            "s",
            "env",
            &args(&["A=1", "duckdb", "db.duckdb"]),
            "{{issue.title}}",
            false
        )
        .is_some());
        assert!(stdin_validation_error("s", "duckdb", &duckdb, "select 1", false).is_none());
        for (cmd, line) in [
            ("echo", vec![]),
            ("test", vec!["-n", "x"]),
            ("seq", vec!["3"]),
        ] {
            assert!(
                stdin_validation_error("s", cmd, &args(&line), "{{issue.title}}", false).is_none(),
                "{cmd}"
            );
        }
        // An evaluator stays refused, approval or not.
        assert!(stdin_validation_error("s", "bash", &[], "{{issue.title}}", true).is_some());
        let step = crate::models::WorkflowStep {
            name: "load".into(),
            step_type: crate::models::StepType::Exec,
            exec_command: Some("duckdb".into()),
            exec_args: duckdb.clone(),
            exec_stdin: Some("{{issue.title}}".into()),
            ..Default::default()
        };
        assert!(runtime_refusal(&step).is_some());
        let approved = crate::models::WorkflowStep {
            exec_unmodelled_args_approved: Some(true),
            ..step
        };
        assert!(runtime_refusal(&approved).is_none());
    }

    #[test]
    fn a_templated_stdin_is_refused_only_where_it_is_code() {
        assert!(stdin_validation_error("s", "bash", &[], "{{issue.title}}", false).is_some());
        assert!(
            stdin_validation_error("s", "python3", &args(&["-"]), "x {{issue.title}}", false)
                .is_some()
        );
        assert!(stdin_validation_error("s", "bash", &[], "echo {{run.id}}", false).is_none());
        assert!(stdin_validation_error(
            "s",
            "python3",
            &args(&["-c", "import sys; print(sys.stdin.read())"]),
            "{{issue.title}}",
            false
        )
        .is_none());
        assert!(stdin_validation_error(
            "s",
            "jq",
            &args(&["."]),
            "{{steps.fetch.data_json}}",
            true
        )
        .is_none());
        let step = crate::models::WorkflowStep {
            name: "run".into(),
            step_type: crate::models::StepType::Exec,
            exec_command: Some("bash".into()),
            exec_stdin: Some("echo {{issue.title}}".into()),
            ..Default::default()
        };
        let found = classify_step(&step, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].phase, "stdin");
        assert!(runtime_refusal(&step).unwrap().contains("(stdin)"));
    }
}
