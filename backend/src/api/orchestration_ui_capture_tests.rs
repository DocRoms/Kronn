//! KT-1682 — a worker that changes the UI is asked for before / after captures
//! (or the reason none could be taken), whatever the transport and language,
//! and the review hand-back asks to retake them after a fix.

use super::*;

#[test]
fn briefs_ask_for_ui_captures_before_the_tests_section() {
    for lang in [RoomLang::Fr, RoomLang::En] {
        for (can_run_shell, projection) in [(true, true), (true, false), (false, true)] {
            let brief = worker_brief_markdown_in(
                lang,
                "KT-1682",
                "Captures",
                "Objective",
                &[],
                "/wt/kt1682",
                "kronn/task/KT-1682",
                "abc1234",
                can_run_shell,
                projection,
                None,
            );
            let needles: &[&str] = match lang {
                RoomLang::Fr => &[
                    "## Captures d'UI",
                    "sous `frontend/`",
                    "avant (branche cible) / après (ton HEAD)",
                    "en clair et en sombre",
                    "docs/conventions/pull-requests.md",
                    "dis lequel et pourquoi dans `limitations`",
                ],
                RoomLang::En => &[
                    "## UI captures",
                    "under `frontend/`",
                    "before (target branch) / after (your HEAD)",
                    "light and dark",
                    "docs/conventions/pull-requests.md",
                    "say which and why in `limitations`",
                ],
            };
            for needle in needles {
                assert!(brief.contains(needle), "missing `{needle}`: {brief}");
            }
            // Before `## Tests`, so over ACP the brief still ends on its own
            // start directive (KT-1278).
            assert!(
                brief.find(needles[0]).unwrap() < brief.find("## Tests").unwrap(),
                "{brief}"
            );
        }
    }
}

#[test]
fn a_request_changes_hand_back_asks_to_retake_the_captures_of_a_changed_screen() {
    for lang in [RoomLang::Fr, RoomLang::En] {
        let decision = ReviewDecisionV1 {
            version: "1".into(),
            task_ref: "KT-1682".into(),
            decision: ReviewVerdict::RequestChanges,
            reviewed_head_sha: None,
            dod_verifications: vec![],
            comment: Some("Fix the header".into()),
            findings: vec![],
        };
        let message = build_review_findings_message("exec-1", 1, &decision, "child", lang);
        let retake = match lang {
            RoomLang::Fr => "reprends les captures concernées depuis le nouveau HEAD",
            RoomLang::En => "retake the affected captures from the new HEAD",
        };
        for needle in [
            "Fix the header",
            retake,
            "docs/conventions/pull-requests.md",
        ] {
            assert!(
                message.content.contains(needle),
                "missing `{needle}`: {}",
                message.content
            );
        }
    }
}
