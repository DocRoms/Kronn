//! Local files an agent cites in a message by path (KT-954).
//!
//! An agent that writes a file and links it by its path leaves a link the UI
//! cannot open. These helpers find such links and decide, file by file,
//! whether it may be attached to the message.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Largest file attached from a link: the same cap as a manual upload.
pub const MAX_LINKED_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Why a linked local file is not attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Missing,
    NotAFile,
    Sensitive,
    OutsideAllowedRoots,
    TooLarge,
}

/// Targets of Markdown links and images that point at a local file, in order
/// of first appearance. Code spans and fenced blocks are skipped: they are not
/// rendered as links.
pub fn local_link_targets(content: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    local_link_spans(content)
        .into_iter()
        .filter_map(|(target, _)| seen.insert(target.clone()).then_some(target))
        .collect()
}

pub fn rewrite_local_links(content: &str, targets: &BTreeMap<String, String>) -> String {
    let mut result = content.to_owned();
    for (target, span) in local_link_spans(content).into_iter().rev() {
        if let Some(replacement) = targets.get(&target) {
            result.replace_range(span, replacement);
        }
    }
    result
}

fn local_link_spans(content: &str) -> Vec<(String, Range<usize>)> {
    let text = strip_code(content);
    let mut offset = 0;
    let mut found = Vec::new();
    while let Some(start) = text[offset..].find("](") {
        offset += start + 2;
        let rest = &text[offset..];
        let Some(target) = link_target(rest) else {
            continue;
        };
        let start = offset + usize::from(rest.starts_with('<'));
        let end = start + target.len();
        if is_local_target(&target) && content.get(start..end) == Some(&target) {
            found.push((target, start..end));
        }
        offset = end;
    }
    found
}

/// Decide whether the file behind a local link may be attached. Every path is
/// compared after resolving symlinks, so a link cannot escape an allowed root
/// through one.
pub fn resolve_attachable(
    target: &str,
    allowed_roots: &[PathBuf],
    forbidden_roots: &[PathBuf],
    home: Option<&Path>,
) -> Result<PathBuf, Refusal> {
    let path = local_path(target, home).ok_or(Refusal::Missing)?;
    if is_sensitive(&path) {
        return Err(Refusal::Sensitive);
    }
    let path = path.canonicalize().map_err(|_| Refusal::Missing)?;
    let meta = std::fs::metadata(&path).map_err(|_| Refusal::Missing)?;
    if !meta.is_file() {
        return Err(Refusal::NotAFile);
    }
    let under = |roots: &[PathBuf]| {
        roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| path.starts_with(root))
    };
    if is_sensitive(&path) || under(forbidden_roots) {
        return Err(Refusal::Sensitive);
    }
    if !under(allowed_roots) {
        return Err(Refusal::OutsideAllowedRoots);
    }
    if meta.len() > MAX_LINKED_FILE_BYTES {
        return Err(Refusal::TooLarge);
    }
    Ok(path)
}

fn strip_code(content: &str) -> String {
    // Preserve byte offsets, including UTF-8 inside examples, for rewriting.
    let mut out = content.as_bytes().to_vec();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let first = trimmed.as_bytes().first().copied().unwrap_or_default();
        let count = trimmed.bytes().take_while(|byte| *byte == first).count();
        let marker = matches!(first, b'`' | b'~') && count >= 3;
        if fence.is_some() || marker {
            out[offset..offset + line.len()].fill(b' ');
            match fence {
                Some((symbol, size)) if marker && symbol == first && count >= size => fence = None,
                None if marker => fence = Some((first, count)),
                _ => {}
            }
        } else {
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] != b'`' {
                    i += 1;
                    continue;
                }
                let start = i;
                let width = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
                i += width;
                let mut end = bytes.len();
                while i < bytes.len() {
                    if bytes[i] != b'`' {
                        i += 1;
                        continue;
                    }
                    let closing = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
                    i += closing;
                    if closing == width {
                        end = i;
                        break;
                    }
                }
                out[offset + start..offset + end].fill(b' ');
            }
        }
        offset += line.len();
    }
    String::from_utf8(out).expect("masking whole byte spans preserves UTF-8")
}

/// The destination of a Markdown link, `rest` starting right after `](`.
fn link_target(rest: &str) -> Option<String> {
    if let Some(inner) = rest.strip_prefix('<') {
        let end = inner.find('>')?;
        return Some(inner[..end].to_string());
    }
    let mut depth = 0usize;
    for (i, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                // An optional title follows the destination after a space.
                let dest = rest[..i].split(" \"").next().unwrap_or_default().trim();
                return (!dest.is_empty()).then(|| dest.to_string());
            }
            ')' => depth -= 1,
            '\n' => return None,
            _ => {}
        }
    }
    None
}

fn is_local_target(target: &str) -> bool {
    let bytes = target.as_bytes();
    (target.starts_with('/') && !target.starts_with("//"))
        || target.starts_with("~/")
        || target.starts_with("file://")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/'))
}

fn local_path(target: &str, home: Option<&Path>) -> Option<PathBuf> {
    let target = target.split(['#', '?']).next()?;
    let target = strip_line_suffix(target);
    if target.starts_with("file://") {
        return reqwest::Url::parse(target).ok()?.to_file_path().ok();
    }
    let decoded = percent_decode(target)?;
    if let Some(rel) = decoded.strip_prefix("~/") {
        return Some(home?.join(rel));
    }
    Some(PathBuf::from(decoded))
}

pub fn points_to_attached_file(target: &str, stored: &Path, home: Option<&Path>) -> bool {
    let Some(path) = local_path(target, home) else {
        return false;
    };
    if is_sensitive(&path) {
        return false;
    }
    match (path.canonicalize(), stored.canonicalize()) {
        (Ok(path), Ok(stored)) => path == stored,
        _ => false,
    }
}

fn strip_line_suffix(target: &str) -> &str {
    let mut path = target;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((prefix, line))
                if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) =>
            {
                path = prefix
            }
            _ => break,
        }
    }
    path
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            // A `%` that starts no escape is a literal character of the name.
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn is_sensitive(path: &Path) -> bool {
    let secret_dir = path.components().any(|c| {
        matches!(
            c.as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .as_str(),
            ".ssh" | ".gnupg" | ".aws" | ".kube" | ".docker"
        )
    });
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    secret_dir
        || name.starts_with(".env")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("id_ecdsa")
        || [".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|ext| name.ends_with(ext))
        || matches!(
            name.as_str(),
            ".netrc" | ".npmrc" | ".pypirc" | "credentials" | "credentials.json"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_local_links_and_images_once_and_skips_code_and_urls() {
        let content = "Voici le [GIF](/private/var/T/sax.gif) et ![aperçu](/private/var/T/sax.gif)\n\
            - [Planche](</tmp/my sheet.png>) [titre](/tmp/a.json \"Disposition\")\n\
            - [web](https://example.com/a.png) [ancre](#section) [proto](//cdn/x.png)\n\
            - `[code](/tmp/inline.png)`\n\
            ```\n[bloc](/tmp/fenced.png)\n```\n\
            - [home](~/out.zip) [file](file:///tmp/f.txt) [win](C:\\out\\w.png) [enc](/tmp/a%20b.png)";
        assert_eq!(
            local_link_targets(content),
            vec![
                "/private/var/T/sax.gif",
                "/tmp/my sheet.png",
                "/tmp/a.json",
                "~/out.zip",
                "file:///tmp/f.txt",
                "C:\\out\\w.png",
                "/tmp/a%20b.png",
            ]
        );
    }

    #[test]
    fn attaches_only_existing_files_under_an_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = allowed.path().join("sprite sheet.png");
        std::fs::write(&file, b"png").unwrap();
        std::fs::write(outside.path().join("x.png"), b"png").unwrap();
        let roots = vec![allowed.path().to_path_buf()];

        let encoded = format!("{}/sprite%20sheet.png", allowed.path().display());
        assert_eq!(
            resolve_attachable(&encoded, &roots, &[], None),
            Ok(file.canonicalize().unwrap())
        );
        let url = format!("file://{}", file.display());
        assert!(resolve_attachable(&url, &roots, &[], None).is_ok());
        assert_eq!(
            resolve_attachable(
                &outside.path().join("x.png").to_string_lossy(),
                &roots,
                &[],
                None
            ),
            Err(Refusal::OutsideAllowedRoots)
        );
        assert_eq!(
            resolve_attachable(
                &allowed.path().join("missing.png").to_string_lossy(),
                &roots,
                &[],
                None
            ),
            Err(Refusal::Missing)
        );
        assert_eq!(
            resolve_attachable(&allowed.path().to_string_lossy(), &roots, &[], None),
            Err(Refusal::NotAFile)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_smuggle_a_file_from_outside_the_root() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("notes.txt");
        std::fs::write(&secret, b"private").unwrap();
        let link = allowed.path().join("looks-local.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        assert_eq!(
            resolve_attachable(
                &link.to_string_lossy(),
                &[allowed.path().to_path_buf()],
                &[],
                None
            ),
            Err(Refusal::OutsideAllowedRoots)
        );
    }

    #[test]
    fn secrets_and_kronn_data_are_never_attached_even_under_an_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let roots = vec![allowed.path().to_path_buf()];
        std::fs::create_dir_all(allowed.path().join(".ssh")).unwrap();
        std::fs::create_dir_all(allowed.path().join("data")).unwrap();
        for name in [
            ".env",
            ".env.local",
            "id_rsa",
            "server.pem",
            "deploy.key",
            ".npmrc",
            ".ssh/config",
            "data/kronn.db",
        ] {
            std::fs::write(allowed.path().join(name), b"secret").unwrap();
        }
        let forbidden = vec![allowed.path().join("data")];
        for name in [
            ".env",
            ".env.local",
            "id_rsa",
            "server.pem",
            "deploy.key",
            ".npmrc",
            ".ssh/config",
            "data/kronn.db",
        ] {
            assert_eq!(
                resolve_attachable(
                    &allowed.path().join(name).to_string_lossy(),
                    &roots,
                    &forbidden,
                    None
                ),
                Err(Refusal::Sensitive),
                "{name}"
            );
        }
    }

    #[test]
    fn a_home_relative_link_needs_a_home() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("out.zip"), b"zip").unwrap();
        let roots = vec![home.path().to_path_buf()];
        assert!(resolve_attachable("~/out.zip", &roots, &[], Some(home.path())).is_ok());
        assert_eq!(
            resolve_attachable("~/out.zip", &roots, &[], None),
            Err(Refusal::Missing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn sensitive_aliases_are_refused_before_symlink_resolution() {
        let root = tempfile::tempdir().unwrap();
        let content = root.path().join("opaque.txt");
        std::fs::write(&content, b"fixture-private").unwrap();
        for name in [".env", ".SSH/config"] {
            let alias = root.path().join(name);
            std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&content, &alias).unwrap();
            assert_eq!(
                resolve_attachable(
                    &alias.to_string_lossy(),
                    &[root.path().to_owned()],
                    &[],
                    None
                ),
                Err(Refusal::Sensitive)
            );
        }
    }

    #[test]
    fn rewrites_exact_link_destinations_and_images_without_touching_examples() {
        let source = "é [one](/tmp/a/out.png) ![two](</tmp/b/out.png> \"title\")\n`[code](/tmp/a/out.png)`\n``[é](/tmp/a/out.png)``\n```rust\n[x](/tmp/a/out.png)\n```\n[again](/tmp/a/out.png)";
        let targets = BTreeMap::from([
            ("/tmp/a/out.png".into(), "/api/files/one".into()),
            ("/tmp/b/out.png".into(), "/api/files/two".into()),
        ]);
        assert_eq!(rewrite_local_links(source, &targets), "é [one](/api/files/one) ![two](</api/files/two> \"title\")\n`[code](/tmp/a/out.png)`\n``[é](/tmp/a/out.png)``\n```rust\n[x](/tmp/a/out.png)\n```\n[again](/api/files/one)");
    }

    #[test]
    fn a_destination_containing_link_like_text_is_rewritten_once() {
        let source = "[odd](/tmp/a](/tmp/b).txt)";
        let targets = BTreeMap::from([("/tmp/a](/tmp/b).txt".into(), "/api/files/odd".into())]);
        assert_eq!(local_link_targets(source), vec!["/tmp/a](/tmp/b).txt"]);
        assert_eq!(
            rewrite_local_links(source, &targets),
            "[odd](/api/files/odd)"
        );
    }

    #[test]
    fn resolves_line_suffixes_and_refuses_remote_file_hosts() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.md");
        std::fs::write(&file, b"content").unwrap();
        let roots = [root.path().to_owned()];
        for suffix in [":12", ":12:3", "#L12", "#L12-L14"] {
            assert_eq!(
                resolve_attachable(&format!("{}{suffix}", file.display()), &roots, &[], None),
                Ok(file.canonicalize().unwrap())
            );
        }
        assert_eq!(
            resolve_attachable("file://remote.example/tmp/a.md", &roots, &[], None),
            Err(Refusal::Missing)
        );
        assert!(points_to_attached_file(
            &file.to_string_lossy(),
            &file,
            None
        ));
        assert!(!points_to_attached_file(
            &root.path().join("other/a.md").to_string_lossy(),
            &file,
            None
        ));
    }
}
