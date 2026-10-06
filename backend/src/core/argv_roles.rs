//! The role of each argument of an Exec command line: plain data, program
//! text, an option the program still parses, or the program to run.
//!
//! A template value is safe only as data, and Kronn trusts it as data
//! without a human only in a tiny, fully specified set: five exact
//! interpreter shapes ([`trusted_shape`]) and the data-only programs. Every
//! other value is [`Role::Unmodelled`]: refused unless a human approved the
//! line. The program models below (interpreters through `inline_code`,
//! wrappers, `awk`, `sed`, `find -exec`, `git -c`, `ssh`, `docker run`,
//! `npx`…) only decide what no approval lifts: a value in code, at an option
//! position or naming a program, and the run-time checks of rendered values.

/// The program that receives argument `index`: the nearest program position
/// before it (a wrapper's program, `find -exec`, `docker run`…), else `cmd`.
pub fn owning_program(cmd: &str, args: &[String], roles: &[Role], index: usize) -> String {
    (0..index)
        .rev()
        .find(|&j| roles.get(j) == Some(&Role::Executable))
        .map(|j| args[j].clone())
        .unwrap_or_else(|| cmd.to_string())
}

/// The program a wrapper launches and its arguments (`env A=1 bash -s` →
/// `bash`, `["-s"]`), following nested wrappers; `None` when `cmd` is not a
/// wrapper or names no program.
pub fn launched_program(cmd: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let mut current: Option<(String, Vec<String>)> = None;
    let (mut cmd, mut args) = (cmd.to_string(), args.to_vec());
    for _ in 0..MAX_DEPTH {
        let Some(spec) = wrapper(&normalize_command(&cmd)) else {
            break;
        };
        let roles = wrapper_roles(&spec, &args, &vec![false; args.len()], 0);
        let Some(index) = roles.iter().position(|role| *role == Role::Executable) else {
            break;
        };
        cmd = args[index].clone();
        args = args[index + 1..].to_vec();
        current = Some((cmd.clone(), args.clone()));
    }
    current
}

/// What an argument is to the program that receives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Plain data: a value here can never change what runs.
    Data,
    /// Program text (inline code, a sed or awk script, a remote command).
    Code,
    /// The program still parses options here: a value could become one.
    Option,
    /// The program to run.
    Executable,
    /// Data unless the rendered value starts with `-`, where the program
    /// would read it as an option (git and find operands): checked at run
    /// time on the rendered value.
    RuntimeOption,
    /// An argument of a program the classifier does not model: refused
    /// unless a human approved the step.
    Unmodelled,
}

/// The program name as the classifier compares it: base name of a path,
/// lower case, without `.exe` and without a version suffix
/// (`/usr/bin/python3.12` → `python`, `node18` → `node`, `Rscript` → `rscript`).
pub fn normalize_command(cmd: &str) -> String {
    let base = cmd
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    let stripped = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if stripped.is_empty() {
        base.to_string()
    } else {
        stripped.to_string()
    }
}

const MAX_DEPTH: usize = 8;

/// Who stands behind the values of a command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trust {
    /// A human approved the step: a value the model leaves as data is
    /// accepted on any line (never in code, an option or a program name).
    pub approved: bool,
    /// An agent wrote the line: no shape is trusted, every value needs the
    /// approval.
    pub agent_written: bool,
    /// Scripts the step pins by hash (`exec_script_files`): the only ones a
    /// script shape trusts.
    pub declared_scripts: Vec<String>,
}

impl From<bool> for Trust {
    fn from(approved: bool) -> Self {
        Self {
            approved,
            ..Self::default()
        }
    }
}

/// Why a command line is not one of the shapes Kronn trusts without a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untrusted {
    /// The program has no trusted shape.
    Program,
    /// An option outside the shape, or the shape's option missing.
    Shape,
    /// The inline code or the script path carries a template value.
    TemplatedCode,
    /// The script is stdin or a device (`-`, `/dev/stdin`, `//dev/./fd/0`…).
    StdinScript,
}

/// Which trusted shape a line has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `bash|sh -c`, `python3 -c`, `node -e`: the code is in the line.
    InlineCode,
    /// `python3 SCRIPT`, `node SCRIPT`: the code is in a file.
    Script,
}

/// The only lines whose values Kronn trusts without a human, matched exactly:
/// `bash|sh -c SCRIPT [NAME ARGS…]`, `python3 -c CODE [ARGS…]`,
/// `python3 SCRIPT [ARGS…]`, `node -e CODE [-- ARGS…]`, `node SCRIPT [ARGS…]`,
/// where `SCRIPT`/`CODE` carry no outside value. A shell script does not
/// start with `-` or `+`: bash reads it as one more option (`bash -c -- X`
/// runs `X`).
pub fn trusted_shape(cmd: &str, args: &[String], tainted: &[bool]) -> Result<Shape, Untrusted> {
    let literal = |i: usize| !tainted.get(i).copied().unwrap_or(false);
    let code_then = |option: &str| -> Result<Shape, Untrusted> {
        if args.first().map(String::as_str) != Some(option) || args.len() < 2 {
            return Err(Untrusted::Shape);
        }
        if !literal(1) {
            return Err(Untrusted::TemplatedCode);
        }
        Ok(Shape::InlineCode)
    };
    let script = || -> Result<Shape, Untrusted> {
        let Some(first) = args.first() else {
            return Err(Untrusted::StdinScript);
        };
        if !literal(0) {
            return Err(Untrusted::TemplatedCode);
        }
        if is_stdin_path(first) {
            return Err(Untrusted::StdinScript);
        }
        if first.starts_with('-') {
            return Err(Untrusted::Shape);
        }
        Ok(Shape::Script)
    };
    match trusted_program(cmd) {
        Some("bash" | "sh") => {
            code_then("-c")?;
            if args[1].starts_with(['-', '+']) {
                return Err(Untrusted::Shape);
            }
            Ok(Shape::InlineCode)
        }
        Some("python3") if args.first().is_some_and(|a| a == "-c") => code_then("-c"),
        Some("python3") => script(),
        Some("node") if args.first().is_some_and(|a| a == "-e") => {
            code_then("-e")?;
            if args.len() > 2 && args[2] != "--" {
                return Err(Untrusted::Shape);
            }
            Ok(Shape::InlineCode)
        }
        Some("node") => script(),
        _ => Err(Untrusted::Program),
    }
}

/// Whether `trust` lets the shape of `cmd args` carry values without a human:
/// a human wrote it, and a script shape runs a script the step pins.
pub fn trusted_by_shape(cmd: &str, args: &[String], tainted: &[bool], trust: &Trust) -> bool {
    if trust.agent_written {
        return false;
    }
    match trusted_shape(cmd, args, tainted) {
        Ok(Shape::InlineCode) => true,
        Ok(Shape::Script) => {
            let script = args[0].trim_start_matches("./");
            trust
                .declared_scripts
                .iter()
                .any(|declared| declared.trim_start_matches("./") == script)
        }
        Err(_) => false,
    }
}

/// The trusted program `cmd` names, by exact base name. On Windows `bash`
/// may be the WSL launcher, which hands its command line to another shell:
/// shells there are not trusted.
fn trusted_program(cmd: &str) -> Option<&'static str> {
    let base = cmd.trim().rsplit(['/', '\\']).next().unwrap_or_default();
    let names: &[&'static str] = if cfg!(windows) {
        &["python3", "node"]
    } else {
        &["bash", "sh", "python3", "node"]
    };
    names.iter().copied().find(|name| *name == base)
}

/// A path that names stdin or a device, after lexical normalisation
/// (`//dev/./stdin`, `/dev/../dev/fd/0`).
pub fn is_stdin_path(path: &str) -> bool {
    if path == "-" {
        return true;
    }
    let normal = normalize_path(path);
    normal == "/dev"
        || normal.starts_with("/dev/")
        || normal == "/proc"
        || normal.starts_with("/proc/")
}

/// `path` with `//`, `.` and `..` resolved lexically (relative paths stay
/// relative; a `..` above the root stays at the root).
pub fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// The role of every argument of `cmd args` for a line a human wrote, with
/// no pinned script. See [`roles_with`].
pub fn roles(cmd: &str, args: &[String], tainted: &[bool]) -> Vec<Role> {
    roles_with(cmd, args, tainted, &Trust::default())
}

/// The role of every argument of `cmd args`. `tainted` marks arguments that
/// carry an outside value: they never end option parsing or name a program,
/// since their rendered text is not known (or not trusted).
///
/// Only a trusted shape ([`trusted_by_shape`]) or a data-only program keeps
/// a value as data without a human, and only on a line a human wrote: on any
/// other line, a value the model leaves as data is [`Role::Unmodelled`]. The
/// model still decides the refusals no approval lifts (code, option and
/// program positions).
pub fn roles_with(cmd: &str, args: &[String], tainted: &[bool], trust: &Trust) -> Vec<Role> {
    let mut flags = tainted.to_vec();
    flags.resize(args.len(), false);
    let mut roles = model_roles(cmd, args, &flags);
    let trusted =
        !trust.agent_written && (is_data_only(cmd) || trusted_by_shape(cmd, args, &flags, trust));
    if flags.iter().any(|value| *value) && !trusted {
        for (role, value) in roles.iter_mut().zip(&flags) {
            if *value && matches!(role, Role::Data | Role::RuntimeOption) {
                *role = Role::Unmodelled;
            }
        }
    }
    roles
}

/// The roles the program model gives, before the trusted-shape rule: the
/// run-time checks still use it on an approved line (an approved `git fetch
/// {{remote}}` must not render to `--upload-pack=…`).
pub fn model_roles(cmd: &str, args: &[String], tainted: &[bool]) -> Vec<Role> {
    let mut flags = tainted.to_vec();
    flags.resize(args.len(), false);
    roles_at(cmd, args, &flags, 0)
}

fn roles_at(cmd: &str, args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    if depth > MAX_DEPTH {
        return vec![Role::Option; args.len()];
    }
    let name = normalize_command(cmd);
    if let Some(spec) = wrapper(&name) {
        return wrapper_roles(&spec, args, tainted, depth);
    }
    if name == "deno" {
        return deno_roles(args, tainted, depth);
    }
    if let Some(roles) = crate::core::inline_code::interpreter_roles(cmd, args, tainted) {
        return roles;
    }
    match name.as_str() {
        "awk" | "gawk" | "mawk" | "nawk" => awk_roles(args, tainted),
        "sed" | "gsed" => sed_roles(args, tainted),
        "osascript" => script_roles(args, tainted, &["-e"], &["-l", "-s"], Tail::AfterScript),
        "lua" | "luajit" => script_roles(args, tainted, &["-e"], &["-l"], Tail::AfterScript),
        "tclsh" | "wish" => script_roles(args, tainted, &[], &["-encoding"], Tail::AfterScript),
        "rscript" => script_roles(args, tainted, &["-e"], &[], Tail::AfterMarker("--args")),
        "r" => script_roles(args, tainted, &["-e"], &["-f"], Tail::AfterMarker("--args")),
        "find" | "gfind" => find_roles(args, tainted, depth),
        "git" => git_roles(args, tainted),
        "ssh" => ssh_roles(args, tainted),
        "docker" | "podman" => docker_roles(args, tainted, depth),
        "npx" | "bunx" | "uvx" => npx_roles(args, tainted, depth),
        "tar" | "gtar" | "bsdtar" => tar_roles(&name, args, tainted),
        "kubectl" | "oc" => kubectl_roles(args, tainted, depth),
        "runuser" => runuser_roles(args, tainted, depth),
        "parallel" => parallel_roles(args, tainted),
        "pip" | "pipx" | "uv" | "go" | "cargo" | "gem" => {
            fetcher_roles(&name, args, tainted, depth)
        }
        "npm" | "pnpm" | "yarn" => package_manager_roles(&name, args, tainted, depth),
        _ if UNMODELLED_EVALUATORS.contains(&name.as_str()) => vec![Role::Option; args.len()],
        _ => match flag_spec(&name) {
            Some(spec) => flag_roles(args, tainted, &spec),
            None if is_data_only(cmd) => data_only_roles(&exact_base(cmd), args, tainted),
            None => vec![Role::Unmodelled; args.len()],
        },
    }
}

/// Data-only programs still parse options: a value is checked at run time
/// until a literal `--` ends them. `echo`, `test` and `[` read no option a
/// value could turn on. `printf`'s format is a small language, so a value
/// there needs a human; so does a file `date` would read (`-f`, `-r`) or a
/// date it would set (`-s`).
fn data_only_roles(name: &str, args: &[String], tainted: &[bool]) -> Vec<Role> {
    if matches!(name, "echo" | "test" | "[" | "true" | "false") {
        return vec![Role::Data; args.len()];
    }
    let mut options_end = false;
    let mut operands = 0;
    let mut roles = vec![Role::Data; args.len()];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let value = is_tainted(tainted, i);
        let format = name == "printf" && operands == 0;
        if options_end || (!value && !is_option(arg)) {
            if value && format {
                roles[i] = Role::Unmodelled;
            }
            operands += usize::from(arg != "--" || options_end);
            i += 1;
            continue;
        }
        if value {
            roles[i] = if !is_option(arg) {
                operands += 1;
                if format {
                    Role::Unmodelled
                } else {
                    Role::RuntimeOption
                }
            } else if data_only_attached(name, arg) {
                Role::Data
            } else {
                Role::Option
            };
            i += 1;
            continue;
        }
        if arg == "--" {
            options_end = true;
        } else if name == "date"
            && matches!(arg, "-f" | "-r" | "-s" | "--file" | "--reference" | "--set")
            && i + 1 < args.len()
        {
            if is_tainted(tainted, i + 1) {
                roles[i + 1] = Role::Unmodelled;
            }
            i += 1;
        }
        i += 1;
    }
    roles
}

/// Whether a templated option of a data-only program keeps its value
/// attached to an option that takes a plain value (`seq -f{{fmt}}`,
/// `date --date={{when}}`), so the rendered text cannot add options.
fn data_only_attached(name: &str, arg: &str) -> bool {
    let (letters, long): (&[char], &[&str]) = match name {
        "basename" => (&['s'], &["--suffix"]),
        "seq" => (&['f', 's'], &["--format", "--separator"]),
        "date" => (&['d'], &["--date"]),
        _ => (&[], &[]),
    };
    let prefix = arg.split("{{").next().unwrap_or_default();
    if prefix.starts_with("--") {
        return prefix
            .split_once('=')
            .is_some_and(|(option, _)| long.contains(&option));
    }
    attached_value(arg, letters)
}

/// Whether a templated option argument keeps its value attached to an option
/// the author wrote: `--name={{v}}`, or `-X{{v}}` where `X` takes a value.
/// Otherwise the rendered text can add options (`-{{x}}`, `--{{x}}`, `-v{{x}}`).
fn attached_value(arg: &str, short_values: &[char]) -> bool {
    let prefix = arg.split("{{").next().unwrap_or_default();
    if let Some(name) = prefix.strip_prefix("--") {
        return name.find('=').is_some_and(|at| at > 0);
    }
    prefix
        .strip_prefix('-')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|letter| short_values.contains(&letter))
}

fn is_tainted(tainted: &[bool], i: usize) -> bool {
    tainted.get(i).copied().unwrap_or(false)
}

fn is_option(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-')
}

/// `--name=value` or `--name`.
fn long_name(arg: &str) -> &str {
    arg.split_once('=').map_or(arg, |(name, _)| name)
}

// ─── Wrappers ────────────────────────────────────────────────────────────────

/// A program that launches another one.
struct Wrapper {
    /// Options whose value is the next argument (or attached after `=`).
    value_options: &'static [&'static str],
    /// Options whose value is a whole command line (`env -S`).
    command_line_options: &'static [&'static str],
    /// Leading positional arguments before the command (`timeout DURATION`).
    leading_positionals: usize,
    /// `NAME=value` assignments may precede the command (`env`, `sudo`).
    assignments: bool,
    /// The remaining arguments are joined into a shell command line
    /// (`watch`), so they are code rather than a program and its argv.
    tail_is_code: bool,
}

fn wrapper(name: &str) -> Option<Wrapper> {
    let spec = |value_options, leading_positionals, assignments| Wrapper {
        value_options,
        command_line_options: &[],
        leading_positionals,
        assignments,
        tail_is_code: false,
    };
    Some(match name {
        "env" => Wrapper {
            value_options: &["-u", "--unset", "-C", "--chdir"],
            command_line_options: &["-S", "--split-string"],
            leading_positionals: 0,
            assignments: true,
            tail_is_code: false,
        },
        "watch" => Wrapper {
            value_options: &["-n", "--interval", "-q", "--equexit"],
            command_line_options: &[],
            leading_positionals: 0,
            assignments: false,
            tail_is_code: true,
        },
        "flock" => Wrapper {
            value_options: &["-w", "--timeout", "-E", "--conflict-exit-code"],
            command_line_options: &["-c", "--command"],
            leading_positionals: 1,
            assignments: false,
            tail_is_code: false,
        },
        "setsid" => spec(&[], 0, false),
        "chroot" => spec(&["--userspec", "--groups"], 1, false),
        "unshare" => spec(
            &[
                "--propagation",
                "--setgroups",
                "--map-user",
                "--map-group",
                "-S",
                "--setuid",
                "-G",
                "--setgid",
                "-R",
                "--root",
                "-w",
                "--wd",
            ],
            0,
            false,
        ),
        "nsenter" => spec(
            &[
                "-t", "--target", "-S", "--setuid", "-G", "--setgid", "-r", "--root", "-w", "--wd",
            ],
            0,
            false,
        ),
        "taskset" => spec(&[], 1, false),
        "ionice" => spec(
            &[
                "-c",
                "--class",
                "-n",
                "--classdata",
                "-p",
                "--pid",
                "-P",
                "--pgid",
                "-u",
                "--uid",
            ],
            0,
            false,
        ),
        "chrt" => spec(
            &[
                "-T",
                "--sched-runtime",
                "-P",
                "--sched-period",
                "-D",
                "--sched-deadline",
            ],
            1,
            false,
        ),
        "strace" => spec(
            &[
                "-o",
                "--output",
                "-e",
                "-p",
                "--attach",
                "-u",
                "--user",
                "-s",
                "--string-limit",
                "-a",
                "-b",
                "-I",
                "-E",
                "--env",
                "-O",
                "-P",
                "-X",
            ],
            0,
            false,
        ),
        "ltrace" => spec(
            &[
                "-o", "--output", "-e", "-p", "-u", "-s", "-a", "-n", "-x", "-L", "-F",
            ],
            0,
            false,
        ),
        "valgrind" => spec(&[], 0, false),
        "systemd-run" => spec(
            &[
                "-u",
                "--unit",
                "-p",
                "--property",
                "-E",
                "--setenv",
                "--uid",
                "--gid",
                "-M",
                "--machine",
                "--description",
                "--slice",
                "--on-calendar",
                "--on-active",
                "--working-directory",
                "-H",
                "--host",
            ],
            0,
            false,
        ),
        "nice" => spec(&["-n", "--adjustment"], 0, false),
        "nohup" | "command" | "busybox" => spec(&[], 0, false),
        "timeout" | "gtimeout" => spec(&["-s", "--signal", "-k", "--kill-after"], 1, false),
        "time" => spec(&["-f", "--format", "-o", "--output"], 0, false),
        "exec" => spec(&["-a"], 0, false),
        "stdbuf" => spec(
            &["-i", "--input", "-o", "--output", "-e", "--error"],
            0,
            false,
        ),
        "sudo" => spec(
            &[
                "-u",
                "--user",
                "-g",
                "--group",
                "-C",
                "--close-from",
                "-D",
                "--chdir",
                "-h",
                "--host",
                "-p",
                "--prompt",
                "-r",
                "--role",
                "-t",
                "--type",
                "-T",
                "--command-timeout",
                "-U",
                "--other-user",
            ],
            0,
            true,
        ),
        "doas" => spec(&["-u", "-C"], 0, false),
        "xargs" | "gxargs" => spec(
            &[
                "-I",
                "-L",
                "-n",
                "-P",
                "-s",
                "-d",
                "-E",
                "-a",
                "--arg-file",
                "--delimiter",
                "--max-args",
                "--max-lines",
                "--max-procs",
                "--max-chars",
                "--eof",
                "--process-slot-var",
            ],
            0,
            false,
        ),
        _ => return None,
    })
}

fn is_assignment(arg: &str) -> bool {
    arg.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// The wrapper's own options and values are option positions; the first
/// remaining argument is the program, and its arguments are classified as
/// if it were run directly.
fn wrapper_roles(spec: &Wrapper, args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut positionals = spec.leading_positionals;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if is_tainted(tainted, i) {
            if spec.assignments && is_assignment(arg) || positionals > 0 {
                positionals = positionals.saturating_sub(usize::from(!is_assignment(arg)));
                i += 1;
                continue;
            }
            // A value where the program name belongs: nothing after it can
            // be classified, and the program itself is not known.
            roles[i] = Role::Executable;
            return roles;
        }
        if arg == "--" {
            i += 1;
            break;
        }
        if is_option(arg) {
            match wrapper_option(spec, arg) {
                WrapperOption::CommandLine => {
                    // That command line and everything after it is code.
                    roles[i..].fill(Role::Code);
                    return roles;
                }
                WrapperOption::TakesNext => i += 2,
                WrapperOption::Flag => i += 1,
            }
            continue;
        }
        if spec.assignments && is_assignment(arg) {
            i += 1;
            continue;
        }
        if positionals > 0 {
            positionals -= 1;
            i += 1;
            continue;
        }
        break;
    }
    if spec.tail_is_code {
        roles[i.min(args.len())..].fill(Role::Code);
        return roles;
    }
    if i < args.len() {
        roles[i] = Role::Executable;
        let inner = roles_at(&args[i], &args[i + 1..], &tainted[i + 1..], depth + 1);
        roles[i + 1..].copy_from_slice(&inner);
    }
    roles
}

enum WrapperOption {
    /// The option's value is a shell command line (`env -S`, `flock -c`).
    CommandLine,
    /// The option takes the next argument as its value.
    TakesNext,
    /// A flag, or an option whose value is attached.
    Flag,
}

/// Whether `option` (exact, or an unambiguous long prefix getopt accepts,
/// e.g. `--spl` for `--split-string`) is in `list`.
fn option_in(option: &str, list: &[&str]) -> bool {
    list.contains(&option)
        || (option.starts_with("--")
            && option.len() > 3
            && list
                .iter()
                .any(|known| known.starts_with("--") && known.starts_with(option)))
}

/// How a wrapper reads one option argument, scanning a short cluster letter
/// by letter (`sudo -Eu root`, `env -iS 'cmd'`, `xargs -0I {}`).
fn wrapper_option(spec: &Wrapper, arg: &str) -> WrapperOption {
    if arg.starts_with("--") {
        let name = long_name(arg);
        if option_in(name, spec.command_line_options) {
            return WrapperOption::CommandLine;
        }
        if option_in(name, spec.value_options) && !arg.contains('=') {
            return WrapperOption::TakesNext;
        }
        return WrapperOption::Flag;
    }
    let cluster = arg.strip_prefix('-').unwrap_or_default();
    for (at, c) in cluster.char_indices() {
        let short = format!("-{c}");
        if spec.command_line_options.contains(&short.as_str()) {
            return WrapperOption::CommandLine;
        }
        if spec.value_options.contains(&short.as_str()) {
            // The rest of the cluster is the value, or the next argument is.
            return if at + c.len_utf8() == cluster.len() {
                WrapperOption::TakesNext
            } else {
                WrapperOption::Flag
            };
        }
    }
    WrapperOption::Flag
}

// ─── Evaluators ──────────────────────────────────────────────────────────────

/// awk: the program text is the first operand unless `-f`/`-e`/`-E` gave
/// it; `-v NAME=value` with a literal name and later operands are data.
fn awk_roles(args: &[String], tainted: &[bool]) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut program_given = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        if arg == "--" || !is_option(arg) {
            let first = if arg == "--" { i + 1 } else { i };
            for (j, role) in roles.iter_mut().enumerate().skip(first) {
                *role = if !program_given && j == first {
                    Role::Code
                } else {
                    Role::Data
                };
            }
            return roles;
        }
        let flag = arg.get(..2).unwrap_or(arg);
        let attached = arg.get(2..).unwrap_or_default();
        let value_index = if attached.is_empty() { i + 1 } else { i };
        match flag {
            "-v" => {
                let value = args
                    .get(value_index)
                    .map(String::as_str)
                    .unwrap_or_default();
                let value = if value_index == i { attached } else { value };
                if is_assignment(value) && value_index < args.len() {
                    roles[value_index] = Role::Data;
                }
                i = value_index + 1;
            }
            "-e" => {
                program_given = true;
                if value_index < args.len() {
                    roles[value_index] = Role::Code;
                }
                i = value_index + 1;
            }
            "-f" | "-E" | "-i" | "-l" | "-F" => {
                program_given |= flag == "-f" || flag == "-E";
                i = value_index + 1;
            }
            _ if arg.starts_with("--source") => {
                program_given = true;
                roles[i] = Role::Code;
                if !arg.contains('=') && i + 1 < args.len() {
                    roles[i + 1] = Role::Code;
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    roles
}

/// sed: the script is `-e`/`--expression` or, without them, the first
/// operand; `-f` names a script file. Other operands are files (data).
fn sed_roles(args: &[String], tainted: &[bool]) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let script_given = args.iter().enumerate().any(|(i, arg)| {
        !is_tainted(tainted, i)
            && (arg.starts_with("--expression")
                || arg.starts_with("--file")
                || (!arg.starts_with("--")
                    && arg.strip_prefix('-').is_some_and(|cluster| {
                        cluster
                            .chars()
                            .take_while(|c| *c != 'i')
                            .any(|c| c == 'e' || c == 'f')
                    })))
    });
    let mut script_seen = script_given;
    let mut options_done = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if is_tainted(tainted, i) && !options_done {
            i += 1;
            continue;
        }
        if options_done || !is_option(arg) {
            if !options_done && arg == "--" {
                options_done = true;
                i += 1;
                continue;
            }
            roles[i] = if script_seen { Role::Data } else { Role::Code };
            script_seen = true;
            options_done = true;
            i += 1;
            continue;
        }
        if arg.starts_with("--expression") || arg.starts_with("--file") {
            let code = arg.starts_with("--expression");
            roles[i] = if code { Role::Code } else { Role::Option };
            if !arg.contains('=') && i + 1 < args.len() {
                roles[i + 1] = if code { Role::Code } else { Role::Option };
                i += 1;
            }
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            i += if arg == "--line-length" { 2 } else { 1 };
            continue;
        }
        // A short cluster: `-n`, `-ne SCRIPT`, `-escript`, `-i.bak`.
        let cluster = arg.strip_prefix('-').unwrap_or_default();
        let mut consumed_next = false;
        for (at, c) in cluster.char_indices() {
            match c {
                'i' => break, // the rest is an in-place suffix
                'e' | 'f' | 'l' => {
                    let role = if c == 'e' { Role::Code } else { Role::Option };
                    if at + 1 < cluster.len() {
                        roles[i] = role;
                    } else if i + 1 < args.len() {
                        roles[i + 1] = role;
                        if c == 'e' {
                            roles[i] = Role::Code;
                        }
                        consumed_next = true;
                    }
                    break;
                }
                _ => {}
            }
        }
        i += if consumed_next { 2 } else { 1 };
    }
    roles
}

/// Where data starts after the script of [`script_roles`].
enum Tail {
    /// Arguments after the script file are its data.
    AfterScript,
    /// Data only after a marker (`--args` for R).
    AfterMarker(&'static str),
}

/// Interpreter-like evaluators: `code_options` take program text, the first
/// operand is a script file, data follows per `tail`.
fn script_roles(
    args: &[String],
    tainted: &[bool],
    code_options: &[&str],
    value_options: &[&str],
    tail: Tail,
) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = &args[i];
        if let Tail::AfterMarker(marker) = tail {
            if arg == marker {
                roles[i + 1..].fill(Role::Data);
                return roles;
            }
        }
        if arg == "--" {
            if let Tail::AfterScript = tail {
                // The script name follows, then its data.
                roles[(i + 2).min(args.len())..].fill(Role::Data);
                return roles;
            }
            i += 1;
            continue;
        }
        if is_option(arg) {
            if let Some(code) = code_options.iter().find(|code| arg.starts_with(**code)) {
                roles[i] = Role::Code;
                if arg.len() == code.len() && i + 1 < args.len() {
                    roles[i + 1] = Role::Code;
                    i += 1;
                }
                i += 1;
                continue;
            }
            i += if value_options.contains(&long_name(arg)) && !arg.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        // The script file itself must not come from a value.
        if let Tail::AfterScript = tail {
            roles[i + 1..].fill(Role::Data);
            return roles;
        }
        i += 1;
    }
    roles
}

/// find: starting points, then an expression. A value may be the operand of
/// a test (`-name {{x}}`) but never an operator or a starting point (it
/// could render to `-delete`); `-exec`, `-execdir`, `-ok`, `-okdir` run a
/// command whose own arguments are classified as such.
fn find_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    const TESTS_WITH_VALUE: &[&str] = &[
        "-name",
        "-iname",
        "-path",
        "-ipath",
        "-wholename",
        "-iwholename",
        "-regex",
        "-iregex",
        "-lname",
        "-ilname",
        "-newer",
        "-anewer",
        "-cnewer",
        "-samefile",
        "-user",
        "-group",
        "-uid",
        "-gid",
        "-type",
        "-xtype",
        "-size",
        "-perm",
        "-mtime",
        "-mmin",
        "-atime",
        "-amin",
        "-ctime",
        "-cmin",
        "-maxdepth",
        "-mindepth",
        "-links",
        "-inum",
        "-used",
        "-fstype",
        "-context",
        "-regextype",
        "-printf",
    ];
    const RUNS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if TESTS_WITH_VALUE.contains(&arg) {
            if i + 1 < args.len() {
                roles[i + 1] = Role::Data;
            }
            i += 2;
            continue;
        }
        if RUNS.contains(&arg) {
            let end = (i + 1..args.len())
                .find(|&j| !is_tainted(tainted, j) && (args[j] == ";" || args[j] == "+"))
                .unwrap_or(args.len());
            if i + 1 < end {
                roles[i + 1] = Role::Executable;
                let inner = roles_at(
                    &args[i + 1],
                    &args[i + 2..end],
                    &tainted[i + 2..end],
                    depth + 1,
                );
                roles[i + 2..end].copy_from_slice(&inner);
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    roles
}

/// git: `-c name=value` and `--config-env` can run commands (`alias.x=!…`,
/// `core.sshCommand`, `credential.helper`…), so their values are code. The
/// global options and the subcommand are option positions. After the
/// subcommand an operand is data unless its rendered value starts with `-`
/// (`--upload-pack=…`), checked at run time; after `--`, plain data.
fn git_roles(args: &[String], tainted: &[bool]) -> Vec<Role> {
    const GLOBAL_VALUES: &[&str] = &["-C", "--git-dir", "--work-tree", "--namespace"];
    const SUBCOMMAND_VALUES: &[&str] = &[
        "-m",
        "--message",
        "-b",
        "-B",
        "--branch",
        "--author",
        "--date",
        "-t",
        "--track",
    ];
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    // Global options, up to the subcommand.
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if arg == "-c" || arg == "--config-env" {
            if i + 1 < args.len() {
                roles[i + 1] = Role::Code;
            }
            i += 2;
            continue;
        }
        if arg.starts_with("--config-env=") || arg.starts_with("--exec-path") {
            roles[i] = Role::Code;
            i += 1;
            continue;
        }
        if is_option(arg) {
            if GLOBAL_VALUES.contains(&long_name(arg)) && !arg.contains('=') {
                if i + 1 < args.len() {
                    roles[i + 1] = Role::Data;
                }
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        break; // the subcommand
    }
    if i >= args.len() || is_tainted(tainted, i) {
        return roles;
    }
    let subcommand = args[i].as_str();
    // Subcommands whose operands are a command to run.
    if subcommand == "submodule" && args.get(i + 1).is_some_and(|next| next == "foreach") {
        roles[i + 2..].fill(Role::Code);
        return roles;
    }
    if subcommand == "bisect" && args.get(i + 1).is_some_and(|next| next == "run") {
        let command = i + 2;
        if command < args.len() {
            roles[command] = Role::Executable;
            let inner = roles_at(
                &args[command],
                &args[command + 1..],
                &tainted[command + 1..],
                1,
            );
            roles[command + 1..].copy_from_slice(&inner);
        }
        return roles;
    }
    if subcommand == "config" {
        // A config value can be a command (`alias.x=!…`, `core.sshCommand`).
        for (j, role) in roles.iter_mut().enumerate().skip(i + 1) {
            *role = if is_tainted(tainted, j) {
                Role::Code
            } else {
                Role::Data
            };
        }
        return roles;
    }
    let code_options = git_command_options(subcommand);
    let mut i = i + 1;
    while i < args.len() {
        let arg = args[i].as_str();
        if !is_tainted(tainted, i) && arg == "--" {
            roles[i + 1..].fill(Role::Data);
            return roles;
        }
        if is_tainted(tainted, i) {
            roles[i] = if is_option(arg) {
                // `--exec={{x}}` / `-x{{x}}`: the value of an option the
                // author wrote; code when that option runs a command.
                if attached_option_matches(arg, code_options) {
                    Role::Code
                } else if attached_value(arg, git_value_letters(subcommand)) {
                    Role::Data
                } else {
                    Role::Option
                }
            } else {
                Role::RuntimeOption
            };
            i += 1;
            continue;
        }
        roles[i] = Role::RuntimeOption;
        if option_in(long_name(arg), code_options) && !arg.contains('=') {
            if i + 1 < args.len() {
                roles[i + 1] = Role::Code;
            }
            i += 2;
            continue;
        }
        if SUBCOMMAND_VALUES.contains(&arg) && i + 1 < args.len() {
            roles[i + 1] = Role::Data;
            i += 2;
            continue;
        }
        i += 1;
    }
    roles
}

/// Short options of a git subcommand that take a value (`commit -m{{msg}}`).
fn git_value_letters(subcommand: &str) -> &'static [char] {
    match subcommand {
        "commit" | "tag" | "merge" | "notes" | "stash" => &['m'],
        "log" | "shortlog" | "rev-list" | "whatchanged" => &['n'],
        _ => &[],
    }
}

/// Options of a git subcommand whose value is a command to run.
fn git_command_options(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "rebase" => &["-x", "--exec"],
        "fetch" | "pull" | "ls-remote" => &["--upload-pack"],
        "clone" => &["-u", "--upload-pack", "-c", "--config", "--template"],
        "push" => &["--receive-pack", "--exec"],
        "archive" => &["--exec", "--remote"],
        "difftool" | "mergetool" => &["-x", "--extcmd", "-t", "--tool"],
        "grep" => &["-O", "--open-files-in-pager"],
        "filter-branch" => &[
            "--env-filter",
            "--tree-filter",
            "--index-filter",
            "--parent-filter",
            "--msg-filter",
            "--commit-filter",
            "--tag-name-filter",
        ],
        "send-email" => &["--sendmail-cmd", "--smtp-server", "--to-cmd", "--cc-cmd"],
        _ => &[],
    }
}

/// Whether `arg` is one of `options` with its value attached (`-xVALUE`,
/// `--exec=VALUE`, or an unambiguous long prefix with `=`).
fn attached_option_matches(arg: &str, options: &[&str]) -> bool {
    if arg.starts_with("--") {
        return arg.contains('=') && option_in(long_name(arg), options);
    }
    options
        .iter()
        .any(|option| option.len() == 2 && !option.starts_with("--") && arg.starts_with(option))
}

/// ssh: options can run local commands (`-o ProxyCommand=…`), the
/// destination could render to an option, and everything after it is a
/// command line for the remote shell. No argument is data.
fn ssh_roles(args: &[String], tainted: &[bool]) -> Vec<Role> {
    const VALUES: &[&str] = &[
        "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p",
        "-Q", "-R", "-S", "-W", "-w",
    ];
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if is_option(arg) {
            i += if VALUES.contains(&arg) { 2 } else { 1 };
            continue;
        }
        roles[i + 1..].fill(Role::Code);
        break;
    }
    roles
}

/// docker / podman: `run`/`create`/`exec` start a program inside a
/// container, classified like a direct command after the image (or
/// container). Options, the image and `--entrypoint` are never values.
fn docker_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    const GLOBAL_VALUES: &[&str] = &[
        "--context",
        "-c",
        "-H",
        "--host",
        "--config",
        "-l",
        "--log-level",
    ];
    const RUN_VALUES: &[&str] = &[
        "-a",
        "--attach",
        "--add-host",
        "--annotation",
        "--blkio-weight",
        "--cap-add",
        "--cap-drop",
        "--cgroup-parent",
        "--cgroupns",
        "--cidfile",
        "--cpu-period",
        "--cpu-quota",
        "-c",
        "--cpu-shares",
        "--cpus",
        "--cpuset-cpus",
        "--cpuset-mems",
        "--device",
        "--dns",
        "--dns-option",
        "--dns-search",
        "--domainname",
        "--entrypoint",
        "-e",
        "--env",
        "--env-file",
        "--expose",
        "--gpus",
        "--group-add",
        "--health-cmd",
        "--health-interval",
        "--health-retries",
        "--health-timeout",
        "-h",
        "--hostname",
        "--ip",
        "--ip6",
        "--ipc",
        "--isolation",
        "-l",
        "--label",
        "--label-file",
        "--link",
        "--log-driver",
        "--log-opt",
        "--mac-address",
        "-m",
        "--memory",
        "--memory-swap",
        "--mount",
        "--name",
        "--network",
        "--net",
        "--network-alias",
        "--pid",
        "--pids-limit",
        "--platform",
        "-p",
        "--publish",
        "--pull",
        "--restart",
        "--runtime",
        "--security-opt",
        "--shm-size",
        "--stop-signal",
        "--stop-timeout",
        "--sysctl",
        "--tmpfs",
        "--ulimit",
        "-u",
        "--user",
        "--userns",
        "--uts",
        "-v",
        "--volume",
        "--volumes-from",
        "-w",
        "--workdir",
        "--detach-keys",
    ];
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if is_option(arg) {
            i += if GLOBAL_VALUES.contains(&long_name(arg)) && !arg.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        break;
    }
    if i >= args.len() || is_tainted(tainted, i) {
        return roles;
    }
    // `docker container run`, `docker compose [-f x] run|exec SERVICE`.
    let mut subcommand = args[i].as_str();
    if subcommand == "container" || subcommand == "compose" {
        let group = subcommand;
        i += 1;
        while i < args.len() && !is_tainted(tainted, i) && is_option(&args[i]) {
            let takes = group == "compose"
                && matches!(
                    long_name(&args[i]),
                    "-f" | "--file"
                        | "-p"
                        | "--project-name"
                        | "--profile"
                        | "--env-file"
                        | "--project-directory"
                )
                && !args[i].contains('=');
            i += if takes { 2 } else { 1 };
        }
        if i >= args.len() || is_tainted(tainted, i) {
            return roles;
        }
        subcommand = args[i].as_str();
    }
    if !matches!(subcommand, "run" | "create" | "exec") {
        // Unknown subcommands are not modelled: a value stays out.
        return roles;
    }
    let mut i = i + 1;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if arg == "--" {
            i += 1;
            break;
        }
        if is_option(arg) {
            i += if RUN_VALUES.contains(&long_name(arg)) && !arg.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        break;
    }
    // `i` is the image (run/create) or the container (exec), then the command.
    let command = i + 1;
    if command < args.len() {
        roles[command] = Role::Executable;
        let inner = roles_at(
            &args[command],
            &args[command + 1..],
            &tainted[command + 1..],
            depth + 1,
        );
        roles[command + 1..].copy_from_slice(&inner);
    }
    roles
}

/// npx: `-c`/`--call` take a shell command line; `-p`/`--package` name what
/// gets installed; the first operand is the program, its arguments follow.
fn npx_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if arg == "--" {
            i += 1;
            break;
        }
        if arg == "-c" || arg.starts_with("--call") {
            roles[i] = Role::Code;
            if !arg.contains('=') && i + 1 < args.len() {
                roles[i + 1] = Role::Code;
                i += 1;
            }
            i += 1;
            continue;
        }
        if is_option(arg) {
            i += if (arg == "-p" || arg == "--package") && !arg.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        break;
    }
    if i < args.len() {
        roles[i] = Role::Executable;
        let inner = roles_at(&args[i], &args[i + 1..], &tainted[i + 1..], depth + 1);
        roles[i + 1..].copy_from_slice(&inner);
    }
    roles
}

// ─── Programs that take code in an option ────────────────────────────────────

/// How the operands (non-option arguments) of a [`flag_roles`] program are read.
#[derive(Clone, Copy)]
enum Operands {
    /// Every operand has this role.
    All(Role),
    /// The first operand is the program to run, the rest are its data.
    ProgramThenData,
    /// The first operand has this role, every later one is code (sqlite3 SQL).
    FirstThenCode(Role),
}

/// A program whose code arrives through options.
struct FlagSpec {
    /// Options whose value (attached after `=` or the next argument) is code.
    code: &'static [&'static str],
    /// Short letters that take code, alone or in a cluster (`rsync -avze`).
    short_code: &'static [char],
    /// `+command` arguments are code (vim).
    plus_is_code: bool,
    /// A templated `NAME=value` operand assigns code (make variables expand
    /// `$(shell …)`).
    assignments_are_code: bool,
    operands: Operands,
    /// Short options taking a plain value (`mysql -u{{user}}`).
    values: &'static [char],
}

fn flag_roles(args: &[String], tainted: &[bool], spec: &FlagSpec) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut operand = 0;
    let mut i = 0;
    let operand_role = |index: usize| match spec.operands {
        Operands::All(role) => role,
        Operands::ProgramThenData if index == 0 => Role::Executable,
        Operands::ProgramThenData => Role::Data,
        Operands::FirstThenCode(role) if index == 0 => role,
        Operands::FirstThenCode(_) => Role::Code,
    };
    while i < args.len() {
        let arg = args[i].as_str();
        if is_tainted(tainted, i) {
            if is_option(arg) {
                // `-x{{v}}` / `--opt={{v}}`: the author wrote the option, the
                // value is the outside part.
                let cluster = arg.strip_prefix('-').unwrap_or_default();
                let code = spec.code.iter().any(|option| {
                    arg.starts_with(option) && !arg.starts_with("--")
                        || arg.starts_with(&format!("{option}="))
                }) || (arg.starts_with("--")
                    && arg.contains('=')
                    && option_in(long_name(arg), spec.code))
                    || (!arg.starts_with("--")
                        && cluster
                            .chars()
                            .take_while(|c| c.is_ascii_alphabetic())
                            .any(|c| spec.short_code.contains(&c)));
                roles[i] = if code {
                    Role::Code
                } else if attached_value(arg, spec.values) {
                    Role::Data
                } else {
                    Role::Option
                };
                i += 1;
                continue;
            }
            roles[i] = if (spec.assignments_are_code && is_assignment(arg))
                || (spec.plus_is_code && arg.starts_with('+'))
            {
                Role::Code
            } else {
                operand_role(operand)
            };
            operand += 1;
            i += 1;
            continue;
        }
        if arg == "--" {
            for (j, role) in roles.iter_mut().enumerate().skip(i + 1) {
                *role = match spec.operands {
                    Operands::FirstThenCode(_) => Role::Code,
                    // make still reads `NAME=value` after `--`.
                    _ if spec.assignments_are_code && is_tainted(tainted, j) => {
                        if is_assignment(&args[j]) {
                            Role::Code
                        } else {
                            Role::RuntimeOption
                        }
                    }
                    _ if j == i + 1
                        && matches!(spec.operands, Operands::ProgramThenData)
                        && operand == 0 =>
                    {
                        Role::Executable
                    }
                    _ => Role::Data,
                };
            }
            return roles;
        }
        if spec.plus_is_code && arg.starts_with('+') {
            roles[i] = Role::Code;
            i += 1;
            continue;
        }
        if is_option(arg) {
            let name = long_name(arg);
            if option_in(name, spec.code) {
                roles[i] = Role::Code;
                if !arg.contains('=') && i + 1 < args.len() {
                    roles[i + 1] = Role::Code;
                    i += 1;
                }
                i += 1;
                continue;
            }
            if !arg.starts_with("--") {
                let cluster = arg.strip_prefix('-').unwrap_or_default();
                if let Some((at, c)) = cluster
                    .char_indices()
                    .find(|(_, c)| spec.short_code.contains(c))
                {
                    roles[i] = Role::Code;
                    if at + c.len_utf8() == cluster.len() && i + 1 < args.len() {
                        roles[i + 1] = Role::Code;
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        roles[i] = operand_role(operand);
        operand += 1;
        i += 1;
    }
    roles
}

/// npm / pnpm / yarn: `exec`, `x`, `dlx` run a program (and `-c`/`--call` a
/// command line), `install`/`add` fetch and run package code, `run` picks a
/// script by name. Other subcommands read operands as data unless they look
/// like options once rendered.
fn package_manager_roles(name: &str, args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut i = 0;
    while i < args.len() {
        if is_tainted(tainted, i) {
            i += 1;
            continue;
        }
        let arg = args[i].as_str();
        if is_option(arg) {
            let takes_value = matches!(
                long_name(arg),
                "--prefix" | "-C" | "--dir" | "--workspace" | "-w" | "--filter" | "-F" | "--cwd"
            ) && !arg.contains('=');
            i += if takes_value { 2 } else { 1 };
            continue;
        }
        break;
    }
    let subcommand = args.get(i).map(String::as_str).unwrap_or_default();
    if i >= args.len() || is_tainted(tainted, i) {
        return roles;
    }
    let rest = i + 1;
    match subcommand {
        "exec" if name == "yarn" => roles[rest..].fill(Role::Code),
        "exec" | "x" | "dlx" => {
            let inner = npx_roles(&args[rest..], &tainted[rest..], depth + 1);
            roles[rest..].copy_from_slice(&inner);
        }
        "install" | "i" | "add" | "ci" | "update" | "up" | "upgrade" | "link" => {
            for (j, role) in roles.iter_mut().enumerate().skip(rest) {
                *role = if is_tainted(tainted, j) {
                    Role::Executable
                } else {
                    Role::Data
                };
            }
        }
        "run" | "run-script" | "rr" => {
            // The script name selects code; values go after `--`.
            let dash = (rest..args.len()).find(|&j| !is_tainted(tainted, j) && args[j] == "--");
            if let Some(dash) = dash {
                roles[dash + 1..].fill(Role::Data);
            }
        }
        _ if name == "yarn" && !matches!(subcommand, "info" | "why" | "list" | "outdated") => {
            // `yarn <script>` runs a script by name.
        }
        _ => {
            for (j, role) in roles.iter_mut().enumerate().skip(rest) {
                *role = if is_tainted(tainted, j) {
                    Role::RuntimeOption
                } else {
                    Role::Data
                };
            }
        }
    }
    roles
}

/// Interpreters and evaluators without a modelled argv: a value anywhere in
/// their arguments is refused rather than assumed to be data.
const UNMODELLED_EVALUATORS: &[&str] = &[
    "bc",
    "dc",
    "expect",
    "julia",
    "ghci",
    "runghc",
    "runhaskell",
    "racket",
    "guile",
    "sbcl",
    "clisp",
    "ecl",
    "erl",
    "escript",
    "elixir",
    "iex",
    "groovy",
    "scala",
    "kotlin",
    "kotlinc",
    "jshell",
    "swift",
    "ocaml",
    "jjs",
    "rhino",
    "tcc",
    "emacs",
    "ed",
    "cmd",
    "wscript",
    "cscript",
    "mshta",
    "crontab",
    "at",
    "batch",
];

/// Programs whose operands are neither code nor paths: a value can only be
/// printed, compared or formatted. Programs that read a path (`cat`, `grep`,
/// `jq`), write one (`cp`, `tee`, `rm`, `chmod`) or read the environment
/// (`printenv`) need a human's approval for a value.
pub const DATA_ONLY_PROGRAMS: &[&str] = &[
    "echo", "printf", "test", "[", "true", "false", "basename", "dirname", "seq", "date",
];

/// Whether `cmd` is a data-only program. The exact base name is compared,
/// without the version-suffix stripping of [`normalize_command`]: `diff3`
/// (which takes `--diff-program`) is not `diff`.
pub fn is_data_only(cmd: &str) -> bool {
    DATA_ONLY_PROGRAMS.contains(&exact_base(cmd).as_str())
}

/// The base name of `cmd`, lowercased, without `.exe` or version stripping.
fn exact_base(cmd: &str) -> String {
    let base = cmd
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

/// An interpreter with no modelled argv (see [`UNMODELLED_EVALUATORS`]).
pub fn is_unmodelled_evaluator(name: &str) -> bool {
    UNMODELLED_EVALUATORS.contains(&name)
}

fn flag_spec(name: &str) -> Option<FlagSpec> {
    let spec = |code, short_code, operands| FlagSpec {
        code,
        short_code,
        plus_is_code: false,
        assignments_are_code: false,
        operands,
        values: &[],
    };
    Some(match name {
        "make" | "gmake" | "bmake" => FlagSpec {
            code: &["--eval", "-E"],
            short_code: &[],
            plus_is_code: false,
            assignments_are_code: true,
            operands: Operands::All(Role::RuntimeOption),
            values: &[],
        },
        "gdb" => spec(
            &[
                "-ex",
                "--ex",
                "-iex",
                "--iex",
                "--eval-command",
                "--init-eval-command",
                "-x",
                "--command",
                "-ix",
                "--init-command",
                "-p",
                "--pid",
                "-eval-command",
                "-init-eval-command",
                "-command",
                "-init-command",
            ],
            &[],
            Operands::ProgramThenData,
        ),
        "tar" | "gtar" | "bsdtar" => FlagSpec {
            values: &['f', 'C', 'T', 'X', 'b'],
            ..spec(
                &[
                    "--to-command",
                    "--checkpoint-action",
                    "--use-compress-program",
                    "-I",
                    "--rsh-command",
                    "--info-script",
                    "--new-volume-script",
                    "-F",
                ],
                &['I', 'F'],
                Operands::All(Role::RuntimeOption),
            )
        },
        // BSD `script [opts] file [command ...]` runs the operands after the
        // file, so only the first operand can be a value.
        "script" => spec(
            &["-c", "--command"],
            &[],
            Operands::FirstThenCode(Role::RuntimeOption),
        ),
        "su" => spec(
            &[
                "-c",
                "--command",
                "-s",
                "--shell",
                "-C",
                "--session-command",
            ],
            &[],
            Operands::All(Role::Option),
        ),
        "scp" => FlagSpec {
            values: &['P', 'i', 'l', 'c'],
            ..spec(&["-o", "-S", "-F", "-J"], &[], Operands::All(Role::Option))
        },
        "sftp" => FlagSpec {
            values: &['P', 'i', 'l', 'B', 'R'],
            ..spec(
                &["-o", "-S", "-F", "-b", "-D", "-J"],
                &[],
                Operands::All(Role::RuntimeOption),
            )
        },
        "rsync" => spec(
            &["--rsh", "--rsync-path"],
            &['e'],
            Operands::All(Role::RuntimeOption),
        ),
        "curl" => FlagSpec {
            values: &[
                'H', 'd', 'u', 'o', 'A', 'X', 'e', 'b', 'F', 'T', 'm', 'x', 'w',
            ],
            ..spec(&["-K", "--config"], &[], Operands::All(Role::RuntimeOption))
        },
        "vim" | "vi" | "nvim" | "ex" | "view" | "gvim" | "vimdiff" => FlagSpec {
            code: &["-c", "--cmd", "-S", "-u", "-U", "-s", "-w", "-W"],
            short_code: &[],
            plus_is_code: true,
            assignments_are_code: false,
            operands: Operands::All(Role::Option),
            values: &[],
        },
        // sqlite3 reads `-cmd` and `--cmd` alike.
        "sqlite" => spec(
            &["-cmd", "--cmd", "-init", "--init"],
            &[],
            Operands::FirstThenCode(Role::RuntimeOption),
        ),
        "mysql" | "mariadb" => FlagSpec {
            values: &['u', 'p', 'h', 'P', 'D', 'S'],
            ..spec(
                &["--execute", "--init-command", "--init-command-add"],
                &['e'],
                Operands::All(Role::RuntimeOption),
            )
        },
        "psql" => FlagSpec {
            values: &['U', 'h', 'p', 'd'],
            ..spec(
                &["--command", "--file", "--set", "--variable"],
                &['c', 'f', 'v'],
                Operands::All(Role::RuntimeOption),
            )
        },
        _ => return None,
    })
}

// ─── More launchers ──────────────────────────────────────────────────────────

/// tar: the traditional first argument is an option cluster without a dash
/// (`tar cIf PROG a.tar dir`), each value letter taking the next argument in
/// order; `I` and `F` take a program.
fn tar_roles(name: &str, args: &[String], tainted: &[bool]) -> Vec<Role> {
    let spec = flag_spec(name).expect("tar has a flag spec");
    let traditional = args.first().is_some_and(|first| {
        !is_tainted(tainted, 0)
            && !first.starts_with('-')
            && !first.is_empty()
            && first.chars().all(|c| c.is_ascii_alphabetic())
    });
    if !traditional {
        return flag_roles(args, tainted, &spec);
    }
    let mut roles = vec![Role::Option; args.len()];
    let mut next = 1;
    for letter in args[0].chars() {
        if matches!(
            letter,
            'f' | 'b' | 'I' | 'F' | 'T' | 'X' | 'C' | 'L' | 'N' | 'K' | 'V' | 'g'
        ) && next < args.len()
        {
            roles[next] = match letter {
                'I' | 'F' => Role::Code,
                _ => Role::RuntimeOption,
            };
            next += 1;
        }
    }
    let rest = flag_roles(&args[next..], &tainted[next..], &spec);
    roles[next..].copy_from_slice(&rest);
    roles
}

/// kubectl / oc: `exec`, `run`, `debug`, `attach` start a program after
/// `--`; before it, a value is refused (pod, image, options). Other
/// subcommands read operands as data unless they look like options.
fn kubectl_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let Some(sub) = (0..args.len()).find(|&i| !is_tainted(tainted, i) && !is_option(&args[i]))
    else {
        return roles;
    };
    if !matches!(args[sub].as_str(), "exec" | "run" | "debug" | "attach") {
        for (j, role) in roles.iter_mut().enumerate().skip(sub + 1) {
            *role = if is_tainted(tainted, j) {
                Role::RuntimeOption
            } else {
                Role::Data
            };
        }
        return roles;
    }
    if let Some(dash) = (sub + 1..args.len()).find(|&j| !is_tainted(tainted, j) && args[j] == "--")
    {
        let command = dash + 1;
        if command < args.len() {
            roles[command] = Role::Executable;
            let inner = roles_at(
                &args[command],
                &args[command + 1..],
                &tainted[command + 1..],
                depth + 1,
            );
            roles[command + 1..].copy_from_slice(&inner);
        }
    }
    roles
}

/// runuser: `-u USER -- cmd args` launches a program; the `su`-like form
/// takes a command line in `-c`.
fn runuser_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let dash = (0..args.len()).find(|&j| !is_tainted(tainted, j) && args[j] == "--");
    let user_form = args[..dash.unwrap_or(args.len())]
        .iter()
        .any(|arg| arg == "-u" || arg.starts_with("--user"));
    if let (true, Some(dash)) = (user_form, dash) {
        let mut roles = vec![Role::Option; args.len()];
        let command = dash + 1;
        if command < args.len() {
            roles[command] = Role::Executable;
            let inner = roles_at(
                &args[command],
                &args[command + 1..],
                &tainted[command + 1..],
                depth + 1,
            );
            roles[command + 1..].copy_from_slice(&inner);
        }
        return roles;
    }
    flag_roles(args, tainted, &flag_spec("su").expect("su has a flag spec"))
}

/// GNU parallel: the command template runs through a shell, so it is code;
/// inputs after `:::` / `::::` are quoted by parallel and stay data.
fn parallel_roles(args: &[String], tainted: &[bool]) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let mut inputs = false;
    for (j, arg) in args.iter().enumerate() {
        if !is_tainted(tainted, j) && arg.starts_with(":::") {
            inputs = true;
            continue;
        }
        roles[j] = if inputs { Role::Data } else { Role::Code };
    }
    roles
}

/// Package fetchers that install or run fetched code: a value naming what
/// to install or run is a program to run.
fn fetcher_roles(name: &str, args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let literal =
        |i: usize| (!is_tainted(tainted, i) && !is_option(&args[i])).then(|| args[i].as_str());
    let Some(sub) = (0..args.len()).find(|&i| literal(i).is_some()) else {
        return roles;
    };
    let second = (sub + 1 < args.len()).then(|| literal(sub + 1)).flatten();
    let (kind, rest) = match (name, args[sub].as_str(), second) {
        ("uv", "tool", Some("run")) => ("run", sub + 2),
        ("uv", "tool", Some("install")) | ("uv", "pip", Some("install")) => ("install", sub + 2),
        ("uv", "run", _) => ("run", sub + 1),
        ("uv", "add", _) => ("install", sub + 1),
        ("pipx", "run", _) => ("run", sub + 1),
        ("pipx", "install" | "inject", _) => ("install", sub + 1),
        ("pip", "install", _) => ("install", sub + 1),
        ("go", "run" | "install" | "get", _) => ("install", sub + 1),
        ("go", "generate", _) => ("deny", sub + 1),
        ("cargo", "install", _) => ("install", sub + 1),
        ("gem", "install", _) => ("install", sub + 1),
        _ => ("data", sub + 1),
    };
    match kind {
        "run" => {
            let inner = npx_roles(&args[rest..], &tainted[rest..], depth + 1);
            roles[rest..].copy_from_slice(&inner);
        }
        "install" => {
            for (j, role) in roles.iter_mut().enumerate().skip(rest) {
                *role = if is_tainted(tainted, j) {
                    Role::Executable
                } else {
                    Role::Data
                };
            }
        }
        "deny" => {}
        _ => {
            for (j, role) in roles.iter_mut().enumerate().skip(rest) {
                *role = if is_tainted(tainted, j) {
                    Role::RuntimeOption
                } else {
                    Role::Data
                };
            }
        }
    }
    roles
}

/// deno: `eval CODE` is code; `run`/`x`/`test`/`bench`/`compile`/`install`
/// take a script or package (never a value), whose arguments are data.
fn deno_roles(args: &[String], tainted: &[bool], depth: usize) -> Vec<Role> {
    let mut roles = vec![Role::Option; args.len()];
    let Some(sub) = (0..args.len()).find(|&i| !is_tainted(tainted, i) && !is_option(&args[i]))
    else {
        // `deno -e`-like flags or no subcommand: option positions only.
        return crate::core::inline_code::interpreter_roles("deno", args, tainted).unwrap_or(roles);
    };
    match args[sub].as_str() {
        "eval" => roles[sub + 1..].fill(Role::Code),
        "run" | "x" | "test" | "bench" | "compile" | "install" => {
            if let Some(script) =
                (sub + 1..args.len()).find(|&i| is_tainted(tainted, i) || !is_option(&args[i]))
            {
                roles[script] = Role::Executable;
                roles[script + 1..].fill(Role::Data);
            }
        }
        _ => {}
    }
    let _ = depth;
    roles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::inline_code::{first_unsafe_placeholder, rendered_refusal, InlineFinding};

    fn line(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn refused(cmd: &str, items: &[&str]) -> bool {
        first_unsafe_placeholder(cmd, &line(items)).is_some()
    }

    /// Accepted once a human approved the line: no refusal an approval
    /// cannot lift (code, option or program position).
    fn approvable(cmd: &str, items: &[&str]) -> bool {
        crate::core::inline_code::first_unsafe_placeholder_with(cmd, &line(items), true).is_none()
    }

    #[test]
    fn program_names_are_normalised() {
        for (raw, expected) in [
            ("/usr/bin/python3.12", "python"),
            ("./venv/bin/python", "python"),
            ("python3", "python"),
            ("pypy3", "pypy"),
            ("node18", "node"),
            ("ruby3.2", "ruby"),
            ("lua5.4", "lua"),
            ("PYTHON.EXE", "python"),
            ("C:\\Python312\\python.exe", "python"),
            ("Rscript", "rscript"),
            ("bash", "bash"),
        ] {
            assert_eq!(normalize_command(raw), expected, "{raw}");
        }
        for (cmd, items) in [
            ("/usr/bin/python3.12", vec!["-c", "print('{{x}}')"]),
            ("./venv/bin/python", vec!["-c", "print('{{x}}')"]),
            ("python3-dbg", vec!["-c", "print('{{x}}')"]),
            ("node18", vec!["-e", "console.log('{{x}}')"]),
            ("ruby3.2", vec!["-e", "puts '{{x}}'"]),
            ("PYTHON.EXE", vec!["-c", "print('{{x}}')"]),
            ("/bin/BASH", vec!["-c", "echo \"{{x}}\""]),
        ] {
            assert!(refused(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn wrappers_are_unwrapped_and_their_own_values_refused() {
        for (cmd, items) in [
            ("env", vec!["python3", "-c", "print('{{x}}')"]),
            ("env", vec!["-i", "A=1", "bash", "-c", "echo \"{{x}}\""]),
            ("env", vec!["PATH={{x}}", "make"]),
            ("env", vec!["-S", "bash -c {{x}}"]),
            ("env", vec!["{{cmd}}", "arg"]),
            ("nice", vec!["-n", "{{x}}", "make"]),
            ("nohup", vec!["sh", "-c", "echo \"{{x}}\""]),
            ("timeout", vec!["{{x}}", "make"]),
            (
                "timeout",
                vec!["-s", "KILL", "5", "node", "-e", "console.log('{{x}}')"],
            ),
            ("time", vec!["-f", "%e", "python3", "-c", "print('{{x}}')"]),
            ("command", vec!["bash", "-c", "echo \"{{x}}\""]),
            ("exec", vec!["-a", "name", "perl", "-e", "print '{{x}}'"]),
            ("stdbuf", vec!["-o", "L", "python3", "-c", "print('{{x}}')"]),
            ("sudo", vec!["-u", "{{user}}", "make"]),
            ("sudo", vec!["X=1", "bash", "-c", "echo \"{{x}}\""]),
            ("doas", vec!["-u", "root", "sh", "-c", "echo \"{{x}}\""]),
            ("xargs", vec!["-I", "{}", "sh", "-c", "echo \"{{x}}\" {}"]),
            ("xargs", vec!["{{cmd}}"]),
            ("busybox", vec!["sh", "-c", "echo \"{{x}}\""]),
            (
                "env",
                vec![
                    "env",
                    "nice",
                    "timeout",
                    "5",
                    "python3",
                    "-c",
                    "print('{{x}}')",
                ],
            ),
        ] {
            assert!(refused(cmd, &items), "{cmd} {items:?}");
        }
        for (cmd, items) in [
            (
                "env",
                vec![
                    "A=1",
                    "python3",
                    "-c",
                    "import sys; print(sys.argv[1])",
                    "{{x}}",
                ],
            ),
            ("timeout", vec!["30", "make", "{{target}}"]),
            (
                "nice",
                vec!["-n", "10", "bash", "-c", "echo \"$1\"", "_", "{{x}}"],
            ),
            ("xargs", vec!["-n", "1", "echo", "{{x}}"]),
        ] {
            assert!(approvable(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn other_evaluators_are_classified_at_their_program_text() {
        for (cmd, items) in [
            ("awk", vec!["{print \"{{x}}\"}"]),
            ("gawk", vec!["-F", ",", "{print \"{{x}}\"}", "f.csv"]),
            ("awk", vec!["-v", "{{x}}", "{print}"]),
            ("awk", vec!["-e", "BEGIN{print \"{{x}}\"}"]),
            ("sed", vec!["s/a/{{x}}/"]),
            ("sed", vec!["-n", "-e", "{{x}}", "f"]),
            ("sed", vec!["--expression={{x}}", "f"]),
            ("sed", vec!["-i.bak", "s/a/{{x}}/", "f"]),
            ("osascript", vec!["-e", "display dialog \"{{x}}\""]),
            ("lua", vec!["-e", "print('{{x}}')"]),
            ("lua5.4", vec!["{{script}}"]),
            ("tclsh", vec!["{{x}}"]),
            ("Rscript", vec!["-e", "print('{{x}}')"]),
            ("R", vec!["-e", "print('{{x}}')"]),
            ("find", vec!["{{dir}}", "-name", "*.log"]),
            ("find", vec![".", "{{expr}}"]),
            (
                "find",
                vec![".", "-exec", "sh", "-c", "echo \"{{x}}\"", ";"],
            ),
            ("find", vec![".", "-execdir", "{{cmd}}", "{}", "+"]),
            ("find", vec![".", "-fprint", "{{file}}"]),
            ("git", vec!["-c", "alias.x=!{{x}}", "x"]),
            ("git", vec!["-c", "core.sshCommand={{x}}", "fetch"]),
            ("git", vec!["--config-env=core.pager={{x}}", "log"]),
            ("git", vec!["{{subcommand}}"]),
            ("ssh", vec!["host", "echo", "{{x}}"]),
            ("ssh", vec!["{{host}}", "uptime"]),
            ("ssh", vec!["-o", "{{x}}", "host"]),
            (
                "docker",
                vec!["run", "--rm", "img", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("docker", vec!["run", "-e", "X={{x}}", "img"]),
            ("docker", vec!["run", "--entrypoint", "{{x}}", "img"]),
            (
                "podman",
                vec!["exec", "box", "python3", "-c", "print('{{x}}')"],
            ),
            ("docker", vec!["run", "{{image}}"]),
            ("npx", vec!["-c", "echo {{x}}"]),
            ("npx", vec!["{{pkg}}"]),
            ("npx", vec!["-p", "{{pkg}}", "tool"]),
            ("zsh", vec!["-c", "echo \"{{x}}\""]),
            ("fish", vec!["-c", "echo \"{{x}}\""]),
            ("dash", vec!["-c", "echo \"{{x}}\""]),
            ("ksh", vec!["-c", "echo \"{{x}}\""]),
        ] {
            assert!(refused(cmd, &items), "{cmd} {items:?}");
        }
        for (cmd, items) in [
            ("awk", vec!["-v", "name={{x}}", "{print name}", "f"]),
            ("awk", vec!["{print}", "{{file}}"]),
            ("sed", vec!["s/a/b/", "{{file}}"]),
            ("lua", vec!["script.lua", "{{x}}"]),
            ("tclsh", vec!["s.tcl", "{{x}}"]),
            ("Rscript", vec!["report.R", "--args", "{{x}}"]),
            ("R", vec!["--vanilla", "--args", "{{x}}"]),
            ("find", vec![".", "-name", "{{pattern}}", "-type", "f"]),
            ("find", vec![".", "-exec", "echo", "{{x}}", "{}", ";"]),
            ("git", vec!["commit", "-m", "{{message}}"]),
            ("git", vec!["checkout", "-b", "{{branch}}"]),
            ("git", vec!["fetch", "{{remote}}"]),
            ("git", vec!["log", "--", "{{path}}"]),
            ("docker", vec!["run", "--rm", "img", "echo", "{{x}}"]),
            ("npx", vec!["prettier", "--check", "out.txt"]),
        ] {
            assert!(approvable(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn a_templated_executable_is_always_refused() {
        assert_eq!(
            first_unsafe_placeholder("{{cmd}}", &line(&["a"])),
            Some(InlineFinding::TemplatedExecutable("cmd".into()))
        );
        assert!(matches!(
            first_unsafe_placeholder("env", &line(&["{{cmd}}"])),
            Some(InlineFinding::TemplatedExecutable(_))
        ));
        // An unknown program needs a human's approval of its arguments.
        assert_eq!(
            first_unsafe_placeholder("mytool", &line(&["{{x}}", "-o", "{{y}}"])),
            Some(InlineFinding::UnmodelledProgram(
                "mytool".into(),
                "x".into()
            ))
        );
    }

    #[test]
    fn an_option_like_rendered_operand_is_refused_at_run_time() {
        let templates = line(&["fetch", "{{remote}}"]);
        assert!(rendered_refusal(
            "s",
            "git",
            &templates,
            &line(&["fetch", "--upload-pack=touch x"]),
            true
        )
        .is_some());
        assert!(
            rendered_refusal("s", "git", &templates, &line(&["fetch", "origin"]), true).is_none()
        );
        let templates = line(&["log", "--", "{{path}}"]);
        assert!(
            rendered_refusal("s", "git", &templates, &line(&["log", "--", "-x"]), true).is_none()
        );
    }

    #[test]
    fn programs_taking_code_in_options_are_classified() {
        for (cmd, items) in [
            ("make", vec!["--eval={{x}}"]),
            ("make", vec!["--eval", "{{x}}"]),
            ("gmake", vec!["-E", "{{x}}", "all"]),
            ("make", vec!["CFLAGS={{x}}", "all"]),
            ("npm", vec!["exec", "-c", "{{x}}"]),
            ("npm", vec!["exec", "--call={{x}}"]),
            ("npm", vec!["exec", "{{pkg}}"]),
            ("npm", vec!["x", "--", "{{pkg}}"]),
            ("pnpm", vec!["dlx", "{{x}}"]),
            ("pnpm", vec!["exec", "-c", "{{x}}"]),
            ("yarn", vec!["dlx", "{{x}}"]),
            ("yarn", vec!["exec", "{{x}}"]),
            ("bunx", vec!["{{x}}"]),
            ("npm", vec!["install", "{{pkg}}"]),
            ("npm", vec!["run", "{{script}}"]),
            ("yarn", vec!["{{script}}"]),
            ("gdb", vec!["-ex", "{{x}}", "--batch"]),
            ("gdb", vec!["--eval-command={{x}}", "./prog"]),
            ("gdb", vec!["--batch", "{{program}}"]),
            ("tar", vec!["--to-command={{x}}", "-xf", "a.tar"]),
            (
                "tar",
                vec!["--checkpoint-action=exec={{x}}", "-cf", "a.tar", "dir"],
            ),
            ("tar", vec!["-I", "{{x}}", "-cf", "a.tar", "dir"]),
            ("rsync", vec!["-e", "{{x}}", "a", "b"]),
            ("rsync", vec!["-avze", "{{x}}", "a", "b"]),
            ("rsync", vec!["--rsh={{x}}", "a", "b"]),
            ("curl", vec!["-K", "{{x}}"]),
            ("curl", vec!["--config={{x}}"]),
            ("vim", vec!["-c", "{{x}}", "f"]),
            ("vim", vec!["+{{x}}", "f"]),
            ("nvim", vec!["{{file}}"]),
            ("ex", vec!["--cmd", "{{x}}"]),
            ("sqlite3", vec!["db.sqlite", "{{sql}}"]),
            ("sqlite3", vec!["-cmd", "{{x}}", "db.sqlite"]),
            ("mysql", vec!["-e", "{{x}}"]),
            ("mysql", vec!["--execute={{x}}", "db"]),
            ("psql", vec!["-c", "{{x}}"]),
            ("psql", vec!["-v", "name={{x}}", "-f", "q.sql"]),
            ("crontab", vec!["{{x}}"]),
            ("at", vec!["{{x}}"]),
            ("bc", vec!["{{x}}"]),
            ("julia", vec!["{{x}}"]),
        ] {
            assert!(refused(cmd, &items), "{cmd} {items:?}");
        }
        for (cmd, items) in [
            ("make", vec!["{{target}}"]),
            ("make", vec!["-C", "dir", "{{target}}"]),
            ("npm", vec!["run", "build", "--", "{{x}}"]),
            ("npm", vec!["audit", "{{x}}"]),
            ("gdb", vec!["--batch", "-ex", "bt", "./prog", "{{core}}"]),
            ("tar", vec!["-xf", "{{archive}}"]),
            ("rsync", vec!["-av", "{{src}}", "dst"]),
            ("curl", vec!["-o", "out", "{{url}}"]),
            ("vim", vec!["--", "{{file}}"]),
            ("sqlite3", vec!["{{db}}", ".tables"]),
            ("mysql", vec!["-u{{user}}", "-e", "select 1", "db"]),
            ("psql", vec!["{{db}}", "-c", "select 1"]),
        ] {
            assert!(approvable(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn an_operand_rendered_as_an_option_is_refused_but_an_option_value_is_not() {
        let templates = line(&["{{target}}"]);
        assert!(rendered_refusal("s", "make", &templates, &line(&["--eval=x"]), true).is_some());
        assert!(rendered_refusal("s", "make", &templates, &line(&["build"]), true).is_none());
        let templates = line(&["-u{{user}}", "db"]);
        assert!(
            rendered_refusal("s", "mysql", &templates, &line(&["-ualice", "db"]), true).is_none()
        );
    }

    #[test]
    fn review_two_findings_are_refused() {
        for (finding, cmd, items) in [
            (
                "F1",
                "sudo",
                vec!["-Eu", "root", "bash", "-c", "echo \"{{x}}\""],
            ),
            (
                "F1",
                "xargs",
                vec!["-0I", "{}", "sh", "-c", "echo \"{{x}}\" {}"],
            ),
            (
                "F1",
                "env",
                vec!["-iu", "HOME", "bash", "-c", "echo \"{{x}}\""],
            ),
            ("F2", "env", vec!["-iS", "bash -c", "{{x}}"]),
            ("F2", "env", vec!["--split=bash -c {{x}}"]),
            (
                "F3",
                "docker",
                vec!["compose", "exec", "svc", "sh", "-c", "echo \"{{x}}\""],
            ),
            (
                "F3",
                "docker",
                vec![
                    "compose",
                    "-f",
                    "c.yml",
                    "run",
                    "--rm",
                    "svc",
                    "python3",
                    "-c",
                    "print('{{x}}')",
                ],
            ),
            (
                "F3",
                "docker",
                vec!["container", "run", "img", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F3", "docker", vec!["logs", "{{container}}"]),
            ("F4", "git", vec!["rebase", "--exec", "{{x}}", "main"]),
            ("F4", "git", vec!["rebase", "-x", "{{x}}"]),
            ("F4", "git", vec!["rebase", "--exe={{x}}", "main"]),
            (
                "F4",
                "git",
                vec!["fetch", "--upload-pack", "{{x}}", "origin"],
            ),
            ("F4", "git", vec!["clone", "-u", "{{x}}", "repo"]),
            ("F4", "git", vec!["archive", "--exec={{x}}", "HEAD"]),
            ("F4", "git", vec!["submodule", "foreach", "{{x}}"]),
            (
                "F4",
                "git",
                vec!["bisect", "run", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F4", "git", vec!["config", "alias.x", "{{x}}"]),
            (
                "F5",
                "kubectl",
                vec!["exec", "pod", "--", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "kubectl", vec!["exec", "{{pod}}", "--", "ls"]),
            ("F5", "watch", vec!["-n", "5", "{{x}}"]),
            ("F5", "flock", vec!["/tmp/lock", "-c", "{{x}}"]),
            (
                "F5",
                "flock",
                vec!["/tmp/lock", "bash", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "script", vec!["-c", "{{x}}", "out.log"]),
            ("F5", "su", vec!["deploy", "-c", "{{x}}"]),
            (
                "F5",
                "runuser",
                vec!["-u", "deploy", "--", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "runuser", vec!["deploy", "-c", "{{x}}"]),
            ("F5", "chroot", vec!["/srv", "sh", "-c", "echo \"{{x}}\""]),
            ("F5", "setsid", vec!["bash", "-c", "echo \"{{x}}\""]),
            ("F5", "unshare", vec!["-r", "sh", "-c", "echo \"{{x}}\""]),
            (
                "F5",
                "nsenter",
                vec!["-t", "1", "-m", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "taskset", vec!["0x1", "bash", "-c", "echo \"{{x}}\""]),
            (
                "F5",
                "ionice",
                vec!["-c", "3", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "chrt", vec!["10", "sh", "-c", "echo \"{{x}}\""]),
            (
                "F5",
                "strace",
                vec!["-f", "-o", "t.log", "sh", "-c", "echo \"{{x}}\""],
            ),
            ("F5", "ltrace", vec!["sh", "-c", "echo \"{{x}}\""]),
            (
                "F5",
                "valgrind",
                vec!["--tool=memcheck", "python3", "-c", "print('{{x}}')"],
            ),
            (
                "F5",
                "systemd-run",
                vec!["--user", "-p", "X=1", "sh", "-c", "echo \"{{x}}\""],
            ),
            (
                "F5",
                "parallel",
                vec!["-j", "2", "echo {{x}} {}", ":::", "a"],
            ),
            ("F5", "scp", vec!["-o", "{{x}}", "a", "host:b"]),
            ("F5", "scp", vec!["a", "host:{{path}}"]),
            ("F5", "sftp", vec!["-o", "{{x}}", "host"]),
            ("F5", "rbash", vec!["-c", "echo \"{{x}}\""]),
            ("F5", "oksh", vec!["-c", "echo \"{{x}}\""]),
            ("F5", "rc", vec!["-c", "echo \"{{x}}\""]),
            ("F6", "make", vec!["--ev={{x}}"]),
            ("F6", "tar", vec!["--to-com={{x}}", "-xf", "a.tar"]),
            ("F6", "rsync", vec!["--rsync-p={{x}}", "a", "b"]),
            ("F6", "psql", vec!["--comm={{x}}"]),
            ("F6", "gdb", vec!["./prog", "-eval-command", "{{x}}"]),
            ("F7", "tar", vec!["-cI", "{{x}}", "-f", "a.tar", "dir"]),
            ("F7", "tar", vec!["cIf", "{{x}}", "a.tar", "dir"]),
            ("F8", "make", vec!["--", "CFLAGS={{x}}"]),
            ("F9", "pip", vec!["install", "{{pkg}}"]),
            ("F9", "pip3", vec!["install", "-U", "{{pkg}}"]),
            ("F9", "pipx", vec!["run", "{{pkg}}"]),
            ("F9", "uvx", vec!["{{pkg}}"]),
            ("F9", "uv", vec!["run", "{{x}}"]),
            ("F9", "uv", vec!["tool", "run", "{{pkg}}"]),
            ("F9", "uv", vec!["pip", "install", "{{pkg}}"]),
            ("F9", "go", vec!["run", "{{pkg}}"]),
            ("F9", "cargo", vec!["install", "{{crate}}"]),
            ("F9", "gem", vec!["install", "{{gem}}"]),
            ("F9", "deno", vec!["run", "{{url}}"]),
            ("F9", "bun", vec!["x", "{{pkg}}"]),
            ("F10", "perl", vec!["-I", "lib.pl", "{{x}}"]),
        ] {
            assert!(refused(cmd, &items), "{finding} {cmd} {items:?}");
        }
        assert!(matches!(
            first_unsafe_placeholder("deno", &line(&["eval", "console.log('{{x}}')"])),
            Some(InlineFinding::Untrusted(_))
        ));
    }

    #[test]
    fn review_two_data_positions_stay_accepted() {
        for (cmd, items) in [
            ("sudo", vec!["-Eu", "root", "make", "{{target}}"]),
            (
                "env",
                vec![
                    "-i",
                    "A=1",
                    "python3",
                    "-c",
                    "import sys; print(sys.argv[1])",
                    "{{x}}",
                ],
            ),
            ("docker", vec!["compose", "exec", "svc", "echo", "{{x}}"]),
            ("git", vec!["rebase", "--exec", "make test", "{{branch}}"]),
            ("git", vec!["commit", "-m", "{{message}}"]),
            ("kubectl", vec!["get", "pods", "{{name}}"]),
            ("kubectl", vec!["exec", "pod", "--", "echo", "{{x}}"]),
            ("flock", vec!["/tmp/lock", "make", "{{target}}"]),
            ("parallel", vec!["echo {}", ":::", "{{x}}"]),
            ("make", vec!["{{target}}"]),
            ("tar", vec!["xf", "{{archive}}"]),
            ("pip", vec!["list"]),
            ("cargo", vec!["test", "{{filter}}"]),
            ("deno", vec!["run", "app.ts", "{{x}}"]),
            ("perl", vec!["script.pl", "{{x}}"]),
        ] {
            assert!(approvable(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn a_make_operand_rendered_as_an_assignment_is_refused_at_run_time() {
        let templates = line(&["--", "{{target}}"]);
        assert!(rendered_refusal(
            "s",
            "make",
            &templates,
            &line(&["--", "SHELL=/tmp/x"]),
            true
        )
        .is_some());
        assert!(rendered_refusal("s", "make", &templates, &line(&["--", "build"]), true).is_none());
    }

    #[test]
    fn an_unmodelled_program_needs_a_human_approval() {
        use crate::core::inline_code::first_unsafe_placeholder_with;
        for (cmd, items, program) in [
            ("terraform", vec!["plan", "-var", "x={{x}}"], "terraform"),
            ("npx", vec!["prettier", "--check", "{{file}}"], "prettier"),
            ("env", vec!["A=1", "aws", "s3", "ls", "{{bucket}}"], "aws"),
            ("find", vec![".", "-exec", "mytool", "{{x}}", ";"], "mytool"),
            ("/opt/bin/mytool", vec!["{{x}}"], "/opt/bin/mytool"),
        ] {
            let items = line(&items);
            assert!(
                matches!(
                    first_unsafe_placeholder(cmd, &items),
                    Some(InlineFinding::UnmodelledProgram(ref p, _)) if p == program
                ),
                "{cmd} {items:?}"
            );
            assert_eq!(
                first_unsafe_placeholder_with(cmd, &items, true),
                None,
                "{cmd} {items:?}"
            );
        }
        // No value, nothing to approve; a data-only program needs no approval.
        assert_eq!(
            first_unsafe_placeholder("terraform", &line(&["plan"])),
            None
        );
        assert_eq!(first_unsafe_placeholder("echo", &line(&["{{x}}"])), None);
        // The approval never covers code or option positions.
        assert!(
            first_unsafe_placeholder_with("bash", &line(&["-c", "echo {{x}}"]), true).is_some()
        );
        // At run time too.
        let templates = line(&["plan", "{{x}}"]);
        let rendered = line(&["plan", "value"]);
        assert!(rendered_refusal("s", "terraform", &templates, &rendered, false).is_some());
        assert!(rendered_refusal("s", "terraform", &templates, &rendered, true).is_none());
    }

    /// Each program left out of `DATA_ONLY_PROGRAMS` for an option that runs
    /// a command, a script or a program: it is not data-only, and a value
    /// given to that option is refused.
    #[test]
    fn the_data_only_list_has_no_command_option() {
        for (program, option) in [
            ("sort", "--compress-program"),
            ("zip", "-TT"),
            ("rg", "--pre"),
            ("less", "+!"),
            ("hostname", "-F"),
            ("diff3", "--diff-program"),
            ("sdiff", "--diff-program"),
            ("tar", "--to-command"),
            ("find", "-exec"),
            ("sed", "-e"),
            ("awk", "-e"),
            ("xargs", "-I"),
            ("git", "-c"),
            ("env", "-S"),
        ] {
            assert!(!DATA_ONLY_PROGRAMS.contains(&program), "{program}");
            assert!(refused(program, &[option, "{{x}}"]), "{program} {option}");
        }
        for program in DATA_ONLY_PROGRAMS {
            assert!(is_data_only(program), "{program}");
            // printf's format is a small language: a value goes after it.
            let line: &[&str] = if *program == "printf" {
                &["%s", "{{x}}"]
            } else {
                &["{{x}}"]
            };
            assert!(!refused(program, line), "{program}");
        }
    }

    /// A templated option argument keeps its value attached only to a known
    /// value-taking option the author wrote; `-{{x}}`, `--{{x}}` or a flag
    /// cluster (`-v{{x}}`) lets the rendered text add options.
    #[test]
    fn a_templated_option_is_data_only_when_its_value_stays_attached() {
        for (cmd, items) in [
            ("git", vec!["rebase", "-{{x}}"]),
            ("git", vec!["fetch", "--{{opt}}"]),
            ("git", vec!["rebase", "-v{{x}}"]),
            ("git", vec!["log", "-{{x}}"]),
            ("tar", vec!["-{{x}}", "a.tar"]),
            ("tar", vec!["-v{{x}}", "a.tar"]),
            ("rsync", vec!["-a{{x}}", "src", "dst"]),
            ("mysql", vec!["-{{x}}"]),
            ("mysql", vec!["-B{{x}}"]),
            ("psql", vec!["--{{x}}"]),
            ("gdb", vec!["-{{x}}"]),
            ("vim", vec!["-{{x}}"]),
            ("sqlite3", vec!["-{{x}}", "db"]),
            ("make", vec!["-{{x}}"]),
            ("date", vec!["-{{x}}"]),
            ("date", vec!["--{{x}}"]),
            ("seq", vec!["-w{{x}}", "3"]),
            ("cp", vec!["-S{{x}}", "a", "b"]),
        ] {
            assert!(!approvable(cmd, &items) || cmd == "cp", "{cmd} {items:?}");
            assert!(refused(cmd, &items), "{cmd} {items:?}");
        }
        for (cmd, items) in [
            ("git", vec!["commit", "-m{{msg}}"]),
            ("git", vec!["log", "-n{{count}}"]),
            ("git", vec!["log", "--author={{who}}"]),
            ("tar", vec!["-f{{archive}}", "-x"]),
            ("mysql", vec!["-u{{user}}", "db"]),
            ("psql", vec!["-U{{user}}", "-d{{db}}"]),
            ("curl", vec!["-H{{header}}", "https://example.org"]),
        ] {
            assert!(refused(cmd, &items), "{cmd} {items:?}");
            assert!(approvable(cmd, &items), "{cmd} {items:?}");
        }
        for (cmd, items) in [
            ("seq", vec!["-f{{fmt}}", "3"]),
            ("seq", vec!["--separator={{sep}}", "3"]),
            ("date", vec!["-d{{when}}"]),
            ("basename", vec!["-s{{suffix}}", "a.txt"]),
            ("basename", vec!["--", "-{{x}}"]),
        ] {
            assert!(!refused(cmd, &items), "{cmd} {items:?}");
        }
        // Only the listed long options keep a value attached.
        assert!(refused("date", &["--set={{when}}"]));
        assert!(refused("date", &["--reference={{file}}"]));
        // An unmodelled program stays default-deny, option-shaped or not.
        assert!(refused("terraform", &["-{{x}}"]));
        assert!(refused("terraform", &["--var={{x}}"]));
        // A known value option rendered without a value would take the next
        // argument: refused at run time.
        let templates = line(&["-u{{user}}", "-e", "select 1"]);
        let empty = line(&["-u", "-e", "select 1"]);
        assert!(rendered_refusal("s", "mysql", &templates, &empty, true).is_some());
        let named = line(&["-uÉquipe 🦀", "-e", "select 1"]);
        assert!(rendered_refusal("s", "mysql", &templates, &named, true).is_none());
    }

    /// sqlite3 reads `--cmd` and `--init` like `-cmd` and `-init`.
    #[test]
    fn sqlite_double_dash_code_options_are_code() {
        for option in ["-cmd", "--cmd", "-init", "--init"] {
            assert!(refused("sqlite3", &[option, "{{x}}", "db"]), "{option}");
            assert!(
                refused("sqlite3", &[&format!("{option}={{{{x}}}}"), "db"]),
                "{option}="
            );
        }
        assert!(!refused("sqlite3", &["db", "--cmd", ".mode csv"]));
    }

    /// BSD `script file command...` runs the operands after the file.
    #[test]
    fn script_operands_after_the_file_are_a_command() {
        assert!(refused("script", &["-q", "out.log", "{{x}}"]));
        assert!(refused("script", &["-q", "out.log", "bash", "-c", "{{x}}"]));
        assert!(approvable("script", &["-q", "{{log}}"]));
        let templates = line(&["-q", "{{log}}"]);
        assert!(rendered_refusal("s", "script", &templates, &line(&["-q", "-c"]), true).is_some());
    }

    /// jq reads its program from `-f` and modules from `-L`.
    #[test]
    fn jq_program_and_module_paths_are_code() {
        for items in [
            vec!["-f", "{{x}}"],
            vec!["--from-file", "{{x}}"],
            vec!["--from-file={{x}}"],
            vec!["-f{{x}}"],
            vec!["-rf", "{{x}}"],
            vec!["-L", "{{x}}", "."],
            vec!["-L{{x}}", "."],
            vec!["--library-path={{x}}", "."],
        ] {
            assert!(refused("jq", &items), "{items:?}");
        }
        // jq reads paths: a value in a data position needs a human.
        for items in [
            vec!["-r", ".name", "{{file}}"],
            vec!["--arg", "name", "{{value}}", ".x"],
        ] {
            assert!(refused("jq", &items), "{items:?}");
            assert!(approvable("jq", &items), "{items:?}");
        }
    }

    /// A data-only program still reads options: a value rendering to one
    /// (`rm {{x}}` as `-rf`) is refused at run time, unless a literal `--`
    /// precedes it.
    #[test]
    fn a_data_only_operand_rendering_to_an_option_is_refused_at_run_time() {
        let tainted = |items: &[&str]| -> Vec<String> { line(items) };
        for program in ["basename", "dirname", "seq", "date", "/bin/date"] {
            let templates = tainted(&["{{x}}", "target"]);
            let refusal = |rendered: &[&str]| {
                rendered_refusal("s", program, &templates, &line(rendered), false)
            };
            assert!(refusal(&["-rf", "target"]).is_some(), "{program} -rf");
            assert!(
                refusal(&["--no-preserve-root", "target"]).is_some(),
                "{program} --"
            );
            assert!(
                refusal(&["Équipe 🦀", "target"]).is_none(),
                "{program} plain value"
            );

            let templates = tainted(&["-v", "{{x}}", "{{y}}"]);
            let rendered = line(&["-v", "a", "-rf"]);
            assert!(
                rendered_refusal("s", program, &templates, &rendered, false).is_some(),
                "{program}: every value before `--` is checked"
            );

            let templates = tainted(&["--", "{{x}}"]);
            let rendered = line(&["--", "-rf"]);
            assert!(
                rendered_refusal("s", program, &templates, &rendered, false).is_none(),
                "{program}: a literal `--` ends options"
            );
        }
        // A value rendering to `--` does not end options for the next one.
        let templates = line(&["{{x}}", "{{y}}"]);
        assert!(rendered_refusal("s", "seq", &templates, &line(&["--", "-rf"]), false).is_some());
        // `echo` and `test` read no option a value could turn on.
        for program in ["echo", "test"] {
            let templates = line(&["{{x}}"]);
            assert!(rendered_refusal("s", program, &templates, &line(&["-n"]), false).is_none());
        }
    }
}
