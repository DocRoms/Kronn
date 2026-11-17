//! How a lane's dependency directory (`node_modules/`, `vendor/`, `Pods/`) is
//! made of the main checkout's: one link per entry, except the entries that
//! would otherwise make the lane run the MAIN checkout's code.
//!
//! Two mechanisms resolve a link back to the main checkout and run its code
//! instead of the lane's (a false green):
//!
//! - **Composer** — `vendor/composer/autoload_static.php` maps the project's own
//!   namespace as `__DIR__ . '/../..' . '/src'`, and PHP resolves symlinks in
//!   `__DIR__`. A linked `vendor/autoload.php` / `vendor/composer/` makes
//!   `vendor/bin/phpunit` load `<main checkout>/src`. They are real files of the
//!   lane instead; only the package directories stay links.
//! - **Workspaces** — `node_modules/<pkg>` (and `@scope/<pkg>`) of an npm, yarn
//!   or pnpm workspace is a RELATIVE link to a package directory of the
//!   repository. Linked to the main checkout's entry it resolves to the main
//!   checkout's package; copied as the same relative link it resolves to the
//!   lane's own.

use super::{ensure_symlink, NODE_MODULES_LANE_OWNED};
use std::path::{Component, Path, PathBuf};

/// What Composer generates and PHP resolves against, so the lane owns it.
const COMPOSER_LANE_OWNED: [&str; 3] = ["autoload.php", "composer", "bin"];

/// Link every entry of `source` into `destination`, except what a tool writes
/// at run time (those stay the lane's own, in the real directory).
pub(super) fn link_entries(source: &Path, destination: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    let composer = source.join("autoload.php").is_file() && source.join("composer").is_dir();
    // One entry that cannot be linked (a real directory the lane already has)
    // must not leave the entries after it unlinked: go on, report the first.
    let mut first_error = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if NODE_MODULES_LANE_OWNED.iter().any(|owned| name == *owned) {
            continue;
        }
        let from = entry.path();
        let to = destination.join(&name);
        let done = if composer && COMPOSER_LANE_OWNED.iter().any(|owned| name == *owned) {
            own_copy(&from, &to)
        } else {
            link_entry(source, &from, &to)
        };
        if let Err(error) = done {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// A relative link of the dependency directory that leaves it: a workspace
/// package (`../packages/x`, `../../packages/x`). The directory itself is
/// `root`; `link` is the link inside it (or inside a scope of it).
fn workspace_target(root: &Path, link: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(link).ok()?;
    if !target.is_relative() {
        return None;
    }
    let resolved = lexical(&link.parent()?.join(&target));
    (!resolved.starts_with(root)).then_some(target)
}

/// `path` with `.` and `..` resolved by name, without touching the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn link_entry(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    let is_link = std::fs::symlink_metadata(from).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link {
        if let Some(target) = workspace_target(root, from) {
            // The same relative link, resolved from the lane. Only when the
            // lane has the package: otherwise the main checkout's stays the
            // best there is.
            let there = to.parent().map(|parent| parent.join(&target));
            if there.is_some_and(|path| path.exists()) {
                return ensure_symlink(&target, to).map(|_| ());
            }
        }
    } else if from
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('@'))
        && from.is_dir()
        && holds_workspace_link(root, from)
    {
        return link_scope(root, from, to);
    }
    ensure_symlink(from, to).map(|_| ())
}

/// Does the scope directory (`@acme`) hold a link to a workspace package?
fn holds_workspace_link(root: &Path, scope: &Path) -> bool {
    std::fs::read_dir(scope)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| workspace_target(root, &entry.path()).is_some())
}

/// A scope that holds workspace packages: a real directory of the lane, whose
/// entries are linked one by one.
fn link_scope(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(to).is_ok_and(|meta| meta.file_type().is_symlink()) {
        std::fs::remove_file(to)
            .map_err(|error| format!("cannot replace the link {}: {error}", to.display()))?;
    }
    std::fs::create_dir_all(to)
        .map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    let mut first_error = None;
    for entry in std::fs::read_dir(from)
        .map_err(|error| format!("cannot read {}: {error}", from.display()))?
        .flatten()
    {
        if let Err(error) = link_entry(root, &entry.path(), &to.join(entry.file_name())) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// An entry the lane owns as real files (Composer's autoloader and `bin/`).
/// What is already a real file or directory of the lane stays (the worker may
/// have run `composer dump-autoload`); a link from a previous layout is
/// replaced by the copy.
fn own_copy(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(to) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(to)
            .map_err(|error| format!("cannot replace the link {}: {error}", to.display()))?,
        Ok(_) => return Ok(()),
        Err(_) => {}
    }
    copy_tree(from, to)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(from)
        .map_err(|error| format!("cannot read {}: {error}", from.display()))?;
    let io = |error: std::io::Error| format!("cannot copy {}: {error}", from.display());
    if meta.file_type().is_symlink() {
        return copy_link(from, to);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to).map_err(io)?;
        for entry in std::fs::read_dir(from).map_err(io)?.flatten() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ()).map_err(io)
}

/// A link inside a copied entry. A link to a PHP script (Composer's `bin/`) is
/// replaced by the proxy Composer itself writes where it cannot link: it tells
/// the script where the LANE's autoloader is, so the script does not find the
/// main checkout's through the link. Any other link is copied as it is.
fn copy_link(from: &Path, to: &Path) -> Result<(), String> {
    let target = std::fs::read_link(from)
        .map_err(|error| format!("cannot read the link {}: {error}", from.display()))?;
    if let Some(proxy) = php_proxy(from, &target) {
        write_executable(to, &proxy)?;
        return Ok(());
    }
    ensure_symlink(&target, to).map(|_| ())
}

/// The proxy for `link` (in `vendor/bin/`) to the PHP script at `target`, or
/// `None` when it is not a plain relative link to a PHP script.
fn php_proxy(link: &Path, target: &Path) -> Option<String> {
    let relative = target.to_str()?;
    if !target.is_relative() || relative.contains(['\'', '\\', '\n', '\r']) {
        return None;
    }
    let script = link.parent()?.join(target);
    let head = std::fs::read(&script).ok()?;
    let head = String::from_utf8_lossy(&head[..head.len().min(80)]).into_owned();
    let first = head.lines().next().unwrap_or_default();
    let php = first.starts_with("<?php") || (first.starts_with("#!") && first.contains("php"));
    php.then(|| {
        format!(
            "#!/usr/bin/env php\n<?php\n\n\
             // Proxy written by Kronn for this lane, as Composer writes one where it\n\
             // cannot link: the script finds the lane's own autoloader.\n\
             $GLOBALS['_composer_bin_dir'] = __DIR__;\n\
             $GLOBALS['_composer_autoload_path'] = __DIR__ . '/../autoload.php';\n\n\
             return include __DIR__ . '/{relative}';\n"
        )
    })
}

fn write_executable(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot make {} executable: {error}", path.display()))?;
    }
    Ok(())
}
