//! Tests for the Quick Exec catalogue — KT-195.
//!
//! The templates exist so the interesting part of a command line is reviewed once
//! and reused. So what is tested is that a caller cannot widen a template past
//! what it declares — which is the only thing standing between an allowlisted
//! `gh` and an arbitrary forge write.

use super::*;
use crate::core::quick_exec::{validate, DENIED_BINARIES};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

// ── the catalogue is coherent ───────────────────────────────────────

#[test]
fn every_template_names_an_allowlisted_binary() {
    // A template pointing at a refused binary can never run. Better to know here
    // than at the one moment someone needs the result.
    assert!(
        unspawnable_templates().is_empty(),
        "templates that can never spawn: {:?}",
        unspawnable_templates()
    );
}

#[test]
fn no_template_uses_a_shell() {
    for candidate in TEMPLATES {
        assert!(
            !DENIED_BINARIES.contains(&candidate.binary),
            "`{}` runs through {}",
            candidate.id,
            candidate.binary
        );
    }
}

#[test]
fn template_ids_are_unique() {
    // `template()` returns the first match, so a duplicate id would silently
    // shadow one of the two.
    let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before, "duplicate template id");
}

#[test]
fn each_family_the_ticket_names_has_a_template() {
    // The DoD lists five families. This fails if one is dropped in a later edit.
    for expected in [
        "backend-tests-filtered", // targeted tests
        "backend-tests-full",     // full gate
        "frontend-typecheck",     // build / typecheck / lint
        "pr-checks",              // forge and CI collection
        "rtk-gain",               // token diagnostics
    ] {
        assert!(template(expected).is_some(), "`{expected}` is missing");
    }
}

#[test]
fn every_template_says_what_a_pass_establishes() {
    // A green result is read by someone deciding whether something is covered.
    // A template that does not state its scope invites that reader to overclaim.
    for candidate in TEMPLATES {
        assert!(
            candidate.establishes.len() > 20,
            "`{}` does not say what it establishes",
            candidate.id
        );
    }
}

#[test]
fn a_template_that_takes_no_argument_declares_a_zero_maximum() {
    // The two fields must agree, or `max_arguments` would let an argument through
    // to a shape that refuses everything — a confusing rejection instead of a
    // clear one.
    for candidate in TEMPLATES {
        if candidate.argument == ArgumentShape::None {
            assert_eq!(candidate.max_arguments, 0, "`{}`", candidate.id);
        } else {
            assert!(candidate.max_arguments > 0, "`{}`", candidate.id);
        }
    }
}

// ── a caller cannot widen a template ────────────────────────────────

#[test]
fn a_flag_is_never_accepted_as_a_template_argument() {
    // THE control. `gh` and `git` are allowlisted and can write; a template pins
    // the subcommand, and this is what stops an extra option from replacing it.
    let cwd = Path::new("/");
    for flag in ["-X", "--method=DELETE", "--upload-file", "-o", "--output"] {
        for id in ["pr-checks", "changed-paths", "backend-tests-filtered"] {
            let rejection = spec_from_template(id, cwd, &args(&[flag]))
                .expect_err(&format!("`{id}` accepted `{flag}`"));
            assert!(
                rejection.0.contains("flag") || rejection.0.contains("shape"),
                "unexpected reason: {}",
                rejection.0
            );
        }
    }
}

#[test]
fn a_template_with_no_argument_refuses_one() {
    let rejection = spec_from_template("backend-clippy", Path::new("/"), &args(&["--fix"]))
        .expect_err("accepted an argument");
    assert!(rejection.0.contains("at most 0"));
}

#[test]
fn more_arguments_than_declared_are_refused() {
    assert!(spec_from_template("pr-checks", Path::new("/"), &args(&["1", "2"])).is_err());
}

#[test]
fn an_unknown_template_id_is_refused() {
    let rejection = spec_from_template("rm-rf", Path::new("/"), &[]).expect_err("accepted");
    assert!(rejection.0.contains("no template"));
}

// ── argument shapes ─────────────────────────────────────────────────

#[test]
fn a_numeric_argument_accepts_only_digits() {
    assert!(ArgumentShape::Numeric.check("138").is_ok());
    for bad in ["138; id", "abc", "13.8", "$(id)", "13 8", ""] {
        assert!(
            ArgumentShape::Numeric.check(bad).is_err(),
            "`{bad}` was accepted as a number"
        );
    }
}

#[test]
fn a_test_filter_accepts_a_module_path_and_nothing_exotic() {
    assert!(ArgumentShape::TestFilter.check("db::review_ledger").is_ok());
    assert!(ArgumentShape::TestFilter
        .check("core::quick_exec::*")
        .is_ok());
    for bad in ["a;b", "a b", "a$(b)", "a|b", "a>b", "a`b`"] {
        assert!(
            ArgumentShape::TestFilter.check(bad).is_err(),
            "`{bad}` was accepted as a filter"
        );
    }
}

#[test]
fn a_path_argument_refuses_traversal() {
    assert!(ArgumentShape::PathLike.check("src/a.test.ts").is_ok());
    // The cwd is already bounded; a `..` argument is how a command is aimed back
    // out of it.
    assert!(ArgumentShape::PathLike.check("../../etc/passwd").is_err());
    assert!(ArgumentShape::PathLike.check("src/../../x").is_err());
}

#[test]
fn a_git_ref_accepts_a_range_and_refuses_the_rest() {
    assert!(ArgumentShape::GitRef.check("main...HEAD").is_ok());
    assert!(ArgumentShape::GitRef.check("d4362e4").is_ok());
    assert!(ArgumentShape::GitRef.check("work/kt-token-economy").is_ok());
    for bad in ["main;id", "main HEAD", "$(git log)", "main|cat"] {
        assert!(
            ArgumentShape::GitRef.check(bad).is_err(),
            "`{bad}` was accepted as a ref"
        );
    }
}

#[test]
fn the_none_shape_accepts_nothing_at_all() {
    // Including values that would be fine for any other shape.
    for value in ["138", "main", "src/a.ts", ""] {
        assert!(ArgumentShape::None.check(value).is_err());
    }
}

// ── the produced spec still goes through the boundary ───────────────

#[test]
fn a_template_spec_passes_validation_and_carries_its_arguments_last() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let spec = spec_from_template(
        "backend-tests-filtered",
        root.path(),
        &args(&["db::review_ledger"]),
    )
    .unwrap();
    assert_eq!(spec.argv, args(&["test", "--lib", "db::review_ledger"]));
    assert!(
        validate(&spec, &roots).is_ok(),
        "a catalogue spec was refused by the boundary"
    );
}

#[test]
fn a_template_spec_is_still_refused_outside_the_roots() {
    // The catalogue narrows what can be asked; it does not grant a working
    // directory. Both checks have to hold.
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let spec = spec_from_template("backend-clippy", elsewhere.path(), &[]).unwrap();
    assert!(validate(&spec, &[root.path().to_path_buf()]).is_err());
}

// ── the slot form ───────────────────────────────────────────────────

#[test]
fn a_slot_is_filled_in_place_rather_than_appended() {
    // Some values belong INSIDE an argument — an API path carrying a PR number.
    // Appending would produce a wrong URL and a confusing 404.
    let root = tempfile::tempdir().unwrap();
    let spec = spec_from_template("pr-review-comments", root.path(), &args(&["138"])).unwrap();
    assert!(
        spec.argv
            .iter()
            .any(|part| part == "repos/{owner}/{repo}/pulls/138/comments"),
        "the slot was not filled: {:?}",
        spec.argv
    );
    assert!(
        !spec.argv.iter().any(|part| part == "138"),
        "the value was appended as well as substituted"
    );
}

#[test]
fn the_gh_owner_and_repo_placeholders_survive_substitution() {
    // `gh` resolves those itself from the working directory. Filling them here
    // would hard-code a repository into the catalogue.
    let root = tempfile::tempdir().unwrap();
    let spec = spec_from_template("pr-reactions", root.path(), &args(&["7"])).unwrap();
    let path = spec.argv.iter().find(|p| p.starts_with("repos/")).unwrap();
    assert_eq!(path, "repos/{owner}/{repo}/issues/7/reactions");
}

#[test]
fn a_template_with_a_slot_refuses_to_run_unfilled() {
    // An unfilled slot would reach `gh` as a literal `{}` and query nothing.
    let root = tempfile::tempdir().unwrap();
    let rejection =
        spec_from_template("pr-metadata", root.path(), &[]).expect_err("ran with an empty slot");
    assert!(rejection.0.contains("slot"));
}

#[test]
fn a_slot_still_only_accepts_the_declared_shape() {
    // Substitution into a URL path is only safe because the value was checked
    // first. This is the assertion that keeps those two facts tied together.
    let root = tempfile::tempdir().unwrap();
    for bad in ["138/../../admin", "a;b", "--paginate"] {
        assert!(
            spec_from_template("pr-review-comments", root.path(), &args(&[bad])).is_err(),
            "`{bad}` reached a URL path"
        );
    }
}

// ── deterministic review collection ─────────────────────────────────

#[test]
fn every_review_collector_exists_and_needs_no_agent() {
    // The DoD is that a review's inputs are fetched without an agent pass. Each
    // collector must therefore exist and report through `Collected`, which keeps
    // the payload in the artifact instead of the summary.
    for id in REVIEW_COLLECTORS {
        let found = template(id).unwrap_or_else(|| panic!("`{id}` is missing"));
        assert!(
            matches!(
                found.summariser,
                Summariser::Collected | Summariser::Generic
            ),
            "`{id}` summarises as {:?}",
            found.summariser
        );
    }
}

#[test]
fn the_collectors_cover_pr_diff_comments_checks_and_reactions() {
    // Named individually so dropping one is a test failure rather than a quietly
    // thinner payload.
    let ids: Vec<&str> = REVIEW_COLLECTORS.to_vec();
    for expected in [
        "pr-metadata",
        "pr-changed-files",
        "pr-review-comments",
        "pr-issue-comments",
        "pr-checks",
        "pr-reactions",
    ] {
        assert!(ids.contains(&expected), "`{expected}` is not collected");
    }
}

#[test]
fn a_collector_never_writes_to_the_forge() {
    // These run unattended. A collector that could POST would make an unattended
    // review a write path.
    for id in REVIEW_COLLECTORS {
        let found = template(id).unwrap();
        for part in found.base_argv {
            assert!(
                !matches!(*part, "-X" | "--method" | "-f" | "--field" | "--input"),
                "`{id}` carries `{part}`, which can turn a read into a write"
            );
        }
    }
}

#[test]
fn a_template_carries_its_own_timeout() {
    // A full suite and a `gh` call have nothing in common here; a single default
    // would either kill the suite or let a hung API call sit for half an hour.
    let full = template("backend-tests-full").unwrap();
    let checks = template("pr-checks").unwrap();
    assert!(full.timeout_secs > checks.timeout_secs * 4);
}
