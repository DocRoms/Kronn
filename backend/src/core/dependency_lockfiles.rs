//! Resolved versions read from lockfiles (KT-1325).
//!
//! `npm outdated` reports an installed version only from `node_modules`; a
//! checkout with a lockfile and no install made every JS row "declared only".
//! The lockfile records the exact version the project resolves, so it is the
//! installed-version source when no tree exists. Composer is read from
//! `composer.lock` the same way.

use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Larger than any real lockfile; anything above is not read.
const MAX_LOCKFILE_BYTES: u64 = 64 * 1024 * 1024;

fn read_lockfile(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_LOCKFILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// A JS lockfile parsed for one manifest directory.
#[derive(Debug)]
pub struct JsLockfile {
    /// File name, shown as the source of the version.
    pub file: &'static str,
    resolver: Resolver,
}

#[derive(Debug)]
enum Resolver {
    Npm {
        value: Value,
        /// The member directory relative to the lockfile (`""` for the root).
        prefix: String,
    },
    Yarn(HashMap<String, Vec<(String, String)>>),
    Pnpm(HashMap<String, String>),
}

impl JsLockfile {
    /// The first readable lockfile in `lock_dir`, parsed for the manifest in
    /// `member_dir` (the same directory, or a workspace member below it).
    pub fn load(lock_dir: &Path, member_dir: &Path) -> Option<Self> {
        let prefix = member_dir
            .strip_prefix(lock_dir)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        for file in [
            "package-lock.json",
            "npm-shrinkwrap.json",
            "yarn.lock",
            "pnpm-lock.yaml",
        ] {
            let Some(content) = read_lockfile(&lock_dir.join(file)) else {
                continue;
            };
            let resolver = match file {
                "yarn.lock" => Some(Resolver::Yarn(parse_yarn_lock(&content))),
                "pnpm-lock.yaml" => {
                    let importer = if prefix.is_empty() { "." } else { &prefix };
                    Some(Resolver::Pnpm(parse_pnpm_importer(&content, importer)))
                }
                _ => serde_json::from_str::<Value>(&content)
                    .ok()
                    .filter(Value::is_object)
                    .map(|value| Resolver::Npm {
                        value,
                        prefix: prefix.clone(),
                    }),
            };
            if let Some(resolver) = resolver {
                return Some(Self { file, resolver });
            }
        }
        None
    }

    /// The version the lockfile resolves `name` to. `declared` is the range
    /// the manifest asks for; Yarn keeps one entry per range, so it picks the
    /// entry. Ambiguity is `None`, never a guess.
    pub fn version(&self, name: &str, declared: Option<&str>) -> Option<String> {
        match &self.resolver {
            Resolver::Npm { value, prefix } => npm_lock_version(value, prefix, name),
            Resolver::Yarn(entries) => {
                let candidates = entries.get(name)?;
                if let Some(declared) = declared {
                    let declared = declared.trim().trim_start_matches("npm:");
                    if let Some((_, version)) =
                        candidates.iter().find(|(range, _)| range == declared)
                    {
                        return Some(version.clone());
                    }
                }
                let first = &candidates.first()?.1;
                candidates
                    .iter()
                    .all(|(_, version)| version == first)
                    .then(|| first.clone())
            }
            Resolver::Pnpm(direct) => direct.get(name).cloned(),
        }
    }
}

fn npm_lock_version(value: &Value, prefix: &str, name: &str) -> Option<String> {
    if let Some(packages) = value.get("packages").and_then(Value::as_object) {
        let hoisted = format!("node_modules/{name}");
        let keys = (!prefix.is_empty())
            .then(|| format!("{prefix}/node_modules/{name}"))
            .into_iter()
            .chain(std::iter::once(hoisted));
        for key in keys {
            let Some(entry) = packages.get(&key) else {
                continue;
            };
            if entry.get("link").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(version) = entry.get("version").and_then(Value::as_str) {
                return Some(version.to_string());
            }
        }
        return None;
    }
    // lockfileVersion 1 has no `packages`: a flat `dependencies` tree.
    value
        .get("dependencies")?
        .get(name)?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

fn unquote(text: &str) -> &str {
    text.trim().trim_matches(|c| c == '"' || c == '\'')
}

/// Yarn classic (`version "1.2.3"`) and berry (`version: 1.2.3`): entries by
/// package name, each with the range it was requested for.
fn parse_yarn_lock(content: &str) -> HashMap<String, Vec<(String, String)>> {
    let mut entries: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut descriptors: Vec<(String, String)> = Vec::new();
    let flush = |descriptors: &mut Vec<(String, String)>,
                 version: Option<String>,
                 entries: &mut HashMap<String, Vec<(String, String)>>| {
        if let Some(version) = version {
            for (name, range) in descriptors.drain(..) {
                entries
                    .entry(name)
                    .or_default()
                    .push((range, version.clone()));
            }
        }
        descriptors.clear();
    };
    let mut version: Option<String> = None;
    for line in content.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            flush(&mut descriptors, version.take(), &mut entries);
            let Some(header) = line.trim_end().strip_suffix(':') else {
                continue;
            };
            for descriptor in header.split(',') {
                let descriptor = unquote(descriptor);
                // The name may start with `@`: split at the last `@` after it.
                let Some(at) = descriptor.get(1..).and_then(|rest| rest.rfind('@')) else {
                    continue;
                };
                let (name, range) = descriptor.split_at(at + 1);
                let range = range[1..].trim_start_matches("npm:");
                if !range.starts_with("workspace:") {
                    descriptors.push((name.to_string(), range.to_string()));
                }
            }
        } else if line.starts_with("  ") && !line.starts_with("   ") {
            let field = line.trim();
            if let Some(rest) = field.strip_prefix("version") {
                if rest.starts_with(' ') || rest.starts_with(':') {
                    let value = unquote(rest.trim_start_matches(':').trim());
                    if !value.is_empty() {
                        version = Some(value.to_string());
                    }
                }
            }
        }
    }
    flush(&mut descriptors, version.take(), &mut entries);
    entries
}

/// A YAML node: the scalar after `key:` and the keys nested below.
#[derive(Debug, Default)]
struct YamlNode {
    value: Option<String>,
    children: Vec<(String, YamlNode)>,
}

impl YamlNode {
    fn get(&self, key: &str) -> Option<&YamlNode> {
        self.children
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, node)| node)
    }
}

/// The `key: value` mappings of a lockfile, by indentation. Not a YAML
/// parser: it understands exactly the nesting pnpm writes and ignores lists.
fn parse_yaml_mappings(content: &str) -> YamlNode {
    // Path of child indexes from the root, with the indent of each level.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut root = YamlNode::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let Some((key, value)) = split_yaml_entry(trimmed) else {
            continue;
        };
        while stack.last().is_some_and(|(level, _)| *level >= indent) {
            stack.pop();
        }
        let mut node = &mut root;
        for (_, index) in &stack {
            node = &mut node.children[*index].1;
        }
        node.children.push((
            key,
            YamlNode {
                value,
                children: Vec::new(),
            },
        ));
        stack.push((indent, node.children.len() - 1));
    }
    root
}

fn split_yaml_entry(line: &str) -> Option<(String, Option<String>)> {
    let (key, rest) = if let Some(quote) = line.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        let end = line[1..].find(quote)? + 1;
        (line[1..end].to_string(), line[end + 1..].strip_prefix(':')?)
    } else {
        let end = line
            .char_indices()
            .find(|(index, c)| {
                let after = &line[index + 1..];
                *c == ':' && (after.is_empty() || after.starts_with(' '))
            })?
            .0;
        (line[..end].to_string(), &line[end + 1..])
    };
    let rest = rest.split(" #").next().unwrap_or("").trim();
    Some((key, (!rest.is_empty()).then(|| unquote(rest).to_string())))
}

/// `1.2.3(react@18)` / `1.2.3_react@18` → `1.2.3`; a `link:` or `file:`
/// target is not a registry version.
fn pnpm_version(raw: &str) -> Option<String> {
    let version = raw.split(['(', '_']).next()?.trim();
    (version.starts_with(|c: char| c.is_ascii_digit())).then(|| version.to_string())
}

/// Direct dependencies of one importer of `pnpm-lock.yaml`. Lockfile v9/v6
/// list importers (`.` is the root); v5 and single-project v6 list the
/// sections at the top, with the version as the bare value.
fn parse_pnpm_importer(content: &str, importer: &str) -> HashMap<String, String> {
    let tree = parse_yaml_mappings(content);
    let project = match tree.get("importers") {
        Some(importers) => match importers.get(importer) {
            Some(project) => project,
            None => return HashMap::new(),
        },
        None => &tree,
    };
    let mut direct = HashMap::new();
    for section in ["dependencies", "devDependencies", "optionalDependencies"] {
        let Some(section) = project.get(section) else {
            continue;
        };
        for (name, node) in &section.children {
            let raw = node
                .get("version")
                .and_then(|version| version.value.as_deref())
                .or(node.value.as_deref());
            if let Some(version) = raw.and_then(pnpm_version) {
                direct.entry(name.clone()).or_insert(version);
            }
        }
    }
    direct
}

/// Versions locked by `composer.lock` (`packages` and `packages-dev`).
pub fn composer_locked_versions(directory: &Path) -> HashMap<String, String> {
    let mut locked = HashMap::new();
    let path: PathBuf = directory.join("composer.lock");
    let Some(value) = read_lockfile(&path).and_then(|c| serde_json::from_str::<Value>(&c).ok())
    else {
        return locked;
    };
    for section in ["packages", "packages-dev"] {
        let Some(packages) = value.get(section).and_then(Value::as_array) else {
            continue;
        };
        for package in packages {
            if let (Some(name), Some(version)) = (
                package.get("name").and_then(Value::as_str),
                package.get("version").and_then(Value::as_str),
            ) {
                locked.insert(name.to_string(), version.to_string());
            }
        }
    }
    locked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) {
        if let Some(parent) = dir.join(name).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn load(dir: &Path) -> JsLockfile {
        JsLockfile::load(dir, dir).expect("a lockfile")
    }

    #[test]
    fn package_lock_v3_and_v2_read_the_packages_tree() {
        for version in [3, 2] {
            let temp = tempfile::TempDir::new().unwrap();
            write(
                temp.path(),
                "package-lock.json",
                &format!(
                    r#"{{"lockfileVersion":{version},"packages":{{"":{{"name":"app"}},
                    "node_modules/lodash":{{"version":"3.10.1"}},
                    "node_modules/@scope/pkg":{{"version":"1.2.3"}},
                    "node_modules/linked":{{"link":true,"resolved":"packages/linked"}}}}}}"#
                ),
            );
            let lock = load(temp.path());
            assert_eq!(lock.file, "package-lock.json");
            assert_eq!(lock.version("lodash", None).as_deref(), Some("3.10.1"));
            assert_eq!(lock.version("@scope/pkg", None).as_deref(), Some("1.2.3"));
            assert_eq!(lock.version("linked", None), None);
            assert_eq!(lock.version("absent", None), None);
        }
    }

    #[test]
    fn package_lock_v1_reads_the_flat_dependencies_tree() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "package-lock.json",
            r#"{"lockfileVersion":1,"dependencies":{"lazysizes":{"version":"2.0.7"}}}"#,
        );
        assert_eq!(
            load(temp.path()).version("lazysizes", None).as_deref(),
            Some("2.0.7")
        );
    }

    #[test]
    fn package_lock_prefers_a_members_own_entry_over_the_hoisted_one() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "package-lock.json",
            r#"{"lockfileVersion":3,"packages":{
                "node_modules/joi":{"version":"17.0.0"},
                "packages/web/node_modules/joi":{"version":"18.0.0"}}}"#,
        );
        let member = temp.path().join("packages/web");
        let lock = JsLockfile::load(temp.path(), &member).unwrap();
        assert_eq!(lock.version("joi", None).as_deref(), Some("18.0.0"));
        assert_eq!(
            load(temp.path()).version("joi", None).as_deref(),
            Some("17.0.0")
        );
    }

    #[test]
    fn yarn_classic_picks_the_entry_for_the_declared_range() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "yarn.lock",
            "# yarn lockfile v1\n\n\
gulp-rev@^8.1.0:\n  version \"8.1.1\"\n  resolved \"https://x\"\n  dependencies:\n    lodash \"^4.0.0\"\n\n\
lodash@^3.10.1:\n  version \"3.10.1\"\n\n\
\"lodash@^4.0.0\", lodash@^4.17.0:\n  version \"4.17.21\"\n\n\
\"@scope/pkg@^1.0.0\":\n  version \"1.2.3\"\n",
        );
        let lock = load(temp.path());
        assert_eq!(lock.file, "yarn.lock");
        assert_eq!(lock.version("gulp-rev", None).as_deref(), Some("8.1.1"));
        assert_eq!(
            lock.version("lodash", Some("^3.10.1")).as_deref(),
            Some("3.10.1")
        );
        assert_eq!(
            lock.version("lodash", Some("^4.17.0")).as_deref(),
            Some("4.17.21")
        );
        // Two versions and no range to choose: ambiguous, so unknown.
        assert_eq!(lock.version("lodash", None), None);
        assert_eq!(
            lock.version("@scope/pkg", Some("^1.0.0")).as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn yarn_berry_reads_the_colon_form_and_npm_protocol() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "yarn.lock",
            "__metadata:\n  version: 8\n\n\
\"lodash@npm:^3.10.1\":\n  version: 3.10.1\n  resolution: \"lodash@npm:3.10.1\"\n  checksum: abc\n  languageName: node\n\n\
\"@scope/pkg@npm:^1.0.0, @scope/pkg@npm:^1.1.0\":\n  version: 1.2.3\n\n\
\"app@workspace:.\":\n  version: 0.0.0-use.local\n",
        );
        let lock = load(temp.path());
        assert_eq!(
            lock.version("lodash", Some("^3.10.1")).as_deref(),
            Some("3.10.1")
        );
        assert_eq!(
            lock.version("@scope/pkg", Some("^1.1.0")).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(lock.version("app", None), None);
    }

    #[test]
    fn pnpm_lock_v9_and_v6_read_the_root_importer() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      lodash:\n        specifier: ^3.10.1\n        version: 3.10.1\n      react-dom:\n        specifier: ^18.0.0\n        version: 18.2.0(react@18.2.0)\n    devDependencies:\n      '@scope/pkg':\n        specifier: ^1.0.0\n        version: 1.2.3\n  packages/web:\n    dependencies:\n      lodash:\n        specifier: ^4.0.0\n        version: 4.17.21\n\npackages:\n  lodash@3.10.1:\n    resolution: {integrity: sha512-x}\n",
        );
        let lock = load(temp.path());
        assert_eq!(lock.file, "pnpm-lock.yaml");
        assert_eq!(lock.version("lodash", None).as_deref(), Some("3.10.1"));
        assert_eq!(lock.version("react-dom", None).as_deref(), Some("18.2.0"));
        assert_eq!(lock.version("@scope/pkg", None).as_deref(), Some("1.2.3"));
        let member = temp.path().join("packages/web");
        let web = JsLockfile::load(temp.path(), &member).unwrap();
        assert_eq!(web.version("lodash", None).as_deref(), Some("4.17.21"));
        assert_eq!(web.version("react-dom", None), None);

        // v6 without importers: the sections sit at the top.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "pnpm-lock.yaml",
            "lockfileVersion: '6.0'\n\ndependencies:\n  lodash:\n    specifier: ^3.10.1\n    version: 3.10.1\n",
        );
        assert_eq!(
            load(temp.path()).version("lodash", None).as_deref(),
            Some("3.10.1")
        );
    }

    #[test]
    fn pnpm_lock_v5_reads_bare_versions() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "pnpm-lock.yaml",
            "lockfileVersion: 5.4\n\nspecifiers:\n  lodash: ^3.10.1\n  react-dom: ^18.0.0\n\ndependencies:\n  lodash: 3.10.1\n  react-dom: 18.2.0_react@18.2.0\n  local: link:../local\n",
        );
        let lock = load(temp.path());
        assert_eq!(lock.version("lodash", None).as_deref(), Some("3.10.1"));
        assert_eq!(lock.version("react-dom", None).as_deref(), Some("18.2.0"));
        assert_eq!(lock.version("local", None), None);
    }

    #[test]
    fn an_unreadable_lockfile_falls_through_to_the_next_one() {
        let temp = tempfile::TempDir::new().unwrap();
        write(temp.path(), "package-lock.json", "{ not json");
        write(
            temp.path(),
            "yarn.lock",
            "lodash@^3.10.1:\n  version \"3.10.1\"\n",
        );
        let lock = load(temp.path());
        assert_eq!(lock.file, "yarn.lock");
        assert!(JsLockfile::load(tempfile::TempDir::new().unwrap().path(), temp.path()).is_none());
    }

    #[test]
    fn composer_lock_lists_packages_and_dev_packages() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "composer.lock",
            r#"{"packages":[{"name":"symfony/console","version":"v6.4.1"}],
            "packages-dev":[{"name":"phpunit/phpunit","version":"10.5.0"}]}"#,
        );
        let locked = composer_locked_versions(temp.path());
        assert_eq!(
            locked.get("symfony/console").map(String::as_str),
            Some("v6.4.1")
        );
        assert_eq!(
            locked.get("phpunit/phpunit").map(String::as_str),
            Some("10.5.0")
        );
        assert!(composer_locked_versions(Path::new("/nonexistent-kronn")).is_empty());
    }
}
