//! The language the orchestrator speaks in a room (KT-1316): the discussion's
//! own `language`, French and English being the two it has text for. Anything
//! else — a language with no translation — reads English rather than guessing.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoomLang {
    #[default]
    En,
    Fr,
}

impl RoomLang {
    /// `fr`, `FR`, `fr-CA`… read French; every other code reads English.
    pub fn from_code(code: &str) -> Self {
        let primary = code
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if primary == "fr" {
            Self::Fr
        } else {
            Self::En
        }
    }

    /// The text for this language: `fr` or `en`.
    pub fn pick<'a>(self, fr: &'a str, en: &'a str) -> &'a str {
        match self {
            Self::Fr => fr,
            Self::En => en,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_codes_read_french_and_everything_else_reads_english() {
        for code in ["fr", "FR", " fr ", "fr-CA", "fr_BE"] {
            assert_eq!(RoomLang::from_code(code), RoomLang::Fr, "{code}");
        }
        for code in ["en", "en-US", "es", "zh", "", "frisian"] {
            assert_eq!(RoomLang::from_code(code), RoomLang::En, "{code}");
        }
    }

    #[test]
    fn pick_returns_the_text_of_the_language() {
        assert_eq!(RoomLang::Fr.pick("bonjour", "hello"), "bonjour");
        assert_eq!(RoomLang::En.pick("bonjour", "hello"), "hello");
    }
}
