use super::*;
use std::path::PathBuf;

fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/db.rs"),
        "fn a() {}\nfn b() {}\nlet q = format!(\"SELECT * FROM t WHERE id = {}\", id);\nconn.execute(&q);\nfn c() {}\nfn d() {}\nfn e() {}\nfn f() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/guard.rs"),
        "pub fn check(id: &str) -> bool {\n    id.chars().all(|c| c.is_ascii_digit())\n}\n",
    )
    .unwrap();
    (dir, root)
}

fn cite(file: &str, line: u32, quote: &str) -> Citation {
    Citation {
        file: file.into(),
        line,
        line_end: None,
        quote: quote.into(),
    }
}

fn finding(title: &str, file: &str, line: u32, quote: &str) -> RawFinding {
    RawFinding {
        title: title.into(),
        category: "security".into(),
        severity: "High".into(),
        citation: cite(file, line, quote),
        impact: "an attacker reads the table".into(),
        remediation: "bind the parameter".into(),
        td_id: None,
    }
}

fn group(id: &str, members: Vec<(AgentType, RawFinding)>) -> FindingGroup {
    FindingGroup {
        id: id.into(),
        members: members
            .into_iter()
            .map(|(author, finding)| AgentFinding { author, finding })
            .collect(),
    }
}

// ── option ──

#[test]
fn the_briefing_option_needs_two_or_three_distinct_eligible_agents() {
    let line =
        |list: &str| format!("# Brief\n\n## Kronn options\n{BRIEFING_OPTION_PREFIX} {list}\n");
    assert_eq!(
        briefing_cross_agents(&line("ClaudeCode, Codex")),
        Some(vec![AgentType::ClaudeCode, AgentType::Codex])
    );
    assert_eq!(
        briefing_cross_agents(&line("ClaudeCode, Codex, OpenCode")).map(|a| a.len()),
        Some(3)
    );
    assert_eq!(
        briefing_cross_agents(&line("Codex, Codex, ClaudeCode")),
        Some(vec![AgentType::Codex, AgentType::ClaudeCode])
    );
    assert_eq!(briefing_cross_agents(&line("ClaudeCode")), None);
    assert_eq!(
        briefing_cross_agents(&line("ClaudeCode, Codex, OpenCode, Kiro")),
        None
    );
    assert_eq!(briefing_cross_agents(&line("ClaudeCode, Custom")), None);
    assert_eq!(briefing_cross_agents(&line("ClaudeCode, Vibe")), None);
    assert_eq!(briefing_cross_agents(&line("ClaudeCode, Nobody")), None);
    assert_eq!(briefing_cross_agents("# Brief\nno option\n"), None);
}

#[test]
fn the_option_line_round_trips() {
    let agents = vec![AgentType::Codex, AgentType::GeminiCli];
    assert_eq!(
        briefing_cross_agents(&briefing_option_line(&agents)),
        Some(agents)
    );
}

#[test]
fn only_finding_steps_are_cross_checked() {
    assert!(is_cross_checked_target("docs/inconsistencies-security.md"));
    for doc in [
        "docs/AGENTS.md",
        "docs/glossary.md",
        "docs/repo-map.md",
        "REVIEW",
    ] {
        assert!(!is_cross_checked_target(doc), "{doc}");
    }
}

// ── blocks ──

#[test]
fn the_last_fenced_block_is_parsed() {
    let text = "Format:\n```kronn-findings\n{\"findings\":[{\"title\":\"x\"}]}\n```\nAnswer:\n```kronn-findings\n{\"findings\":[]}\n```\n";
    let out: DetectionOutput = parse_fenced(text, FINDINGS_FENCE).unwrap();
    assert!(out.findings.is_empty());
    assert!(parse_fenced::<DetectionOutput>("no block", FINDINGS_FENCE).is_err());
    assert!(
        parse_fenced::<DetectionOutput>("```kronn-findings\nnot json\n```", FINDINGS_FENCE)
            .is_err()
    );
    assert!(parse_fenced::<DetectionOutput>("```kronn-findings\n{}\n", FINDINGS_FENCE).is_err());
}

// ── citation check ──

#[test]
fn a_citation_checks_file_line_and_quote() {
    let (_d, root) = repo();
    assert!(verify_citation(&root, &cite("src/db.rs", 3, "SELECT * FROM t WHERE")).is_ok());
    assert!(verify_citation(&root, &cite("./src/db.rs", 3, "SELECT   *  FROM t")).is_ok());
    // Within the tolerance, then beyond it.
    assert!(verify_citation(&root, &cite("src/db.rs", 5, "SELECT * FROM t")).is_ok());
    assert!(verify_citation(&root, &cite("src/db.rs", 6, "SELECT * FROM t")).is_err());
    assert!(verify_citation(&root, &cite("src/missing.rs", 1, "fn a()")).is_err());
    assert!(verify_citation(&root, &cite("src/db.rs", 400, "fn a()")).is_err());
    assert!(verify_citation(&root, &cite("src/db.rs", 0, "fn a()")).is_err());
    assert!(verify_citation(&root, &cite("src/db.rs", 3, "DROP TABLE")).is_err());
    assert!(verify_citation(&root, &cite("src/db.rs", 3, "fn")).is_err());
    assert!(verify_citation(&root, &cite("../etc/passwd", 1, "root:x")).is_err());
    assert!(verify_citation(&root, &cite("/etc/passwd", 1, "root:x")).is_err());
}

#[test]
fn a_proof_stands_only_when_every_citation_verifies() {
    let (_d, root) = repo();
    let good = cite("src/guard.rs", 2, "is_ascii_digit");
    let bad = cite("src/guard.rs", 2, "sanitize_sql");
    assert!(verify_proof(&root, std::slice::from_ref(&good)).is_ok());
    assert!(verify_proof(&root, &[]).is_err());
    assert!(verify_proof(&root, &[good, bad]).is_err());
}

#[test]
fn a_false_citation_drops_the_finding_before_any_model() {
    let (_d, root) = repo();
    let detections = vec![
        (
            AgentType::ClaudeCode,
            DetectionOutput {
                findings: vec![
                    finding("SQL built by format", "src/db.rs", 3, "format!(\"SELECT"),
                    finding("Invented file", "src/nope.rs", 3, "anything here"),
                ],
                checklist: vec![],
            },
        ),
        (
            AgentType::Codex,
            DetectionOutput {
                findings: vec![finding("Wrong quote", "src/db.rs", 3, "eval(user_input)")],
                checklist: vec![],
            },
        ),
    ];
    let (kept, dropped) = filter_findings(&root, &detections);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].finding.title, "SQL built by format");
    let titles: Vec<&str> = dropped.iter().map(|d| d.finding.title.as_str()).collect();
    assert_eq!(titles, vec!["Invented file", "Wrong quote"]);
}

// ── matching ──

#[test]
fn matching_groups_one_thing_and_never_merges_one_agents_findings() {
    let near = |a: AgentType, line: u32, cat: &str| AgentFinding {
        author: a,
        finding: RawFinding {
            category: cat.into(),
            ..finding("t", "src/db.rs", line, "q")
        },
    };
    let groups = match_findings(vec![
        near(AgentType::ClaudeCode, 3, "security"),
        near(AgentType::Codex, 8, "Security"),
        near(AgentType::ClaudeCode, 4, "security"),
        near(AgentType::OpenCode, 3, "performance"),
        near(AgentType::OpenCode, 40, "security"),
    ]);
    let shape: Vec<(String, Vec<AgentType>)> = groups
        .iter()
        .map(|g| (g.id.clone(), g.detected_by()))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("F1".into(), vec![AgentType::ClaudeCode, AgentType::Codex]),
            ("F2".into(), vec![AgentType::ClaudeCode]),
            ("F3".into(), vec![AgentType::OpenCode]),
            ("F4".into(), vec![AgentType::OpenCode]),
        ]
    );
}

// ── challengers ──

#[test]
fn a_challenger_never_authored_and_preferably_never_detected_the_finding() {
    let agents = vec![AgentType::ClaudeCode, AgentType::Codex, AgentType::OpenCode];
    let f = || finding("t", "src/db.rs", 3, "q");
    let groups = vec![
        group("F1", vec![(AgentType::ClaudeCode, f())]),
        group(
            "F2",
            vec![(AgentType::ClaudeCode, f()), (AgentType::Codex, f())],
        ),
        group(
            "F3",
            vec![
                (AgentType::Codex, f()),
                (AgentType::OpenCode, f()),
                (AgentType::ClaudeCode, f()),
            ],
        ),
    ];
    let (assigned, unchecked) = assign_challengers(&groups, &agents);
    assert!(unchecked.is_empty());
    assert_eq!(assigned["F1"], AgentType::Codex);
    assert_eq!(assigned["F2"], AgentType::OpenCode);
    // Every agent detected F3: someone other than its author checks it.
    assert_eq!(assigned["F3"], AgentType::OpenCode);
}

#[test]
fn the_cap_leaves_the_least_severe_findings_unchecked() {
    let agents = vec![AgentType::ClaudeCode, AgentType::Codex];
    let mut groups: Vec<FindingGroup> = (0..MAX_CROSS_CHECKED)
        .map(|i| {
            group(
                &format!("F{}", i + 1),
                vec![(AgentType::ClaudeCode, finding("t", "a", 1, "q"))],
            )
        })
        .collect();
    let mut low = finding("t", "a", 1, "q");
    low.severity = "Low".into();
    groups.insert(0, group("F0", vec![(AgentType::Codex, low)]));
    let (assigned, unchecked) = assign_challengers(&groups, &agents);
    assert_eq!(unchecked, vec!["F0".to_string()]);
    assert_eq!(assigned.len(), MAX_CROSS_CHECKED);
}

// ── verdicts, replies, arbitration ──

#[test]
fn a_refutation_without_verified_proof_only_contests() {
    let (_d, root) = repo();
    let refute = |proof: Vec<Citation>| Verdict {
        id: "F1".into(),
        verdict: VerdictKind::Refute,
        argument: "the id is validated".into(),
        proof,
    };
    let none = evaluate_verdict(&root, Some(&refute(vec![])), None);
    assert!(matches!(none, ChallengeResult::RefutedUnproven { .. }));
    let false_proof = evaluate_verdict(
        &root,
        Some(&refute(vec![cite("src/guard.rs", 2, "escape_sql(")])),
        None,
    );
    assert!(matches!(
        false_proof,
        ChallengeResult::RefutedUnproven { .. }
    ));
    let proven = evaluate_verdict(
        &root,
        Some(&refute(vec![cite("src/guard.rs", 2, "is_ascii_digit")])),
        None,
    );
    assert!(matches!(proven, ChallengeResult::RefutedProven { .. }));
    assert_eq!(arbitrate(Some(&none), None), Outcome::Contested);
    assert_eq!(arbitrate(Some(&false_proof), None), Outcome::Contested);
    assert!(Outcome::Contested.in_audit());
    assert_eq!(arbitrate(Some(&proven), None), Outcome::Dismissed);
    assert!(!Outcome::Dismissed.in_audit());
    assert!(needs_reply(&proven) && !needs_reply(&none));
}

#[test]
fn the_reply_decides_between_dismissal_and_a_human() {
    let (_d, root) = repo();
    let proven = ChallengeResult::RefutedProven {
        argument: "validated".into(),
        proof: vec![cite("src/guard.rs", 2, "is_ascii_digit")],
    };
    let reply = |stance: Stance, proof: Vec<Citation>| Reply {
        id: "F1".into(),
        stance,
        argument: "the guard is never called".into(),
        proof,
    };
    let maintained = evaluate_reply(
        &root,
        Some(&reply(
            Stance::Maintain,
            vec![cite("src/db.rs", 4, "conn.execute(&q)")],
        )),
        None,
    );
    assert_eq!(
        arbitrate(Some(&proven), Some(&maintained)),
        Outcome::Disputed
    );
    let unproven = evaluate_reply(
        &root,
        Some(&reply(
            Stance::Maintain,
            vec![cite("src/db.rs", 4, "check(id)")],
        )),
        None,
    );
    assert_eq!(
        arbitrate(Some(&proven), Some(&unproven)),
        Outcome::Dismissed
    );
    let withdrawn = evaluate_reply(&root, Some(&reply(Stance::Withdraw, vec![])), None);
    assert_eq!(
        arbitrate(Some(&proven), Some(&withdrawn)),
        Outcome::Dismissed
    );
    let silent = evaluate_reply(&root, None, Some("agent failed"));
    assert_eq!(arbitrate(Some(&proven), Some(&silent)), Outcome::Dismissed);
    assert!(Outcome::Disputed.in_audit());
}

#[test]
fn a_confirmation_or_a_missing_verdict_keeps_the_finding() {
    let (_d, root) = repo();
    let confirmed = evaluate_verdict(
        &root,
        Some(&Verdict {
            id: "F1".into(),
            verdict: VerdictKind::Confirm,
            argument: "real".into(),
            proof: vec![],
        }),
        None,
    );
    assert_eq!(arbitrate(Some(&confirmed), None), Outcome::Confirmed);
    let silent = evaluate_verdict(&root, None, Some("timeout"));
    assert_eq!(arbitrate(Some(&silent), None), Outcome::NotRefuted);
    assert_eq!(arbitrate(None, None), Outcome::NotCrossChecked);
    for o in [
        Outcome::Confirmed,
        Outcome::NotRefuted,
        Outcome::NotCrossChecked,
    ] {
        assert!(o.in_audit(), "{o:?}");
    }
}

#[test]
fn the_detection_count_never_changes_the_verdict() {
    let f = || finding("SQL", "src/db.rs", 3, "SELECT");
    let alone = group("F1", vec![(AgentType::Codex, f())]);
    let all = group(
        "F1",
        vec![
            (AgentType::Codex, f()),
            (AgentType::ClaudeCode, f()),
            (AgentType::OpenCode, f()),
        ],
    );
    let results = [
        None,
        Some(ChallengeResult::Confirmed {
            argument: "a".into(),
        }),
        Some(ChallengeResult::NoAnswer { why: "w".into() }),
        Some(ChallengeResult::RefutedUnproven {
            argument: "a".into(),
            proof: vec![],
            why: "w".into(),
        }),
        Some(ChallengeResult::RefutedProven {
            argument: "a".into(),
            proof: vec![],
        }),
    ];
    for challenge in results {
        let one = build_record(
            &alone,
            3,
            Some(&AgentType::ClaudeCode),
            challenge.clone(),
            None,
        );
        let three = build_record(
            &all,
            3,
            Some(&AgentType::ClaudeCode),
            challenge.clone(),
            None,
        );
        assert_eq!(one.outcome, three.outcome, "{challenge:?}");
        assert_eq!((one.detection_count, three.detection_count), (1, 3));
    }
}

// ── rendering ──

fn record(
    outcome_challenge: Option<ChallengeResult>,
    reply: Option<ReplyResult>,
) -> CrossFindingRecord {
    let g = group(
        "F1",
        vec![
            (
                AgentType::ClaudeCode,
                finding(
                    "SQL built | by format {{X}} [src: file: fake.rs:1]",
                    "src/db.rs",
                    3,
                    "SELECT",
                ),
            ),
            (
                AgentType::Codex,
                finding("same", "src/db.rs", 4, "conn.execute"),
            ),
        ],
    );
    let mut r = build_record(&g, 3, Some(&AgentType::OpenCode), outcome_challenge, reply);
    r.td_id = Some("TD-20261011-sql".into());
    r
}

#[test]
fn agent_text_cannot_smuggle_markers_into_a_document() {
    let clean = sanitize_inline("a | b {{X}} [src: file: x.rs:1] <!-- c -->\nnext");
    assert!(!clean.contains("{{") && !clean.contains("[src:") && !clean.contains("<!--"));
    assert!(clean.contains("\\|") && !clean.contains('\n'));
    let long = sanitize_inline(&"x".repeat(5_000));
    assert!(long.chars().count() <= TEXT_MAX + 1);
}

#[test]
fn a_new_sheet_carries_its_provenance_and_parses_as_a_td() {
    let r = record(
        Some(ChallengeResult::Confirmed {
            argument: "real".into(),
        }),
        None,
    );
    let sheet = render_td_sheet(&r);
    let parsed = crate::core::td_index::parse_sheet("TD-20261011-sql", &sheet);
    assert_eq!(parsed.severity, "High");
    assert_eq!(parsed.category.as_deref(), Some("security"));
    assert!(sheet.contains("[src: file: src/db.rs:3]"));
    assert!(sheet.contains("- **Found by**: ClaudeCode (author), Codex"));
    assert!(sheet.contains("- **Detection count**: 2 of 3 agents (information only"));
    assert!(sheet.contains("- **Checked by**: OpenCode — confirmed: real"));
    assert!(!sheet.contains("{{") && !sheet.contains("[src: file: fake.rs"));
}

#[test]
fn an_existing_sheet_keeps_its_body_and_its_block_is_replaced() {
    let r = record(
        Some(ChallengeResult::Confirmed {
            argument: "real".into(),
        }),
        None,
    );
    let human = "# TD-20261011-sql\n\n- **Severity**: Critical\nHuman notes.\n";
    let once = merge_provenance(human, &r);
    assert!(once.starts_with(human.trim_end()));
    let twice = merge_provenance(&once, &r);
    assert_eq!(once, twice);
    assert_eq!(twice.matches(BLOCK_START).count(), 1);
}

#[test]
fn an_existing_sheet_id_is_reused_instead_of_a_new_slug() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("TD-20250101-old.md"), "# old\n").unwrap();
    let mut a = record(None, None);
    a.proposed_td_id = Some("TD-20250101-old".into());
    let mut b = record(None, None);
    b.proposed_td_id = Some("TD-20250101-missing".into());
    let mut c = record(None, None);
    c.title = b.title.clone();
    let mut dismissed = record(
        Some(ChallengeResult::RefutedProven {
            argument: "a".into(),
            proof: vec![],
        }),
        None,
    );
    dismissed.td_id = None;
    let mut records = vec![a, b, c, dismissed];
    assign_td_ids(&mut records, dir.path(), "20261011");
    assert_eq!(records[0].td_id.as_deref(), Some("TD-20250101-old"));
    let b_id = records[1].td_id.clone().unwrap();
    assert!(
        b_id.starts_with("TD-20261011-sql-built-by-format"),
        "{b_id}"
    );
    assert_eq!(records[2].td_id.clone().unwrap(), format!("{b_id}-2"));
    assert_eq!(records[3].td_id, None);
}

#[test]
fn the_index_lists_audited_findings_and_logs_the_rest() {
    let (_d, root) = repo();
    let _ = root;
    let kept = record(
        Some(ChallengeResult::RefutedUnproven {
            argument: "no".into(),
            proof: vec![],
            why: "no proof cited".into(),
        }),
        None,
    );
    let mut gone = record(
        Some(ChallengeResult::RefutedProven {
            argument: "validated".into(),
            proof: vec![cite("src/guard.rs", 2, "is_ascii_digit")],
        }),
        Some(ReplyResult::Withdrawn {
            argument: "ok".into(),
        }),
    );
    gone.td_id = None;
    gone.title = "Gone finding".into();
    let dropped = vec![DroppedFinding {
        author: AgentType::Codex,
        finding: finding("Invented", "src/nope.rs", 9, "x"),
        reason: "`src/nope.rs:9`: not found".into(),
    }];
    let agents = vec![AgentType::ClaudeCode, AgentType::Codex, AgentType::OpenCode];
    let index = render_index(&IndexInput {
        heading: "Security findings (cross-agent audit)",
        agents: &agents,
        records: &[kept, gone],
        dropped: &dropped,
        checklists: &[(
            AgentType::ClaudeCode,
            vec![ChecklistLine {
                item: "HSTS".into(),
                status: "verified absent".into(),
            }],
        )],
        failed_agents: &[],
    });
    let table = index.split("## Cross-check verdicts").next().unwrap();
    assert!(table.contains("| TD-20261011-sql |") && table.contains("(contested)"));
    assert!(!table.contains("Gone finding"));
    let dismissed = index
        .split("## Dismissed with verified proof (logged)")
        .nth(1)
        .unwrap();
    assert!(
        dismissed.contains("Gone finding") && dismissed.contains("[src: file: src/guard.rs:2]")
    );
    assert!(dismissed.contains("withdrawn by the author"));
    let dropped_section = index
        .split("## Dropped by the citation check")
        .nth(1)
        .unwrap();
    assert!(dropped_section.contains("Invented") && !dropped_section.contains("[src:"));
    assert!(index.contains("HSTS — verified absent"));
    assert!(index.contains("information only: it never decided a verdict"));
}

#[test]
fn the_challenger_is_not_told_who_found_a_finding() {
    let f = finding("SQL", "src/db.rs", 3, "SELECT");
    let prompt = challenge_prompt(&AgentType::Codex, &[("F1", &f)]);
    assert!(prompt.contains("\"id\": \"F1\"") && prompt.contains(VERDICTS_FENCE));
    assert!(!prompt.contains("ClaudeCode") && !prompt.to_lowercase().contains("detection count"));
}

#[test]
fn a_dispute_card_holds_both_arguments_and_recommends_nothing() {
    let mut r = record(
        Some(ChallengeResult::RefutedProven {
            argument: "AGAINST-ARG".into(),
            proof: vec![cite("src/guard.rs", 2, "is_ascii_digit")],
        }),
        Some(ReplyResult::MaintainedProven {
            argument: "FOR-ARG".into(),
            proof: vec![cite("src/db.rs", 4, "conn.execute")],
        }),
    );
    assert_eq!(r.outcome, Outcome::Disputed);
    assert_eq!(dispute_card(&r), None, "no key, no card");
    r.question_key = Some(question_key("run-1234-5678", &r));
    let card = dispute_card(&r).unwrap();
    let json = card
        .strip_prefix("```kronn-question\n")
        .and_then(|c| c.strip_suffix("\n```"))
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(spec["version"], 1);
    assert_eq!(spec["key"], "cross-audit.run12345.TD-20261011-sql");
    let context = spec["context"].as_str().unwrap();
    assert!(context.contains("FOR-ARG") && context.contains("src/db.rs:4"));
    assert!(context.contains("AGAINST-ARG") && context.contains("src/guard.rs:2"));
    assert_eq!(spec["recommended_option_ids"], serde_json::json!([]));
}
