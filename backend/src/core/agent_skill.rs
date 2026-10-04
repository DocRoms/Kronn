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

use std::collections::BTreeMap;

/// `name` is at most this many characters.
pub const NAME_MAX: usize = 64;
/// `description` is at most this many characters.
pub const DESCRIPTION_MAX: usize = 1024;
/// `compatibility`, when present, is at most this many characters.
pub const COMPATIBILITY_MAX: usize = 500;

/// A skill file split into the header fields the specification defines and its
/// Markdown body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSkillFile {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
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
    if !file.metadata.is_empty() {
        header.push_str("metadata:\n");
        // A key is written bare, so one that could break the line is dropped.
        for (key, value) in file.metadata.iter().filter(|(key, _)| metadata_key_ok(key)) {
            header.push_str(&format!("  {key}: {}\n", quoted(value)));
        }
    }
    format!("---\n{header}---\n\n{}\n", file.body.trim_end_matches('\n'))
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

    #[test]
    fn a_file_without_a_header_is_refused() {
        assert!(parse("# Just markdown\n").is_err());
        assert!(parse("---\nname: open\n").is_err());
    }
}
