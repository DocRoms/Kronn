//! Stack-specific Step 8 checklist items (KT-152), injected only for the
//! ecosystems the project actually uses, so a Rust repo is never asked about
//! `opcache` and a PHP repo never about `cargo deny`.
//!
//! Detection reuses `docs_migration::detect_stack_languages` (root and one level
//! down) and adds the Dockerfile base image, so "the image runs PHP" counts.

use std::path::Path;

use crate::core::audit_baseline::{dockerfile_instructions, Inventory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ecosystem {
    Php,
    Node,
    Rust,
    Python,
    Go,
}

impl Ecosystem {
    pub fn label(self) -> &'static str {
        match self {
            Ecosystem::Php => "PHP",
            Ecosystem::Node => "Node.js",
            Ecosystem::Rust => "Rust",
            Ecosystem::Python => "Python",
            Ecosystem::Go => "Go",
        }
    }

    /// Judgement items: each needs reading config in context, so none of
    /// them is a detector.
    pub fn checklist(self) -> &'static [&'static str] {
        match self {
            Ecosystem::Php => &[
                "`display_errors = Off` in the production php.ini or image (stack traces leak into HTTP responses otherwise; High when production shows On).",
                "`opcache.enable = 1` in production (every request recompiles otherwise).",
                "The production image or deploy runs `composer install --no-dev` (dev tools stay out of production).",
            ],
            Ecosystem::Node => &[
                "Production installs come from the lockfile (`npm ci`, `pnpm install --frozen-lockfile`, `yarn install --immutable`) and the lockfile is committed.",
                "The runtime image or process runs with `NODE_ENV=production` and without devDependencies.",
            ],
            Ecosystem::Rust => &[
                "CI runs `cargo clippy` with `-D warnings` and a supply-chain check (`cargo audit` or `cargo deny`).",
                "`unwrap()`/`expect()` on request or input paths (a panic there is a denial of service).",
            ],
            Ecosystem::Python => &[
                "Dependencies are pinned (a lock file, `==` pins or hashes), not open ranges.",
                "No `DEBUG = True` (Django) or debug mode (Flask) in production settings; production runs a WSGI/ASGI server, not the dev server.",
            ],
            Ecosystem::Go => &[
                "CI runs `go vet` and `govulncheck`.",
                "HTTP servers set read/write timeouts (a bare `http.ListenAndServe` has none).",
            ],
        }
    }
}

/// Ecosystems in use, in stable order.
pub fn detect_ecosystems(root: &Path, inv: &Inventory) -> Vec<Ecosystem> {
    let stack = crate::core::docs_migration::detect_stack_languages(root);
    let mut out = Vec::new();
    if stack.php || image_runs_php(inv) {
        out.push(Ecosystem::Php);
    }
    if stack.typescript || stack.javascript {
        out.push(Ecosystem::Node);
    }
    if stack.rust {
        out.push(Ecosystem::Rust);
    }
    if stack.python {
        out.push(Ecosystem::Python);
    }
    if stack.go {
        out.push(Ecosystem::Go);
    }
    out
}

/// A Dockerfile stage built on an official PHP image.
fn image_runs_php(inv: &Inventory) -> bool {
    inv.dockerfiles.iter().any(|path| {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !meta.is_file() || meta.len() > 512 * 1024 {
            return false;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        dockerfile_instructions(&content)
            .iter()
            .filter(|i| i.keyword == "FROM")
            .filter_map(|i| i.args.split_whitespace().find(|t| !t.starts_with("--")))
            .any(|image| {
                let name = image.rsplit('/').next().unwrap_or(image).to_lowercase();
                name == "php" || name.starts_with("php:") || name.starts_with("php@")
            })
    })
}

/// The Step 8 block listing the stack-specific items, or saying none apply.
pub fn render_ecosystem_block(ecosystems: &[Ecosystem]) -> String {
    let mut out = String::from("## Stack-specific checklist\n\n");
    if ecosystems.is_empty() {
        out.push_str(
            "No PHP, Node.js, Rust, Python or Go manifest detected: no stack-specific item applies.\n",
        );
        return out;
    }
    let names: Vec<&str> = ecosystems.iter().map(|e| e.label()).collect();
    out.push_str(&format!(
        "Detected: {}. Each item that does not hold is a TD; one that holds or cannot apply is noted in the matching coverage row.\n\n",
        names.join(", ")
    ));
    for eco in ecosystems {
        out.push_str(&format!("**{}**\n", eco.label()));
        for item in eco.checklist() {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn detect(root: &Path) -> Vec<Ecosystem> {
        detect_ecosystems(root, &Inventory::collect(root))
    }

    #[test]
    fn each_manifest_maps_to_its_ecosystem() {
        for (file, eco) in [
            ("composer.json", Ecosystem::Php),
            ("package.json", Ecosystem::Node),
            ("Cargo.toml", Ecosystem::Rust),
            ("pyproject.toml", Ecosystem::Python),
            ("go.mod", Ecosystem::Go),
        ] {
            let t = tempfile::tempdir().unwrap();
            write(t.path(), file, "{}");
            assert_eq!(detect(t.path()), vec![eco], "{file}");
        }
    }

    #[test]
    fn a_php_base_image_counts_as_php_without_composer() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docker/Dockerfile", "FROM php:8.3-fpm\n");
        assert_eq!(detect(t.path()), vec![Ecosystem::Php]);
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "Dockerfile", "FROM phpmyadmin:5\n");
        assert!(
            detect(t.path()).is_empty(),
            "phpmyadmin is not a PHP app image"
        );
    }

    #[test]
    fn a_monorepo_sub_directory_is_detected() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "backend/Cargo.toml", "[package]");
        write(t.path(), "frontend/package.json", "{}");
        assert_eq!(detect(t.path()), vec![Ecosystem::Node, Ecosystem::Rust]);
    }

    #[test]
    fn php_items_never_reach_a_project_without_php() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "Cargo.toml", "[package]");
        let block = render_ecosystem_block(&detect(t.path()));
        assert!(block.contains("cargo deny"), "{block}");
        assert!(!block.contains("display_errors"), "{block}");
        assert!(!block.contains("opcache"), "{block}");
    }

    #[test]
    fn php_items_reach_a_php_project() {
        let block = render_ecosystem_block(&[Ecosystem::Php]);
        assert!(block.contains("display_errors = Off"));
        assert!(block.contains("opcache.enable = 1"));
        assert!(!block.contains("cargo"));
    }

    #[test]
    fn no_stack_says_so_instead_of_staying_silent() {
        let block = render_ecosystem_block(&[]);
        assert!(block.contains("no stack-specific item applies"));
    }
}
