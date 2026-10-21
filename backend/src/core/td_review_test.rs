use super::*;

fn sheet(severity: &str, status: &str) -> String {
    format!(
        "---\nname: td-x\ndescription: Le cache fuit — café ☕\nmetadata:\n  type: tech-debt\n---\n\n# TD\n\n\
- **Area**: Backend (cache)\n- **Severity**: {severity}\n- **Status**: {status}\n- **Effort**: M\n- **Blast radius**: Module\n\n\
## Problem (fact)\nThe cache never evicts.\n\n## Impact\nMemory grows.\n\n\
## Where (pointers)\n- `backend/src/cache.rs:42` — insert path\n- `frontend/src/App.tsx#L7`, see also https://example.com/x\n- docs/notes.md\n- `../etc/passwd:1`\n\n\
## Suggested direction\nEvict.\n\n## Next step\nMeasure.\n\n## Verification\nMemory stays flat.\n"
    )
}

fn item(id: &str, severity: &str, state: TdReviewState) -> TdReviewItem {
    let mut item = item_from(id, &sheet(severity, "Verified in source"), String::new());
    item.state = state;
    item
}

#[test]
fn a_sheet_yields_every_field_the_review_shows() {
    let item = item_from(
        "TD-20261001-cache",
        &sheet("High (perf)", "Verified in source"),
        "docs/tech-debt/TD-20261001-cache.md".into(),
    );
    assert_eq!(item.title, "Le cache fuit — café ☕");
    assert_eq!(item.severity, "High");
    assert_eq!(item.area, "Backend (cache)");
    assert_eq!(item.effort.as_deref(), Some("M"));
    assert_eq!(item.blast_radius.as_deref(), Some("Module"));
    assert_eq!(item.state, TdReviewState::Pending);
    assert_eq!(item.problem.as_deref(), Some("The cache never evicts."));
    assert_eq!(item.impact.as_deref(), Some("Memory grows."));
    assert_eq!(item.verification.as_deref(), Some("Memory stays flat."));
    let pointers: Vec<(String, Option<u32>)> = item
        .pointers
        .iter()
        .map(|p| (p.path.clone(), p.line))
        .collect();
    assert_eq!(
        pointers,
        [
            ("backend/src/cache.rs".to_string(), Some(42)),
            ("frontend/src/App.tsx".to_string(), Some(7)),
            ("docs/notes.md".to_string(), None),
        ],
        "URLs and parent escapes are not pointers"
    );
}

#[test]
fn every_decision_maps_to_its_state_and_only_a_real_decision_settles() {
    let cases = [
        ("Verified in source", TdReviewState::Pending, false),
        ("Confirmed by user", TdReviewState::Confirmed, true),
        ("Accepted decision", TdReviewState::AcceptedDecision, true),
        ("Deferred", TdReviewState::Deferred, false),
        ("Rejected", TdReviewState::Rejected, true),
    ];
    for (status, state, settled) in cases {
        let item = item_from("TD-1", &sheet("Low", status), String::new());
        assert_eq!(item.state, state, "{status}");
        assert_eq!(is_settled(item.state), settled, "{status}");
    }
}

#[test]
fn the_gate_needs_every_critical_and_high_settled_and_ignores_deferrals() {
    use TdReviewState::*;
    let low_pending = item("TD-4", "Low", Pending);
    let mut items = vec![
        item("TD-1", "Critical", Confirmed),
        item("TD-2", "High", Deferred),
        low_pending.clone(),
    ];
    let refused = gate(&items);
    assert_eq!((refused.blocking_total, refused.blocking_settled), (2, 1));
    assert!(!refused.met, "a deferred High does not count");
    items[1].state = AcceptedDecision;
    assert!(gate(&items).met, "a pending Low does not block");
    items[1].state = Pending;
    assert!(!gate(&items).met);
    items[1].state = Rejected;
    assert!(gate(&items).met);
    assert!(!gate(&[]).met, "no sheet at all is not a reviewed audit");
    assert!(gate(&[low_pending]).met);
}

#[test]
fn counts_report_states_and_remaining_per_severity() {
    use TdReviewState::*;
    let items = vec![
        item("TD-1", "Critical", Confirmed),
        item("TD-2", "Critical", Deferred),
        item("TD-3", "High", Pending),
        item("TD-4", "Medium", Rejected),
        item("TD-5", "weird", AcceptedDecision),
    ];
    let (counts, by_severity) = counts(&items);
    assert_eq!(
        (
            counts.pending,
            counts.confirmed,
            counts.accepted_decision,
            counts.deferred,
            counts.rejected
        ),
        (1, 1, 1, 1, 1)
    );
    let rows: Vec<(&str, u32, u32)> = by_severity
        .iter()
        .map(|c| (c.severity.as_str(), c.total, c.remaining))
        .collect();
    assert_eq!(
        rows,
        [
            ("Critical", 2, 1),
            ("High", 1, 1),
            ("Medium", 1, 0),
            ("Low", 0, 0),
            ("Unknown", 1, 0)
        ]
    );
}

#[test]
fn mentions_are_whole_tokens_and_excerpts_keep_the_context() {
    let text =
        "Voir TD-20261001-a et (TD-20261001-b).\nXTD-20261001-c n'en est pas un, TD- non plus.";
    let ids: Vec<String> = mentioned_td_ids(text).into_iter().collect();
    assert_eq!(ids, ["TD-20261001-a", "TD-20261001-b"]);
    assert!(!mentioned_td_ids("TD-20261001-ab").contains("TD-20261001-a"));

    let long = (0..20)
        .map(|i| {
            if i == 10 {
                "Sur TD-20261001-a : vrai problème.".to_string()
            } else {
                format!("ligne {i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (excerpt, cut) = mention_excerpt(&long, "TD-20261001-a", 500);
    assert!(cut);
    assert_eq!(
        excerpt,
        "ligne 9\nSur TD-20261001-a : vrai problème.\nligne 11"
    );
    let (whole, cut) = mention_excerpt("TD-20261001-a seul", "TD-20261001-a", 500);
    assert!(!cut);
    assert_eq!(whole, "TD-20261001-a seul");
}

#[test]
fn talk_titles_round_trip_and_the_prompt_carries_the_sheet() {
    let item = item_from(
        "TD-20261001-cache",
        &sheet("High", "Verified in source"),
        "docs/tech-debt/TD-20261001-cache.md".into(),
    );
    let title = talk_title(&item);
    assert_eq!(talk_td_id(&title), Some("TD-20261001-cache"));
    assert_eq!(talk_td_id("Validation audit AI"), None);
    assert_eq!(talk_td_id("TD · ../x"), None);
    let prompt = talk_prompt(&item, "  Est-ce vraiment un bug ?  ");
    assert!(prompt.contains("`TD-20261001-cache` (High, Backend (cache))"));
    assert!(prompt.contains("Where: backend/src/cache.rs:42, frontend/src/App.tsx:7"));
    assert!(prompt.ends_with("Est-ce vraiment un bug ?"));
}

#[test]
fn read_items_lists_sheets_only_and_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let td = dir.path().join("tech-debt");
    std::fs::create_dir_all(&td).unwrap();
    std::fs::write(td.join("TD-2.md"), sheet("Low", "Rejected")).unwrap();
    std::fs::write(td.join("TD-1.md"), sheet("Critical", "Verified")).unwrap();
    std::fs::write(td.join("README.md"), "not a sheet").unwrap();
    let items = read_items(dir.path());
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["TD-1", "TD-2"]);
    let name = dir.path().file_name().unwrap().to_str().unwrap();
    assert_eq!(items[0].source_path, format!("{name}/tech-debt/TD-1.md"));
    assert!(read_items(&dir.path().join("missing")).is_empty());
}
