//! The role of each argument of an Exec command line: plain data, program
//! text, an option the program still parses, or the program to run.
//!
//! A template value is safe only as data. This module knows how the
//! programs that run other programs or evaluate program text read their
//! argv: interpreters (through `inline_code`), wrappers that launch another
//! command (`env`, `sudo`, `timeout`, `xargs`…), and other evaluators
//! (`awk`, `sed`, `find -exec`, `git -c`, `ssh`, `docker run`, `npx`…).
//! An unknown program gets every argument as data: argv is not parsed by a
//! shell, so a value cannot become code there.

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

/// The role of every argument of `cmd args`. `tainted` marks arguments that
/// carry an outside value: they never end option parsing or name a program,
/// since their rendered text is not known (or not trusted).
pub fn roles(cmd: &str, args: &[String], tainted: &[bool]) -> Vec<Role> {
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
        "npx" | "bunx" => npx_roles(args, tainted, depth),
        "npm" | "pnpm" | "yarn" => package_manager_roles(&name, args, tainted, depth),
        _ if UNMODELLED_EVALUATORS.contains(&name.as_str()) => vec![Role::Option; args.len()],
        _ => match flag_spec(&name) {
            Some(spec) => flag_roles(args, tainted, &spec),
            None => vec![Role::Data; args.len()],
        },
    }
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
}

fn wrapper(name: &str) -> Option<Wrapper> {
    let spec = |value_options, leading_positionals, assignments| Wrapper {
        value_options,
        command_line_options: &[],
        leading_positionals,
        assignments,
    };
    Some(match name {
        "env" => Wrapper {
            value_options: &["-u", "--unset", "-C", "--chdir"],
            command_line_options: &["-S", "--split-string"],
            leading_positionals: 0,
            assignments: true,
        },
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
            let name = long_name(arg);
            if spec.command_line_options.contains(&name) {
                roles[i] = Role::Code;
                if !arg.contains('=') && i + 1 < args.len() {
                    roles[i + 1] = Role::Code;
                }
                // The rest is part of that command line too.
                for role in roles.iter_mut().skip(i) {
                    *role = Role::Code;
                }
                return roles;
            }
            if spec.value_options.contains(&name) && !arg.contains('=') {
                i += 2;
            } else {
                i += 1;
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
    if i < args.len() {
        roles[i] = Role::Executable;
        let inner = roles_at(&args[i], &args[i + 1..], &tainted[i + 1..], depth + 1);
        roles[i + 1..].copy_from_slice(&inner);
    }
    roles
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
    let mut i = i + 1;
    while i < args.len() {
        let arg = args[i].as_str();
        if !is_tainted(tainted, i) && arg == "--" {
            roles[i + 1..].fill(Role::Data);
            return roles;
        }
        roles[i] = Role::RuntimeOption;
        if !is_tainted(tainted, i) && SUBCOMMAND_VALUES.contains(&arg) && i + 1 < args.len() {
            roles[i + 1] = Role::Data;
            i += 2;
            continue;
        }
        i += 1;
    }
    roles
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
    let subcommand = args.get(i).map(String::as_str).unwrap_or_default();
    if !matches!(subcommand, "run" | "create" | "exec") || is_tainted(tainted, i) {
        for (j, role) in roles.iter_mut().enumerate().skip(i + 1) {
            *role = if is_tainted(tainted, j) {
                Role::RuntimeOption
            } else {
                Role::Data
            };
        }
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
                }) || (!arg.starts_with("--")
                    && cluster
                        .chars()
                        .next()
                        .is_some_and(|c| spec.short_code.contains(&c)));
                roles[i] = if code { Role::Code } else { Role::Data };
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
            if spec.code.contains(&name) {
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
    };
    Some(match name {
        "make" | "gmake" | "bmake" => FlagSpec {
            code: &["--eval", "-E"],
            short_code: &[],
            plus_is_code: false,
            assignments_are_code: true,
            operands: Operands::All(Role::RuntimeOption),
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
            ],
            &[],
            Operands::ProgramThenData,
        ),
        "tar" | "gtar" | "bsdtar" => spec(
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
            &[],
            Operands::All(Role::RuntimeOption),
        ),
        "rsync" => spec(
            &["--rsh", "--rsync-path"],
            &['e'],
            Operands::All(Role::RuntimeOption),
        ),
        "curl" => spec(&["-K", "--config"], &[], Operands::All(Role::RuntimeOption)),
        "vim" | "vi" | "nvim" | "ex" | "view" | "gvim" | "vimdiff" => FlagSpec {
            code: &["-c", "--cmd", "-S", "-u", "-U", "-s", "-w", "-W"],
            short_code: &[],
            plus_is_code: true,
            assignments_are_code: false,
            operands: Operands::All(Role::Option),
        },
        "sqlite" => spec(
            &["-cmd", "-init"],
            &[],
            Operands::FirstThenCode(Role::RuntimeOption),
        ),
        "mysql" | "mariadb" => spec(
            &["--execute", "--init-command", "--init-command-add"],
            &['e'],
            Operands::All(Role::RuntimeOption),
        ),
        "psql" => spec(
            &["--command", "--file", "--set", "--variable"],
            &['c', 'f', 'v'],
            Operands::All(Role::RuntimeOption),
        ),
        _ => return None,
    })
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
            assert!(!refused(cmd, &items), "{cmd} {items:?}");
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
            ("npx", vec!["prettier", "--check", "{{file}}"]),
            ("mytool", vec!["{{x}}", "--flag", "{{y}}"]),
        ] {
            assert!(!refused(cmd, &items), "{cmd} {items:?}");
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
        // An unknown program keeps its arguments as data.
        assert_eq!(
            first_unsafe_placeholder("mytool", &line(&["{{x}}", "-o", "{{y}}"])),
            None
        );
    }

    #[test]
    fn an_option_like_rendered_operand_is_refused_at_run_time() {
        let templates = line(&["fetch", "{{remote}}"]);
        assert!(rendered_refusal(
            "s",
            "git",
            &templates,
            &line(&["fetch", "--upload-pack=touch x"])
        )
        .is_some());
        assert!(rendered_refusal("s", "git", &templates, &line(&["fetch", "origin"])).is_none());
        let templates = line(&["log", "--", "{{path}}"]);
        assert!(rendered_refusal("s", "git", &templates, &line(&["log", "--", "-x"])).is_none());
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
            assert!(!refused(cmd, &items), "{cmd} {items:?}");
        }
    }

    #[test]
    fn an_operand_rendered_as_an_option_is_refused_but_an_option_value_is_not() {
        let templates = line(&["{{target}}"]);
        assert!(rendered_refusal("s", "make", &templates, &line(&["--eval=x"])).is_some());
        assert!(rendered_refusal("s", "make", &templates, &line(&["build"])).is_none());
        let templates = line(&["-u{{user}}", "db"]);
        assert!(rendered_refusal("s", "mysql", &templates, &line(&["-ualice", "db"])).is_none());
    }
}
