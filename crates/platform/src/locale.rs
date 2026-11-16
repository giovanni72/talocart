//! Available languages discovered from complete translation catalogue pairs at build time.
//! `talocart migrate` synchronizes their database registry; serving remains read-only.
mod registry {
    include!(concat!(env!("OUT_DIR"), "/locales.rs"));
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locale(&'static str);
impl Locale {
    pub const ITALIAN: Self = Self("it");
    pub const ENGLISH: Self = Self("en");
    pub const SUPPORTED: &'static [(&'static str, &'static str)] = registry::SUPPORTED;
    pub const DEFAULT_CODE: &'static str = registry::DEFAULT_CODE;
    pub fn parse(value: &str) -> Option<Self> {
        Self::SUPPORTED
            .iter()
            .find(|(code, _)| *code == value)
            .map(|(code, _)| Self(code))
    }
    pub fn code(self) -> &'static str {
        self.0
    }
}

impl Default for Locale {
    fn default() -> Self {
        Self(Self::DEFAULT_CODE)
    }
}
