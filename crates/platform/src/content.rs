//! Shared translation form contract. Each owner supplies its static storage definition.
use crate::{
    identity::{Identity, IdentityError, Permission, Session},
    locale::Locale,
};
use sqlx::{FromRow, PgPool};
use thiserror::Error;

#[derive(Clone, Debug, Default, FromRow, PartialEq, Eq)]
pub struct Translation {
    pub locale: String,
    pub name: String,
    pub description: String,
    pub seo_title: String,
    pub seo_description: String,
}
/// Complete multilingual form, checked against the primary language under the write lock.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub primary: String,
    pub rows: Vec<Translation>,
    /// Active-language snapshot rendered by the browser; internal writes may omit it.
    pub active: Option<Vec<String>>,
    /// None means complete rows. Browser submissions list fields actually present.
    pub submitted: Option<std::collections::BTreeSet<(String, &'static str)>>,
}
impl Draft {
    pub fn apply(&self, stored: &mut Vec<Translation>) {
        for patch in &self.rows {
            let index = match stored.iter().position(|r| r.locale == patch.locale) {
                Some(index) => index,
                None => {
                    stored.push(Translation {
                        locale: patch.locale.clone(),
                        ..Translation::default()
                    });
                    stored.len() - 1
                }
            };
            let row = &mut stored[index];
            for (field, value, target) in [
                ("name", &patch.name, &mut row.name),
                ("description", &patch.description, &mut row.description),
                ("seo_title", &patch.seo_title, &mut row.seo_title),
                (
                    "seo_description",
                    &patch.seo_description,
                    &mut row.seo_description,
                ),
            ] {
                if self.submitted.as_ref().is_none_or(|fields| {
                    fields
                        .iter()
                        .any(|(locale, key)| locale == &patch.locale && *key == field)
                }) {
                    target.clone_from(value);
                }
            }
        }
    }
}
#[derive(FromRow)]
pub struct StoredTranslation {
    pub entity_id: i64,
    #[sqlx(flatten)]
    pub translation: Translation,
}
#[derive(Clone, Copy)]
pub struct Definition {
    pub table: &'static str,
    pub name_column: &'static str,
    pub description_column: Option<&'static str>,
    pub name_limit: usize,
    pub description_limit: usize,
    pub seo: bool,
    pub read: Permission,
    pub write: Permission,
}
#[derive(Debug, Error)]
pub enum ContentError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error("Invalid translations")]
    Invalid(Vec<(String, &'static str)>),
    #[error("Resource not found")]
    NotFound,
    #[error("Content conflicts with an existing name")]
    Conflict,
    #[error("Language configuration changed")]
    LanguageChanged,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for ContentError {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Conflict
        } else {
            Self::Technical(
                crate::technical::DatabaseFailure::capture(error, module_path!()).into(),
            )
        }
    }
}
impl Definition {
    pub fn validate(
        &self,
        rows: &[Translation],
        primary: &str,
    ) -> Result<Vec<Translation>, ContentError> {
        let mut normalized = rows.to_vec();
        let mut errors = Vec::new();
        if Locale::parse(primary).is_none()
            || rows.iter().filter(|r| r.locale == primary).count() != 1
            || rows.iter().any(|r| {
                Locale::parse(&r.locale).is_none()
                    || rows.iter().filter(|other| other.locale == r.locale).count() != 1
            })
        {
            return Err(ContentError::Invalid(vec![(String::new(), "locale")]));
        }
        for row in &mut normalized {
            for (key, text, max, multiline) in [
                ("name", &mut row.name, self.name_limit, false),
                (
                    "description",
                    &mut row.description,
                    self.description_limit,
                    self.description_limit > 500,
                ),
                (
                    "seo_title",
                    &mut row.seo_title,
                    if self.seo { 120 } else { 0 },
                    false,
                ),
                (
                    "seo_description",
                    &mut row.seo_description,
                    if self.seo { 320 } else { 0 },
                    false,
                ),
            ] {
                *text = text.trim().to_owned();
                if text.chars().count() > max
                    || text
                        .chars()
                        .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
                    || (key == "name" && row.locale == primary && text.is_empty())
                {
                    errors.push((row.locale.clone(), key));
                }
            }
        }
        if errors.is_empty() {
            if matches!(self.table, "catalog_products" | "information_pages") {
                for row in &mut normalized {
                    row.description = crate::rich_text::normalize(&row.description);
                    if row.description.chars().count() > self.description_limit {
                        return Err(ContentError::Invalid(vec![(
                            row.locale.clone(),
                            "description",
                        )]));
                    }
                }
            }
            Ok(normalized)
        } else {
            Err(ContentError::Invalid(errors))
        }
    }
    pub async fn prepare(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        draft: Option<&Draft>,
        id: i64,
        mut main: Translation,
    ) -> Result<Vec<Translation>, ContentError> {
        crate::database::commercial_lock::exclusive(tx).await?;
        let primary: String = sqlx::query_scalar(
            "SELECT primary_locale FROM storefront_settings WHERE id=1 FOR UPDATE",
        )
        .fetch_one(&mut **tx)
        .await?;
        let active: Vec<String> =
            sqlx::query_scalar("SELECT code FROM storefront_languages ORDER BY code")
                .fetch_all(&mut **tx)
                .await?;
        let mut rows = Vec::new();
        if let Some(draft) = draft {
            if primary != draft.primary
                || draft
                    .active
                    .as_ref()
                    .is_some_and(|expected| expected != &active)
            {
                return Err(ContentError::LanguageChanged);
            }
            if draft.rows.iter().any(|r| !active.contains(&r.locale)) {
                return Err(ContentError::Invalid(vec![(String::new(), "locale")]));
            }
            if id > 0 {
                rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "SELECT locale,name,description,seo_title,seo_description FROM {}_translations WHERE entity_id=$1 AND locale=ANY($2)", self.table)))
                    .bind(id).bind(&active).fetch_all(&mut **tx).await?;
            }
            draft.apply(&mut rows);
        }
        main.locale = primary.clone();
        rows.retain(|r| r.locale != primary);
        rows.push(main);
        self.validate(&rows, &primary)
    }
    /// The owner has authorized and written the resource in this same transaction.
    pub async fn persist(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: i64,
        rows: &[Translation],
    ) -> Result<(), ContentError> {
        for row in rows {
            let sql = if self.table == "catalog_option_values" {
                "INSERT INTO catalog_option_values_translations(entity_id,locale,name,description,seo_title,seo_description,option_id) SELECT id,$2,$3,$4,$5,$6,option_id FROM catalog_option_values WHERE id=$1 ON CONFLICT(entity_id,locale) DO UPDATE SET name=EXCLUDED.name,description=EXCLUDED.description,seo_title=EXCLUDED.seo_title,seo_description=EXCLUDED.seo_description".to_owned()
            } else {
                format!(
                    "INSERT INTO {}_translations(entity_id,locale,name,description,seo_title,seo_description) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(entity_id,locale) DO UPDATE SET name=EXCLUDED.name,description=EXCLUDED.description,seo_title=EXCLUDED.seo_title,seo_description=EXCLUDED.seo_description",
                    self.table
                )
            };
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(id)
                .bind(&row.locale)
                .bind(&row.name)
                .bind(&row.description)
                .bind(&row.seo_title)
                .bind(&row.seo_description)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }
    pub async fn load_many(
        &self,
        pool: &PgPool,
        identity: &Identity,
        session: &Session,
        ids: &[i64],
    ) -> Result<Vec<StoredTranslation>, ContentError> {
        identity.authorize(session, self.read).await?;
        if ids.len() > 100 {
            return Err(ContentError::Invalid(vec![]));
        }
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT entity_id::bigint,locale,name,description,seo_title,seo_description FROM {}_translations WHERE entity_id=ANY($1) ORDER BY entity_id,locale",self.table))).bind(ids).fetch_all(pool).await?)
    }
    /// Raw language values for editing: no fallback is materialized in empty secondary fields.
    pub async fn load(
        &self,
        pool: &PgPool,
        identity: &Identity,
        session: &Session,
        id: i64,
    ) -> Result<Vec<Translation>, ContentError> {
        identity.authorize(session, self.read).await?;
        let mut connection = pool.acquire().await?;
        self.load_on(&mut connection, id).await
    }
    /// Rechecks authorization and reads translations on the caller transaction.
    pub async fn load_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: &Session,
        id: i64,
    ) -> Result<Vec<Translation>, ContentError> {
        Identity::authorize_read_transaction(tx, session, self.read).await?;
        self.load_on(tx, id).await
    }
    async fn load_on(
        &self,
        connection: &mut sqlx::PgConnection,
        id: i64,
    ) -> Result<Vec<Translation>, ContentError> {
        let exists: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE id=$1)",
            self.table
        )))
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
        if !exists {
            return Err(ContentError::NotFound);
        }
        let codes: Vec<_> = Locale::SUPPORTED.iter().map(|(code, _)| *code).collect();
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT l.code AS locale,coalesce(t.name,'') AS name,coalesce(t.description,'') AS description,coalesce(t.seo_title,'') AS seo_title,coalesce(t.seo_description,'') AS seo_description FROM unnest($2::text[]) l(code) LEFT JOIN {}_translations t ON t.entity_id=$1 AND t.locale=l.code ORDER BY l.code",self.table))).bind(id).bind(codes).fetch_all(&mut *connection).await?)
    }
    /// All languages commit with the source fields. Settings changes are serialized with this write.
    pub async fn save(
        &self,
        pool: &PgPool,
        session: &Session,
        csrf: &str,
        id: i64,
        expected_primary: &str,
        rows: &[Translation],
    ) -> Result<(), ContentError> {
        session.check_csrf(csrf)?;
        session.require(self.write)?;
        let rows = self.validate(rows, expected_primary)?;
        let mut tx = pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, self.write).await?;
        // Same global ordering as commerce writes, before locking a content row/settings.
        crate::database::commercial_lock::exclusive(&mut tx).await?;
        let primary: String = sqlx::query_scalar(
            "SELECT primary_locale FROM storefront_settings WHERE id=1 FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if primary != expected_primary {
            return Err(ContentError::LanguageChanged);
        }
        let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT id::bigint FROM {} WHERE id=$1 FOR UPDATE",
            self.table
        )))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.is_none() {
            return Err(ContentError::NotFound);
        }
        // Keep the shared fallback snapshot in the same explicit transaction as translations.
        // Name uniqueness belongs to translation rows, never to this snapshot.
        let main = rows
            .iter()
            .find(|r| r.locale == primary)
            .ok_or_else(|| ContentError::Invalid(vec![(primary.clone(), "name")]))?;
        let mut assignments = vec![format!("{}=$2", self.name_column)];
        if let Some(column) = self.description_column {
            assignments.push(format!("{column}=$3"));
        }
        if self.seo {
            assignments.extend(["seo_title=$4".into(), "seo_description=$5".into()]);
        }
        // Bind all fields through a typed input CTE, including unused columns.
        let sql = format!(
            "WITH input AS (SELECT $1::bigint AS id,$2::text AS name,$3::text AS description,$4::text AS seo_title,$5::text AS seo_description) UPDATE {} SET {} FROM input WHERE {}.id=input.id",
            self.table,
            assignments.join(","),
            self.table
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(&main.name)
            .bind(&main.description)
            .bind(&main.seo_title)
            .bind(&main.seo_description)
            .execute(&mut *tx)
            .await?;
        self.persist(&mut tx, id, &rows).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Fixed, compiled language identities are the only SQL literals interpolated by owners.
#[derive(Clone, Copy, Debug)]
pub struct Language {
    pub current: Locale,
    pub primary: Locale,
}
impl Language {
    pub fn new(current: Locale, primary: Locale) -> Self {
        Self { current, primary }
    }
    pub fn expression(&self, field: &str, original: &str) -> String {
        self.aliased_expression(field, original, "ct", "cp")
    }
    pub fn joins(&self, table: &str, id: &str) -> String {
        self.aliased_joins(table, id, "ct", "cp")
    }
    /// SQL identifiers and expressions must be compiled owner definitions, never user input.
    /// Use the same aliases in the projection and joins for each translated entity.
    pub fn aliased_expression(
        &self,
        field: &str,
        original: &str,
        current: &str,
        primary: &str,
    ) -> String {
        if self.current == self.primary {
            format!("coalesce(nullif({current}.{field},''),{original})")
        } else {
            format!(
                "coalesce(nullif({current}.{field},''),nullif({primary}.{field},''),{original})"
            )
        }
    }
    /// Identifiers follow the same static-owner contract as `aliased_expression`.
    pub fn aliased_joins(&self, table: &str, id: &str, current: &str, primary: &str) -> String {
        let mut joins = format!(
            "LEFT JOIN {table}_translations {current} ON {current}.entity_id={id} AND {current}.locale='{}'",
            self.current.code()
        );
        if self.current != self.primary {
            joins.push_str(&format!(" LEFT JOIN {table}_translations {primary} ON {primary}.entity_id={id} AND {primary}.locale='{}'", self.primary.code()));
        }
        joins
    }
}

impl From<IdentityError> for ContentError {
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
    fn primary_translation_uses_one_join_and_keeps_original_fallback() {
        for locale in [Locale::ENGLISH, Locale::ITALIAN] {
            let language = Language::new(locale, locale);
            assert_eq!(
                language
                    .joins("catalog_products", "p.id")
                    .matches("LEFT JOIN")
                    .count(),
                1
            );
            assert_eq!(
                language.expression("name", "p.name"),
                "coalesce(nullif(ct.name,''),p.name)"
            );
        }
        let secondary = Language::new(Locale::ITALIAN, Locale::ENGLISH);
        assert_eq!(
            secondary
                .joins("catalog_products", "p.id")
                .matches("LEFT JOIN")
                .count(),
            2
        );
        assert_eq!(
            secondary.expression("name", "p.name"),
            "coalesce(nullif(ct.name,''),nullif(cp.name,''),p.name)"
        );
    }
    fn definition() -> Definition {
        Definition {
            table: "catalog_products",
            name_column: "name",
            description_column: Some("description"),
            name_limit: 120,
            description_limit: 4000,
            seo: true,
            read: Permission::ProductsRead,
            write: Permission::ProductsWrite,
        }
    }
    #[test]
    fn translations_validate_primary_unicode_limits_and_control_characters() {
        let rows = vec![
            Translation {
                locale: "it".into(),
                name: "  Magliette  ".into(),
                description: "Riga uno\nRiga due".into(),
                ..Translation::default()
            },
            Translation {
                locale: "en".into(),
                ..Translation::default()
            },
        ];
        let valid = definition()
            .validate(&rows, "it")
            .expect("secondary translations may be empty");
        assert_eq!(valid[0].name, "Magliette");
        assert!(
            matches!(definition().validate(&rows,"en"),Err(ContentError::Invalid(fields)) if fields.contains(&("en".into(),"name")))
        );
        for (field, text) in [
            ("name", "è".repeat(121)),
            ("description", "x".repeat(4001)),
            ("seo_title", "bad\ntext".into()),
            ("seo_description", "x".repeat(321)),
        ] {
            let mut invalid = rows.clone();
            match field {
                "name" => invalid[1].name = text,
                "description" => invalid[1].description = text,
                "seo_title" => invalid[1].seo_title = text,
                _ => invalid[1].seo_description = text,
            }
            assert!(
                matches!(definition().validate(&invalid,"it"),Err(ContentError::Invalid(fields)) if fields.contains(&("en".into(),field)))
            );
        }
        assert!(definition().validate(&rows[..1], "it").is_ok());
        let mut duplicate = rows.clone();
        duplicate[1].locale = "it".into();
        assert!(definition().validate(&duplicate, "it").is_err());
        duplicate[1].locale = "unknown".into();
        assert!(definition().validate(&duplicate, "it").is_err());
        let mut homepage = definition();
        homepage.description_limit = 500;
        assert!(homepage.validate(&rows, "it").is_err());
    }
}
