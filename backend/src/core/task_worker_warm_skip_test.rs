//! KT-1980 — the warm-skip decision, a pure function over the execution's
//! validations and the project's declared first-build command.
use super::*;

fn commands(list: &[&str]) -> Vec<String> {
    list.iter().map(|command| command.to_string()).collect()
}

fn skipped(list: &[&str], declared: Option<&str>) -> WarmSkip {
    match decide(&commands(list), declared) {
        WarmDecision::Skip(skip) => skip,
        WarmDecision::Warm(why) => panic!("{list:?} should skip the warm-up, warmed: {why}"),
    }
}

fn warmed(list: &[&str], declared: Option<&str>) -> String {
    match decide(&commands(list), declared) {
        WarmDecision::Warm(why) => why,
        WarmDecision::Skip(skip) => panic!("{list:?} should warm the lane, skipped: {skip:?}"),
    }
}

const FRONT: [&str; 4] = [
    "pnpm --dir frontend exec tsc -b --pretty false",
    "pnpm --dir frontend exec oxlint",
    "pnpm --dir frontend lint:i18n",
    "pnpm --dir frontend exec vitest run src/a.test.ts src/b.test.ts src/c.test.ts",
];

#[test]
fn front_only_validations_skip_the_warm_up_and_say_which_rule_and_validations() {
    let skip = skipped(&FRONT, None);
    assert_eq!(skip.validations, commands(&FRONT));
    assert!(skip.reason.contains(RULE), "{}", skip.reason);
    for command in FRONT {
        assert!(skip.reason.contains(command), "{}", skip.reason);
    }
    assert_eq!(skip.build_binaries, commands(&["cargo", "make"]));
}

#[test]
fn rtk_wrapped_front_validations_skip_too() {
    skipped(
        &[
            "rtk pnpm --dir frontend exec tsc",
            "rtk proxy npx vitest run",
        ],
        None,
    );
}

#[test]
fn no_validation_warms_because_the_intent_is_unknown() {
    let why = warmed(&[], None);
    assert!(why.contains("no validation"), "{why}");
}

#[test]
fn a_cargo_validation_warms() {
    warmed(&[FRONT[0], "cargo test --lib warm_skip"], None);
}

#[test]
fn a_make_validation_warms() {
    warmed(&["make check", FRONT[1]], None);
}

#[test]
fn an_rtk_wrapped_cargo_validation_warms() {
    warmed(&["rtk cargo clippy --all-targets -- -D warnings"], None);
    warmed(&["rtk proxy cargo fmt --check"], None);
}

#[test]
fn a_cargo_given_as_a_path_still_warms() {
    warmed(&["/usr/local/bin/cargo test"], None);
}

#[test]
fn an_unknown_binary_warms_when_in_doubt() {
    let why = warmed(&[FRONT[0], "python3 scripts/check.py"], None);
    assert!(why.contains("python3"), "{why}");
    // A repository executable may do anything, even under a known name.
    warmed(&["./node_modules/.bin/vitest run"], None);
}

#[test]
fn a_build_binary_named_further_on_the_command_line_warms() {
    warmed(
        &["pnpm --dir backend exec cargo test --manifest-path backend/Cargo.toml"],
        None,
    );
}

#[test]
fn the_binary_of_the_declared_first_build_command_warms() {
    // The project builds its warm unit with `npm ci`: an npm gate needs it.
    let why = warmed(&["npm test"], Some("npm ci"));
    assert!(why.contains("npm"), "{why}");
    // Through env prefixes, an rtk wrapper, `cd` and chained commands.
    let declared = Some("cd app && FOO=1 rtk proxy composer install --no-interaction");
    warmed(&["composer test"], declared);
    assert_eq!(
        build_binaries(declared),
        commands(&["cargo", "make", "composer"])
    );
    // Not named by the declared command: the rule still knows it does not build.
    skipped(&["pnpm test"], Some("composer install"));
}

#[test]
fn a_skip_refuses_later_gates_that_need_a_build_and_accepts_the_others() {
    let skip = skipped(&FRONT, Some("npm ci"));
    assert!(skip.refusal_for(&commands(&[FRONT[2]])).is_none());
    let refusal = skip
        .refusal_for(&commands(&[FRONT[2], "cargo test"]))
        .expect("a cargo gate needs the warm lane");
    assert!(refusal.contains("cargo test"), "{refusal}");
    assert!(skip.refusal_for(&commands(&["npm run build"])).is_some());
    assert!(skip.refusal_for(&commands(&["bash run.sh"])).is_some());
}

#[test]
fn the_skip_survives_the_journal() {
    let skip = skipped(&FRONT, None);
    let json = serde_json::to_value(&skip).unwrap();
    assert_eq!(serde_json::from_value::<WarmSkip>(json).unwrap(), skip);
}
