//! Tests for the Context Architecture Audit — KT-194.
//!
//! An audit is believed. So the cases that matter are the ones where it would be
//! confidently wrong: an unreadable file counted as free, a repository with no
//! convention reported as clean, a hard rule proposed for deletion, or a duplicate
//! that survives every pass because both copies look critical.

use super::*;

fn scanned(path: &str, content: &str) -> ScannedFile {
    ScannedFile {
        path: path.to_string(),
        content: Some(content.to_string()),
    }
}

fn existing(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

// ── inventory and precedence ────────────────────────────────────────

#[test]
fn a_file_is_attributed_to_every_agent_that_reads_it() {
    // AGENTS.md is shared. Attributing it to one agent would understate the cost
    // for the others.
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Rules\n\nSome guidance here for everyone.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    let agents = &audit.files[0].agents;
    for expected in ["ClaudeCode", "Codex", "GeminiCli", "Vibe"] {
        assert!(
            agents.contains(&expected.to_string()),
            "missing {expected}: {agents:?}"
        );
    }
}

#[test]
fn precedence_follows_the_conventions_own_order() {
    // Claude Code reads CLAUDE.local.md before CLAUDE.md. Reporting them
    // alphabetically would describe a loading order that does not happen.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nContent long enough to be a paragraph here.\n",
            ),
            scanned(
                "CLAUDE.local.md",
                "# B\n\nContent long enough to be a paragraph.\n",
            ),
        ],
        &existing(&["CLAUDE.md", "CLAUDE.local.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.files, vec!["CLAUDE.local.md", "CLAUDE.md"]);
}

#[test]
fn a_wildcard_convention_matches_one_level_only() {
    // `.cursor/rules/*` is a directory of rule files, not a tree. Matching deeper
    // would pull in whatever someone nested there.
    assert!(path_matches(".cursor/rules/*", ".cursor/rules/style.mdc"));
    assert!(!path_matches(
        ".cursor/rules/*",
        ".cursor/rules/deep/style.mdc"
    ));
    assert!(path_matches("CLAUDE.md", "CLAUDE.md"));
    assert!(!path_matches("CLAUDE.md", "docs/CLAUDE.md"));
}

#[test]
fn an_agent_with_no_file_here_is_reported_rather_than_omitted() {
    // "Windsurf loads nothing in this repo" matters when a teammate uses it.
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# A\n\nGuidance long enough to count as one.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    let windsurf = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "Windsurf")
        .unwrap();
    assert!(windsurf.files.is_empty());
    assert!(audit
        .findings
        .iter()
        .any(|f| f.where_ == "Windsurf" && f.what.contains("loads no instruction file")));
}

// ── absence is never a zero ─────────────────────────────────────────

#[test]
fn an_unreadable_file_has_unknown_cost_not_zero() {
    // THE rule. A file too large to read, or unreadable, still costs an agent
    // whatever it costs.
    let audit = analyse(
        vec![ScannedFile {
            path: "CLAUDE.md".into(),
            content: None,
        }],
        &existing(&["CLAUDE.md"]),
    );
    assert_eq!(audit.files[0].bytes, None, "unreadable became zero");
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(
        claude.total_bytes, None,
        "a total was reported over an unknown"
    );
    assert_eq!(claude.unreadable_files.len(), 1);
    assert!(audit
        .findings
        .iter()
        .any(|f| f.what.contains("unknown rather than zero")));
}

#[test]
fn one_unreadable_file_makes_the_whole_total_unknown() {
    // A partial sum presented as the total understates the cost by an unknown
    // amount, which is worse than saying nothing.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nA paragraph long enough to be counted here.\n",
            ),
            ScannedFile {
                path: "AGENTS.md".into(),
                content: None,
            },
        ],
        &existing(&["CLAUDE.md", "AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.total_bytes, None);
    assert!(render(&audit).contains("unknown"));
}

#[test]
fn a_repository_with_no_convention_is_not_a_clean_result() {
    // An empty audit reads as "nothing to fix". Here it means agents start blind.
    let audit = analyse(Vec::new(), &existing(&[]));
    assert!(audit.no_convention_found);
    let text = render(&audit);
    assert!(text.contains("no project context at all"), "got: {text}");
    assert!(!audit.findings.is_empty());
}

#[test]
fn having_no_convention_is_said_before_the_findings_list() {
    // The finding carries the same words, so the test above passes even without the
    // banner — it is position that the banner adds, and a findings list can be long
    // enough that the reader never reaches the one line that matters.
    let audit = analyse(Vec::new(), &existing(&[]));
    let text = render(&audit);
    let banner = text.find("no project context at all").expect("no banner");
    let findings = text.find("FINDINGS").expect("no findings section");
    assert!(
        banner < findings,
        "the reader meets the findings list before learning there is no convention"
    );
}

// ── classification ──────────────────────────────────────────────────

#[test]
fn a_hard_rule_is_critical() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Safety\n\nYou MUST never commit a secret to this repository, ever.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert_eq!(audit.files[0].sections[0].class, SectionClass::Critical);
}

#[test]
fn a_duplicate_is_caught_even_when_both_copies_state_a_hard_rule() {
    // THE case duplication survives. Classed critical in both places, it is never
    // proposed for a move and stays paid for twice on every session.
    // Deliberately past DUPLICATE_MIN_BYTES: a shorter paragraph is not treated as
    // duplication at all, and a fixture under the threshold would test the
    // threshold instead of the rule.
    let shared = "You MUST always run the full test suite before pushing anything, \
                  because a partial run has hidden real failures more than once in \
                  this repository and the cost of that is measured in hours rather \
                  than minutes, which is why the rule is stated so emphatically and \
                  repeated wherever someone might be tempted to skip it.";
    let audit = analyse(
        vec![
            scanned("AGENTS.md", &format!("# Testing\n\n{shared}\n")),
            scanned("CLAUDE.md", &format!("# Testing\n\n{shared}\n")),
        ],
        &existing(&["AGENTS.md", "CLAUDE.md"]),
    );
    assert!(
        audit.files.iter().any(|file| file
            .sections
            .iter()
            .any(|s| s.class == SectionClass::Duplicated)),
        "a duplicated hard rule was classed critical in both files"
    );
}

#[test]
fn a_short_repeated_line_is_not_reported_as_duplication() {
    // Headings and one-line notes repeat for ordinary reasons. Flagging them buries
    // the real cases.
    let short = "Run the tests.";
    let audit = analyse(
        vec![
            scanned(
                "AGENTS.md",
                &format!("# A\n\n{short}\n\nMore content to fill this section out.\n"),
            ),
            scanned(
                "CLAUDE.md",
                &format!("# B\n\n{short}\n\nOther content to fill this one out.\n"),
            ),
        ],
        &existing(&["AGENTS.md", "CLAUDE.md"]),
    );
    assert!(!audit.files.iter().any(|file| file
        .sections
        .iter()
        .any(|s| s.class == SectionClass::Duplicated)));
}

#[test]
fn a_dead_reference_makes_a_section_possibly_obsolete() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Layout\n\nThe deployment steps live in [the runbook](docs/runbook.md) \
             and should be followed exactly as written there.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert_eq!(
        audit.files[0].sections[0].class,
        SectionClass::PossiblyObsolete
    );
    assert!(audit.files[0]
        .broken_links
        .contains(&"docs/runbook.md".to_string()));
}

#[test]
fn a_dated_incident_narrative_is_historical() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# History\n\nOn 2026-06-30 an incident lost every stored token because a \
             key was regenerated silently, and the config was clobbered by a bash \
             script that rewrote it in place.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert_eq!(audit.files[0].sections[0].class, SectionClass::Historical);
}

#[test]
fn a_date_without_a_narrative_is_not_historical() {
    // A rule that happens to cite a date is still a rule. Classing it historical
    // would propose archiving something load-bearing.
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Deps\n\nPin every dependency added after 2026-01-01 to an exact \
             version, with no caret range at all.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert_ne!(audit.files[0].sections[0].class, SectionClass::Historical);
}

#[test]
fn a_pointer_is_routed_even_when_what_it_points_at_is_mandatory() {
    // A pointer is the cheap form and the mechanism tiering relies on; classing it
    // critical would keep the pointed-at content inline forever.
    let audit = analyse(
        vec![scanned(
            "CLAUDE.md",
            "# Start\n\nYou MUST read [the agent guide](AGENTS.md) first.\n",
        )],
        &existing(&["CLAUDE.md", "AGENTS.md"]),
    );
    assert_eq!(audit.files[0].sections[0].class, SectionClass::Routed);
}

// ── the proposal never touches a hard rule, and never writes ─────────

#[test]
fn a_critical_section_is_never_proposed_for_a_move() {
    // Deciding a hard rule is not needed up front is a human's call. However large
    // it is, a heuristic must not put it on the list.
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            &format!(
                "# Safety\n\nYou MUST never do this. {}\n",
                "x".repeat(5_000)
            ),
        )],
        &existing(&["AGENTS.md"]),
    );
    assert_eq!(audit.files[0].sections[0].class, SectionClass::Critical);
    assert!(
        audit.proposal.is_empty(),
        "a hard rule was proposed: {:?}",
        audit.proposal
    );
}

#[test]
fn only_movable_classes_reach_the_proposal() {
    for class in [
        SectionClass::Critical,
        SectionClass::Universal,
        SectionClass::Routed,
    ] {
        assert!(!class.is_movable(), "{class:?} was movable");
    }
    for class in [
        SectionClass::Historical,
        SectionClass::Duplicated,
        SectionClass::PossiblyObsolete,
    ] {
        assert!(class.is_movable(), "{class:?} was not movable");
    }
}

#[test]
fn the_proposal_puts_the_biggest_saving_first() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            &format!(
                "# Small\n\nOn 2026-01-01 an incident happened here briefly.\n\
                 # Big\n\nOn 2026-02-02 an incident happened. {}\n",
                "y".repeat(3_000)
            ),
        )],
        &existing(&["AGENTS.md"]),
    );
    assert!(audit.proposal.len() >= 2);
    assert!(audit.proposal[0].bytes > audit.proposal[1].bytes);
}

#[test]
fn the_report_says_the_proposal_is_a_proposal() {
    // The audit has no apply path; the text must not read as a changelog of moves
    // already made.
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# History\n\nOn 2026-06-30 an incident lost the stored tokens entirely.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert!(render(&audit).contains("nothing is rewritten"));
}

// ── routes and cycles ───────────────────────────────────────────────

#[test]
fn a_redirect_cycle_is_reported() {
    // Costs an agent the whole loop before it starts, and is invisible in either
    // file on its own.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the guide](AGENTS.md) for the details here.\n",
            ),
            scanned(
                "AGENTS.md",
                "# B\n\nRead [the claude file](CLAUDE.md) for the details.\n",
            ),
        ],
        &existing(&["CLAUDE.md", "AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert!(!claude.redirect_cycles.is_empty(), "no cycle found");
    assert!(audit
        .findings
        .iter()
        .any(|f| f.what.contains("redirect cycle")));
}

#[test]
fn a_file_naming_its_own_path_is_not_a_cycle() {
    // Found on a real repository: `docs/AGENTS.md` mentions its own path, and the
    // audit reported that as a redirect cycle once per agent — 11 findings for
    // something that costs nothing, since the file is already loaded.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the guide](docs/AGENTS.md) for the project details here.\n",
            ),
            scanned(
                "docs/AGENTS.md",
                "# B\n\nThis file, [docs/AGENTS.md](docs/AGENTS.md), is the entry point.\n",
            ),
        ],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    );
    assert!(
        audit
            .per_agent
            .iter()
            .all(|context| context.redirect_cycles.is_empty()),
        "a self-reference was reported as a cycle: {:?}",
        audit
            .per_agent
            .iter()
            .map(|c| &c.redirect_cycles)
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_plain_chain_is_not_reported_as_a_cycle() {
    // Otherwise every tiered setup would report a false error, and the real ones
    // would be ignored.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the guide](AGENTS.md) for the details here.\n",
            ),
            scanned(
                "AGENTS.md",
                "# B\n\nGuidance that points nowhere in particular.\n",
            ),
        ],
        &existing(&["CLAUDE.md", "AGENTS.md"]),
    );
    assert!(audit
        .per_agent
        .iter()
        .all(|context| context.redirect_cycles.is_empty()));
}

#[test]
fn a_url_is_not_treated_as_a_broken_repo_path() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Links\n\nSee [the docs](https://example.com/guide) for background reading.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

#[test]
fn an_anchor_is_not_treated_as_a_broken_repo_path() {
    let audit = analyse(
        vec![scanned(
            "AGENTS.md",
            "# Links\n\nSee [the section below](#testing) for the full explanation of it.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

#[test]
fn a_link_with_an_anchor_resolves_to_its_file() {
    // `docs/AGENTS.md#tier-1` points at a file that exists. Reporting it broken
    // would fill the findings with noise and hide the real ones.
    let audit = analyse(
        vec![scanned(
            "CLAUDE.md",
            "# Links\n\nSee [tier 1](docs/AGENTS.md#tier-1) for what loads every time.\n",
        )],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

// ── link resolution ─────────────────────────────────────────────────
//
// Every test above used root-level files, so all of them passed while the audit
// resolved links from the wrong base. Run against the real Kronn repository it
// reported 8 of 8 findings falsely — `docs/AGENTS.md` links to
// `conventions/agents-md-format-v1.md`, which is `docs/conventions/…` and exists —
// and then proposed archiving 8 sections on the strength of them. These are the
// tests that would have caught it.

#[test]
fn a_link_resolves_relative_to_the_file_that_contains_it() {
    // The rule every markdown renderer uses. This is the case that made the audit
    // confidently wrong on a real repository.
    let audit = analyse(
        vec![scanned(
            "docs/AGENTS.md",
            "# Index\n\nSee [the format](conventions/format-v1.md) for the details.\n",
        )],
        &existing(&["docs/AGENTS.md", "docs/conventions/format-v1.md"]),
    );
    assert!(
        audit.files[0].broken_links.is_empty(),
        "a valid relative link was reported broken: {:?}",
        audit.files[0].broken_links
    );
    assert_ne!(
        audit.files[0].sections[0].class,
        SectionClass::PossiblyObsolete,
        "a live section was proposed for the archive"
    );
}

#[test]
fn a_parent_traversal_in_a_link_resolves() {
    let audit = analyse(
        vec![scanned(
            "docs/AGENTS.md",
            "# Up\n\nSee [the root guide](../CLAUDE.md) for the session rules here.\n",
        )],
        &existing(&["docs/AGENTS.md", "CLAUDE.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

#[test]
fn a_root_relative_link_without_a_slash_is_accepted_too() {
    // Some repositories write links from the repo root by convention. Reporting
    // every one of them as broken would make the audit unusable there, so a target
    // that resolves either way counts as live.
    let audit = analyse(
        vec![scanned(
            "docs/AGENTS.md",
            "# Root\n\nSee [the readme](README.md) for the overview of the project.\n",
        )],
        &existing(&["docs/AGENTS.md", "README.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

#[test]
fn a_leading_slash_resolves_from_the_repository_root() {
    let audit = analyse(
        vec![scanned(
            "docs/AGENTS.md",
            "# Abs\n\nSee [the readme](/README.md) for the overview of the project.\n",
        )],
        &existing(&["docs/AGENTS.md", "README.md"]),
    );
    assert!(audit.files[0].broken_links.is_empty());
}

#[test]
fn a_genuinely_dead_relative_link_is_still_reported() {
    // The fix must not make the check toothless: a target that resolves nowhere,
    // from either base, is still a finding.
    let audit = analyse(
        vec![scanned(
            "docs/AGENTS.md",
            "# Gone\n\nSee [the runbook](operations/runbook.md) for the deploy steps.\n",
        )],
        &existing(&["docs/AGENTS.md"]),
    );
    assert_eq!(
        audit.files[0].broken_links,
        vec!["operations/runbook.md"],
        "a dead link stopped being reported"
    );
}

#[test]
fn a_redirect_target_is_stored_resolved_so_cycles_are_found_across_directories() {
    // `docs/AGENTS.md` → `../CLAUDE.md` → `docs/AGENTS.md` is a cycle. Storing the
    // target as written would compare `../CLAUDE.md` against `CLAUDE.md` and miss
    // it.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the guide](docs/AGENTS.md) for the project details here.\n",
            ),
            scanned(
                "docs/AGENTS.md",
                "# B\n\nRead [the claude file](../CLAUDE.md) for the session rules.\n",
            ),
        ],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert!(
        !claude.redirect_cycles.is_empty(),
        "a cycle across directories was missed"
    );
}

// ── what an agent really loads ───────────────────────────────────────
//
// Found on front_euronews, the repository in heaviest real use, and it is the
// pattern Kronn itself installs: nine ~200-byte root files each saying "read
// docs/AGENTS.md first", pointing at a 15 152 B tiered index. Counting only the
// entry points reported 421 B for Claude Code against a real 15 573 B —
// understated 36x — and the drift report called that index UNUSED. The 45 tests
// before this section all passed with the bug in place, because none of them had a
// redirect that led anywhere.

/// The front_euronews shape, minimised: a small entry point pointing at a big
/// index.
fn redirect_shape() -> ContextAudit {
    analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# Start\n\nYou MUST read [docs/AGENTS.md](docs/AGENTS.md) first — it is \
                 the single entry point for every agent working here.\n",
            ),
            scanned(
                "docs/AGENTS.md",
                &format!(
                    "# Index\n\nThe real context lives here. {}\n",
                    "x".repeat(4_000)
                ),
            ),
        ],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    )
}

#[test]
fn a_file_reached_only_through_a_redirect_is_still_paid_for() {
    let audit = redirect_shape();
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert!(
        claude.files.contains(&"docs/AGENTS.md".to_string()),
        "the redirect target is missing from the agent's context: {:?}",
        claude.files
    );
    assert!(
        claude.total_bytes.unwrap() > 4_000,
        "the total ignored the redirect target: {:?} B",
        claude.total_bytes
    );
}

#[test]
fn the_entry_point_comes_before_what_it_points_at() {
    // The list is read as load order. Reversing it would describe an agent reading
    // the index before the file that told it to.
    let audit = redirect_shape();
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.files[0], "CLAUDE.md");
    assert_eq!(claude.files[1], "docs/AGENTS.md");
}

#[test]
fn a_redirect_target_lists_the_agents_that_reach_it() {
    // `agents_reading` only knows convention patterns, so this file matched none
    // and reported zero — while every agent in the repository loads it.
    let audit = redirect_shape();
    let index = audit
        .files
        .iter()
        .find(|file| file.path == "docs/AGENTS.md")
        .unwrap();
    assert!(
        index.agents.contains(&"ClaudeCode".to_string()),
        "the most-loaded file claimed no readers: {:?}",
        index.agents
    );
}

#[test]
fn a_redirect_chain_is_followed_transitively() {
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the guide](AGENTS.md) before anything else.\n",
            ),
            scanned(
                "AGENTS.md",
                "# B\n\nThen read [the index](docs/AGENTS.md) for the real detail here.\n",
            ),
            scanned("docs/AGENTS.md", &format!("# C\n\n{}\n", "y".repeat(2_000))),
        ],
        &existing(&["CLAUDE.md", "AGENTS.md", "docs/AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(
        claude.files.len(),
        3,
        "chain not followed: {:?}",
        claude.files
    );
    assert!(claude.total_bytes.unwrap() > 2_000);
}

#[test]
fn a_target_two_entry_points_share_is_counted_once() {
    // Claude Code reads both CLAUDE.md and AGENTS.md. If both point at the index,
    // the agent still loads it once — charging twice would invent cost.
    let body = format!("# C\n\n{}\n", "z".repeat(1_000));
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the index](docs/AGENTS.md) first of all.\n",
            ),
            scanned(
                "AGENTS.md",
                "# B\n\nRead [the index](docs/AGENTS.md) first of all.\n",
            ),
            scanned("docs/AGENTS.md", &body),
        ],
        &existing(&["CLAUDE.md", "AGENTS.md", "docs/AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(
        claude
            .files
            .iter()
            .filter(|p| *p == "docs/AGENTS.md")
            .count(),
        1
    );
    let expected: usize = audit.files.iter().filter_map(|f| f.bytes).sum();
    assert_eq!(claude.total_bytes, Some(expected));
}

#[test]
fn a_redirect_to_a_missing_file_does_not_inflate_the_total() {
    // It is already reported as a broken link. Charging for bytes nobody can read
    // would turn one finding into a wrong number.
    let audit = analyse(
        vec![scanned(
            "CLAUDE.md",
            "# A\n\nRead [the index](docs/AGENTS.md) before doing anything at all.\n",
        )],
        &existing(&["CLAUDE.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.files, vec!["CLAUDE.md"]);
    assert!(
        !audit.files[0].broken_links.is_empty(),
        "the break went unreported"
    );
}

#[test]
fn an_unreadable_redirect_target_makes_the_total_unknown() {
    // Same rule as an unreadable entry point: a partial sum is wrong by an unknown
    // amount, and the redirect is where the bytes actually are.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# A\n\nRead [the index](docs/AGENTS.md) first of all.\n",
            ),
            ScannedFile {
                path: "docs/AGENTS.md".into(),
                content: None,
            },
        ],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.total_bytes, None);
    assert_eq!(claude.unreadable_files, vec!["docs/AGENTS.md"]);
}

#[test]
fn a_backticked_path_in_prose_counts_as_a_redirect() {
    // The dominant real form, and the reason the first fix still reported 421 B for
    // front_euronews: its CLAUDE.md says "Read `docs/AGENTS.md` first" — a
    // backticked path, not a markdown link. Detecting only `](target)` found the
    // redirect in docroms-web, which uses links, and missed the two repositories
    // that matter most.
    let audit = analyse(
        vec![
            scanned(
                "CLAUDE.md",
                "# Start\n\n> **CRITICAL — Read `docs/AGENTS.md` first.**\n> \
                 You MUST follow it before any action.\n",
            ),
            scanned(
                "docs/AGENTS.md",
                &format!("# Index\n\n{}\n", "x".repeat(3_000)),
            ),
        ],
        &existing(&["CLAUDE.md", "docs/AGENTS.md"]),
    );
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert!(
        claude.total_bytes.unwrap() > 3_000,
        "a backticked redirect was ignored: {:?} B",
        claude.total_bytes
    );
}

#[test]
fn an_ordinary_backticked_path_is_not_a_redirect() {
    // The detector has to stay narrow, or every mention of a source file would be
    // charged to the agent as context it never loads.
    let audit = analyse(
        vec![scanned(
            "CLAUDE.md",
            "# Rules\n\nThe entry point lives in `src/main.rs` and the config in \
             `Cargo.toml`; neither is instruction context.\n",
        )],
        &existing(&["CLAUDE.md", "src/main.rs", "Cargo.toml"]),
    );
    assert!(
        audit.files[0].redirects_to.is_empty(),
        "a source file was treated as instruction context: {:?}",
        audit.files[0].redirects_to
    );
}

#[test]
fn a_backticked_instruction_file_that_does_not_exist_is_not_charged() {
    // A mention is not proof of existence. Charging for it would invent context.
    let audit = analyse(
        vec![scanned(
            "CLAUDE.md",
            "# Start\n\nRead `docs/AGENTS.md` first, before doing anything at all.\n",
        )],
        &existing(&["CLAUDE.md"]),
    );
    assert!(audit.files[0].redirects_to.is_empty());
    let claude = audit
        .per_agent
        .iter()
        .find(|context| context.agent == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.files, vec!["CLAUDE.md"]);
}

#[test]
fn a_redirect_target_is_never_reported_as_unused() {
    // The drift report called the single most-loaded file in front_euronews a pack
    // nobody loads. That is the worst kind of wrong: it invites deleting it.
    let audit = redirect_shape();
    let drift = drift(&analyse(Vec::new(), &existing(&[])), &audit);
    assert!(
        !drift.unused_files.contains(&"docs/AGENTS.md".to_string()),
        "a redirect target was reported as unused: {:?}",
        drift.unused_files
    );
}

// ── drift ───────────────────────────────────────────────────────────

fn audit_of(pairs: &[(&str, &str)]) -> ContextAudit {
    let paths: Vec<&str> = pairs.iter().map(|(path, _)| *path).collect();
    analyse(
        pairs
            .iter()
            .map(|(path, body)| scanned(path, body))
            .collect(),
        &existing(&paths),
    )
}

#[test]
fn growth_is_reported_with_its_delta() {
    let before = audit_of(&[(
        "AGENTS.md",
        "# A\n\nShort guidance that fits on one line.\n",
    )]);
    let after = audit_of(&[(
        "AGENTS.md",
        "# A\n\nShort guidance that fits on one line.\n\nAnd a good deal more text \
         added afterwards to make this file noticeably larger than it used to be.\n",
    )]);
    let drift = drift(&before, &after);
    assert_eq!(drift.grown.len(), 1);
    assert!(
        drift.grown[0].1 > 50,
        "delta looks wrong: {:?}",
        drift.grown
    );
}

#[test]
fn a_new_instruction_file_is_reported() {
    // It changes every session's cost without anyone deciding to.
    let before = audit_of(&[(
        "AGENTS.md",
        "# A\n\nGuidance long enough to be a paragraph.\n",
    )]);
    let after = audit_of(&[
        (
            "AGENTS.md",
            "# A\n\nGuidance long enough to be a paragraph.\n",
        ),
        (
            "GEMINI.md",
            "# G\n\nOther guidance long enough to be a paragraph.\n",
        ),
    ]);
    assert_eq!(drift(&before, &after).new_files, vec!["GEMINI.md"]);
}

#[test]
fn a_deleted_file_is_not_reported_as_a_shrink() {
    // Losing an instruction file and trimming one are different events; folding
    // them together would show a deletion as an improvement.
    let before = audit_of(&[
        (
            "AGENTS.md",
            "# A\n\nGuidance long enough to be a paragraph.\n",
        ),
        (
            "GEMINI.md",
            "# G\n\nOther guidance long enough to be a paragraph.\n",
        ),
    ]);
    let after = audit_of(&[(
        "AGENTS.md",
        "# A\n\nGuidance long enough to be a paragraph.\n",
    )]);
    let drift = drift(&before, &after);
    assert!(drift.grown.is_empty());
    assert!(drift.new_files.is_empty());
}

#[test]
fn a_newly_broken_route_is_reported_and_an_old_one_is_not() {
    // Reporting a long-standing breakage on every run is how drift output starts
    // getting ignored.
    let before = analyse(
        vec![scanned(
            "AGENTS.md",
            "# A\n\nSee [the old runbook](docs/old.md) for the deployment steps here.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    let after = analyse(
        vec![scanned(
            "AGENTS.md",
            "# A\n\nSee [the old runbook](docs/old.md) and [the new one](docs/new.md) \
             for the deployment steps here.\n",
        )],
        &existing(&["AGENTS.md"]),
    );
    let drift = drift(&before, &after);
    assert_eq!(drift.newly_broken_routes, vec!["docs/new.md"]);
}

#[test]
fn a_file_no_agent_reads_is_reported_as_unused() {
    // Real cost, zero effect: the pack nobody loads.
    let audit = analyse(
        vec![scanned(
            "docs/NOTES.md",
            "# N\n\nSomething nobody's tooling ever reads here.\n",
        )],
        &existing(&["docs/NOTES.md"]),
    );
    let drift = drift(&audit_of(&[]), &audit);
    assert_eq!(drift.unused_files, vec!["docs/NOTES.md"]);
}

// ── bounds ──────────────────────────────────────────────────────────

#[test]
fn the_report_stays_within_its_budget() {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for index in 0..40 {
        pairs.push((
            "docs/AGENTS.md".to_string(),
            format!(
                "# Section {index}\n\nOn 2026-01-01 an incident happened. {}\n",
                "z".repeat(2_000)
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let text = render(&audit_of(&refs));
    assert!(
        text.len() <= AUDIT_REPORT_MAX_BYTES + 32,
        "report was {} bytes",
        text.len()
    );
}
