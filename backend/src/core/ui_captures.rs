//! KT-1682 — what a worker brief and a review hand-back say about the
//! before / after captures a UI-changing PR carries. The rule itself lives in
//! `docs/conventions/pull-requests.md`; these are the two places an agent is
//! told it, in the language of the room.

use crate::core::room_lang::RoomLang;

/// The brief section: a worker that changes what renders under `frontend/`
/// takes the captures and hands them over, or says why it could not. A lane
/// cannot push, so the principal publishes them.
pub fn brief_section(lang: RoomLang) -> &'static str {
    lang.pick(
        "## Captures d'UI\n\
         Si ton changement modifie ce qui s'affiche sous `frontend/` (composants, CSS, textes \
         visibles à l'écran), prends des captures avant (branche cible) / après (ton HEAD) de \
         chaque écran modifié, en clair et en sombre, comme le décrit \
         `docs/conventions/pull-requests.md`, et nomme-les (écran, thème, chemin) dans le \
         `summary` de ta livraison pour que le principal les publie sur la PR. Si tu ne peux pas \
         capturer un écran, dis lequel et pourquoi dans `limitations`. Aucun changement d'UI : \
         rien à capturer.\n\n",
        "## UI captures\n\
         If your change alters what renders under `frontend/` (components, CSS, strings shown \
         on screen), take before (target branch) / after (your HEAD) captures of each changed \
         screen, light and dark, as `docs/conventions/pull-requests.md` describes, and name them \
         (screen, theme, path) in your delivery `summary` so the principal publishes them on the \
         PR. If you cannot capture a screen, say which and why in `limitations`. No UI change: \
         nothing to capture.\n\n",
    )
}

/// The tail of a `request_changes` hand-back: a fix that changes an already
/// captured screen retakes its captures from the new head.
pub fn retake_hint(lang: RoomLang) -> &'static str {
    lang.pick(
        "\n\n## Captures d'UI\n\
         Si un correctif modifie un écran de `frontend/` déjà capturé, reprends les captures \
         concernées depuis le nouveau HEAD et nomme-les dans le `summary` \
         (`docs/conventions/pull-requests.md`).",
        "\n\n## UI captures\n\
         If a fix changes an already captured `frontend/` screen, retake the affected captures \
         from the new HEAD and name them in the `summary` (`docs/conventions/pull-requests.md`).",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_texts_name_the_convention_in_each_language() {
        for lang in [RoomLang::Fr, RoomLang::En] {
            for text in [brief_section(lang), retake_hint(lang)] {
                assert!(text.contains("docs/conventions/pull-requests.md"), "{text}");
                assert!(text.contains("frontend/"), "{text}");
                assert!(text.contains("summary"), "{text}");
            }
            assert!(brief_section(lang).contains("limitations"));
        }
        assert!(brief_section(RoomLang::En).starts_with("## UI captures\n"));
        assert!(brief_section(RoomLang::Fr).starts_with("## Captures d'UI\n"));
    }
}
