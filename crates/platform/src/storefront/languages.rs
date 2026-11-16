use super::{Storefront, StorefrontError};
use crate::{
    identity::{Identity, Permission, Session},
    locale::Locale,
};

/// Validated public language configuration. The primary language is always active;
/// language codes come from the compiled registry and duplicates are rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Languages {
    primary: Locale,
    active: Vec<Locale>,
}
impl Default for Languages {
    fn default() -> Self {
        Self {
            primary: Locale::default(),
            active: Locale::SUPPORTED
                .iter()
                .filter_map(|(code, _)| Locale::parse(code))
                .collect(),
        }
    }
}
impl Languages {
    pub fn new(primary: &str, active: &[String]) -> Result<Self, StorefrontError> {
        let mut errors = Vec::new();
        let primary = Locale::parse(primary);
        if primary.is_none() || !primary.is_some_and(|p| active.iter().any(|code| code == p.code()))
        {
            errors.push("primary_locale");
        }
        let unique: std::collections::BTreeSet<_> = active.iter().collect();
        if active.is_empty()
            || unique.len() != active.len()
            || active.iter().any(|code| Locale::parse(code).is_none())
        {
            errors.push("active_locales");
        }
        if !errors.is_empty() {
            return Err(StorefrontError::Invalid(errors));
        }
        let primary = primary.ok_or_else(|| StorefrontError::Invalid(vec!["primary_locale"]))?;
        let active = Locale::SUPPORTED
            .iter()
            .filter_map(|(code, _)| {
                if active.iter().any(|selected| selected == code) {
                    Locale::parse(code)
                } else {
                    None
                }
            })
            .collect();
        Ok(Self { primary, active })
    }
    /// Caller holds the store/installation lock and commits this with its owning mutation.
    pub async fn persist(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorefrontError> {
        let codes: Vec<_> = self.active.iter().map(|locale| locale.code()).collect();
        sqlx::query("DELETE FROM storefront_languages WHERE NOT (code=ANY($1))")
            .bind(&codes)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO storefront_languages(code) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING").bind(&codes).execute(&mut **tx).await?;
        sqlx::query("UPDATE storefront_settings SET primary_locale=$1 WHERE id=1")
            .bind(self.primary.code())
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO storefront_language_prefixes(code) SELECT unnest($1::text[]) EXCEPT SELECT $2::text ON CONFLICT DO NOTHING").bind(&codes).bind(self.primary.code()).execute(&mut **tx).await?;
        Ok(())
    }
    pub fn primary(&self) -> Locale {
        self.primary
    }
    pub fn active(&self) -> &[Locale] {
        &self.active
    }
    pub fn contains(&self, locale: Locale) -> bool {
        self.active.contains(&locale)
    }
}
impl Storefront {
    pub async fn save_languages(
        &self,
        session: &Session,
        csrf: &str,
        primary: &str,
        active: &[String],
    ) -> Result<(), StorefrontError> {
        session.check_csrf(csrf)?;
        session.require(Permission::SettingsWrite)?;
        let languages = Languages::new(primary, active)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::SettingsWrite).await?;
        crate::database::commercial_lock::exclusive(&mut tx).await?;
        sqlx::query_scalar::<_, i16>("SELECT id FROM storefront_settings WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        languages.persist(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primary_must_be_active_and_codes_are_unique_and_registered() {
        for (primary, codes) in [
            ("en", vec!["it"]),
            ("unknown", vec!["unknown"]),
            ("it", vec![]),
            ("it", vec!["it", "it"]),
            ("en", vec!["en", "unknown"]),
        ] {
            let codes = codes.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(Languages::new(primary, &codes).is_err());
        }
        let english = Languages::new("en", &["en".into()]).unwrap();
        assert_eq!(english.primary(), Locale::ENGLISH);
        assert_eq!(english.active(), &[Locale::ENGLISH]);
        assert!(!english.contains(Locale::ITALIAN));
        assert_eq!(
            Languages::new("it", &["en".into(), "it".into()]).unwrap(),
            Languages {
                primary: Locale::ITALIAN,
                active: vec![Locale::ITALIAN, Locale::ENGLISH]
            }
        );
    }
}
