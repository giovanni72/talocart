//! Themes shipped in this binary. Templates/assets are composed explicitly by web.
use thiserror::Error;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Default,
    Essential,
}
pub const COMPILED: &[Theme] = &[Theme::Default, Theme::Essential];
#[derive(Debug, Error, PartialEq, Eq)]
#[error("Configured theme is not included in this build")]
pub struct UnavailableTheme;
impl Theme {
    pub fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Essential => "essential",
        }
    }
    pub fn parse(id: &str) -> Result<Self, UnavailableTheme> {
        COMPILED
            .iter()
            .copied()
            .find(|theme| theme.id() == id)
            .ok_or(UnavailableTheme)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_compiled_identifiers_are_accepted_without_aliases_or_fallbacks() {
        assert_eq!(Theme::parse("default"), Ok(Theme::Default));
        assert_eq!(Theme::parse("essential"), Ok(Theme::Essential));
        for id in [
            "",
            "Default",
            " essential",
            "unknown",
            "../../templates",
            "<script>",
        ] {
            assert_eq!(Theme::parse(id), Err(UnavailableTheme));
        }
    }
}
