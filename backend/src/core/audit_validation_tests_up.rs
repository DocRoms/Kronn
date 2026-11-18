use super::*;
use crate::core::td_sheet::comment_start;

fn td(status: &str) -> String {
    format!(
            "---\nname: td-test\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-09-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# TD-test\n\n- **Severity**: High\n- **Status**: {status}\n"
        )
}

/// The front matter of a sheet, through a strict YAML parser: Kronn's own
/// reader is lenient, a standard parser (Ruby, serde_yaml) is not.
fn strict_front_matter(content: &str) -> serde_yaml::Value {
    let rest = content
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("no front matter:\n{content}"));
    let end = rest
        .find("\n---")
        .unwrap_or_else(|| panic!("unterminated front matter:\n{content}"));
    serde_yaml::from_str(&rest[..end])
        .unwrap_or_else(|error| panic!("strict YAML parse failed: {error}\n{content}"))
}

#[test]
fn a_hash_inside_quotes_is_not_taken_for_a_comment() {
    // Was refused as "a metadata flow mapping on several lines".
    let input = "---\nname: td-x\nmetadata: {type: tech-debt, note: \"a #b\"}\n---\n\n# TD-x\n\n- **Status**: Inferred\n";
    let written = update_td(
        input,
        TdResolutionAction::Confirm,
        TdDecisionOrigin::Review,
        None,
    )
    .unwrap();
    assert_eq!(
        strict_front_matter(&written)["metadata"]["note"].as_str(),
        Some("a #b")
    );
    assert_eq!(comment_start("a: \"x # y\" # z"), Some(11));
    assert_eq!(comment_start("a: 'x # y'"), None);
    assert_eq!(comment_start("a: it's # z"), Some(8));
    assert_eq!(comment_start("a: b#c"), None);
}
