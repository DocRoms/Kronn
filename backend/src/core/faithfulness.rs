//! Gate-2 — faithfulness checker (`claim ⊨ evidence`). Spec §2, posture B:
//! the verdict is INFORMATIVE (surfaced to the human in the validation modal),
//! NEVER an auto-block.
//!
//! Backend is config-selected (`faithfulness_backend = off | nli | llm`).
//! **Default = Off**, validated by the proto (`docs/research/nli-proto-findings.md`):
//! local NLI under-recognizes the loose descriptive claims agents write (would
//! false-flag ~85% of legitimate claims) → not safe to be active untuned. When
//! enabled, **LLM-judge is the quality backend**; local NLI is a later, distilled,
//! opt-in cheap tail-signal. The selector exists now so turning it on is a config
//! flip + one impl, with zero churn at the call site (the pipeline §6 step 4).

use crate::models::learnings::Faithfulness;

#[derive(Debug, Clone, PartialEq)]
pub struct FaithfulnessVerdict {
    pub verdict: Faithfulness,
    pub score: f32,
    pub checker: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaithfulnessBackend {
    Off,
    Nli,
    Llm,
}

impl FaithfulnessBackend {
    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "nli" => Self::Nli,
            "llm" => Self::Llm,
            _ => Self::Off,
        }
    }
}

/// Score `claim ⊨ evidence_quote`. Returns `None` when the backend is `Off`
/// (the pipeline then stores `faithfulness = NULL` and the modal shows no Gate-2
/// chip). `Nli`/`Llm` are deferred — wired as `None` so the call site is stable
/// (0.9.0 ships with the gate off; the LLM-judge impl lands in PR4a-bis).
pub fn check(
    backend: FaithfulnessBackend,
    _claim: &str,
    _evidence_quote: &str,
) -> Option<FaithfulnessVerdict> {
    match backend {
        FaithfulnessBackend::Off => None,
        FaithfulnessBackend::Nli | FaithfulnessBackend::Llm => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_parse_defaults_to_off() {
        assert_eq!(FaithfulnessBackend::from_str_lenient("llm"), FaithfulnessBackend::Llm);
        assert_eq!(FaithfulnessBackend::from_str_lenient("NLI"), FaithfulnessBackend::Nli);
        assert_eq!(FaithfulnessBackend::from_str_lenient(""), FaithfulnessBackend::Off);
        assert_eq!(FaithfulnessBackend::from_str_lenient("garbage"), FaithfulnessBackend::Off);
    }

    #[test]
    fn off_backend_yields_no_verdict() {
        assert!(check(FaithfulnessBackend::Off, "x", "y").is_none());
    }
}
