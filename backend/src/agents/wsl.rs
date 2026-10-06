//! Windows-native Kronn starting an agent installed in WSL: what has to change
//! at the Windows/Linux boundary — paths, environment and the backend URL.
//!
//! Everything here is pure (no platform `cfg`) so the Linux and macOS test
//! runs cover it; only the decision to cross into WSL depends on the host.

use std::path::{Path, PathBuf};

/// Override for the backend URL given to an agent running in WSL. Needed in
/// WSL2's default NAT networking, where `127.0.0.1` is the Linux VM itself.
pub(crate) const WSL_BACKEND_URL_ENV: &str = "KRONN_WSL_BACKEND_URL";

/// Never forwarded: WSL keeps its own home, shell and `PATH`, and `WSLENV` is
/// the list itself.
const NEVER_FORWARDED: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "COPILOT_HOME",
    "SHELL",
    "PATH",
    "WSLENV",
];

/// Path values only the Linux side needs: translated (`/p`) and forwarded only
/// into WSL (`/u`), so a Windows program started from WSL keeps its own.
const LINUX_SIDE_PATHS: &[&str] = &["TMPDIR", "TEMP", "TMP"];

/// Whether a launch crosses into WSL: Windows-native Kronn starting a binary
/// that was found inside WSL (or is a Linux absolute path).
pub(crate) fn runs_in_wsl(windows_native: bool, command: &str, resolved_via_wsl: bool) -> bool {
    windows_native && (resolved_via_wsl || command.starts_with('/'))
}

/// `wsl.exe --cd <linux dir> -e <command> <args…>`, with every Windows path
/// Kronn put in the arguments translated for the Linux side.
pub(crate) fn wrap_invocation(
    command: String,
    args: Vec<String>,
    work_dir: &Path,
) -> (String, Vec<String>, PathBuf) {
    let work_dir_text = work_dir.to_string_lossy();
    let linux_work_dir =
        windows_path_to_wsl(&work_dir_text).unwrap_or_else(|| work_dir_text.into_owned());
    let mut wsl_args = vec![
        "--cd".to_string(),
        linux_work_dir,
        "-e".to_string(),
        command,
    ];
    wsl_args.extend(translate_args(args));
    ("wsl.exe".to_string(), wsl_args, work_dir.to_path_buf())
}

/// The WSL path of a Windows absolute path, or `None` when `value` is not one.
///
/// Drive paths (`C:\x`, `C:/x`, `\\?\C:\x`) map to `/mnt/<drive>/x`; paths of
/// a WSL distribution seen from Windows (`\\wsl$\Ubuntu\home\x`,
/// `\\wsl.localhost\Ubuntu\home\x`) map back to `/home/x`. Other UNC shares
/// have no WSL equivalent and are left alone.
pub(crate) fn windows_path_to_wsl(value: &str) -> Option<String> {
    let value = value.strip_prefix(r"\\?\").unwrap_or(value);
    if let Some(unc) = value.strip_prefix(r"\\") {
        let mut parts = unc.splitn(3, ['\\', '/']);
        let host = parts.next()?;
        if !(host.eq_ignore_ascii_case("wsl$") || host.eq_ignore_ascii_case("wsl.localhost")) {
            return None;
        }
        let _distro = parts.next().filter(|distro| !distro.is_empty())?;
        let rest = parts.next().unwrap_or("").replace('\\', "/");
        return Some(format!("/{}", rest.trim_start_matches('/')));
    }
    let mut chars = value.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    // Both checked characters are ASCII, so byte 2 is a char boundary.
    let rest = &value[2..];
    if !(rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/')) {
        // `C:foo` is relative to the drive's current directory: not translatable.
        return None;
    }
    let rest = rest.replace('\\', "/");
    let rest = if rest.is_empty() { "/".into() } else { rest };
    Some(format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest))
}

/// Arguments with Kronn's Windows paths translated: whole path arguments
/// (`--add-dir C:\repo`), paths inside the inline JSON of `--mcp-config` and
/// `--settings` (bridge command, sandbox roots) and inside Codex `-c` TOML
/// overrides (bridge command, trusted project keys).
pub(crate) fn translate_args(args: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    for arg in args {
        let translated = match out.last().map(String::as_str) {
            Some("--mcp-config" | "--settings") => translate_json_document(&arg),
            Some("-c" | "--config") => translate_toml_override(&arg),
            _ => None,
        }
        .or_else(|| windows_path_to_wsl(&arg));
        out.push(translated.unwrap_or(arg));
    }
    out
}

/// The document re-serialized with its path strings translated, or `None`
/// when it is not JSON or holds no Windows path (left byte-identical).
fn translate_json_document(document: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(document).ok()?;
    if !(value.is_object() || value.is_array()) || !translate_json_value(&mut value) {
        return None;
    }
    serde_json::to_string(&value).ok()
}

fn translate_json_value(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => match windows_path_to_wsl(text) {
            Some(translated) => {
                *text = translated;
                true
            }
            None => false,
        },
        // Every item is visited: `any` would stop at the first translated one.
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= translate_json_value(item);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for item in map.values_mut() {
                changed |= translate_json_value(item);
            }
            changed
        }
        _ => false,
    }
}

/// `key=<toml value>` with path strings and path table keys translated, or
/// `None` when nothing changes (the override is then passed as written).
fn translate_toml_override(arg: &str) -> Option<String> {
    let (key, raw) = arg.split_once('=')?;
    let mut table: toml::Table = toml::from_str(&format!("v={raw}")).ok()?;
    let mut value = table.remove("v")?;
    if !translate_toml_value(&mut value) {
        return None;
    }
    Some(format!("{key}={value}"))
}

fn translate_toml_value(value: &mut toml::Value) -> bool {
    match value {
        toml::Value::String(text) => match windows_path_to_wsl(text) {
            Some(translated) => {
                *text = translated;
                true
            }
            None => false,
        },
        toml::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= translate_toml_value(item);
            }
            changed
        }
        toml::Value::Table(table) => {
            let mut changed = false;
            let entries = std::mem::take(table);
            for (key, mut item) in entries {
                changed |= translate_toml_value(&mut item);
                let key = match windows_path_to_wsl(&key) {
                    Some(translated) => {
                        changed = true;
                        translated
                    }
                    None => key,
                };
                table.insert(key, item);
            }
            changed
        }
        _ => false,
    }
}

/// The `WSLENV` that forwards `names` into WSL on top of `existing`, or
/// `None` when there is nothing to add.
///
/// `wsl.exe` passes a Windows variable to Linux only when `WSLENV` lists it.
/// Names only, never values. A name the user already lists keeps the user's
/// flags. Without `/u`, a variable also flows back to Windows programs the
/// agent starts through interop — the bundled `kronn-internal.exe` bridge
/// needs the discussion id, URL and token that way.
pub(crate) fn wslenv_value<'a>(
    existing: Option<&str>,
    names: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let existing = existing.unwrap_or("").trim_matches(':');
    let mut listed: Vec<String> = existing
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.split('/').next().unwrap_or("").to_ascii_uppercase())
        .collect();
    let mut added: Vec<String> = Vec::new();
    for name in names {
        let forwardable = !name.is_empty()
            && !name.contains([':', '/', '='])
            && !NEVER_FORWARDED
                .iter()
                .any(|never| never.eq_ignore_ascii_case(name));
        if !forwardable || listed.contains(&name.to_ascii_uppercase()) {
            continue;
        }
        listed.push(name.to_ascii_uppercase());
        let linux_side_path = LINUX_SIDE_PATHS
            .iter()
            .any(|path| path.eq_ignore_ascii_case(name));
        added.push(if linux_side_path {
            format!("{name}/up")
        } else {
            name.to_string()
        });
    }
    if added.is_empty() {
        return None;
    }
    let added = added.join(":");
    Some(if existing.is_empty() {
        added
    } else {
        format!("{existing}:{added}")
    })
}

/// Set `WSLENV` on a launch that crosses into WSL: every variable the launch's
/// built environment carries that `forward` accepts. Nothing is inherited past
/// the built environment, so this list is complete.
pub(crate) fn apply_wslenv(command: &mut tokio::process::Command, forward: impl Fn(&str) -> bool) {
    let std_command = command.as_std();
    let mut set: Vec<String> = Vec::new();
    let mut existing: Option<String> = None;
    for (key, value) in std_command.get_envs() {
        let key = key.to_string_lossy().into_owned();
        match value {
            Some(value) if key.eq_ignore_ascii_case("WSLENV") => {
                existing = Some(value.to_string_lossy().into_owned());
            }
            Some(_) if forward(&key) => set.push(key),
            _ => {}
        }
    }
    let existing = existing.or_else(|| crate::core::child_env::var("WSLENV").ok());
    if let Some(value) = wslenv_value(existing.as_deref(), set.iter().map(String::as_str)) {
        command.env("WSLENV", value);
    }
}

/// The backend URL given to an agent: the operator's WSL override when the
/// agent runs in WSL and one is set, otherwise the usual one.
pub(crate) fn agent_backend_url(
    runs_in_wsl: bool,
    base: Option<&str>,
    wsl_override: Option<&str>,
) -> String {
    let wsl_override = wsl_override.map(str::trim).filter(|url| !url.is_empty());
    match (runs_in_wsl, wsl_override) {
        (true, Some(url)) => url.to_string(),
        _ => base
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .unwrap_or("http://127.0.0.1:3140")
            .to_string(),
    }
}

/// `networkingMode` from the `[wsl2]` section of a `.wslconfig`, lowercased.
pub(crate) fn wslconfig_networking_mode(content: &str) -> Option<String> {
    let mut in_wsl2 = false;
    let mut mode = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_wsl2 = section.trim().eq_ignore_ascii_case("wsl2");
            continue;
        }
        if !in_wsl2 {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("networkingMode") {
                let value = value.split(['#', ';']).next().unwrap_or("");
                mode = Some(value.trim().trim_matches('"').to_ascii_lowercase());
            }
        }
    }
    mode.filter(|mode| !mode.is_empty())
}

/// True when a Linux-side bridge would get `127.0.0.1` in NAT mode, where
/// it does not reach Windows. WSL2 defaults to NAT when nothing is set.
pub(crate) fn backend_unreachable_warning_needed(
    override_set: bool,
    networking_mode: Option<&str>,
) -> bool {
    !override_set && networking_mode != Some("mirrored")
}

/// Log once per process when a WSL agent probably cannot reach the backend.
pub(crate) fn warn_once_if_backend_unreachable(override_set: bool) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        let mode = crate::core::child_env::var_os("USERPROFILE")
            .map(|home| Path::new(&home).join(".wslconfig"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|content| wslconfig_networking_mode(&content));
        if backend_unreachable_warning_needed(override_set, mode.as_deref()) {
            tracing::warn!(
                "An agent runs in WSL and WSL networking is not mirrored: a Linux-side \
                 kronn-internal bridge cannot reach 127.0.0.1 on Windows. Set \
                 networkingMode=mirrored in %USERPROFILE%\\.wslconfig, or set {} to a URL \
                 WSL can reach. See docs/operations/windows-wsl-agents.md.",
                WSL_BACKEND_URL_ENV
            );
        }
    });
}

/// Why a native ACP agent found only inside WSL is refused on Windows.
pub(crate) fn native_acp_wsl_refusal(program: &str) -> String {
    format!(
        "`{program}` is installed only inside WSL. Kronn on Windows starts this agent's ACP \
         runtime as a Windows program, and does not route it through WSL. Install `{program}` \
         on Windows, or run Kronn inside WSL. See docs/operations/windows-wsl-agents.md."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths_map_to_mnt() {
        assert_eq!(
            windows_path_to_wsl(r"C:\Users\Romu\proj").as_deref(),
            Some("/mnt/c/Users/Romu/proj")
        );
        assert_eq!(
            windows_path_to_wsl("D:/work/a b/x.json").as_deref(),
            Some("/mnt/d/work/a b/x.json")
        );
        assert_eq!(
            windows_path_to_wsl(r"\\?\E:\long\path").as_deref(),
            Some("/mnt/e/long/path")
        );
        assert_eq!(windows_path_to_wsl(r"C:\").as_deref(), Some("/mnt/c/"));
        assert_eq!(windows_path_to_wsl("C:").as_deref(), Some("/mnt/c/"));
        // Mixed separators, as `Path::join(".kronn/tmp")` produces on Windows.
        assert_eq!(
            windows_path_to_wsl(r"C:\proj\.kronn/tmp").as_deref(),
            Some("/mnt/c/proj/.kronn/tmp")
        );
    }

    #[test]
    fn unicode_paths_translate_without_panicking() {
        assert_eq!(
            windows_path_to_wsl(r"C:\Users\Rémi\données\🚀").as_deref(),
            Some("/mnt/c/Users/Rémi/données/🚀")
        );
        assert_eq!(windows_path_to_wsl("é:"), None);
        assert_eq!(windows_path_to_wsl("🚀"), None);
    }

    #[test]
    fn wsl_distribution_paths_map_back_to_linux() {
        assert_eq!(
            windows_path_to_wsl(r"\\wsl.localhost\Ubuntu\home\romu\proj").as_deref(),
            Some("/home/romu/proj")
        );
        assert_eq!(
            windows_path_to_wsl(r"\\wsl$\Debian\srv").as_deref(),
            Some("/srv")
        );
        assert_eq!(windows_path_to_wsl(r"\\WSL$\Debian").as_deref(), Some("/"));
    }

    #[test]
    fn non_windows_paths_are_not_translated() {
        for value in [
            "",
            "python3",
            "/usr/bin/claude",
            "stream-json",
            r"\\server\share\x",
            "C:relative",
            "1:\\x",
            "--mcp-config",
            "https://example.com",
            r"\\wsl$",
        ] {
            assert_eq!(windows_path_to_wsl(value), None, "{value:?}");
        }
    }

    #[test]
    fn inline_mcp_config_bridge_paths_are_translated() {
        let config = serde_json::json!({
            "mcpServers": {
                "kronn-internal": {
                    "command": "python3",
                    "args": [r"C:\Program Files\Kronn\scripts\disc-introspection-mcp.py"],
                    "env": {"KRONN_BACKEND_URL": "http://127.0.0.1:3140"}
                },
                "bundled": {"command": r"C:\Program Files\Kronn\kronn-internal.exe", "args": []},
                "npx-one": {"command": "npx", "args": ["-y", "pkg"], "env": {"TOKEN": "${TOKEN}"}}
            }
        })
        .to_string();
        let args = translate_args(vec![
            "--print".into(),
            "--mcp-config".into(),
            config,
            "--strict-mcp-config".into(),
        ]);
        assert_eq!(args[1], "--mcp-config");
        let translated: serde_json::Value = serde_json::from_str(&args[2]).unwrap();
        let servers = &translated["mcpServers"];
        assert_eq!(servers["kronn-internal"]["command"], "python3");
        assert_eq!(
            servers["kronn-internal"]["args"][0],
            "/mnt/c/Program Files/Kronn/scripts/disc-introspection-mcp.py"
        );
        assert_eq!(
            servers["bundled"]["command"],
            "/mnt/c/Program Files/Kronn/kronn-internal.exe"
        );
        assert_eq!(servers["npx-one"]["env"]["TOKEN"], "${TOKEN}");
        assert_eq!(
            servers["kronn-internal"]["env"]["KRONN_BACKEND_URL"],
            "http://127.0.0.1:3140"
        );
        assert_eq!(args[3], "--strict-mcp-config");
    }

    #[test]
    fn mcp_config_path_and_whole_path_arguments_are_translated() {
        let args = translate_args(vec![
            "--mcp-config".into(),
            r"C:\Users\Romu\proj\.mcp.json".into(),
            "--add-dir".into(),
            r"D:\repos\docs".into(),
            "a prompt that mentions C:\\nothing as text".into(),
        ]);
        assert_eq!(args[1], "/mnt/c/Users/Romu/proj/.mcp.json");
        assert_eq!(args[3], "/mnt/d/repos/docs");
        assert_eq!(args[4], "a prompt that mentions C:\\nothing as text");
    }

    #[test]
    fn json_without_windows_paths_is_passed_byte_identical() {
        let config = r#"{"mcpServers": {"x": {"command": "npx"}}}"#.to_string();
        let args = translate_args(vec!["--mcp-config".into(), config.clone()]);
        assert_eq!(args[1], config);
    }

    #[test]
    fn sandbox_settings_roots_are_translated() {
        let settings = serde_json::json!({
            "sandbox": {"failIfUnavailable": true, "filesystem": {"allowWrite": [r"C:\wt\task", r"D:\cache"]}}
        })
        .to_string();
        let args = translate_args(vec!["--settings".into(), settings]);
        let parsed: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        let roots = &parsed["sandbox"]["filesystem"]["allowWrite"];
        assert_eq!(roots[0], "/mnt/c/wt/task");
        assert_eq!(roots[1], "/mnt/d/cache");
        assert_eq!(parsed["sandbox"]["failIfUnavailable"], true);
    }

    #[test]
    fn codex_toml_overrides_translate_bridge_and_trusted_projects() {
        let bridge = format!(
            "mcp_servers={{\"kronn-internal\"={{command={},args={},env_vars=[\"KRONN_DISCUSSION_ID\"]}}}}",
            serde_json::to_string(r"C:\Kronn\kronn-internal.exe").unwrap(),
            serde_json::to_string(&Vec::<String>::new()).unwrap()
        );
        let mut projects = toml::Table::new();
        projects.insert(
            r"C:\proj".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "trust_level".into(),
                toml::Value::String("trusted".into()),
            )])),
        );
        let trust = format!("projects={}", toml::Value::Table(projects));
        let args = translate_args(vec![
            "exec".into(),
            "-c".into(),
            trust,
            "-c".into(),
            bridge,
            "-c".into(),
            "model_reasoning_effort=\"high\"".into(),
        ]);

        let parse = |arg: &str| -> toml::Table { toml::from_str(arg).unwrap() };
        let trust = parse(&args[2]);
        assert_eq!(
            trust["projects"]["/mnt/c/proj"]["trust_level"].as_str(),
            Some("trusted")
        );
        let bridge = parse(&args[4]);
        let internal = &bridge["mcp_servers"]["kronn-internal"];
        assert_eq!(
            internal["command"].as_str(),
            Some("/mnt/c/Kronn/kronn-internal.exe")
        );
        assert_eq!(
            internal["env_vars"].as_array().unwrap()[0].as_str(),
            Some("KRONN_DISCUSSION_ID")
        );
        assert_eq!(args[6], "model_reasoning_effort=\"high\"");
    }

    #[test]
    fn wrap_invocation_translates_cwd_and_args() {
        let (program, args, cwd) = wrap_invocation(
            "/home/romu/.local/bin/claude".into(),
            vec!["--add-dir".into(), r"C:\other".into()],
            Path::new(r"C:\Users\Romu\proj"),
        );
        assert_eq!(program, "wsl.exe");
        assert_eq!(
            args,
            vec![
                "--cd",
                "/mnt/c/Users/Romu/proj",
                "-e",
                "/home/romu/.local/bin/claude",
                "--add-dir",
                "/mnt/c/other"
            ]
        );
        assert_eq!(cwd, Path::new(r"C:\Users\Romu\proj"));
    }

    #[test]
    fn runs_in_wsl_only_from_windows_native() {
        assert!(runs_in_wsl(true, "/usr/bin/claude", false));
        assert!(runs_in_wsl(true, "claude", true));
        assert!(!runs_in_wsl(true, r"C:\npm\claude.cmd", false));
        assert!(!runs_in_wsl(false, "/usr/bin/claude", true));
    }

    #[test]
    fn wslenv_lists_names_with_path_flags() {
        let value = wslenv_value(
            None,
            ["KRONN_DISCUSSION_ID", "TMPDIR", "ANTHROPIC_API_KEY", "TEMP"],
        )
        .unwrap();
        assert_eq!(
            value,
            "KRONN_DISCUSSION_ID:TMPDIR/up:ANTHROPIC_API_KEY:TEMP/up"
        );
    }

    #[test]
    fn wslenv_appends_to_the_users_list_and_keeps_their_flags() {
        let value = wslenv_value(
            Some("GOPATH/l:KRONN_DISCUSSION_ID/u:"),
            ["kronn_discussion_id", "KRONN_BACKEND_URL"],
        )
        .unwrap();
        assert_eq!(value, "GOPATH/l:KRONN_DISCUSSION_ID/u:KRONN_BACKEND_URL");
    }

    #[test]
    fn wslenv_skips_home_path_and_malformed_names() {
        assert_eq!(
            wslenv_value(
                None,
                [
                    "HOME",
                    "USERPROFILE",
                    "PATH",
                    "WSLENV",
                    "COPILOT_HOME",
                    "",
                    "A:B",
                    "A/p"
                ]
            ),
            None
        );
        assert_eq!(wslenv_value(Some("X"), ["X", "x"]), None);
        assert_eq!(
            wslenv_value(None, ["KRONN_A", "KRONN_A"]).unwrap(),
            "KRONN_A"
        );
    }

    #[test]
    fn apply_wslenv_forwards_every_variable_the_launch_sets() {
        let mut command = tokio::process::Command::new("wsl.exe");
        command
            .env_clear()
            .env("KRONN_DISCUSSION_ID", "d1")
            .env("KRONN_BACKEND_URL", "http://localhost:3140")
            .env("KRONN_BRIDGE_TOKEN", "kbt_x")
            .env("TMPDIR", r"C:\proj\.kronn\tmp")
            .env("HOME", r"C:\Users\Romu")
            .env("SystemRoot", r"C:\Windows")
            .env("KRONN_MCP_REF_0", "secret-value")
            .env("WSLENV", "GOPATH/l");
        apply_wslenv(&mut command, |name| name != "SystemRoot");
        let wslenv = command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "WSLENV")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap();
        let entries: Vec<&str> = wslenv.split(':').collect();
        assert_eq!(entries[0], "GOPATH/l");
        for expected in [
            "KRONN_DISCUSSION_ID",
            "KRONN_BACKEND_URL",
            "KRONN_BRIDGE_TOKEN",
            "TMPDIR/up",
            "KRONN_MCP_REF_0",
        ] {
            assert!(
                entries.contains(&expected),
                "{expected} missing from {wslenv}"
            );
        }
        // Filtered by the caller, or WSL-owned.
        for absent in ["SystemRoot", "HOME", "KRONN_AUTH_TOKEN"] {
            assert!(
                !entries.iter().any(|entry| entry.starts_with(absent)),
                "{absent} must not be forwarded: {wslenv}"
            );
        }
        // Values never travel in WSLENV.
        assert!(!wslenv.contains("secret-value"));
    }

    #[test]
    fn backend_url_uses_the_wsl_override_only_in_wsl() {
        let base = Some("http://127.0.0.1:4000");
        let wsl = Some("http://172.20.0.1:4000");
        assert_eq!(agent_backend_url(true, base, wsl), "http://172.20.0.1:4000");
        assert_eq!(agent_backend_url(false, base, wsl), "http://127.0.0.1:4000");
        assert_eq!(agent_backend_url(true, base, None), "http://127.0.0.1:4000");
        assert_eq!(
            agent_backend_url(true, base, Some("  ")),
            "http://127.0.0.1:4000"
        );
        assert_eq!(agent_backend_url(true, None, None), "http://127.0.0.1:3140");
    }

    #[test]
    fn wslconfig_networking_mode_is_read_from_the_wsl2_section() {
        let content = "# comment\n[experimental]\nnetworkingMode=nat\n[wsl2]\nmemory=8GB\n NetworkingMode = \"Mirrored\" ; note\n";
        assert_eq!(
            wslconfig_networking_mode(content).as_deref(),
            Some("mirrored")
        );
        assert_eq!(wslconfig_networking_mode("[wsl2]\nmemory=8GB\n"), None);
        assert_eq!(wslconfig_networking_mode(""), None);
        assert_eq!(
            wslconfig_networking_mode("[experimental]\nnetworkingMode=mirrored\n"),
            None
        );
    }

    #[test]
    fn nat_mode_warns_unless_overridden_or_mirrored() {
        assert!(backend_unreachable_warning_needed(false, None));
        assert!(backend_unreachable_warning_needed(false, Some("nat")));
        assert!(!backend_unreachable_warning_needed(false, Some("mirrored")));
        assert!(!backend_unreachable_warning_needed(true, None));
    }

    #[test]
    fn native_acp_refusal_names_the_program_and_the_way_out() {
        let message = native_acp_wsl_refusal("gemini");
        assert!(message.contains("`gemini` is installed only inside WSL"));
        assert!(message.contains("Install `gemini` on Windows, or run Kronn inside WSL"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_hosts_never_wrap_through_wsl() {
        let (program, args, cwd) = crate::agents::runner::platform_agent_invocation(
            "/usr/bin/claude".into(),
            vec!["--add-dir".into(), r"C:\other".into()],
            true,
            Path::new("/work"),
        );
        assert_eq!(program, "/usr/bin/claude");
        assert_eq!(args, vec!["--add-dir", r"C:\other"]);
        assert_eq!(cwd, Path::new("/work"));
    }

    /// The real host decision: on Windows-native Kronn a WSL binary goes
    /// through `wsl.exe` with translated paths (runs in the Windows CI job).
    #[cfg(windows)]
    #[test]
    fn windows_host_wraps_wsl_binaries_with_translated_paths() {
        let (program, args, _) = crate::agents::runner::platform_agent_invocation(
            "/usr/bin/claude".into(),
            vec!["--mcp-config".into(), r"C:\proj\.mcp.json".into()],
            true,
            Path::new(r"C:\proj"),
        );
        assert_eq!(program, "wsl.exe");
        assert_eq!(
            args,
            vec![
                "--cd",
                "/mnt/c/proj",
                "-e",
                "/usr/bin/claude",
                "--mcp-config",
                "/mnt/c/proj/.mcp.json"
            ]
        );

        let (program, args, _) = crate::agents::runner::platform_agent_invocation(
            r"C:\npm\claude.cmd".into(),
            vec![r"C:\proj\.mcp.json".into()],
            false,
            Path::new(r"C:\proj"),
        );
        assert_eq!(program, r"C:\npm\claude.cmd");
        assert_eq!(args, vec![r"C:\proj\.mcp.json"]);
    }
}
