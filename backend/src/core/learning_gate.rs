//! Pure pieces of the continual-learning validation pipeline (spec §6).
//! HTTP-free → unit-testable. The API handler (`api/learnings.rs`) orchestrates
//! these + DB calls. Hard rejections (empty evidence, secret, fabricated
//! evidence, negative-learning threshold) live in the handler; this module
//! provides the verdicts/heuristics those decisions read.

use crate::core::anti_halluc::{verify_source_marker_roots, SourceStatus};
use crate::models::learnings::Evidence;
use serde::{Deserialize, Serialize};
use std::path::Path;
use ts_rs::TS;

/// Gate-1 verdict for one evidence row (rendered per-row in the validation modal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EvidenceCheck {
    pub reference: String,
    pub status: String, // SourceStatus debug name, or "Unchecked" for non-file kinds
    pub fabricated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceReport {
    pub checks: Vec<EvidenceCheck>,
    pub any_fabricated: bool,
    pub verified_count: usize,
}

/// Gate-1 (existence) over a learning's evidence — spec §6 step 2. File / code
/// refs are mechanically verified against the project roots; url / user / cmd /
/// disc are `Unchecked` (can't be resolved here → NOT fabricated). A red
/// (`fabricated`) evidence makes the candidate rejectable by the caller.
pub fn verify_evidence(evidence: &[Evidence], roots: &[&Path]) -> EvidenceReport {
    let mut checks = Vec::new();
    let mut any_fabricated = false;
    let mut verified_count = 0;
    for e in evidence {
        match e.kind.to_ascii_lowercase().as_str() {
            "file" | "code" | "code-comment" => {
                let c = verify_source_marker_roots(&e.reference, roots);
                let fab = c.status.is_fabricated();
                any_fabricated |= fab;
                if c.status == SourceStatus::Verified {
                    verified_count += 1;
                }
                checks.push(EvidenceCheck {
                    reference: e.reference.clone(),
                    status: format!("{:?}", c.status),
                    fabricated: fab,
                });
            }
            _ => checks.push(EvidenceCheck {
                reference: e.reference.clone(),
                status: "Unchecked".into(),
                fabricated: false,
            }),
        }
    }
    EvidenceReport {
        checks,
        any_fabricated,
        verified_count,
    }
}

/// Safeguard #2 — anti-generalization. An absolute quantifier
/// (always/never/everywhere/toujours/jamais/partout) with NO scoping qualifier
/// reads as over-broad → the handler warns and asks to reformulate. Word-boundary
/// padded so "jamais" doesn't fire inside another token.
pub fn is_overgeneralized(claim: &str) -> bool {
    let padded = format!(" {} ", claim.to_ascii_lowercase());
    const ABSOLUTES: &[&str] = &[
        " toujours ",
        " jamais ",
        " partout ",
        " always ",
        " never ",
        " everywhere ",
    ];
    const SCOPES: &[&str] = &[
        "dans ", "pour ", "sur ", " in ", " for ", " when ", "quand ", "lorsqu", "module",
        "fichier", "projet",
    ];
    ABSOLUTES.iter().any(|a| padded.contains(a)) && !SCOPES.iter().any(|s| padded.contains(s))
}

/// Safeguard #5 — confidence haircut. LLM self-scored confidence is optimistic;
/// apply a fixed 0.85 multiplier before storing.
pub fn haircut(confidence: Option<f32>) -> Option<f32> {
    confidence.map(|c| (c * 0.85).clamp(0.0, 1.0))
}

/// True if `s` carries a REAL `YYYY-MM-DD` calendar date (the `user:<date>`
/// convention). Scans for a 10-char `dddd-dd-dd` window then validates it with
/// `chrono` — so `2099-99-99` / `0000-13-45` are rejected, not just shape-matched.
pub fn is_dated_user_ref(s: &str) -> bool {
    let b = s.as_bytes();
    let d = |c: u8| c.is_ascii_digit();
    (0..b.len().saturating_sub(9)).any(|i| {
        let w = &b[i..i + 10];
        d(w[0])
            && d(w[1])
            && d(w[2])
            && d(w[3])
            && w[4] == b'-'
            && d(w[5])
            && d(w[6])
            && w[7] == b'-'
            && d(w[8])
            && d(w[9])
            && chrono::NaiveDate::parse_from_str(std::str::from_utf8(w).unwrap_or(""), "%Y-%m-%d")
                .is_ok()
    })
}

/// §5 binding — a `preference` (routes to User scope, high blast-radius) must
/// carry at least one dated `user` evidence (`[src: user:YYYY-MM-DD]`). The
/// human gate alone isn't enough for the user/global scope per spec invariant #5.
pub fn preference_has_dated_user_evidence(evidence: &[Evidence]) -> bool {
    evidence
        .iter()
        .any(|e| e.kind.eq_ignore_ascii_case("user") && is_dated_user_ref(&e.reference))
}

/// Stable hash key of a claim for negative-learning dedup (safeguard #6a). Keys
/// on kind + scope + normalized claim so the SAME claim re-proposed maps to the
/// same rejection counter.
pub fn claim_hash(kind: &str, scope: Option<&str>, claim: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    kind.hash(&mut h);
    scope.unwrap_or("").hash(&mut h);
    claim
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::learnings::Evidence;
    use std::path::PathBuf;

    fn temp_project() -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("kronn_lgate_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
        d
    }

    fn ev(kind: &str, reference: &str) -> Evidence {
        Evidence {
            kind: kind.into(),
            reference: reference.into(),
            quote: None,
        }
    }

    #[test]
    fn verify_evidence_flags_fabricated_file() {
        let root = temp_project();
        let report = verify_evidence(
            &[ev("file", "src/foo.rs:2"), ev("file", "src/ghost.rs:9")],
            &[root.as_path()],
        );
        assert_eq!(
            report.verified_count, 1,
            "real file verifies: {:?}",
            report.checks
        );
        assert!(report.any_fabricated, "missing file is fabricated");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn url_and_user_evidence_are_unchecked_not_fabricated() {
        let report = verify_evidence(
            &[ev("url", "https://x.test"), ev("user", "user:2026-05-31")],
            &[],
        );
        assert!(!report.any_fabricated);
        assert_eq!(report.verified_count, 0);
        assert!(report.checks.iter().all(|c| c.status == "Unchecked"));
    }

    #[test]
    fn overgeneralization_detected_without_scope() {
        assert!(is_overgeneralized("Toujours utiliser pnpm"));
        assert!(is_overgeneralized("never use var"));
        // scoped → fine
        assert!(!is_overgeneralized(
            "Dans ce projet, toujours utiliser pnpm"
        ));
        assert!(!is_overgeneralized("Use tabs for indentation")); // no absolute
    }

    #[test]
    fn haircut_applies_and_clamps() {
        assert_eq!(haircut(Some(1.0)), Some(0.85));
        assert_eq!(haircut(Some(0.0)), Some(0.0));
        assert_eq!(haircut(None), None);
    }

    #[test]
    fn dated_user_ref_detection() {
        assert!(is_dated_user_ref("user:2026-06-01"));
        assert!(is_dated_user_ref("disc-42 confirmed 2026-01-15"));
        assert!(!is_dated_user_ref("user:no-date-here"));
        assert!(!is_dated_user_ref("2026-6-1")); // not zero-padded → not 10-char window
                                                 // shape-valid but NOT a real calendar date → rejected (chrono check)
        assert!(!is_dated_user_ref("user:2099-99-99"));
        assert!(!is_dated_user_ref("user:0000-13-45"));
    }

    #[test]
    fn preference_requires_dated_user_evidence() {
        let dated = vec![ev("user", "user:2026-06-01")];
        let undated = vec![ev("user", "user")];
        let file_only = vec![ev("file", "src/foo.rs:1")];
        assert!(preference_has_dated_user_evidence(&dated));
        assert!(!preference_has_dated_user_evidence(&undated));
        assert!(!preference_has_dated_user_evidence(&file_only));
    }

    #[test]
    fn claim_hash_is_stable_and_normalizing() {
        let a = claim_hash("fact", None, "Uses  pnpm strict");
        let b = claim_hash("fact", None, "uses pnpm strict");
        assert_eq!(a, b, "case/whitespace-insensitive");
        let c = claim_hash("preference", None, "uses pnpm strict");
        assert_ne!(a, c, "kind is part of the key");
    }
}
