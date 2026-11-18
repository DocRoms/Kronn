
use super::*;

#[serial_test::serial] // mutates the process-global anti-halluc mode
#[test]
fn preamble_gating_follows_global_mode() {
    let mode = ModeOverride::set("off");
    assert!(preamble_if_active().is_none());
    mode.switch("warn");
    assert!(preamble_if_active().is_some());
    mode.switch("enforce");
    assert!(preamble_if_active().is_some());
}

#[test]
fn preamble_contract_pins_pointer_and_citation_grammar() {
    // 0.8.7 — PREAMBLE was shortened to a POINTER toward
    // `docs/AGENTS.md` § anti-hallucination (the canonical source).
    // The pointer MUST still carry the citation grammar inline so
    // the agent has the format reference even before reading the
    // doc (token-budget safety net). Pin both : the pointer phrase
    // toward AGENTS.md + the structured `[src: …]` grammar.
    assert!(
        PREAMBLE.contains("`docs/AGENTS.md`") || PREAMBLE.contains("docs/AGENTS.md"),
        "PREAMBLE must point to `docs/AGENTS.md` (the canonical source)",
    );
    assert!(
        PREAMBLE.contains("anti-hallucination"),
        "PREAMBLE must mention the anti-hallucination section name",
    );
    // The structured citation grammar — `[src: file:]` is the form
    // mechanically verified by `verify_source_marker` ; the others
    // ship the format reference inline so an agent that hasn't
    // loaded AGENTS.md yet still knows how to cite.
    assert!(
        PREAMBLE.contains("[src: file:"),
        "[src: file:] grammar missing"
    );
    assert!(
        PREAMBLE.contains("[src: url:"),
        "[src: url:] grammar missing"
    );
    // Honesty discipline — fabrication = path/line that doesn't
    // resolve or escapes the project root.
    assert!(
        PREAMBLE.contains("rejected as fabricated")
            || PREAMBLE.contains("escape") && PREAMBLE.contains("rejected"),
        "PREAMBLE must warn that bad citations are rejected",
    );
    // Size sanity — PREAMBLE is the fallback pointer, not the
    // full doctrine. If it grows past ~250 words it means someone
    // re-inlined the cascade and forgot the redesign.
    let word_count = PREAMBLE.split_whitespace().count();
    assert!(
            word_count < 200,
            "PREAMBLE ballooned to {word_count} words — it must stay a short pointer (the cascade lives in docs/AGENTS.md § anti-hallu)",
        );
}

// ── Linter: flags ─────────────────────────────────────────────────

#[test]
fn flags_unsourced_technical_claim() {
    let r = lint_assertions("The retry logic is implemented with an exponential backoff loop.");
    assert_eq!(r.unsourced_count, 1);
    assert_eq!(r.flagged_spans.len(), 1);
}

#[test]
fn flags_french_unsourced_claim() {
    let r = lint_assertions("La fonction de cache se trouve dans le coeur du serveur résilient.");
    assert_eq!(r.unsourced_count, 1, "report: {:?}", r);
}

#[test]
fn counts_multiple_claims() {
    let txt = "The auth token is stored in the session table. \
                   The default is a thirty minute expiry window for everyone.";
    let r = lint_assertions(txt);
    assert_eq!(r.unsourced_count, 2);
}

// ── Linter: suppressions ──────────────────────────────────────────

#[test]
fn does_not_flag_sentence_with_file_path_anchor() {
    let r = lint_assertions(
        "The retry logic is implemented in backend/src/core/retry.rs for all callers.",
    );
    assert_eq!(r.unsourced_count, 0, "file path is a self-source: {:?}", r);
}

#[test]
fn does_not_flag_sentence_with_backticked_code_ref() {
    let r =
        lint_assertions("The default is resolved by `config::default_model_tier()` at startup.");
    assert_eq!(
        r.unsourced_count, 0,
        "backtick code ref anchors it: {:?}",
        r
    );
}

#[test]
fn does_not_flag_sentence_with_url() {
    let r = lint_assertions(
        "The endpoint returns paginated JSON per https://api.example.com/docs spec.",
    );
    assert_eq!(r.unsourced_count, 0);
}

#[test]
fn does_not_flag_hedged_claim() {
    let r = lint_assertions(
        "I think the function probably returns the cached value, but let me check.",
    );
    assert_eq!(r.unsourced_count, 0, "hedge suppresses the flag: {:?}", r);
}

#[test]
fn does_not_flag_src_annotated_claim() {
    let r = lint_assertions("The default is a 30 minute window [src: backend session config].");
    assert_eq!(r.unsourced_count, 0);
}

// ── 0.8.7 patch — CVE / version / security vocabulary ─────────────
// Real-world disc 0c9c2032 (Symfony CVE diagnostic) exposed the
// original heuristic's blind spot : none of the agent's claims
// matched a cue, so `lint_report` stayed `None` and the pill never
// surfaced. These pin the expanded `CLAIM_CUES` so that regression
// can't happen silently.

#[test]
fn flags_french_security_vulnerability_claim() {
    let r = lint_assertions("La version 7.3.6 est vulnérable à une faille de PATH_INFO.");
    assert!(
        r.unsourced_count >= 1,
        "expected vulnérable to flag: {:?}",
        r
    );
}

#[test]
fn flags_french_affected_versions_claim() {
    let r = lint_assertions("Les versions affectées vont de 6.0 à 7.3.6 d'après l'avis.");
    assert!(
        r.unsourced_count >= 1,
        "expected versions affectées to flag: {:?}",
        r
    );
}

#[test]
fn flags_french_fixed_in_claim() {
    let r = lint_assertions("La faille est corrigé dans la release 7.3.7 publiée hier.");
    assert!(
        r.unsourced_count >= 1,
        "expected corrigé dans to flag: {:?}",
        r
    );
}

#[test]
fn flags_english_cve_id_claim() {
    let r =
        lint_assertions("CVE-2025-64500 is a high-severity issue in Symfony's PATH_INFO handling.");
    assert!(r.unsourced_count >= 1, "expected cve- to flag: {:?}", r);
}

#[test]
fn flags_english_fixed_in_claim() {
    let r =
        lint_assertions("The bug is fixed in version 7.3.7 according to the upstream changelog.");
    assert!(r.unsourced_count >= 1, "expected fixed in to flag: {:?}", r);
}

#[test]
fn cve_claim_with_source_marker_is_not_flagged() {
    let r = lint_assertions(
        "CVE-2025-64500 affects 7.3.6 [src: https://symfony.com/blog/cve-2025-64500].",
    );
    assert_eq!(
        r.unsourced_count, 0,
        "anchored CVE claim must not flag: {:?}",
        r
    );
}

#[test]
fn hedged_cve_claim_is_not_flagged() {
    let r = lint_assertions("I think CVE-2025-64500 might be the relevant one, let me check.");
    assert_eq!(
        r.unsourced_count, 0,
        "hedged CVE claim must not flag: {:?}",
        r
    );
}

// ── Anti-FP regression : the broader cues we explicitly REJECTED
// must not trigger on everyday prose. These pin the precision
// discipline of the 0.8.7 patch — if a future expansion re-adds
// any of these, the corresponding test fails and forces the
// decision back to a human.

#[test]
fn does_not_flag_generic_added_in_phrase() {
    // "added in" is intentionally NOT a cue — too broad, fires on
    // build-log / changelog prose that's already self-anchoring.
    let r = lint_assertions("We added in a new dropdown for the user picker last sprint.");
    assert_eq!(
        r.unsourced_count, 0,
        "generic 'added in' must not flag: {:?}",
        r
    );
}

#[test]
fn does_not_flag_generic_available_in_phrase() {
    let r = lint_assertions("That feature is available in the documentation we just shipped.");
    assert_eq!(
        r.unsourced_count, 0,
        "generic 'available in' must not flag: {:?}",
        r
    );
}

#[test]
fn does_not_flag_generic_current_version_phrase() {
    // "the current version" alone is too generic — keep the FP risk
    // off the table unless it's framed as "the current version is …".
    let r = lint_assertions("Reviewing the current version of the layout before we ship.");
    assert_eq!(
        r.unsourced_count, 0,
        "generic 'current version' must not flag: {:?}",
        r
    );
}

#[test]
fn does_not_flag_generic_disponible_dans() {
    let r = lint_assertions("Cette fonctionnalité est disponible dans la doc mise à jour hier.");
    assert_eq!(
        r.unsourced_count, 0,
        "generic 'disponible dans' must not flag: {:?}",
        r
    );
}

#[test]
fn does_not_flag_generic_ajoute_dans() {
    let r = lint_assertions("On a ajouté dans la sidebar un raccourci vers les paramètres.");
    assert_eq!(
        r.unsourced_count, 0,
        "generic 'ajouté dans' must not flag: {:?}",
        r
    );
}

// ── Anti-FP for the 3 historical categories (0.8.7 R2 — note from
// anti-hallu expert integrated by tech-lead) :
//   (a) citations of doc inside fenced code [already covered above]
//   (b) proper nouns with FR/ES apostrophes
//   (c) bare semantic versions in non-claim prose

#[test]
fn does_not_flag_proper_noun_with_apostrophe_fr() {
    // Apostrophes in FR proper nouns ("L'équipe", "d'Élodie") used to
    // trip the heuristic when the surrounding cue-like tokens looked
    // technical. Real-world miss : "L'équipe d'Élodie a configuré le
    // déploiement chez Vercel." has zero verifiable claim but the
    // apostrophe-heavy phrasing must NOT trip the linter.
    let r = lint_assertions("L'équipe d'Élodie a configuré le déploiement chez Vercel.");
    assert_eq!(
        r.unsourced_count, 0,
        "FR proper noun with apostrophes must not flag: {:?}",
        r,
    );
}

#[test]
fn does_not_flag_proper_noun_with_apostrophe_es() {
    // Spanish has fewer apostrophe-y constructs but loanwords + names
    // (Ortiz's, O'Brien, l'Hospital) appear in code comments and
    // commit messages. None of them should trip the linter.
    let r = lint_assertions("Ortiz's PR landed yesterday after O'Brien reviewed it.");
    assert_eq!(
        r.unsourced_count, 0,
        "ES/EN apostrophe proper noun must not flag: {:?}",
        r,
    );
}

#[test]
fn does_not_flag_bare_semver_in_neutral_prose() {
    // Bare semantic versions ("Version 1.2.3 sort demain.") appear
    // in release-notes-style prose without being a verifiable claim
    // about THIS project. Without a claim cue ("is fixed in",
    // "affected versions"), bare semver must NOT trip the linter —
    // otherwise every changelog blurb generates noise.
    let r = lint_assertions("Version 1.2.3 sort demain selon le calendrier prévu.");
    assert_eq!(
        r.unsourced_count, 0,
        "bare semver in neutral prose must not flag: {:?}",
        r,
    );
}

#[test]
fn corpus_false_positive_rate_under_5_percent() {
    // 0.8.7 redesign R2 P1-B (architecte) — quantified FP gate.
    // Loads `backend/tests/fixtures/anti_halluc_corpus.jsonl` (50
    // messages, each labelled with its `expected_unsourced` count).
    // A FP = a message where the linter flags MORE assertions than
    // expected. We assert the FP rate stays under 5% — Wilson 95%
    // confidence interval at n=50 → [0.014, 0.165], puissance
    // suffisante pour détecter un régresseur 5%→15%+.
    //
    // ⚠ When CLAIM_CUES expand, this test catches an over-broad
    // change before it lands. If a real positive needs to be added
    // (i.e. a NEW kind of legitimate claim), update the corpus
    // entry's `expected_unsourced` field accordingly.
    let fixture_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .join("tests/fixtures/anti_halluc_corpus.jsonl");
    let content = std::fs::read_to_string(&fixture_path).expect("fixture must exist");

    let mut total = 0usize;
    let mut false_positives = 0usize;
    let mut fp_examples: Vec<String> = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSONL line: {line}\n{e}"));
        let text = entry["text"].as_str().expect("text field");
        let expected = entry["expected_unsourced"]
            .as_u64()
            .expect("expected_unsourced field") as u32;
        let category = entry["category"].as_str().unwrap_or("?");

        let report = lint_assertions(text);
        total += 1;
        if report.unsourced_count > expected {
            false_positives += 1;
            if fp_examples.len() < 5 {
                fp_examples.push(format!(
                    "[{category}] expected={expected} got={} text=\"{}\"",
                    report.unsourced_count,
                    text.chars().take(80).collect::<String>(),
                ));
            }
        }
    }

    assert!(total >= 50, "corpus must have ≥ 50 entries, got {total}");
    let fp_rate = false_positives as f64 / total as f64;
    assert!(
        fp_rate <= 0.05,
        "FP rate {:.1}% exceeds 5% gate ({fp}/{total}). Examples:\n  {}",
        fp_rate * 100.0,
        fp_examples.join("\n  "),
        fp = false_positives,
        total = total,
    );
}

#[test]
fn ignores_fenced_code_blocks() {
    let txt = "Here is the impl:\n```rust\nlet the_endpoint = returns_default();\n```\nDone.";
    let r = lint_assertions(txt);
    // The claim-cue-looking tokens are inside the fence → ignored.
    assert_eq!(
        r.unsourced_count, 0,
        "fenced code must not be linted: {:?}",
        r
    );
}

#[test]
fn ignores_short_and_non_technical_prose() {
    let r = lint_assertions("Sounds good. Thanks! I'll get started now and report back soon.");
    assert_eq!(r.unsourced_count, 0);
}

#[test]
fn empty_input_is_empty_report() {
    assert!(lint_assertions("").is_empty());
    assert!(lint_assertions("   \n  ").is_empty());
}

// ── Linter: robustness ────────────────────────────────────────────

#[test]
fn excerpt_is_char_boundary_safe_with_multibyte() {
    // 300 accented chars in one flagged sentence → excerpt must not panic
    // and must cap at SPAN_EXCERPT_MAX_CHARS + 1 (ellipsis).
    let long = format!("The function {} se trouve ici", "é".repeat(300));
    let r = lint_assertions(&long);
    assert_eq!(r.unsourced_count, 1);
    assert!(r.flagged_spans[0].text.chars().count() <= SPAN_EXCERPT_MAX_CHARS + 1);
}

#[test]
fn emoji_in_claim_does_not_panic() {
    let r = lint_assertions("The route is handled by the gateway 🚀 reliably for users.");
    // Whether flagged or not, the point is no panic + valid report.
    assert!(r.unsourced_count <= 1);
}

#[test]
fn span_count_is_capped_but_count_is_accurate() {
    // Build 30 distinct unsourced claims; count tracks all, spans cap.
    let mut txt = String::new();
    for i in 0..30 {
        txt.push_str(&format!(
            "The parameter number {i} is configured for the whole fleet here. "
        ));
    }
    let r = lint_assertions(&txt);
    assert_eq!(r.unsourced_count, 30);
    assert_eq!(r.flagged_spans.len(), MAX_FLAGGED_SPANS);
}

// ── Niveau 1 — extraction ─────────────────────────────────────────

#[test]
fn extract_multiple_markers() {
    let txt = "A [src: foo.rs:1] and B [src: url: https://x.com] end.";
    let m = extract_source_markers(txt);
    assert_eq!(m, vec!["foo.rs:1", "url: https://x.com"]);
}

#[test]
fn extract_skips_markers_inside_fences() {
    let txt = "Real [src: a.rs:1].\n```\nfake [src: b.rs:9]\n```\nDone.";
    let m = extract_source_markers(txt);
    assert_eq!(m, vec!["a.rs:1"], "fenced marker must be ignored");
}

#[test]
fn extract_balances_nested_brackets() {
    let m = extract_source_markers("see [src: arr[0].field] here");
    assert_eq!(m, vec!["arr[0].field"]);
}

// KT-1311 — an unclosed marker (a banner cut at 240 chars) must not blind
// the markers after it.
#[test]
fn an_unclosed_marker_does_not_swallow_the_markers_after_it() {
    let txt =
        "banner (… [src: file: .github/wor).\n\n- a [src: file: a.rs:1]\n- b [src: file: b.rs:2]\n";
    assert_eq!(
        extract_source_markers(txt),
        vec!["file: a.rs:1", "file: b.rs:2"]
    );
}

#[test]
fn an_unclosed_marker_ends_at_the_next_marker_on_the_same_line() {
    let txt = "cut [src: file: .gi then [src: file: a.rs:1] and [src: file: b.rs:2]";
    assert_eq!(
        extract_source_markers(txt),
        vec!["file: a.rs:1", "file: b.rs:2"]
    );
}

#[test]
fn an_unclosed_marker_stops_at_a_paragraph_break_not_at_a_stray_bracket() {
    let txt = "cut [src: file: .gi\n\nlater stray ] bracket [src: file: a.rs:1]";
    assert_eq!(extract_source_markers(txt), vec!["file: a.rs:1"]);
    assert!(extract_source_markers("tail [src: file: a.rs:1").is_empty());
}

#[test]
fn a_marker_wrapped_over_two_lines_is_still_read() {
    assert_eq!(
        extract_source_markers("see [src:\nfile: a.rs:7] ok"),
        vec!["file: a.rs:7"]
    );
}

#[test]
fn extract_empty_when_none() {
    assert!(extract_source_markers("no markers here at all").is_empty());
}

// ── Pre-tag quick win — literal-mention false positives ──────────
// Live incident (2026-07-14 room, 4×): TALKING about the `[src:]`
// grammar in backticks was linted as a fabricated citation.

#[test]
fn extract_ignores_a_syntax_mention_in_backticks() {
    let txt = "la citation `[src:]` est obligatoire dans la doc.";
    assert!(
        extract_source_markers(txt).is_empty(),
        "quoted syntax is a mention, not a citation"
    );
}

#[test]
fn extract_ignores_a_full_quoted_marker_in_inline_code() {
    // Codex non-regression ask: literal text that LOOKS like a
    // provenance, quoted in inline code — must extract nothing.
    let txt = "utilise `[src: file: main.rs:10]` pour citer une ligne.";
    assert!(extract_source_markers(txt).is_empty());
    let double = "ou ``[src: url: https://x.com]`` en double backticks.";
    assert!(extract_source_markers(double).is_empty());
}

#[test]
fn extract_drops_an_empty_marker_in_prose() {
    assert!(extract_source_markers("un [src:] nu est une mention de grammaire.").is_empty());
    assert!(extract_source_markers("avec espaces [src:   ] aussi.").is_empty());
}

#[test]
fn extract_keeps_the_real_marker_next_to_a_quoted_mention() {
    let txt = "La grammaire `[src:]` s'applique ici [src: foo.rs:1].";
    assert_eq!(extract_source_markers(txt), vec!["foo.rs:1"]);
}

#[test]
fn extract_survives_an_unclosed_backtick() {
    // A lone backtick in prose must not swallow the rest of the line.
    let txt = "un ` isolé puis une vraie citation [src: real.rs:2] ensuite.";
    assert_eq!(extract_source_markers(txt), vec!["real.rs:2"]);
}

#[test]
fn extract_drops_an_unclosed_marker() {
    // Copilot (PR 119): partially-typed prose must not become a
    // citation — no closing `]` means no marker.
    assert!(extract_source_markers("truncated at end [src: foo.rs:1").is_empty());
    let mixed = "closed [src: a.rs:1] then truncated [src: b.rs:9";
    assert_eq!(extract_source_markers(mixed), vec!["a.rs:1"]);
}

#[test]
fn extract_does_not_weld_prose_across_a_dropped_span() {
    // Copilot (PR 119 round 3): "[s" + `span` + "rc: …]" must not
    // concatenate into a fresh "[src:" once the span is removed — the
    // span leaves a space behind.
    let txt = "des crochets [s`et du code`rc: foo.rs:1] ne fusionnent pas.";
    assert!(extract_source_markers(txt).is_empty());
    let with_real = "idem [s`x`rc: nope] mais [src: ok.rs:4] reste.";
    assert_eq!(extract_source_markers(with_real), vec!["ok.rs:4"]);
}

#[test]
fn extract_is_bounded_on_a_backtick_heavy_line() {
    // Copilot (PR 119): > MAX_BACKTICK_RUNS_PER_LINE runs on one line
    // skips span-stripping (degraded = pre-quick-win behaviour) instead
    // of pairing runs — the call stays cheap and markers outside spans
    // still extract.
    let noise = "` x ".repeat(100); // 100 unmatched single-backtick runs
    let txt = format!("{noise} et [src: real.rs:3] à la fin.");
    assert_eq!(extract_source_markers(&txt), vec!["real.rs:3"]);
}

#[test]
fn extract_quoted_returns_only_the_markers_in_inline_code() {
    let txt = "prose [src: a.rs:1], quoted `[src: file: b.rs:2]` and ``[src: c.rs]``.\n\
                   ```\n[src: fenced.rs:1]\n```\n";
    assert_eq!(
        extract_quoted_source_markers(txt),
        vec!["file: b.rs:2", "c.rs"]
    );
    assert_eq!(extract_source_markers(txt), vec!["a.rs:1"]);
}

#[test]
fn quoted_marker_that_resolves_is_verified_like_the_others() {
    let root = temp_project();
    let r = analyze("Defined in `[src: file: src/foo.rs:3]`.", Some(&root));
    assert_eq!(r.verified_count(), 1, "{r:?}");
    assert_eq!((r.fabricated_count, r.unverified_count), (0, 0));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn quoted_marker_that_fails_counts_as_unverified_never_green() {
    let root = temp_project();
    let text = "Bad `[src: file: src/ghost.rs:1]`, escape `[src: file: /etc/passwd:1]`, \
                    grammar `[src: file: <path>:<line>]`, refused `[src: training-data]`.";
    let r = analyze(text, Some(&root));
    assert_eq!(r.verified_count(), 0, "{r:?}");
    assert_eq!(
        r.fabricated_count, 0,
        "an example must not read as fabricated"
    );
    assert_eq!(r.unverified_count, 4, "{r:?}");
    assert!(r.sources.iter().all(
        |s| s.status == SourceStatus::Unchecked && s.detail.starts_with("quoted in backticks")
    ));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn quoted_marker_is_deduped_against_the_same_file_in_prose() {
    let root = temp_project();
    let r = analyze(
        "See [src: file: src/foo.rs:2] and `[src: file: src/foo.rs:4]`.",
        Some(&root),
    );
    assert_eq!(r.sources.len(), 1, "{r:?}");
    std::fs::remove_dir_all(&root).ok();
}

// ── Niveau 1 — line-spec parsing ──────────────────────────────────

#[test]
fn split_path_and_lines_variants() {
    assert_eq!(split_path_and_lines("foo.rs"), ("foo.rs", None));
    assert_eq!(
        split_path_and_lines("foo.rs:42"),
        ("foo.rs", Some((42, 42)))
    );
    assert_eq!(
        split_path_and_lines("a/b/foo.rs:10-20"),
        ("a/b/foo.rs", Some((10, 20)))
    );
    // A non-numeric tail is part of the path, not a line spec.
    assert_eq!(split_path_and_lines("weird:name"), ("weird:name", None));
    // Reversed range is rejected → treated as no line spec.
    assert_eq!(split_path_and_lines("foo.rs:20-10"), ("foo.rs:20-10", None));
    // A presence marker names the file, not a line of it.
    assert_eq!(split_path_and_lines("foo.rs:exists"), ("foo.rs", None));
    assert_eq!(split_path_and_lines("foo.rs:Tracked"), ("foo.rs", None));
}

#[test]
fn a_presence_marker_verifies_the_file_exists_without_a_line() {
    let root = temp_project();
    let found = verify_source_marker("file: src/foo.rs:exists", Some(&root));
    assert_eq!(found.status, SourceStatus::Verified, "{found:?}");
    let missing = verify_source_marker("file: src/gone.rs:exists", Some(&root));
    assert_eq!(missing.status, SourceStatus::NotFound, "{missing:?}");
    // `tracked` is the index's word, not the disk's: this root is no repository yet.
    let untracked = verify_source_marker("file: src/foo.rs:tracked", Some(&root));
    assert_eq!(untracked.status, SourceStatus::NotFound, "{untracked:?}");
    assert!(
        untracked.detail.contains("git does not track it"),
        "{untracked:?}"
    );
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "src/foo.rs"]);
    let tracked = verify_source_marker("file: src/foo.rs:tracked", Some(&root));
    assert_eq!(tracked.status, SourceStatus::Verified, "{tracked:?}");
    std::fs::remove_dir_all(&root).ok();
}

// KT-1307: the en and em dash a model writes for a range hyphen are read the
// same way by every gate (verifier, citation drift, documentary gate).
#[test]
fn en_and_em_dash_ranges_are_line_ranges() {
    assert_eq!(parse_line_bounds("17\u{2013}23"), Some((17, 23)));
    assert_eq!(parse_line_bounds("17\u{2014}23"), Some((17, 23)));
    assert_eq!(parse_line_bounds("17 \u{2013} 23"), Some((17, 23)));
    assert_eq!(parse_line_bounds("9-3"), Some((9, 3)));
    assert_eq!(parse_line_bounds("extends"), None);
    assert_eq!(parse_line_bounds("5\u{2013}"), None);
    assert_eq!(
        split_path_and_lines("a/foo.rs:10\u{2013}20"),
        ("a/foo.rs", Some((10, 20)))
    );
    assert_eq!(
        split_path_and_lines("foo.rs:20\u{2014}10"),
        ("foo.rs:20\u{2014}10", None)
    );
    assert_eq!(
        file_citation("file: src/a.rs:2\u{2013}4"),
        Some(("src/a.rs".into(), Some((2, 4))))
    );
    assert_eq!(
        file_citation_anchored("file: src/a.rs:2\u{2014}4#retry_once"),
        Some(("src/a.rs".into(), Some((2, 4)), Some("retry_once".into())))
    );
}

#[test]
fn analyze_verifies_a_dash_range_like_a_hyphen_range() {
    let root = temp_project(); // src/foo.rs = 5 lines
    let green = analyze(
        "Claim [src: file: src/foo.rs:2\u{2013}4] and [src: file: src/foo.rs:1\u{2014}5].",
        Some(&root),
    );
    assert_eq!(green.sources.len(), 2, "{green:?}");
    assert!(
        green.sources.iter().all(|s| !s.status.is_fabricated()),
        "{green:?}"
    );
    let red = analyze("Claim [src: file: src/foo.rs:4\u{2013}9].", Some(&root));
    assert!(red.sources[0].status.is_fabricated(), "{red:?}");
    std::fs::remove_dir_all(&root).ok();
}

// ── Niveau 1 — classification ─────────────────────────────────────

#[test]
fn classify_detects_kinds() {
    assert_eq!(classify_source("https://x.com").0, SourceKind::Url);
    assert_eq!(classify_source("url: https://x.com").0, SourceKind::Url);
    assert_eq!(
        classify_source("user-confirmed 2026-05-25").0,
        SourceKind::User
    );
    assert_eq!(classify_source("commit: abc123").0, SourceKind::Commit);
    assert_eq!(classify_source("file: src/a.rs:1").0, SourceKind::File);
    // Bare path defaults to File, prefix stripped.
    let (k, r) = classify_source("file: src/a.rs:1");
    assert_eq!(k, SourceKind::File);
    assert_eq!(r, "src/a.rs:1");
}

#[test]
fn an_unknown_typed_kind_is_other_never_a_file() {
    for raw in [
        "command: git log --oneline -5",
        "Command: git log",
        "shell: ls -la docs/",
        "test-run: cargo test",
    ] {
        assert_eq!(classify_source(raw).0, SourceKind::Other, "{raw}");
        assert_eq!(file_citation(raw), None, "{raw}");
    }
    let check = verify_source_marker("command: git log --oneline", Some(Path::new("/tmp")));
    assert_eq!(check.kind, SourceKind::Other);
    assert_eq!(check.status, SourceStatus::Unchecked);
}

#[test]
fn a_mistyped_kind_is_verified_as_a_file_never_a_free_pass() {
    // `fil:` / `path:` / `source:` … are not on the closed list: a path-like
    // text is judged as a file and is fabricated when it is missing.
    let root = temp_project();
    for raw in [
        "fil: src/ghost.rs:1",
        "path: src/ghost.rs",
        "source: src/ghost.rs:3",
        "ref: src/ghost.rs",
        "files: src/ghost.rs:1-2",
        "src: src/ghost.rs:1",
        "path: Ghostfile",
        "path: ghost.rs:4",
    ] {
        assert_eq!(classify_source(raw).0, SourceKind::File, "{raw}");
        let check = verify_source_marker(raw, Some(&root));
        assert!(check.status.is_fabricated(), "{raw}: {check:?}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_mistyped_kind_is_fabricated_under_the_enforce_gate() {
    let root = temp_project();
    for raw in [
        "fil: src/ghost.rs:1",
        "path: src/ghost.rs",
        "source: src/ghost.rs",
    ] {
        let text = format!("Claim [src: {raw}].");
        let report = analyze(&text, Some(&root));
        assert_eq!(report.fabricated_count, 1, "{raw}: {report:?}");
        let verdict = crate::api::audit::anti_hallu_enforce::lint_step_file(&text, &[&root]);
        assert_eq!(verdict.count(), 1, "{raw}");
        assert_eq!(
            crate::api::audit::anti_hallu_enforce::decide(&verdict, 1, 3),
            crate::api::audit::anti_hallu_enforce::GateDecision::Retry,
            "{raw}"
        );
        assert_eq!(
            crate::api::audit::anti_hallu_enforce::decide(&verdict, 3, 3),
            crate::api::audit::anti_hallu_enforce::GateDecision::Fail,
            "{raw}"
        );
    }
    // The text is judged as written, prefix included: even over a real file
    // a mistyped kind is refused, so the retry feedback makes the agent fix it.
    let real = analyze("See [src: path: src/foo.rs:2].", Some(&root));
    assert_eq!(real.fabricated_count, 1, "{real:?}");
    let fixed = analyze("See [src: file: src/foo.rs:2].", Some(&root));
    assert_eq!(fixed.fabricated_count, 0, "{fixed:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_listed_non_file_kind_stays_other_and_is_counted_unverified() {
    let root = temp_project();
    let text = "Recent work [src: command: git log --oneline -5] and \
                    [src: test-run: cargo test --lib].";
    let r = analyze(text, Some(&root));
    assert_eq!(r.fabricated_count, 0, "{r:?}");
    assert_eq!(r.unverified_count, 2, "{r:?}");
    assert!(r.sources.iter().all(|s| s.kind == SourceKind::Other));
    // Unknown word + prose (no path in it) is unverifiable too, and counted.
    let prose = analyze("[src: note: the team agreed in standup]", Some(&root));
    assert_eq!(prose.fabricated_count, 0, "{prose:?}");
    assert_eq!(prose.unverified_count, 1, "{prose:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn paths_and_known_kinds_are_not_swallowed_by_the_unknown_kind_rule() {
    for raw in [
        "src/a.rs:10",
        "a.rs:10",
        "Dockerfile",
        "docker-compose.yml:2-25",
        "file: src/a.rs:1",
    ] {
        assert_eq!(classify_source(raw).0, SourceKind::File, "{raw}");
    }
}

#[test]
fn classify_full_provenance_gradient() {
    assert_eq!(
        classify_source("training-data: anything").0,
        SourceKind::TrainingData
    );
    assert_eq!(classify_source("training-data").0, SourceKind::TrainingData);
    assert_eq!(
        classify_source("inferred: backend/src/api/").0,
        SourceKind::Inferred
    );
    assert_eq!(classify_source("hypothesis").0, SourceKind::Hypothesis);
    assert_eq!(
        classify_source("code-comment: GptLoader.ts:36").0,
        SourceKind::CodeComment
    );
    assert_eq!(classify_source("api: log#9").0, SourceKind::Api);
}

#[test]
fn classify_does_not_misclassify_files_starting_with_type_names() {
    // Real files whose names begin with a type keyword stay File (the
    // token-boundary check) — regression guard for the `user_service.rs` bug.
    for path in [
        "user_service.rs:10",
        "api_client.rs:1",
        "commit_log.rs",
        "filer.rs",
    ] {
        assert_eq!(classify_source(path).0, SourceKind::File, "{path}");
    }
}

#[test]
fn training_data_is_rejected_not_verified() {
    // The spec's headline promise: training-data is refused, never file-verified.
    let c = verify_source_marker("training-data: I recall that…", Some(Path::new("/tmp")));
    assert_eq!(c.kind, SourceKind::TrainingData);
    assert_eq!(c.status, SourceStatus::Rejected);
    assert!(
        c.status.is_fabricated(),
        "rejected must count toward fabricated_count"
    );
}

#[test]
fn inferred_and_hypothesis_are_unchecked_not_file_verified() {
    let root = temp_project();
    // Even with a real file ref, inferred is soft → never file-verified.
    let inf = verify_source_marker("inferred: src/foo.rs", Some(&root));
    assert_eq!(inf.kind, SourceKind::Inferred);
    assert_eq!(inf.status, SourceStatus::Unchecked);
    assert!(!inf.status.is_fabricated());
    let hyp = verify_source_marker("hypothesis", Some(&root));
    assert_eq!(hyp.status, SourceStatus::Unchecked);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn code_comment_is_existence_verified_but_low_trust() {
    let root = temp_project();
    let ok = verify_source_marker("code-comment: src/foo.rs:2", Some(&root));
    assert_eq!(ok.kind, SourceKind::CodeComment);
    assert_eq!(ok.status, SourceStatus::Verified);
    assert!(
        ok.detail.contains("not authoritative"),
        "detail: {}",
        ok.detail
    );
    // A code-comment citing a missing file is still flagged fabricated.
    let bad = verify_source_marker("code-comment: src/ghost.rs:1", Some(&root));
    assert_eq!(bad.status, SourceStatus::NotFound);
    assert!(bad.status.is_fabricated());
    std::fs::remove_dir_all(&root).ok();
}

// ── Niveau 1 — mechanical verification (real temp project) ────────

fn temp_project() -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!("kronn_antihalluc_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(d.join("src")).unwrap();
    // A 5-line file at src/foo.rs.
    std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
    d
}

#[test]
fn verify_existing_file_no_line() {
    let root = temp_project();
    let c = verify_source_marker("src/foo.rs", Some(&root));
    assert_eq!(c.status, SourceStatus::Verified, "{:?}", c);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_line_in_and_out_of_bounds() {
    let root = temp_project();
    assert_eq!(
        verify_source_marker("src/foo.rs:3", Some(&root)).status,
        SourceStatus::Verified
    );
    assert_eq!(
        verify_source_marker("src/foo.rs:5", Some(&root)).status,
        SourceStatus::Verified
    );
    // The empty line after the final newline is what editors number N+1: a
    // range may end on it, but a citation of that line alone cites nothing.
    assert_eq!(
        verify_source_marker("src/foo.rs:5-6", Some(&root)).status,
        SourceStatus::Verified
    );
    assert_eq!(
        verify_source_marker("src/foo.rs:6", Some(&root)).status,
        SourceStatus::OutOfBounds
    );
    assert_eq!(
        verify_source_marker("src/foo.rs:7", Some(&root)).status,
        SourceStatus::OutOfBounds
    );
    // Range
    assert_eq!(
        verify_source_marker("src/foo.rs:2-4", Some(&root)).status,
        SourceStatus::Verified
    );
    assert_eq!(
        verify_source_marker("src/foo.rs:4-9", Some(&root)).status,
        SourceStatus::OutOfBounds
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_missing_file_is_not_found() {
    let root = temp_project();
    let c = verify_source_marker("src/nope.rs:1", Some(&root));
    assert_eq!(c.status, SourceStatus::NotFound, "{:?}", c);
    std::fs::remove_dir_all(&root).ok();
}

// ── Bare-basename resolution (kills the "Foo.ts:42 unverified" FP) ──

/// Make a fresh temp project root and run `f` to populate it. Returns root.
fn temp_root_with(f: impl FnOnce(&Path)) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!("kronn_basename_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    f(&d);
    d
}

fn write_file(root: &Path, rel: &str, lines: usize) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "x\n".repeat(lines)).unwrap();
}

#[test]
fn bare_basename_resolves_to_unique_nested_file() {
    // The core fix: a bare `Widget.ts:2` cited without its full path, living
    // deep in the tree, now verifies instead of showing "unverified".
    let root = temp_root_with(|r| write_file(r, "app/assets/ts/Services/Widget.ts", 3));
    let (status, detail) = verify_file_ref("Widget.ts:2", &[&root]);
    assert_eq!(status, SourceStatus::Verified, "{detail}");
    assert!(detail.contains("unique basename"), "{detail}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bare_basename_skips_dot_kronn_worktree_copies() {
    // The real front_euronews case: 1 real file + a copy under
    // `.kronn/worktrees/`. Without the skip the basename looks ambiguous;
    // with it, the main copy is the unique match → Verified.
    let root = temp_root_with(|r| {
        write_file(r, "app/assets/ts/Widget.ts", 3);
        write_file(r, ".kronn/worktrees/wt-abc/app/assets/ts/Widget.ts", 3);
        write_file(r, "node_modules/pkg/Widget.ts", 3);
    });
    let (status, _d) = verify_file_ref("Widget.ts:1", &[&root]);
    assert_eq!(status, SourceStatus::Verified, "{_d}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bare_basename_two_real_copies_stay_ambiguous() {
    // Two genuine copies (NOT under a skip dir) → we refuse to guess.
    let root = temp_root_with(|r| {
        write_file(r, "a/Widget.ts", 3);
        write_file(r, "b/Widget.ts", 3);
    });
    let (status, detail) = verify_file_ref("Widget.ts:1", &[&root]);
    assert_eq!(status, SourceStatus::NotFound);
    assert!(detail.contains("ambiguous"), "{detail}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bare_basename_absent_is_not_found() {
    let root = temp_root_with(|r| write_file(r, "app/Other.ts", 3));
    let (status, _d) = verify_file_ref("Widget.ts:1", &[&root]);
    assert_eq!(status, SourceStatus::NotFound);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bare_basename_unique_but_line_out_of_bounds() {
    // Resolved by name, but the cited line is past EOF → honest OutOfBounds.
    let root = temp_root_with(|r| write_file(r, "deep/dir/Widget.ts", 2));
    let (status, _d) = verify_file_ref("Widget.ts:99", &[&root]);
    assert_eq!(status, SourceStatus::OutOfBounds, "{_d}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bare_basename_same_file_in_two_roots_is_not_ambiguous() {
    // Isolated-disc shape: roots = [worktree, main], same file in both.
    // First root wins (mirrors exact-path semantics) → not double-counted.
    let wt = temp_root_with(|r| write_file(r, "app/Widget.ts", 3));
    let main = temp_root_with(|r| write_file(r, "app/Widget.ts", 3));
    let (status, _d) = verify_file_ref("Widget.ts:1", &[&wt, &main]);
    assert_eq!(status, SourceStatus::Verified, "{_d}");
    std::fs::remove_dir_all(&wt).ok();
    std::fs::remove_dir_all(&main).ok();
}

#[test]
fn inline_bare_basename_anchor_no_longer_unverified() {
    // End-to-end via analyze_roots: the exact disc d344b52b false positive.
    // A backticked bare anchor `Widget.ts:2` for a nested file used to land
    // in `unverified_count`; it now resolves green.
    let root = temp_root_with(|r| write_file(r, "app/assets/ts/Services/Widget.ts", 5));
    let report = analyze_roots("See `Widget.ts:2` for the logic.", &[&root]);
    assert_eq!(report.unverified_count, 0, "should not be a soft-amber FP");
    assert!(
        report
            .sources
            .iter()
            .any(|s| s.status == SourceStatus::Verified),
        "the inline anchor must resolve to a Verified source: {:?}",
        report.sources
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_relative_path_traversal_is_jailed() {
    // Relative paths still go through the lexical jail under `root`,
    // so a `../` smuggled inside a relative citation is OutsideProject.
    let root = temp_project();
    let rel = verify_source_marker("../../../../../../etc/passwd:1", Some(&root));
    assert_eq!(rel.status, SourceStatus::OutsideProject, "{:?}", rel);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_absolute_path_outside_root_is_rejected() {
    // `/etc/passwd` exists on every Unix host: it must still be refused.
    let root = temp_project();
    let c = verify_source_marker("file: /etc/passwd:1", Some(&root));
    assert_eq!(c.status, SourceStatus::OutsideProject, "{c:?}");
    assert!(c.status.is_fabricated());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_absolute_outside_root_gives_no_existence_oracle() {
    // An existing and a missing file outside the root read the same.
    let root = temp_project();
    let sibling = std::env::temp_dir().join(format!("kronn_lint_sibling_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(sibling.join("src")).unwrap();
    std::fs::write(sibling.join("src/data.yml"), "hello: world\n").unwrap();
    let present = verify_source_marker(
        &sibling.join("src/data.yml").display().to_string(),
        Some(&root),
    );
    let absent = verify_source_marker(
        &sibling.join("src/nope.yml").display().to_string(),
        Some(&root),
    );
    let ghost = verify_source_marker(
        "/this/path/should/not/exist/anywhere-7a3f.txt:1",
        Some(&root),
    );
    for c in [&present, &absent, &ghost] {
        assert_eq!(c.status, SourceStatus::OutsideProject, "{c:?}");
    }
    assert_eq!(present.detail, absent.detail);
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&sibling).ok();
}

#[test]
fn verify_absolute_path_under_root_is_jailed_like_a_relative_one() {
    let root = temp_project();
    let inside = format!("{}:3", root.join("src/foo.rs").display());
    assert_eq!(
        verify_source_marker(&inside, Some(&root)).status,
        SourceStatus::Verified
    );
    let missing = root.join("src/ghost.rs").display().to_string();
    assert_eq!(
        verify_source_marker(&missing, Some(&root)).status,
        SourceStatus::NotFound
    );
    // A `..` that climbs out of the root is outside, even if written under it.
    let climb = format!("{}/src/../../etc/passwd", root.display());
    assert_eq!(
        verify_source_marker(&climb, Some(&root)).status,
        SourceStatus::OutsideProject
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_absolute_path_without_root_is_unchecked() {
    let c = verify_source_marker("/etc/passwd", None);
    assert_eq!(c.status, SourceStatus::Unchecked, "{c:?}");
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_symlink_escape_gives_no_existence_oracle() {
    // Both targets lie behind the same escaping link: an existing and a
    // missing file outside must get the same verdict, through a relative
    // or an absolute citation.
    use std::os::unix::fs::symlink;
    let root = temp_project();
    let outside =
        std::env::temp_dir().join(format!("kronn_antihalluc_out_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "leaked\n").unwrap();
    symlink(&outside, root.join("link")).unwrap();
    let abs = |name: &str| root.join("link").join(name).display().to_string();
    for raw in [
        "link/secret.txt".to_string(),
        "link/missing.txt".to_string(),
        abs("secret.txt"),
        abs("missing.txt"),
    ] {
        let c = verify_source_marker(&raw, Some(&root));
        assert_eq!(c.status, SourceStatus::OutsideProject, "{raw}: {c:?}");
    }
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_symlink_with_dotdot_target_escaping_is_outside() {
    // A relative link target that climbs out (`../..`) is refused.
    use std::os::unix::fs::symlink;
    let root = temp_project();
    symlink("../../../../../../../../etc", root.join("src/etc")).unwrap();
    let c = verify_source_marker("src/etc/passwd", Some(&root));
    assert_eq!(c.status, SourceStatus::OutsideProject, "{c:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_symlink_with_absolute_target_inside_root_is_verified() {
    use std::os::unix::fs::symlink;
    let root = temp_project();
    symlink(root.join("src"), root.join("alias")).unwrap();
    let c = verify_source_marker("alias/foo.rs:5", Some(&root));
    assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_symlink_loop_terminates_as_not_found() {
    use std::os::unix::fs::symlink;
    let root = temp_project();
    symlink("b", root.join("a")).unwrap();
    symlink("a", root.join("b")).unwrap();
    let c = verify_source_marker("a/x.rs", Some(&root));
    assert_eq!(c.status, SourceStatus::NotFound, "{c:?}");
    std::fs::remove_dir_all(&root).ok();
}

// P0-5 of the QA roadmap — symlink escape via a RELATIVE path. The
// lexical jail catches `../` traversal in `verify_relative_path_…`
// above ; it does NOT catch a symlink whose components look innocent
// but whose canonical resolution points outside the project.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_relative_symlink_escape_is_outside_project() {
    use std::os::unix::fs::symlink;

    let root = temp_project();
    // Create a directory OUTSIDE the project root that holds a secret.
    let outside =
        std::env::temp_dir().join(format!("kronn_antihalluc_outside_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "leaked\n").unwrap();
    // Plant a symlink INSIDE the project root that points at the
    // outside dir. A naïve lexical check on `link/secret.txt` would
    // resolve to root/link/secret.txt → starts_with(root) → pass.
    symlink(&outside, root.join("link")).unwrap();

    let c = verify_source_marker("link/secret.txt", Some(&root));
    assert_eq!(
        c.status,
        SourceStatus::OutsideProject,
        "symlink escape must be caught by the jailed walk; got {c:?}"
    );
    assert!(
        c.detail.to_lowercase().contains("symlink"),
        "detail should hint at symlink: {}",
        c.detail
    );

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn verify_relative_symlink_inside_project_stays_verified() {
    // Companion to the escape test — a symlink INSIDE the project
    // (pointing at another file inside the same root) must NOT be
    // rejected. Otherwise the canonicalize guard would over-fire
    // on legitimate symlinked vendored deps / submodules.
    use std::os::unix::fs::symlink;

    let root = temp_project();
    // root/src/foo.rs already exists ; alias it as root/alias.rs.
    symlink(root.join("src/foo.rs"), root.join("alias.rs")).unwrap();

    let c = verify_source_marker("alias.rs", Some(&root));
    assert_eq!(
        c.status,
        SourceStatus::Verified,
        "intra-project symlinks must remain Verified: {c:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_empty_ref_and_no_root() {
    let root = temp_project();
    assert_eq!(
        verify_source_marker("file:", Some(&root)).status,
        SourceStatus::EmptyRef
    );
    // No project root → can't resolve a file path → Unchecked, not fabricated.
    assert_eq!(
        verify_source_marker("src/foo.rs:1", None).status,
        SourceStatus::Unchecked
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_url_and_user_are_unchecked_not_fabricated() {
    let url = verify_source_marker("url: https://example.com/doc", Some(Path::new("/tmp")));
    assert_eq!(url.kind, SourceKind::Url);
    assert_eq!(url.status, SourceStatus::Unchecked);
    assert!(!url.status.is_fabricated());
    let user = verify_source_marker("user-confirmed 2026-05-25", Some(Path::new("/tmp")));
    assert_eq!(user.kind, SourceKind::User);
    assert!(!user.status.is_fabricated());
}

#[test]
fn analyze_combines_heuristic_and_mechanical() {
    let root = temp_project();
    let txt = "The retry logic is implemented [src: src/foo.rs:2]. \
                   The cache is implemented [src: src/ghost.rs:1]. \
                   The worker pool is implemented with great care everywhere.";
    let r = analyze(txt, Some(&root));
    // ghost.rs doesn't exist → 1 fabricated; foo.rs:2 verified.
    assert_eq!(r.fabricated_count, 1, "sources: {:?}", r.sources);
    assert_eq!(r.sources.len(), 2);
    // The third sentence has a cue + no anchor → niveau 0 flags it; the
    // first two are anchored by their [src:] markers so they don't.
    assert_eq!(r.unsourced_count, 1, "spans: {:?}", r.flagged_spans);
    assert!(!r.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

// ── 0.8.8 — P4 heuristic precision (false-positive fixes) ─────────

#[test]
fn di_opinion_conditional_not_flagged() {
    // The real-world false positive: an opinion inside a conditional.
    let t = "- La conformité DI n'est pas toujours une amélioration — si le \
                 cycle de vie est géré par le DOM, DI peut être un anti-pattern ici";
    assert_eq!(
        lint_assertions(t).unsourced_count,
        0,
        "{:?}",
        lint_assertions(t)
    );
}

#[test]
fn genuine_unsourced_claim_still_flags() {
    // No suppressor, no anchor, a clear claim cue → must still flag.
    let t = "La fonction handleAuth est définie côté serveur dans le module auth";
    assert_eq!(lint_assertions(t).unsourced_count, 1);
}

#[test]
fn hedge_accent_space_variant_suppresses() {
    // "peut être" (space + accent) must suppress like "peut-être" — the
    // accent/hyphen gap that let the DI sentence through.
    let t = "Le cache est géré localement, ça peut être un peu lent au démarrage";
    assert_eq!(lint_assertions(t).unsourced_count, 0);
}

#[test]
fn conditional_cue_not_flagged() {
    let t = "Si le paramètre timeout est configuré à zéro, la requête ne coupe jamais";
    assert_eq!(lint_assertions(t).unsourced_count, 0);
}

#[test]
fn conditional_si_does_not_match_inside_version() {
    // Regression: "si" must NOT match inside "version" and wrongly suppress.
    let t = "La version 7.3.6 est vulnérable à une faille de désérialisation";
    assert!(
        lint_assertions(t).unsourced_count >= 1,
        "{:?}",
        lint_assertions(t)
    );
}

#[test]
fn question_and_heading_not_flagged() {
    assert_eq!(
        lint_assertions("Quand tu dis que le VCL est configuré, tu veux dire quoi exactement ?")
            .unsourced_count,
        0
    );
    assert_eq!(
        lint_assertions("### Problème 3 — où la route est définie côté backend").unsourced_count,
        0
    );
}

#[test]
fn opinion_recommendation_not_flagged() {
    assert_eq!(
        lint_assertions("Le paramètre devrait être configuré côté infra plutôt qu'ici")
            .unsourced_count,
        0
    );
    assert_eq!(
        lint_assertions("Je recommande que la route soit gérée par un middleware dédié")
            .unsourced_count,
        0
    );
}

// ── 0.8.8 — P2 green badge (has_signal / verified_count) ──────────

#[test]
fn verified_only_report_has_signal_green() {
    let root = temp_project();
    let r = analyze(
        "The retry logic is implemented [src: file: src/foo.rs:2].",
        Some(&root),
    );
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.unsourced_count, 0);
    assert!(r.verified_count() >= 1, "sources: {:?}", r.sources);
    assert!(
        r.has_signal(),
        "all-verified report must carry a signal (green)"
    );
    assert!(!r.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn unchecked_only_report_has_signal_option_b() {
    // Option B (2026-05-30): a bare URL can't be machine-verified, but we
    // still SURFACE it (neutral "unverifiable" pill) — warn about everything.
    let r = analyze(
        "See the upstream docs [src: url: https://example.com/x].",
        None,
    );
    assert_eq!(r.fabricated_count, 0);
    assert_eq!(r.unsourced_count, 0);
    assert_eq!(r.verified_count(), 0, "sources: {:?}", r.sources);
    assert!(!r.sources.is_empty(), "the url source must be listed");
    assert!(
        r.has_signal(),
        "Option B: an unchecked-only report still has a signal"
    );
    assert!(!r.is_empty());
}

#[test]
fn truly_empty_report_has_no_signal() {
    let r = LintReport::empty();
    assert!(!r.has_signal());
    assert!(r.is_empty());
}

// ── 0.8.8 — P1 niveau-1.5 natural inline anchors ──────────────────

#[test]
fn inline_backticked_anchor_auto_verified_green() {
    let root = temp_project();
    // No [src:] marker — just a natural backticked `path:line` anchor.
    let r = analyze(
        "The retry path lives in `src/foo.rs:3`, see there.",
        Some(&root),
    );
    assert_eq!(r.fabricated_count, 0);
    assert!(r.verified_count() >= 1, "sources: {:?}", r.sources);
    assert!(r.has_signal());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn inline_nonexistent_anchor_is_dropped_not_fabricated() {
    // A backticked path that doesn't resolve must NOT become a red
    // fabricated flag (niveau-1.5 is positive-only).
    let root = temp_project();
    let r = analyze("Logic in `src/ghost.rs:1` handles it.", Some(&root));
    assert_eq!(r.fabricated_count, 0, "sources: {:?}", r.sources);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn inline_anchor_not_double_counted_with_src_marker() {
    // Same file cited BOTH as a formal [src:] marker AND inline backtick
    // must count as ONE verified source, not two (no green-chip inflation).
    let root = temp_project();
    let txt = "The retry logic [src: file: src/foo.rs:2] lives in `src/foo.rs:3`.";
    let r = analyze(txt, Some(&root));
    assert_eq!(r.verified_count(), 1, "sources: {:?}", r.sources);
    std::fs::remove_dir_all(&root).ok();
}

// ── 0.8.8 — finalize_lint_report (the extracted finalize seam) ────
// ONE sequential test: `set_mode` mutates a process-global, so splitting
// these into parallel tests would race (the known "mode-drift" risk).
#[serial_test::serial] // mutates the process-global anti-halluc mode
#[test]
fn finalize_lint_report_behavior() {
    let root = temp_project();
    let worktree = temp_project();
    let main = std::env::temp_dir().join(format!("kronn_main_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&main).unwrap();

    // 1. Mode off → always None, even with a perfectly-verified citation.
    let mode = ModeOverride::set("off");
    assert!(
        finalize_lint_report(
            "Implemented [src: file: src/foo.rs:2].",
            None,
            root.to_str().unwrap(),
            &[]
        )
        .is_none(),
        "off mode must not produce a report"
    );

    // 2. Mode warn → a verified citation produces a stored GREEN report.
    mode.switch("warn");
    let green = finalize_lint_report(
        "Retry logic [src: file: src/foo.rs:2].",
        None,
        root.to_str().unwrap(),
        &[],
    )
    .expect("verified report must be stored");
    assert_eq!(green.fabricated_count, 0);
    assert_eq!(green.unsourced_count, 0);
    assert!(green.verified_count() >= 1, "{:?}", green.sources);

    // 3. Plain prose, no cue, no citation → no signal → None.
    assert!(finalize_lint_report("Voilà, c'est fait.", None, "", &[]).is_none());

    // 4. Worktree root tried FIRST: file exists only in the worktree.
    let wt = finalize_lint_report(
        "See `src/foo.rs:1`.",
        Some(worktree.to_str().unwrap()),
        main.to_str().unwrap(),
        &[],
    )
    .expect("worktree-local file must verify (green)");
    assert!(wt.verified_count() >= 1, "{:?}", wt.sources);

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&worktree).ok();
    std::fs::remove_dir_all(&main).ok();
}

#[test]
fn inline_anchor_requires_path_separator_and_ext() {
    // Bare backticked identifiers / calls are NOT treated as file refs.
    assert!(!looks_like_file_anchor("handleAuth()"));
    assert!(!looks_like_file_anchor("foo.rs")); // no slash
    assert!(!looks_like_file_anchor("src/foo")); // no ext
    assert!(looks_like_file_anchor("src/foo.rs"));
    assert!(looks_like_file_anchor("backend/src/lib.rs:440"));
}

// ── 0.8.8 — P3 citation cleaning + multi-root fallback ────────────

#[test]
fn verify_tolerates_backticks_quotes_punct() {
    let root = temp_project();
    for raw in [
        "`src/foo.rs:2`",
        "\"src/foo.rs\"",
        "'src/foo.rs'",
        "(src/foo.rs)",
        "src/foo.rs.",
    ] {
        let c = verify_source_marker(raw, Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "raw={raw} -> {c:?}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn clean_reference_strips_wrappers() {
    assert_eq!(clean_reference("`src/a.rs:1`"), "src/a.rs:1");
    assert_eq!(clean_reference("\"src/a.rs\""), "src/a.rs");
    assert_eq!(clean_reference("(src/a.rs)."), "src/a.rs");
    assert_eq!(clean_reference("src/a.rs"), "src/a.rs");
}

// Regression (2026-06): the dominant citation false-positive. Agents write
// a markdown-backticked path with the line spec OUTSIDE the backticks —
// `src/foo.rs`:3 — so the closing backtick is internal at clean time and
// survives onto path_str after the :line split. Re-cleaning path_str fixes
// it. (The `src/foo.rs:3` form, line INSIDE, was already handled.)
#[test]
fn verify_backticked_path_with_line_spec_outside() {
    let root = temp_project(); // src/foo.rs = 5 lines
    let abs = format!("`{}/src/foo.rs`:3", root.display());
    let cases: &[(&str, SourceStatus)] = &[
        ("`src/foo.rs`:3", SourceStatus::Verified),
        ("`src/foo.rs`:2-4", SourceStatus::Verified),
        ("`src/foo.rs`:7", SourceStatus::OutOfBounds), // path resolved AND bounds ran
        ("`src/foo.rs`", SourceStatus::Verified),
        ("file: `src/foo.rs`:3", SourceStatus::Verified), // file: prefix + backticks combined
        (abs.as_str(), SourceStatus::Verified),           // absolute, backticked, line outside
        ("`src/ghost.rs`:3", SourceStatus::NotFound),     // clean path, genuinely absent
    ];
    for (raw, want) in cases {
        let c = verify_source_marker(raw, Some(&root));
        assert_eq!(c.status, *want, "raw={raw:?} -> {c:?}");
    }
    // A reference that's nothing but a wrapper + line must not false-Verify
    // against the root directory.
    let empty = verify_source_marker("`:3", Some(&root));
    assert_eq!(empty.status, SourceStatus::EmptyRef, "{empty:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn path_segment_suffix_is_aligned() {
    assert!(path_has_segment_suffix("apps/x.php", "apps/x.php")); // equal
    assert!(path_has_segment_suffix(
        "/r/application/apps/x.php",
        "apps/x.php"
    ));
    assert!(!path_has_segment_suffix("/r/myapps/x.php", "apps/x.php")); // not segment-aligned
    assert!(!path_has_segment_suffix("/r/apps/xx.php", "apps/x.php")); // filename differs
}

// The dominant inline-anchor FP: project code lives under `application/`, so
// a path cited relative to the app root (`apps/website/src/X.php`) misses at
// the project root and must be resolved by a unique segment-aligned suffix.
#[test]
fn verify_resolves_app_subdir_relative_by_suffix() {
    let root = temp_project();
    std::fs::create_dir_all(root.join("application/apps/website/src")).unwrap();
    std::fs::write(
        root.join("application/apps/website/src/Justin.php"),
        "a\nb\nc\n",
    )
    .unwrap();
    let c = verify_source_marker("apps/website/src/Justin.php:2", Some(&root));
    assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
    assert!(c.detail.contains("unique path suffix"), "{c:?}");
    // Bounds still enforced on the resolved file.
    let oob = verify_source_marker("apps/website/src/Justin.php:9", Some(&root));
    assert_eq!(oob.status, SourceStatus::OutOfBounds, "{oob:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_suffix_ambiguous_stays_not_found() {
    let root = temp_project();
    std::fs::create_dir_all(root.join("a/dir")).unwrap();
    std::fs::create_dir_all(root.join("b/dir")).unwrap();
    std::fs::write(root.join("a/dir/Same.php"), "x\n").unwrap();
    std::fs::write(root.join("b/dir/Same.php"), "x\n").unwrap();
    let c = verify_source_marker("dir/Same.php", Some(&root));
    assert_eq!(
        c.status,
        SourceStatus::NotFound,
        "ambiguous suffix must not verify: {c:?}"
    );
    assert!(c.detail.contains("ambiguous"), "{c:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_cross_repo_path_via_linked_root() {
    // A sibling repo passed as a second root: a repo-name-prefixed citation
    // `front_node/plugins/...` resolves against the linked root.
    let main = temp_project();
    let parent = std::env::temp_dir().join(format!("kronn_node_service_{}", uuid::Uuid::new_v4()));
    let sibling = parent.join("front_node");
    std::fs::create_dir_all(sibling.join("plugins/timeline")).unwrap();
    std::fs::write(sibling.join("plugins/timeline/index.js"), "a\nb\n").unwrap();
    let c = verify_source_marker_roots(
        "front_node/plugins/timeline/index.js:1",
        &[main.as_path(), sibling.as_path()],
    );
    assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
    // A genuinely absent cross-repo path still fails.
    let ghost = verify_source_marker_roots(
        "front_node/plugins/ghost/nope.js",
        &[main.as_path(), sibling.as_path()],
    );
    assert_eq!(ghost.status, SourceStatus::NotFound, "{ghost:?}");
    std::fs::remove_dir_all(&main).ok();
    std::fs::remove_dir_all(&parent).ok();
}

#[test]
fn verify_falls_back_to_second_root() {
    // File exists only in root #1 (worktree), not root #2 (main checkout).
    let worktree = temp_project();
    let main = std::env::temp_dir().join(format!("kronn_main_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&main).unwrap(); // empty: no src/foo.rs
    let c = verify_source_marker_roots("src/foo.rs:1", &[worktree.as_path(), main.as_path()]);
    assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
    std::fs::remove_dir_all(&worktree).ok();
    std::fs::remove_dir_all(&main).ok();
}

#[test]
fn verify_traversal_still_jailed_multi_root() {
    let a = temp_project();
    let b = temp_project();
    let c = verify_source_marker_roots("../../../../etc/passwd:1", &[a.as_path(), b.as_path()]);
    assert_ne!(c.status, SourceStatus::Verified, "{c:?}");
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

#[test]
fn verify_missing_in_all_roots_is_not_found() {
    let a = temp_project();
    let b = temp_project();
    let c = verify_source_marker_roots("src/ghost.rs:1", &[a.as_path(), b.as_path()]);
    assert_eq!(c.status, SourceStatus::NotFound, "{c:?}");
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

// ── 0.8.8 — P5 PREAMBLE wording invariants ────────────────────────

#[test]
fn preamble_blesses_natural_anchors_and_opinion_convention() {
    // The new wording must tell agents natural backticked paths count …
    assert!(
        PREAMBLE.contains("backticked"),
        "PREAMBLE must bless natural anchors"
    );
    // … and give them a way to mark opinions/guesses.
    assert!(
        PREAMBLE.contains("recommendation") || PREAMBLE.contains("recommande"),
        "PREAMBLE must describe the opinion-marking convention"
    );
}

// ── corpus precision guard ────────────────────────────────────────
//
// 0.8.8 — a CORPUS-BASED precision pin for the niveau-0 prose
// heuristic (`lint_assertions`). Distinct from the JSONL fixture
// gate above (`corpus_false_positive_rate_under_5_percent`): this is
// an inline, hand-curated, mixed FR/EN/ES set split into two labelled
// arrays so a regression NAMES the exact culprit sentence.
//
//   - SHOULD_FLAG   : genuine confident technical assertions about
//     THIS codebase — claim cue, NO anchor, NO hedge. Each MUST
//     produce `unsourced_count >= 1`. Pins RECALL.
//   - MUST_NOT_FLAG : the known false-positive classes (opinion,
//     conditional, question, heading, imperative bullet, hedge,
//     anchored, third-party disclaimer, bare semver / proper noun).
//     Each MUST produce `unsourced_count == 0`. Pins PRECISION.
//
// If a future heuristic change silently flips precision or recall,
// one of the two tests below fails and prints the offending sentence.

/// Genuine confident technical claims that MUST flag (recall pin).
/// All anchorless, hedgeless, cued. Mixed FR / EN / ES.
const SHOULD_FLAG: &[&str] = &[
    // ── FR — code / config ──
    "La fonction handleAuth est définie dans le module auth côté serveur",
    "La route de déconnexion est gérée par le middleware de session",
    "Le paramètre de timeout est configuré à trente secondes pour tous",
    "La table des sessions est stockée dans la base principale du backend",
    "La méthode de cache renvoie systématiquement la valeur expirée ici",
    // ── EN — code / config ──
    "The handleAuth function is defined in the server-side auth module",
    "The endpoint returns a 500 error when given a null input payload",
    "The retry logic is implemented with an exponential backoff loop here",
    "The session token is stored in the primary database table by default",
    "The cache invalidation flag is set in the startup configuration block",
    // ── ES — security / CVE (ES code/config verbs like "está
    //   configurado" / "está definido" have NO cue in CLAIM_CUES — a
    //   documented recall gap, see HEURISTIC WEAKNESSES below. The
    //   cross-lingual cues that DO fire on Spanish prose are the
    //   tokens shared verbatim: "CVE-" and bare-frame English verbs.
    //   This ES claim flags via the language-agnostic "cve-" cue.) ──
    "CVE-2025-64500 afecta a la versión 7.3.6 del framework según el aviso",
    // ── FR — security / CVE / version ──
    "La version 7.3.6 du framework est vulnérable à une faille PATH_INFO",
    "La faille de désérialisation est corrigé dans la release 7.3.7 publiée",
    "Les versions affectées vont de la 6.0 jusqu'à la 7.3.6 inclusivement",
    "La dernière version est la 8.2.0 sortie ce matin sur le dépôt amont",
    // ── EN — security / CVE / version ──
    "CVE-2025-64500 is a high-severity flaw in the request handling layer",
    "The serialization bug is fixed in version 7.3.7 of the upstream library",
    "The deserialization gadget is vulnerable to a remote code execution path",
];

/// False-positive classes that MUST NOT flag (precision pin).
/// Mixed FR / EN / ES, one sentence per failure mode.
const MUST_NOT_FLAG: &[&str] = &[
    // ── opinions / recommendations ──
    "Les services dédiés sont préférables pour ce genre de découpage métier",
    "On devrait extraire ça dans un module séparé pour clarifier la logique",
    "Je recommande que la route soit gérée par un middleware dédié et testé",
    "À mon avis le paramètre devrait être configuré côté infra plutôt qu'ici",
    "I recommend the endpoint should be handled by a dedicated gateway layer",
    "It would be better if the cache were configured with a shorter window",
    // ── conditionals / hypotheticals ──
    "Si le cache est configuré ainsi, la fonction renvoie une valeur périmée",
    "Quand le flag est activé, la route est gérée par le nouveau middleware",
    "When the feature flag is on, the endpoint returns the cached response",
    "Lorsque le timeout est configuré à zéro, la requête ne coupe jamais ici",
    "If the parameter is set in the override block, the default is ignored",
    // ── questions ──
    // NOTE: a question is reliably suppressed ONLY when it has no
    // claim cue, OR a conditional opener precedes the cue. A bare
    // interrogative ("Où … est configuré ?", "Est-ce que … est
    // définie ?") is NOT suppressed by `is_question` — see HEURISTIC
    // WEAKNESS #1 below — so those two FR forms are parked in the
    // documented BORDERLINE list, not asserted here.
    "Where is the retry logic implemented in the current backend codebase ?",
    // ── markdown headings ──
    "### Problème 3 — où la route de déconnexion est définie côté backend",
    "## La fonction handleAuth est définie où exactement dans ce dépôt **",
    // ── imperative bullets ──
    "- Spécifier l'endpoint exact que la passerelle doit appeler au démarrage",
    "- Vérifier que le paramètre de timeout est configuré correctement partout",
    "- Add the missing endpoint and configure the retry flag in the manifest",
    // ── hedged ──
    "Je pense que la fonction handleAuth est définie dans le module auth ici",
    "Peut-être que l'endpoint renvoie une 500 sur une entrée nulle, à vérifier",
    "Le cache est géré localement, ça peut être un peu lent au tout démarrage",
    "The endpoint probably returns a 500 on null input, but let me check first",
    // ── anchored (backtick path / [src:] / URL) ──
    "The retry logic is implemented in `backend/src/core/retry.rs` for callers",
    "La fonction de cache se trouve dans `backend/src/core/cache.rs` au démarrage",
    "The default window is thirty minutes [src: file: src/config.rs:12] exactly",
    "The endpoint returns paginated JSON per https://api.example.com/docs spec",
    // ── third-party / runtime disclaimers ──
    // The FR "X est géré par l'équipe infra" form carries the "est
    // géré" cue and reads as a confident attribution claim, so the
    // linter flags it — arguably correct (it IS an unsourced claim
    // about ownership). Parked in BORDERLINE, not asserted here. The
    // EN disclaimer below carries no cue → reliably silent.
    "That part is owned by the platform team and lives outside this repository",
    // ── bare semver / proper nouns (no claim cue) ──
    "Version 1.2.3 sort demain selon le calendrier de release prévu par avance",
    "L'équipe d'Élodie a configuré le déploiement chez le fournisseur cloud hier",
];

/// Genuinely-ambiguous sentences whose CURRENT linter verdict is a
/// known, documented edge — NOT asserted as pass/fail here, kept so
/// the corpus stays honest (they were NOT deleted to make a test
/// green). See HEURISTIC WEAKNESSES below. Each tuple is
/// `(sentence, currently_flags)`.
const BORDERLINE: &[(&str, bool)] = &[
    // WEAKNESS #1 — FIXED 2026-05-29. Bare interrogatives used to be
    // flagged: `split_sentences` consumed the terminal `?` as a
    // boundary delimiter and DROPPED it, so `is_question` (which checks
    // `ends_with('?')`) never fired on a trailing question. The fix
    // retains a terminating `?`/`!` on the flushed sentence, so a cued
    // question (incl. the French "… ?" with the standard space-before-?)
    // is now correctly suppressed. These two no longer flag:
    (
        "Où est-ce que le paramètre de timeout est configuré dans ce projet ?",
        false,
    ),
    (
        "Est-ce que la fonction handleAuth est définie côté serveur ou client ?",
        false,
    ),
    // WEAKNESS #2 — an ownership attribution ("X est géré par l'équipe
    // Y") carries the "est géré" cue and is flagged. Defensible as a
    // genuine unsourced claim, but it reads to a human as a scoping
    // disclaimer, so it is borderline rather than a clean FP:
    (
        "Ce point est géré par l'équipe infra, pas dans ce repo applicatif côté nous",
        true,
    ),
];

// ── HEURISTIC WEAKNESSES (found while building this corpus) ────────
//
// #1  Bare-interrogative blind spot (precision) — FIXED 2026-05-29.
//     `is_question` WAS effectively unreachable for a question that
//     ENDS the text: `split_sentences` stripped the terminal `?` before
//     `is_question` ran, so a cued question with no conditional opener
//     ("Où … est configuré ?") was wrongly flagged. The fix retains the
//     terminating `?`/`!` on the flushed sentence; the BORDERLINE tuples
//     above now record the corrected (non-flagging) verdict.
//
// #2  No Spanish cue coverage (recall). CLAIM_CUES has zero ES verbs:
//     "está configurado / definido / implementado / almacenado" don't
//     match anything. Spanish technical claims only flag when they
//     reuse a language-agnostic token ("CVE-", a backticked path, an
//     English frame). Genuine ES prose claims pass through unflagged.
//
// #3  Ownership-attribution overlap (precision). "est géré par …"
//     fires on scoping disclaimers that a human reads as "not my
//     problem", not as a checkable assertion. Borderline by design.

#[test]
fn borderline_cases_documented_not_silently_dropped() {
    // We don't assert these are "right" — only that their CURRENT
    // verdict matches what the comment block above claims, so the doc
    // can't drift away from the code without this test catching it.
    for &(s, expected_flags) in BORDERLINE {
        let flags = lint_assertions(s).unsourced_count >= 1;
        assert_eq!(
                flags, expected_flags,
                "BORDERLINE verdict drifted for {s:?}: doc says flags={expected_flags}, got flags={flags}. \
                 Update the HEURISTIC WEAKNESSES note (and the heuristic if this is a real fix).",
            );
    }
}

#[test]
fn corpus_should_flag_all_genuine_claims() {
    // RECALL pin: every genuine confident claim must flag. On failure
    // we collect the culprits so a regression names exactly which
    // sentence the heuristic stopped catching.
    let mut missed: Vec<&str> = Vec::new();
    for &s in SHOULD_FLAG {
        let r = lint_assertions(s);
        if r.unsourced_count < 1 {
            missed.push(s);
        }
    }
    assert!(
        missed.is_empty(),
        "{}/{} genuine claims were NOT flagged (recall regression):\n  - {}",
        missed.len(),
        SHOULD_FLAG.len(),
        missed.join("\n  - "),
    );
}

#[test]
fn corpus_false_positive_rate_under_threshold() {
    // PRECISION pin: every false-positive-class sentence must stay at
    // unsourced_count == 0. Target FP count is 0; we keep a ≤ 5%
    // tolerance band so a single genuinely-borderline case doesn't
    // hard-fail the suite, but we ALWAYS print the offenders so the
    // regression is visible and a human can decide.
    let mut offenders: Vec<String> = Vec::new();
    for &s in MUST_NOT_FLAG {
        let r = lint_assertions(s);
        if r.unsourced_count != 0 {
            offenders.push(format!(
                "[cue={}] \"{}\"",
                r.flagged_spans
                    .first()
                    .map(|f| f.reason.as_str())
                    .unwrap_or("?"),
                s,
            ));
        }
    }
    let total = MUST_NOT_FLAG.len();
    let fp = offenders.len();
    let fp_rate = fp as f64 / total as f64;
    assert!(
        fp_rate <= 0.05,
        "FP rate {:.1}% exceeds 5% gate ({fp}/{total}) — precision regression:\n  {}",
        fp_rate * 100.0,
        offenders.join("\n  "),
    );
    // Document the ideal: we currently expect ZERO false positives.
    // If this ever trips while the rate gate still passes, a borderline
    // case slipped in — move it to a documented borderline list above
    // rather than silently tolerating drift.
    assert_eq!(
        fp,
        0,
        "expected zero false positives on the curated corpus, got {fp}:\n  {}",
        offenders.join("\n  "),
    );
}

// ════════════════════════════════════════════════════════════════════
// EXHAUSTIVE MATRIX COVERAGE (added 2026-05-29)
// Grouped in sub-modules; each re-imports the parent scope (`super::*`)
// so `temp_project()`, the public API, and the private helpers are all
// in scope. No duplication of the baseline tests above — these widen the
// matrix on the gaps called out in the QA roadmap.
// ════════════════════════════════════════════════════════════════════

// ── 1. classify_source — full prefix matrix ──────────────────────────
mod classify_matrix {
    use super::*;

    #[test]
    fn every_typed_prefix_with_colon() {
        // (input, expected kind, expected stripped reference)
        let cases: &[(&str, SourceKind, &str)] = &[
            ("file: src/a.rs:10", SourceKind::File, "src/a.rs:10"),
            // A `url:`-prefixed value that contains an actual URL is caught
            // by the bare-URL substring check FIRST (before prefix
            // stripping), so the whole string is kept as the reference.
            // This is fine — Url is Unchecked, the reference is display-only.
            (
                "url: https://x.io/p",
                SourceKind::Url,
                "url: https://x.io/p",
            ),
            // A `url:`-prefixed value WITHOUT a scheme falls through to the
            // keyword branch and DOES get stripped.
            ("url: docs/guide", SourceKind::Url, "docs/guide"),
            (
                "user: 2026-05-25: standup note",
                SourceKind::User,
                "2026-05-25: standup note",
            ),
            ("api: GET /widgets#3", SourceKind::Api, "GET /widgets#3"),
            ("commit: deadbeef99", SourceKind::Commit, "deadbeef99"),
            (
                "code-comment: src/a.rs:4",
                SourceKind::CodeComment,
                "src/a.rs:4",
            ),
            (
                "inferred: from the pattern",
                SourceKind::Inferred,
                "from the pattern",
            ),
            (
                "hypothesis: maybe a cache",
                SourceKind::Hypothesis,
                "maybe a cache",
            ),
            (
                "training-data: i recall",
                SourceKind::TrainingData,
                "i recall",
            ),
        ];
        for (raw, kind, reference) in cases {
            let (k, r) = classify_source(raw);
            assert_eq!(k, *kind, "kind for {raw:?}");
            assert_eq!(r, *reference, "reference for {raw:?}");
        }
    }

    #[test]
    fn typed_prefix_with_whitespace_no_colon() {
        // `match_type_keyword` accepts `<keyword><space>` as a boundary too.
        assert_eq!(classify_source("file src/a.rs").0, SourceKind::File);
        assert_eq!(classify_source("commit deadbeef").0, SourceKind::Commit);
        assert_eq!(
            classify_source("inferred the design").0,
            SourceKind::Inferred
        );
        // …and the bare keyword on its own → empty remainder.
        let (k, r) = classify_source("hypothesis");
        assert_eq!(k, SourceKind::Hypothesis);
        assert_eq!(r, "");
    }

    #[test]
    fn bare_url_http_and_https_no_prefix() {
        assert_eq!(classify_source("https://example.com/a").0, SourceKind::Url);
        assert_eq!(classify_source("http://example.com/a").0, SourceKind::Url);
        // URL embedded after some prose still detected (substring match).
        assert_eq!(
            classify_source("see https://example.com here").0,
            SourceKind::Url
        );
    }

    #[test]
    fn user_confirmed_phrase_both_forms() {
        assert_eq!(
            classify_source("user-confirmed 2026-05-25").0,
            SourceKind::User
        );
        assert_eq!(
            classify_source("user confirmed by the lead").0,
            SourceKind::User
        );
        // Case-insensitive.
        assert_eq!(
            classify_source("User-Confirmed yesterday").0,
            SourceKind::User
        );
    }

    #[test]
    fn token_boundary_real_filenames_are_file_not_type() {
        // A file whose NAME starts with a type keyword must stay File —
        // the keyword-boundary check is the guard. Exhaustive sweep over
        // every prefix that has a same-named-file collision risk.
        let collisions = [
            "user_service.rs:10",
            "userland.ts",
            "api_client.rs:1",
            "apiconfig.json",
            "commit_log.rs",
            "committed.txt",
            "inferred_types.ts",
            "filesystem.rs",
            "urls.txt",
            "hypotheses.md",
        ];
        for path in collisions {
            assert_eq!(
                classify_source(path).0,
                SourceKind::File,
                "{path} must be File (token boundary), not a typed source",
            );
        }
    }

    #[test]
    fn unknown_bare_token_falls_back_to_file() {
        // No known prefix, no URL, no user phrase → File fallback, whole
        // string kept as the reference.
        let (k, r) = classify_source("just/some/path.rs:3");
        assert_eq!(k, SourceKind::File);
        assert_eq!(r, "just/some/path.rs:3");
    }

    #[test]
    fn training_data_wins_over_substring_collisions() {
        // `training-data` is checked first; it must not be shadowed by a
        // later substring match.
        assert_eq!(classify_source("training-data").0, SourceKind::TrainingData);
    }

    #[test]
    fn api_keyword_does_not_swallow_url_prefix() {
        // `url:` and `api:` are distinct keywords. A `url:` whose value
        // happens to contain "api" stays Url.
        assert_eq!(
            classify_source("url: https://api.example.com").0,
            SourceKind::Url
        );
    }
}

// ── 2. verify_source_marker — full status matrix ─────────────────────
mod verify_status_matrix {
    use super::*;

    #[test]
    fn verified_exists_single_line_in_bounds() {
        let root = temp_project();
        assert_eq!(
            verify_source_marker("src/foo.rs:1", Some(&root)).status,
            SourceStatus::Verified
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verified_range_fully_in_bounds() {
        let root = temp_project();
        assert_eq!(
            verify_source_marker("src/foo.rs:1-5", Some(&root)).status,
            SourceStatus::Verified
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn not_found_relative_missing_file() {
        let root = temp_project();
        assert_eq!(
            verify_source_marker("src/missing.rs", Some(&root)).status,
            SourceStatus::NotFound
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn out_of_bounds_single_line_and_range() {
        let root = temp_project(); // foo.rs has 5 lines
        assert_eq!(
            verify_source_marker("src/foo.rs:99", Some(&root)).status,
            SourceStatus::OutOfBounds
        );
        assert_eq!(
            verify_source_marker("src/foo.rs:3-50", Some(&root)).status,
            SourceStatus::OutOfBounds
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_ref_variants() {
        let root = temp_project();
        for raw in ["file:", "file: ", "file:   "] {
            assert_eq!(
                verify_source_marker(raw, Some(&root)).status,
                SourceStatus::EmptyRef,
                "raw={raw:?}",
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn outside_project_relative_escape() {
        let root = temp_project();
        assert_eq!(
            verify_source_marker("../escape.rs", Some(&root)).status,
            SourceStatus::OutsideProject,
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unchecked_soft_and_network_tiers() {
        let root = temp_project();
        // No root → Unchecked.
        assert_eq!(
            verify_source_marker("src/foo.rs:1", None).status,
            SourceStatus::Unchecked
        );
        // URL / api / commit / user / inferred / hypothesis are never
        // file-verified — Unchecked regardless of root.
        let unchecked = [
            "url: https://x.io",
            "api: GET /x",
            "commit: abc1234",
            "user-confirmed 2026-01-01",
            "inferred: a guess",
            "hypothesis: maybe",
        ];
        for raw in unchecked {
            let c = verify_source_marker(raw, Some(&root));
            assert_eq!(c.status, SourceStatus::Unchecked, "raw={raw}");
            assert!(
                !c.status.is_fabricated(),
                "raw={raw} must not be fabricated"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejected_training_data_is_fabricated() {
        let root = temp_project();
        // Even pointing at a real file, training-data is Rejected.
        let c = verify_source_marker("training-data: src/foo.rs:1", Some(&root));
        assert_eq!(c.status, SourceStatus::Rejected);
        assert!(c.status.is_fabricated());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn code_comment_verified_but_low_trust_detail() {
        let root = temp_project();
        let c = verify_source_marker("code-comment: src/foo.rs:2", Some(&root));
        assert_eq!(c.status, SourceStatus::Verified);
        assert!(
            c.detail.to_lowercase().contains("not authoritative")
                || c.detail.to_lowercase().contains("code comment"),
            "low-trust detail expected: {}",
            c.detail,
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn absolute_path_under_root_is_verified() {
        let root = temp_project();
        // foo.rs lives under root; cite it by absolute path.
        let abs = root.join("src/foo.rs");
        let c = verify_source_marker(&format!("{}", abs.display()), Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn is_fabricated_covers_exactly_the_hard_failures() {
        // Pin the exact membership of `is_fabricated` — drives the red pill.
        assert!(SourceStatus::NotFound.is_fabricated());
        assert!(SourceStatus::OutOfBounds.is_fabricated());
        assert!(SourceStatus::EmptyRef.is_fabricated());
        assert!(SourceStatus::OutsideProject.is_fabricated());
        assert!(SourceStatus::Rejected.is_fabricated());
        // …and NOT the honest-uncertainty ones.
        assert!(!SourceStatus::Verified.is_fabricated());
        assert!(!SourceStatus::Unchecked.is_fabricated());
    }
}

// ── 3. clean_reference — wrapper-stripping edge cases ─────────────────
mod clean_reference_edges {
    use super::*;

    #[test]
    fn strips_every_single_wrapper_kind() {
        assert_eq!(clean_reference("`a/b.rs`"), "a/b.rs");
        assert_eq!(clean_reference("\"a/b.rs\""), "a/b.rs");
        assert_eq!(clean_reference("'a/b.rs'"), "a/b.rs");
        assert_eq!(clean_reference("<a/b.rs>"), "a/b.rs");
        assert_eq!(clean_reference("(a/b.rs)"), "a/b.rs");
    }

    #[test]
    fn strips_trailing_sentence_punctuation() {
        assert_eq!(clean_reference("a/b.rs."), "a/b.rs");
        assert_eq!(clean_reference("a/b.rs,"), "a/b.rs");
        assert_eq!(clean_reference("a/b.rs;"), "a/b.rs");
    }

    #[test]
    fn strips_nested_wrappers() {
        assert_eq!(clean_reference("`\"a/b.rs\"`"), "a/b.rs");
        assert_eq!(clean_reference("(`a/b.rs`)."), "a/b.rs");
        assert_eq!(clean_reference("<`a/b.rs:42`>,"), "a/b.rs:42");
    }

    #[test]
    fn leading_dot_slash_is_preserved_as_relative() {
        // `clean_reference` only strips wrappers/punct — it must NOT mangle
        // a `./` relative prefix into something else.
        assert_eq!(clean_reference("./src/a.rs"), "./src/a.rs");
        assert_eq!(clean_reference("`./src/a.rs`"), "./src/a.rs");
    }

    #[test]
    fn backtick_wrapped_escape_stays_an_escape_not_unwrapped_into_safety() {
        // A `../` smuggled inside backticks must STILL read as `../…` after
        // cleaning — clean_reference strips the ticks but never resolves the
        // path, so the downstream jail still sees the escape.
        assert_eq!(clean_reference("`../../etc/passwd`"), "../../etc/passwd");
        let root = temp_project();
        let c = verify_source_marker("`../../etc/passwd:1`", Some(&root));
        assert_eq!(
            c.status,
            SourceStatus::OutsideProject,
            "backtick-wrapped escape must remain jailed: {c:?}",
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_with_parens_in_name_round_trips_through_verify() {
        // clean_reference strips OUTER parens greedily, so we verify the
        // realistic case: a backtick-wrapped path verifies without paren
        // collision.
        let root = temp_project();
        std::fs::write(root.join("src/weird.rs"), "x\n").unwrap();
        let c = verify_source_marker("`src/weird.rs`", Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn idempotent_on_clean_input() {
        assert_eq!(clean_reference("src/a.rs:1-9"), "src/a.rs:1-9");
    }
}

// ── 4. multi-root resolution matrix ──────────────────────────────────
mod multi_root_matrix {
    use super::*;

    fn empty_root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("kronn_empty_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn found_in_root1_only() {
        let r1 = temp_project();
        let r2 = empty_root();
        let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&r1).ok();
        std::fs::remove_dir_all(&r2).ok();
    }

    #[test]
    fn found_in_root2_only() {
        let r1 = empty_root();
        let r2 = temp_project();
        let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&r1).ok();
        std::fs::remove_dir_all(&r2).ok();
    }

    #[test]
    fn found_in_neither_is_not_found() {
        let r1 = empty_root();
        let r2 = empty_root();
        let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
        assert_eq!(c.status, SourceStatus::NotFound, "{c:?}");
        std::fs::remove_dir_all(&r1).ok();
        std::fs::remove_dir_all(&r2).ok();
    }

    #[test]
    fn found_in_both_resolves_verified() {
        let r1 = temp_project();
        let r2 = temp_project();
        let c = verify_source_marker_roots("src/foo.rs:3", &[r1.as_path(), r2.as_path()]);
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&r1).ok();
        std::fs::remove_dir_all(&r2).ok();
    }

    #[test]
    fn escape_in_all_roots_is_outside_project() {
        let r1 = temp_project();
        let r2 = temp_project();
        let c = verify_source_marker_roots("../../../../etc/passwd", &[r1.as_path(), r2.as_path()]);
        assert_eq!(c.status, SourceStatus::OutsideProject, "{c:?}");
        std::fs::remove_dir_all(&r1).ok();
        std::fs::remove_dir_all(&r2).ok();
    }

    #[test]
    fn empty_roots_slice_is_unchecked() {
        let c = verify_source_marker_roots("src/foo.rs:1", &[]);
        assert_eq!(c.status, SourceStatus::Unchecked, "{c:?}");
    }
}

// ── 5. inline file anchors — looks_like / extract ────────────────────
mod inline_anchor_matrix {
    use super::*;

    #[test]
    fn every_supported_extension_is_an_anchor() {
        const EXTS: &[&str] = &[
            "rs", "ts", "tsx", "js", "jsx", "py", "toml", "json", "sql", "md", "yml", "yaml",
            "css", "html", "sh", "php", "go", "java", "rb", "vue", "svelte", "c", "cpp", "h",
        ];
        for ext in EXTS {
            let path = format!("src/dir/file.{ext}");
            assert!(looks_like_file_anchor(&path), "{path} should be an anchor");
        }
    }

    #[test]
    fn rejects_no_slash() {
        assert!(!looks_like_file_anchor("foo.rs"));
        assert!(!looks_like_file_anchor("Cargo.toml"));
    }

    #[test]
    fn rejects_no_extension() {
        assert!(!looks_like_file_anchor("src/foo"));
        assert!(!looks_like_file_anchor("a/b/c"));
    }

    #[test]
    fn accepts_root_level_file_with_explicit_line() {
        // Recall fix (live test, DOCROMS_WEB): a root-level file cited WITH
        // a line is a real file anchor even without a path separator —
        // `composer.json:1` resolved on disk but was previously dropped.
        assert!(looks_like_file_anchor("composer.json:1"));
        assert!(looks_like_file_anchor("Cargo.toml:5"));
        assert!(looks_like_file_anchor("README.md:3"));
        // … but the SAME filename with NO line + NO slash stays out, so
        // prose like `node.js` / `package.json` (mentioned, not cited) is
        // not mistaken for a citation.
        assert!(!looks_like_file_anchor("composer.json"));
        assert!(!looks_like_file_anchor("node.js"));
    }

    #[test]
    fn rejects_whitespace_and_empty() {
        assert!(!looks_like_file_anchor(""));
        assert!(!looks_like_file_anchor("src/foo bar.rs"));
    }

    #[test]
    fn accepts_anchor_with_line_and_range() {
        assert!(looks_like_file_anchor("src/foo.rs:42"));
        assert!(looks_like_file_anchor("a/b/c.ts:10-20"));
    }

    #[test]
    fn accepts_web_template_and_i18n_extensions() {
        // Symfony / web-project files. A 4-persona forensic re-pass found
        // EVERY `.twig` / `.xlf` citation went unverified because these were
        // missing from the allowlist (the central files of a Symfony repo).
        assert!(looks_like_file_anchor(
            "templates/pages/projets.html.twig:82"
        ));
        assert!(looks_like_file_anchor("templates/menu/header.html.twig"));
        assert!(looks_like_file_anchor("translations/messages.it.xlf"));
        assert!(looks_like_file_anchor("assets/styles/app.scss:10"));
        // Double extension needs no special case: ends_with(".twig").
        assert!(looks_like_file_anchor("a/b.html.twig"));
        // Precision kept: a bare root filename without slash OR :line stays
        // out (ambiguous), and bare extensionless prose never matches.
        assert!(!looks_like_file_anchor("twig"));
        assert!(!looks_like_file_anchor("projets.html.twig"));
    }

    #[test]
    fn ext_lists_are_unified_via_shared_const() {
        // contains_code_anchor (niveau-0) and looks_like_file_anchor
        // (niveau-1.5) now share SOURCE_EXTS so they cannot drift — a real
        // divergence the forensic re-pass found (.php lived in one list only).
        for ext in [".twig", ".xlf", ".scss", ".php", ".vue", ".rs"] {
            assert!(SOURCE_EXTS.contains(&ext), "SOURCE_EXTS must carry {ext}");
            // A path token with this ext + a separator reads as a niveau-0
            // anchor (so the citing sentence isn't flagged unsourced).
            let s = format!("voir `dir/file{ext}` pour les détails");
            assert!(
                contains_code_anchor(&s),
                "contains_code_anchor missed {ext}"
            );
        }
    }

    #[test]
    fn analyze_verifies_real_twig_anchor_green() {
        // Clarisse cited `templates/pages/projets.html.twig:82` (a real file,
        // line 82 exact on disk). Before the fix: dropped. Now: green.
        let root = temp_project();
        std::fs::create_dir_all(root.join("templates/pages")).unwrap();
        std::fs::write(
            root.join("templates/pages/projets.html.twig"),
            "l1\nl2\nl3\n",
        )
        .unwrap();
        let r = analyze(
            "La section va dans `templates/pages/projets.html.twig:2`.",
            Some(&root),
        );
        assert_eq!(
            r.verified_count(),
            1,
            "twig anchor must verify green: {:?}",
            r.sources
        );
        assert_eq!(r.fabricated_count, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn analyze_surfaces_out_of_bounds_twig_line_honestly() {
        // Lea cited `home/projets.html.twig:97-116` on a 24-line file. Now
        // that `.twig` resolves, an inline anchor with an out-of-bounds line
        // is surfaced as soft-amber "couldn't verify" (NOT silently dropped,
        // NOT red — inline anchors are lower-confidence than formal [src:]).
        let root = temp_project();
        std::fs::create_dir_all(root.join("templates/home")).unwrap();
        std::fs::write(root.join("templates/home/projets.html.twig"), "a\nb\nc\n").unwrap();
        let r = analyze(
            "Voir `templates/home/projets.html.twig:97-116`.",
            Some(&root),
        );
        assert_eq!(
            r.verified_count(),
            0,
            "out-of-bounds line must NOT be green"
        );
        assert_eq!(
            r.unverified_count, 1,
            "must surface as soft-amber: {:?}",
            r.sources
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extract_skips_fenced_code() {
        let txt = "See `src/a.rs`.\n```\n`src/fenced.rs`\n```\nEnd.";
        let anchors = extract_inline_file_anchors(txt);
        assert_eq!(anchors, vec!["src/a.rs"], "fenced anchor must be skipped");
    }

    #[test]
    fn extract_multiple_anchors_in_one_text() {
        let txt = "Both `src/a.rs:1` and `lib/b.ts` are relevant here.";
        let anchors = extract_inline_file_anchors(txt);
        assert_eq!(anchors, vec!["src/a.rs:1", "lib/b.ts"]);
    }

    #[test]
    fn extract_ignores_non_anchor_backticks() {
        // Backticked identifiers / calls / non-path tokens are NOT anchors.
        let txt = "Call `handleAuth()` then read `Cargo.toml` and `foo`.";
        assert!(extract_inline_file_anchors(txt).is_empty());
    }

    #[test]
    fn dedup_inline_anchor_against_src_marker_same_path() {
        // Same path via [src:] AND inline backtick → counted once.
        let root = temp_project();
        let txt = "Logic [src: file: src/foo.rs:2] sits in `src/foo.rs:4`.";
        let r = analyze(txt, Some(&root));
        assert_eq!(
            r.verified_count(),
            1,
            "must not double-count: {:?}",
            r.sources
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inline_anchor_with_line_range_auto_verified() {
        let root = temp_project(); // foo.rs = 5 lines
        let r = analyze("Range `src/foo.rs:2-4` is the hot path.", Some(&root));
        assert!(r.verified_count() >= 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        std::fs::remove_dir_all(&root).ok();
    }
}

// ── 6. lint_assertions — FR/EN/ES precision matrix ───────────────────
mod lint_precision_matrix {
    use super::*;

    // ----- SHOULD flag: genuine claim, cue present, no anchor/hedge -----

    #[test]
    fn flags_genuine_en_claims() {
        let claims = [
            "The auth token is stored in the session table for every request.",
            "The endpoint returns a paginated list of widgets for the caller.",
            "The migration is defined for the whole production fleet here.",
        ];
        for c in claims {
            assert_eq!(lint_assertions(c).unsourced_count, 1, "should flag: {c}");
        }
    }

    #[test]
    fn flags_genuine_fr_claims() {
        let claims = [
            "La route principale est gérée par un middleware dédié côté serveur.",
            "Le paramètre de timeout est configuré globalement pour tout le cluster.",
            "La table des sessions se trouve dans le schéma applicatif principal.",
        ];
        for c in claims {
            assert_eq!(lint_assertions(c).unsourced_count, 1, "should flag: {c}");
        }
    }

    #[test]
    fn es_genuine_claims_now_flag_recall_gap_closed() {
        // 0.8.8 — the ES recall gap is CLOSED: native Spanish claim cues
        // ("la función", "se encuentra", "está definido"…) now exist, so a
        // pure-Spanish code claim flags WITHOUT relying on a shared token.
        let pure_es =
            "La función de autenticación se encuentra en el servidor principal del sistema.";
        assert!(
            lint_assertions(pure_es).unsourced_count >= 1,
            "pure Spanish code claim must now flag (gap closed): {:?}",
            lint_assertions(pure_es),
        );
        let es_cve =
            "El sistema es vulnerable: CVE-2025-12345 afecta al parser principal del backend.";
        assert!(
            lint_assertions(es_cve).unsourced_count >= 1,
            "ES CVE claim must flag"
        );
        // Precision kept: an ES OPINION must still NOT flag.
        let es_opinion = "En mi opinión, sería preferible mover esa lógica a un servicio dedicado y reutilizable.";
        assert_eq!(
            lint_assertions(es_opinion).unsourced_count,
            0,
            "ES opinion must not flag: {:?}",
            lint_assertions(es_opinion),
        );
        // And an ES hedge suppresses.
        let es_hedge =
            "Creo que la función de login está definida en el controlador, pero no estoy seguro.";
        assert_eq!(
            lint_assertions(es_hedge).unsourced_count,
            0,
            "ES hedge must suppress"
        );
    }

    // ----- MUST NOT flag: suppressors -----

    #[test]
    fn opinion_frames_suppress_en_and_fr() {
        let opinions = [
            "I recommend the route should be handled by a dedicated middleware.",
            "In my opinion the parameter should be configured at the infra layer.",
            "Je recommande que la table soit gérée par un service dédié.",
            "À mon avis le paramètre devrait être configuré ailleurs.",
        ];
        for o in opinions {
            assert_eq!(
                lint_assertions(o).unsourced_count,
                0,
                "opinion must not flag: {o}"
            );
        }
    }

    #[test]
    fn conditionals_suppress_fr_and_en() {
        let conds = [
            "Si le paramètre est configuré à zéro, la requête ne coupe jamais vraiment.",
            "Quand le cache est géré par le DOM, le comportement change pour tout le monde.",
            "If the parameter is configured to zero, the request never times out at all.",
            "When the route is handled upstream, the gateway returns early for callers.",
        ];
        for c in conds {
            assert_eq!(
                lint_assertions(c).unsourced_count,
                0,
                "conditional must not flag: {c}"
            );
        }
    }

    #[test]
    fn questions_suppress() {
        let qs = [
            "Where exactly is the auth token stored in the session table anyway?",
            "Est-ce que la route est vraiment gérée par le middleware dédié ?",
        ];
        for q in qs {
            assert_eq!(
                lint_assertions(q).unsourced_count,
                0,
                "question must not flag: {q}"
            );
        }
    }

    #[test]
    fn markdown_headings_suppress() {
        let headings = [
            "### The endpoint returns a paginated list of widgets for the caller",
            "## Où la route est définie côté backend pour tous les appels",
            "The endpoint returns a paginated list of widgets for the caller**",
        ];
        for h in headings {
            assert_eq!(
                lint_assertions(h).unsourced_count,
                0,
                "heading must not flag: {h}"
            );
        }
    }

    #[test]
    fn imperative_bullets_suppress() {
        let bullets = [
            "- Configure the endpoint that returns the paginated widget list for callers",
            "* Vérifier que la route est gérée par le middleware dédié partout",
            "Ajouter le paramètre qui est configuré pour tout le cluster ici",
        ];
        for b in bullets {
            assert_eq!(
                lint_assertions(b).unsourced_count,
                0,
                "imperative must not flag: {b}"
            );
        }
    }

    #[test]
    fn hedges_suppress_all_accent_variants() {
        // peut-être / peut être / peut etre must all suppress equally.
        let hedged = [
            "Le cache est peut-être géré localement, ça reste à confirmer côté serveur.",
            "Le cache est peut être géré localement, ça reste à confirmer côté serveur.",
            "Le cache est peut etre géré localement, ça reste à confirmer côté serveur.",
            "I think the endpoint returns a cached value, but I should check first.",
        ];
        for h in hedged {
            assert_eq!(
                lint_assertions(h).unsourced_count,
                0,
                "hedge must not flag: {h}"
            );
        }
    }

    #[test]
    fn anchors_suppress_every_form() {
        let root = temp_project();
        // backtick path, [src:] marker, URL — each suppresses the flag.
        let anchored = [
            "The endpoint returns the widget list in `backend/src/api/widgets.rs`.",
            "The endpoint returns the widget list [src: file: src/foo.rs:1].",
            "The endpoint returns the widget list per https://api.example.com/docs page.",
        ];
        for a in anchored {
            let r = analyze(a, Some(&root));
            assert_eq!(
                r.unsourced_count, 0,
                "anchor must suppress: {a} -> {:?}",
                r.flagged_spans
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn exact_di_sentence_regression() {
        // The canonical DI false-positive: opinion inside a conditional.
        let t = "- La conformité DI n'est pas toujours une amélioration — si le \
                     cycle de vie est géré par le DOM, DI peut être un anti-pattern ici";
        assert_eq!(
            lint_assertions(t).unsourced_count,
            0,
            "{:?}",
            lint_assertions(t)
        );
    }

    #[test]
    fn version_si_boundary_regression() {
        // "si" must NOT match inside "version" and wrongly suppress a real claim.
        let t = "La version 7.3.6 est vulnérable à une faille de désérialisation critique.";
        assert!(
            lint_assertions(t).unsourced_count >= 1,
            "{:?}",
            lint_assertions(t)
        );
    }

    #[test]
    fn conditional_after_cue_still_flags() {
        // The conditional guard only suppresses when the opener sits BEFORE
        // the cue. A claim cue first, with a trailing conditional clause,
        // still flags (it's an assertion with a caveat, not a hypothesis).
        let t = "The auth token is stored in the session table, if you must know the detail.";
        assert_eq!(
            lint_assertions(t).unsourced_count,
            1,
            "{:?}",
            lint_assertions(t)
        );
    }
}

// ── 7. analyze / finalize / report-aggregation matrix ────────────────
mod report_aggregation_matrix {
    use super::*;

    #[test]
    fn mixed_report_counts_all_three_signals() {
        let root = temp_project();
        let txt = "Verified [src: file: src/foo.rs:2]. \
                       Fabricated [src: file: src/ghost.rs:1]. \
                       The worker pool is implemented with great care for the whole fleet.";
        let r = analyze(txt, Some(&root));
        assert_eq!(r.fabricated_count, 1, "sources: {:?}", r.sources);
        assert_eq!(r.verified_count(), 1, "sources: {:?}", r.sources);
        assert_eq!(r.unsourced_count, 1, "spans: {:?}", r.flagged_spans);
        assert!(r.has_signal());
        assert!(!r.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_unchecked_has_signal_option_b() {
        let r = analyze(
            "Docs [src: url: https://x.io] and [src: commit: abc123].",
            None,
        );
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert_eq!(r.verified_count(), 0);
        // Option B: surfaced (neutral "unverifiable"), not silent.
        assert!(r.has_signal());
        assert!(!r.is_empty());
    }

    #[test]
    fn verified_only_is_green_signal() {
        let root = temp_project();
        let r = analyze(
            "Retry [src: file: src/foo.rs:2] and `src/foo.rs:3` again.",
            Some(&root),
        );
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert_eq!(r.verified_count(), 1, "deduped to one: {:?}", r.sources);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn max_sources_verified_cap_is_respected() {
        // Emit far more than MAX_SOURCES_VERIFIED markers; the verified
        // sources vec must never exceed the cap.
        let root = temp_project();
        let mut txt = String::new();
        for _ in 0..(MAX_SOURCES_VERIFIED + 20) {
            txt.push_str("[src: file: src/foo.rs:1] ");
        }
        let r = analyze(&txt, Some(&root));
        assert!(
            r.sources.len() <= MAX_SOURCES_VERIFIED,
            "sources len {} must be capped at {}",
            r.sources.len(),
            MAX_SOURCES_VERIFIED,
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fabricated_and_verified_both_present_in_signal() {
        let root = temp_project();
        let txt = "Good [src: file: src/foo.rs:1]. Bad [src: file: src/nope.rs:1].";
        let r = analyze(txt, Some(&root));
        assert_eq!(r.verified_count(), 1);
        assert_eq!(r.fabricated_count, 1);
        assert!(r.has_signal(), "both signals present");
        std::fs::remove_dir_all(&root).ok();
    }

    // ONE sequential test for the mode-global drift caveat.
    #[serial_test::serial] // mutates the process-global anti-halluc mode
    #[test]
    fn finalize_mode_off_then_warn_sequential() {
        let root = temp_project();
        // off → None even for a perfect citation.
        let mode = ModeOverride::set("off");
        assert!(
            finalize_lint_report(
                "Retry [src: file: src/foo.rs:2].",
                None,
                root.to_str().unwrap(),
                &[],
            )
            .is_none(),
            "off mode must produce no report",
        );
        // warn → green stored report.
        mode.switch("warn");
        let rep = finalize_lint_report(
            "Retry [src: file: src/foo.rs:2].",
            None,
            root.to_str().unwrap(),
            &[],
        )
        .expect("warn mode must store a verified report");
        assert!(rep.verified_count() >= 1, "{:?}", rep.sources);
        // no-signal prose → None even in warn.
        assert!(
            finalize_lint_report("Voilà, terminé pour aujourd'hui.", None, "", &[]).is_none(),
            "no-signal prose must produce no report",
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

// ── per-return-type scenario matrix ───────────────────────────────
//
// QA hardening (0.8.7): for EACH of the six anti-hallu pill states the
// UI can render, 2-3 DETERMINISTIC tests assert the exact counts +
// statuses an `analyze`/`analyze_roots` call produces against a seeded
// temp project. The six states:
//   1. VERIFIED   (green)      — a source mechanically resolved.
//   2. UNSOURCED  (amber)      — a claim cue with no anchor.
//   3. FABRICATED (red)        — a FORMAL [src:] failed verification.
//   4. UNVERIFIED (soft amber) — a NON-resolving INLINE backtick anchor.
//   5. NO SIGNAL  (no pill)    — plain prose, nothing to show.
//   6. UNCHECKED  (no pill)    — only soft/non-file tiers (url/user/…).
// Plus mixed-report scenarios proving the four counts coexist correctly.
//
// These tests are mode-INDEPENDENT (they call `analyze`/`analyze_roots`
// directly, never `finalize_lint_report` except where the gate itself is
// under test, and the one such test sets the mode locally + restores it).
mod scenario_matrix {
    use super::*;

    /// A richer seeded project than `temp_project()`: adds a second source
    /// file so "multiple verified sources" is expressible.
    fn temp_project_multi() -> PathBuf {
        let root = temp_project(); // src/foo.rs = 5 lines
                                   // A 3-line file at src/bar.ts.
        std::fs::write(root.join("src/bar.ts"), "x\ny\nz\n").unwrap();
        root
    }

    // ── 1. VERIFIED (green) ───────────────────────────────────────

    #[test]
    fn verified_formal_marker_resolves_green() {
        let root = temp_project();
        let r = analyze("The retry path is [src: file: src/foo.rs:2].", Some(&root));
        assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verified_inline_backtick_anchor_resolves_green() {
        // Niveau 1.5: a NATURAL backticked `path:line` that resolves is a
        // green verified source (auto-verified inline anchor), no formal
        // [src:] needed.
        let root = temp_project();
        let r = analyze("See `src/foo.rs:3` for details.", Some(&root));
        assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal());
        assert!(
            r.sources[0]
                .detail
                .contains("inline anchor (auto-verified)"),
            "{:?}",
            r.sources[0],
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verified_multiple_distinct_sources_all_green() {
        // Two DISTINCT files (dedup is by bare path, so they don't collapse).
        let root = temp_project_multi();
        let r = analyze(
            "Logic in [src: file: src/foo.rs:1] and helper `src/bar.ts:2`.",
            Some(&root),
        );
        assert_eq!(r.verified_count(), 2, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 2. UNSOURCED (amber) ──────────────────────────────────────

    #[test]
    fn unsourced_english_claim_no_anchor_amber() {
        let root = temp_project();
        // "the function" cue, no anchor, long enough, not a question/heading.
        let r = analyze(
            "The function returns the cached connection pool handle every time.",
            Some(&root),
        );
        assert!(r.unsourced_count >= 1, "{:?}", r.flagged_spans);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.verified_count(), 0);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unsourced_french_claim_no_anchor_amber() {
        let root = temp_project();
        // "se trouve" cue, no anchor.
        let r = analyze(
            "La configuration du cache se trouve directement dans le module principal.",
            Some(&root),
        );
        assert!(r.unsourced_count >= 1, "{:?}", r.flagged_spans);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.verified_count(), 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unsourced_count_matches_number_of_claims() {
        let root = temp_project();
        // Two independent unsourced claims (separate sentences, separate cues).
        let r = analyze(
            "The endpoint is defined in the gateway layer. \
                 La méthode renvoie systématiquement une valeur encodée.",
            Some(&root),
        );
        assert_eq!(r.unsourced_count, 2, "{:?}", r.flagged_spans);
        assert_eq!(r.fabricated_count, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 3. FABRICATED (red) — FORMAL [src:] only ──────────────────

    #[test]
    fn fabricated_not_found_red() {
        let root = temp_project();
        let r = analyze("Defined in [src: file: ghost.rs:1].", Some(&root));
        assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
        assert_eq!(r.sources[0].status, SourceStatus::NotFound);
        assert!(r.sources[0].status.is_fabricated());
        assert_eq!(r.verified_count(), 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fabricated_out_of_bounds_and_outside_project_red() {
        let root = temp_project();
        // OutOfBounds (foo.rs is 5 lines) + OutsideProject (../ escape).
        let r = analyze(
            "Lines [src: file: src/foo.rs:9999] and [src: file: ../../etc/passwd:1].",
            Some(&root),
        );
        assert_eq!(r.fabricated_count, 2, "{:?}", r.sources);
        let statuses: Vec<_> = r.sources.iter().map(|s| s.status).collect();
        assert!(
            statuses.contains(&SourceStatus::OutOfBounds),
            "{statuses:?}"
        );
        assert!(
            statuses.contains(&SourceStatus::OutsideProject),
            "{statuses:?}"
        );
        assert!(r.sources.iter().all(|s| s.status.is_fabricated()));
        assert_eq!(r.verified_count(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fabricated_training_data_is_rejected_red() {
        let root = temp_project();
        let r = analyze(
            "This is well known [src: training-data: GPT prior].",
            Some(&root),
        );
        assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
        assert_eq!(r.sources[0].kind, SourceKind::TrainingData);
        assert_eq!(r.sources[0].status, SourceStatus::Rejected);
        assert!(r.sources[0].status.is_fabricated());
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 4. UNVERIFIED (soft amber, NEW) — INLINE anchor only ──────

    #[test]
    fn unverified_inline_not_found_soft_amber_not_red() {
        let root = temp_project();
        // NON-resolving INLINE backtick anchor → unverified, NOT fabricated.
        let r = analyze("Check `src/ghost.rs:1` for the handler.", Some(&root));
        assert_eq!(r.unverified_count, 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0, "inline must NOT escalate to red");
        assert_eq!(r.verified_count(), 0);
        assert!(r.has_signal());
        // The source IS listed, status Unchecked, detail says "couldn't verify".
        assert_eq!(r.sources.len(), 1);
        assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
        assert!(
            r.sources[0].detail.contains("couldn't verify"),
            "{:?}",
            r.sources[0],
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unverified_inline_out_of_bounds_soft_amber_not_red() {
        let root = temp_project();
        // Existing file, line beyond length, but cited INLINE → soft amber.
        let r = analyze("The fix is at `src/foo.rs:9999` in the loop.", Some(&root));
        assert_eq!(r.unverified_count, 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
        assert!(r.sources[0].detail.contains("couldn't verify"));
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unverified_two_inline_anchors_both_soft() {
        let root = temp_project();
        let r = analyze(
            "See `src/ghost.rs:1` and also `src/missing.ts:2`.",
            Some(&root),
        );
        assert_eq!(r.unverified_count, 2, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.verified_count(), 0);
        assert!(r
            .sources
            .iter()
            .all(|s| s.status == SourceStatus::Unchecked));
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 5. NO SIGNAL (no pill) ────────────────────────────────────

    #[test]
    fn no_signal_plain_prose_is_empty() {
        let root = temp_project();
        let r = analyze(
            "Voilà, c'est terminé pour aujourd'hui. Bonne soirée à tous.",
            Some(&root),
        );
        assert!(!r.has_signal());
        assert!(r.is_empty());
        assert_eq!(r.unsourced_count, 0);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unverified_count, 0);
        assert_eq!(r.verified_count(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_signal_english_smalltalk_is_empty() {
        let root = temp_project();
        let r = analyze(
            "Thanks, that all looks good to me. Have a great weekend!",
            Some(&root),
        );
        assert!(!r.has_signal());
        assert!(r.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[serial_test::serial] // mutates the process-global anti-halluc mode
    #[test]
    fn no_signal_finalize_returns_none() {
        // The has_signal gate in finalize_lint_report drops a no-signal
        // report. Mode-dependent → set + restore locally, kept in ONE fn.
        let root = temp_project();
        let _mode = ModeOverride::set("warn");
        let out = finalize_lint_report(
            "Voilà, c'est terminé pour aujourd'hui. Bonne soirée.",
            None,
            root.to_str().unwrap(),
            &[],
        );
        assert!(out.is_none(), "no-signal prose must finalize to None");
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 6. UNCHECKED / non-vérifiable (no pill, listed for drawer) ─

    #[test]
    fn unchecked_url_and_user_no_signal_but_listed() {
        let root = temp_project();
        let r = analyze(
            "Docs [src: url: https://example.test/x] and [src: user-confirmed 2026-01-01].",
            Some(&root),
        );
        // Neither fabricated nor verified nor unverified → no pill.
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.verified_count(), 0);
        assert_eq!(r.unverified_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert!(
            r.has_signal(),
            "Option B: unchecked-only is surfaced (neutral pill)"
        );
        // The sources ARE listed for drawer transparency.
        assert_eq!(r.sources.len(), 2, "{:?}", r.sources);
        assert!(r
            .sources
            .iter()
            .all(|s| s.status == SourceStatus::Unchecked));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unchecked_inferred_and_commit_no_signal_but_listed() {
        let root = temp_project();
        let r = analyze(
            "Probably [src: inferred: derived from the trait bound] per [src: commit: abc1234].",
            Some(&root),
        );
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.verified_count(), 0);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal()); // Option B: surfaced
        assert_eq!(r.sources.len(), 2, "{:?}", r.sources);
        let kinds: Vec<_> = r.sources.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SourceKind::Inferred), "{kinds:?}");
        assert!(kinds.contains(&SourceKind::Commit), "{kinds:?}");
        assert!(r
            .sources
            .iter()
            .all(|s| s.status == SourceStatus::Unchecked));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unchecked_hypothesis_only_no_signal() {
        let root = temp_project();
        let r = analyze(
            "It may be [src: hypothesis: the cache is invalidated on write].",
            Some(&root),
        );
        assert_eq!(r.sources.len(), 1, "{:?}", r.sources);
        assert_eq!(r.sources[0].kind, SourceKind::Hypothesis);
        assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
        assert!(r.has_signal()); // Option B: surfaced (neutral)
        std::fs::remove_dir_all(&root).ok();
    }

    // ── MIXED reports — all four counts coexist ───────────────────

    #[test]
    fn mixed_verified_unsourced_fabricated_unverified_all_counts() {
        let root = temp_project_multi();
        // - verified : formal [src: file: src/foo.rs:2]   → green
        // - fabricated: formal [src: file: ghost.rs:1]    → red (NotFound)
        // - unverified: inline `src/missing.ts:3`         → soft amber
        // - unsourced : a bare claim sentence with a cue, no anchor → amber
        let text = "\
The pool lives in [src: file: src/foo.rs:2]. \
Handler at [src: file: ghost.rs:1]. \
See `src/missing.ts:3` too.
La méthode renvoie toujours une connexion réutilisée.";
        let r = analyze(text, Some(&root));
        assert_eq!(r.verified_count(), 1, "verified: {:?}", r.sources);
        assert_eq!(r.fabricated_count, 1, "fabricated: {:?}", r.sources);
        assert_eq!(r.unverified_count, 1, "unverified: {:?}", r.sources);
        assert_eq!(r.unsourced_count, 1, "unsourced: {:?}", r.flagged_spans);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mixed_red_dominates_but_green_still_counted() {
        // Priority sanity: a report with BOTH a fabricated and a verified
        // source keeps both counts truthful (the UI decides pill priority;
        // the report stays honest — red present, green present).
        let root = temp_project();
        let text = "Good [src: file: src/foo.rs:1] but bad [src: file: ghost.rs:9].";
        let r = analyze(text, Some(&root));
        assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
        assert_eq!(r.unverified_count, 0);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mixed_unchecked_tier_does_not_inflate_any_count() {
        // A report mixing one VERIFIED file ref with soft/unchecked tiers
        // (url, inferred) → only the file counts as verified; the unchecked
        // tiers are listed but do NOT bump verified/fabricated/unverified.
        let root = temp_project();
        let text = "\
Logic in [src: file: src/foo.rs:1], see [src: url: https://example.test/y], \
probably [src: inferred: from the signature].";
        let r = analyze(text, Some(&root));
        assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unverified_count, 0);
        assert_eq!(r.sources.len(), 3, "all three listed: {:?}", r.sources);
        assert!(r.has_signal(), "the one verified file gives a green signal");
        std::fs::remove_dir_all(&root).ok();
    }
}
