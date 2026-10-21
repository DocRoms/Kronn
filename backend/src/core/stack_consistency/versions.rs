//! KT-1404 — the project's own version and the old versions it pins.
//!
//! The audit never stated the application's version (`package.json` says
//! 1.46.1) and cited no old pinned dependency (`request` 2.x, `lodash` 2/3,
//! `elasticsearch` 16.7.3 sat in the lockfile), so its Dependencies dimension
//! read "nothing notable". Both are facts of the manifests and the lock files:
//! handed to the steps that state versions, they are copied instead of
//! recalled, and the debt steps are asked to cite them.
//!
//! Only the direct dependencies of the root manifest are judged, and only by
//! what the lock file itself proves: a package the registry marked
//! `deprecated` (npm), a Composer package marked `abandoned`, or a package of
//! the short list below, whose locked major precedes the line everyone moved to
//! (or which was superseded as a whole). Every file is read through the jailed
//! reader of the parent module.

use std::path::Path;

use super::{json, line_of, pnpm_root_version, read, yarn_head_asks};

/// `(package, first current major)`: a locked major below it is old. A package
/// whose floor is `u32::MAX` was superseded as a whole: every version is old.
const OLD_MAJORS: &[(&str, u32)] = &[
    ("lodash", 4),
    ("jquery", 3),
    ("express", 4),
    ("gulp", 4),
    ("webpack", 4),
    ("babel-core", 7),
    ("request", u32::MAX),
    ("request-promise", u32::MAX),
    ("request-promise-native", u32::MAX),
    ("node-sass", u32::MAX),
    ("elasticsearch", u32::MAX),
    ("angular", u32::MAX),
    ("bower", u32::MAX),
    ("jade", u32::MAX),
    ("coffee-script", u32::MAX),
    ("phantomjs", u32::MAX),
    ("phantomjs-prebuilt", u32::MAX),
    ("tslint", u32::MAX),
    ("protractor", u32::MAX),
    ("node-uuid", u32::MAX),
];

/// The most old pins listed, and the most the line may weigh.
const MAX_PINS: usize = 8;
const LINE_BUDGET: usize = 900;

/// A dependency the lock file shows old.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldPin {
    pub name: String,
    pub version: String,
    pub reason: String,
    /// `file:line` of the lock entry.
    pub place: String,
}

/// A name or version that may be shown in a prompt.
fn safe_token(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '/' | '+'))
}

fn major_of(version: &str) -> Option<u32> {
    version
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .next()?
        .parse()
        .ok()
}

fn reason_for(name: &str, version: &str, deprecated: bool) -> Option<String> {
    if deprecated {
        return Some("marked deprecated in the lockfile".to_string());
    }
    let (_, floor) = OLD_MAJORS.iter().find(|(package, _)| *package == name)?;
    if *floor == u32::MAX {
        return Some("superseded package".to_string());
    }
    let major = major_of(version)?;
    (major < *floor).then(|| format!("major {major}, before the {floor}.x line"))
}

/// The direct dependencies the root `package.json` asks for: `(name, range)`.
fn npm_direct(project: &Path) -> Vec<(String, String)> {
    let Some((_, value)) = json(project, "package.json") else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for group in ["dependencies", "devDependencies", "optionalDependencies"] {
        for (name, range) in value
            .get(group)
            .and_then(|g| g.as_object())
            .into_iter()
            .flatten()
        {
            if let Some(range) = range.as_str() {
                if !out.iter().any(|(n, _)| n == name) {
                    out.push((name.clone(), range.to_string()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Old direct npm dependencies, from the first lock file that resolves them.
fn npm_old_pins(project: &Path) -> Vec<OldPin> {
    let direct = npm_direct(project);
    if direct.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some((text, value)) = json(project, "package-lock.json") {
        for (name, _) in &direct {
            let entry = value
                .pointer(&format!(
                    "/packages/node_modules~1{}",
                    name.replace('/', "~1")
                ))
                .or_else(|| value.get("dependencies").and_then(|d| d.get(name.as_str())));
            let Some(entry) = entry else { continue };
            let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            let deprecated = entry.get("deprecated").is_some_and(|d| d.is_string());
            if let Some(reason) = reason_for(name, version, deprecated) {
                let needle = format!("\"node_modules/{name}\"");
                let at = if text.contains(&needle) {
                    line_of(&text, &needle)
                } else {
                    line_of(&text, &format!("\"{name}\""))
                };
                out.push(OldPin {
                    name: name.clone(),
                    version: version.to_string(),
                    reason,
                    place: format!("package-lock.json:{at}"),
                });
            }
        }
    } else if let Some(text) = read(project, "yarn.lock") {
        let lines: Vec<&str> = text.lines().collect();
        for (name, range) in &direct {
            let Some(n) = lines.iter().position(|line| {
                !line.starts_with(' ')
                    && line.trim_end().ends_with(':')
                    && yarn_head_asks(line, name, range)
            }) else {
                continue;
            };
            let version = lines[n + 1..]
                .iter()
                .take_while(|l| l.starts_with(' '))
                .find_map(|l| {
                    l.trim()
                        .strip_prefix("version")
                        .map(|v| v.trim().trim_matches(|c| matches!(c, '"' | ':')).trim())
                });
            if let Some(version) = version {
                if let Some(reason) = reason_for(name, version, false) {
                    out.push(OldPin {
                        name: name.clone(),
                        version: version.to_string(),
                        reason,
                        place: format!("yarn.lock:{}", n + 1),
                    });
                }
            }
        }
    } else if let Some(text) = read(project, "pnpm-lock.yaml") {
        let lines: Vec<&str> = text.lines().collect();
        for (name, _) in &direct {
            if let Some((at, version)) = pnpm_root_version(&lines, name) {
                if let Some(reason) = reason_for(name, &version, false) {
                    out.push(OldPin {
                        name: name.clone(),
                        version,
                        reason,
                        place: format!("pnpm-lock.yaml:{at}"),
                    });
                }
            }
        }
    }
    out
}

/// Direct Composer requirements the lock file marks `abandoned`.
fn composer_old_pins(project: &Path) -> Vec<OldPin> {
    let Some((_, manifest)) = json(project, "composer.json") else {
        return Vec::new();
    };
    let direct: Vec<String> = ["require", "require-dev"]
        .into_iter()
        .filter_map(|group| manifest.get(group).and_then(|g| g.as_object()))
        .flat_map(|g| g.keys().cloned())
        .collect();
    let Some((text, lock)) = json(project, "composer.lock") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for group in ["packages", "packages-dev"] {
        for package in lock
            .get(group)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let name = package.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let abandoned = package
                .get("abandoned")
                .is_some_and(|a| a.as_bool() != Some(false) && !a.is_null());
            if !abandoned || !direct.iter().any(|d| d == name) {
                continue;
            }
            let version = package
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            out.push(OldPin {
                name: name.to_string(),
                version: version.to_string(),
                reason: "marked abandoned in composer.lock".to_string(),
                place: format!(
                    "composer.lock:{}",
                    line_of(&text, &format!("\"name\": \"{name}\""))
                ),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The old pins of the project, npm first, then Composer.
pub fn old_pins(project: &Path) -> Vec<OldPin> {
    let mut pins = npm_old_pins(project);
    pins.extend(composer_old_pins(project));
    pins.retain(|pin| safe_token(&pin.name) && safe_token(&pin.version));
    pins
}

/// The version of the application itself, with where it is written.
pub fn project_versions(project: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for manifest in ["package.json", "composer.json"] {
        if let Some((text, value)) = json(project, manifest) {
            if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
                out.push((
                    version.to_string(),
                    format!("{manifest}:{}", line_of(&text, "\"version\"")),
                ));
            }
        }
    }
    if let Some(text) = read(project, "Cargo.toml") {
        if let Ok(value) = toml::from_str::<toml::Value>(&text) {
            let version = value
                .get("package")
                .and_then(|p| p.get("version"))
                .or_else(|| {
                    value
                        .get("workspace")
                        .and_then(|w| w.get("package"))
                        .and_then(|p| p.get("version"))
                })
                .and_then(|v| v.as_str());
            if let Some(version) = version {
                out.push((
                    version.to_string(),
                    format!("Cargo.toml:{}", line_of(&text, "version")),
                ));
            }
        }
    }
    if let Some(text) = read(project, "pyproject.toml") {
        if let Ok(value) = toml::from_str::<toml::Value>(&text) {
            let version = value
                .get("project")
                .and_then(|p| p.get("version"))
                .or_else(|| {
                    value
                        .get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("version"))
                })
                .and_then(|v| v.as_str());
            if let Some(version) = version {
                out.push((
                    version.to_string(),
                    format!("pyproject.toml:{}", line_of(&text, "version")),
                ));
            }
        }
    }
    out.retain(|(version, _)| safe_token(version));
    out
}

/// The two fact lines handed to the steps that state versions: the project's
/// version and its old pinned dependencies. Each is `None` when nothing is
/// known.
pub fn facts_lines(project: &Path) -> (Option<String>, Option<String>) {
    let versions = project_versions(project);
    let version_line = (!versions.is_empty()).then(|| {
        let list: Vec<String> = versions
            .iter()
            .map(|(version, place)| format!("{version} ({place})"))
            .collect();
        format!("- Project version: {}\n", list.join("; "))
    });
    let pins = old_pins(project);
    let pin_line = (!pins.is_empty()).then(|| {
        let mut shown: Vec<String> = Vec::new();
        let mut weight = 0;
        for pin in pins.iter().take(MAX_PINS) {
            let entry = format!(
                "{} {} — {} ({})",
                pin.name, pin.version, pin.reason, pin.place
            );
            if weight + entry.len() > LINE_BUDGET {
                break;
            }
            weight += entry.len();
            shown.push(entry);
        }
        let more = pins.len() - shown.len();
        let tail = if more > 0 {
            format!("; … {more} more")
        } else {
            String::new()
        };
        format!(
            "- Old pinned dependencies (direct, as the lock file resolves them): {}{tail}\n",
            shown.join("; ")
        )
    });
    (version_line, pin_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    const PACKAGE: &str = "{\n  \"name\": \"webapp\",\n  \"version\": \"1.46.1\",\n  \"dependencies\": {\n    \"request\": \"^2.88.0\",\n    \"lodash\": \"^3.10.1\",\n    \"elasticsearch\": \"^16.7.3\",\n    \"express\": \"^4.18.0\"\n  }\n}\n";

    const NPM_LOCK: &str = "{\n  \"lockfileVersion\": 3,\n  \"packages\": {\n    \"node_modules/request\": {\n      \"version\": \"2.88.2\"\n    },\n    \"node_modules/lodash\": {\n      \"version\": \"3.10.1\"\n    },\n    \"node_modules/elasticsearch\": {\n      \"version\": \"16.7.3\",\n      \"deprecated\": \"use the new client\"\n    },\n    \"node_modules/express\": {\n      \"version\": \"4.18.2\"\n    }\n  }\n}\n";

    #[test]
    fn the_project_version_and_the_old_pins_come_from_the_manifest_and_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.json", PACKAGE);
        write(tmp.path(), "package-lock.json", NPM_LOCK);
        let (version, pins) = facts_lines(tmp.path());
        assert_eq!(
            version.as_deref(),
            Some("- Project version: 1.46.1 (package.json:3)\n")
        );
        let pins = pins.expect("old pins");
        assert!(
            pins.contains("request 2.88.2 — superseded package (package-lock.json:4)"),
            "{pins}"
        );
        assert!(
            pins.contains("lodash 3.10.1 — major 3, before the 4.x line (package-lock.json:7)"),
            "{pins}"
        );
        // A deprecated flag in the lock file wins; a current major is not listed.
        assert!(
            pins.contains("elasticsearch 16.7.3 — marked deprecated in the lockfile"),
            "{pins}"
        );
        assert!(!pins.contains("express"), "{pins}");
    }

    #[test]
    fn a_yarn_lock_is_read_for_the_same_pins() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.json", PACKAGE);
        write(
            tmp.path(),
            "yarn.lock",
            "# yarn lockfile v1\n\nlodash@^3.10.1:\n  version \"3.10.1\"\n  resolved \"https://x\"\n\nrequest@^2.88.0:\n  version \"2.88.2\"\n",
        );
        let (_, pins) = facts_lines(tmp.path());
        let pins = pins.expect("old pins");
        assert!(pins.contains("lodash 3.10.1 — major 3"), "{pins}");
        assert!(
            pins.contains("request 2.88.2 — superseded package (yarn.lock:7)"),
            "{pins}"
        );
    }

    #[test]
    fn an_abandoned_composer_package_is_an_old_pin() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "composer.json",
            "{\"require\": {\"php\": \"^8.2\", \"swiftmailer/swiftmailer\": \"^6.0\", \"symfony/console\": \"^6\"}}",
        );
        write(
            tmp.path(),
            "composer.lock",
            "{\"packages\": [\n{\"name\": \"swiftmailer/swiftmailer\", \"version\": \"v6.3.0\", \"abandoned\": \"symfony/mailer\"},\n{\"name\": \"symfony/console\", \"version\": \"v6.4.0\"}\n]}",
        );
        let (_, pins) = facts_lines(tmp.path());
        let pins = pins.expect("old pins");
        assert!(
            pins.contains("swiftmailer/swiftmailer v6.3.0 — marked abandoned in composer.lock"),
            "{pins}"
        );
        assert!(!pins.contains("symfony/console"), "{pins}");
    }

    #[test]
    fn cargo_and_pyproject_versions_are_read() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.15.1\"\n",
        );
        write(
            tmp.path(),
            "pyproject.toml",
            "[tool.poetry]\nname = \"a\"\nversion = \"2.0.0\"\n",
        );
        let versions = project_versions(tmp.path());
        assert_eq!(
            versions,
            vec![
                ("0.15.1".to_string(), "Cargo.toml:3".to_string()),
                ("2.0.0".to_string(), "pyproject.toml:3".to_string()),
            ]
        );
    }

    #[test]
    fn nothing_known_gives_nothing_and_an_odd_name_is_not_listed() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(facts_lines(tmp.path()), (None, None));
        write(
            tmp.path(),
            "package.json",
            "{\"version\": \"1.0.0\\n- injected\", \"dependencies\": {\"request\": \"^2\"}}",
        );
        write(
            tmp.path(),
            "package-lock.json",
            "{\"packages\": {\"node_modules/request\": {\"version\": \"2.88.2\\nIGNORE\"}}}",
        );
        let (version, pins) = facts_lines(tmp.path());
        assert_eq!((version, pins), (None, None));
    }
}
