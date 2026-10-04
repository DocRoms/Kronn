//! The Agent Skills file format (<https://agentskills.io/specification>): a
//! `<slug>/SKILL.md` whose YAML header carries `name` and `description`, and
//! whose body is the skill itself.
//!
//! Every skill Kronn writes into a repository goes through [`render`], so the
//! result is a real skill any agent can load: the fields Kronn adds for its own
//! use (display name, icon, category…) live under `metadata`, which the
//! specification reserves for exactly that. [`parse`] reads such a file back —
//! and any other skill found in the repository — and [`validate`] states
//! whether a header meets the specification.
//!
//! Claude Code adds `arguments` and `argument-hint` to the header for skills
//! that take values (<https://code.claude.com/docs/en/skills.md>); Kronn's own
//! description of those values is JSON under the flat [`VARIABLES_KEY`].

use std::collections::BTreeMap;

/// `name` is at most this many characters.
pub const NAME_MAX: usize = 64;
/// `description` is at most this many characters.
pub const DESCRIPTION_MAX: usize = 1024;
/// `compatibility`, when present, is at most this many characters.
pub const COMPATIBILITY_MAX: usize = 500;
/// An argument name is at most this many characters.
pub const ARGUMENT_NAME_MAX: usize = 64;
/// The `metadata` key holding Kronn's [`SkillVariable`] list as a JSON string.
pub const VARIABLES_KEY: &str = "kronn-variables";

use crate::models::SkillVariable;

/// A skill file split into the header fields the specification defines and its
/// Markdown body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSkillFile {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
    /// Claude Code named positional arguments, substituted as `$name`.
    pub arguments: Vec<String>,
    /// Claude Code `argument-hint`, shown during autocomplete.
    pub argument_hint: Option<String>,
    /// Free-form string map the specification leaves to the client.
    pub metadata: BTreeMap<String, String>,
    pub body: String,
}

/// Whether `name` is a valid skill name: 1 to 64 lowercase letters, digits and
/// hyphens, neither starting nor ending with a hyphen, and no two in a row.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_MAX
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether `name` can be a `$name` placeholder: an ASCII letter or `_`, then
/// letters, digits, `_` or `-` (Claude Code's own example uses `from-lang`).
/// A leading digit would read as the positional `$0`, `$1`…
pub fn valid_argument_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().count() <= ARGUMENT_NAME_MAX
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Checks a declared argument list: valid names, none twice.
pub fn validate_arguments(arguments: &[String]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for name in arguments {
        if !valid_argument_name(name) {
            return Err(format!(
                "skill argument `{name}` must start with a letter or `_` and hold only ASCII letters, digits, `_` and `-` (at most {ARGUMENT_NAME_MAX})"
            ));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("skill argument `{name}` is declared twice"));
        }
    }
    Ok(())
}

/// Reads the `kronn-variables` JSON: a list of [`SkillVariable`] whose names
/// are declared in `arguments`, each described once.
pub fn parse_variables(json: &str, arguments: &[String]) -> Result<Vec<SkillVariable>, String> {
    let variables: Vec<SkillVariable> = serde_json::from_str(json).map_err(|error| {
        format!("`metadata.{VARIABLES_KEY}` is not a valid variable list: {error}")
    })?;
    let mut seen = std::collections::BTreeSet::new();
    for variable in &variables {
        if !arguments.iter().any(|name| name == &variable.name) {
            return Err(format!(
                "`metadata.{VARIABLES_KEY}` describes `{}`, which `arguments` does not declare",
                variable.name
            ));
        }
        if !seen.insert(variable.name.as_str()) {
            return Err(format!(
                "`metadata.{VARIABLES_KEY}` describes `{}` twice",
                variable.name
            ));
        }
    }
    Ok(variables)
}

/// The `kronn-variables` value for `variables`, `None` when there is none.
pub fn variables_json(variables: &[SkillVariable]) -> Option<String> {
    if variables.is_empty() {
        return None;
    }
    serde_json::to_string(variables).ok()
}

/// Whether `file` meets the specification for a skill stored in a folder named
/// `folder`: a valid `name` equal to the folder name, a non-empty
/// `description` within its bound and a `compatibility` within its own.
pub fn validate(file: &AgentSkillFile, folder: &str) -> Result<(), String> {
    if !valid_name(&file.name) {
        return Err(format!(
            "skill name `{}` must be 1-{NAME_MAX} lowercase letters, digits and single hyphens",
            file.name
        ));
    }
    if file.name != folder {
        return Err(format!(
            "skill name `{}` must match its folder `{folder}`",
            file.name
        ));
    }
    let description = file.description.chars().count();
    if description == 0 || description > DESCRIPTION_MAX {
        return Err(format!(
            "skill description must be 1-{DESCRIPTION_MAX} characters, not {description}"
        ));
    }
    if file
        .compatibility
        .as_deref()
        .is_some_and(|value| value.chars().count() > COMPATIBILITY_MAX)
    {
        return Err(format!(
            "skill compatibility must be at most {COMPATIBILITY_MAX} characters"
        ));
    }
    validate_arguments(&file.arguments)?;
    if let Some(json) = file.metadata.get(VARIABLES_KEY) {
        parse_variables(json, &file.arguments)?;
    }
    Ok(())
}

/// Reads a YAML scalar: a double-quoted one is decoded as the JSON string it
/// is a superset of, a single-quoted one loses its quotes (`''` is `'`),
/// anything else is taken as written.
pub fn decode_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        if let Ok(decoded) = serde_json::from_str::<String>(raw) {
            return decoded;
        }
        return raw[1..raw.len() - 1].to_string();
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return raw[1..raw.len() - 1].replace("''", "'");
    }
    raw.to_string()
}

/// Writes `value` as a YAML double-quoted scalar (also a valid TOML basic
/// string): a newline or a quote in it can never start another key.
pub(crate) fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// A YAML plain scalar when the name cannot be read as anything but a string,
/// a quoted one otherwise (`true`, `123`…).
fn name_scalar(name: &str) -> String {
    let reserved = matches!(
        name,
        "true" | "false" | "null" | "yes" | "no" | "on" | "off"
    );
    if reserved || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        quoted(name)
    } else {
        name.to_string()
    }
}

fn metadata_key_ok(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The text of a `SKILL.md`. The header is always valid YAML; the body follows
/// after one blank line and the file ends with a single newline.
pub fn render(file: &AgentSkillFile) -> String {
    let mut header = format!(
        "name: {}\ndescription: {}\n",
        name_scalar(&file.name),
        quoted(&file.description)
    );
    if let Some(license) = file.license.as_deref().filter(|value| !value.is_empty()) {
        header.push_str(&format!("license: {}\n", quoted(license)));
    }
    if let Some(value) = file.compatibility.as_deref().filter(|v| !v.is_empty()) {
        header.push_str(&format!("compatibility: {}\n", quoted(value)));
    }
    if let Some(tools) = file.allowed_tools.as_deref().filter(|v| !v.is_empty()) {
        header.push_str(&format!("allowed-tools: {}\n", quoted(tools)));
    }
    header.push_str(&arguments_lines(
        &file.arguments,
        file.argument_hint.as_deref(),
    ));
    if !file.metadata.is_empty() {
        header.push_str("metadata:\n");
        // A key is written bare, so one that could break the line is dropped.
        for (key, value) in file.metadata.iter().filter(|(key, _)| metadata_key_ok(key)) {
            header.push_str(&format!("  {key}: {}\n", quoted(value)));
        }
    }
    format!("---\n{header}---\n\n{}\n", file.body.trim_end_matches('\n'))
}

/// The `arguments` and `argument-hint` header lines, empty when there are none.
/// The list is written as a JSON array, which YAML reads as a flow sequence.
pub(crate) fn arguments_lines(arguments: &[String], hint: Option<&str>) -> String {
    let mut lines = String::new();
    if !arguments.is_empty() {
        let list = serde_json::to_string(arguments).unwrap_or_else(|_| "[]".to_string());
        lines.push_str(&format!("arguments: {list}\n"));
    }
    if let Some(hint) = hint.filter(|value| !value.is_empty()) {
        lines.push_str(&format!("argument-hint: {}\n", quoted(hint)));
    }
    lines
}

/// Reads an `arguments` value: a flow list (`[a, b]`), a space-separated
/// string, or — when `raw` is empty — the block list (`- a`) that follows.
/// Returns the names and the index of the first line after them.
fn arguments_value(lines: &[&str], index: usize, raw: &str) -> (Vec<String>, usize) {
    if raw.is_empty() {
        let mut next = index;
        let mut names = Vec::new();
        while let Some(line) = lines.get(next) {
            let Some(item) = line.trim_start().strip_prefix('-') else {
                break;
            };
            names.push(decode_scalar(item));
            next += 1;
        }
        return (names, next);
    }
    let names = if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        inner
            .split(',')
            .map(decode_scalar)
            .filter(|name| !name.is_empty())
            .collect()
    } else {
        decode_scalar(raw)
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    (names, index)
}

/// Splits a `SKILL.md` into its header lines and its body, `None` without a
/// well-formed `---` header.
fn split_header(text: &str) -> Option<(Vec<&str>, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut header = Vec::new();
    let mut consumed = text.find('\n')? + 1;
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            return Some((header, &text[consumed..]));
        }
        header.push(line.trim_end_matches(['\r', '\n']));
    }
    None
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The lines of a block scalar (`|`, `>`) that starts after `lines[index]`,
/// and the index of the first line that is not part of it.
fn block_scalar(lines: &[&str], index: usize, indicator: &str) -> (String, usize) {
    let mut next = index + 1;
    let mut collected: Vec<&str> = Vec::new();
    while let Some(line) = lines.get(next) {
        if !line.trim().is_empty() && indentation(line) == 0 {
            break;
        }
        collected.push(line.trim());
        next += 1;
    }
    while collected.last().is_some_and(|line| line.is_empty()) {
        collected.pop();
    }
    let value = if indicator.starts_with('>') {
        collected.join(" ")
    } else {
        collected.join("\n")
    };
    (value, next)
}

/// Reads a `SKILL.md`. Unknown header fields are ignored; `name` and
/// `description` may be missing here — [`validate`] is what rejects that.
pub fn parse(text: &str) -> Result<AgentSkillFile, String> {
    let (header, body) = split_header(text).ok_or("SKILL.md has no `---` header")?;
    let mut file = AgentSkillFile {
        body: body
            .strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body)
            .trim_end_matches(['\r', '\n'])
            .to_string(),
        ..AgentSkillFile::default()
    };
    let mut index = 0;
    while index < header.len() {
        let line = header[index];
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || indentation(line) > 0 {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let (key, raw) = (key.trim(), raw.trim());
        if key == "metadata" && raw.is_empty() {
            while let Some(entry) = header
                .get(index)
                .filter(|line| line.trim().is_empty() || indentation(line) > 0)
            {
                index += 1;
                if let Some((name, value)) = entry.trim().split_once(':') {
                    file.metadata
                        .insert(name.trim().to_string(), decode_scalar(value));
                }
            }
            continue;
        }
        if key == "arguments" {
            let (names, next) = arguments_value(&header, index, raw);
            file.arguments = names;
            index = next;
            continue;
        }
        let value = if raw.starts_with('|') || raw.starts_with('>') {
            let (value, next) = block_scalar(&header, index - 1, raw);
            index = next;
            value
        } else {
            decode_scalar(raw)
        };
        match key {
            "name" => file.name = value,
            "description" => file.description = value,
            "license" if !value.is_empty() => file.license = Some(value),
            "compatibility" if !value.is_empty() => file.compatibility = Some(value),
            "allowed-tools" if !value.is_empty() => file.allowed_tools = Some(value),
            "argument-hint" if !value.is_empty() => file.argument_hint = Some(value),
            _ => {}
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentSkillFile {
        AgentSkillFile {
            name: "rust".into(),
            description: "Idiomatic Rust: \"ownership\", errors.\nSecond line.".into(),
            license: Some("MIT".into()),
            compatibility: None,
            allowed_tools: Some("Bash Read".into()),
            arguments: Vec::new(),
            argument_hint: None,
            metadata: BTreeMap::from([
                ("kronn-name".to_string(), "Rust Best Practices".to_string()),
                ("kronn-icon".to_string(), "🦀".to_string()),
            ]),
            body: "# Rust\n\nUse `Result`.".into(),
        }
    }

    #[test]
    fn names_follow_the_specification() {
        for good in ["rust", "a", "code-review", "v2-api", &"a".repeat(64)] {
            assert!(valid_name(good), "{good} is valid");
        }
        for bad in [
            "",
            "Rust",
            "-rust",
            "rust-",
            "ru--st",
            "ru st",
            "ru_st",
            "rüst",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad:?} is not valid");
        }
    }

    #[test]
    fn a_rendered_skill_reads_back_unchanged_and_is_valid() {
        let text = render(&sample());
        assert!(
            text.starts_with("---\nname: rust\ndescription: \""),
            "{text}"
        );
        assert!(text.ends_with("Use `Result`.\n"));
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed, sample());
        validate(&parsed, "rust").unwrap();
    }

    #[test]
    fn the_folder_and_the_description_are_checked() {
        let mut file = sample();
        assert!(validate(&file, "other").unwrap_err().contains("must match"));
        file.description.clear();
        assert!(validate(&file, "rust").unwrap_err().contains("description"));
        file.description = "x".repeat(DESCRIPTION_MAX + 1);
        assert!(validate(&file, "rust").is_err());
        file.description = "x".repeat(DESCRIPTION_MAX);
        assert!(validate(&file, "rust").is_ok());
        file.name = "Rust".into();
        assert!(validate(&file, "Rust").is_err());
    }

    #[test]
    fn a_name_yaml_would_not_read_as_a_string_is_quoted() {
        for name in ["true", "null", "123", "3d-models"] {
            let text = render(&AgentSkillFile {
                name: name.into(),
                description: "d".into(),
                ..AgentSkillFile::default()
            });
            assert!(text.contains(&format!("name: \"{name}\"\n")), "{text}");
            assert_eq!(parse(&text).unwrap().name, name);
        }
    }

    #[test]
    fn hand_written_headers_are_understood() {
        let text = "---\r\nname: review\r\ndescription: >\r\n  Review the\r\n  diff.\r\nlicense: 'Apache-2.0'\r\nmetadata:\r\n  author: \"a b\"\r\n  version: 2\r\nallowed-tools: Bash(git:*) Read\r\n---\r\n\r\nBody.\r\n";
        let file = parse(text).unwrap();
        assert_eq!(file.name, "review");
        assert_eq!(file.description, "Review the diff.");
        assert_eq!(file.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(file.allowed_tools.as_deref(), Some("Bash(git:*) Read"));
        assert_eq!(file.metadata["author"], "a b");
        assert_eq!(file.metadata["version"], "2");
        assert_eq!(file.body, "Body.");
    }

    fn variabilized() -> AgentSkillFile {
        let variables = vec![
            SkillVariable {
                name: "ticket".into(),
                label: "Numéro du ticket 🎫".into(),
                description: Some("Clé « Jira », ex. EW-1".into()),
                required: true,
                default_value: Some("EW-1".into()),
                control: None,
            },
            SkillVariable {
                name: "from-lang".into(),
                label: String::new(),
                description: None,
                required: false,
                default_value: None,
                control: Some(crate::models::PromptVariableControl::Textarea),
            },
        ];
        let mut file = sample();
        file.arguments = vec!["ticket".into(), "from-lang".into()];
        file.argument_hint = Some("[ticket] [from-lang]".into());
        file.metadata.insert(
            VARIABLES_KEY.to_string(),
            variables_json(&variables).unwrap(),
        );
        file.body = "Migrate $ticket from $from-lang.".into();
        file
    }

    #[test]
    fn a_skill_with_arguments_reads_back_unchanged_and_is_valid() {
        let text = render(&variabilized());
        assert!(
            text.contains(
                "arguments: [\"ticket\",\"from-lang\"]\nargument-hint: \"[ticket] [from-lang]\"\n"
            ),
            "{text}"
        );
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed, variabilized());
        validate(&parsed, "rust").unwrap();
        assert_eq!(render(&parsed), text, "render is stable");
        let variables =
            parse_variables(&parsed.metadata[VARIABLES_KEY], &parsed.arguments).unwrap();
        assert_eq!(variables[0].label, "Numéro du ticket 🎫");
        assert_eq!(
            variables[1].control,
            Some(crate::models::PromptVariableControl::Textarea)
        );
    }

    #[test]
    fn every_claude_code_arguments_shape_is_read() {
        for header in [
            "arguments: [issue, branch]",
            "arguments: [\"issue\", 'branch']",
            "arguments: issue branch",
            "arguments: \"issue branch\"",
            "arguments:\n  - issue\n  - branch",
            "arguments:\n- issue\n- \"branch\"",
        ] {
            let text =
                format!("---\nname: m\ndescription: d\n{header}\nlicense: MIT\n---\nBody.\n");
            let file = parse(&text).unwrap();
            assert_eq!(file.arguments, vec!["issue", "branch"], "{header}");
            assert_eq!(file.license.as_deref(), Some("MIT"), "{header}");
        }
        let file = parse("---\nname: m\ndescription: d\narguments: []\n---\nB\n").unwrap();
        assert!(file.arguments.is_empty());
    }

    #[test]
    fn bad_argument_names_are_refused_with_a_clear_error() {
        for bad in ["1st", "a b", "é", "$x", "", &"a".repeat(65)] {
            let error = validate_arguments(&[bad.to_string()]).unwrap_err();
            assert!(error.contains("skill argument"), "{bad}: {error}");
        }
        for good in ["x", "_x", "from-lang", "a1_b-2"] {
            validate_arguments(&[good.to_string()]).unwrap();
        }
        let error = validate_arguments(&["a".into(), "a".into()]).unwrap_err();
        assert!(error.contains("twice"), "{error}");
    }

    #[test]
    fn a_bad_variable_list_is_refused_with_a_clear_error() {
        let arguments = vec!["ticket".to_string()];
        for (json, expected) in [
            ("not json", "not a valid variable list"),
            ("{\"name\":\"ticket\"}", "not a valid variable list"),
            ("[{\"label\":\"no name\"}]", "not a valid variable list"),
            ("[{\"name\":\"other\"}]", "does not declare"),
            ("[{\"name\":\"ticket\"},{\"name\":\"ticket\"}]", "twice"),
        ] {
            let error = parse_variables(json, &arguments).unwrap_err();
            assert!(error.contains(expected), "{json}: {error}");
            assert!(error.contains(VARIABLES_KEY), "{json}: {error}");
        }
        assert!(parse_variables("[]", &[]).unwrap().is_empty());
        let mut file = variabilized();
        file.metadata.insert(VARIABLES_KEY.into(), "[".into());
        assert!(validate(&file, "rust").unwrap_err().contains(VARIABLES_KEY));
        file.metadata.remove(VARIABLES_KEY);
        file.arguments.push("1bad".into());
        assert!(validate(&file, "rust").unwrap_err().contains("1bad"));
    }

    #[test]
    fn a_file_without_a_header_is_refused() {
        assert!(parse("# Just markdown\n").is_err());
        assert!(parse("---\nname: open\n").is_err());
    }
}
