//! Adversarial QA for the anti-hallucination niveau-0 prose heuristic
//! ([`kronn::core::anti_halluc::lint_assertions`]), dimension = "lang_claims".
//!
//! We probe PRECISION (opinions / conditionals / questions / headings /
//! imperative-bullets / hedges — incl. the accented "peut être" — MUST NOT
//! flag) AND RECALL (genuine, unsourced, unhedged FR/EN code claims MUST flag,
//! unsourced_count >= 1) across FR / EN / ES.
//!
//! Each test asserts the CORRECT/expected behaviour per the documented
//! semantics. A red test => suspected bug in the heuristic (reported, not
//! weakened to green).

use kronn::core::anti_halluc::{analyze, lint_assertions};

// ── helpers ────────────────────────────────────────────────────────────────

/// A genuine, confident technical claim must trip the heuristic.
fn assert_flags(text: &str, label: &str) {
    let r = lint_assertions(text);
    assert!(
        r.unsourced_count >= 1,
        "[{label}] expected a flag (unsourced_count>=1) but got 0 for: {text:?}\nreport={r:?}"
    );
}

/// An opinion / question / heading / hedge / conditional must stay silent.
fn assert_clean(text: &str, label: &str) {
    let r = lint_assertions(text);
    assert_eq!(
        r.unsourced_count, 0,
        "[{label}] expected NO flag but got {} for: {text:?}\nspans={:?}",
        r.unsourced_count, r.flagged_spans
    );
}

// ── RECALL: genuine code claims that MUST flag ──────────────────────────────

#[test]
fn recall_fr_genuine_code_claim_flags() {
    // "la fonction" + "est définie" cues, no anchor, no hedge, declarative.
    assert_flags(
        "La fonction principale est définie dans le module de démarrage du backend.",
        "recall_fr",
    );
}

#[test]
fn recall_en_genuine_code_claim_flags() {
    // "the endpoint" + "is handled" cues, no anchor.
    assert_flags(
        "The login endpoint is handled by an asynchronous middleware layer.",
        "recall_en",
    );
}

#[test]
fn recall_en_default_value_claim_flags() {
    // "defaults to" cue — a classic unsourced config assertion.
    assert_flags(
        "The retry counter defaults to five attempts before the request aborts.",
        "recall_en_default",
    );
}

#[test]
fn recall_fr_cve_security_claim_flags() {
    // 0.8.7 security/CVE vocabulary patch — "est vulnérable".
    assert_flags(
        "Cette dépendance est vulnérable à une injection dans le parseur XML.",
        "recall_fr_cve",
    );
}

#[test]
fn recall_en_version_claim_flags() {
    // "the latest version is" — explicit version-state framing.
    assert_flags(
        "The latest version is rolled out automatically to every connected client.",
        "recall_en_version",
    );
}

// ── PRECISION: things that MUST NOT flag ────────────────────────────────────

#[test]
fn precision_fr_opinion_recommendation_clean() {
    // "je recommande" opinion frame suppresses even with a claim cue present.
    assert_clean(
        "Je recommande que la fonction soit définie dans un module dédié plus tard.",
        "precision_fr_opinion",
    );
}

#[test]
fn precision_en_opinion_should_be_clean() {
    // "should be" / "we should" normative frame.
    assert_clean(
        "The endpoint should be handled by a dedicated service in my opinion.",
        "precision_en_opinion",
    );
}

#[test]
fn precision_fr_hedge_accented_peut_etre_clean() {
    // Accented "peut être" must fold to the "peut etre" hedge (the DI bug).
    assert_clean(
        "La route est peut être définie ailleurs, je ne suis pas certain du fichier.",
        "precision_fr_peut_etre_accented",
    );
}

#[test]
fn precision_fr_hedge_hyphen_peut_etre_clean() {
    // Hyphenated "peut-être" must fold identically to the accented form.
    assert_clean(
        "La fonction est peut-être définie dans un autre module du projet backend.",
        "precision_fr_peut_etre_hyphen",
    );
}

#[test]
fn precision_fr_conditional_opener_clean() {
    // Claim cue ("est géré") sits AFTER conditional "si" → hypothetical.
    assert_clean(
        "Si le cycle de vie est géré par le DOM alors le composant se nettoie seul.",
        "precision_fr_conditional",
    );
}

#[test]
fn precision_en_question_clean() {
    // Trailing '?' — interrogative, not an assertion, despite a cue.
    assert_clean(
        "Is the login endpoint handled by the new middleware layer here?",
        "precision_en_question",
    );
}

#[test]
fn precision_fr_question_french_typography_clean() {
    // French " ?" spacing — split_sentences must retain the '?' so is_question fires.
    assert_clean(
        "La fonction de connexion est définie dans quel fichier exactement ?",
        "precision_fr_question_spaced",
    );
}

#[test]
fn precision_en_imperative_bullet_clean() {
    // Imperative-led bullet — a proposal/TODO, not a claim.
    assert_clean(
        "- Configure the retry option so the function returns a typed error here.",
        "precision_en_imperative_bullet",
    );
}

#[test]
fn precision_fr_heading_clean() {
    // Markdown heading — a title, not an assertion.
    assert_clean(
        "## La fonction principale est définie ici dans le module de démarrage",
        "precision_fr_heading",
    );
}

#[test]
fn precision_anchored_backtick_path_clean() {
    // A backticked path:line anchor self-sources the claim → not flagged.
    assert_clean(
        "The login endpoint is handled inside `backend/src/lib.rs:42` precisely.",
        "precision_anchored_path",
    );
}

// ── ES: documented sparse-recall behaviour ──────────────────────────────────

#[test]
fn es_genuine_claim_flags_recall_gap_closed() {
    // 0.8.8 — the ES recall gap is CLOSED. Native Spanish claim cues
    // ("se encuentra", "la función", "está definido"…) were added, so a
    // purely-Spanish code claim now flags WITHOUT a shared FR/EN token.
    let es_only = "La función principal se encuentra en el módulo de arranque del servidor.";
    let r = lint_assertions(es_only);
    assert!(
        r.unsourced_count >= 1,
        "Spanish-only code claim must now flag (gap closed): {r:?}"
    );
}

#[test]
fn es_opinion_clean_no_false_positive() {
    // Even though ES recall is weak, an ES opinion must never false-positive.
    assert_clean(
        "En mi opinión la función debería estar definida en otro módulo distinto.",
        "es_opinion",
    );
}

// ── ROBUSTNESS / INVARIANTS ─────────────────────────────────────────────────

#[test]
fn no_panic_on_adversarial_inputs() {
    // Must never panic regardless of bytes thrown at it.
    let inputs = [
        "",
        "   ",
        "?",
        "###",
        "`",
        "[src:",
        "[src: file: ",
        "[[[src:::]]]",
        "La fonction est définie 🚀🔥 dans le module émoji accentué ààà.",
        "日本語のテキスト the function is defined ここに here precisely now.",
        "\u{202E}RTL override the endpoint is handled here in the layer now.",
        "peut\u{2019}etre la route est definie ailleurs dans le projet backend.",
    ];
    for s in inputs {
        let _ = lint_assertions(s); // no panic
        let _ = analyze(s, None); // no panic, project-less
    }
    // A genuinely huge input must also not panic / blow up.
    let huge = "The function is defined here without any anchor at all. ".repeat(5000);
    let _ = lint_assertions(&huge);
}

#[test]
fn determinism_same_text_same_report() {
    let text =
        "The endpoint is handled by middleware. La fonction est définie dans le backend ici.";
    let a = lint_assertions(text);
    let b = lint_assertions(text);
    assert_eq!(a, b, "lint_assertions must be deterministic");
    // analyze (project-less) must be deterministic too.
    let c = analyze(text, None);
    let d = analyze(text, None);
    assert_eq!(c, d, "analyze must be deterministic");
}

#[test]
fn empty_and_whitespace_are_silent() {
    for s in ["", "   ", "\n\t\n"] {
        let r = lint_assertions(s);
        assert_eq!(r.unsourced_count, 0, "empty-ish must be silent: {s:?}");
        assert!(r.flagged_spans.is_empty());
        assert!(!r.has_signal(), "empty-ish must carry no signal: {s:?}");
    }
}
