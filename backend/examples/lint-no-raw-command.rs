//! AST-aware lint: detect raw `Command::new` calls in production code.
//!
//! Replaces the bash `grep 'Command::new'` lint that lived in CI pre-0.8.2.
//! That variant false-positived on:
//!
//! 1. **Multi-line string literals** — the Security audit prompt mentions
//!    the pattern `Command::new("sh").arg("-c")` as something the auditor
//!    should *flag in user code*. The grep saw a match and failed the build.
//! 2. **Documentation comments** — `///` and `//!` blocks that explain
//!    why production code must use `cmd::sync_cmd`/`async_cmd` instead of
//!    `Command::new` were being misread as violations.
//!
//! The grep tried to paper over (2) with `grep -vE '^\s*//'` but had no way
//! to handle (1), and adding the next anti-pattern to an audit prompt
//! would re-break it.
//!
//! This linter parses each `.rs` file with `syn`, walks the AST, and only
//! reports actual call expressions whose function path ends in
//! `Command::new`. Strings and comments are invisible to the AST, so the
//! whole class of false positives goes away.
//!
//! Items annotated with `#[cfg(test)]` (or `#[cfg(any(test, ...))]`) are
//! exempt — the wrapper crate `crate::core::cmd` is the only place real
//! `Command::new` calls are allowed in prod, and tests can use it freely.
//!
//! Run from `backend/`:
//!     cargo run --example lint-no-raw-command --quiet
//!
//! Exit code 0 = clean, 1 = at least one violation.
//! When `$GITHUB_ACTIONS` is set, prints `::error file=…,line=…::…` markers
//! so the GitHub workflow surfaces the violation inline on the PR diff.

use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use walkdir::WalkDir;

/// Files exempt from the rule:
///   - `core/cmd.rs` is the wrapper that legitimately wraps `Command::new`.
///   - Anything matching `*_test.rs` is test-only by convention.
fn is_exempt_file(path: &Path) -> bool {
    let p = path.to_string_lossy();
    p.ends_with("core/cmd.rs") || p.ends_with("core\\cmd.rs") || p.contains("_test.rs")
}

/// True when an attribute is `#[cfg(test)]` or `#[cfg(any(test, ...))]` or
/// any other `cfg` form that requires the `test` predicate. Conservative:
/// when in doubt, we treat the item as test-only and skip it. False
/// negatives here are tolerable (we miss a real violation in a weird
/// cfg combo); false positives are not (we'd break the build on legit
/// prod code).
fn is_test_cfg(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("test") {
            found = true;
        }
        // Recurse into `any(...)` / `all(...)` / `not(...)`.
        if meta.path.is_ident("any") || meta.path.is_ident("all") {
            let _ = meta.parse_nested_meta(|inner| {
                if inner.path.is_ident("test") {
                    found = true;
                }
                Ok(())
            });
        }
        Ok(())
    });
    found
}

fn attrs_mark_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(is_test_cfg)
}

struct Linter {
    in_test_depth: usize,
    violations: Vec<(usize, String)>,
}

impl Linter {
    fn new() -> Self {
        Self {
            in_test_depth: 0,
            violations: Vec::new(),
        }
    }

    /// True when the call expression's function path ends in `Command::new`
    /// — covers `Command::new(...)`, `std::process::Command::new(...)`,
    /// `process::Command::new(...)`, and any aliased re-export. We don't
    /// resolve types (that would need rustc), but the convention in this
    /// codebase is that the only `Command::new` worth flagging is the std
    /// one; if a future crate adds another `Command::new` that's not a
    /// process spawn, the rule still applies as a "don't shadow stdlib
    /// names" guard.
    fn is_command_new(call: &syn::ExprCall) -> bool {
        let syn::Expr::Path(p) = call.func.as_ref() else {
            return false;
        };
        let segments = &p.path.segments;
        let n = segments.len();
        n >= 2 && segments[n - 2].ident == "Command" && segments[n - 1].ident == "new"
    }
}

impl<'ast> Visit<'ast> for Linter {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match item {
            syn::Item::Fn(i) => &i.attrs,
            syn::Item::Mod(i) => &i.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Struct(i) => &i.attrs,
            syn::Item::Enum(i) => &i.attrs,
            syn::Item::Trait(i) => &i.attrs,
            syn::Item::Const(i) => &i.attrs,
            syn::Item::Static(i) => &i.attrs,
            syn::Item::Type(i) => &i.attrs,
            syn::Item::Use(i) => &i.attrs,
            _ => &[],
        };
        let entered = attrs_mark_test(attrs);
        if entered {
            self.in_test_depth += 1;
        }
        visit::visit_item(self, item);
        if entered {
            self.in_test_depth -= 1;
        }
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs: &[syn::Attribute] = match item {
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Const(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            _ => &[],
        };
        let entered = attrs_mark_test(attrs);
        if entered {
            self.in_test_depth += 1;
        }
        visit::visit_impl_item(self, item);
        if entered {
            self.in_test_depth -= 1;
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if self.in_test_depth == 0 && Self::is_command_new(call) {
            let line = call.span().start().line;
            let snippet = quote_to_string(&call.func);
            self.violations.push((line, snippet));
        }
        visit::visit_expr_call(self, call);
    }
}

/// Render the function path back to a short string, e.g.
/// `std::process::Command::new`, for the error message. We avoid pulling
/// in `quote` for this — a simple manual join is enough.
fn quote_to_string(expr: &syn::Expr) -> String {
    let syn::Expr::Path(p) = expr else {
        return "Command::new".into();
    };
    p.path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn lint_file(path: &Path) -> std::io::Result<Vec<(usize, String)>> {
    let src = fs::read_to_string(path)?;
    // Cheap pre-filter: if the file doesn't even contain the substring,
    // skip the parse. Saves ~100ms across the whole src/ tree.
    if !src.contains("Command::new") {
        return Ok(Vec::new());
    }
    let parsed = match syn::parse_file(&src) {
        Ok(f) => f,
        Err(e) => {
            // Don't fail the lint when the file doesn't parse — that's
            // rustc's job. We just can't analyze it. Print a warning so
            // CI shows the skip but doesn't break.
            eprintln!(
                "warning: {} could not be parsed by syn: {}",
                path.display(),
                e
            );
            return Ok(Vec::new());
        }
    };
    let mut lint = Linter::new();
    lint.visit_file(&parsed);
    Ok(lint.violations)
}

fn main() -> std::io::Result<()> {
    // Optional CLI arg: a directory to lint. Defaults to `src/` so the
    // canonical invocation `cargo run --example lint-no-raw-command` from
    // `backend/` just works. The arg form is what the test fixture below
    // uses — and what a user can use to lint a subtree under review.
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("src"));
    if !root.is_dir() {
        eprintln!(
            "error: {} is not a directory (run from `backend/` or pass an explicit root).",
            root.display(),
        );
        std::process::exit(2);
    }

    let mut total_violations = 0usize;
    let is_gh = std::env::var("GITHUB_ACTIONS").is_ok();

    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|e| std::io::Error::other(e.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_exempt_file(path) {
            continue;
        }
        let violations = lint_file(path)?;
        for (line, snippet) in &violations {
            total_violations += 1;
            if is_gh {
                println!(
                    "::error file={},line={}::Raw {} in prod code (use cmd::sync_cmd/async_cmd — see src/core/cmd.rs)",
                    path.display(),
                    line,
                    snippet,
                );
            } else {
                println!(
                    "VIOLATION {}:{}: raw `{}` — use `cmd::sync_cmd`/`async_cmd`",
                    path.display(),
                    line,
                    snippet,
                );
            }
        }
    }

    if total_violations == 0 {
        println!("✓ No raw Command::new in production code");
        Ok(())
    } else {
        eprintln!("\n✗ {total_violations} violation(s). On Windows every raw Command::new flashes a console window.");
        eprintln!("  Fix: use `crate::core::cmd::sync_cmd(...)` or `async_cmd(...)` instead.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    //! Lint-the-linter: a fixture that exercises every classification
    //! branch (real call, string literal, raw string, doc comment,
    //! cfg(test) mod, cfg(any(test, …)) fn, fully-qualified path) so a
    //! future tweak to `is_test_cfg` or `is_command_new` can't silently
    //! re-break the audit prompt build.
    //!
    //! Run with: `cargo test --example lint-no-raw-command`.

    use super::*;
    use syn::visit::Visit;

    fn lint_src(src: &str) -> Vec<(usize, String)> {
        let parsed = syn::parse_file(src).expect("fixture must parse");
        let mut lint = Linter::new();
        lint.visit_file(&parsed);
        lint.violations
    }

    #[test]
    fn flags_unqualified_command_new_in_prod() {
        let src = r#"
            use std::process::Command;
            fn run() { let _ = Command::new("git").status(); }
        "#;
        let v = lint_src(src);
        assert_eq!(v.len(), 1, "expected 1 violation, got: {v:?}");
    }

    #[test]
    fn flags_fully_qualified_command_new_in_prod() {
        let src = r#"
            fn run() { let _ = std::process::Command::new("ls").status(); }
        "#;
        let v = lint_src(src);
        assert_eq!(v.len(), 1, "expected 1 violation, got: {v:?}");
    }

    #[test]
    fn ignores_command_new_inside_string_literal() {
        // The audit prompt regression — must not be flagged.
        // Two `#` on the outer raw string so the inner `r#"..."#` parses.
        let src = r##"
            fn doc() {
                let _ = "Look for Command::new('sh').arg('-c') in user code";
                let _ = r#"Raw string Command::new also here"#;
            }
        "##;
        let v = lint_src(src);
        assert!(v.is_empty(), "string literals must not trigger: {v:?}");
    }

    #[test]
    fn ignores_command_new_in_multiline_string() {
        // The exact shape that broke CI on 0.8.2 — multi-line escaped
        // string inside a prompt declaration.
        let src = r#"
            fn audit_prompt() -> String {
                "- Shell: no exec, system, Command::new('sh').arg('-c') with user input.\n\
                 - More text on next line.".into()
            }
        "#;
        let v = lint_src(src);
        assert!(v.is_empty(), "multi-line string must not trigger: {v:?}");
    }

    #[test]
    fn ignores_command_new_in_doc_comment() {
        let src = r#"
            /// Use this wrapper instead of `Command::new` — see `cmd::sync_cmd`.
            fn wrapper() {}
        "#;
        let v = lint_src(src);
        assert!(v.is_empty(), "doc comments must not trigger: {v:?}");
    }

    #[test]
    fn ignores_command_new_under_cfg_test_mod() {
        let src = r#"
            #[cfg(test)]
            mod tests {
                use std::process::Command;
                #[test]
                fn t() { let _ = Command::new("git").status(); }
            }
        "#;
        let v = lint_src(src);
        assert!(v.is_empty(), "cfg(test) mod must be skipped: {v:?}");
    }

    #[test]
    fn ignores_command_new_under_cfg_any_test() {
        let src = r#"
            #[cfg(any(test, feature = "extra"))]
            fn helper() { let _ = std::process::Command::new("git").status(); }
        "#;
        let v = lint_src(src);
        assert!(v.is_empty(), "cfg(any(test, ...)) must be skipped: {v:?}");
    }

    #[test]
    fn ignores_command_new_under_cfg_test_fn() {
        let src = r#"
            #[cfg(test)]
            fn helper() { let _ = std::process::Command::new("git").status(); }
        "#;
        let v = lint_src(src);
        assert!(v.is_empty(), "cfg(test) fn must be skipped: {v:?}");
    }

    #[test]
    fn flags_call_outside_cfg_test_in_same_file() {
        // Mixed file: prod code + a cfg(test) mod. Only the prod call
        // should be flagged.
        let src = r#"
            fn prod() { let _ = std::process::Command::new("ls").status(); }

            #[cfg(test)]
            mod tests {
                use std::process::Command;
                #[test]
                fn t() { let _ = Command::new("git").status(); }
            }
        "#;
        let v = lint_src(src);
        assert_eq!(v.len(), 1, "expected only the prod call, got: {v:?}");
    }
}
