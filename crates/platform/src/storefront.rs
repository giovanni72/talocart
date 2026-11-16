//! Persistent presentation settings; permissions stay with the identity owner.
use crate::content::{Draft, Translation};
use crate::identity::{Identity, IdentityError, Permission, Session};
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use thiserror::Error;
mod content;
mod languages;
pub use languages::Languages;
#[derive(Clone, Debug, Deserialize, FromRow)]
pub struct Settings {
    #[serde(default)]
    pub maintenance_enabled: bool,
    #[serde(default)]
    pub indexing_enabled: bool,
    #[serde(default = "default_theme")]
    pub theme_id: String,
    pub palette: String,
    pub typography: String,
    pub density: String,
    #[serde(default)]
    pub show_hero: bool,
    #[serde(default)]
    pub show_featured: bool,
    pub heading: String,
    pub introduction: String,
    pub seo_title: String,
    pub seo_description: String,
    pub public_origin: String,
    #[serde(default = "default_primary")]
    pub primary_locale: String,
    #[serde(default = "default_active")]
    pub active_locales: Vec<String>,
    #[serde(default)]
    pub published_prefixes: Vec<String>,
}
fn default_theme() -> String {
    crate::themes::Theme::Default.id().into()
}
fn default_primary() -> String {
    crate::locale::Locale::DEFAULT_CODE.into()
}
fn default_active() -> Vec<String> {
    crate::locale::Locale::SUPPORTED
        .iter()
        .map(|(code, _)| (*code).into())
        .collect()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            maintenance_enabled: false,
            indexing_enabled: false,
            theme_id: default_theme(),
            palette: "ocean".into(),
            typography: "modern".into(),
            density: "compact".into(),
            show_hero: true,
            show_featured: true,
            heading: "Le cose fatte bene.".into(),
            introduction: "Scopri i prodotti del nostro negozio.".into(),
            seo_title: String::new(),
            seo_description: String::new(),
            public_origin: String::new(),
            primary_locale: default_primary(),
            active_locales: default_active(),
            published_prefixes: Vec::new(),
        }
    }
}
impl Settings {
    pub fn languages(&self) -> Result<Languages, StorefrontError> {
        Languages::new(&self.primary_locale, &self.active_locales)
    }
    pub fn title(&self, fallback: &str) -> String {
        if self.seo_title.is_empty() {
            fallback.into()
        } else {
            self.seo_title.clone()
        }
    }
    pub fn description(&self) -> String {
        if self.seo_description.is_empty() {
            self.introduction.clone()
        } else {
            self.seo_description.clone()
        }
    }
    /// Only a validated configured origin or a trusted bind-derived fallback is used.
    pub fn canonical(&self, fallback: &str, path: &str) -> String {
        format!(
            "{}{}",
            if self.public_origin.is_empty() {
                fallback
            } else {
                &self.public_origin
            },
            path
        )
    }
}
#[derive(Debug, Error)]
pub enum StorefrontError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error(transparent)]
    Content(#[from] crate::content::ContentError),
    #[error("Controlla i campi evidenziati")]
    Invalid(Vec<&'static str>),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for StorefrontError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(e, module_path!()).into())
    }
}
pub fn text_valid(text: &str, maximum: usize) -> bool {
    text.chars().count() <= maximum && !text.chars().any(char::is_control)
}
fn origin(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    if !text_valid(value, 255)
        || value.chars().any(char::is_whitespace)
        || value.contains('@')
        || value.contains('\\')
    {
        return None;
    }
    let u = url::Url::parse(value).ok()?;
    let loopback = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !u.has_host()
        || !(u.scheme() == "https" || (u.scheme() == "http" && loopback))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return None;
    }
    Some(u.origin().ascii_serialization())
}
fn validate(mut s: Settings) -> Result<Settings, StorefrontError> {
    let mut errors = Vec::new();
    if crate::themes::Theme::parse(&s.theme_id).is_err() {
        errors.push("theme_id");
    }
    if !matches!(s.palette.as_str(), "forest" | "ocean" | "clay") {
        errors.push("palette");
    }
    if !matches!(s.typography.as_str(), "editorial" | "modern") {
        errors.push("typography");
    }
    if !matches!(s.density.as_str(), "comfortable" | "compact") {
        errors.push("density");
    }
    for (key, text, max, required) in [
        ("heading", &mut s.heading, 120, true),
        ("introduction", &mut s.introduction, 500, false),
        ("seo_title", &mut s.seo_title, 120, false),
        ("seo_description", &mut s.seo_description, 320, false),
    ] {
        *text = text.trim().to_owned();
        if !text_valid(text, max) || (required && text.is_empty()) {
            errors.push(key);
        }
    }
    match origin(s.public_origin.trim()) {
        Some(o) => s.public_origin = o,
        None => errors.push("public_origin"),
    }
    if errors.is_empty() {
        Ok(s)
    } else {
        Err(StorefrontError::Invalid(errors))
    }
}
#[derive(Clone)]
pub struct Storefront {
    pool: PgPool,
}
impl Storefront {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    /// Read on every public request: closures propagate without a restart or stale cache.
    pub async fn maintenance_enabled(&self) -> Result<bool, IdentityError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT maintenance_enabled FROM storefront_settings WHERE id=1",
        )
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false))
    }
    pub async fn set_maintenance(
        &self,
        session: &Session,
        csrf: &str,
        enabled: bool,
    ) -> Result<(), IdentityError> {
        let mut tx = self.pool.begin().await?;
        Self::set_maintenance_in(&mut tx, session, csrf, enabled).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn set_maintenance_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        csrf: &str,
        enabled: bool,
    ) -> Result<(), IdentityError> {
        Identity::authorize_transaction(tx, session, csrf, Permission::SettingsWrite).await?;
        sqlx::query("UPDATE storefront_settings SET maintenance_enabled=$1 WHERE id=1")
            .bind(enabled)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn settings(&self) -> Result<Settings, IdentityError> {
        Self::read_settings(&self.pool).await
    }
    async fn read_settings<'e, E>(executor: E) -> Result<Settings, IdentityError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let settings: Settings = sqlx::query_as("SELECT s.*, ARRAY(SELECT code FROM storefront_languages ORDER BY code) AS active_locales, ARRAY(SELECT code FROM storefront_language_prefixes ORDER BY code) AS published_prefixes FROM storefront_settings s WHERE id=1")
            .fetch_one(executor).await?;
        crate::themes::Theme::parse(&settings.theme_id).map_err(|_| IdentityError::Unavailable)?;
        Ok(settings)
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn settings_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Settings, IdentityError> {
        Self::read_settings(&mut **tx).await
    }
    /// Save presentation fields only; language policy has its own authorized transaction.
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        settings: Settings,
    ) -> Result<(), StorefrontError> {
        self.save_with_content(session, csrf, settings, None).await
    }
    pub async fn save_with_content(
        &self,
        session: &Session,
        csrf: &str,
        settings: Settings,
        content: Option<&Draft>,
    ) -> Result<(), StorefrontError> {
        let mut tx = self.pool.begin().await?;
        self.save_in_inner(&mut tx, session, csrf, settings, content, false)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Saves presentation and translations atomically with other configuration.
    pub async fn save_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        csrf: &str,
        settings: Settings,
        content: Option<&Draft>,
    ) -> Result<(), StorefrontError> {
        self.save_in_inner(tx, session, csrf, settings, content, true)
            .await
    }
    async fn save_in_inner(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        csrf: &str,
        settings: Settings,
        content: Option<&Draft>,
        update_theme: bool,
    ) -> Result<(), StorefrontError> {
        session.check_csrf(csrf)?;
        session.require(Permission::SettingsWrite)?;
        let s = validate(settings)?;
        Identity::authorize_transaction(tx, session, csrf, Permission::SettingsWrite).await?;
        let definition = Self::CONTENT;
        let translations = definition
            .prepare(
                tx,
                content,
                1,
                Translation {
                    name: s.heading.clone(),
                    description: s.introduction.clone(),
                    seo_title: s.seo_title.clone(),
                    seo_description: s.seo_description.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        sqlx::query("UPDATE storefront_settings SET palette=$1,typography=$2,density=$3,show_hero=$4,show_featured=$5,heading=$6,introduction=$7,seo_title=$8,seo_description=$9,public_origin=$10,theme_id=CASE WHEN $12 THEN $11 ELSE theme_id END,indexing_enabled=$13 WHERE id=1")
        .bind(s.palette).bind(s.typography).bind(s.density).bind(s.show_hero).bind(s.show_featured).bind(s.heading).bind(s.introduction).bind(s.seo_title).bind(s.seo_description).bind(s.public_origin).bind(s.theme_id).bind(update_theme).bind(s.indexing_enabled).execute(&mut **tx).await?;
        definition.persist(tx, 1, &translations).await?;
        Ok(())
    }
}
impl From<IdentityError> for StorefrontError {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_theme_is_ocean_modern_and_compact() {
        let settings = Settings::default();
        assert!(!settings.indexing_enabled);
        assert_eq!(
            (
                &*settings.palette,
                &*settings.typography,
                &*settings.density
            ),
            ("ocean", "modern", "compact")
        );
        assert!(validate(settings).is_ok());
    }
    #[test]
    fn unicode_limits_normalization_and_metadata_fallbacks() {
        let settings = Settings {
            heading: format!("  {}  ", "è".repeat(120)),
            introduction: "è".repeat(500),
            seo_title: "è".repeat(120),
            seo_description: "è".repeat(320),
            ..Settings::default()
        };
        let validated = validate(settings.clone()).unwrap();
        assert_eq!(validated.heading, "è".repeat(120));
        assert_eq!(validated.title("Fallback"), settings.seo_title);
        assert_eq!(validated.description(), settings.seo_description);
        let invalid = Settings {
            heading: "è".repeat(121),
            introduction: "è".repeat(501),
            seo_title: "è".repeat(121),
            seo_description: "è".repeat(321),
            ..Settings::default()
        };
        assert!(
            matches!(validate(invalid), Err(StorefrontError::Invalid(fields)) if fields == ["heading", "introduction", "seo_title", "seo_description"])
        );
        let empty = validate(Settings {
            seo_title: "  ".into(),
            seo_description: "  ".into(),
            ..Settings::default()
        })
        .unwrap();
        assert_eq!(empty.title("Nome negozio"), "Nome negozio");
        assert_eq!(empty.description(), empty.introduction);
    }
    #[test]
    fn origins_reject_untrusted_or_non_root_urls() {
        for v in [
            "https://user:secret@example.test",
            "//example.test",
            "http://example.test",
            "https://example.test/shop",
            "https://example.test/?token=x",
            "https://example.test/#fragment",
            "https://example.test/\\evil",
            "https://example.test\n",
            "ftp://example.test",
            "https://@example.test",
        ] {
            assert!(origin(v).is_none(), "{v}");
        }
        for v in [
            "",
            "https://shop.example.test",
            "http://127.0.0.1:3102",
            "http://[::1]:3000",
        ] {
            assert!(origin(v).is_some(), "{v}");
        }
        assert_eq!(
            origin("https://EXAMPLE.test:443/"),
            Some("https://example.test".into())
        );
    }
    #[test]
    fn settings_are_allowlisted_and_text_is_bounded() {
        assert!(validate(Settings::default()).is_ok());
        for (field, value) in [
            ("palette", "#000"),
            ("typography", "remote-font"),
            ("density", "huge"),
            ("heading", ""),
            ("seo_title", "bad\ntext"),
        ] {
            let mut s = Settings::default();
            match field {
                "palette" => s.palette = value.into(),
                "typography" => s.typography = value.into(),
                "density" => s.density = value.into(),
                "heading" => s.heading = value.into(),
                _ => s.seo_title = value.into(),
            }
            assert!(matches!(validate(s), Err(StorefrontError::Invalid(e)) if e.contains(&field)));
        }
        assert!(!text_valid(&"x".repeat(321), 320));
        assert_eq!(
            Settings::default().canonical("http://127.0.0.1:3000", "/catalog"),
            "http://127.0.0.1:3000/catalog"
        );
    }
}
